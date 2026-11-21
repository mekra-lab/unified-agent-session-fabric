//! Local immutable LF-bounded captures with durable receipts and deletion barriers.
pub mod files;
pub mod policy;
use uasf_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    collections::BTreeSet,
};
use uasf_core::model::sha;
pub const LIMIT: u64 = 32 * 1024 * 1024;
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(uasf_json::Error),
    Value,
    Key,
    Integrity,
    Denied(&'static str),
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl From<uasf_json::Error> for Error {
    fn from(e: uasf_json::Error) -> Self { Self::Json(e) }
}
pub type Result<T> = std::result::Result<T, Error>;
pub fn response(e: &Error) -> Value {
    match e {
        Error::Denied(status) => json!({"status":status}),
        _ =>
            json!({"status":"unsupported","reason":match
                    e{Error::Io(e)=>match
                        e.kind(){std::io::ErrorKind::NotFound=>"FileNotFoundError",std::io::ErrorKind::PermissionDenied=>"PermissionError",std::io::ErrorKind::AlreadyExists=>"FileExistsError",std::io::ErrorKind::NotADirectory=>"NotADirectoryError",std::io::ErrorKind::IsADirectory=>"IsADirectoryError",_=>"OSError"},Error::Json(_)=>"JSONDecodeError",Error::Key=>"KeyError",Error::Integrity=>"RuntimeError",_=>"ValueError"}}),
    }
}
pub fn uuid() -> Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}",&h[..8],&h[8..12],&h[12..16],&h[16..20],&h[20..]))
}
pub fn now() -> String {
    let duration =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let seconds = duration.as_secs() as libc::time_t;
    let mut date = std::mem::MaybeUninit::<libc::tm>::uninit();
    unsafe {
        libc::gmtime_r(&seconds, date.as_mut_ptr());
        let t = date.assume_init();
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}+00:00",t.tm_year+1900,t.tm_mon+1,t.tm_mday,t.tm_hour,t.tm_min,t.tm_sec,duration.subsec_micros())
    }
}
fn text(v: &Value) -> Result<&str> { v.as_str().ok_or(Error::Key) }
pub fn allowed(kind: &str, path: &Path) -> bool {
    if files::relative(path).is_err() { return false }
    let parts: Vec<_> = path.components().collect();
    if parts.len() < 2 ||
            path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
        return false
    }
    let first = parts[0].as_os_str();
    let name = path.file_name().unwrap().to_string_lossy();
    if kind == "codex" {
        return first == "sessions" && name.starts_with("rollout-")
    }
    let stem = path.file_stem().unwrap().to_string_lossy();
    kind == "claude" && first == "projects" && stem.len() == 36 &&
        stem.bytes().enumerate().all(|(i, b)|
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else { b.is_ascii_hexdigit() })
}
pub struct Store {
    pub root: PathBuf,
    pub registry: Value,
}
impl Store {
    pub fn new(root: PathBuf, registry: Value) -> Result<Self> {
        files::absolute(&root)?;
        for p in root.ancestors() {
            if p.join(".git").is_file() || p.join(".git/HEAD").exists() {
                return Err(Error::Value)
            }
        }
        if registry["version"] != 1 { return Err(Error::Value) }
        for config in
            registry["sources"].as_object().ok_or(Error::Key)?.values() {
            let original = Path::new(text(&config["root"])?);
            files::absolute(original)?;
            if original.starts_with(&root) || root.starts_with(original) ||
                    !["codex", "claude"].contains(&text(&config["kind"])?) {
                return Err(Error::Value)
            }
            if let Some(items) = config.get("files") {
                for item in items.as_array().ok_or(Error::Value)? {
                    files::relative(Path::new(text(item)?))?;
                }
            }
        }
        for folder in ["", "tmp", "codex", "claude"] {
            files::directory(&root.join(folder), true)?;
        }
        if fs::metadata(&root)?.mode() & 0o077 != 0 {
            return Err(Error::Value)
        }
        Ok(Self { root, registry })
    }
    pub fn save(&self, state: &Value) -> Result<()> {
        files::save(&self.root, state)
    }
    fn load(&self) -> Result<Value> {
        let path = self.root.join("state.json");
        let mut state =
            if path.exists() {
                files::json(&path)?
            } else {
                json!({"version":1,"receipts":{},"blocked_paths":[],"blocked_inodes":[],"events":[]})
            };
        if state["version"] != 1 { return Err(Error::Value) }
        self.recover(&mut state)?;
        Ok(state)
    }
    pub fn state(&self) -> Result<Value> {
        let _access = policy::access(&self.root)?;
        let _lock = files::Lock::acquire(&self.root.join("lock"), true)?;
        self.load()
    }
    pub fn recover(&self, state: &mut Value) -> Result<()> {
        let entries: Vec<_> =
            state["receipts"].as_object().ok_or(Error::Key)?.iter().map(|(k,
                            v)| (k.clone(), v.clone())).collect();
        let mut changed = false;
        for (g, r) in entries {
            if r["status"] == "pending" {
                let rel = Path::new(text(&r["staging_relative"])?);
                let temp = Path::new(text(&r["temporary"])?);
                files::relative(rel)?;
                files::relative(temp)?;
                let target = self.root.join(rel);
                let temporary = self.root.join(temp);
                if !target.exists() && temporary.exists() {
                    fs::rename(&temporary, &target)?;
                    files::directory(target.parent().unwrap(),
                                    false)?.sync_all()?;
                }
                if target.exists() {
                    let data =
                        files::snapshot(target.parent().unwrap(),
                                    Path::new(target.file_name().unwrap()),
                                    &mut |_| Ok(())).map_err(|_| Error::Integrity)?;
                    if r["length"] != data.data.len() ||
                            r["sha256"] != sha(&data.data) {
                        return Err(Error::Integrity)
                    }
                    state["receipts"][&g]["status"] = "active".into();
                    state["receipts"][&g].as_object_mut().unwrap().remove("temporary");
                } else {
                    state["receipts"].as_object_mut().unwrap().remove(&g);
                }
                changed = true;
            } else if r["status"] == "deleting" {
                self.finish_delete(state, &g)?;
                changed = true;
            }
        }
        if changed { self.save(state)?; }
        let referenced: BTreeSet<_> =
            state["receipts"].as_object().ok_or(Error::Key)?.values().filter_map(|r|
                        r["temporary"].as_str()).collect();
        for entry in fs::read_dir(self.root.join("tmp"))? {
            let path = entry?.path();
            if !referenced.contains(path.strip_prefix(&self.root).unwrap().to_string_lossy().as_ref())
                {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }
    fn admitted(&self, source: &str, relative: &str)
        -> Result<(Value, String, bool)> {
        let config =
            self.registry["sources"].get(source).ok_or(Error::Key)?.clone();
        let path = Path::new(relative);
        files::relative(path)?;
        let rel =
            path.components().collect::<PathBuf>().to_string_lossy().into_owned();
        let ok =
            if let Some(files) = config.get("files") {
                files.as_array().ok_or(Error::Value)?.iter().any(|v|
                            v == &rel) && path.extension().is_some_and(|s| s == "jsonl")
            } else { allowed(text(&config["kind"])?, Path::new(&rel)) };
        Ok((config, rel, ok))
    }
    fn record(&self, state: &mut Value, source: &str, rel: &str,
        result: Value) -> Result<Value> {
        let mut event = json!({"at":now(),"source":source,"relative":rel});
        event.as_object_mut().unwrap().extend(result.as_object().ok_or(Error::Value)?.clone());
        state["events"].as_array_mut().ok_or(Error::Key)?.push(event);
        self.save(state)?;
        Ok(result)
    }
    /// Hold the deletion admission lease before a caller acquires upstream locks.
    pub fn with_capture<T, E: From<Error>>(&self, f: impl FnOnce(&CaptureLease<'_>) -> std::result::Result<T,E>) -> std::result::Result<T,E> {
        let access = policy::access(&self.root)?;
        f(&CaptureLease { store: self, access })
    }
    pub fn capture(&self, source: &str, rel: &str) -> Result<Value> {
        self.capture_with_hook(source, rel, &mut |_| Ok(()))
    }
    pub fn capture_with_hook(&self, source: &str, rel: &str,
        hook: &mut dyn FnMut(&str) -> Result<()>) -> Result<Value> {
        let access =
            match policy::access(&self.root) {
                Ok(a) => a,
                Err(Error::Denied(s)) => return Ok(json!({"status":s})),
                Err(e) => return Err(e),
            };
        self.capture_under(source, rel, hook, &access.jobs, None)
    }
    fn capture_under(&self, source: &str, rel: &str,
        hook: &mut dyn FnMut(&str) -> Result<()>, jobs: &Value, expected: Option<(u64, &str)>)
        -> Result<Value> {
        let _lock = files::Lock::acquire(&self.root.join("lock"), true)?;
        let mut state = self.load()?;
        let (config, rel, ok) = self.admitted(source, rel)?;
        let original = Path::new(text(&config["root"])?);
        let path = original.join(&rel).to_string_lossy().into_owned();
        if !ok {
            return self.record(&mut state, source, &rel,
                    json!({"status":"rejected","reason":"not_registered"}))
        }
        if state["blocked_paths"].as_array().ok_or(Error::Key)?.iter().any(|v|
                    v == &path) {
            return self.record(&mut state, source, &rel,
                    json!({"status":"deleted"}))
        }
        let captured =
            match files::snapshot(original, Path::new(&rel), hook) {
                Ok(c) => c,
                Err(v) => return self.record(&mut state, source, &rel, v),
            };
        let identity =
            json!([captured.metadata.dev(),captured.metadata.ino()]);
        let source_data = captured.data;
        if expected.is_some_and(|(length, digest)| length != source_data.len() as u64 || digest != sha(&source_data)) {
            return Ok(json!({"status":"rejected","reason":"admission_mismatch"}));
        }
        let inspection = if state["blocked_inodes"].as_array().ok_or(Error::Key)?.contains(&identity) {
            policy::Inspection::Match
        } else {
            policy::blocked(jobs, source, &source_data, &path, &identity)?
        };
        match inspection {
            policy::Inspection::Match => return self.record(&mut state, source, &rel,
                json!({"status":"deleted"})),
            policy::Inspection::Unavailable => return self.record(&mut state, source, &rel,
                json!({"status":"not_captured","reason":"deletion_inspection_unavailable"})),
            policy::Inspection::NoMatch => {}
        }
        let source_length = source_data.len();
        let source_digest = sha(&source_data);
        let end =
            source_data.iter().rposition(|b|
                        *b == b'\n').map_or(0, |p| p + 1);
        let data = &source_data[..end];
        let observation =
            json!({"source_length":source_length,"length":end,"unpublished_length":source_length-end,"publication_policy":"last_lf"});
        if data.is_empty() {
            let mut result = observation.clone();
            result["status"] = "not_captured".into();
            result["reason"] = "no_complete_line".into();
            return self.record(&mut state, source, &rel, result)
        }
        let digest = sha(data);
        let previous =
            state["receipts"].as_object().ok_or(Error::Key)?.values().filter(|r|
                            r["status"] == "active" && r["source"] == source &&
                                r["original_path"] == path).last().cloned();
        let mut relation = "initial";
        if let Some(p) = &previous {
            let n =
                p.get("source_length").unwrap_or(&p["length"]).as_u64().ok_or(Error::Value)?
                    as usize;
            let prior = p.get("source_sha256").unwrap_or(&p["sha256"]);
            relation =
                if identity != json!([p["device"],p["inode"]]) {
                    "replacement"
                } else if source_length < n {
                    "shrink"
                } else if sha(&source_data[..n]) != *prior {
                    "prefix_changed"
                } else if source_length > n { "append" } else { "unchanged" };
            if relation == "unchanged" &&
                    end < (p["length"].as_u64().ok_or(Error::Value)? as usize) {
                relation = "publication_boundary_changed"
            }
            if ["append", "unchanged"].contains(&relation) &&
                        p["length"] == end && p["sha256"] == digest {
                let old = self.root.join(text(&p["staging_relative"])?);
                let check =
                    files::snapshot(old.parent().unwrap(),
                        Path::new(old.file_name().unwrap()), &mut |_| Ok(()));
                if !check.is_ok_and(|c|
                                c.data.len() == end && sha(&c.data) == digest) {
                    return self.record(&mut state, source, &rel,
                            json!({"status":"unsupported","reason":"published_copy_missing_or_changed"}))
                }
                let mut event = observation.clone();
                event["status"] = "unchanged".into();
                event["generation"] = p["generation"].clone();
                self.record(&mut state, source, &rel, event)?;
                return Ok(json!({"status":"unchanged","receipt":p,"observation":observation}))
            }
        }
        let generation = uuid()?;
        let lineage =
            if relation == "append" {
                previous.as_ref().unwrap()["file_generation"].clone()
            } else { json!(generation) };
        let kind = text(&config["kind"])?;
        let target =
            if kind == "claude" {
                format!("claude/projects/{generation}/{generation}.jsonl")
            } else {
                let name =
                    Path::new(&rel).file_name().unwrap().to_string_lossy();
                format!("codex/sessions/{generation}/{}",if
                    name.starts_with("rollout-"){name.into_owned()}else{format!("rollout-{generation}.jsonl")})
            };
        let final_path = self.root.join(&target);
        files::directory(final_path.parent().unwrap(), true)?;
        let temp = format!("tmp/{generation}.capture");
        let mut stream =
            OpenOptions::new().write(true).create_new(true).mode(0o400).open(self.root.join(&temp))?;
        stream.write_all(data)?;
        stream.sync_all()?;
        files::directory(&self.root.join("tmp"), false)?.sync_all()?;
        let receipt =
            json!({"status":"pending","source":source,"kind":kind,"original_path":path,"device":captured.metadata.dev(),"inode":captured.metadata.ino(),"source_facts":files::facts(&captured.metadata),"generation":generation,"file_generation":lineage,"previous_generation":previous.as_ref().map(|p|p["generation"].clone()),"relation":relation,"length":end,"sha256":digest,"captured_at":captured.at,"source_length":source_length,"source_sha256":source_digest,"unpublished_length":source_length-end,"publication_policy":"last_lf","staging_relative":target,"temporary":temp,"atomic_snapshot_claimed":false});
        state["receipts"][&generation] = receipt;
        self.save(&state)?;
        hook("receipt_durable")?;
        fs::rename(self.root.join(temp), &final_path)?;
        files::directory(final_path.parent().unwrap(), false)?.sync_all()?;
        hook("published")?;
        state["receipts"][&generation]["status"] = "active".into();
        state["receipts"][&generation].as_object_mut().unwrap().remove("temporary");
        self.save(&state)?;
        Ok(json!({"status":"published","receipt":state["receipts"][&generation]}))
    }
    pub fn finish_delete(&self, state: &mut Value, generation: &str)
        -> Result<()> {
        let rel = text(&state["receipts"][generation]["staging_relative"])?;
        files::relative(Path::new(rel))?;
        let path = self.root.join(rel);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        files::directory(path.parent().unwrap(), false)?.sync_all()?;
        state["receipts"][generation] =
            json!({"status":"deleted","generation":generation,"staging_relative":rel});
        Ok(())
    }
    pub fn delete(&self, source: &str, rel: &str) -> Result<Value> {
        self.delete_with_hook(source, rel, &mut |_| Ok(()))
    }
    pub fn delete_with_hook(&self, source: &str, rel: &str,
        hook: &mut dyn FnMut(&str) -> Result<()>) -> Result<Value> {
        if policy::binding(&self.root, None, false)?.is_some() {
            return Ok(json!({"status":"coordinator_required"}))
        }
        let _lock = files::Lock::acquire(&self.root.join("lock"), true)?;
        let mut state = self.load()?;
        let (config, rel, ok) = self.admitted(source, rel)?;
        if !ok {
            return Ok(json!({"status":"rejected","reason":"not_registered"}))
        }
        let mut paths: BTreeSet<String> =
            state["blocked_paths"].as_array().ok_or(Error::Key)?.iter().map(|v|
                            text(v).map(String::from)).collect::<Result<_>>()?;
        paths.insert(Path::new(text(&config["root"])?).join(&rel).to_string_lossy().into_owned());
        let mut ids: BTreeSet<(u64, u64)> =
            state["blocked_inodes"].as_array().ok_or(Error::Key)?.iter().map(|v|
                            Ok((v[0].as_u64().ok_or(Error::Value)?,
                                    v[1].as_u64().ok_or(Error::Value)?))).collect::<Result<_>>()?;
        if let Ok(c) =
                files::snapshot(Path::new(text(&config["root"])?),
                    Path::new(&rel), &mut |_| Ok(())) {
            ids.insert((c.metadata.dev(), c.metadata.ino()));
        }
        let live: Vec<_> =
            state["receipts"].as_object().ok_or(Error::Key)?.values().filter(|r|
                            r["status"] == "active").cloned().collect();
        loop {
            let before = (paths.len(), ids.len());
            for r in &live {
                let id =
                    (r["device"].as_u64().ok_or(Error::Value)?,
                        r["inode"].as_u64().ok_or(Error::Value)?);
                let path = text(&r["original_path"])?;
                if paths.contains(path) || ids.contains(&id) {
                    paths.insert(path.into());
                    ids.insert(id);
                }
            }
            if before == (paths.len(), ids.len()) { break }
        }
        state["blocked_paths"] = json!(paths);
        state["blocked_inodes"] = json!(ids);
        let mut generations = Vec::new();
        for r in live {
            if paths.contains(text(&r["original_path"])?) {
                let g = text(&r["generation"])?;
                state["receipts"][g]["status"] = "deleting".into();
                generations.push(g.to_string());
            }
        }
        self.save(&state)?;
        hook("delete_durable")?;
        for g in &generations { self.finish_delete(&mut state, g)? }
        self.save(&state)?;
        Ok(json!({"status":"deleted","generations":generations,"archive_deletion":"pending_external_excision"}))
    }
    pub fn scan(&self, source: &str) -> Result<Value> {
        // Hold the queue barrier across the entire scan and every publication.
        let access =
            match policy::access(&self.root) {
                Ok(a) => a,
                Err(Error::Denied(s)) => return Ok(json!([{"status":s}])),
                Err(e) => return Err(e),
            };
        let config = self.registry["sources"].get(source).ok_or(Error::Key)?;
        let mut found = Vec::new();
        let mut rejected = Vec::new();
        if let Some(list) = config.get("files") {
            for r in list.as_array().ok_or(Error::Value)? {
                found.push(text(r)?.to_string());
            }
        } else {
            fn walk(fd: &std::fs::File, base: &Path, kind: &str,
                found: &mut Vec<String>, rejected: &mut Vec<String>)
                -> Result<()> {
                use std::os::fd::AsRawFd;
                // Enumeration uses the already-open directory; subsequent opens stay relative to it.
                let mut entries: Vec<_> =
                    fs::read_dir(format!("/proc/self/fd/{}",fd.as_raw_fd()))?.collect::<std::io::Result<_>>()?;
                entries.sort_by_key(|e| e.file_name());
                for e in entries {
                    let name = e.file_name();
                    let rel = base.join(&name);
                    let info = e.file_type()?;
                    if info.is_symlink() {
                        rejected.push(rel.to_string_lossy().into_owned());
                    } else if info.is_dir() {
                        let child = files::open_at(fd, &name, true)?;
                        walk(&child, &rel, kind, found, rejected)?;
                    } else if allowed(kind, &rel) {
                        found.push(rel.to_string_lossy().into_owned());
                    }
                }
                Ok(())
            }
            let walked =
                files::directory(Path::new(text(&config["root"])?),
                        false).and_then(|fd|
                        walk(&fd, Path::new(""), text(&config["kind"])?, &mut found,
                            &mut rejected));
            if let Err(e) = walked {
                let _lock =
                    files::Lock::acquire(&self.root.join("lock"), true)?;
                let mut state = self.load()?;
                let errno =
                    if let Error::Io(e) = e { e.raw_os_error() } else { None };
                return Ok(json!([self.record(&mut
                            state,source,".",json!({"status":"rejected","reason":"unsafe_scan","errno":errno}))?]))
            }
        }
        let mut results = Vec::new();
        for r in found {
            let mut v =
                self.capture_under(source, &r, &mut |_| Ok(()),
                        &access.jobs, None)?;
            v["relative"] = r.into();
            results.push(v);
        }
        if !rejected.is_empty() {
            let _lock = files::Lock::acquire(&self.root.join("lock"), true)?;
            let mut state = self.load()?;
            for r in rejected {
                let mut v =
                    self.record(&mut state, source, &r,
                            json!({"status":"rejected","reason":"symlink"}))?;
                v["relative"] = r.into();
                results.push(v);
            }
        }
        Ok(json!(results))
    }
}

/// Unforgeable admission capability; queue lock remains held for this lifetime.
pub struct CaptureLease<'a> { store: &'a Store, access: policy::Access }
impl CaptureLease<'_> {
    pub fn capture(&self, source: &str, relative: &str, length: u64, sha256: &str) -> Result<Value> {
        self.store.capture_under(source, relative, &mut |_| Ok(()), &self.access.jobs, Some((length,sha256)))
    }
}
