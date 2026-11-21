//! Synthetic deletion regressions with independent expected access states.
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};
use uasf_json::{json, Value};
use uasf_core::{model::sha, Binding, ByteProvider, ByteResult, Capture, Core, LocalProvider, Source};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const MARKER: &str = "synthetic-body-to-erase";
struct Store(PathBuf);
impl Store {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("uasf-delete-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn core(&self) -> Core { Core::open(self.0.join("index.sqlite"), self.0.join("persistent.sqlite")).unwrap() }
    fn remove_index(&self) { std::fs::remove_file(self.0.join("index.sqlite")).unwrap(); }
    fn rows(&self, table: &str) -> Vec<Value> {
        assert!(["tombstones", "provenance", "evidence", "names"].contains(&table));
        let db = rusqlite::Connection::open(self.0.join("persistent.sqlite")).unwrap();
        let mut q = db.prepare(&format!("SELECT value FROM {table} ORDER BY id")).unwrap();
        q.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| uasf_json::from_str(&r.unwrap()).unwrap()).collect()
    }
    fn cli(&self, op: &str, input: &Value) -> Value {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_uasf-core"))
            .arg("--index").arg(self.0.join("index.sqlite"))
            .arg("--persistent").arg(self.0.join("persistent.sqlite"))
            .args([op, "--json", &input.to_string()]).output().unwrap();
        assert!(output.status.success(), "{:?}", output);
        uasf_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Store { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn lines(rows: &[Value]) -> Vec<u8> {
    rows.iter().flat_map(|r| [r.to_string().as_bytes(), b"\n"].concat()).collect()
}
fn capture(name: &str, session: &str, window: Option<&str>, data: &[u8]) -> Capture {
    Capture { source: "A".into(), capture: name.into(), generation: format!("gen-{name}"), length: data.len() as u64,
        sha256: sha(data), session: session.into(), window: window.map(str::to_owned),
        file_generation: window.map(|w| format!("file-{w}")), native_session: window.is_some() }
}
fn register(core: &Core, engine: &str, c: &Capture) {
    core.register_source(&Source { id: c.source.clone(), engine: engine.into(), observation: "fixture".into() }).unwrap();
    core.register_capture(c).unwrap();
}
fn row(n: u64, kind: &str, payload: Value) -> Value { json!({"ordinal":n,"type":kind,"payload":payload}) }
fn fixture() -> (Capture, Capture, LocalProvider) {
    let parent = vec![row(0, "session_meta", json!({"session_id":"S","history_mode":"paginated"})),
        row(1, "event_msg", json!({"type":"task_started","turn_id":"T"})),
        row(2, "response_item", json!({"type":"message","content":MARKER})),
        row(3, "event_msg", json!({"type":"task_complete","turn_id":"T"})),
        row(4, "response_item", json!({"type":"message","content":"surviving-body"}))];
    let base = json!({"thread_id":"P","end_ordinal_exclusive":4,"end_byte_offset":lines(&parent[..4]).len()});
    let fork = vec![row(4, "session_meta", json!({"session_id":"F","history_mode":"paginated","history_base":base,"forked_from_id":"S","forked_from_ordinal_exclusive":4})),
        row(5, "response_item", json!({"type":"message","content":MARKER}))];
    let (pb, fb) = (lines(&parent), lines(&fork));
    let p = capture("parent", "S", Some("P"), &pb);
    let f = capture("fork", "F", Some("F"), &fb);
    let provider = LocalProvider::new(BTreeMap::from([(p.id(), Binding::Bytes(pb)), (f.id(), Binding::Bytes(fb))]));
    (p, f, provider)
}
fn query(op: &str, c: &Capture, caps: &[&Capture]) -> Value {
    let mut q = json!({"op":op,"source":c.source,"session":c.session});
    if op != "evidence" {
        q["boundary"] = json!(caps.iter().map(|c| c.id()).collect::<Vec<_>>());
        if op != "events" { q["leaf"] = json!(c.window); }
    }
    q
}
fn fact(c: &Capture, name: &str, anchor: Value) -> Value {
    json!({"evidence_id":name,"source":c.source,"session":c.session,"fact":"received","assessment":"observed",
        "method":"synthetic-test","observed_at":"synthetic-time","anchors":[anchor],"body":{"text":MARKER,"$serde_json::private::Number":"123"}})
}
fn session_delete(c: &Capture) -> Value { json!([{"kind":"session","source":c.source,"session":c.session}]) }
fn assert_blocked(v: &Value) {
    let events = v["data"]["events"].as_array().unwrap();
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e["body"].is_null() && e["body_access"] == "blocked"));
    assert_ne!(v["status"], "deleted");
    assert!(!v.to_string().contains(MARKER));
}
fn assert_erased(v: &Value) {
    assert_eq!(v["status"], "deleted");
    assert!(!v.to_string().contains(MARKER));
}

#[test]
fn session_erasure_reaches_existing_reader_new_process_and_rebuilt_index() {
    let s = Store::new();
    let core = s.core();
    let (p, _, provider) = fixture();
    register(&core, "codex", &p);
    core.rebuild(&provider).unwrap();
    core.set_display_name("A", "S", MARKER).unwrap();
    let evidence = fact(&p, "receipt", p.anchor());
    core.add_evidence(&evidence).unwrap();
    let reader = s.core(); // Open before the writer deletes, but no stale cached policy.
    let q = query("events", &p, &[&p]);
    assert!(reader.query(&q).unwrap().to_string().contains(MARKER));
    let result = s.cli("delete", &session_delete(&p));
    assert_eq!(result["scope"], "core_index_and_durable_evidence_only");
    assert_erased(&reader.query(&q).unwrap());
    assert_erased(&s.cli("query", &q));
    core.add_evidence(&evidence).unwrap(); // Replay must not resurrect erased evidence.
    core.set_display_name("A", "S", MARKER).unwrap();
    assert!(s.rows("names")[0]["display_name"].is_null());
    assert!(s.rows("evidence")[0]["body"].is_null());
    for file in ["index.sqlite", "persistent.sqlite"] {
        assert!(!std::fs::read(s.0.join(file)).unwrap().windows(MARKER.len()).any(|w| w == MARKER.as_bytes()));
    }
    assert!(provider.read(&p, 0, p.length).data.unwrap().windows(MARKER.len()).any(|w| w == MARKER.as_bytes()));
    drop(reader); drop(core);
    s.remove_index();
    let core = s.core();
    assert_eq!(core.query(&json!({"op":"sessions"})).unwrap()["reasons"], json!(["index_requires_rebuild"]));
    assert_erased(&s.cli("query", &query("evidence", &p, &[])));
    core.rebuild_version(&provider, "synthetic-next-version").unwrap();
    assert_erased(&s.cli("query", &q));
    assert_eq!(s.rows("tombstones").len(), 1);
}

struct ParentDenied<'a> { provider: &'a LocalProvider, parent: String }
impl ByteProvider for ParentDenied<'_> {
    fn read(&self, c: &Capture, start: u64, end: u64) -> ByteResult {
        if c.id() == self.parent { ByteResult::denied("deleted", "synthetic_provider_deleted") }
        else { self.provider.read(c, start, end) }
    }
}
#[test]
fn late_fork_after_deleted_parent_is_withheld_without_erasing_fork() {
    let s = Store::new();
    let core = s.core();
    let (p, f, provider) = fixture();
    register(&core, "codex", &p);
    core.rebuild(&provider).unwrap();
    core.delete(&session_delete(&p)).unwrap();
    register(&core, "codex", &f);
    core.set_display_name("A", "F", "Fork name").unwrap();
    core.add_evidence(&fact(&f, "fork-fact", f.anchor())).unwrap();
    let denied = ParentDenied { provider: &provider, parent: p.id() };
    core.rebuild(&denied).unwrap();
    let q = query("events", &f, &[&f]);
    assert_blocked(&core.query(&q).unwrap());
    assert!(core.query(&q).unwrap()["data"]["events"][0]["body_reasons"].as_array().unwrap().contains(&json!("deleted_parent_session")));
    let before = s.rows("provenance");
    assert!(!before.iter().find(|v| v["id"] == p.id()).unwrap()["trusted"]["boundaries"].as_array().unwrap().is_empty());
    assert!(core.query(&query("evidence", &f, &[])).unwrap()["data"]["facts"][0]["body"].is_null());
    assert_eq!(s.rows("evidence")[0]["body"]["text"], MARKER);
    assert_eq!(s.rows("names")[0]["display_name"], "Fork name");
    assert_eq!(s.rows("tombstones").len(), 1);
    drop(core); s.remove_index();
    let core = s.core(); core.rebuild(&denied).unwrap();
    assert_blocked(&s.cli("query", &q));
    assert_eq!(s.rows("provenance"), before);
    assert_eq!(s.rows("tombstones").len(), 1);
    assert_eq!(s.rows("evidence")[0]["body"]["text"], MARKER);
}

#[test]
fn unverified_fork_never_creates_tombstone_or_overwrites_trusted_parent() {
    let s = Store::new(); let core = s.core();
    let (p, f, mut provider) = fixture();
    register(&core, "codex", &p); core.rebuild(&provider).unwrap();
    let trusted = s.rows("provenance")[0]["trusted"].clone();
    core.delete(&session_delete(&p)).unwrap();
    register(&core, "codex", &f);
    core.add_evidence(&fact(&f, "fork-fact", f.anchor())).unwrap();
    core.set_display_name("A", "F", "Fork name").unwrap();
    let tombstones = s.rows("tombstones");
    provider.verified = false; core.rebuild(&provider).unwrap();
    assert_blocked(&core.query(&query("events", &f, &[&p, &f])).unwrap());
    let provenance = s.rows("provenance");
    assert_eq!(provenance.iter().find(|v| v["id"] == p.id()).unwrap()["trusted"], trusted);
    assert!(provenance.iter().find(|v| v["id"] == f.id()).unwrap()["trusted"].is_null());
    assert_eq!(s.rows("tombstones"), tombstones);
    assert_eq!(s.rows("evidence")[0]["body"]["text"], MARKER);
    assert_eq!(s.rows("names")[0]["display_name"], "Fork name");
    drop(core); s.remove_index();
    let core = s.core(); core.rebuild(&provider).unwrap();
    assert_blocked(&s.cli("query", &query("events", &f, &[&p, &f])));
    assert_eq!(s.rows("tombstones"), tombstones);
}

#[test]
fn reversible_block_survives_restart_and_restores_evidence_and_name() {
    let s = Store::new(); let core = s.core();
    let (p, f, mut provider) = fixture();
    register(&core, "codex", &f);
    core.add_evidence(&fact(&f, "receipt", f.anchor())).unwrap();
    core.set_display_name("A", "F", "Preserved name").unwrap();
    provider.verified = false; core.rebuild(&provider).unwrap();
    assert_blocked(&core.query(&query("events", &f, &[&f])).unwrap());
    let original = s.rows("evidence");
    assert_eq!(original[0]["body"]["$serde_json::private::Number"], "123");
    assert!(s.rows("tombstones").is_empty());
    drop(core); s.remove_index();
    let core = s.core(); provider.verified = true; core.rebuild(&provider).unwrap();
    assert_blocked(&s.cli("query", &query("events", &f, &[&f])));
    register(&core, "codex", &p); core.rebuild(&provider).unwrap();
    let response = s.cli("query", &query("events", &f, &[&p, &f]));
    assert_eq!(response["status"], "complete");
    assert!(response["data"]["events"].as_array().unwrap().iter().all(|e| !e["body"].is_null() && e["body_access"] == "available"));
    assert_eq!(s.cli("query", &query("evidence", &f, &[]))["data"]["facts"][0]["body"], original[0]["body"]);
    assert_eq!(s.rows("names")[0]["display_name"], "Preserved name");
    assert_eq!(s.rows("evidence"), original);
    assert!(s.rows("tombstones").is_empty());
}

#[test]
fn range_deletion_is_precise_and_inherited_body_is_only_blocked() {
    let s = Store::new(); let core = s.core();
    let (p, f, provider) = fixture();
    for c in [&p, &f] { register(&core, "codex", c); }
    core.rebuild(&provider).unwrap(); core.set_display_name("A", "S", "Survives range").unwrap();
    let events = core.query(&query("events", &p, &[&p])).unwrap()["data"]["events"].as_array().unwrap().clone();
    let victim = events.iter().find(|e| e["ordinal"] == 2).unwrap();
    let survivor = events.iter().find(|e| e["ordinal"] == 4).unwrap();
    core.add_evidence(&fact(&p, "victim", victim["anchors"][0].clone())).unwrap();
    core.add_evidence(&fact(&p, "survivor", survivor["anchors"][0].clone())).unwrap();
    core.add_evidence(&fact(&f, "fork", f.anchor())).unwrap();
    core.delete(&json!([{"kind":"range","anchor":victim["anchors"][0]}])).unwrap();
    core.rebuild(&provider).unwrap();
    let response = core.query(&query("events", &p, &[&p])).unwrap();
    let events = response["data"]["events"].as_array().unwrap();
    assert_eq!(events.iter().find(|e| e["ordinal"] == 2).unwrap()["body_access"], "deleted");
    assert!(!events.iter().find(|e| e["ordinal"] == 4).unwrap()["body"].is_null());
    assert_eq!(s.rows("names")[0]["display_name"], "Survives range");
    let sessions = core.query(&json!({"op":"sessions"})).unwrap();
    assert!(sessions["data"]["sessions"].as_array().unwrap().iter().all(|s| s["status"] != "deleted"));
    let fork = core.query(&query("events", &f, &[&p, &f])).unwrap(); assert_blocked(&fork);
    assert!(fork["data"]["events"][0]["body_reasons"].as_array().unwrap().contains(&json!("inherited_deleted_range")));
    let facts = s.rows("evidence");
    assert_eq!(facts.iter().filter(|e| e["status"] == "deleted" && e["body"].is_null()).count(), 1);
    assert_eq!(facts.iter().filter(|e| !e["body"].is_null()).count(), 2);
    drop(core); s.remove_index(); let core = s.core(); core.rebuild(&provider).unwrap();
    assert_eq!(s.rows("evidence"), facts);
    assert_blocked(&s.cli("query", &query("events", &f, &[&p, &f])));
}

#[test]
fn fork_without_history_base_is_incomplete_and_never_tombstoned() {
    let bytes = lines(&[row(0, "session_meta", json!({"session_id":"F","history_mode":"paginated","forked_from_id":"S","forked_from_ordinal_exclusive":4}))]);
    let c = capture("no-base", "F", Some("F"), &bytes);
    let provider = LocalProvider::new(BTreeMap::from([(c.id(), Binding::Bytes(bytes))]));
    let s = Store::new(); let core = s.core(); register(&core, "codex", &c); core.rebuild(&provider).unwrap();
    let h = core.query(&query("history", &c, &[&c])).unwrap();
    assert_eq!(h["status"], "incomplete"); assert!(h["data"]["turn_ids"].is_null());
    assert!(h["reasons"].as_array().unwrap().contains(&json!("fork_history_base_missing")));
    assert!(h["data"]["edges"].as_array().unwrap().iter().any(|e| e["kind"] == "fork" && e["status"] == "unknown"));
    assert!(s.rows("tombstones").is_empty());
}

#[test]
fn evidence_and_deletion_require_sha_and_invalid_batch_is_atomic() {
    let s = Store::new(); let core = s.core(); let (p, _, provider) = fixture();
    register(&core, "codex", &p); core.rebuild(&provider).unwrap();
    for digest in [Value::Null, json!(""), json!("x".repeat(64)), json!("A".repeat(64)), json!("0".repeat(63)), json!(123), json!(format!("{}\n", "0".repeat(64)))] {
        let mut a = p.anchor();
        if digest.is_null() { a.as_object_mut().unwrap().remove("sha256"); } else { a["sha256"] = digest; }
        assert!(core.add_evidence(&fact(&p, "invalid", a.clone())).is_err());
        assert!(core.delete(&json!([{"kind":"session","source":"A","session":"S"},{"kind":"range","anchor":a}])).is_err());
        assert!(s.rows("tombstones").is_empty()); assert!(s.rows("evidence").is_empty());
    }
    for invalid in [json!([]), json!({}), json!([null]), json!([{"kind":"session","source":"A","session":"S","extra":true}]), json!([{"kind":"session","source":"A","session":""}])] {
        assert!(core.delete(&invalid).is_err());
    }
    assert!(core.query(&query("events", &p, &[&p])).unwrap().to_string().contains(MARKER));
    core.add_evidence(&fact(&p, "valid", p.anchor())).unwrap();
    assert_eq!(core.delete(&json!([{"kind":"range","anchor":p.anchor()}])).unwrap()["status"], "deleted");
}

#[test]
fn session_tombstone_covers_late_capture_but_not_same_id_in_other_source() {
    let s = Store::new(); let core = s.core(); let (p, _, provider) = fixture();
    core.delete(&session_delete(&p)).unwrap();
    register(&core, "codex", &p); core.rebuild(&provider).unwrap();
    assert_erased(&core.query(&query("events", &p, &[&p])).unwrap());
    let mut other = p.clone(); other.source = "B".into();
    register(&core, "codex", &other);
    let mut bindings = provider.bindings.clone(); bindings.insert(other.id(), bindings[&p.id()].clone());
    core.rebuild(&LocalProvider::new(bindings)).unwrap();
    let result = core.query(&query("events", &other, &[&other])).unwrap();
    assert_eq!(result["status"], "complete"); assert!(result.to_string().contains(MARKER));
}

#[test]
fn deleted_parent_registration_blocks_fork_even_without_ever_verified_parent() {
    let s = Store::new(); let core = s.core(); let (p, f, mut provider) = fixture();
    register(&core, "codex", &p); core.delete(&session_delete(&p)).unwrap();
    provider.bindings.remove(&p.id()); register(&core, "codex", &f); core.rebuild(&provider).unwrap();
    let result = core.query(&query("events", &f, &[&f])).unwrap(); assert_blocked(&result);
    assert!(result["data"]["events"][0]["body_reasons"].as_array().unwrap().contains(&json!("deleted_parent_session")));
    assert_eq!(s.rows("tombstones").len(), 1);
}

#[test]
fn missing_window_and_bad_provider_digest_cannot_create_deletion_authority() {
    let s = Store::new(); let core = s.core(); let (p, mut f, mut provider) = fixture();
    register(&core, "codex", &p); core.rebuild(&provider).unwrap();
    let trusted = s.rows("provenance")[0]["trusted"].clone();
    core.delete(&session_delete(&p)).unwrap(); f.window = None;
    register(&core, "codex", &f); provider.verified = false; core.rebuild(&provider).unwrap();
    assert_blocked(&core.query(&query("events", &f, &[&f])).unwrap());
    struct Bad<'a>(&'a LocalProvider);
    impl ByteProvider for Bad<'_> {
        fn read(&self, c: &Capture, start: u64, end: u64) -> ByteResult {
            let mut r = self.0.read(c, start, end); r.verification = "verified".into(); r.sha256 = Some("0".repeat(64)); r
        }
    }
    core.rebuild(&Bad(&provider)).unwrap();
    let prov = s.rows("provenance"); let parent = prov.iter().find(|v| v["id"] == p.id()).unwrap();
    assert_eq!(parent["trusted"], trusted); assert_eq!(parent["observation"], "mismatch");
    assert_eq!(s.rows("tombstones").len(), 1);
}

#[test]
fn bounded_redaction_pages_erase_every_row() {
    let s = Store::new(); let core = s.core();
    let bytes = lines(&(0..270).map(|n| json!({"type":"cost-state","position":n,"value":MARKER})).collect::<Vec<_>>());
    let c = capture("many", "C", None, &bytes);
    let provider = LocalProvider::new(BTreeMap::from([(c.id(), Binding::Bytes(bytes))]));
    register(&core, "claude", &c); core.rebuild(&provider).unwrap();
    core.delete(&session_delete(&c)).unwrap();
    let result = core.query(&query("events", &c, &[&c])).unwrap();
    let events = result["data"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 270);
    assert!(events.iter().all(|e| e["body"].is_null() && e["body_access"] == "deleted"));
    assert!(!std::fs::read(s.0.join("index.sqlite")).unwrap().windows(MARKER.len()).any(|w| w == MARKER.as_bytes()));
    drop(core); s.remove_index(); let core = s.core(); core.rebuild(&provider).unwrap();
    assert_eq!(core.query(&query("events", &c, &[&c])).unwrap(), result);
}

#[test]
fn deletion_after_inherited_boundary_does_not_block_fork() {
    let s = Store::new(); let core = s.core(); let (p, f, provider) = fixture();
    for c in [&p, &f] { register(&core, "codex", c); }
    core.rebuild(&provider).unwrap();
    let before = core.query(&query("events", &p, &[&p])).unwrap();
    let last = before["data"]["events"].as_array().unwrap().iter().find(|e| e["ordinal"] == 4).unwrap();
    core.delete(&json!([{"kind":"range","anchor":last["anchors"][0]}])).unwrap();
    let result = core.query(&query("events", &f, &[&p, &f])).unwrap();
    assert_eq!(result["status"], "complete");
    assert!(result["data"]["events"].as_array().unwrap().iter().all(|e| e["body_access"] == "available"));
}

#[test]
fn partial_compaction_deletion_withholds_remaining_bodies_without_erasing_them() {
    let s = Store::new(); let core = s.core(); let (p, f, mut provider) = fixture();
    let fork = provider.read(&f, 0, f.length).data.unwrap();
    let meta = uasf_json::from_slice(fork.split(|b| *b == b'\n').next().unwrap()).unwrap();
    let bytes = lines(&[
        row(4, "session_meta", json!({"session_id":"S","history_mode":"paginated","history_base":meta["payload"]["history_base"]})),
        row(5, "compacted", json!({"window_id":"new-context","previous_window_id":"old-context","retained_context":[]})),
        row(6, "response_item", json!({"type":"message","content":MARKER}))]);
    let w = capture("compacted", "S", Some("W"), &bytes);
    provider.bindings.insert(w.id(), Binding::Bytes(bytes));
    for c in [&p, &w] { register(&core, "codex", c); }
    core.rebuild(&provider).unwrap();
    let before = core.query(&query("events", &w, &[&w])).unwrap();
    let events = before["data"]["events"].as_array().unwrap();
    let compact = events.iter().find(|e| e["ordinal"] == 5).unwrap();
    let remaining = events.iter().find(|e| e["ordinal"] == 6).unwrap();
    core.add_evidence(&fact(&w, "remaining-fact", remaining["anchors"][0].clone())).unwrap();
    core.delete(&json!([{"kind":"range","anchor":compact["anchors"][0]}])).unwrap();
    let result = core.query(&query("events", &w, &[&p, &w])).unwrap();
    let events = result["data"]["events"].as_array().unwrap();
    let remaining = events.iter().find(|e| e["capture_id"] == w.id() && e["ordinal"] == 6).unwrap();
    assert_eq!(remaining["body_access"], "blocked");
    assert!(remaining["body"].is_null());
    assert!(remaining["body_reasons"].as_array().unwrap().contains(&json!("compaction_input_membership_unknown")));
    assert_eq!(s.rows("evidence")[0]["body"]["text"], MARKER);
    assert_eq!(s.rows("tombstones").len(), 1);
    assert!(std::fs::read(s.0.join("index.sqlite")).unwrap().windows(MARKER.len()).any(|w| w == MARKER.as_bytes()));
    drop(core); s.remove_index(); let core = s.core(); core.rebuild(&provider).unwrap();
    assert_eq!(core.query(&query("events", &w, &[&p, &w])).unwrap(), result);
}
