use uasf_json::{json, Value};
use std::{fs, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};
use uasf_intake::{Intake, sha, policy};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    dir: PathBuf,
    store: Intake,
}
impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("intake-contract-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir(&dir).unwrap();
        let store =
            Intake::open(&dir.join("intake.db"),
                    json!({"A":["app"],"B":["app"]})).unwrap();
        Self { dir, store }
    }
    fn call(&mut self, r: Value) -> Value {
        self.store.execute(&r, "app").unwrap_or_else(|e| e.response())
    }
    fn bind(&self, jobs: Value) {
        use std::os::unix::fs::PermissionsExt;
        let root = self.dir.join("queue");
        fs::create_dir_all(&root).unwrap();
        let marker = self.dir.join("intake.db.deletion-binding.json");
        fs::write(&marker,
                json!({"version":1,"kind":"intake","resource":self.store.path,"root":root,"owner_uid":unsafe{libc::geteuid()}}).to_string()).unwrap();
        fs::set_permissions(marker,
                fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(root.join("jobs.json"),
                json!({"version":1,"binding":{"intake":self.store.path},"jobs":jobs}).to_string()).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.dir); }
}
fn req() -> Value {
    json!({"source":"A","session":"s","key":"k","payload_base64":"/wANCg=="})
}
fn event(kind: &str, n: Value) -> Value {
    json!({"source":"A","session":"s","stream":"stream","kind":kind,"sequence":n,"key":format!("{kind}-{n}"),"payload_base64":if
            kind=="chunk"{"YQ=="}else{""}})
}
#[test]
fn original_bytes_and_receipt() {
    let mut f = Fixture::new();
    let r = f.call(req());
    assert_eq!(r["status"],"accepted");
    let receipt = &r["receipt"];
    assert_eq!(receipt["payload_sha256"],sha(b"\xff\0\r\n"));
    assert_eq!(receipt["payload_length"],4);
    for k in ["principal", "account", "model", "path", "token"] {
        assert!(receipt.get(k).is_none());
    }
    assert_eq!(receipt["conversation_completeness"],"unknown");
    let get =
        f.call(json!({"op":"get","source":"A","receipt_id":receipt["receipt_id"]}));
    assert_eq!(get["payload_base64"],"/wANCg==");
    assert_eq!(get["receipt"],*receipt);
}
#[test]
fn retry_restart_preserves_receipt() {
    let mut f = Fixture::new();
    let r = f.call(req());
    f.store = Intake::open(&f.store.path, json!({"A":["app"]})).unwrap();
    let again = f.call(req());
    assert_eq!(r["receipt"],again["receipt"]);
    assert_eq!(again["replayed"],true);
}
#[test]
fn conflict_preserves_original() {
    let mut f = Fixture::new();
    let first = f.call(req());
    for (k, v) in
        [("payload_base64", json!("Yg==")), ("session", json!("other")),
                ("representation", json!("sdk_json"))] {
        let mut r = req();
        r[k] = v;
        assert_eq!(f.call(r),json!({"status":"conflict","receipt_id":first["receipt"]["receipt_id"]}));
    }
    assert_eq!(f.call(req())["receipt"],first["receipt"]);
}
#[test]
fn scope_and_permission() {
    let mut f = Fixture::new();
    let a = f.call(req());
    let mut r = req();
    r["source"] = json!("B");
    let b = f.call(r);
    assert_ne!(a["receipt"]["receipt_id"],b["receipt"]["receipt_id"]);
    assert_eq!(f.call(json!({"op":"get","source":"B","receipt_id":a["receipt"]["receipt_id"]}))["status"],"not_found");
    assert_eq!(f.store.execute(&req(),"stranger").unwrap()["status"],"forbidden");
    let mut r = req();
    r["source"] = json!("unknown");
    assert_eq!(f.call(r)["status"],"forbidden");
}
#[test]
fn keyless_is_not_content_deduplication() {
    let mut f = Fixture::new();
    let mut r = req();
    r.as_object_mut().unwrap().remove("key");
    let a = f.call(r.clone());
    let b = f.call(r);
    assert_ne!(a["receipt"]["receipt_id"],b["receipt"]["receipt_id"]);
    assert_eq!(a["receipt"]["deduplication"],"unknown");
}
fn state(f: &mut Fixture) -> Value {
    f.call(json!({"op":"stream_state","source":"A","session":"s","stream":"stream"}))
}
#[test]
fn unfinished_survives_restart() {
    let mut f = Fixture::new();
    f.call(event("chunk", json!(0)));
    f.store = Intake::open(&f.store.path, json!({"A":["app"]})).unwrap();
    let s = state(&mut f);
    assert_eq!(s["stream_state"],"incomplete");
    assert_eq!(s["terminal"],Value::Null);
    assert_eq!(s["chunks"].as_array().unwrap().len(),1);
}
#[test]
fn missing_sequence_never_complete() {
    let mut f = Fixture::new();
    f.call(event("chunk", json!(1)));
    f.call(event("complete", Value::Null));
    assert_eq!(state(&mut f)["stream_state"],"incomplete");
    assert_eq!(state(&mut f)["contiguous"],false);
}
#[test]
fn closed_stream_replay_and_sequence_conflict() {
    let mut f = Fixture::new();
    let first = f.call(event("chunk", json!(0)));
    let mut r = event("chunk", json!(0));
    r["key"] = json!("different");
    assert_eq!(f.call(r)["status"],"sequence_conflict");
    f.call(event("complete", Value::Null));
    assert_eq!(state(&mut f)["stream_state"],"complete");
    assert_eq!(f.call(event("chunk",json!(1)))["status"],"stream_closed");
    assert_eq!(f.call(event("chunk",json!(0)))["receipt"],first["receipt"]);
}
#[test]
fn keyless_duplicate_chunk_is_ambiguous() {
    let mut f = Fixture::new();
    let mut r = event("chunk", json!(0));
    r["key"] = Value::Null;
    f.call(r.clone());
    f.call(r);
    f.call(event("complete", Value::Null));
    let s = state(&mut f);
    assert_eq!(s["chunks"].as_array().unwrap().len(),2);
    assert_eq!(s["stream_state"],"incomplete");
}
#[test]
fn interrupted_capture_does_not_store_error() {
    let mut f = Fixture::new();
    let out =
        f.store.capture_stream(vec![Ok(b"a".to_vec()),Ok(b"partial".to_vec()),Err("SYNTHETIC-ERROR-MUST-NOT-BE-STORED")],
                "A", "s", "stream", "app", "attempt").unwrap();
    assert_eq!(out["status"],"incomplete");
    let s = state(&mut f);
    assert_eq!(s["terminal"],"interrupted");
    assert_eq!(s["chunks"].as_array().unwrap().len(),2);
    let b = fs::read(&f.store.path).unwrap();
    assert!(!b.windows(b"SYNTHETIC-ERROR".len()).any(|w|w==b"SYNTHETIC-ERROR"));
}
#[test]
fn capture_retries_exactly() {
    let mut f = Fixture::new();
    let chunks = vec![Ok::<_,()>(b"a".to_vec()),Ok(b"b".to_vec())];
    let a =
        f.store.capture_stream(chunks.clone(), "A", "s", "stream", "app",
                "fixed").unwrap();
    let b =
        f.store.capture_stream(chunks, "A", "s", "stream", "app",
                "fixed").unwrap();
    assert_eq!(a["chunks"],b["chunks"]);
    assert_eq!(b["terminal"]["replayed"],true);
}
#[test]
fn validation_and_no_arbitrary_metadata() {
    let mut f = Fixture::new();
    for (k, v) in
        [("session", json!("")), ("key", json!("")),
                ("representation", json!("network_bytes")),
                ("payload_base64", json!("?")),
                ("Authorization", json!("synthetic")),
                ("stream", json!("bad"))] {
        let mut r = req();
        r[k] = v;
        assert_eq!(f.call(r)["status"],"invalid_request");
    }
    for n in [json!(-1), json!(true), json!(0.5), Value::Null] {
        assert_eq!(f.call(event("chunk",n))["status"],"invalid_request");
    }
    let mut r = event("complete", Value::Null);
    r["payload_base64"] = json!("YQ==");
    assert_eq!(f.call(r)["status"],"invalid_request");
}
#[test]
fn immutable_schema_and_full_commit() {
    let mut f = Fixture::new();
    f.call(req());
    let db = rusqlite::Connection::open(&f.store.path).unwrap();
    assert!(db.execute("UPDATE observation SET payload=x'00'",[]).is_err());
    let n: i64 =
        db.query_row("SELECT count(*) FROM observation", [],
                |r| r.get(0)).unwrap();
    assert_eq!(n,1);
}
#[test]
fn tombstones_are_source_scoped_and_idempotent() {
    let mut f = Fixture::new();
    let r = f.call(req());
    let id = r["receipt"]["receipt_id"].as_str().unwrap().to_string();
    assert_eq!(f.store.erase(&[id.clone(),"absent".into()],None).unwrap()["status"],"deleted");
    assert_eq!(f.store.erase(&[id.clone()],None).unwrap()["status"],"deleted");
    assert_eq!(f.call(json!({"op":"get","source":"A","receipt_id":id}))["status"],"deleted");
    let mut r = req();
    r["payload_base64"] = json!("Yg==");
    assert_eq!(f.call(r.clone())["status"],"deleted");
    r["source"] = json!("B");
    assert_eq!(f.call(r)["status"],"accepted");
    let db = rusqlite::Connection::open(&f.store.path).unwrap();
    let key: String =
        db.query_row("SELECT key_hash FROM deletion_tombstone", [],
                |r| r.get(0)).unwrap();
    assert_eq!(key,sha(b"k"));
}
#[test]
fn concurrent_retry_one_receipt() {
    let f = Fixture::new();
    let handles: Vec<_> =
        (0..12).map(|_|
                    {
                        let path = f.store.path.clone();
                        std::thread::spawn(move ||
                                {
                                    let mut s =
                                        Intake::open(&path, json!({"A":["app"]})).unwrap();
                                    s.execute(&req(), "app").unwrap()["receipt"].clone()
                                })
                    }).collect();
    let results: Vec<_> =
        handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(results.iter().all(|r|r==&results[0]));
}
#[test]
fn late_binding_pending_blocks_all_entrypoints() {
    let mut f = Fixture::new();
    let first = f.call(req());
    f.bind(json!({"job":{"status":"in_progress"}}));
    for r in
        [req(),
                json!({"op":"get","source":"A","receipt_id":first["receipt"]["receipt_id"]}),
                json!({"op":"stream_state","source":"A","session":"s","stream":"stream"})]
        {
        assert_eq!(f.call(r),json!({"status":"pending_deletion"}));
    }
    assert_eq!(f.store.erase(&[],None).unwrap_err().response()["status"],"coordinator_required");
}
#[test]
fn done_session_and_key_rules() {
    let mut f = Fixture::new();
    f.bind(json!({"job":{"status":"done","keys":[["A",sha(b"k")]],"selector":{"kind":"session","source":"A","session":"removed"}}}));
    assert_eq!(f.call(req())["status"],"deleted");
    let mut r = req();
    r["key"] = json!("new");
    r["session"] = json!("removed");
    assert_eq!(f.call(r.clone())["status"],"deleted");
    r["source"] = json!("B");
    assert_eq!(f.call(r)["status"],"accepted");
}
#[test]
fn escaped_text_and_corrupt_payload_fail_closed() {
    let state =
        json!({"jobs":{"j":{"status":"complete","keys":[],"selector":{"kind":"text","length":6,"sha256":sha(b"SECRET")}}}});
    assert!(policy::blocked(&state,"A",br#"{"text":"\u0053ECRET"}"#,"s",None).unwrap());
    assert!(policy::blocked(&state,"A",b"SECRET","s",None).unwrap());
    assert!(!policy::blocked(&state,"A",br#"{"text":"safe"}"#,"s",None).unwrap());
    assert!(policy::blocked(&state,"A",b"{incomplete","s",None).is_err());
}
#[test]
fn damaged_journal_never_opens() {
    let mut f = Fixture::new();
    f.bind(json!({}));
    fs::write(f.dir.join("queue/jobs.json"), "{").unwrap();
    assert_eq!(f.call(req())["status"],"deletion_state_unavailable");
    fs::remove_file(f.dir.join("queue/jobs.json")).unwrap();
    assert_eq!(f.call(req())["status"],"deletion_state_unavailable");
}
#[test]
fn unsafe_binding_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new();
    f.bind(json!({}));
    fs::set_permissions(f.dir.join("intake.db.deletion-binding.json"),
            fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(f.call(req())["status"],"invalid_deletion_binding");
}
#[test]
fn coordinator_fd_requires_held_matching_description() {
    use std::os::fd::AsRawFd;
    let mut f = Fixture::new();
    let first = f.call(req());
    f.bind(json!({"j":{"status":"pending"}}));
    let lock =
        fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(f.dir.join("queue/lock")).unwrap();
    assert!(policy::coordinator(&f.store.path,Some(lock.as_raw_fd())).is_err());
    assert_eq!(unsafe{libc::flock(lock.as_raw_fd(),libc::LOCK_SH|libc::LOCK_NB)},0);
    assert!(policy::coordinator(&f.store.path,Some(lock.as_raw_fd())).is_err());
    assert_eq!(unsafe{libc::flock(lock.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0);
    assert_eq!(unsafe { libc::fcntl(lock.as_raw_fd(),libc::F_SETFD,0) },0);
    drop(policy::coordinator(&f.store.path,Some(lock.as_raw_fd())).unwrap());
    assert_ne!(unsafe { libc::fcntl(lock.as_raw_fd(),libc::F_GETFD) } & libc::FD_CLOEXEC,0);
    let separate =
        fs::OpenOptions::new().read(true).write(true).open(f.dir.join("queue/lock")).unwrap();
    assert!(policy::coordinator(&f.store.path,Some(separate.as_raw_fd())).is_err());
    assert_eq!(f.store.erase(&[first["receipt"]["receipt_id"].as_str().unwrap().into()],Some(lock.as_raw_fd())).unwrap()["status"],"deleted");
    assert!(policy::access(&f.store.path).is_err());
    assert_eq!(unsafe{libc::flock(separate.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},-1);
}
#[test]
fn secure_delete_removes_live_payload() {
    let mut f = Fixture::new();
    let payload = "SYNTHETIC-DELETE-MARKER";
    let r =
        f.call(json!({"source":"A","session":"s","key":"erase-key","payload_base64":"U1lOVEhFVElDLURFTEVURS1NQVJLRVI="}));
    f.store.erase(&[r["receipt"]["receipt_id"].as_str().unwrap().into()],
            None).unwrap();
    let bytes = fs::read(&f.store.path).unwrap();
    assert!(!bytes.windows(payload.len()).any(|b|b==payload.as_bytes()));
}

#[test]
fn malformed_completed_selector_blocks_reads() {
    let mut f = Fixture::new();
    let first = f.call(req());
    f.bind(json!({"job":{"status":"done","keys":[],"selector":{"kind":"text","length":6}}}));
    assert_eq!(f.call(json!({"op":"get","source":"A","receipt_id":first["receipt"]["receipt_id"]}))["status"],"deletion_state_unavailable");
}

#[test]
fn shared_json_deletion_corpus_never_creates_observation_or_receipt() {
    use base64::Engine;
    let cases = uasf_json::from_str(include_str!("../../uasf-json/tests/inspection.json")).unwrap();
    for c in cases.as_array().unwrap() {
        let mut f = Fixture::new();
        let mut selector = c["selector"].clone();
        if selector["kind"] == "text" {
            let text = selector.as_object_mut().unwrap().remove("text").unwrap();
            selector["length"] = json!(text.as_str().unwrap().len());
            selector["sha256"] = json!(sha(text.as_str().unwrap().as_bytes()));
        } else { selector["source"] = json!("A"); }
        f.bind(json!({"j":{"status":"complete","keys":[],"selector":selector}}));
        let mut r = req();
        r["key"] = c["name"].clone();
        r["payload_base64"] = json!(base64::engine::general_purpose::STANDARD.encode(c["input"].as_str().unwrap()));
        for _ in 0..2 {
            let response = f.call(r.clone());
            assert_eq!(response["status"], if c["decode_ok"] == true { "deleted" } else { "deletion_state_unavailable" }, "{}",c["name"]);
            assert!(response.get("receipt").is_none());
            f.store = Intake::open(&f.store.path, json!({"A":["app"]})).unwrap();
        }
        let db = rusqlite::Connection::open(&f.store.path).unwrap();
        assert_eq!(db.query_row("SELECT count(*) FROM observation",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(db.query_row("SELECT count(*) FROM deletion_tombstone",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }
}
