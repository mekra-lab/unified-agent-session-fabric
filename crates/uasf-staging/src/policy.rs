//! The collector-owned deletion barrier stays locked through publication.
use crate::{
    files::{self, Lock},
    Result, Error,
};
use uasf_json::Value;
use std::{fs, os::unix::fs::MetadataExt, path::{Path, PathBuf}};
use uasf_core::model::sha;
pub fn binding(resource: &Path, expected: Option<&Path>, required: bool)
    -> Result<Option<PathBuf>> {
    let path = resource.join("deletion-binding.json");
    if !path.exists() {
        return if required || expected.is_some() {
                Err(Error::Denied("deletion_binding_required"))
            } else { Ok(None) }
    }
    let m = fs::symlink_metadata(&path)?;
    let uid = unsafe { libc::geteuid() };
    if m.file_type().is_symlink() || m.uid() != uid || m.mode() & 0o077 != 0 {
        return Err(Error::Denied("invalid_deletion_binding"))
    }
    let v = files::json(&path)?;
    if v["version"] != 1 ||
                        v["resource"] != resource.to_string_lossy().as_ref() ||
                    v["kind"] != "staging" || v["owner_uid"] != uid ||
            expected.is_some_and(|p|
                    v["root"] != p.to_string_lossy().as_ref()) {
        return Err(Error::Denied("deletion_binding_mismatch"))
    }
    let root = PathBuf::from(v["root"].as_str().ok_or(Error::Value)?);
    files::absolute(&root)?;
    Ok(Some(root))
}
pub struct Access {
    pub jobs: Value,
    _lock: Option<Lock>,
}
pub fn access(resource: &Path) -> Result<Access> {
    let Some(root) =
        binding(resource, None,
                false)? else {
            return Ok(Access { jobs: Value::Null, _lock: None })
        };
    let lock =
        Lock::acquire(&root.join("lock"),
                    false).map_err(|e|
                    match e {
                        Error::Io(ref e) if
                            e.raw_os_error() == Some(libc::EWOULDBLOCK) =>
                            Error::Denied("pending_deletion"),
                        _ => Error::Denied("deletion_state_unavailable"),
                    })?;
    let state =
        files::json(&root.join("jobs.json")).map_err(|_|
                    Error::Denied("deletion_state_unavailable"))?;
    if state["version"] != 1 ||
            state["binding"]["staging"] != resource.to_string_lossy().as_ref()
        {
        return Err(Error::Denied("invalid_deletion_journal"))
    }
    let jobs =
        state["jobs"].as_object().ok_or(Error::Denied("deletion_state_unavailable"))?;
    if jobs.values().any(|j|
                j["status"] != "complete" && j["status"] != "done") {
        return Err(Error::Denied("pending_deletion"))
    }
    Ok(Access { jobs: state, _lock: Some(lock) })
}
/// A failed inspection is never evidence that a deletion selector does not match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inspection {
    Match,
    NoMatch,
    Unavailable,
}

fn rows(data: &[u8]) -> (Vec<Value>, bool) {
    let mut values = Vec::new();
    let mut unavailable = false;
    for line in data.split(|b| *b == b'\n' || *b == b'\r') {
        if line.iter().all(|b| matches!(b, b' ' | b'\t')) { continue; }
        match uasf_json::from_slice(line) {
            Ok(value) => values.push(value),
            Err(_) => unavailable = true,
        }
    }
    (values, unavailable)
}

fn documents(data: &[u8]) -> (Vec<Value>, bool) {
    match uasf_json::from_slice(data) {
        Ok(v) => (vec![v], false),
        Err(_) => rows(data),
    }
}

pub fn contains(data: &[u8], sel: &Value) -> Inspection {
    let Some(n) = sel["length"].as_u64().and_then(|n| usize::try_from(n).ok())
        else { return Inspection::Unavailable };
    let expected = sel["sha256"].as_str().unwrap_or("");
    let matches = |v: &[u8]| {
        if n == 0 { sha(b"") == expected }
        else { v.windows(n).any(|w| sha(w) == expected) }
    };
    if matches(data) { return Inspection::Match }
    let (mut pending, unavailable) = documents(data);
    while let Some(v) = pending.pop() {
        match v {
            Value::String(s) => if matches(s.as_bytes()) { return Inspection::Match },
            Value::Array(a) => pending.extend(a),
            Value::Object(o) => for (k, v) in o {
                if matches(k.as_bytes()) { return Inspection::Match }
                pending.push(v)
            },
            _ => {}
        }
    }
    if unavailable { Inspection::Unavailable } else { Inspection::NoMatch }
}

pub fn blocked(state: &Value, source: &str, data: &[u8], path: &str,
    identity: &Value) -> Result<Inspection> {
    if state.is_null() { return Ok(Inspection::NoMatch); }
    let jobs = state.get("jobs").and_then(Value::as_object).ok_or(Error::Key)?;
    let mut unavailable = false;
    for j in jobs.values() {
        let paths = j.get("paths").ok_or(Error::Key)?.as_array().ok_or(Error::Value)?;
        let inodes = j.get("inodes").ok_or(Error::Key)?.as_array().ok_or(Error::Value)?;
        if paths.iter().any(|p| p == path) || inodes.contains(identity) {
            return Ok(Inspection::Match);
        }
        let sel = j.get("selector").ok_or(Error::Key)?;
        let kind = sel.get("kind").and_then(Value::as_str).ok_or(Error::Key)?;
        if kind == "text" {
            if sel.get("length").and_then(Value::as_u64).is_none()
                || sel.get("sha256").and_then(Value::as_str).is_none() {
                return Err(Error::Key);
            }
            match contains(data, sel) {
                Inspection::Match => return Ok(Inspection::Match),
                Inspection::Unavailable => unavailable = true,
                Inspection::NoMatch => {}
            }
        }
        if kind == "session" {
            let selected_source = sel.get("source").and_then(Value::as_str).ok_or(Error::Key)?;
            let session = sel.get("session").and_then(Value::as_str).ok_or(Error::Key)?;
            if selected_source != source { continue; }
            let (values, failed) = rows(data);
            unavailable |= failed;
            for row in values {
                if (row["type"] == "session_meta" && row["payload"]["id"].is_string() && row["payload"]["id"] == session)
                    || (row["sessionId"].is_string() && row["sessionId"] == session) {
                    return Ok(Inspection::Match);
                }
            }
        }
    }
    Ok(if unavailable { Inspection::Unavailable } else { Inspection::NoMatch })
}
