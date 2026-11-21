//! Durable deletion plans. Payload removal and archive excision are separate,
//! retryable effects; a pending plan blocks admission until every effect is acked.
pub mod process;
use uasf_json::{json, Value};
use std::{
    collections::BTreeSet, fs::{self, File, OpenOptions},
    io::Write, os::{fd::AsRawFd, unix::fs::{OpenOptionsExt, MetadataExt}},
    path::{Path, PathBuf},
};
use rusqlite::{Connection, OpenFlags};
use uasf_core::model::sha;
use uasf_json as decode;
use uasf_staging::{
    files, policy::{self, Inspection},
    Store,
};

pub const STEPS: [&str; 5] =
    ["staging", "intake", "cache", "archive", "blocklist"];
#[derive(Debug)]
pub struct Error(pub &'static str);
pub type Result<T> = std::result::Result<T, Error>;
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self { Self("OSError") }
}
impl From<uasf_json::Error> for Error {
    fn from(_: uasf_json::Error) -> Self { Self("JSONDecodeError") }
}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self { Self("RuntimeError") }
}
impl From<uasf_staging::Error> for Error {
    fn from(_: uasf_staging::Error) -> Self { Self("RuntimeError") }
}
fn text(v: &Value) -> Result<&str> { v.as_str().ok_or(Error("ValueError")) }
fn array(v: &Value) -> Result<&Vec<Value>> {
    v.as_array().ok_or(Error("ValueError"))
}
fn object(v: &Value) -> Result<&uasf_json::Map<String, Value>> {
    v.as_object().ok_or(Error("ValueError"))
}
pub fn atomic(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().ok_or(Error("ValueError"))?;
    let temp = parent.join(format!(".pending-{}",uasf_staging::uuid()?));
    let mut f =
        OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
    f.write_all(&uasf_json::to_vec(value)?)?;
    f.sync_all()?;
    fs::rename(&temp, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn read(path: &Path) -> Result<Value> {
    Ok(decode::from_slice(&fs::read(path)?)?)
}

/// Owns the same open file description inherited by a trusted child executor.
/// Dropping it releases the lock only after all child operations have returned.
pub struct Lease(File);
impl Lease {
    fn acquire(root: &Path, wait: bool) -> Result<Self> {
        let file =
            OpenOptions::new().read(true).write(true).create(true).truncate(false).custom_flags(libc::O_NOFOLLOW
                                | libc::O_CLOEXEC).mode(0o600).open(root.join("lock"))?;
        if unsafe {
                    libc::flock(file.as_raw_fd(),
                        libc::LOCK_EX | if wait { 0 } else { libc::LOCK_NB })
                } != 0 {
            return Err(Error("pending_deletion"));
        }
        Ok(Self(file))
    }
    pub fn fd(&self) -> i32 { self.0.as_raw_fd() }
}

pub trait Executor {
    fn complete_integration(&mut self, _job:&Value) -> Result<()> { Err(Error("integration_executor_required")) }
    fn erase(&mut self, receipts: &Value, lease: &Lease)
    -> Result<()>;
    fn excise(&mut self, scope: &str)
    -> Result<()>;
}

pub struct Coordinator {
    integration: Value,
    pub root: PathBuf,
    pub store: Store,
    pub intake: PathBuf,
}
impl Coordinator {
    pub fn open(root: PathBuf, staging: PathBuf, registry: Value,
        intake: PathBuf) -> Result<Self> {
        files::absolute(&root)?;
        files::absolute(&intake)?;
        Ok(Self { integration: Value::Null, root, store: Store::new(staging, registry)?, intake })
    }
    pub fn new(root: PathBuf, staging: PathBuf, registry: Value,
        intake: PathBuf) -> Result<Self> {
        Self::integrated(root,staging,registry,intake,Value::Null,true)
    }
    pub fn integrated(root:PathBuf,staging:PathBuf,registry:Value,intake:PathBuf,integration:Value,initialize:bool)->Result<Self> {
        let mut this=Self::open(root,staging,registry,intake)?;
        if !integration.is_null() {
            if object(&integration)?.len()!=2 || integration.get("core").is_none() || integration.get("completion_command").is_none() {return Err(Error("ValueError"));}
            for name in ["index","persistent"] {files::absolute(Path::new(text(&integration["core"][name])?))?;}
            let command=array(&integration["completion_command"])?;
            if command.is_empty() || !Path::new(text(&command[0])?).is_absolute() {return Err(Error("ValueError"));}
            for word in command {text(word)?;}
        }
        this.integration=integration;
        if !initialize {return Ok(this);}
        if this.root.starts_with(&this.store.root) || this.store.root.starts_with(&this.root) || this.intake.starts_with(&this.root) {return Err(Error("ValueError"));}
        files::directory(&this.root,true)?;
        if fs::metadata(&this.root)?.mode()&0o077!=0 {return Err(Error("ValueError"));}
        files::directory(&this.root.join("cache"),true)?;
        let _lock = Lease::acquire(&this.root, true)?;
        this.scrub()?;
        let path = this.root.join("jobs.json");
        if !path.exists() {
            if this.root.join("initialized.json").exists() {
                return Err(Error("RuntimeError"));
            }
            this.save(&json!({"version":1,"jobs":{},"links":{},"binding":this.binding()}))?;
        } else { this.load()?; }
        atomic(&this.root.join("initialized.json"), &json!({"version":1}))?;
        this.bind(&this.store.root.join("deletion-binding.json"),
                &this.store.root, "staging")?;
        this.bind(&this.intake.with_file_name(format!("{}.deletion-binding.json",this.intake.file_name().ok_or(Error("ValueError"))?.to_string_lossy())),
                &this.intake, "intake")?;
        if !this.integration.is_null() {
            let core=uasf_core::Core::open(text(&this.integration["core"]["index"])?,text(&this.integration["core"]["persistent"])?) .map_err(|_|Error("RuntimeError"))?;
            core.bind_deletion(&this.root).map_err(|_|Error("RuntimeError"))?;
        }
        Ok(this)
    }
    fn bind(&self, path: &Path, resource: &Path, kind: &str) -> Result<()> {
        let value =
            json!({"version":1,"kind":kind,"resource":resource,"root":self.root,"owner_uid":unsafe{libc::geteuid()}});
        if path.exists() {
            if read(path)? != value { return Err(Error("ValueError")); }
        } else { atomic(path, &value)?; }
        Ok(())
    }
    fn binding(&self) -> Value {
        {
            let mut value=json!({"staging":self.store.root,"registry":self.store.registry,"intake":self.intake});
            if !self.integration.is_null(){value["integration"]=self.integration.clone();} value
        }
    }
    fn scrub(&self) -> Result<()> {
        let mut removed = false;
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.file_name().is_some_and(|n|
                        n.to_string_lossy().starts_with(".pending-")) {
                fs::remove_file(path)?;
                removed = true;
            }
        }
        if removed { File::open(&self.root)?.sync_all()?; }
        Ok(())
    }
    fn load(&self) -> Result<Value> {
        self.scrub()?;
        let value = read(&self.root.join("jobs.json"))?;
        if value["version"] != 1 || value["binding"] != self.binding() {
            return Err(Error("ValueError"));
        }
        object(&value["jobs"])?;
        object(&value["links"])?;
        Ok(value)
    }
    fn save(&self, state: &Value) -> Result<()> {
        atomic(&self.root.join("jobs.json"), state)
    }
    pub fn state(&self) -> Result<Value> {
        let _lock = Lease::acquire(&self.root, true)?;
        self.load()
    }
    pub fn pending(state: &Value) -> bool {
        state["jobs"].as_object().is_none_or(|jobs|
                jobs.values().any(|j| j["status"] != "complete"))
    }
    fn database(&self) -> Result<Connection> {
        Ok(Connection::open_with_flags(&self.intake,
                    OpenFlags::SQLITE_OPEN_READ_ONLY)?)
    }
    fn admitted(&self, source: &str, rel: &str) -> Result<PathBuf> {
        files::relative(Path::new(rel))?;
        let c = &self.store.registry["sources"][source];
        let ok =
            if let Some(items) = c.get("files") {
                array(items)?.iter().any(|v| v == rel) &&
                    Path::new(rel).extension().is_some_and(|x| x == "jsonl")
            } else {
                uasf_staging::allowed(text(&c["kind"])?, Path::new(rel))
            };
        if !ok { return Err(Error("ValueError")); }
        Ok(PathBuf::from(text(&c["root"])?).join(Path::new(rel).components().collect::<PathBuf>()))
    }
    fn staging_state(&self) -> Result<Value> {
        let path = self.store.root.join("state.json");
        let mut state =
            if path.exists() {
                read(&path)?
            } else {
                json!({"version":1,"receipts":{},"events":[],"blocked_paths":[],"blocked_inodes":[]})
            };
        if state["version"] != 1 { return Err(Error("ValueError")); }
        self.store.recover(&mut state)?;
        Ok(state)
    }
    pub fn link_receipt(&self, id: &str, source: &str, rel: &str)
        -> Result<Value> {
        let _lock = Lease::acquire(&self.root, true)?;
        let mut state = self.load()?;
        if Self::pending(&state) { return Err(Error("RuntimeError")); }
        let path = self.admitted(source, rel)?;
        if self.database()?.query_row("SELECT count(*) FROM observation WHERE receipt_id=?1",
                        [id], |r| r.get::<_, i64>(0))? != 1 {
            return Err(Error("ValueError"));
        }
        if state["links"].get(id).is_some_and(|v|
                    v != path.to_string_lossy().as_ref()) {
            return Err(Error("ValueError"));
        }
        state["links"][id] = json!(path);
        self.save(&state)?;
        Ok(Value::Null)
    }
    pub fn request(&self, selector: &Value, input_map: &Value,
        id: Option<&str>, hook: &mut dyn FnMut(&str) -> Result<()>)
        -> Result<Value> {
        let mut selector = selector.clone();
        let mut pending_text = None;
        let kind = text(&selector["kind"])?.to_owned();
        match kind.as_str() {
            "text" => {
                let s = text(&selector["text"])?;
                if s.is_empty() { return Err(Error("ValueError")); }
                pending_text = Some(s.to_owned());
                selector =
                    json!({"kind":"text","length":s.len(),"sha256":sha(s.as_bytes())});
            }
            "file" => {
                let src = text(&selector["source"])?;
                let rel = text(&selector["relative"])?;
                self.admitted(src, rel)?;
                selector =
                    json!({"kind":"file","source":src,"relative":Path::new(rel).components().collect::<PathBuf>()});
            }
            "session" | "receipt" => {
                if text(&selector["source"])?.is_empty() ||
                        text(&selector[if kind == "session" {
                                                "session"
                                            } else { "receipt_id" }])?.is_empty() {
                    return Err(Error("ValueError"));
                }
            }
            _ => return Err(Error("ValueError")),
        }
        let id = id.map(String::from).unwrap_or(uasf_staging::uuid()?);
        let _lock = Lease::acquire(&self.root, true)?;
        let mut state = self.load()?;
        if let Some(old) = state["jobs"].get(&id) {
            return if old["selector"] == selector {
                    Ok(old.clone())
                } else { Err(Error("ValueError")) };
        }
        if Self::pending(&state) { return Err(Error("RuntimeError")); }
        let mut paths = BTreeSet::new();
        let mut inodes = BTreeSet::<(u64, u64)>::new();
        let mut targets = Vec::new();
        let mut inputs = BTreeSet::<String>::new();
        if kind == "file" {
            let src = text(&selector["source"])?;
            let rel = text(&selector["relative"])?;
            paths.insert(self.admitted(src,
                                rel)?.to_string_lossy().into_owned());
            let root =
                Path::new(text(&self.store.registry["sources"][src]["root"])?);
            if let Ok(c) =
                    files::snapshot(root, Path::new(rel), &mut |_| Ok(())) {
                inodes.insert((c.metadata.dev(), c.metadata.ino()));
            }
        }
        {
            let _stage_lock =
                files::Lock::acquire(&self.store.root.join("lock"), true)?;
            let staging = self.staging_state()?;
            let live: Vec<_> =
                object(&staging["receipts"])?.values().filter(|r|
                            r["status"] == "active").collect();
            for r in &live {
                let rel = Path::new(text(&r["staging_relative"])?);
                files::relative(rel)?;
                let capture =
                    files::snapshot(&self.store.root, rel,
                                &mut |_| Ok(())).map_err(|_| Error("RuntimeError"))?;
                let data = &capture.data;
                if r["sha256"] != sha(data) || r["length"] != data.len() {
                    return Err(Error("RuntimeError"));
                }
                let yes =
                    if kind == "text" {
                        inspected(policy::contains(data, &selector))?
                    } else if kind == "session" &&
                            r["source"] == selector["source"] {
                        session_match(data, text(&selector["session"])?)?
                    } else { false };
                if yes {
                    paths.insert(text(&r["original_path"])?.to_owned());
                }
            }
            loop {
                let before = (paths.len(), inodes.len());
                for r in &live {
                    let identity = identity(r)?;
                    let path = text(&r["original_path"])?;
                    if paths.contains(path) || inodes.contains(&identity) {
                        paths.insert(path.to_owned());
                        inodes.insert(identity);
                    }
                }
                if before == (paths.len(), inodes.len()) { break; }
            }
            for r in live {
                if paths.contains(text(&r["original_path"])?) {
                    let staged =
                        self.store.root.join(text(&r["staging_relative"])?);
                    let ids =
                        input_map[staged.to_string_lossy().as_ref()].as_array().ok_or(Error("RuntimeError"))?;
                    if ids.is_empty() { return Err(Error("RuntimeError")); }
                    for id in ids {
                        let id = text(id)?;
                        if id.is_empty() { return Err(Error("RuntimeError")); }
                        inputs.insert(id.to_owned());
                    }
                    let source = text(&r["source"])?;
                    let root =
                        Path::new(text(&self.store.registry["sources"][source]["root"])?);
                    let relative =
                        Path::new(text(&r["original_path"])?).strip_prefix(root).map_err(|_|
                                    Error("ValueError"))?;
                    targets.push(json!({"source":source,"relative":relative}));
                }
            }
            if kind == "file" {
                targets.push(json!({"source":selector["source"],"relative":selector["relative"]}));
            }
        }
        let db = self.database()?;
        let mut query =
            db.prepare("SELECT receipt_id,source,idem_key,session,payload FROM observation ORDER BY rowid")?;
        let mut rows = query.query([])?;
        let mut receipts = Vec::new();
        let mut keys = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let source: String = row.get(1)?;
            let key: Option<String> = row.get(2)?;
            let session: String = row.get(3)?;
            let data: Vec<u8> = row.get(4)?;
            let matched =
                if kind == "text" {
                                inspected(policy::contains(&data, &selector))?
                            } else { false } ||
                            (kind == "session" && selector["source"] == source &&
                                    selector["session"] == session) ||
                        (kind == "receipt" && selector["source"] == source &&
                                selector["receipt_id"] == id) ||
                    state["links"].get(&id).and_then(Value::as_str).is_some_and(|p|
                            paths.contains(p));
            if matched {
                receipts.push(id);
                if let Some(key) = key {
                    keys.push(json!([source,sha(key.as_bytes())]));
                }
            }
        }
        let mut job =
            json!({"id":id,"selector":selector,"paths":paths,"inodes":inodes,"targets":targets,"inputs":inputs,"receipts":receipts,"keys":keys,"steps":[],"archive_done":[],"status":"pending","attempts":0});
        if let Some(text) = pending_text {
            job["pending_text"] = text.into();
        }
        state["jobs"][&id] = job.clone();
        self.save(&state)?;
        hook("intent")?;
        Ok(job)
    }
    fn redact(&self, job: &Value) -> Result<()> {
        let _lock =
            files::Lock::acquire(&self.store.root.join("lock"), true)?;
        let mut state = self.staging_state()?;
        let mut paths: BTreeSet<String> =
            array(&state["blocked_paths"])?.iter().map(|v|
                            text(v).map(String::from)).collect::<Result<_>>()?;
        let mut ids: BTreeSet<(u64, u64)> =
            array(&state["blocked_inodes"])?.iter().map(|v|
                            pair(v)).collect::<Result<_>>()?;
        for path in array(&job["paths"])? {
            paths.insert(text(path)?.to_owned());
        }
        for v in array(&job["inodes"])? { ids.insert(pair(v)?); }
        let live: Vec<_> =
            object(&state["receipts"])?.values().filter(|r|
                            r["status"] == "active").cloned().collect();
        let mut generations = Vec::new();
        for r in live {
            if paths.contains(text(&r["original_path"])?) ||
                    ids.contains(&identity(&r)?) {
                let g = text(&r["generation"])?;
                state["receipts"][g]["status"] = "deleting".into();
                generations.push(g.to_owned());
            }
        }
        state["blocked_paths"] = json!(paths);
        state["blocked_inodes"] = json!(ids);
        self.store.save(&state)?;
        for generation in generations {
            self.store.finish_delete(&mut state, &generation)?;
        }
        self.store.save(&state)?;
        Ok(())
    }
    pub fn resume(&self, executor: &mut dyn Executor,
        hook: &mut dyn FnMut(&str) -> Result<()>) -> Result<Value> {
        self.resume_saving(executor, hook, &mut |state| self.save(state))
    }
    fn resume_saving(&self, executor: &mut dyn Executor,
        hook: &mut dyn FnMut(&str) -> Result<()>, save: &mut dyn FnMut(&Value) -> Result<()>) -> Result<Value> {
        let lease = Lease::acquire(&self.root, true)?;
        let mut state = self.load()?;
        let ids: Vec<_> = object(&state["jobs"])?.keys().cloned().collect();
        for id in ids {
            if state["jobs"][&id]["status"] == "complete" { continue; }
            state["jobs"][&id]["status"] = "in_progress".into();
            let attempts =
                state["jobs"][&id]["attempts"].as_u64().ok_or(Error("ValueError"))?;
            state["jobs"][&id]["attempts"] = json!(attempts+1);
            save(&state)?;
            let effect =
                (|| -> Result<()>
                            {
                                let steps:Vec<&str>=if self.integration.is_null(){STEPS.to_vec()}else{vec!["staging","intake","cache","archive","integration","blocklist"]};
                                for step in steps {
                                    if array(&state["jobs"][&id]["steps"])?.iter().any(|v|
                                                v == step) {
                                        continue;
                                    }
                                    match step {
                                        "staging" => self.redact(&state["jobs"][&id])?,
                                        "intake" => {
                                            executor.erase(&state["jobs"][&id]["receipts"], &lease)?;
                                            for receipt in
                                                array(&state["jobs"][&id]["receipts"])?.clone() {
                                                state["links"].as_object_mut().ok_or(Error("ValueError"))?.remove(text(&receipt)?);
                                            }
                                        }
                                        "cache" => {
                                            for entry in fs::read_dir(self.root.join("cache"))? {
                                                fs::remove_file(entry?.path())?;
                                            }
                                            File::open(self.root.join("cache"))?.sync_all()?;
                                        }
                                        "archive" => {
                                            let job = &state["jobs"][&id];
                                            let scopes =
                                                if job["selector"]["kind"] == "text" {
                                                    vec![format!("text:{}",text(&job["pending_text"])?)]
                                                } else {
                                                    array(&job["inputs"])?.iter().map(|v|
                                                                    text(v).map(|s|
                                                                            format!("input:{s}"))).collect::<Result<Vec<_>>>()?
                                                };
                                            for (i, scope) in scopes.iter().enumerate() {
                                                if array(&state["jobs"][&id]["archive_done"])?.contains(&json!(i))
                                                    {
                                                    continue;
                                                }
                                                executor.excise(scope)?;
                                                hook(&format!("archive-effect-{i}"))?;
                                                state["jobs"][&id]["archive_done"].as_array_mut().unwrap().push(json!(i));
                                                save(&state)?;
                                                hook(&format!("archive-ack-{i}"))?;
                                            }
                                        }
                                        "integration" => executor.complete_integration(&state["jobs"][&id])?,
                                        "blocklist" => {
                                            state["jobs"][&id].as_object_mut().unwrap().remove("pending_text");
                                        }
                                        _ => unreachable!(),
                                    }
                                    hook(&format!("{step}-effect"))?;
                                    state["jobs"][&id]["steps"].as_array_mut().unwrap().push(json!(step));
                                    if step == "blocklist" {
                                        state["jobs"][&id]["status"] = "complete".into();
                                        state["jobs"][&id].as_object_mut().unwrap().remove("error");
                                    }
                                    save(&state)?;
                                    hook(step)?;
                                }
                                // A prior failed final save (or post-save hook) may
                                // leave all acknowledgements in a pending journal.
                                // Completion is derived from the durable steps on every retry.
                                state["jobs"][&id]["status"] = "complete".into();
                                state["jobs"][&id].as_object_mut().unwrap().remove("error");
                                state["jobs"][&id].as_object_mut().unwrap().remove("pending_text");
                                save(&state)?;
                                Ok(())
                            })();
            if let Err(e) = effect {
                state["jobs"][&id]["status"] = "pending".into();
                state["jobs"][&id]["error"] = e.0.into();
                save(&state)?;
            }
        }
        let jobs: uasf_json::Map<_, _> =
            object(&state["jobs"])?.iter().map(|(k, v)|
                        (k.clone(),
                            json!({"status":v["status"],"steps":v["steps"]}))).collect();
        Ok(json!({"status":if
                    Self::pending(&state){"pending"}else{"complete"},"jobs":jobs}))
    }
    pub fn cache(&self, key: &str, data: Option<&[u8]>) -> Result<Value> {
        let _lock =
            match Lease::acquire(&self.root, false) {
                Ok(l) => l,
                Err(_) => return Ok(json!({"status":"pending_deletion"})),
            };
        let state = self.load()?;
        if !object(&state["jobs"])?.is_empty() {
            return Ok(json!({"status":if
                            data.is_some(){"disabled_after_deletion"}else if
                            Self::pending(&state){"pending_deletion"}else{"deleted"}}));
        }
        let path =
            self.root.join("cache").join(format!("{}.json",sha(key.as_bytes())));
        if let Some(data) = data {
            atomic(&path,
                    &json!({"hex":data.iter().map(|b|format!("{b:02x}")).collect::<String>()}))?;
            Ok(json!({"status":"ok"}))
        } else if path.exists() {
            let mut v = read(&path)?;
            v["status"] = "ok".into();
            Ok(v)
        } else { Ok(json!({"status":"not_found"})) }
    }
}
fn pair(v: &Value) -> Result<(u64, u64)> {
    Ok((v[0].as_u64().ok_or(Error("ValueError"))?,
            v[1].as_u64().ok_or(Error("ValueError"))?))
}
fn identity(r: &Value) -> Result<(u64, u64)> {
    pair(&json!([r["device"],r["inode"]]))
}
fn inspected(i: Inspection) -> Result<bool> {
    match i {
        Inspection::Match => Ok(true),
        Inspection::NoMatch => Ok(false),
        Inspection::Unavailable =>
            Err(Error("deletion_inspection_unavailable")),
    }
}
fn session_match(data: &[u8], session: &str) -> Result<bool> {
    let selector =
        json!({"kind":"session","source":"selected","session":session});
    inspected(policy::blocked(&json!({"jobs":{"selection":{"paths":[],"inodes":[],"selector":selector}}}),
                "selected", data, "", &Value::Null)?)
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    struct Never;
    impl Executor for Never {
        fn erase(&mut self, _: &Value, _: &Lease) -> Result<()> { panic!("effect repeated") }
        fn excise(&mut self, _: &str) -> Result<()> { panic!("effect repeated") }
    }
    #[test]
    fn one_final_save_failure_is_recoverable_after_reopen() {
        let dir=std::env::temp_dir().join(format!("del-final-save-{}",uasf_staging::uuid().unwrap()));
        fs::create_dir(&dir).unwrap();
        let q=Coordinator::new(dir.join("queue"),dir.join("staging"),json!({"version":1,"sources":{}}),dir.join("intake.db")).unwrap();
        let mut state=q.state().unwrap();
        state["jobs"]["j"]=json!({"status":"pending","attempts":0,"steps":["staging","intake","cache","archive"],"pending_text":"erased"});
        q.save(&state).unwrap();
        let mut injected=false;
        let result=q.resume_saving(&mut Never,&mut |_|Ok(()),&mut |s| {
            if s["jobs"]["j"]["status"]=="complete" && !injected {
                injected=true;
                return Err(Error("OSError"));
            }
            q.save(s)
        }).unwrap();
        assert!(injected); assert_eq!(result["status"],"pending");
        let saved=q.state().unwrap();
        assert_eq!(saved["jobs"]["j"]["steps"],json!(STEPS));
        assert_eq!(saved["jobs"]["j"]["status"],"pending");
        let q=Coordinator::open(q.root,q.store.root,q.store.registry,q.intake).unwrap();
        assert_eq!(q.resume(&mut Never,&mut |_|Ok(())).unwrap()["status"],"complete");
        let saved=q.state().unwrap();
        assert!(saved["jobs"]["j"].get("error").is_none());
        assert!(saved["jobs"]["j"].get("pending_text").is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}
