use std::{fs,io::{self,Read},os::{fd::{AsRawFd,FromRawFd},unix::fs::MetadataExt},path::{Path,PathBuf}};
use serde::{Serialize,Deserialize};
use sha2::{Digest,Sha256};
use crate::{error,sys};
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {pub id:String,pub kind:String,pub binary:PathBuf,pub root:PathBuf,pub args:Vec<String>}
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {pub version:u32,pub bundle:PathBuf,pub routes:Vec<Route>}
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {pub version:u32,pub bundle:PathBuf,pub device:u64,pub inode:u64,pub route:Route,pub config_sha256:String}
fn decode<T:uasf_json::FromValue>(mut f:fs::File)->io::Result<T>{let mut b=Vec::new();f.by_ref().take(crate::REQUEST_LIMIT as u64+1).read_to_end(&mut b)?;if b.len()>crate::REQUEST_LIMIT{return Err(error("query_untrusted_binding"));}uasf_json::from_slice(&b).and_then(uasf_json::from_value).map_err(|_|error("query_untrusted_binding"))}
pub fn load(path:&Path)->io::Result<Config>{let c:Config=decode(sys::private(path,false)?)?;c.validate()?;Ok(c)}
impl Config {
 pub fn validate(&self)->io::Result<()> {
  sys::private(&self.bundle,true)?;
  if self.version!=1 || self.bundle.canonicalize()?!=self.bundle || self.routes.is_empty(){return Err(error("query_invalid_configuration"));}
  let mut roots=std::collections::BTreeSet::new();let mut ids=std::collections::BTreeSet::new();
  for r in &self.routes {
   let m=fs::metadata(&r.root)?;
   if r.id.is_empty() || !ids.insert(&r.id) || !roots.insert((m.dev(),m.ino())) || r.root.canonicalize()?!=r.root || m.uid()!=unsafe{libc::geteuid()} || !m.is_dir() || m.mode()&0o022!=0 || r.binary.canonicalize()?!=r.binary{return Err(error("query_invalid_configuration"));}
   let keys:Vec<&str>=if r.kind=="core" {vec!["--index","--persistent"]}else if r.kind=="gate" {vec!["--bridge","--registry","--sources","--staging","--source"]}else{return Err(error("query_invalid_configuration"));};
   if r.args.len()!=keys.len()*2 || r.args.chunks(2).any(|p|!keys.contains(&p[0].as_str())) || keys.iter().any(|k|r.args.chunks(2).filter(|p|p[0]==*k).count()!=1){return Err(error("query_invalid_configuration"));}
   let anchor=r.arg(if r.kind=="core"{"--persistent"}else{"--staging"}).unwrap();let anchor=Path::new(anchor);let root=if r.kind=="core"{anchor.parent().ok_or(error("root"))?}else{anchor};if root.canonicalize()?!=r.root{return Err(error("query_invalid_configuration"));}
  }Ok(())
 }
 pub fn sha(&self)->String{format!("{:x}",Sha256::digest(uasf_json::to_vec(&uasf_json::to_value(self).unwrap()).unwrap()))}
 pub fn bind(&self)->io::Result<()> {
  self.validate()?;let _lock=sys::lock(&self.bundle.join("service.lock"))?;let meta=fs::metadata(&self.bundle)?;
  // Validate every existing binding before installing any; never replace a different owner.
  for r in &self.routes {if let Some(b)=binding(&r.root)?{if b.config_sha256!=self.sha() || b.bundle!=self.bundle || b.route.id!=r.id{return Err(error("query_already_bound"));}}}
  for r in &self.routes{let path=r.root.join(".uasf-queue.json");if path.exists(){continue;}let b=Binding{version:1,bundle:self.bundle.clone(),device:meta.dev(),inode:meta.ino(),route:r.clone(),config_sha256:self.sha()};sys::json_file(&path,&uasf_json::to_value(&b).unwrap())?;fs::File::open(&r.root)?.sync_all()?;}Ok(())
 }
 pub fn unbind(&self)->io::Result<()> {
  let _single=sys::lock(&self.bundle.join("service.lock"))?;
  let _slots=[sys::lock(&self.bundle.join("slot-0"))?,sys::lock(&self.bundle.join("slot-1"))?];
  for r in &self.routes{if let Some(b)=binding(&r.root)?{if b.config_sha256!=self.sha(){return Err(error("query_binding_mismatch"));}}}
  for r in &self.routes{let path=r.root.join(".uasf-queue.json");if path.exists(){fs::remove_file(path)?;fs::File::open(&r.root)?.sync_all()?;}}Ok(())
 }
 pub fn check_bindings(&self)->io::Result<()> {for r in &self.routes{let b=binding(&r.root)?.ok_or(error("query_binding_missing"))?;if b.config_sha256!=self.sha() || b.route.id!=r.id{return Err(error("query_binding_mismatch"));}}Ok(())}
}
impl Route {pub fn arg(&self,key:&str)->Option<&str>{self.args.chunks(2).find(|p|p[0]==key).map(|p|p[1].as_str())}}
pub fn binding(root:&Path)->io::Result<Option<Binding>> {
 let path=root.join(".uasf-queue.json");if !path.try_exists()? {return Ok(None);}
 let b:Binding=decode(sys::private(&path,false)?)?;let f=sys::private(&b.bundle,true)?;let m=f.metadata()?;
 if b.version!=1 || m.dev()!=b.device || m.ino()!=b.inode || b.route.root!=root.canonicalize()?{return Err(error("query_binding_mismatch"));}Ok(Some(b))
}
/// The guardian deliberately passes this one locked descriptor; descendants must not inherit it.
pub fn worker(b:&Binding)->io::Result<bool>{
 let Some(value)=std::env::var_os("UASF_QUEUE_LEASE_FD") else{return Ok(false)};
 let fd:i32=value.to_str().and_then(|v|v.parse().ok()).filter(|v|*v>2).ok_or(error("query_worker_identity"))?;
 let duplicate=unsafe{libc::fcntl(fd,libc::F_DUPFD_CLOEXEC,3)};if duplicate<0{return Err(error("query_worker_identity"));}
 let f=unsafe{fs::File::from_raw_fd(duplicate)};let meta=f.metadata()?;
 let matches=(0..2).any(|n|fs::metadata(b.bundle.join(format!("slot-{n}"))).is_ok_and(|m|m.dev()==meta.dev()&&m.ino()==meta.ino()));
 if !matches || unsafe{libc::flock(f.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}!=0{return Err(error("query_worker_identity"));}
 unsafe{libc::fcntl(fd,libc::F_SETFD,libc::FD_CLOEXEC);}Ok(true)
}

fn fields(v:&uasf_json::Value,names:&[&str])->uasf_json::Result<()> {
 if v.as_object().is_none_or(|o|o.len()!=names.len() || names.iter().any(|k|!o.contains_key(*k))){return Err(<uasf_json::Error as serde::de::Error>::custom("configuration fields"));}Ok(())
}
fn text(v:&uasf_json::Value,key:&str)->uasf_json::Result<String>{uasf_json::from_value(v[key].clone())}
impl uasf_json::FromValue for Route {fn from_value(v:uasf_json::Value)->uasf_json::Result<Self>{fields(&v,&["id","kind","binary","root","args"])?;Ok(Self{id:text(&v,"id")?,kind:text(&v,"kind")?,binary:PathBuf::from(text(&v,"binary")?),root:PathBuf::from(text(&v,"root")?),args:uasf_json::from_value(v["args"].clone())?})}}
impl uasf_json::FromValue for Config {fn from_value(v:uasf_json::Value)->uasf_json::Result<Self>{fields(&v,&["version","bundle","routes"])?;let version:u64=uasf_json::from_value(v["version"].clone())?;if version!=1{return Err(<uasf_json::Error as serde::de::Error>::custom("version"));}Ok(Self{version:1,bundle:PathBuf::from(text(&v,"bundle")?),routes:uasf_json::from_value(v["routes"].clone())?})}}
impl uasf_json::FromValue for Binding {fn from_value(v:uasf_json::Value)->uasf_json::Result<Self>{fields(&v,&["version","bundle","device","inode","route","config_sha256"])?;let version:u64=uasf_json::from_value(v["version"].clone())?;if version!=1{return Err(<uasf_json::Error as serde::de::Error>::custom("version"));}Ok(Self{version:1,bundle:PathBuf::from(text(&v,"bundle")?),device:uasf_json::from_value(v["device"].clone())?,inode:uasf_json::from_value(v["inode"].clone())?,route:uasf_json::from_value(v["route"].clone())?,config_sha256:text(&v,"config_sha256")?})}}
