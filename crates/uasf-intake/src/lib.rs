//! Durable, source-scoped submission of observed bytes. No provider calls.
pub mod policy;
use base64::{
    engine::general_purpose::{STANDARD, GeneralPurpose, GeneralPurposeConfig},
    alphabet, Engine,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use uasf_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap, fs::File, io::Read, path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub enum Error {
    Denied(&'static str),
    Invalid,
    Io(std::io::Error),
    Sql(rusqlite::Error),
    Json(uasf_json::Error),
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self { Self::Sql(e) }
}
impl From<uasf_json::Error> for Error {
    fn from(e: uasf_json::Error) -> Self { Self::Json(e) }
}
impl Error {
    pub fn response(&self) -> Value {
        status(match self {
                Self::Denied(s) => s,
                Self::Invalid | Self::Json(_) => "invalid_request",
                _ => "intake_unavailable",
            })
    }
}
pub fn status(s: &str) -> Value { json!({"status":s}) }
pub fn sha(data: &[u8]) -> String { format!("{:x}", Sha256::digest(data)) }
pub fn canonical(v: &Value) -> Result<String> { Ok(uasf_json::canonical(v)) }
fn uuid() -> Result<String> {
    let mut b = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h: String = b.iter().map(|v| format!("{v:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20],
            &h[20..]))
}
fn sync_parent(path: &Path) -> Result<()> {
    File::open(path.parent().ok_or(Error::Invalid)?)?.sync_all()?;
    Ok(())
}
pub fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key).and_then(Value::as_str).ok_or(Error::Invalid)
}
fn sequence_number(v: &Value) -> Option<String> {
    let s = v.as_number()?.to_string();
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        Some(s)
    } else { None }
}
fn optional<'a>(v: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        _ => Err(Error::Invalid),
    }
}
fn fields(v: &Value, allowed: &[&str]) -> Result<()> {
    if v.as_object().ok_or(Error::Invalid)?.keys().any(|k|
                !allowed.contains(&k.as_str())) {
        return Err(Error::Invalid)
    }
    Ok(())
}

/// The principal comes from a trusted local caller, never from submitted JSON.
pub struct Intake {
    pub path: PathBuf,
    db: Connection,
    grants: BTreeMap<String, Vec<String>>,
}
impl Intake {
    pub fn open(path: &Path, grants: Value) -> Result<Self> {
        let path = policy::absolute(path)?;
        if !path.parent().is_some_and(Path::is_dir) {
            return Err(Error::Invalid)
        }
        let grants = uasf_json::from_value(grants)?;
        let db = Connection::open(&path)?;
        db.busy_timeout(Duration::from_secs(30))?;
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA secure_delete=ON;
            CREATE TABLE IF NOT EXISTS observation (
              receipt_id TEXT PRIMARY KEY, source TEXT NOT NULL, idem_key TEXT, session TEXT NOT NULL,
              envelope TEXT NOT NULL, payload BLOB NOT NULL, receipt TEXT NOT NULL, UNIQUE(source, idem_key));
            CREATE TRIGGER IF NOT EXISTS immutable_update BEFORE UPDATE ON observation
              BEGIN SELECT RAISE(ABORT, 'immutable observation'); END;
            CREATE TABLE IF NOT EXISTS deletion_tombstone (
              receipt_id TEXT PRIMARY KEY, source TEXT NOT NULL, key_hash TEXT, UNIQUE(source, key_hash));")?;
        sync_parent(&path)?;
        Ok(Self { path, db, grants })
    }
    fn authorized(&self, source: &str, principal: &str) -> bool {
        self.grants.get(source).is_some_and(|g|
                g.iter().any(|p| p == principal))
    }
    /// One request, with the deletion lease held until its result is constructed.
    pub fn execute(&mut self, request: &Value, principal: &str)
        -> Result<Value> {
        let op =
            request.get("op").and_then(Value::as_str).unwrap_or("submit");
        if !["submit", "get", "stream_state"].contains(&op) {
            return Ok(status("unsupported"))
        }
        let source = string(request, "source")?;
        if !self.authorized(source, principal) {
            return Ok(status("forbidden"))
        }
        let access =
            match policy::access(&self.path) {
                Ok(a) => a,
                Err(Error::Denied(s)) => return Ok(status(s)),
                Err(e) => return Err(e),
            };
        match op {
            "submit" => {
                fields(request,
                        &["op", "source", "session", "key", "payload_base64",
                                    "representation", "kind", "stream", "sequence"])?;
                let payload =
                    GeneralPurpose::new(&alphabet::STANDARD,
                                    GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true)).decode(string(request,
                                        "payload_base64")?).map_err(|_| Error::Invalid)?;
                let session = string(request, "session")?;
                let key = optional(request, "key")?;
                if policy::blocked(&access.jobs, source, &payload, session,
                            key)? {
                    return Ok(status("deleted"))
                }
                self.submit(request, &payload)
            }
            "get" => {
                fields(request, &["op", "source", "receipt_id"])?;
                let id = string(request, "receipt_id")?;
                let row: Option<(String, Vec<u8>)> =
                    self.db.query_row("SELECT receipt,payload FROM observation WHERE receipt_id=? AND source=?",
                                params![id,source],
                                |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
                if let Some((receipt, payload)) = row {
                    Ok(json!({"status":"ok","receipt":uasf_json::from_str(&receipt)?,"payload_base64":STANDARD.encode(payload)}))
                } else {
                    let exists: bool =
                        self.db.query_row("SELECT EXISTS(SELECT 1 FROM deletion_tombstone WHERE receipt_id=? AND source=?)",
                                params![id,source], |r| r.get(0))?;
                    Ok(status(if exists { "deleted" } else { "not_found" }))
                }
            }
            _ => {
                fields(request, &["op", "source", "session", "stream"])?;
                let session = string(request, "session")?;
                let stream = string(request, "stream")?;
                if policy::blocked(&access.jobs, source, b"", session, None)?
                    {
                    return Ok(status("deleted"))
                }
                let mut statement =
                    self.db.prepare("SELECT receipt FROM observation WHERE source=? AND session=? ORDER BY rowid")?;
                let rows =
                    statement.query_map(params![source,session],
                            |r| r.get::<_, String>(0))?;
                let mut chunks = Vec::new();
                let mut terminal = Value::Null;
                for row in rows {
                    let r: Value = uasf_json::from_str(&row?)?;
                    if r["stream"] != stream { continue }
                    if r["kind"] == "chunk" {
                        chunks.push(r);
                    } else if r["kind"] == "complete" ||
                            r["kind"] == "interrupted" {
                        terminal = r["kind"].clone();
                    }
                }
                chunks.sort_by_cached_key(|v|
                        {
                            let s = sequence_number(&v["sequence"]).unwrap_or_default();
                            (s.len(), s)
                        });
                let contiguous =
                    chunks.iter().enumerate().all(|(i, r)|
                            r["sequence"].as_u64() == Some(i as u64));
                Ok(json!({"status":"ok","stream_state":if terminal=="complete"
                            && contiguous {"complete"}else{"incomplete"},
                            "terminal":terminal,"contiguous":contiguous,"chunks":chunks,"conversation_completeness":"unknown"}))
            }
        }
    }
    fn submit(&mut self, r: &Value, payload: &[u8]) -> Result<Value> {
        let source = string(r, "source")?;
        let session = string(r, "session")?;
        let key = optional(r, "key")?;
        let representation = r.get("representation").unwrap_or(&Value::Null);
        let representation =
            if r.get("representation").is_none() {
                "app_bytes"
            } else { representation.as_str().ok_or(Error::Invalid)? };
        let kind =
            r.get("kind").map(|v|
                                v.as_str().ok_or(Error::Invalid)).transpose()?.unwrap_or("record");
        let stream = optional(r, "stream")?;
        let sequence = r.get("sequence").cloned().unwrap_or(Value::Null);
        if session.is_empty() || key == Some("") ||
                                    !["app_bytes", "sdk_json"].contains(&representation) ||
                                !["record", "chunk", "complete",
                                                "interrupted"].contains(&kind) ||
                            (kind != "record" &&
                                    (stream.is_none() || stream == Some(""))) ||
                        (kind == "record" &&
                                (stream.is_some() || !sequence.is_null())) ||
                    (kind == "chunk" && sequence_number(&sequence).is_none()) ||
                (["complete", "interrupted"].contains(&kind) &&
                        (!payload.is_empty() || !sequence.is_null())) {
            return Err(Error::Invalid)
        }
        let env =
            json!({"session":session,"representation":representation,"kind":kind,"stream":stream,"sequence":sequence});
        let envelope = canonical(&env)?;
        let tx =
            self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(k) = key {
            let deleted: bool =
                tx.query_row("SELECT EXISTS(SELECT 1 FROM deletion_tombstone WHERE source=? AND key_hash=?)",
                        params![source,sha(k.as_bytes())], |r| r.get(0))?;
            if deleted { return Ok(status("deleted")) }
        }
        let prior: Option<(String, String, Vec<u8>, String)> =
            if let Some(k) = key {
                tx.query_row("SELECT receipt_id,envelope,payload,receipt FROM observation WHERE source=? AND idem_key=?",
                            params![source,k],
                            |r|
                                Ok((r.get(0)?, r.get(1)?, r.get(2)?,
                                        r.get(3)?))).optional()?
            } else { None };
        let (receipt, replayed) =
            if let Some((id, e, p, receipt)) = prior {
                if e != envelope || p != payload {
                    return Ok(json!({"status":"conflict","receipt_id":id}))
                }
                (uasf_json::from_str(&receipt)?, true)
            } else {
                if let Some(stream) = stream {
                    let mut statement =
                        tx.prepare("SELECT envelope FROM observation WHERE source=? AND session=?")?;
                    let rows =
                        statement.query_map(params![source,session],
                                |r| r.get::<_, String>(0))?;
                    let mut conflict = false;
                    for row in rows {
                        let e: Value = uasf_json::from_str(&row?)?;
                        if e["stream"] != stream { continue }
                        if e["kind"] == "complete" || e["kind"] == "interrupted" {
                            return Ok(status("stream_closed"))
                        }
                        conflict |=
                            key.is_some() && kind == "chunk" &&
                                e["sequence"] == sequence;
                    }
                    if conflict { return Ok(status("sequence_conflict")) }
                }
                let mut receipt = env;
                let fields =
                    json!({"version":1,"receipt_id":uuid()?,"source":source,"received_ns":u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|
                            Error::Invalid)?.as_nanos()).map_err(|_|
                            Error::Invalid)?,"key":key,"deduplication":if
                            key.is_some(){"keyed"}else{"unknown"},"payload_length":payload.len(),"payload_sha256":sha(payload),"raw_reference":{"owner":"uasf-intake","table":"observation"},"observation_basis":"app_submission","conversation_completeness":"unknown","model_input_completeness":"unknown","normalization":"unsupported","durability":"sqlite-full-commit"});
                receipt.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
                tx.execute("INSERT INTO observation VALUES (?,?,?,?,?,?,?)",
                        params![receipt["receipt_id"].as_str(),source,key,session,envelope,payload,canonical(&receipt)?])?;
                (receipt, false)
            };
        tx.commit()?;
        sync_parent(&self.path)?;
        Ok(json!({"status":"accepted","receipt":receipt,"replayed":replayed}))
    }
    /// Bound deletion requires the coordinator's inherited, already-held flock.
    /// This descriptor is a trusted process capability, never a JSON input field.
    pub fn erase(&mut self, ids: &[String], coordinator_fd: Option<i32>)
        -> Result<Value> {
        let _lease = policy::coordinator(&self.path, coordinator_fd)?;
        let tx =
            self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for id in ids {
            let row: Option<(String, Option<String>)> =
                tx.query_row("SELECT source,idem_key FROM observation WHERE receipt_id=?",
                            [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            if let Some((source, key)) = row {
                tx.execute("INSERT OR IGNORE INTO deletion_tombstone VALUES (?,?,?)",
                        params![id,source,key.map(|k|sha(k.as_bytes()))])?;
                tx.execute("DELETE FROM observation WHERE receipt_id=?",
                        [id])?;
            }
        }
        tx.commit()?;
        sync_parent(&self.path)?;
        Ok(status("deleted"))
    }
    /// Each iterator item commits independently; error text is never preserved.
    pub fn capture_stream<I,
        E>(&mut self, chunks: I, source: &str, session: &str, stream: &str,
        principal: &str, attempt_key: &str) -> Result<Value> where
        I: IntoIterator<Item = std::result::Result<Vec<u8>, E>> {
        if attempt_key.is_empty() { return Err(Error::Invalid) }
        let mut receipts = Vec::new();
        for (sequence, item) in chunks.into_iter().enumerate() {
            let payload =
                match item {
                    Ok(p) => p,
                    Err(_) => {
                        let terminal =
                            self.terminal(source, session, stream, principal,
                                    attempt_key, "interrupted")?;
                        return Ok(json!({"status":"incomplete","chunks":receipts,"terminal":terminal}));
                    }
                };
            let r =
                self.execute(&json!({"source":source,"session":session,"stream":stream,"kind":"chunk","sequence":sequence,"key":canonical(&json!([attempt_key,"chunk",sequence]))?,"payload_base64":STANDARD.encode(payload)}),
                        principal)?;
            if r["status"] != "accepted" { return Ok(r) }
            receipts.push(r["receipt"].clone());
        }
        let terminal =
            self.terminal(source, session, stream, principal, attempt_key,
                    "complete")?;
        Ok(json!({"status":terminal["status"],"chunks":receipts,"terminal":terminal}))
    }
    fn terminal(&mut self, source: &str, session: &str, stream: &str,
        principal: &str, key: &str, kind: &str) -> Result<Value> {
        self.execute(&json!({"source":source,"session":session,"stream":stream,"kind":kind,"key":canonical(&json!([key,"terminal"]))?,"payload_base64":""}),
            principal)
    }
}

#[cfg(test)]
mod crash_tests {
    use super::*;
    use std::{
        ffi::{c_void, CStr},
        process::Command,
    };
    // Test-only SQLite trace callbacks terminate at the actual COMMIT statement.
    // Neither a CLI switch nor a production environment crash hook is exposed.
    unsafe extern "C" fn stop_at_commit(event: u32, context: *mut c_void,
        statement: *mut c_void, _: *mut c_void) -> i32 {
        let mode = context as usize;
        let sql = rusqlite::ffi::sqlite3_sql(statement.cast());
        if !sql.is_null() && CStr::from_ptr(sql).to_bytes() == b"COMMIT" &&
                ((mode == 1 &&
                                event == rusqlite::ffi::SQLITE_TRACE_STMT as u32) ||
                        (mode == 2 &&
                                event == rusqlite::ffi::SQLITE_TRACE_PROFILE as u32)) {
            libc::_exit(if mode == 1 { 71 } else { 72 });
        }
        0
    }
    #[test]
    fn crash_worker() {
        let Ok(path) =
            std::env::var("UASF_INTAKE_TEST_DATABASE") else { return };
        let mode: usize =
            std::env::var("UASF_INTAKE_TEST_PHASE").unwrap().parse().unwrap();
        let mut store =
            Intake::open(Path::new(&path), json!({"A":["app"]})).unwrap();
        unsafe {
            rusqlite::ffi::sqlite3_trace_v2(store.db.handle(),
                (rusqlite::ffi::SQLITE_TRACE_STMT |
                            rusqlite::ffi::SQLITE_TRACE_PROFILE) as u32,
                Some(stop_at_commit), mode as *mut c_void);
        }
        store.execute(&json!({"source":"A","session":"s","key":"fixed","payload_base64":"YQ=="}),
                "app").unwrap();
        panic!("crash callback was not reached");
    }
    fn exercise(mode: usize) {
        let root =
            std::env::temp_dir().join(format!("intake-crash-{}-{mode}",std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("intake.db");
        let child =
            Command::new(std::env::current_exe().unwrap()).args(["--exact",
                                        "crash_tests::crash_worker",
                                        "--nocapture"]).env("UASF_INTAKE_TEST_DATABASE",
                            &path).env("UASF_INTAKE_TEST_PHASE",
                        mode.to_string()).output().unwrap();
        assert_eq!(child.status.code(),Some(if mode==1{71}else{72}));
        assert!(!String::from_utf8_lossy(&child.stdout).contains("accepted"));
        let mut store = Intake::open(&path, json!({"A":["app"]})).unwrap();
        let count: i64 =
            store.db.query_row("SELECT count(*) FROM observation", [],
                    |r| r.get(0)).unwrap();
        assert_eq!(count,if mode==1{0}else{1});
        let result =
            store.execute(&json!({"source":"A","session":"s","key":"fixed","payload_base64":"YQ=="}),
                    "app").unwrap();
        assert_eq!(result["replayed"],mode==2);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn death_before_commit_rolls_back() { exercise(1) }
    #[test]
    fn death_after_commit_before_ack_replays() { exercise(2) }
}
