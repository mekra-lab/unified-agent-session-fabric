//! Optional durable deletion-domain admission, independent of query queue bindings.
use std::{fs::{File,OpenOptions},io::Read,os::{fd::AsRawFd,unix::fs::{MetadataExt,OpenOptionsExt}},path::Path};
use rusqlite::{params,OptionalExtension};
use crate::{Core,model::{Error,Result}};
use uasf_json::{json,Value};

pub fn pending() -> Value { json!({"status":"incomplete","reasons":["pending_deletion"]}) }
fn private(path:&Path,directory:bool)->Result<File>{
    let file=OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC|if directory{libc::O_DIRECTORY}else{0}).open(path)?;
    let m=file.metadata()?;
    if m.uid()!=unsafe{libc::geteuid()} || m.mode()&0o077!=0 || (directory&&!m.is_dir()) || (!directory&&(!m.is_file()||m.nlink()!=1)) {return Err(Error::Invalid("pending_deletion"));}
    Ok(file)
}
impl Core {
    /// Bind once. Removing or replacing the derived index does not remove this policy.
    pub fn bind_deletion(&self,root:&Path)->Result<()> {
        if !root.is_absolute() || root.canonicalize()?!=root {return Err(Error::Invalid("deletion_binding"));}
        private(root,true)?;
        let old:Option<String>=self.db.query_row("SELECT root FROM durable.deletion_binding WHERE id=1",[],|r|r.get(0)).optional()?;
        if old.as_deref().is_some_and(|v|v!=root.to_string_lossy()) {return Err(Error::Invalid("deletion_binding"));}
        self.db.execute("INSERT OR IGNORE INTO durable.deletion_binding(id,root) VALUES(1,?)",params![root.to_string_lossy()])?;
        Ok(())
    }
    pub(crate) fn deletion_lease(&self)->Result<Option<File>> {
        let root:Option<String>=self.db.query_row("SELECT root FROM durable.deletion_binding WHERE id=1",[],|r|r.get(0)).optional()?;
        let Some(root)=root else{return Ok(None)};let root=Path::new(&root);
        private(root,true)?;
        let lock=private(&root.join("lock"),false)?;
        if unsafe{libc::flock(lock.as_raw_fd(),libc::LOCK_SH|libc::LOCK_NB)}!=0 {return Err(Error::Invalid("pending_deletion"));}
        let file=private(&root.join("jobs.json"),false)?;let mut bytes=Vec::new();file.take(16*1024*1024+1).read_to_end(&mut bytes)?;
        if bytes.len()>16*1024*1024 {return Err(Error::Invalid("pending_deletion"));}
        let v:Value=uasf_json::from_slice(&bytes)?;
        if v["version"]!=1 || v["binding"]["integration"]["core"]["persistent"]!=self.persistent_path.to_string_lossy().as_ref() || v["jobs"].as_object().is_none_or(|jobs|jobs.values().any(|j|j["status"]!="complete")) {return Err(Error::Invalid("pending_deletion"));}
        Ok(Some(lock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs,os::unix::fs::PermissionsExt};
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new()->Self {
            let p=std::env::temp_dir().join(format!("deletion-domain-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir(&p).unwrap();fs::set_permissions(&p,fs::Permissions::from_mode(0o700)).unwrap();Self(p)
        }
        fn core(&self,index:&str)->Core {Core::open(self.0.join(index),self.0.join("persistent.sqlite")).unwrap()}
        fn file(&self,name:&str,data:&[u8]) {fs::write(self.0.join(name),data).unwrap();fs::set_permissions(self.0.join(name),fs::Permissions::from_mode(0o600)).unwrap();}
        fn state(&self,status:&str) {self.file("jobs.json",&uasf_json::to_vec(&json!({"version":1,"binding":{"integration":{"core":{"persistent":self.0.join("persistent.sqlite")}}},"jobs":{"j":{"status":status}}})).unwrap());}
    }
    impl Drop for Fixture {fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}
    #[test]
    fn durable_domain_blocks_new_indexes_missing_corrupt_and_pending_journals() {
        let f=Fixture::new();let c=f.core("one.sqlite");assert!(c.deletion_lease().unwrap().is_none());
        c.bind_deletion(&f.0).unwrap();f.file("lock",b"");
        for state in ["missing","corrupt","pending","in_progress"] {
            match state {"missing"=>{},"corrupt"=>f.file("jobs.json",b"{}"),s=>f.state(s)}
            for db in [&c,&f.core("two.sqlite")] {
                assert_eq!(db.query(&json!({"op":"sessions"})).unwrap(),pending());
                let mut out=Vec::new();assert_eq!(db.query_to_writer(&json!({"op":"sessions"}),&mut out).unwrap(),"incomplete");
                assert_eq!(uasf_json::from_slice(&out).unwrap(),pending());
            }
        }
        f.state("complete");assert!(c.deletion_lease().unwrap().is_some());
        let other=Fixture::new();assert!(c.bind_deletion(&other.0).is_err());
    }
    #[test]
    fn query_delivery_holds_shared_lease_and_busy_writer_refuses_new_readers() {
        let f=Fixture::new();let c=f.core("index.sqlite");c.bind_deletion(&f.0).unwrap();f.file("lock",b"");f.state("complete");
        let writer=private(&f.0.join("lock"),false).unwrap();
        assert_eq!(unsafe{libc::flock(writer.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0);
        assert!(c.deletion_lease().is_err());unsafe{libc::flock(writer.as_raw_fd(),libc::LOCK_UN);}
        struct Output<'a>(&'a File,bool);
        impl std::io::Write for Output<'_>{
            fn write(&mut self,b:&[u8])->std::io::Result<usize>{assert_ne!(unsafe{libc::flock(self.0.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0);self.1=true;Ok(b.len())}
            fn flush(&mut self)->std::io::Result<()>{Ok(())}
        }
        let mut out=Output(&writer,false);c.query_to_writer(&json!({"op":"sessions"}),&mut out).unwrap();assert!(out.1);
        assert_eq!(unsafe{libc::flock(writer.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0);
    }
}
