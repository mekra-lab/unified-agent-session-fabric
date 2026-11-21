use std::{io::{self,Read,Write},os::fd::AsRawFd,path::Path};
use crate::{config::{Binding,binding},error,now,sys,DEADLINE_NS};
/// A successful response is identical to the original worker bytes, without its trailing LF.
/// A completed spool avoids exposing internal errors as partial JSON. Transport failures after
/// output starts cannot retract bytes and must not append a second JSON document.
pub fn forward(b:&Binding,bytes:&[u8],started:u64,output:i32,newline:bool,remote:Option<(u32,u64)>)->io::Result<i32>{
 let deadline=started.saturating_add(DEADLINE_NS);if now()>=deadline{return Err(error("query_deadline_exceeded"));}
 let dir=sys::private(&b.bundle,true)?;let socket=format!("/proc/self/fd/{}/query.sock",dir.as_raw_fd());let stream=sys::connect(&socket,deadline)?;
 stream.set_nonblocking(true)?;let fd=stream.as_raw_fd();
 let request=uasf_json::json!({"route":b.route.id,"nonce":sys::random()?,"deadline":deadline,"request":String::from_utf8(bytes.to_vec()).map_err(|_|error("query_invalid_request"))?,"remote":remote.map(|(pid,start)|uasf_json::json!([pid,start]))});
 let encoded=uasf_json::to_vec(&request).map_err(|_|error("query_invalid_request"))?;
 sys::write_deadline(fd,&(encoded.len() as u32).to_be_bytes(),deadline)?;sys::write_deadline(fd,&encoded,deadline)?;
 receive(&b.bundle,&b.route.kind,fd,deadline,output,newline)
}
fn receive(bundle:&Path,kind:&str,fd:i32,deadline:u64,output:i32,newline:bool)->io::Result<i32>{
 let mut header=[0u8;crate::RESPONSE_HEADER_SIZE];sys::read_exact(fd,&mut header,deadline)?;
 let (length,exit,ack)=crate::parse_response_header(header)?;
 // Anonymous disk spool owned by this client; pending clients do not allocate response-sized RAM.
 let mut file=sys::anonymous(bundle)?;
 let mut left=length;let mut buffer=[0u8;16384];while left>0{let n=left.min(buffer.len() as u64) as usize;sys::read_exact(fd,&mut buffer[..n],deadline)?;file.write_all(&buffer[..n])?;left-=n as u64;}
 use std::io::{Seek,SeekFrom};file.seek(SeekFrom::Start(0))?;
 if kind=="gate" && exit==2 && length<8192 {
  let mut small=Vec::new();file.read_to_end(&mut small)?;
  if let Ok(v)=uasf_json::from_slice(&small){if let Some(reason)=v["reasons"].as_array().and_then(|a|a.first()).and_then(|v|v.as_str()).filter(|s|s.starts_with("query_")) {
   file.set_len(0)?;file.seek(SeekFrom::Start(0))?;file.write_all(&crate::refusal("gate",reason))?;
  }}file.seek(SeekFrom::Start(0))?;
 }

 let old=sys::nonblock(output)?;let result=(||{loop{let n=file.read(&mut buffer)?;if n==0{break;}sys::write_deadline(output,&buffer[..n],deadline)?;}if newline{sys::write_deadline(output,b"\n",deadline)?;}if ack{sys::write_deadline(fd,b"A",deadline)?;}Ok(exit)})();unsafe{libc::fcntl(output,libc::F_SETFL,old);}result.map_err(|_:io::Error|error("query_delivery_failed"))
}
pub fn core(index:&Path,persistent:&Path,input:&str,started:u64)->io::Result<Option<i32>>{
 let parent=persistent.parent().filter(|p|!p.as_os_str().is_empty()).unwrap_or(Path::new("."));let Some(b)=binding(parent)?else{return Ok(None)};
 if b.route.kind!="core" || canonical_file(Path::new(b.route.arg("--index").ok_or(error("binding"))?))?!=canonical_file(index)? || canonical_file(Path::new(b.route.arg("--persistent").ok_or(error("binding"))?))?!=canonical_file(persistent)? {return Err(error("query_binding_mismatch"));}
 if crate::config::worker(&b)?{return Ok(None);}
 let bytes=if input=="-"{sys::read_input(0,started+DEADLINE_NS)?}else{if input.len()>crate::REQUEST_LIMIT{return Err(error("query_request_limit"));}input.as_bytes().to_vec()};
 forward(&b,&bytes,started,1,true,None).map(Some)
}

fn canonical_file(path:&Path)->io::Result<std::path::PathBuf>{if path.exists(){path.canonicalize()}else{Ok(path.parent().unwrap_or(Path::new(".")).canonicalize()?.join(path.file_name().ok_or(error("path"))?))}}

#[cfg(test)]
mod tests {
 use super::*;
 use std::{fs,os::fd::FromRawFd};
 fn reply(reason:&str,ack:bool,kind:&str)->(io::Result<i32>,Vec<u8>) {
  let root=std::env::temp_dir().join(format!("queue-reply-{}-{}",std::process::id(),now()));fs::create_dir(&root).unwrap();
  let body=crate::refusal("core",reason);let mut pipe=[0;2];assert_eq!(unsafe{libc::pipe2(pipe.as_mut_ptr(),libc::O_CLOEXEC)},0);
  let reader=unsafe{fs::File::from_raw_fd(pipe[0])};let mut writer=unsafe{fs::File::from_raw_fd(pipe[1])};
  writer.write_all(&crate::response_header(body.len() as u64,2,ack)).unwrap();writer.write_all(&body).unwrap();drop(writer);
  sys::nonblock(reader.as_raw_fd()).unwrap();let mut output=sys::anonymous(&root).unwrap();
  let result=receive(&root,kind,reader.as_raw_fd(),now()+1_000_000_000,output.as_raw_fd(),true);
  use std::io::{Seek,SeekFrom};output.seek(SeekFrom::Start(0)).unwrap();let mut bytes=Vec::new();output.read_to_end(&mut bytes).unwrap();fs::remove_dir_all(root).unwrap();(result,bytes)
 }
 #[test] fn terminal_duplicate_is_delivered_after_peer_close_without_ack() {
  let (result,bytes)=reply("query_duplicate_request",false,"core");assert_eq!(result.unwrap(),2);
  let mut expected=crate::refusal("core","query_duplicate_request");expected.push(b'\n');assert_eq!(bytes,expected);
 }
 #[test] fn terminal_deadline_preserves_gate_reason_after_peer_close() {
  let (result,bytes)=reply("query_deadline_exceeded",false,"gate");assert_eq!(result.unwrap(),2);
  let mut expected=crate::refusal("gate","query_deadline_exceeded");expected.push(b'\n');assert_eq!(bytes,expected);
 }
 #[test] fn worker_ack_failure_is_still_delivery_failure() {
  let (result,bytes)=reply("query_worker_failed",true,"core");assert_eq!(result.unwrap_err().to_string(),"query_delivery_failed");
  let mut expected=crate::refusal("core","query_worker_failed");expected.push(b'\n');assert_eq!(bytes,expected);
 }
}
