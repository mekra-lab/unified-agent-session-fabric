use std::{fs::{File, OpenOptions}, io::{self, Read, Write}, os::{fd::{AsRawFd, RawFd}, unix::fs::{MetadataExt, OpenOptionsExt}}, path::Path};
use crate::{error,now};
pub fn private(path: &Path, directory: bool) -> io::Result<File> {
    let f=OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC|if directory {libc::O_DIRECTORY}else{0}).open(path)?;
    let m=f.metadata()?;
    if m.uid()!=unsafe{libc::geteuid()} || m.is_dir()!=directory || m.mode()&0o777!=if directory{0o700}else{0o600} || (!directory && (!m.is_file() || m.nlink()!=1)) {return Err(error("query_untrusted_binding"));} Ok(f)
}
pub fn file(path:&Path)->io::Result<File>{OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path)}
pub fn lock(path:&Path)->io::Result<File>{
    let f=OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path)?;
    let m=f.metadata()?;if !m.is_file() || m.uid()!=unsafe{libc::geteuid()} || m.mode()&0o777!=0o600 || m.nlink()!=1{return Err(error("query_untrusted_lock"));}
    if unsafe{libc::flock(f.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}!=0{return Err(io::Error::last_os_error());}Ok(f)
}
pub fn nonblock(fd:RawFd)->io::Result<i32>{let old=unsafe{libc::fcntl(fd,libc::F_GETFL)};if old<0 || unsafe{libc::fcntl(fd,libc::F_SETFL,old|libc::O_NONBLOCK)}<0{return Err(io::Error::last_os_error());}Ok(old)}
pub fn poll(fd:RawFd,events:i16,deadline:u64)->io::Result<()> {
    loop {let left=deadline.saturating_sub(now());if left==0{return Err(error("query_deadline_exceeded"));}
    let mut p=libc::pollfd{fd,events,revents:0};let n=unsafe{libc::poll(&mut p,1,((left/1_000_000).min(50)+1) as i32)};
    if n<0 && io::Error::last_os_error().kind()!=io::ErrorKind::Interrupted{return Err(io::Error::last_os_error());}
    if n>0{return Ok(());}}
}
pub fn write_deadline(fd:RawFd,mut bytes:&[u8],deadline:u64)->io::Result<()> {
    while !bytes.is_empty(){poll(fd,libc::POLLOUT,deadline)?;let n=unsafe{libc::write(fd,bytes.as_ptr().cast(),bytes.len())};
    if n<0 {let e=io::Error::last_os_error();if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted){continue;}return Err(e);}if n==0{return Err(error("query_delivery_failed"));}bytes=&bytes[n as usize..];}Ok(())
}
pub fn read_exact(fd:RawFd,mut bytes:&mut [u8],deadline:u64)->io::Result<()> {
    while !bytes.is_empty(){poll(fd,libc::POLLIN,deadline)?;let n=unsafe{libc::read(fd,bytes.as_mut_ptr().cast(),bytes.len())};
    if n<0{let e=io::Error::last_os_error();if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted){continue;}return Err(e);}if n==0{return Err(error("query_service_unavailable"));}bytes=&mut bytes[n as usize..];}Ok(())
}
pub fn read_input(fd:RawFd,deadline:u64)->io::Result<Vec<u8>>{
    let old=nonblock(fd)?;let result=(||{let mut out=Vec::new();let mut b=[0u8;8192];loop{poll(fd,libc::POLLIN,deadline)?;let n=unsafe{libc::read(fd,b.as_mut_ptr().cast(),b.len())};if n==0{return Ok(out);}if n<0{let e=io::Error::last_os_error();if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted){continue;}return Err(e);}out.extend_from_slice(&b[..n as usize]);if out.len()>crate::REQUEST_LIMIT{return Err(error("query_request_limit"));}}})();unsafe{libc::fcntl(fd,libc::F_SETFL,old);}result
}
pub fn stat(pid:u32)->Option<(u32,u64)>{let s=std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;let fields:Vec<_>=s[s.rfind(')')?+2..].split_whitespace().collect();Some((fields.get(1)?.parse().ok()?,fields.get(19)?.parse().ok()?))}
pub fn rss(pid:u32)->u64{std::fs::read_to_string(format!("/proc/{pid}/status")).ok().and_then(|s|s.lines().find(|l|l.starts_with("VmRSS:")).and_then(|l|l.split_whitespace().nth(1)).and_then(|v|v.parse::<u64>().ok())).unwrap_or(0)*1024}
pub fn processes()->Vec<(u32,u32,u64)>{std::fs::read_dir("/proc").into_iter().flatten().filter_map(|e|e.ok()?.file_name().to_str()?.parse().ok()).filter_map(|p|stat(p).map(|(parent,start)|(p,parent,start))).collect()}
pub fn descendants(root:u32)->Vec<u32>{let all=processes();let mut found=vec![root];let mut i=0;while i<found.len(){let p=found[i];for (id,parent,_) in &all {if *parent==p && !found.contains(id){found.push(*id);}}i+=1;}found}
pub fn random()->io::Result<String>{let mut b=[0u8;16];File::open("/dev/urandom")?.read_exact(&mut b)?;Ok(b.iter().map(|v|format!("{v:02x}")).collect())}
pub fn json_file(path:&Path,v:&uasf_json::Value)->io::Result<()> {let mut f=file(path)?;f.write_all(&uasf_json::to_vec(v).map_err(|_|error("json"))?)?;f.sync_all()}

pub fn anonymous(root:&Path)->io::Result<File>{use std::os::{unix::ffi::OsStrExt,fd::FromRawFd};let path=std::ffi::CString::new(root.as_os_str().as_bytes()).map_err(|_|error("spool"))?;let fd=unsafe{libc::open(path.as_ptr(),libc::O_TMPFILE|libc::O_RDWR|libc::O_CLOEXEC,0o600)};if fd<0{return Err(io::Error::last_os_error());}Ok(unsafe{File::from_raw_fd(fd)})}
pub fn connect(path:&str,deadline:u64)->io::Result<std::os::unix::net::UnixStream>{use std::os::fd::FromRawFd;
 let mut address:libc::sockaddr_un=unsafe{std::mem::zeroed()};address.sun_family=libc::AF_UNIX as _;if path.len()>=address.sun_path.len(){return Err(error("socket path"));}for (a,b) in address.sun_path.iter_mut().zip(path.bytes()){*a=b as _;}
 let fd=unsafe{libc::socket(libc::AF_UNIX,libc::SOCK_STREAM|libc::SOCK_NONBLOCK|libc::SOCK_CLOEXEC,0)};if fd<0{return Err(io::Error::last_os_error());}let socket=unsafe{std::os::unix::net::UnixStream::from_raw_fd(fd)};
 if unsafe{libc::connect(fd,(&address as *const libc::sockaddr_un).cast(),std::mem::size_of_val(&address) as _)}<0{let e=io::Error::last_os_error();if !matches!(e.raw_os_error(),Some(libc::EINPROGRESS|libc::EAGAIN)){return Err(e);}poll(fd,libc::POLLOUT,deadline)?;let mut code=0i32;let mut len=std::mem::size_of_val(&code) as libc::socklen_t;if unsafe{libc::getsockopt(fd,libc::SOL_SOCKET,libc::SO_ERROR,(&mut code as *mut i32).cast(),&mut len)}<0{return Err(io::Error::last_os_error());}if code!=0{return Err(io::Error::from_raw_os_error(code));}}Ok(socket)
}
