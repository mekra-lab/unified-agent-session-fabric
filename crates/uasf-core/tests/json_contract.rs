use std::{collections::BTreeMap, path::PathBuf, sync::atomic::{AtomicUsize, Ordering}};
use uasf_json::{json, Value};
use uasf_core::{model::sha, Binding, Capture, Core, LocalProvider, Source};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture { root: PathBuf, capture: Capture, provider: LocalProvider }
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("uasf-json-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&root).unwrap();
        let raw = br#"{"type":"api-record","schema":"uasf.synthetic-api/1","conversation":"S","call_id":"call","complete":true,"request":{"$serde_json::private::Number":"123"}}
"#.to_vec();
        let capture = Capture { source: "A".into(), capture: "submission".into(), generation: "generation".into(), length: raw.len() as u64, sha256: sha(&raw), session: "S".into(), window: None, file_generation: None, native_session: false };
        let provider = LocalProvider::new(BTreeMap::from([(capture.id(), Binding::Bytes(raw))]));
        Self { root, capture, provider }
    }
    fn core(&self) -> Core { Core::open(self.root.join("index.sqlite"), self.root.join("persistent.sqlite")).unwrap() }
    fn install(&self, core: &Core) {
        core.register_source(&Source { id: "A".into(), engine: "api".into(), observation: "fixture".into() }).unwrap();
        core.register_capture(&self.capture).unwrap();
        core.rebuild(&self.provider).unwrap();
    }
    fn evidence(&self, body: Value) -> Value { json!({"evidence_id":"receipt","source":"A","session":"S","fact":"received","assessment":"observed","method":"synthetic","observed_at":"synthetic-time","anchors":[self.capture.anchor()],"body":body}) }
    fn events(&self, core: &Core) -> Value { core.query(&json!({"op":"events","source":"A","session":"S","boundary":[self.capture.id()]})).unwrap() }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); } }

#[test]
fn marker_object_survives_original_parser_index_and_rebuild() {
    let f = Fixture::new();
    {
        let core = f.core(); f.install(&core);
        assert_eq!(f.events(&core)["data"]["events"][0]["body"]["request"], json!({"$serde_json::private::Number":"123"}));
    }
    std::fs::remove_file(f.root.join("index.sqlite")).unwrap();
    let core = f.core(); core.rebuild(&f.provider).unwrap();
    assert_eq!(f.events(&core)["data"]["events"][0]["body"]["request"], json!({"$serde_json::private::Number":"123"}));
}

#[test]
fn marker_evidence_survives_durable_reread_and_idempotent_submission() {
    let f = Fixture::new();
    let body = json!({"$serde_json::private::Number":"123", "next": [{"$serde_json::private::Number":"not a number"}], "$serde_json::private::RawValue":"false"});
    let evidence = f.evidence(body.clone());
    { let core = f.core(); f.install(&core); core.add_evidence(&evidence).unwrap(); }
    let core = f.core();
    core.add_evidence(&evidence).unwrap();
    let facts = core.query(&json!({"op":"evidence","source":"A","session":"S"})).unwrap();
    assert_eq!(facts["data"]["facts"][0]["body"], body);
}

#[test]
fn evidence_numeric_spelling_is_idempotent_but_types_and_zero_sign_are_distinct() {
    let f = Fixture::new(); let core = f.core(); f.install(&core);
    for text in ["1e0", "1.00", "1.0"] {
        core.add_evidence(&f.evidence(uasf_json::from_str(text).unwrap())).unwrap();
    }
    assert!(core.add_evidence(&f.evidence(json!(1))).is_err());
    assert!(core.add_evidence(&f.evidence(json!(true))).is_err());
    let mut minus = f.evidence(json!(-0.0)); minus["evidence_id"] = json!("zero");
    core.add_evidence(&minus).unwrap();
    let mut plus = minus; plus["body"] = json!(0.0);
    assert!(core.add_evidence(&plus).is_err());
}

#[test]
fn decoder_preserves_marker_keys_at_every_position_and_roundtrips() {
    use uasf_json::from_str;
    for raw in [
        r#"{"$serde_json::private::Number":"123"}"#,
        r#"{"$serde_json::private::Number":"not numeric","extra":17}"#,
        r#"{"first":1,"$serde_json::private::Number":"123"}"#,
        r#"{"\u0024serde_json::private::Number":{"nested":false}}"#,
        r#"[{"$serde_json::private::Number":null},{"$serde_json::private::RawValue":"[1]"}]"#,
        r#"{"$serde_json::private::Number":"123","large":1267650600228229401496703205376,"float":1e0}"#,
    ] {
        let decoded = from_str(raw).unwrap();
        assert!(decoded.is_object() || decoded.is_array());
        let roundtrip = from_str(&uasf_json::to_string(&decoded).unwrap()).unwrap();
        assert_eq!(decoded, roundtrip);
    }
    let object = from_str(r#"{"\u0024serde_json::private::Number":"123"}"#).unwrap();
    assert_eq!(object, json!({"$serde_json::private::Number":"123"}));
    assert_eq!(from_str(r#"{"x":1,"x":2,"y":3}"#).unwrap(), json!({"x":2,"y":3}));
}

#[test]
fn decoder_normalizes_floats_without_losing_integer_precision_or_zero_sign() {
    use uasf_core::{json::from_str, model::canonical};
    let first = from_str("1e0").unwrap();
    for text in ["1.00", "1.0", "10e-1"] { assert_eq!(first, from_str(text).unwrap()); }
    assert_ne!(canonical(&first), canonical(&from_str("1").unwrap()));
    assert_ne!(canonical(&from_str("-0.0").unwrap()), canonical(&from_str("0.0").unwrap()));
    assert_eq!(from_str("-0").unwrap(), json!(0));
    assert_eq!(canonical(&from_str("1267650600228229401496703205376").unwrap()), "1267650600228229401496703205376");
    for text in ["{} trailing", "[01]", "1e9999", "NaN"] { assert!(from_str(text).is_err()); }
}

#[test]
fn shared_json_corpus_original_parser_and_database_reread() {
    let cases = uasf_json::from_str(include_str!("../../uasf-json/tests/inspection.json")).unwrap();
    for c in cases.as_array().unwrap() {
        let mut f = Fixture::new();
        let raw = format!("{{\"type\":\"api-record\",\"schema\":\"uasf.synthetic-api/1\",\"conversation\":\"S\",\"call_id\":\"call\",\"complete\":true,\"request\":{}}}\n",c["input"].as_str().unwrap().trim()).into_bytes();
        f.capture.length = raw.len() as u64; f.capture.sha256 = sha(&raw);
        f.provider = LocalProvider::new(BTreeMap::from([(f.capture.id(),Binding::Bytes(raw))]));
        { let core = f.core(); f.install(&core); }
        let core = f.core();
        let event = f.events(&core)["data"]["events"][0].clone();
        if c["decode_ok"] == true {
            assert!(event["body"]["request"].is_object(),"{}", c["name"]);
            assert_eq!(event["body"]["request"],uasf_json::from_str(c["input"].as_str().unwrap()).unwrap());
        } else {
            assert_eq!(event["status"],"unsupported","{}",c["name"]);
            assert!(event["body"].is_null());
        }
    }
}
