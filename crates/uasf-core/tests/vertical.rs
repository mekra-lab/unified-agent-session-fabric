use std::{
    collections::BTreeMap, path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use uasf_json::{json, Value};
use uasf_core::{
    model::{anchor_id, key, sha},
    parser, provider::LIMIT, Binding, ByteProvider, ByteResult, Capture, Core,
    LocalProvider, Source,
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p =
            std::env::temp_dir().join(format!("uasf-core-test-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn core(&self) -> Core {
        Core::open(self.0.join("index.sqlite"),
                self.0.join("persistent.sqlite")).unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn lines(rows: Vec<Value>) -> Vec<u8> {
    rows.into_iter().flat_map(|r|
                {
                    let mut b = uasf_json::to_vec(&r).unwrap();
                    b.push(b'\n');
                    b
                }).collect()
}
fn capture(source: &str, name: &str, session: &str, window: Option<&str>,
    bytes: &[u8]) -> Capture {
    Capture {
        source: source.into(),
        capture: name.into(),
        generation: format!("generation-{name}"),
        length: bytes.len() as u64,
        sha256: sha(bytes),
        session: session.into(),
        window: window.map(str::to_owned),
        file_generation: window.map(|w| format!("file-{w}")),
        native_session: window.is_some(),
    }
}
fn register(core: &Core, engine: &str, c: &Capture) {
    core.register_source(&Source {
                    id: c.source.clone(),
                    engine: engine.into(),
                    observation: "fixture".into(),
                }).unwrap();
    core.register_capture(c).unwrap();
}
fn row(n: u64, kind: &str, payload: Value) -> Value {
    json!({"ordinal":n,"type":kind,"payload":payload})
}
fn task(n: u64, name: &str, done: bool) -> Value {
    row(n, "event_msg",
        json!({"type":if
                done{"task_complete"}else{"task_started"},"turn_id":name}))
}
fn codex() -> (Vec<Capture>, LocalProvider) {
    let parent =
        vec![row(0,"session_meta",json!({"session_id":"S","history_mode":"paginated"})),task(1,"T1",false),task(2,"T1",true),task(3,"T2",false),task(4,"T2",true)];
    let end = lines(parent[..3].to_vec()).len();
    let base =
        json!({"thread_id":"P","end_ordinal_exclusive":3,"end_byte_offset":end});
    let child =
        vec![row(3,"session_meta",json!({"session_id":"S","history_mode":"paginated","history_base":base})),task(4,"T3",false),row(5,"compacted",json!({"window_id":"context-2","previous_window_id":"context-1","retained_context":[]})),task(6,"T3",true)];
    let fork =
        vec![row(3,"session_meta",json!({"session_id":"F","history_mode":"paginated","history_base":base,"forked_from_id":"S","forked_from_ordinal_exclusive":3}))];
    let mut caps = vec![];
    let mut bindings = BTreeMap::new();
    for (name, session, rows) in
        [("P", "S", parent), ("W", "S", child), ("F", "F", fork)] {
        let bytes = lines(rows);
        let c = capture("A", name, session, Some(name), &bytes);
        bindings.insert(c.id(), Binding::Bytes(bytes));
        caps.push(c);
    }
    (caps, LocalProvider::new(bindings))
}
fn request(caps: &[Capture], session: &str, leaf: &str) -> Value {
    json!({"op":"history","source":"A","session":session,"leaf":leaf,"boundary":caps.iter().map(Capture::id).collect::<Vec<_>>()})
}
#[test]
fn pinned_ids_and_source_separation() {
    assert_eq!(key("session",json!(["A","S"])),"session:6b35bd61593ce6e7deff47777adc5433ca6c4ae4c071aa73ca09a6a2a1a061f4");
    let a = capture("A", "same", "S", None, b"x\n");
    let b = capture("B", "same", "S", None, b"x\n");
    assert_ne!(a.id(),b.id());
    assert_ne!(a.session_id(),b.session_id());
    assert_ne!(anchor_id(&a.anchor()),anchor_id(&b.anchor()));
}
#[test]
fn local_provider_pins_prefix_and_range() {
    let data = b"alpha\nbeta\n";
    let c = capture("A", "capture", "S", None, data);
    let mut p =
        LocalProvider::new(BTreeMap::from([(c.id(),
                            Binding::Bytes([data.as_slice(), b"later"].concat()))]));
    assert_eq!(p.read(&c,6,11).data,Some(b"beta\n".to_vec()));
    assert_eq!(p.read(&c,11,12).verification,"unsupported");
    p.bindings.insert(c.id(), Binding::Bytes(b"changed prefix".to_vec()));
    assert!(p.read(&c,0,1).data.is_none());
    let mut huge = c.clone();
    huge.length = LIMIT + 1;
    assert_eq!(p.read(&huge,0,0).reason.as_deref(),Some("range_or_size_limit"));
}
#[test]
fn local_file_provider_reads_only_registered_file() {
    let scratch = Scratch::new();
    let bytes = b"synthetic\n";
    let path = scratch.0.join("fixture.jsonl");
    std::fs::write(&path, bytes).unwrap();
    let c = capture("A", "file", "S", None, bytes);
    let p =
        LocalProvider::new(BTreeMap::from([(c.id(), Binding::File(path))]));
    assert_eq!(p.read(&c,0,c.length).data.as_deref(),Some(bytes.as_slice()));
    let missing = capture("A", "missing", "S", None, bytes);
    assert_eq!(p.read(&missing,0,1).reason.as_deref(),Some("fixture_unavailable"));
}
#[test]
fn codex_selected_history_and_fork() {
    let scratch = Scratch::new();
    let core = scratch.core();
    let (caps, p) = codex();
    for c in &caps { register(&core, "codex", c); }
    core.rebuild(&p).unwrap();
    let h = core.query(&request(&caps, "S", "W")).unwrap();
    assert_eq!(h["status"],"complete");
    assert_eq!(h["data"]["turn_ids"],json!(["T1","T3"]));
    assert_eq!(h["data"]["excluded_turns"][0]["native_id"],"T2");
    assert_eq!(h["data"]["events"].as_array().unwrap().iter().filter(|e|e["type"]=="compacted").count(),1);
    let f = core.query(&request(&caps, "F", "F")).unwrap();
    assert_eq!(f["data"]["turn_ids"],json!(["T1"]));
    let mut q = request(&caps, "S", "W");
    q["op"] = json!("turn");
    q["turn_id"] = h["data"]["turns"][0]["id"].clone();
    let t = core.query(&q).unwrap();
    assert_eq!(t["data"]["selection"],"active");
    assert_eq!(t["data"]["turns"][0]["anchors"].as_array().unwrap().len(),2);
    for e in h["data"]["events"].as_array().unwrap() {
        let a = &e["anchors"][0];
        let c = caps.iter().find(|c| c.id() == e["capture_id"]).unwrap();
        assert_eq!(p.read(c,a["start"].as_u64().unwrap(),a["end"].as_u64().unwrap()).sha256.unwrap(),a["sha256"]);
    }
}
#[test]
fn missing_parent_is_incomplete_and_child_body_withheld() {
    let scratch = Scratch::new();
    let core = scratch.core();
    let (caps, p) = codex();
    register(&core, "codex", &caps[1]);
    core.rebuild(&p).unwrap();
    let q = request(&caps[1..2], "S", "W");
    let h = core.query(&q).unwrap();
    assert_eq!(h["status"],"incomplete");
    assert!(h["data"]["turn_ids"].is_null());
    let mut q = q;
    q["op"] = json!("events");
    q.as_object_mut().unwrap().remove("leaf");
    let events = core.query(&q).unwrap();
    for e in events["data"]["events"].as_array().unwrap() {
        assert_eq!(e["body_access"],"blocked");
        assert!(e["body"].is_null());
    }
}
#[test]
fn fork_without_boundary_cannot_be_complete() {
    let bytes =
        lines(vec![row(0,"session_meta",json!({"session_id":"F","history_mode":"paginated","forked_from_id":"S"}))]);
    let c = capture("A", "fork", "F", Some("F"), &bytes);
    let p =
        LocalProvider::new(BTreeMap::from([(c.id(), Binding::Bytes(bytes))]));
    let b =
        parser::parse(&c, "codex", &p.read(&c, 0, c.length), parser::VERSION);
    assert_eq!(b["capture"][0]["status"],"incomplete");
    assert!(b["capture"][0]["reasons"].as_array().unwrap().contains(&json!("fork_boundary_missing")));
    assert_eq!(b["edge"][0]["kind"],"fork");
}
#[test]
fn unfinished_and_invalid_records_keep_byte_anchors() {
    for (raw, state, reason) in
        [(b"{".as_slice(), "incomplete", "unfinished_jsonl"),
                (b"[]\n".as_slice(), "unsupported", "invalid_json_record"),
                (b"\xff\n".as_slice(), "unsupported", "invalid_json_record")]
        {
        let c = capture("A", "bad", "C", None, raw);
        let p =
            LocalProvider::new(BTreeMap::from([(c.id(),
                                Binding::Bytes(raw.to_vec()))]));
        let b =
            parser::parse(&c, "claude", &p.read(&c, 0, c.length),
                parser::VERSION);
        assert_eq!(b["event"][0]["status"],state);
        assert_eq!(b["event"][0]["reasons"],json!([reason]));
        assert_eq!(b["event"][0]["anchors"][0],c.anchor());
    }
}
#[test]
fn parser_rechecks_provider_digest() {
    let bytes = lines(vec![json!({"type":"cost-state"})]);
    let c = capture("A", "claude", "C", None, &bytes);
    let b =
        parser::parse(&c, "claude",
            &ByteResult {
                    data: Some(bytes),
                    sha256: Some("0".repeat(64)),
                    verification: "verified".into(),
                    reason: None,
                }, parser::VERSION);
    assert_eq!(b["capture"][0]["reasons"],json!(["provider_capture_mismatch"]));
    assert!(b["event"].is_empty());
}
#[test]
fn claude_graph_preserves_uuidless_and_nonchain_references() {
    let bytes =
        lines(vec![json!({"type":"cost-state"}),json!({"type":"user","uuid":"u","parentUuid":null}),json!({"type":"assistant","uuid":"a","parentUuid":"u","requestRef":"r"}),json!({"type":"api-request","id":"r","keep":0,"tail":["missing"],"shapeHash":"shape"}),json!({"type":"system","subtype":"compact_boundary","uuid":"compact","parentUuid":null,"logicalParentUuid":"a","compactMetadata":{"preservedSegment":{"headUuid":"a"}}}),json!({"type":"user","uuid":"summary","parentUuid":"compact","isCompactSummary":true})]);
    let c = capture("A", "claude", "C", None, &bytes);
    let p =
        LocalProvider::new(BTreeMap::from([(c.id(), Binding::Bytes(bytes))]));
    let scratch = Scratch::new();
    let core = scratch.core();
    register(&core, "claude", &c);
    core.rebuild(&p).unwrap();
    let h = core.query(&request(&[c], "C", "summary")).unwrap();
    assert_eq!(h["status"],"unknown");
    assert_eq!(h["data"]["uuidless_event_ids"].as_array().unwrap().len(),2);
    assert_eq!(h["data"]["observed_chain"].as_array().unwrap().len(),2);
    assert_eq!(h["data"]["exposed"],"unknown");
    assert_eq!(h["data"]["outside_chain_is_excluded"],false);
    assert!(h["data"]["edges"].as_array().unwrap().iter().any(|e|e["kind"]=="preservedSegment"&&e["status"]=="complete"));
    let facts =
        core.query(&json!({"op":"evidence","source":"A","session":"C"})).unwrap();
    assert_eq!(facts["data"]["facts"],json!([]));
}
#[test]
fn fileless_api_and_independent_evidence_survive_rebuild() {
    let bytes =
        lines(vec![json!({"type":"api-record","schema":"uasf.synthetic-api/1","conversation":"app","call_id":"call","complete":true,"request":{"text":"synthetic"}})]);
    let c = capture("A", "api", "app", None, &bytes);
    let p =
        LocalProvider::new(BTreeMap::from([(c.id(), Binding::Bytes(bytes))]));
    let scratch = Scratch::new();
    let before;
    {
        let core = scratch.core();
        register(&core, "api", &c);
        core.set_display_name("A", "app", "Application").unwrap();
        core.rebuild(&p).unwrap();
        let evidence =
            json!({"evidence_id":"receipt","source":"A","session":"app","fact":"received","assessment":"observed","method":"test","observed_at":"synthetic-time","anchors":[c.anchor()]});
        core.add_evidence(&evidence).unwrap();
        let mut bad = evidence.clone();
        bad["anchors"][0].as_object_mut().unwrap().remove("sha256");
        assert!(core.add_evidence(&bad).is_err());
        let mut conflict = evidence.clone();
        conflict["fact"] = json!("exposed");
        assert!(core.add_evidence(&conflict).is_err());
        before = core.query(&request(&[c.clone()], "app", "unused")).unwrap();
        assert_eq!(before["status"],"unknown");
        assert_eq!(before["data"]["turns"][0]["native_id"],Value::Null);
    }
    std::fs::remove_file(scratch.0.join("index.sqlite")).unwrap();
    let core = scratch.core();
    assert_eq!(core.query(&json!({"op":"sessions"})).unwrap()["reasons"],json!(["index_requires_rebuild"]));
    core.rebuild(&p).unwrap();
    assert_eq!(before,core.query(&request(&[c.clone()],"app","unused")).unwrap());
    let facts =
        core.query(&json!({"op":"evidence","source":"A","session":"app"})).unwrap();
    assert_eq!(facts["data"]["absent_facts"],json!(["exposed","used"]));
    assert_eq!(facts["data"]["facts"][0]["automatic_promotion"],false);
    assert_eq!(core.query(&json!({"op":"sessions"})).unwrap()["data"]["sessions"][0]["display_name"],"Application");
}
#[test]
fn immutable_registration_and_no_cross_source_merge() {
    let scratch = Scratch::new();
    let core = scratch.core();
    let bytes = lines(vec![json!({"type":"cost-state"})]);
    let mut bindings = BTreeMap::new();
    for source in ["A", "B"] {
        let c = capture(source, "same", "C", None, &bytes);
        register(&core, "claude", &c);
        bindings.insert(c.id(), Binding::Bytes(bytes.clone()));
        let mut changed = c;
        changed.sha256 = "0".repeat(64);
        assert!(core.register_capture(&changed).is_err());
    }
    core.rebuild(&LocalProvider::new(bindings)).unwrap();
    let s = core.query(&json!({"op":"sessions"})).unwrap();
    let a = s["data"]["sessions"].as_array().unwrap();
    assert_eq!(a.len(),2);
    assert_ne!(a[0]["id"],a[1]["id"]);
    assert_ne!(a[0]["display_label"],a[1]["display_label"]);
}
#[test]
fn long_graph_cycle_is_iterative() {
    let mut p = BTreeMap::new();
    for n in 1..20000 { p.insert(n.to_string(), (n - 1).to_string()); }
    assert!(uasf_core::graph::cycle_reachable(&p,p.keys().cloned()).is_empty());
    p.insert("0".into(), "1".into());
    assert_eq!(uasf_core::graph::cycle_reachable(&p,p.keys().cloned()).len(),20000);
}
#[test]
fn parser_version_does_not_change_original_ids() {
    let bytes = lines(vec![json!({"type":"cost-state"})]);
    let c = capture("A", "version", "C", None, &bytes);
    let p =
        LocalProvider::new(BTreeMap::from([(c.id(), Binding::Bytes(bytes))]));
    let result = p.read(&c, 0, c.length);
    let a = parser::parse(&c, "claude", &result, "m1.2");
    let b = parser::parse(&c, "claude", &result, "test-next");
    assert_eq!(a["event"][0]["id"],b["event"][0]["id"]);
    assert_eq!(a["event"][0]["anchors"],b["event"][0]["anchors"]);
    assert_ne!(a["event"][0]["parser_version"],b["event"][0]["parser_version"]);
}
#[test]
fn cli_and_library_query_agree() {
    let scratch = Scratch::new();
    let core = scratch.core();
    let (caps, p) = codex();
    for c in &caps { register(&core, "codex", c); }
    core.rebuild(&p).unwrap();
    let q = request(&caps, "S", "W");
    let expected = core.query(&q).unwrap();
    drop(core);
    let output =
        std::process::Command::new(env!("CARGO_BIN_EXE_uasf-core")).args(["--index",
                            scratch.0.join("index.sqlite").to_str().unwrap(),
                            "--persistent",
                            scratch.0.join("persistent.sqlite").to_str().unwrap(),
                            "query", "--json", &q.to_string()]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(uasf_json::from_slice(&output.stdout).unwrap(),expected);
}
#[test]
fn explicitly_deleted_store_can_be_reopened() {
    let scratch = Scratch::new();
    let core = scratch.core();
    core.delete(&json!([{"kind":"session","source":"A","session":"S"}])).unwrap();
    drop(core);
    let reopened = scratch.core();
    assert_eq!(reopened.query(&json!({"op":"sources"})).unwrap()["status"], "complete");
}

#[test]
fn numeric_canonicalization_preserves_identity_and_large_integer_bodies() {
    let v: Value =
        uasf_json::from_str("[1e-7,1e-5,1e16,1e20,0.0001,1000000000000000.0,-0.0,1267650600228229401496703205376]").unwrap();
    assert_eq!(uasf_core::model::canonical(&v),"[1e-07,1e-05,1e+16,1e+20,0.0001,1000000000000000.0,-0.0,1267650600228229401496703205376]");
    assert_eq!(v[7].to_string(),"1267650600228229401496703205376");
}

#[test]
fn capture_sql_selection_does_not_decode_unselected_rows() {
    let scratch=Scratch::new(); let core=scratch.core(); let (caps,provider)=codex();
    register(&core,"codex",&caps[0]);core.rebuild(&provider).unwrap();
    let req=request(&caps[..1],"S","P");let expected=core.query(&req).unwrap();
    let db=rusqlite::Connection::open(scratch.0.join("index.sqlite")).unwrap();
    // Valid JSON whose huge floating token is intentionally unsupported by the
    // safe decoder. A foreign capture must not be materialized for this query.
    db.execute("INSERT INTO objects VALUES ('event','foreign',?)",
        [r#"{"id":"foreign","capture_id":"unselected","body":1e9999}"#]).unwrap();
    assert_eq!(core.query(&req).unwrap(),expected);
}

#[test]
fn streamed_claude_rows_match_parser_and_keep_compaction_provenance() {
    let scratch=Scratch::new();let core=scratch.core();
    let bytes=lines(vec![json!({"type":"user","uuid":"u","sessionId":"S","message":{"content":"hi"}}),
        json!({"type":"system","uuid":"c","sessionId":"S","subtype":"compact_boundary","parentUuid":null,"logicalParentUuid":"u"})]);
    let c=capture("A","C","S",None,&bytes);register(&core,"claude",&c);
    let provider=LocalProvider::new(BTreeMap::from([(c.id(),Binding::Bytes(bytes))]));
    let result=provider.read(&c,0,c.length);let expected=parser::parse(&c,"claude",&result,parser::VERSION);
    core.rebuild(&provider).unwrap();
    let db=rusqlite::Connection::open(scratch.0.join("index.sqlite")).unwrap();
    for kind in ["capture","event","turn","edge","window"] {
        let mut stmt=db.prepare("SELECT value FROM objects WHERE kind=? ORDER BY id").unwrap();
        let actual:Vec<Value>=stmt.query_map([kind],|r|r.get::<_,String>(0)).unwrap()
            .map(|r|uasf_json::from_str(&r.unwrap()).unwrap()).collect();
        let mut wanted=expected[kind].clone();wanted.sort_by_key(|v|v["id"].as_str().unwrap().to_owned());
        assert_eq!(actual,wanted,"{kind}");
    }
    let durable=rusqlite::Connection::open(scratch.0.join("persistent.sqlite")).unwrap();
    let raw:String=durable.query_row("SELECT value FROM provenance WHERE id=?",[c.id()],|r|r.get(0)).unwrap();
    assert_eq!(uasf_json::from_str(&raw).unwrap()["trusted"]["has_compaction"],true);
}

#[test]
fn writer_bytes_snapshot_delivery_and_late_errors() {
    use std::io::{self, Write};
    let tmp = Scratch::new(); let core = tmp.core();
    let bytes = lines(vec![json!({"uuid":"one","type":"user","message":{"content":{"slice":[0,99],"anchors":[{"slice":[0,99]}],"$serde_json::private::Number":"12345"}}}), json!({"uuid":"two","parentUuid":"one","type":"assistant","message":{"content":"kept"}})]);
    let c = capture("s", "capture", "session", None, &bytes);
    register(&core, "claude", &c);
    core.rebuild(&LocalProvider::new(BTreeMap::from([(c.id(), Binding::Bytes(bytes))]))).unwrap();
    let request = json!({"op":"history","source":"s","session":"session","boundary":[c.id()],"leaf":"two"});
    let expected = uasf_json::to_vec(&core.query(&request).unwrap()).unwrap();
    struct DeletingWriter<'a> { core: &'a Core, bytes: Vec<u8> }
    impl Write for DeletingWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.bytes.is_empty() { self.core.delete(&json!([{"kind":"session","source":"s","session":"session"}])).unwrap(); }
            self.bytes.extend_from_slice(bytes); Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
    let other = tmp.core(); let mut dest = DeletingWriter { core: &other, bytes: Vec::new() };
    core.query_to_writer(&request, &mut dest).unwrap(); assert_eq!(dest.bytes, expected);
    let mut after = Vec::new(); core.query_to_writer(&request, &mut after).unwrap();
    assert_eq!(after, uasf_json::to_vec(&core.query(&request).unwrap()).unwrap());
    assert_eq!(uasf_json::from_slice(&after).unwrap()["status"], "deleted");
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> { Err(io::Error::new(io::ErrorKind::BrokenPipe,"closed")) }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
    assert!(matches!(core.query_to_writer(&request, &mut Broken), Err(uasf_core::model::Error::Delivery(_))));
    // A late corrupt edge is found only after events have been spooled.
    let db = rusqlite::Connection::open(tmp.0.join("index.sqlite")).unwrap();
    db.execute("UPDATE objects SET value=json_set(value,'$.body',json('1e9999')) WHERE kind='edge'", []).unwrap();
    let mut untouched = b"prefix".to_vec(); assert!(core.query_to_writer(&request, &mut untouched).is_err()); assert_eq!(untouched,b"prefix");
    assert!(std::fs::read_dir(&tmp.0).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains("spool")));
}

#[test]
fn admission_slots_refuse_release_and_reject_symlinks() {
    use uasf_core::admission::{acquire, Refusal, SLOTS};
    let tmp = Scratch::new();
    let mut leases: Vec<_> = (0..SLOTS).map(|_| acquire(&tmp.0).unwrap()).collect();
    assert!(matches!(acquire(&tmp.0), Err(Refusal::Capacity)));
    leases.pop(); let last = acquire(&tmp.0).unwrap();
    let mut child = std::process::Command::new("/bin/sleep").arg("10").spawn().unwrap();
    drop(last); drop(leases);
    // A live exec child must not retain its parent's descriptor locks.
    let leases: Vec<_> = (0..SLOTS).map(|_| acquire(&tmp.0).unwrap()).collect();
    assert!(child.try_wait().unwrap().is_none()); child.kill().unwrap(); child.wait().unwrap(); drop(leases);
    let slot = tmp.0.join(".uasf-query-slots/0"); std::fs::remove_file(&slot).unwrap();
    std::os::unix::fs::symlink(tmp.0.join("index.sqlite"), slot).unwrap();
    assert!(matches!(acquire(&tmp.0), Err(Refusal::Unavailable)));
}

#[test]
fn temporary_record_limit_counts_all_captures_before_projection() {
    for counts in [vec![10_000], vec![10_001], vec![5_000,5_001]] {
        let scratch=Scratch::new(); let core=scratch.core(); let mut bindings=BTreeMap::new(); let mut caps=Vec::new();
        for (i,n) in counts.iter().enumerate() {
            let bytes=lines((0..*n).map(|j| json!({"type":"user","uuid":format!("{i}-{j}"),"message":{"content":"synthetic"}})).collect());
            let c=capture("A", &format!("C{i}"),"S",None,&bytes);
            register(&core,"claude",&c); bindings.insert(c.id(),Binding::Bytes(bytes)); caps.push(c);
        }
        core.rebuild(&LocalProvider::new(bindings)).unwrap();
        // Selecting only the first capture cannot hide other access-policy work.
        let r=json!({"op":"events","source":"A","session":"S","boundary":[caps[0].id()]});
        let value=core.query(&r).unwrap(); let mut bytes=Vec::new();core.query_to_writer(&r,&mut bytes).unwrap();
        assert_eq!(value,uasf_json::from_slice(&bytes).unwrap());
        if counts.iter().sum::<usize>()>10_000 {
            assert_eq!(value["reasons"],json!(["query_record_limit"]));assert_eq!(value["data"],json!({}));
            // Persisted metadata survives reopening; deleting bodies is not a bypass.
            core.delete(&json!([{"kind":"session","source":"A","session":"S"}])).unwrap();
            drop(core); assert_eq!(scratch.core().query(&r).unwrap()["reasons"],json!(["query_record_limit"]));
            // A poisoned body must never be decoded on a refused request.
            let db=rusqlite::Connection::open(scratch.0.join("index.sqlite")).unwrap();
            db.execute("UPDATE objects SET value=json_set(value,'$.body',json('1e9999')) WHERE kind='event' AND id=(SELECT id FROM objects WHERE kind='event' LIMIT 1)",[]).unwrap();
            let reopened=scratch.core();let mut bytes=Vec::new();
            reopened.query_to_writer(&r,&mut bytes).unwrap();
            assert_eq!(uasf_json::from_slice(&bytes).unwrap()["reasons"],json!(["query_record_limit"]));
        } else { assert_eq!(value["data"]["events"].as_array().unwrap().len(),10_000); }
    }
}

#[test]
fn temporary_depth_limit_includes_parents_outside_boundary() {
    for depth in [128,129] {
        let scratch=Scratch::new();let core=scratch.core();let mut bindings=BTreeMap::new();let mut caps: Vec<Capture>=Vec::new();
        for i in 0..=depth {
            let mut meta=json!({"session_id":"S","history_mode":"paginated"});
            if let Some(parent)=caps.last() { meta["history_base"]=json!({"thread_id":parent.window,"end_ordinal_exclusive":i,"end_byte_offset":parent.length}); }
            let bytes=lines(vec![row(i,"session_meta",meta)]);
            let c=capture("A",&format!("C{i}"),"S",Some(&format!("W{i}")),&bytes);
            register(&core,"codex",&c);bindings.insert(c.id(),Binding::Bytes(bytes));caps.push(c);
        }
        core.rebuild(&LocalProvider::new(bindings)).unwrap();
        let r=request(&caps,"S",&format!("W{depth}"));let value=core.query(&r).unwrap();
        if depth==128 { assert!(!arr_test(&value["reasons"]).contains(&json!("query_depth_limit"))); }
        else {
            assert_eq!(value["reasons"],json!(["query_depth_limit"]));
            let mut narrow=r;narrow["boundary"]=json!([caps.last().unwrap().id()]);
            assert_eq!(core.query(&narrow).unwrap()["reasons"],json!(["query_depth_limit"]));
        }
    }
}
fn arr_test(v:&Value)->&Vec<Value> {v.as_array().unwrap()}

#[test]
fn temporary_limit_counts_invalid_and_unfinished_source_records() {
    let scratch=Scratch::new();let core=scratch.core();
    let bytes=[vec![b'\n';10_000],b"unfinished".to_vec()].concat();
    let c=capture("A","invalid","S",None,&bytes);register(&core,"claude",&c);
    core.rebuild(&LocalProvider::new(BTreeMap::from([(c.id(),Binding::Bytes(bytes))]))).unwrap();
    let r=json!({"op":"history","source":"A","session":"S","boundary":[c.id()],"leaf":"absent"});
    assert_eq!(core.query(&r).unwrap()["reasons"],json!(["query_record_limit"]));
}

#[test]
fn unavailable_huge_capture_cannot_overflow_budget_or_create_erasure() {
    let scratch=Scratch::new();let core=scratch.core();
    let mut c=capture("A","missing","S",None,b"");c.length=u64::MAX;
    register(&core,"claude",&c);
    core.rebuild(&LocalProvider::new(BTreeMap::new())).unwrap();
    let r=json!({"op":"events","source":"A","session":"S","boundary":[c.id()]});
    assert_eq!(core.query(&r).unwrap()["reasons"],json!(["query_record_limit"]));
    let db=rusqlite::Connection::open(scratch.0.join("persistent.sqlite")).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM tombstones",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}

#[test]
fn shared_persistent_large_evidence_invalidates_other_index_before_decode() {
    let scratch=Scratch::new();let a=scratch.core();
    let bytes=lines(vec![json!({"type":"user","uuid":"u","message":{"content":"synthetic"}})]);
    let c=capture("A","one","S",None,&bytes);register(&a,"claude",&c);
    let provider=LocalProvider::new(BTreeMap::from([(c.id(),Binding::Bytes(bytes))]));
    a.rebuild(&provider).unwrap();
    let b=Core::open(scratch.0.join("other.sqlite"),scratch.0.join("persistent.sqlite")).unwrap();
    b.rebuild(&provider).unwrap();
    // Recalculate A after B's rebuild so both begin with the same generation.
    a.set_display_name("A","S","synthetic").unwrap();
    let q=json!({"op":"evidence","source":"A","session":"S"});
    assert_eq!(a.query(&q).unwrap()["status"],"complete");
    b.add_evidence(&json!({"evidence_id":"large","source":"A","session":"S","fact":"received","assessment":"observed","method":"synthetic","observed_at":"synthetic","anchors":[c.anchor()],"body":"x".repeat(64*1024*1024+1)})).unwrap();
    assert_eq!(a.query(&q).unwrap()["reasons"],json!(["query_materialization_limit"]));
    let mut out=Vec::new();a.query_to_writer(&q,&mut out).unwrap();
    assert_eq!(uasf_json::from_slice(&out).unwrap()["reasons"],json!(["query_materialization_limit"]));
    assert!(out.len()<256);
    assert_eq!(b.query(&q).unwrap()["reasons"],json!(["query_materialization_limit"]));
    drop(a);assert_eq!(scratch.core().query(&q).unwrap()["reasons"],json!(["query_materialization_limit"]));
}

#[test]
fn shared_persistent_generation_covers_mutations_rollback_and_refresh() {
    let scratch=Scratch::new();let a=scratch.core();let b=Core::open(scratch.0.join("b.sqlite"),scratch.0.join("persistent.sqlite")).unwrap();
    let bytes=lines(vec![json!({"type":"user","uuid":"u"})]);let c=capture("A","C","S",None,&bytes);
    register(&a,"claude",&c);let provider=LocalProvider::new(BTreeMap::from([(c.id(),Binding::Bytes(bytes))]));
    a.rebuild(&provider).unwrap();b.rebuild(&provider).unwrap();
    a.set_display_name("A","S","first").unwrap();
    b.set_display_name("A","S","second").unwrap();
    let q=json!({"op":"events","source":"A","session":"S","boundary":[c.id()]});
    assert_eq!(a.query(&q).unwrap()["status"],"complete");
    a.rebuild(&provider).unwrap();assert_ne!(a.query(&q).unwrap()["status"],"unsupported");
    assert!(b.delete(&json!([{"kind":"invalid"}])).is_err());
    assert_ne!(a.query(&q).unwrap()["status"],"unsupported");
    b.delete(&json!([{"kind":"session","source":"A","session":"S"}])).unwrap();
    assert_eq!(a.query(&q).unwrap()["status"],"deleted");
    a.rebuild(&provider).unwrap();assert_eq!(a.query(&q).unwrap()["status"],"deleted");
}

#[test]
fn scoped_query_ignores_unrelated_registration_and_policy_rows_across_indexes() {
    let scratch=Scratch::new();let a=scratch.core();let (caps,provider)=codex();
    for c in &caps {register(&a,"codex",c);}a.rebuild(&provider).unwrap();
    let q=request(&caps[..2],"S","W");let before=a.query(&q).unwrap();let metrics=a.scope_metrics();
    let b=Core::open(scratch.0.join("b.sqlite"),scratch.0.join("persistent.sqlite")).unwrap();
    for i in 0..40 {
        let mut c=caps[0].clone();c.session=format!("unrelated-{i}");c.capture=format!("unrelated-{i}");c.window=Some(format!("unrelated-{i}"));c.length=100_000;
        b.register_capture(&c).unwrap();
        b.delete(&json!([{"kind":"session","source":"A","session":c.session}])).unwrap();
    }
    assert_eq!(a.query(&q).unwrap(),before);
    let after=a.scope_metrics();assert_eq!(metrics.records,after.records);assert_eq!(metrics.rows,after.rows);assert_eq!(metrics.response_rows,after.response_rows);assert_eq!(metrics.decoded_bytes,after.decoded_bytes);
    assert_eq!(after.fullscan_steps,0);assert!(after.vm_steps<=metrics.vm_steps+1000);
    // Listings retain their declared full scope, including unready captures.
    assert_eq!(a.query(&json!({"op":"sessions","source":"A"})).unwrap()["reasons"],json!(["query_record_limit"]));
}

#[test]
fn scoped_owner_candidates_outside_boundary_and_cross_source_are_not_response_members() {
    let scratch=Scratch::new();let a=scratch.core();let (caps,provider)=codex();
    for c in &caps {register(&a,"codex",c);}a.rebuild(&provider).unwrap();
    let q=request(&[caps[0].clone(),caps[2].clone()],"F","F");let before=a.query(&q).unwrap();
    let mut alias=caps[0].clone();alias.session="alias".into();alias.capture="alias".into();
    a.register_capture(&alias).unwrap();
    assert_eq!(a.query(&q).unwrap(),before); // unselected candidate is not inherited
    a.delete(&json!([{"kind":"session","source":"A","session":"alias"}])).unwrap();
    let blocked=a.query(&q).unwrap();assert_eq!(blocked["status"],"unknown");assert!(blocked.to_string().contains("deleted_parent_session"));
    assert!(!blocked["data"]["events"].as_array().unwrap().iter().any(|e|e["capture_id"]==alias.id()));
    let mut other=alias;other.source="other".into();register(&a,"codex",&other);
    a.delete(&json!([{"kind":"session","source":"other","session":"S"}])).unwrap();
    assert_eq!(a.query(&q).unwrap(),blocked);
}

#[test]
fn scoped_missing_summary_and_late_parent_do_not_become_absent_dependencies() {
    let scratch=Scratch::new();let core=scratch.core();let (caps,provider)=codex();
    register(&core,"codex",&caps[2]);core.rebuild(&provider).unwrap();
    let q=request(&[caps[2].clone()],"F","F");assert_ne!(core.query(&q).unwrap()["status"],"complete");
    register(&core,"codex",&caps[0]);
    core.delete(&json!([{"kind":"session","source":"A","session":"S"}])).unwrap();
    let response=core.query(&q).unwrap();assert!(response.to_string().contains("deleted_parent_session") || response["status"]=="incomplete");
    core.rebuild(&provider).unwrap();
    let db=rusqlite::Connection::open(scratch.0.join("index.sqlite")).unwrap();db.execute("DELETE FROM scope_summary WHERE id=?",[caps[2].id()]).unwrap();
    assert_eq!(core.query(&q).unwrap()["reasons"],json!(["index_requires_rebuild"]));
    core.rebuild(&provider).unwrap();assert_ne!(core.query(&q).unwrap()["status"],"complete");
}

#[test]
fn scoped_candidate_expansion_is_bounded_before_untrusted_rows_are_decoded() {
    let scratch=Scratch::new();let core=scratch.core();let (caps,provider)=codex();
    for c in &caps {register(&core,"codex",c);}core.rebuild(&provider).unwrap();
    // Simulate many immutable registrations in one transaction. The lookup
    // indexes are maintained by SQLite even before provenance is available.
    let db=rusqlite::Connection::open(scratch.0.join("persistent.sqlite")).unwrap();db.execute_batch("BEGIN").unwrap();
    for i in 0..10_001 {
        let mut c=caps[0].clone();c.capture=format!("candidate-{i}");c.session=format!("candidate-{i}");
        db.execute("INSERT INTO captures VALUES (?,?)",rusqlite::params![c.id(),uasf_json::to_string(&c).unwrap()]).unwrap();
    }
    db.execute_batch("COMMIT").unwrap();
    let q=request(&[caps[0].clone(),caps[2].clone()],"F","F");
    let v=core.query(&q).unwrap();assert_eq!(v["reasons"],json!(["query_policy_limit"]));assert_eq!(v["data"],json!({}));
    assert!(core.scope_metrics().decoded_bytes<100_000);
}

#[test]
fn scoped_evidence_follows_explicit_cross_source_anchor_and_session_withholding() {
    let scratch=Scratch::new();let core=scratch.core();let (caps,provider)=codex();
    for c in &caps {register(&core,"codex",c);}core.rebuild(&provider).unwrap();
    core.register_source(&Source{id:"report".into(),engine:"api".into(),observation:"fixture".into()}).unwrap();
    let e=json!({"evidence_id":"cross","source":"report","session":"E","fact":"received","assessment":"observed","method":"synthetic","observed_at":"synthetic","anchors":[caps[0].anchor()],"body":"synthetic-body"});
    core.add_evidence(&e).unwrap();let q=json!({"op":"evidence","source":"report","session":"E"});
    assert_eq!(core.query(&q).unwrap()["data"]["facts"][0]["body_access"],"available");
    core.delete(&json!([{"kind":"session","source":"A","session":"S"}])).unwrap();
    let v=core.query(&q).unwrap();assert_eq!(v["data"]["facts"][0]["body_access"],"deleted");assert!(v["data"]["facts"][0]["body"].is_null());
}

#[test]
fn scoped_erasure_keeps_exact_unsigned_byte_ranges_above_sql_integer_limit() {
    let scratch=Scratch::new();let core=scratch.core();
    core.register_source(&Source{id:"api".into(),engine:"api".into(),observation:"fixture".into()}).unwrap();
    let start=i64::MAX as u64+100;
    let anchor=json!({"source":"api","capture":"large-offset","generation":"g","start":start,"end":start+1,"sha256":sha(b"x")});
    let e=json!({"evidence_id":"offset","source":"api","session":"S","fact":"received","assessment":"observed","method":"synthetic","observed_at":"synthetic","anchors":[anchor],"body":"retained"});
    let q=json!({"op":"evidence","source":"api","session":"S"});core.add_evidence(&e).unwrap();
    let mut adjacent=anchor.clone();adjacent["start"]=json!(start+1);adjacent["end"]=json!(start+2);
    core.delete(&json!([{"kind":"range","anchor":adjacent}])).unwrap();assert_eq!(core.query(&q).unwrap()["data"]["facts"][0]["body"],"retained");
    core.delete(&json!([{"kind":"range","anchor":anchor}])).unwrap();assert!(core.query(&q).unwrap()["data"]["facts"][0]["body"].is_null());
}

#[test]
fn scoped_missing_length_metadata_refuses_body_until_atomic_reregistration() {
    let scratch=Scratch::new();let core=scratch.core();
    core.register_source(&Source{id:"api".into(),engine:"api".into(),observation:"fixture".into()}).unwrap();
    let a=json!({"source":"api","capture":"synthetic","generation":"g","start":0,"end":1,"sha256":sha(b"x")});
    let e=json!({"evidence_id":"sized","source":"api","session":"S","fact":"received","assessment":"observed","method":"synthetic","observed_at":"synthetic","anchors":[a],"body":"kept"});
    let id=core.add_evidence(&e).unwrap();let q=json!({"op":"evidence","source":"api","session":"S"});
    let db=rusqlite::Connection::open(scratch.0.join("persistent.sqlite")).unwrap();db.execute("DELETE FROM scope_sizes WHERE kind='evidence' AND id=?",[id]).unwrap();
    assert_eq!(core.query(&q).unwrap()["reasons"],json!(["query_scope_unavailable"]));
    core.add_evidence(&e).unwrap();assert_eq!(core.query(&q).unwrap()["data"]["facts"][0]["body"],"kept");
}

#[test]
fn scoped_scalar_reservations_keep_material_limits_without_output_multipliers() {
    let scratch=Scratch::new();let core=scratch.core();
    let bytes=lines(vec![json!({"type":"user","uuid":"u"})]);let c=capture("A","C","S",None,&bytes);
    register(&core,"claude",&c);core.rebuild(&LocalProvider::new(BTreeMap::from([(c.id(),Binding::Bytes(bytes))]))).unwrap();
    let q=json!({"op":"events","source":"A","session":"S","boundary":[c.id()]});
    let db=rusqlite::Connection::open(scratch.0.join("index.sqlite")).unwrap();
    db.execute("UPDATE scope_summary SET edges=19999 WHERE id=?",[c.id()]).unwrap();assert_eq!(core.query(&q).unwrap()["status"],"complete");
    db.execute("UPDATE scope_summary SET edges=20000 WHERE id=?",[c.id()]).unwrap();assert_eq!(core.query(&q).unwrap()["reasons"],json!(["query_edge_limit"]));
    db.execute("UPDATE scope_summary SET edges=0 WHERE id=?",[c.id()]).unwrap();core.query(&q).unwrap();
    let policy=core.scope_metrics().decoded_bytes as i64;
    db.execute("UPDATE scope_summary SET bytes=?,anchor=0 WHERE id=?",rusqlite::params![(256*1024*1024-1024*1024)/8-policy,c.id()]).unwrap();
    assert_eq!(core.query(&q).unwrap()["status"],"complete");
    db.execute("UPDATE scope_summary SET anchor=1000000 WHERE id=?",[c.id()]).unwrap();
    let value=core.query(&q).unwrap();assert_eq!(value["status"],"complete");
    assert_eq!(core.scope_metrics().output_estimate_bytes,Some(uasf_json::to_vec(&value).unwrap().len() as u64));
    let durable=rusqlite::Connection::open(scratch.0.join("persistent.sqlite")).unwrap();
    durable.execute("UPDATE scope_sizes SET bytes=67108864 WHERE kind='sources'",[]).unwrap();
    assert_eq!(core.query(&json!({"op":"sources"})).unwrap()["status"],"complete");
    durable.execute("UPDATE scope_sizes SET bytes=67108865 WHERE kind='sources'",[]).unwrap();
    assert_eq!(core.query(&json!({"op":"sources"})).unwrap()["reasons"],json!(["query_materialization_limit"]));
}

#[test]
fn scoped_catalog_policy_visits_accept_exact_limit_and_never_return_partial_list() {
    let scratch=Scratch::new();let core=scratch.core();let db=rusqlite::Connection::open(scratch.0.join("persistent.sqlite")).unwrap();
    db.execute_batch("BEGIN").unwrap();
    for i in 0..5000 {
        let source=Source{id:format!("synthetic-{i}"),engine:"api".into(),observation:"fixture".into()};
        db.execute("INSERT INTO sources VALUES (?,?)",rusqlite::params![source.id,uasf_json::to_string(&source).unwrap()]).unwrap();
    }
    db.execute_batch("COMMIT").unwrap();let q=json!({"op":"sources"});
    let v=core.query(&q).unwrap();assert_eq!(v["data"]["sources"].as_array().unwrap().len(),5000);assert_eq!(core.scope_metrics().rows,10000);
    core.register_source(&Source{id:"extra".into(),engine:"api".into(),observation:"fixture".into()}).unwrap();
    let v=core.query(&q).unwrap();assert_eq!(v["reasons"],json!(["query_policy_limit"]));assert_eq!(v["data"],json!({}));
}

#[test]
fn scoped_output_reservation_does_not_multiply_aggregate_turn_anchor_arrays() {
    let scratch=Scratch::new();let core=scratch.core();let mut rows=vec![row(0,"session_meta",json!({"session_id":"S","history_mode":"paginated"})),task(1,"T",false)];
    rows.extend((2..1999).map(|n|row(n,"response_item",json!({"type":"message","role":"user","content":"synthetic"}))));rows.push(task(1999,"T",true));
    let bytes=lines(rows);let c=capture("A","single-turn","S",Some("W"),&bytes);
    register(&core,"codex",&c);core.rebuild(&LocalProvider::new(BTreeMap::from([(c.id(),Binding::Bytes(bytes))]))).unwrap();
    let mut out=Vec::new();assert_eq!(core.query_to_writer(&request(&[c],"S","W"),&mut out).unwrap(),"complete");
    assert_eq!(core.scope_metrics().records,2000);
    assert_eq!(uasf_json::from_slice(&out).unwrap()["data"]["events"].as_array().unwrap().len(),1999);
}

#[test]
fn operation_output_counts_match_wire_with_repeated_compaction_references() {
    let tmp = Scratch::new(); let core = tmp.core();
    let mut rows = vec![row(0,"session_meta",json!({"session_id":"S","history_mode":"paginated"}))];
    for n in 1..=300 {
        rows.push(row(n,"compacted",json!({"window_id":format!("context-{n}"),"previous_window_id":if n==1 {Value::Null} else {json!("context-1")},"retained_context":["synthetic\n\"é"]})));
    }
    let bytes=lines(rows); let c=capture("A","W","S",Some("W"),&bytes); register(&core,"codex",&c);
    core.rebuild(&LocalProvider::new(BTreeMap::from([(c.id(),Binding::Bytes(bytes))]))).unwrap();
    for op in ["history","events","lineage","status"] {
        let mut q=request(&[c.clone()],"S","W"); q["op"]=json!(op);
        let expected=uasf_json::to_vec(&core.query(&q).unwrap()).unwrap();
        let mut wire=Vec::new(); core.query_to_writer(&q,&mut wire).unwrap();
        assert_eq!(wire,expected);
        assert_eq!(core.scope_metrics().output_estimate_bytes,Some(wire.len() as u64));
        assert_eq!(core.scope_metrics().serialized_output_bytes,Some(wire.len() as u64));
    }
    let h=core.query(&request(&[c],"S","W")).unwrap();
    assert_eq!(h["data"]["edges"].as_array().unwrap().iter().filter(|e| e["target_anchors"].as_array().is_some_and(|a|!a.is_empty())).count(),299);
    // Reusing the connection for an invalid request must not retain old counts.
    let mut wire=Vec::new(); core.query_to_writer(&json!({"op":"invalid"}),&mut wire).unwrap();
    assert_eq!(core.scope_metrics().output_estimate_bytes,Some(wire.len() as u64));
    assert_eq!(core.scope_metrics().serialized_output_bytes,Some(wire.len() as u64));
}

#[test]
fn physical_row_order_does_not_change_projection_or_spooled_wire() {
    let scratch=Scratch::new();let core=scratch.core();let (caps,provider)=codex();
    for c in &caps {register(&core,"codex",c);}
    core.rebuild(&provider).unwrap();
    let mut requests=vec![json!({"op":"sessions","source":"A"})];
    for op in ["history","events","lineage","status"] {
        let mut q=request(&caps[..2],"S","W");q["op"]=json!(op);requests.push(q);
    }
    let before:Vec<_>=requests.iter().map(|q|{
        let mut wire=Vec::new();core.query_to_writer(q,&mut wire).unwrap();
        assert_eq!(wire,uasf_json::to_vec(&core.query(q).unwrap()).unwrap());wire
    }).collect();
    drop(core);
    let db=rusqlite::Connection::open(scratch.0.join("index.sqlite")).unwrap();
    let rows={let mut stmt=db.prepare("SELECT kind,id,value FROM objects ORDER BY id DESC").unwrap();
        stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).unwrap().collect::<std::result::Result<Vec<_>,_>>().unwrap()};
    db.execute_batch("BEGIN").unwrap();
    for (kind,id,value) in rows {db.execute("INSERT OR REPLACE INTO objects VALUES (?,?,?)",rusqlite::params![kind,id,value]).unwrap();}
    db.execute_batch("COMMIT").unwrap();drop(db);
    let core=scratch.core();
    for (q,expected) in requests.iter().zip(before) {
        let mut wire=Vec::new();core.query_to_writer(q,&mut wire).unwrap();
        assert_eq!(wire,expected);assert_eq!(core.scope_metrics().fullscan_steps,0);
        assert_eq!(uasf_json::to_vec(&core.query(q).unwrap()).unwrap(),expected);
    }
}

fn permission_fixture() -> Vec<Value> {
    include_str!("permission_mode.jsonl").lines().map(|s| uasf_json::from_str(s).unwrap()).collect()
}
fn permission_bundle(rows: Vec<Value>) -> (Capture, LocalProvider, parser::Bundle) {
    let data=lines(rows);
    let c=capture("permission", "permission", "synthetic-permission-session", None, &data);
    let provider=LocalProvider::new(BTreeMap::from([(c.id(),Binding::Bytes(data))]));
    let bundle=parser::parse(&c,"claude",&provider.read(&c,0,c.length),parser::VERSION);
    (c,provider,bundle)
}
#[test]
fn permission_mode_is_uninterpreted_metadata_with_no_graph_or_turn_identity() {
    for mode in ["plan", "default", "future-mode", ""] {
        let mut rows=permission_fixture();rows[0]["permissionMode"]=json!(mode);
        let (_,_,b)=permission_bundle(rows);
        assert_eq!(b["capture"][0]["status"],"complete");
        assert_eq!(b["capture"][0]["parser_version"],"claude/m1.3");
        assert_eq!(b["event"].len(),4);assert_eq!(b["turn"].len(),2);assert_eq!(b["edge"].len(),1);
        for i in [0,2] {
            let e=&b["event"][i];assert!(e["native_id"].is_null());assert!(e["turn_id"].is_null());
            assert_eq!(e["links"],json!([]));assert!(e.get("compaction").is_none());
            assert_eq!(e["body"]["type"],"permission-mode");
        }
        assert_eq!(b["edge"][0]["kind"],"parent");
    }
}
#[test]
fn permission_mode_rejects_every_shape_extension_without_creating_dependencies() {
    let mut invalid=vec![];
    for key in ["extra","uuid","parentUuid","logicalParentUuid","requestRef","subtype","compactMetadata"] {
        let mut r=permission_fixture()[0].clone();r[key]=json!("synthetic-reference");invalid.push(r);
    }
    for value in [Value::Null,json!(false),json!(1),json!([]),json!({})] {
        let mut r=permission_fixture()[0].clone();r["permissionMode"]=value;invalid.push(r);
    }
    for value in [Value::Null,json!("other-session"),json!(1)] {
        let mut r=permission_fixture()[0].clone();r["sessionId"]=value;invalid.push(r);
    }
    for key in ["sessionId","permissionMode"] {
        let mut r=permission_fixture()[0].clone();r.as_object_mut().unwrap().remove(key);invalid.push(r);
    }
    assert_eq!(invalid.len(),17);
    for r in invalid {
        let (_,_,b)=permission_bundle(vec![r]);
        assert_eq!(b["capture"][0]["status"],"unsupported");
        assert_eq!(b["capture"][0]["reasons"],json!(["unsupported_claude_record"]));
        assert!(b["edge"].is_empty());assert!(b["turn"].is_empty());
        assert_eq!(b["event"][0]["links"],json!([]));
    }
}
#[test]
fn permission_mode_streamed_rebuild_history_and_deletion_preserve_anchors() {
    let scratch=Scratch::new();let core=scratch.core();
    let (c,provider,b)=permission_bundle(permission_fixture());register(&core,"claude",&c);
    assert_eq!(core.rebuild(&provider).unwrap()["parser_version"],"m1.3");
    let q=json!({"op":"history","source":c.source,"session":c.session,"boundary":[c.id()],"leaf":"synthetic-assistant"});
    let v=core.query(&q).unwrap();assert_eq!(v["status"],"unknown");
    assert_eq!(v["reasons"],json!(["model_input_unknown","native_authority_not_compared"]));
    let mut expected=b["event"].clone();
    for e in &mut expected { e["body_access"]=json!("available"); }
    assert_eq!(v["data"]["events"],json!(expected));
    assert_eq!(v["data"]["observed_chain"].as_array().unwrap().len(),2);
    assert_eq!(v["data"]["edges"].as_array().unwrap().len(),1);
    for e in v["data"]["events"].as_array().unwrap() {
        let a=&e["anchors"][0];let raw=provider.read(&c,0,c.length).data.unwrap();
        assert_eq!(a["sha256"],sha(&raw[a["start"].as_u64().unwrap() as usize..a["end"].as_u64().unwrap() as usize]));
    }
    drop(core);let core=scratch.core();assert_eq!(core.query(&q).unwrap(),v);
    // Explicit deletion still includes metadata bodies; no new retention exception.
    core.delete(&json!([{"kind":"session","source":c.source,"session":c.session}])).unwrap();
    core.rebuild(&provider).unwrap();assert_eq!(core.query(&q).unwrap()["status"],"deleted");
}
#[test]
fn permission_mode_extension_does_not_relabel_legacy_or_custom_parser_versions() {
    let (c,p,_)=permission_bundle(permission_fixture());
    let b=parser::parse(&c,"claude",&p.read(&c,0,c.length),"explicit-test-version");
    assert_eq!(b["capture"][0]["parser_version"],"claude/explicit-test-version");
    let rows=permission_fixture().into_iter().filter(|r|r["type"]!="permission-mode").collect();
    let (_,_,b)=permission_bundle(rows);assert_eq!(b["capture"][0]["parser_version"],"claude/m1.2");
}
