//! Receipt-bound reads through an external archive bridge. No archive crate dependency.
pub mod files;
pub mod transport;
use std::{
    fs, path::{Path, PathBuf},
    process::{Command, Stdio},
    io::{Read, Write},
    thread, time::{Duration, Instant},
};
use uasf_json::{json, Value};
use uasf_core::model::sha;
use files::{private, absolute, relative, Lock};
pub const LIMIT: u64 = 32 * 1024 * 1024;
pub const REQUEST_LIMIT: u64 = 1024 * 1024;
pub const RESPONSE_LIMIT: u64 = 2 * LIMIT + REQUEST_LIMIT;
#[derive(Debug)]
pub struct Error;
pub type Result<T> = std::result::Result<T, Error>;
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self { Self }
}
impl From<uasf_json::Error> for Error {
    fn from(_: uasf_json::Error) -> Self { Self }
}
pub fn rejected(status: &str, reason: &str) -> Value {
    json!({"status":status,"reason":reason})
}
fn unsupported() -> Value { rejected("unsupported", "request not supported") }
fn invalid() -> Value {
    rejected("unsupported",
        "invalid request, configuration, or unavailable verification")
}
fn string(v: &Value) -> Result<&str> { v.as_str().ok_or(Error) }
fn array(v: &Value) -> Result<&Vec<Value>> { v.as_array().ok_or(Error) }
fn object(v: &Value) -> Result<&uasf_json::Map<String, Value>> {
    v.as_object().ok_or(Error)
}
fn integer(v: &Value) -> Option<u64> { v.as_u64() }
fn denied_bridge(v: &Value) -> Value {
    let s =
        match v["status"].as_str() {
            Some("excised") => "deleted",
            Some("mis_captured" | "content_changed") => "mismatch",
            Some("out_of_scope") => "out_of_scope",
            Some("unsupported") => "unsupported",
            _ => "unverified",
        };
    rejected(s, "archive did not satisfy verification")
}
// The archive protocol follows JSON number equality, while bools remain invalid
// in fields explicitly requiring an integer (length, ordinal, range offsets).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) =>
            a.len() == b.len() &&
                a.iter().all(|(k, v)| b.get(k).is_some_and(|w| same(v, w))),
        (Value::Array(a), Value::Array(b)) =>
            a.len() == b.len() && a.iter().zip(b).all(|(v, w)| same(v, w)),
        (Value::Bool(a), Value::Number(_)) => same(&json!(u8::from(*a)), b),
        (Value::Number(_), Value::Bool(b)) => same(a, &json!(u8::from(*b))),
        (Value::Number(a), Value::Number(b)) => {
            match (a.is_f64(), b.is_f64()) {
                (false, false) => a.to_string() == b.to_string(),
                (true, true) => a.as_f64() == b.as_f64(),
                (true, false) =>
                    a.as_f64().is_some_and(|f|
                            f.fract() == 0.0 && format!("{f:.0}") == b.to_string()),
                (false, true) =>
                    b.as_f64().is_some_and(|f|
                            f.fract() == 0.0 && format!("{f:.0}") == a.to_string()),
            }
        }
        _ => a == b,
    }
}
pub trait Bridge {
    fn call(&self, request: &Value)
    -> Result<Value>;
}
pub struct ProcessBridge {
    pub binary: PathBuf,
    pub registry: PathBuf,
}
impl Bridge for ProcessBridge {
    fn call(&self, request: &Value) -> Result<Value> {
        let mut cmd = Command::new(&self.binary);
        cmd.args(["--registry", self.registry.to_str().ok_or(Error)?,
                                            "--allow-unverified-raw"]).env_clear().env("PATH",
                            "/usr/bin:/bin").env("LANG",
                        "C.UTF-8").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(tmp) = std::env::var_os("TMPDIR") {
            private(Path::new(&tmp), true)?;
            cmd.env("TMPDIR", tmp);
        }
        let mut child = cmd.spawn()?;
        let bytes = uasf_json::to_vec(request)?;
        let mut input = child.stdin.take().ok_or(Error)?;
        let writer = thread::spawn(move || input.write_all(&bytes));
        let output = child.stdout.take().ok_or(Error)?;
        let errors = child.stderr.take().ok_or(Error)?;
        let out =
            thread::spawn(move ||
                    {
                        let mut b = Vec::new();
                        output.take(RESPONSE_LIMIT +
                                        1).read_to_end(&mut b).map(|_| b)
                    });
        let err =
            thread::spawn(move ||
                    {
                        let mut b = Vec::new();
                        errors.take(REQUEST_LIMIT +
                                        1).read_to_end(&mut b).map(|_| b)
                    });
        let start = Instant::now();
        let mut timed_out = false;
        let status =
            loop {
                if let Some(status) = child.try_wait()? { break status; }
                if start.elapsed() > Duration::from_secs(60) {
                    timed_out = true;
                    child.kill()?;
                    break child.wait()?;
                }
                thread::sleep(Duration::from_millis(5));
            };
        let write = writer.join().map_err(|_| Error)?;
        let bytes = out.join().map_err(|_| Error)??;
        let _ = err.join().map_err(|_| Error)??;
        if timed_out || !status.success() || write.is_err() ||
                bytes.len() as u64 > RESPONSE_LIMIT {
            return Err(Error);
        }
        let v = uasf_json::from_slice(&bytes)?;
        object(&v)?;
        Ok(v)
    }
}
pub struct Gate<B: Bridge> {
    pub bridge: B,
    pub registry: PathBuf,
    pub staging: PathBuf,
    pub sources: Value,
    pub source: String,
}
impl<B: Bridge> Gate<B> {
    pub fn config(&self) -> Result<()> {
        private(&self.staging, true)?;
        private(&self.staging.join("state.json"), false)?;
        private(&self.registry, false)?;
        if self.sources["version"] != 1 { return Err(Error); }
        absolute(&self.staging)?;
        for config in object(&self.sources["sources"])?.values() {
            let root = Path::new(string(&config["root"])?);
            absolute(root)?;
            if root.starts_with(&self.staging) ||
                        self.staging.starts_with(root) ||
                    !matches!(config["kind"].as_str(),Some("claude"|"codex")) {
                return Err(Error);
            }
        }
        let config = files::json(&self.registry)?;
        if config["version"] != 1 { return Err(Error); }
        let selected = &config["sources"][&self.source];
        let archive = Path::new(string(&selected["archive"])?);
        private(archive, true)?;
        private(&archive.join("archive.db"), false)?;
        let roots: std::collections::BTreeSet<_> =
            array(&selected["collection_roots"])?.iter().map(|v|
                            string(v).map(str::to_owned)).collect::<Result<_>>()?;
        let expected =
            std::collections::BTreeSet::from([self.staging.join("claude").to_string_lossy().into_owned(),
                        self.staging.join("codex").to_string_lossy().into_owned()]);
        if roots != expected { return Err(Error); }
        Ok(())
    }
    pub fn handle(&self, request: &Value) -> Value {
        if private(&self.staging, true).is_err() { return invalid(); }
        let _lease = match uasf_core::admission::acquire_gate(&self.staging) {
            Ok(lease) => lease,
            Err(uasf_core::admission::Refusal::Capacity) => return rejected("unsupported", "query_capacity_exceeded"),
            Err(_) => return rejected("unsupported", "query_admission_unavailable"),
        };
        self.handle_admitted(request, &_lease)
    }
    pub fn handle_admitted(&self, request: &Value, _lease: &uasf_core::admission::Lease) -> Value {
        self.inner(request).unwrap_or_else(|_| invalid())
    }
    // Keep the deletion queue lease alive through all bridge calls and delivery checks.
    fn queue(&self) -> Result<Option<Lock>> {
        let marker = self.staging.join("deletion-binding.json");
        if !marker.exists() { return Ok(None); }
        private(&marker, false)?;
        let b = files::json(&marker)?;
        if b["version"] != 1 || b["kind"] != "staging" ||
                    b["resource"] != self.staging.to_string_lossy().as_ref() ||
                b["owner_uid"] != files::uid() {
            return Err(Error);
        }
        let root = Path::new(string(&b["root"])?);
        let lock = Lock::acquire(&root.join("lock"), false)?;
        let state = files::json(&root.join("jobs.json"))?;
        if state["version"] != 1 ||
                    state["binding"]["staging"] !=
                        self.staging.to_string_lossy().as_ref() ||
                object(&state["jobs"])?.values().any(|j|
                        !matches!(j["status"].as_str(),Some("complete"|"done"))) {
            return Err(Error);
        }
        Ok(Some(lock))
    }
    fn recover(&self, state: &mut Value) -> Result<()> {
        let mut changed = false;
        let keys: Vec<_> =
            object(&state["receipts"])?.keys().cloned().collect();
        for generation in keys {
            let r = state["receipts"][&generation].clone();
            if !matches!(r["status"].as_str(),Some("pending"|"deleting")) {
                continue;
            }
            let rel = Path::new(string(&r["staging_relative"])?);
            relative(rel)?;
            let final_path = self.staging.join(rel);
            if r["status"] == "deleting" {
                match fs::remove_file(&final_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(Error),
                }
                fs::File::open(final_path.parent().ok_or(Error)?)?.sync_all()?;
                state["receipts"][&generation] =
                    json!({"status":"deleted","generation":generation,"staging_relative":r["staging_relative"]});
            } else {
                let temporary = Path::new(string(&r["temporary"])?);
                relative(temporary)?;
                let temp = self.staging.join(temporary);
                if !final_path.exists() && temp.exists() {
                    fs::rename(&temp, &final_path)?;
                    fs::File::open(final_path.parent().ok_or(Error)?)?.sync_all()?;
                }
                if final_path.exists() {
                    let data = files::snapshot(&self.staging, rel)?;
                    if integer(&r["length"]) != Some(data.len() as u64) ||
                            r["sha256"] != sha(&data) {
                        return Err(Error);
                    }
                    state["receipts"][&generation]["status"] = json!("active");
                    state["receipts"][&generation].as_object_mut().ok_or(Error)?.remove("temporary");
                } else {
                    state["receipts"].as_object_mut().ok_or(Error)?.remove(&generation);
                }
            }
            changed = true;
        }
        if changed { files::atomic_json(&self.staging, state)?; }
        for entry in fs::read_dir(self.staging.join("tmp"))? {
            let path = entry?.path();
            let rel =
                path.strip_prefix(&self.staging).map_err(|_|
                                Error)?.to_string_lossy();
            if !object(&state["receipts"])?.values().any(|r|
                            r["temporary"] == rel.as_ref()) {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }
    fn inner(&self, request: &Value) -> Result<Value> {
        let Some(fields) =
            request.as_object() else { return Ok(unsupported()); };
        let op = request["op"].as_str().unwrap_or("");
        if !["read", "resolve", "anchor"].contains(&op) {
            return Ok(unsupported());
        }
        if fields.keys().any(|k|
                    !["op", "generation", "reference"].contains(&k.as_str()) &&
                            !(op == "anchor" && k == "ordinal") &&
                        !(op == "resolve" && k == "expected_sha256")) {
            return Ok(rejected("unsupported",
                        "consumer cannot supply receipts, paths, registries, or raw options"));
        }
        let Some(generation) =
            request["generation"].as_str().filter(|s|
                    !s.is_empty() &&
                        s.chars().count() <=
                            100) else { return Ok(unsupported()); };
        self.config()?;
        let lease = self.queue();
        if lease.is_err() {
            let state = files::json(&self.staging.join("state.json"))?;
            return Ok(if state["receipts"].get(generation).is_some() {
                        rejected("deleted", "staging tombstone")
                    } else {
                        rejected("unverified", "no trusted staging receipt")
                    });
        }
        let _lease = lease?;
        let _lock = Lock::acquire(&self.staging.join("lock"), true)?;
        let mut state = files::json(&self.staging.join("state.json"))?;
        if state["version"] != 1 { return Err(Error); }
        self.recover(&mut state)?;
        let Some(r) =
            state["receipts"].get(generation) else {
                return Ok(rejected("unverified",
                            "no trusted staging receipt"));
            };
        if matches!(r["status"].as_str(),Some("deleted"|"deleting")) {
            return Ok(rejected("deleted", "staging tombstone"));
        }
        if r["status"] != "active" {
            return Ok(rejected("unverified", "receipt is not active"));
        }
        if r["generation"] != generation {
            return Ok(rejected("mismatch", "receipt generation binding"));
        }
        let Some(length) =
            integer(&r["length"]).filter(|n|
                    *n <=
                        LIMIT) else {
                return Ok(rejected("unsupported",
                            "full verification exceeds supported size"));
            };
        let source = string(&r["source"])?;
        let Some(config) =
            self.sources["sources"].get(source) else {
                return Ok(rejected("out_of_scope",
                            "unregistered receipt source"));
            };
        let original = Path::new(string(&r["original_path"])?);
        if absolute(original).is_err() ||
                !original.starts_with(Path::new(string(&config["root"])?)) {
            return Ok(rejected("out_of_scope",
                        "receipt original source binding"));
        }
        let rel = Path::new(string(&r["staging_relative"])?);
        let kind = string(&config["kind"])?;
        let parts: Vec<_> = rel.components().map(|s| s.as_os_str()).collect();
        if r["kind"] != kind || relative(rel).is_err() || parts.len() != 4 ||
                        parts[0] != kind ||
                    parts[1] !=
                        (if kind == "codex" { "sessions" } else { "projects" }) ||
                parts[2] != generation {
            return Ok(rejected("out_of_scope", "receipt staging binding"));
        }
        let path = self.staging.join(rel);
        private(&path, false)?;
        let content =
            match files::snapshot(&self.staging, rel) {
                Ok(v) => v,
                Err(_) =>
                    return Ok(rejected("unverified",
                                "staging copy unavailable or unstable")),
            };
        if content.len() as u64 != length || r["sha256"] != sha(&content) {
            return Ok(rejected("mismatch",
                        "staging bytes differ from independent receipt"));
        }
        let listing =
            self.bridge.call(&json!({"op":"list","source":self.source}))?;
        if listing["status"] != "ok" { return Ok(denied_bridge(&listing)); }
        let observations: Vec<_> =
            array(&listing["observations"])?.iter().filter(|o|
                        o["path"] == path.to_string_lossy().as_ref()).collect();
        if observations.is_empty() {
            return Ok(rejected("unverified", "staging copy not captured"));
        }
        if observations.iter().any(|o| o["class"] == "excised") {
            return Ok(rejected("deleted", "archive tombstone"));
        }
        let first = observations[0];
        for k in ["input", "revision"] { first.get(k).ok_or(Error)?; }
        for k in ["archive_id", "source_binding"] {
            listing.get(k).ok_or(Error)?;
        }
        if observations.iter().any(|o|
                    o["input"] != first["input"] ||
                        o["revision"] != first["revision"]) {
            return Ok(rejected("unverified",
                        "ambiguous immutable staging observation"));
        }
        let item =
            array(&listing["inputs"])?.iter().find(|i|
                            i["input"] == first["input"]).ok_or(Error)?;
        let rev =
            array(&item["revisions"])?.iter().find(|r|
                            r["number"] == first["revision"]).ok_or(Error)?;
        let reference =
            json!({"source":self.source,"archive_id":listing["archive_id"],"source_binding":listing["source_binding"],"input":first["input"],"revision":first["revision"],"chain":rev["chain"],"start":0,"end":length});
        if rev["length"] != length || first["revision"].is_null() ||
                rev["chain"].is_null() {
            return Ok(rejected("mismatch",
                        "full revision length or binding"));
        }
        if fields.contains_key("reference") &&
                !same(&request["reference"], &reference) {
            return Ok(rejected("mismatch",
                        "saved reference binding does not match receipt"));
        }
        let checked =
            self.bridge.call(&json!({"op":"verify_capture","reference":reference,"expected_length":length,"expected_sha256":r["sha256"]}))?;
        if checked["status"] != "ok" { return Ok(denied_bridge(&checked)); }
        if !same(&checked["reference"], &reference) ||
                    checked["length"] != length ||
                checked["sha256"] != r["sha256"] {
            return Ok(rejected("mismatch", "verification response binding"));
        }
        let raw =
            self.bridge.call(&json!({"op":"read","reference":reference}))?;
        if raw["status"] != "ok" { return Ok(denied_bridge(&raw)); }
        let data = decode_hex(string(&raw["hex"])?)?;
        if !same(&raw["reference"], &reference) || data.len() as u64 != length
                    || sha(&data) != r["sha256"] || data != content {
            return Ok(rejected("mismatch",
                        "returned full bytes differ from trusted receipt"));
        }
        let mut response =
            json!({"reference":reference,"length":length,"sha256":sha(&data)});
        if op == "read" {
            response["hex"] = json!(encode_hex(&data));
        } else if op == "resolve" {
            if request["expected_sha256"] != r["sha256"] {
                return Ok(rejected("mismatch", "resolve digest mismatch"));
            }
        } else {
            let Some(ordinal) =
                integer(&request["ordinal"]) else {
                    return Ok(unsupported());
                };
            let anchor =
                self.bridge.call(&json!({"op":"anchor","reference":reference,"path":path,"ordinal":ordinal}))?;
            if anchor["status"] != "ok" { return Ok(denied_bridge(&anchor)); }
            let ar = &anchor["reference"];
            let bounds = integer(&ar["start"]).zip(integer(&ar["end"]));
            let mut full = ar.clone();
            object(&full)?;
            full["start"] = json!(0);
            full["end"] = json!(length);
            if bounds.is_none_or(|(s, e)| s >= e || e > length) ||
                        !same(&full, &reference) || anchor["ordinal"] != ordinal {
                return Ok(rejected("mismatch", "anchor binding or bytes"));
            }
            let (start, end) = bounds.unwrap();
            if anchor["sha256"] != sha(&data[start as usize..end as usize]) {
                return Ok(rejected("mismatch", "anchor binding or bytes"));
            }
            response =
                json!({"reference":ar,"length":end-start,"sha256":anchor["sha256"],"ordinal":ordinal,"window_id":anchor.get("window_id").ok_or(Error)?});
        }
        let deletion =
            self.bridge.call(&json!({"op":"excisions","source":self.source}))?;
        if deletion["status"] != "ok" { return Ok(denied_bridge(&deletion)); }
        if deletion["invalidate_all"] != false {
            return Ok(rejected("deleted",
                        "archive excision invalidates delivery"));
        }
        response["status"] = json!("verified");
        response["generation"] = json!(generation);
        response["capture_verification"] =
            json!("trusted_staging_full_prefix_and_returned_bytes");
        response["capture_time_verified"] = json!(false);
        response["deletion_check"] =
            json!("read_snapshot_not_delivery_lease");
        let mut provenance = json!({});
        for k in
            ["source", "generation", "file_generation", "relation", "length",
                    "sha256"] {
            provenance[k] = r.get(k).ok_or(Error)?.clone();
        }
        response["original_provenance"] = provenance;
        Ok(response)
    }
}
pub fn decode_hex(s: &str) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut it = s.bytes();
    while let Some(b) = it.next() {
        if b.is_ascii_whitespace() { continue; }
        let second = it.next().ok_or(Error)?;
        let n = (b as char).to_digit(16).ok_or(Error)?;
        let m = (second as char).to_digit(16).ok_or(Error)?;
        result.push((n * 16 + m) as u8);
    }
    Ok(result)
}
pub fn encode_hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}
