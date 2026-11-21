use std::{collections::{BTreeMap,BTreeSet},fs::{self,File},io::{self,Read,Write},os::{fd::AsRawFd,unix::{net::{UnixListener,UnixStream},process::CommandExt,fs::MetadataExt}},process::{Child,Command,Stdio},sync::atomic::{AtomicBool,Ordering},time::Duration};
use crate::{config::{Config,Route},model::{Queue,Ticket,WAITING,SLOTS},sys,error,now,DEADLINE_NS,REQUEST_LIMIT};
static STOP:AtomicBool=AtomicBool::new(false);
extern "C" fn stop(_:i32){STOP.store(true,Ordering::SeqCst);}
struct Peer {socket:UnixStream, pid:u32,start:u64, deadline:u64, input:Vec<u8>, route:Option<Route>, request:Vec<u8>, source:String, nonce:String, owns_identity:bool, remote:Option<(u32,u64)>, admitted:bool, job:Option<Job>, send:Vec<u8>, offset:usize, body:Option<File>, remaining:u64, ack:bool, close:bool}
struct Job {child:Child,output:File,status:Vec<u8>,completed:bool}
impl Peer {
 fn reject(&mut self,reason:&str){if self.offset>0 || self.body.is_some(){self.close=true;return;}let bytes=crate::refusal(self.route.as_ref().map(|r|r.kind.as_str()).unwrap_or("core"),reason);self.send=crate::response_header(bytes.len() as u64,2,self.job.is_some()).to_vec();self.send.extend_from_slice(&bytes);self.offset=0;self.remaining=0;self.ack=false;}
 fn cancelled(&mut self){if let Some(j)=&mut self.job{j.child.stdin.take();}self.close=true;}
}
fn credentials(socket:&UnixStream)->io::Result<(u32,u64)>{let mut c:libc::ucred=unsafe{std::mem::zeroed()};let mut len=std::mem::size_of_val(&c) as libc::socklen_t;if unsafe{libc::getsockopt(socket.as_raw_fd(),libc::SOL_SOCKET,libc::SO_PEERCRED,(&mut c as *mut libc::ucred).cast(),&mut len)}!=0 || c.uid!=unsafe{libc::geteuid()}{return Err(error("query_peer_denied"));}let p=c.pid as u32;Ok((p,sys::stat(p).ok_or(error("query_peer_gone"))?.1))}
fn ready(route:&Route,_request:&[u8])->bool {
 if route.kind=="gate" {return true;} // Policy/configuration errors belong to the worker response.
 let Some(index)=route.arg("--index")else{return false};
 if !std::path::Path::new(index).exists(){return true;} // Preserve index_requires_rebuild.
 let Ok(db)=rusqlite::Connection::open_with_flags(index,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY|rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)else{return false};
 let _=db.busy_timeout(Duration::ZERO);
 // This is an availability hint, not an access/index-completeness verdict.
 // Preserve unknown boundaries and incomplete projections for the real worker.
 db.query_row("PRAGMA schema_version",[],|r|r.get::<_,i64>(0)).is_ok()
}

fn start(peer:&mut Peer,slot:File,config:&Config,_id:u64)->io::Result<()> {
 let route=peer.route.as_ref().unwrap();let mut job=sys::anonymous(&config.bundle)?;let mut input=sys::anonymous(&config.bundle)?;let output=sys::anonymous(&config.bundle)?;
 job.write_all(&uasf_json::to_vec(&uasf_json::json!({"route":uasf_json::to_value(route).unwrap(),"deadline":peer.deadline,"bundle":config.bundle})).unwrap())?;
 input.write_all(&peer.request)?;use std::io::{Seek,SeekFrom};job.seek(SeekFrom::Start(0))?;input.seek(SeekFrom::Start(0))?;
 let fds=[slot.as_raw_fd(),job.as_raw_fd(),input.as_raw_fd(),output.as_raw_fd()];let mut command=Command::new(std::env::current_exe()?);
 command.arg("__guardian").args(fds.iter().map(i32::to_string)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
 unsafe{command.pre_exec(move||{for fd in fds{if libc::fcntl(fd,libc::F_SETFD,0)<0{return Err(io::Error::last_os_error());}}Ok(())});}
 let child=command.spawn()?;sys::nonblock(child.stdout.as_ref().unwrap().as_raw_fd())?;
 // The shared open-file description is now held by the guardian, not the service.
 drop(slot);peer.input.clear();peer.request.clear();peer.job=Some(Job{child,output,status:Vec::new(),completed:false});Ok(())
}
fn reap(peer:&mut Peer)->io::Result<bool>{
 if let Some(j)=&mut peer.job {if let Some(status)=j.child.try_wait()? {
  if std::os::unix::process::ExitStatusExt::signal(&status).is_some(){return Err(error("query_guardian_lost"));}
  peer.job=None;return Ok(true);
 }}Ok(false)
}
pub fn serve(config:Config)->io::Result<()> {
 config.validate()?;config.check_bindings()?;
 let _single=sys::lock(&config.bundle.join("service.lock"))?;
 let dir=sys::private(&config.bundle,true)?;let path=config.bundle.join("query.sock");
 if path.exists(){let m=fs::symlink_metadata(&path)?;use std::os::unix::fs::FileTypeExt;if !m.file_type().is_socket() || m.uid()!=unsafe{libc::geteuid()}{return Err(error("query_untrusted_socket"));}fs::remove_file(&path)?;}
 STOP.store(false,Ordering::SeqCst);unsafe{libc::prctl(libc::PR_SET_CHILD_SUBREAPER,1,0,0,0);libc::signal(libc::SIGTERM,stop as *const () as usize);libc::signal(libc::SIGINT,stop as *const () as usize);}
 let old=unsafe{libc::umask(0o177)};let server=UnixListener::bind(format!("/proc/self/fd/{}/query.sock",dir.as_raw_fd()));unsafe{libc::umask(old);}let server=server?;server.set_nonblocking(true)?;
 let mut peers=BTreeMap::<u64,Peer>::new();let mut queue=Queue::default();let mut next=0u64;let mut identities=BTreeSet::new();let mut last_probe=0;let mut peak=0;
 let result=(||->io::Result<()> {loop{
  let mut progressed=false;let tick=now();let stopping=STOP.load(Ordering::SeqCst);
  if !stopping {for _ in 0..64 {let (mut socket,_)=match server.accept(){Ok(v)=>v,Err(e)if e.kind()==io::ErrorKind::WouldBlock=>break,Err(e)=>return Err(e)};socket.set_nonblocking(true)?;
   progressed=true;let Ok((pid,start))=credentials(&socket)else{continue};
   if peers.len().saturating_sub(queue.active())>=WAITING {let b=crate::refusal("core","query_queue_full");let mut wire=crate::response_header(b.len() as u64,2,false).to_vec();wire.extend(b);let _=socket.write(&wire);continue;}
   next+=1;peers.insert(next,Peer{socket,pid,start,deadline:tick+DEADLINE_NS,input:Vec::new(),route:None,request:Vec::new(),source:String::new(),nonce:String::new(),owns_identity:false,remote:None,admitted:false,job:None,send:Vec::new(),offset:0,body:None,remaining:0,ack:false,close:false});
  }}
  for (&id,p) in &mut peers {
   if stopping || tick>=p.deadline {queue.cancel_waiting(id);if p.job.is_some(){p.cancelled();}else{p.reject(if stopping{"query_service_stopping"}else{"query_deadline_exceeded"});p.close=true;}}
   if !p.admitted && p.send.is_empty() && !p.close {
    let mut b=[0u8;16384];match p.socket.read(&mut b){Ok(0)=>{p.close=true;},Ok(n)=>{progressed|=n>0;p.input.extend_from_slice(&b[..n])},Err(e)if e.kind()==io::ErrorKind::WouldBlock=>{},Err(_)=>p.close=true}
    if p.input.len()>=4 {let length=u32::from_be_bytes(p.input[..4].try_into().unwrap()) as usize;
     if length>REQUEST_LIMIT*6+4096 {p.reject("query_request_limit");}
     else if p.input.len()>=length+4 {
      let parsed=(||->io::Result<()> {let v=uasf_json::from_slice(&p.input[4..4+length]).map_err(|_|error("query_invalid_request"))?;
       let route=config.routes.iter().find(|r|Some(r.id.as_str())==v["route"].as_str()).ok_or(error("query_route_denied"))?.clone();
       p.deadline=p.deadline.min(v["deadline"].as_u64().ok_or(error("query_invalid_request"))?);p.nonce=v["nonce"].as_str().filter(|n|n.len()==32&&n.bytes().all(|b|b.is_ascii_hexdigit())).ok_or(error("query_invalid_request"))?.into();
       if !identities.insert((p.pid,p.start,p.nonce.clone())){return Err(error("query_duplicate_request"));}p.owns_identity=true;
       p.request=v["request"].as_str().ok_or(error("query_invalid_request"))?.as_bytes().to_vec();if p.request.len()>REQUEST_LIMIT{return Err(error("query_request_limit"));}
       let request=uasf_json::from_slice(&p.request).unwrap_or(uasf_json::Value::Null);
       p.source=if route.kind=="gate"{route.arg("--source").unwrap_or("gate")}else{request["source"].as_str().unwrap_or("@catalog")}.to_owned();
       if route.kind=="gate" {if let Some(a)=v["remote"].as_array(){if a.len()==2{if let (Some(pid),Some(start))=(a[0].as_u64(),a[1].as_u64()){if pid<=u32::MAX as u64 && sys::stat(pid as u32).is_some_and(|(_,s)|s==start){p.remote=Some((pid as u32,start));}}}}}
       let available=ready(&route,&p.request);p.route=Some(route);p.input.clear();
       queue.submit(Ticket{id,source:p.source.clone(),deadline:p.deadline,ready:available}).map_err(error)?;p.admitted=true;Ok(())})();
      if let Err(e)=parsed {p.reject(&e.to_string());}
     }
    }
   }
   if p.admitted || !p.send.is_empty() {
    let mut b=[0u8;1];match p.socket.read(&mut b){Ok(0)=>p.cancelled(),Ok(_)=>{if p.ack {if b[0]==b'A'{if let Some(j)=&mut p.job{if let Some(mut control)=j.child.stdin.take(){let _=control.write_all(b"A");}}}p.close=true;}else{p.cancelled();}},Err(e)if e.kind()==io::ErrorKind::WouldBlock=>{},Err(_)=>p.cancelled()}
   }
   if let Some(j)=&mut p.job {
    if !j.completed && !p.close {let mut b=[0u8;64];match j.child.stdout.as_mut().unwrap().read(&mut b){Ok(n)=>{progressed|=n>0;j.status.extend_from_slice(&b[..n])},Err(e)if e.kind()==io::ErrorKind::WouldBlock=>{},Err(e)=>return Err(e)}
     if j.status.contains(&b'\n') {let code=std::str::from_utf8(&j.status).ok().and_then(|s|s.trim().parse::<i32>().ok()).unwrap_or(125);let mut file=j.output.try_clone()?;use std::io::{Seek,SeekFrom};file.seek(SeekFrom::Start(0))?;let len=file.metadata()?.len();
      if ![0,2].contains(&code) || len==0 || len>crate::RESPONSE_LIMIT {p.reject("query_worker_failed");}
      else {p.send=crate::response_header(len,code,true).to_vec();p.body=Some(file);p.remaining=len;p.offset=0;}
      if let Some(j)=&mut p.job{j.completed=true;}
     }
    }
   }
   if !p.send.is_empty() && (tick<p.deadline || p.job.is_none()) {
    match p.socket.write(&p.send[p.offset..]){Ok(n)=>{progressed|=n>0;p.offset+=n},Err(e)if e.kind()==io::ErrorKind::WouldBlock=>{},Err(_)=>p.cancelled()}
    if p.offset==p.send.len(){p.send.clear();p.offset=0;if let Some(body)=&mut p.body {if p.remaining>0{let mut bytes=vec![0u8;(p.remaining.min(16384)) as usize];body.read_exact(&mut bytes)?;p.remaining-=bytes.len() as u64;p.send=bytes;}else{p.body=None;p.ack=true;}}else if p.job.is_some(){p.ack=true;}else{p.close=true;}}
   }
   if reap(p)?{queue.reaped(id);if !p.ack && !p.close && p.send.is_empty(){p.reject("query_worker_failed");}else{p.close=true;}}
  }
  let removed:Vec<_>=peers.iter().filter(|(_,p)|p.close&&p.job.is_none()).map(|(id,_)|*id).collect();for id in removed{let p=peers.remove(&id).unwrap();queue.cancel_waiting(id);if p.owns_identity{identities.remove(&(p.pid,p.start,p.nonce));}}
  if tick.saturating_sub(last_probe)>=50_000_000 {
   last_probe=tick;
   for (&id,p) in &peers{if p.admitted&&p.job.is_none(){queue.ready(id,ready(p.route.as_ref().unwrap(),&p.request));}}
   let mut pids:BTreeSet<_>=sys::descendants(std::process::id()).into_iter().collect();for p in peers.values(){if sys::stat(p.pid).is_some_and(|(_,s)|s==p.start){pids.insert(p.pid);}if let Some((pid,start))=p.remote{if sys::stat(pid).is_some_and(|(_,s)|s==start){pids.insert(pid);}}}
   let rss=pids.into_iter().map(sys::rss).sum::<u64>();peak=peak.max(rss);
   if rss>crate::RSS_LIMIT{for (&id,p) in &mut peers{queue.cancel_waiting(id);if p.job.is_some(){p.cancelled();}else{p.reject("query_memory_limit");}}}
  }
  if !stopping {for slot in 0..SLOTS {if queue.active()>=SLOTS{break;}if let Ok(lease)=sys::lock(&config.bundle.join(format!("slot-{slot}"))){if let Some(ticket)=queue.dispatch(tick){let p=peers.get_mut(&ticket.id).unwrap();if start(p,lease,&config,ticket.id).is_err(){queue.reaped(ticket.id);p.reject("query_worker_unavailable");}}}}}
  if stopping&&peers.is_empty(){break;}
  if !progressed {std::thread::sleep(Duration::from_millis(2));}
 }Ok(())})();
 // A lost guardian is a fail-closed service failure. Reap all adopted descendants before exit.
 for p in peers.values_mut(){p.cancelled();}
 for p in peers.values_mut(){if let Some(j)=&mut p.job{let _=j.child.wait();}}
 loop {let children:Vec<_>=sys::processes().into_iter().filter(|(_,parent,_)|*parent==std::process::id()).collect();if children.is_empty(){break;}for (pid,_,_) in children{unsafe{libc::kill(pid as i32,libc::SIGKILL);libc::waitpid(pid as i32,std::ptr::null_mut(),0);}}}
 let _=fs::remove_file(&path);eprintln!("queue_peak_rss_bytes={peak}");result
}

#[cfg(test)]
mod tests {
 use super::*;
 #[test]
 fn readiness_preserves_missing_index_response_and_isolates_a_busy_index() {
  let root=std::env::temp_dir().join(format!("queue-ready-{}-{}",std::process::id(),crate::now()));fs::create_dir(&root).unwrap();
  let index=root.join("index.sqlite");let route=Route{id:"core".into(),kind:"core".into(),binary:std::env::current_exe().unwrap(),root:root.clone(),args:vec!["--index".into(),index.to_str().unwrap().into(),"--persistent".into(),root.join("persistent.sqlite").to_str().unwrap().into()]};
  assert!(ready(&route,b"{}"));
  let db=rusqlite::Connection::open(&index).unwrap();db.execute_batch("CREATE TABLE metadata(id TEXT); BEGIN EXCLUSIVE").unwrap();assert!(!ready(&route,b"{}"));db.execute_batch("ROLLBACK").unwrap();assert!(ready(&route,b"{}"));drop(db);fs::remove_dir_all(root).unwrap();
 }
}
