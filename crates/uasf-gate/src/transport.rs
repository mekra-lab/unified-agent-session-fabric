//! EOF-framed JSON over a collector-owned Unix socket.
use std::{
    fs, io::{Read, Write},
    net::Shutdown,
    os::unix::{
        net::{UnixListener, UnixStream},
        fs::{MetadataExt, PermissionsExt},
    },
    path::Path, sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use uasf_json::Value;
use crate::{
    files, Bridge, Error, Gate, Result, REQUEST_LIMIT, RESPONSE_LIMIT,
    rejected,
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) { STOP.store(true, Ordering::SeqCst); }
pub fn receive(stream: impl Read, limit: u64) -> Result<Value> {
    let mut b = Vec::new();
    stream.take(limit + 1).read_to_end(&mut b)?;
    if b.len() as u64 > limit { return Err(Error); }
    Ok(uasf_json::from_slice(&b)?)
}
pub fn query(path: &Path, request: &Value) -> Result<Value> {
    let bytes = uasf_json::to_vec(request)?;
    if bytes.len() as u64 > REQUEST_LIMIT {
        return Ok(rejected("unsupported", "request exceeds limit"));
    }
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(300)))?;
    stream.set_write_timeout(Some(Duration::from_secs(300)))?;
    stream.write_all(&bytes)?;
    stream.shutdown(Shutdown::Write)?;
    receive(stream, RESPONSE_LIMIT)
}
pub fn socket_parent(path: &Path) -> Result<()> {
    let full =
        if path.is_absolute() {
            path.to_owned()
        } else { std::env::current_dir()?.join(path) };
    files::absolute(&full)?;
    files::trusted_parents(&full)?;
    let m = fs::symlink_metadata(full.parent().ok_or(Error)?)?;
    if !m.is_dir() || m.uid() != files::uid() || m.mode() & 0o022 != 0 ||
            fs::symlink_metadata(&full).is_ok() {
        return Err(Error);
    }
    Ok(())
}
struct SocketGuard<'a> {
    path: &'a Path,
    device: u64,
    inode: u64,
}
impl Drop for SocketGuard<'_> {
    fn drop(&mut self) {
        if fs::symlink_metadata(self.path).is_ok_and(|m|
                    m.dev() == self.device && m.ino() == self.inode) {
            let _ = fs::remove_file(self.path);
        }
    }
}
pub fn serve<B: Bridge>(gate: &Gate<B>, path: &Path, mode: u32)
    -> Result<()> {
    if ![0o600, 0o660].contains(&mode) { return Err(Error); }
    socket_parent(path)?;
    gate.config()?;
    let admission = uasf_core::admission::acquire_gate(&gate.staging).map_err(|_| Error)?;
    STOP.store(false, Ordering::SeqCst);
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
    }
    let previous = unsafe { libc::umask(0o177) };
    let server = UnixListener::bind(path);
    unsafe { libc::umask(previous); }
    let server = server?;
    let meta = fs::symlink_metadata(path)?;
    let _guard = SocketGuard { path, device: meta.dev(), inode: meta.ino() };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    server.set_nonblocking(true)?;
    while !STOP.load(Ordering::SeqCst) {
        let (mut stream, _) =
            match server.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted =>
                    continue,
                Err(_) => return Err(Error),
            };
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let response =
            match receive(&stream, REQUEST_LIMIT) {
                Ok(r) => gate.handle_admitted(&r, &admission),
                Err(_) =>
                    rejected("unsupported", "malformed or oversized request"),
            };
        let _ =
            stream.write_all(uasf_json::to_string(&response)?.as_bytes());
    }
    Ok(())
}

/// Opt-in queue frontend. The socket permission boundary stays in this process;
/// it never holds an execution slot or deletion lock while a request is waiting.
pub fn serve_scheduled(binding: &uasf_queue::config::Binding, path: &Path, mode: u32) -> Result<()> {
    use std::{os::fd::AsRawFd, sync::{Arc, atomic::AtomicUsize}};
    if ![0o600,0o660].contains(&mode) {return Err(Error);}
    socket_parent(path)?;STOP.store(false,Ordering::SeqCst);
    unsafe{libc::signal(libc::SIGTERM,stop as *const () as libc::sighandler_t);libc::signal(libc::SIGINT,stop as *const () as libc::sighandler_t);}
    let previous=unsafe{libc::umask(0o177)};let server=UnixListener::bind(path);unsafe{libc::umask(previous);}let server=server?;
    let meta=fs::symlink_metadata(path)?;let _guard=SocketGuard{path,device:meta.dev(),inode:meta.ino()};
    fs::set_permissions(path,fs::Permissions::from_mode(mode))?;server.set_nonblocking(true)?;
    let active=Arc::new(AtomicUsize::new(0));let mut threads=Vec::new();
    while !STOP.load(Ordering::SeqCst) {
        threads.retain_mut(|t:&mut Option<std::thread::JoinHandle<()>>|if t.as_ref().unwrap().is_finished(){let _=t.take().unwrap().join();false}else{true});
        match server.accept() {
            Ok((stream,_))=>{
                let started=uasf_queue::now();let deadline=started+uasf_queue::DEADLINE_NS;
                if active.load(Ordering::SeqCst)>=34 {
                    let _=uasf_queue::sys::nonblock(stream.as_raw_fd());
                    let _=uasf_queue::sys::write_deadline(stream.as_raw_fd(),&uasf_queue::refusal("gate","query_queue_full"),deadline);continue;
                }
                let binding=binding.clone();let active=active.clone();active.fetch_add(1,Ordering::SeqCst);
                threads.push(Some(std::thread::spawn(move||{
                    let fd=stream.as_raw_fd();let mut credential:libc::ucred=unsafe{std::mem::zeroed()};let mut len=std::mem::size_of_val(&credential) as libc::socklen_t;
                    let remote=if unsafe{libc::getsockopt(fd,libc::SOL_SOCKET,libc::SO_PEERCRED,(&mut credential as *mut libc::ucred).cast(),&mut len)}==0 {uasf_queue::sys::stat(credential.pid as u32).map(|(_,start)|(credential.pid as u32,start))}else{None};
                    let result=uasf_queue::sys::read_input(fd,deadline).and_then(|bytes|{
                        if uasf_json::from_slice(&bytes).is_err(){return Err(uasf_queue::error("malformed or oversized request"));}
                        uasf_queue::client::forward(&binding,&bytes,started,fd,false,remote)
                    });
                    if let Err(e)=result {if e.to_string()!="query_delivery_failed" {let _=uasf_queue::sys::nonblock(fd);let _=uasf_queue::sys::write_deadline(fd,&uasf_queue::refusal("gate",&if e.to_string()=="malformed or oversized request" {e.to_string()}else{uasf_queue::reason(&e)}),deadline);}}
                    active.fetch_sub(1,Ordering::SeqCst);
                })));
            },
            Err(e) if matches!(e.kind(),std::io::ErrorKind::WouldBlock|std::io::ErrorKind::Interrupted)=>std::thread::sleep(Duration::from_millis(2)),
            Err(_)=>return Err(Error),
        }
    }
    for t in threads {let _=t.unwrap().join();}Ok(())
}
