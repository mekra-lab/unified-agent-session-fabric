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

fn compare(s:&Scratch,r:&Value) {
    let core=s.core();let mut expected=Vec::new();core.query_to_writer(r,&mut expected).unwrap();drop(core);
    let db=s.0.join("probe.sqlite");uasf_index_probe::load(&s.0.join("index.sqlite"),&s.0.join("persistent.sqlite"),&db).unwrap();
    let mut actual=Vec::new();uasf_index_probe::query(&db,r,&mut actual).unwrap();assert_eq!(String::from_utf8(actual).unwrap(),String::from_utf8(expected).unwrap());
    let mut untouched=b"prefix".to_vec();assert!(uasf_index_probe::query(&db,&json!({"op":"events"}),&mut untouched).is_err());assert_eq!(untouched,b"prefix");
    let mut bad=r.clone();bad["leaf"]=json!("missing");
    if r["source"]=="A" {let mut untouched=b"prefix".to_vec();assert!(uasf_index_probe::query(&db,&bad,&mut untouched).is_err());assert_eq!(untouched,b"prefix");}
    assert!(!std::fs::read_dir(&s.0).unwrap().any(|x|x.unwrap().path().extension().is_some_and(|x|x=="output")));
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
