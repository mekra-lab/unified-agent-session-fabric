use std::{collections::BTreeMap,path::PathBuf,sync::atomic::{AtomicUsize,Ordering}};
use uasf_json::{json,Value};
use uasf_core::{model::sha,Binding,Capture,Core,LocalProvider,Source};
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


fn prepared(s:&Scratch,r:&Value)->(PathBuf,Value) {
    let probe=s.0.join("probe.sqlite");if !probe.exists(){uasf_index_probe::load(&s.0.join("index.sqlite"),&s.0.join("persistent.sqlite"),&probe).unwrap();}
    let db=s.0.join(format!("page-{}.sqlite",NEXT.fetch_add(1,Ordering::Relaxed)));
    let m=uasf_page_probe::prepare(&probe,&s.0.join("persistent.sqlite"),r,&db).unwrap();
    let mut r=r.clone();r["page"]=json!({"manifest":m["manifest"],"max_records":3});(db,r)
}
fn page(db:&std::path::Path,r:&Value,now:i64)->(Value,Value){let mut out=vec![];let stats=uasf_page_probe::page(db,r,now,&mut out).unwrap();(uasf_json::from_slice(&out).unwrap(),stats)}
fn compare(s:&Scratch,r:&Value) {
    let expected=s.core().query(r).unwrap();let (db,mut r)=prepared(s,r);let mut assembled=json!({});let mut identity=None;let mut paths=std::collections::BTreeSet::new();
    loop {
        let (p,stats)=page(&db,&r,1000);assert_eq!(p["page"]["complete"],true,"{p}");assert!(stats["stats"]["vm_steps"].as_u64().unwrap()<250000);
        assert_eq!(p["status"],expected["status"]);assert_eq!(p["reasons"],expected["reasons"]);
        if identity.is_none(){identity=Some(p["page"]["traversal_id"].clone());assembled=p["data"]["header"]["fields"].clone();for(c,n)in p["data"]["header"]["arrays"].as_object().unwrap(){assembled[c]=json!(vec![Value::Null;n["length"].as_u64().unwrap() as usize]);}}
        assert_eq!(identity.as_ref().unwrap(),&p["page"]["traversal_id"]);
        for item in p["data"]["items"].as_array().unwrap(){let path=item["path"].as_array().unwrap();assert!(paths.insert(uasf_json::to_string(&item["path"]).unwrap()));let c=path[0].as_str().unwrap();let n=path[1].as_u64().unwrap() as usize;
            if item["kind"]=="object"{let mut v=item["fields"].clone();for(k,n)in item["arrays"].as_object().unwrap(){v[k]=json!(vec![Value::Null;n.as_u64().unwrap() as usize]);}assembled[c][n]=v;}
            else if path.len()==2{assembled[c][n]=item["value"].clone();}else{assembled[c][n][path[2].as_str().unwrap()][path[3].as_u64().unwrap() as usize]=item["value"].clone();}
        }
        if p["page"]["next_cursor"].is_null(){break;}r["page"]["cursor"]=p["page"]["next_cursor"].clone();
    }assert_eq!(assembled,expected["data"]);
}
fn claude_case(variant:&str) {
    let s=Scratch::new();let core=s.core();let mut rows=vec![json!({"uuid":"a","parentUuid":null,"type":"user","sessionId":"S","message":{"content":"hello"}}),json!({"uuid":"b","parentUuid":"a","type":"assistant","sessionId":"S","message":{"content":"reply"}}),json!({"uuid":"c","parentUuid":"b","type":"user","sessionId":"S","message":{"content":{"$serde_json::private::Number":"12345"}}})];
    if variant=="cycle"{rows[0]["parentUuid"]=json!("c");}
    if variant=="duplicate"{rows[1]["uuid"]=json!("a");}
    if variant=="unresolved"{rows[0]["parentUuid"]=json!("absent");}
    let bytes=lines(rows);let cap=capture("C","one","S",None,&bytes);register(&core,"claude",&cap);core.rebuild(&LocalProvider::new(BTreeMap::from([(cap.id(),Binding::Bytes(bytes))]))).unwrap();drop(core);
    compare(&s,&json!({"op":"history","source":"C","session":"S","boundary":[cap.id()],"leaf":"c"}));
}
#[test]fn claude_linear(){claude_case("linear");}
#[test]fn claude_cycle(){claude_case("cycle");}
#[test]fn claude_duplicate(){claude_case("duplicate");}
#[test]fn claude_unresolved(){claude_case("unresolved");}
fn codex_case(session:&str,leaf:&str){let s=Scratch::new();let core=s.core();let (caps,p)=codex();for c in &caps{register(&core,"codex",c);}core.rebuild(&p).unwrap();drop(core);compare(&s,&request(&caps,session,leaf));}
#[test]fn codex_revert_compaction(){codex_case("S","W");}
#[test]fn codex_fork(){codex_case("F","F");}

#[test]fn cursor_replay_tamper_expiration_and_binding(){
    let s=Scratch::new();let core=s.core();let(caps,p)=codex();for c in &caps{register(&core,"codex",c);}core.rebuild(&p).unwrap();
    let(db,mut r)=prepared(&s,&request(&caps,"S","W"));let(first,_)=page(&db,&r,1000);r["page"]["cursor"]=first["page"]["next_cursor"].clone();
    assert_eq!(page(&db,&r,1001).0,page(&db,&r,1001).0);
    assert_eq!(page(&db,&r,1900).0["reasons"][0],"page_cursor_expired");
    assert_eq!(page(&db,&r,999).0["reasons"][0],"page_cursor_clock_untrusted");
    let mut changed=r.clone();changed["page"]["max_records"]=json!(4);assert_eq!(page(&db,&changed,1001).0["reasons"][0],"page_cursor_binding_mismatch");
    changed=r.clone();let mut t=changed["page"]["cursor"].as_str().unwrap().as_bytes().to_vec();t[20]=if t[20]==b'a'{b'b'}else{b'a'};changed["page"]["cursor"]=json!(String::from_utf8(t).unwrap());assert_eq!(page(&db,&changed,1001).0["reasons"][0],"page_cursor_invalid");
}
#[test]fn durable_change_on_second_index_invalidates_old_reader(){
    let s=Scratch::new();let core=s.core();let(caps,p)=codex();for c in &caps{register(&core,"codex",c);}core.rebuild(&p).unwrap();let(db,r)=prepared(&s,&request(&caps,"S","W"));
    assert_eq!(page(&db,&r,1000).0["page"]["complete"],true);
    let other=Core::open(s.0.join("other.sqlite"),s.0.join("persistent.sqlite")).unwrap();other.set_display_name("A","S","changed").unwrap();
    let rejected=page(&db,&r,1001).0;assert_eq!(rejected["data"],json!({}));assert_eq!(rejected["reasons"][0],"page_cursor_data_changed");
}
#[test]fn prepared_position_summary_and_limits(){
    let s=Scratch::new();let core=s.core();let(caps,p)=codex();for c in &caps{register(&core,"codex",c);}core.rebuild(&p).unwrap();let(db,mut r)=prepared(&s,&request(&caps,"S","W"));
    r["range"]=json!({"kind":"positions","collection":"events","start":1,"end":2});let(v,stats)=page(&db,&r,1000);assert_eq!(v["data"]["header"]["arrays"]["events"]["selected_count"],1);assert_eq!(v["data"]["items"][0]["path"],json!(["events",1]));assert!(stats["stats"]["summary_reads"].as_u64().unwrap()<=3);
    r["range"]["start"]=json!(2);let(v,_)=page(&db,&r,1000);assert_eq!(v["page"]["range_complete"],true);assert_eq!(v["data"]["items"],json!([]));
    r["page"]["max_records"]=json!(1025);assert_eq!(page(&db,&r,1000).0["reasons"][0],"page_limit_invalid");
}
#[test]fn events_and_lineage_reassemble(){
    for op in ["events","lineage"]{let s=Scratch::new();let core=s.core();let(caps,p)=codex();for c in &caps{register(&core,"codex",c);}core.rebuild(&p).unwrap();let mut r=request(&caps,"S","W");r["op"]=json!(op);if op=="events"{r.as_object_mut().unwrap().remove("leaf");}compare(&s,&r);}
}
#[test]fn oversized_tail_is_rejected_without_prefix_scan(){
    let s=Scratch::new();let core=s.core();let mut rows=vec![];
    for i in 0..200{rows.push(json!({"type":"user","sessionId":"S","uuid":format!("U{i}"),"parentUuid":null,"message":{"content":if i==199{"x".repeat(4*1024*1024)}else{"small".into()}}}));}
    let bytes=lines(rows);let c=capture("A","large","S",None,&bytes);register(&core,"claude",&c);core.rebuild(&LocalProvider::new(BTreeMap::from([(c.id(),Binding::Bytes(bytes))]))).unwrap();
    let r=json!({"op":"events","source":"A","session":"S","boundary":[c.id()]});let(db,mut r)=prepared(&s,&r);r["page"]["max_bytes"]=json!(4194304);
    let(v,stats)=page(&db,&r,1000);assert_eq!(v["reasons"][0],"page_item_too_large");assert_eq!(v["data"],json!({}));assert!(stats["stats"]["fetched_rows"].as_u64().unwrap()<32);
    r["range"]=json!({"kind":"positions","collection":"events","start":100,"end":199});let(v,stats)=page(&db,&r,1000);assert_eq!(v["page"]["complete"],true);assert_eq!(v["data"]["header"]["arrays"]["events"]["selected_count"],99);assert!(stats["stats"]["fetched_rows"].as_u64().unwrap()<64);
}
#[test]fn repeated_domain_append_requires_new_traversal(){
    let s=Scratch::new();let core=s.core();let(caps,p)=codex();for c in &caps{register(&core,"codex",c);}core.rebuild(&p).unwrap();let base=request(&caps,"S","W");let mut ids=std::collections::BTreeSet::new();
    for n in 0..3{
        let(db,mut r)=prepared(&s,&base);let(v,_)=page(&db,&r,1000);assert!(ids.insert(v["page"]["traversal_id"].as_str().unwrap().to_owned()));r["page"]["cursor"]=v["page"]["next_cursor"].clone();
        // A durable registration is the append-intent boundary. A collector has
        // not parsed the new capture yet; conservative invalidation is immediate.
        let bytes=lines(vec![row(0,"session_meta",json!({"session_id":"S","history_mode":"paginated"}))]);let c=capture("A",&format!("append-{n}"),"S",Some(&format!("append-{n}")),&bytes);core.register_capture(&c).unwrap();
        assert_eq!(page(&db,&r,1001).0["reasons"][0],"page_cursor_data_changed");
    }
}
#[test]fn committed_delete_blocks_continuation_and_new_traversal(){
    let s=Scratch::new();let core=s.core();let(caps,p)=codex();for c in &caps{register(&core,"codex",c);}core.rebuild(&p).unwrap();let(db,mut r)=prepared(&s,&request(&caps,"S","W"));let(first,_)=page(&db,&r,1000);
    core.delete(&json!([{"kind":"session","source":"A","session":"S"}])).unwrap();
    r["page"]["cursor"]=first["page"]["next_cursor"].clone();let rejected=page(&db,&r,1001).0;assert_eq!(rejected["data"],json!({}));assert_eq!(rejected["page"]["complete"],false);
    r["page"]["cursor"]=Value::Null;assert_eq!(page(&db,&r,1002).0["data"],json!({}));
}
