//! Shared deletion journal and flock contract. No daemon or staging dependency.
use crate::{Error, Result, sha, string};
use uasf_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf, Component},
};
fn denied(s: &'static str) -> Error { Error::Denied(s) }
pub fn absolute(path: &Path) -> Result<PathBuf> {
    let joined =
        if path.is_absolute() {
            path.to_path_buf()
        } else { std::env::current_dir()?.join(path) };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => { out.pop(); }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}
fn json(path: &Path) -> Result<Value> {
    uasf_json::from_slice(&fs::read(path).map_err(|_|
                            denied("deletion_state_unavailable"))?).map_err(|_|
            denied("deletion_state_unavailable"))
}
pub fn binding(resource: &Path, expected: Option<&Path>, required: bool)
    -> Result<Option<PathBuf>> {
    let resource = absolute(resource)?;
    let mut marker = resource.as_os_str().to_os_string();
    marker.push(".deletion-binding.json");
    let marker = PathBuf::from(marker);
    let meta =
        match fs::symlink_metadata(&marker) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound =>
                return if required || expected.is_some() {
                        Err(denied("deletion_binding_required"))
                    } else { Ok(None) },
            Err(_) => return Err(denied("deletion_state_unavailable")),
        };
    let uid = unsafe { libc::geteuid() };
    if !meta.is_file() || meta.file_type().is_symlink() || meta.uid() != uid
            || meta.mode() & 0o077 != 0 {
        return Err(denied("invalid_deletion_binding"))
    }
    let value = json(&marker)?;
    if value["version"] != 1 || value["kind"] != "intake" ||
                    value["resource"] != resource.to_string_lossy().as_ref() ||
                value["owner_uid"] != uid ||
            expected.map(absolute).transpose()?.is_some_and(|e|
                    value["root"] != e.to_string_lossy().as_ref()) {
        return Err(denied("deletion_binding_mismatch"))
    }
    let root =
        PathBuf::from(string(&value,
                        "root").map_err(|_| denied("deletion_binding_mismatch"))?);
    if !root.is_absolute() || absolute(&root)? != root {
        return Err(denied("deletion_binding_mismatch"))
    }
    Ok(Some(root))
}
fn open_lock(root: &Path) -> Result<File> {
    let f =
        OpenOptions::new().read(true).write(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW
                            |
                            libc::O_CLOEXEC).open(root.join("lock")).map_err(|_|
                    denied("deletion_state_unavailable"))?;
    if !f.metadata()?.is_file() {
        return Err(denied("deletion_state_unavailable"))
    }
    Ok(f)
}
fn flock(f: &File) -> Result<()> {
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } <
            0 {
        let e = std::io::Error::last_os_error();
        return Err(denied(if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
                        "pending_deletion"
                    } else { "deletion_state_unavailable" }))
    }
    Ok(())
}
fn state(root: &Path, resource: &Path) -> Result<Value> {
    let s = json(&root.join("jobs.json"))?;
    if s["version"] != 1 ||
            s["binding"]["intake"] != resource.to_string_lossy().as_ref() {
        return Err(denied("invalid_deletion_journal"))
    }
    let jobs =
        s["jobs"].as_object().ok_or(denied("deletion_state_unavailable"))?;
    for j in jobs.values() {
        if !j["status"].is_string() {
            return Err(denied("deletion_state_unavailable"))
        }
        if j["status"] == "complete" || j["status"] == "done" {
            let keys =
                j["keys"].as_array().ok_or(denied("deletion_state_unavailable"))?;
            if keys.iter().any(|k|
                        k.as_array().is_none_or(|a|
                                a.len() != 2 || !a[0].is_string() ||
                                    a[1].as_str().is_none_or(|h|
                                            h.len() != 64 ||
                                                !h.bytes().all(|b| b.is_ascii_hexdigit())))) {
                return Err(denied("deletion_state_unavailable"));
            }
            let sel = &j["selector"];
            match sel["kind"].as_str() {
                Some("session") if
                    sel["source"].is_string() && sel["session"].is_string() =>
                    {}
                Some("text") if
                    sel["length"].as_u64().is_some() &&
                        sel["sha256"].as_str().is_some_and(|h|
                                h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                    => {}
                Some("file" | "receipt") => {}
                _ => return Err(denied("deletion_state_unavailable")),
            }
        }
    }
    Ok(s)
}
pub struct Access {
    pub jobs: Value,
    _lock: Option<File>,
}
pub fn access(resource: &Path) -> Result<Access> {
    let resource = absolute(resource)?;
    let Some(root) =
        binding(&resource, None,
                false)? else {
            return Ok(Access { jobs: Value::Null, _lock: None })
        };
    let lock = open_lock(&root)?;
    flock(&lock)?;
    let jobs = state(&root, &resource)?;
    if jobs["jobs"].as_object().unwrap().values().any(|j|
                j["status"] != "complete" && j["status"] != "done") {
        return Err(denied("pending_deletion"))
    }
    Ok(Access { jobs, _lock: Some(lock) })
}
/// Duplicate the inherited capability; never unlock it on drop (the parent owns it).
/// Even a shared lock on a separate open must be blocked; the inherited description must
/// accept LOCK_EX without waiting. Merely naming an fd or opening the lock is insufficient.
pub fn coordinator(resource: &Path, fd: Option<i32>) -> Result<Option<File>> {
    let Some(root) =
        binding(resource, None,
                false)? else {
            return if fd.is_none() {
                    Ok(None)
                } else { Err(denied("coordinator_required")) }
        };
    let fd = fd.filter(|fd| *fd >= 3).ok_or(denied("coordinator_required"))?;
    // The inherited descriptor itself must not leak to a later exec either.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(denied("coordinator_required"));
    }
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if copy < 0 { return Err(denied("coordinator_required")) }
    let inherited = unsafe { File::from_raw_fd(copy) };
    let probe = open_lock(&root)?;
    let a = inherited.metadata()?;
    let b = probe.metadata()?;
    if !a.is_file() || (a.dev(), a.ino()) != (b.dev(), b.ino()) {
        return Err(denied("coordinator_required"))
    }
    // A shared probe distinguishes an existing exclusive lease from a shared
    // lease. Never silently upgrade a caller's shared lock into a capability.
    if unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0
        || std::io::Error::last_os_error().raw_os_error() != Some(libc::EWOULDBLOCK) {
        return Err(denied("coordinator_required"));
    }
    flock(&inherited).map_err(|_| denied("coordinator_required"))?;
    state(&root, &absolute(resource)?)?;
    Ok(Some(inherited))
}
fn docs(data: &[u8]) -> (Vec<Value>, bool) {
    if data.is_empty() { return (vec![], false) }
    if let Ok(v) = uasf_json::from_slice(data) { return (vec![v], false) }
    let mut values = Vec::new();
    let mut invalid = false;
    for line in data.split(|b| *b == b'\n' || *b == b'\r') {
        if line.iter().all(u8::is_ascii_whitespace) { continue }
        match uasf_json::from_slice(line) {
            Ok(v) => values.push(v),
            Err(_) => invalid = true,
        }
    }
    (values, invalid)
}
fn contains(data: &[u8], sel: &Value) -> Result<bool> {
    let n =
        sel["length"].as_u64().and_then(|n|
                        usize::try_from(n).ok()).ok_or(denied("deletion_state_unavailable"))?;
    let hash =
        string(sel,
                    "sha256").map_err(|_|
                    denied("deletion_state_unavailable"))?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(denied("deletion_state_unavailable"))
    }
    let matches =
        |v: &[u8]|
            if n == 0 {
                sha(b"") == hash
            } else { v.windows(n).any(|w| sha(w) == hash) };
    if matches(data) { return Ok(true) }
    let (mut pending, invalid) = docs(data);
    while let Some(v) = pending.pop() {
        match v {
            Value::String(s) => if matches(s.as_bytes()) { return Ok(true) },
            Value::Array(a) => pending.extend(a),
            Value::Object(o) =>
                for (k, v) in o {
                    if matches(k.as_bytes()) { return Ok(true) }
                    pending.push(v)
                },
            _ => {}
        }
    }
    if invalid { return Err(denied("deletion_state_unavailable")) }
    Ok(false)
}
pub fn blocked(state: &Value, source: &str, data: &[u8], session: &str,
    key: Option<&str>) -> Result<bool> {
    if state.is_null() { return Ok(false) }
    for j in
        state["jobs"].as_object().ok_or(denied("deletion_state_unavailable"))?.values()
        {
        let keys =
            j["keys"].as_array().ok_or(denied("deletion_state_unavailable"))?;
        if let Some(key) = key {
            if keys.contains(&uasf_json::json!([source,sha(key.as_bytes())]))
                {
                return Ok(true)
            }
        }
        let sel = &j["selector"];
        match string(sel,
                        "kind").map_err(|_| denied("deletion_state_unavailable"))? {
            "text" => if contains(data, sel)? { return Ok(true) },
            "session" => {
                let selected_source =
                    string(sel,
                                "source").map_err(|_|
                                denied("deletion_state_unavailable"))?;
                let selected_session =
                    string(sel,
                                "session").map_err(|_|
                                denied("deletion_state_unavailable"))?;
                if selected_source != source { continue }
                if selected_session == session { return Ok(true) }
                let (values, invalid) = docs(data);
                for row in values {
                    if (row["type"] == "session_meta" &&
                                    row["payload"]["id"] == selected_session) ||
                            row["sessionId"] == selected_session {
                        return Ok(true)
                    }
                }
                if invalid {
                    return Err(denied("deletion_state_unavailable"))
                }
            }
            "file" | "receipt" => {}
            _ => return Err(denied("deletion_state_unavailable")),
        }
    }
    Ok(false)
}
