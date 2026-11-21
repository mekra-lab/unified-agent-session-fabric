//! One guardian owns the slot through execution, delivery, and descendant reap.
use std::{fs,io::{self,Read,Write},os::{fd::FromRawFd,unix::process::CommandExt},process::{Command,Stdio},time::Duration};
use crate::{error,sys,now};
pub fn run(lease:i32,jobfd:i32,inputfd:i32,outputfd:i32)->io::Result<()> {
 let mut jobfile=unsafe{fs::File::from_raw_fd(jobfd)};let input=unsafe{fs::File::from_raw_fd(inputfd)};let output=unsafe{fs::File::from_raw_fd(outputfd)};for fd in [jobfd,inputfd,outputfd]{unsafe{libc::fcntl(fd,libc::F_SETFD,libc::FD_CLOEXEC);}}
 let slot=unsafe{fs::File::from_raw_fd(lease)};unsafe{libc::fcntl(lease,libc::F_SETFD,libc::FD_CLOEXEC);libc::prctl(libc::PR_SET_CHILD_SUBREAPER,1,0,0,0);}
 let mut raw=String::new();jobfile.read_to_string(&mut raw)?;let job=uasf_json::from_str(&raw).map_err(|_|error("job"))?;
 let route:crate::config::Route=uasf_json::from_value(job["route"].clone()).map_err(|_|error("route"))?;let deadline=job["deadline"].as_u64().ok_or(error("deadline"))?;
 let mut completed_output=output.try_clone()?;
 let mut command=Command::new(&route.binary);if route.kind=="gate"{command.arg("check-local");}command.args(&route.args);if route.kind=="core"{command.arg("query");}
 command.env_clear().env("PATH","/usr/bin:/bin").env("LANG","C.UTF-8").env("TMPDIR",job["bundle"].as_str().ok_or(error("bundle"))?).env("UASF_QUEUE_LEASE_FD",lease.to_string()).stdin(input).stdout(output).stderr(Stdio::null());
 let guardian=std::process::id();unsafe{command.pre_exec(move||{
  if libc::setpgid(0,0)!=0{return Err(io::Error::last_os_error());}
  libc::prctl(libc::PR_SET_PDEATHSIG,libc::SIGKILL,0,0,0);if libc::getppid()!=guardian as i32{libc::_exit(125);}
  libc::fcntl(lease,libc::F_SETFD,0);
  let limit=libc::rlimit{rlim_cur:crate::RESPONSE_LIMIT+1,rlim_max:crate::RESPONSE_LIMIT+1};if libc::setrlimit(libc::RLIMIT_FSIZE,&limit)!=0{return Err(io::Error::last_os_error());}Ok(())
 });}
 if now()>=deadline{return Err(error("query_deadline_exceeded"));}
 let mut child=command.spawn()?;let group=child.id() as i32;sys::nonblock(0)?;
 let mut completed=false;let mut acknowledged=false;
 while now()<deadline {
  let mut control=[0u8;1];let n=unsafe{libc::read(0,control.as_mut_ptr().cast(),1)};
  if n==0{break;}if n>0{acknowledged=control[0]==b'A';break;}
  if !completed {if let Some(status)=child.try_wait()? {
   // Even a successful parent is not finished while its descendants remain.
   cleanup(group)?;
   let code=status.code().unwrap_or(125);let f=&mut completed_output;
   let length=f.metadata()?.len();if length>0{use std::io::{Seek,SeekFrom};f.seek(SeekFrom::End(-1))?;let mut last=[0];f.read_exact(&mut last)?;if last[0]==b'\n'{f.set_len(length-1)?;}}
   let code=if code==2 {use std::io::{Seek,SeekFrom};if f.metadata()?.len()<=crate::REQUEST_LIMIT as u64 {f.seek(SeekFrom::Start(0))?;let mut bytes=Vec::new();f.read_to_end(&mut bytes)?;if uasf_json::from_slice(&bytes).is_ok(){2}else{125}}else{125}}else{code};
   writeln!(io::stdout(),"{code}")?;io::stdout().flush()?;completed=true;
  }}
  std::thread::sleep(Duration::from_millis(2));
 }
 unsafe{libc::kill(-group,libc::SIGKILL);}let _=child.wait();cleanup(group)?;drop(slot);
 if completed&&acknowledged{Ok(())}else{Err(error("query_cancelled_or_deadline"))}
}
fn cleanup(group:i32)->io::Result<()> {
 unsafe{libc::kill(-group,libc::SIGKILL);}
 loop {
  // Escaped process-group children are adopted by this subreaper when their parents exit.
  for (pid,parent,_) in sys::processes(){if parent==std::process::id(){unsafe{libc::kill(pid as i32,libc::SIGKILL);}}}
  let mut status=0;let n=unsafe{libc::waitpid(-1,&mut status,libc::WNOHANG)};
  if n<0 {let e=io::Error::last_os_error();if e.raw_os_error()==Some(libc::ECHILD){return Ok(());}if e.kind()!=io::ErrorKind::Interrupted{return Err(e);}}
  if n==0{std::thread::sleep(Duration::from_millis(2));}
 }
}
