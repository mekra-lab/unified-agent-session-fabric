use uasf_json::{json, Value};
use std::{collections::BTreeMap, fs};
use uasf_intake::{Intake, sha};
#[derive(Default)]
struct Normalizer {
    ids: BTreeMap<String, String>,
    times: BTreeMap<String, Value>,
}
impl Normalizer {
    fn normalize(&mut self, v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                if let Some(id) = o.get("receipt_id").and_then(Value::as_str)
                    {
                    assert_eq!(id.len(),36);
                    assert_eq!(&id[14..15],"4");
                    let next = format!("$receipt-{}",self.ids.len()+1);
                    self.ids.entry(id.into()).or_insert(next);
                    if let Some(time) = o.get("received_ns") {
                        assert!(time.as_u64().unwrap()>0);
                        assert_eq!(self.times.entry(id.into()).or_insert(time.clone()),time,"timestamp changed on retry");
                    }
                }
                let mut result: uasf_json::Map<String, Value> =
                    o.iter().map(|(k, v)|
                                (k.clone(), self.normalize(v))).collect();
                if let Some(id) = o.get("receipt_id").and_then(Value::as_str)
                    {
                    let symbol = &self.ids[id];
                    result.insert("receipt_id".into(), json!(symbol));
                    if o.contains_key("received_ns") {
                        result.insert("received_ns".into(),
                            json!(format!("$time-{}",&symbol[1..])));
                    }
                }
                Value::Object(result)
            }
            Value::Array(a) =>
                Value::Array(a.iter().map(|v| self.normalize(v)).collect()),
            _ => v.clone(),
        }
    }
    fn resolve(&self, v: &Value) -> Value {
        match v {
            Value::String(s) if s.starts_with("$receipt-") =>
                json!(self.ids.iter().find(|(_,alias)|*alias==s).unwrap().0),
            Value::Array(a) =>
                Value::Array(a.iter().map(|v| self.resolve(v)).collect()),
            Value::Object(o) =>
                Value::Object(o.iter().map(|(k, v)|
                                (k.clone(), self.resolve(v))).collect()),
            _ => v.clone(),
        }
    }
}
#[test]
fn frozen_workflow_survives_restart_without_python() {
    let bytes = include_bytes!("golden.json");
    assert_eq!(sha(bytes),include_str!("golden.sha256").trim());
    let frozen = uasf_json::from_slice(bytes).unwrap();
    let cases = frozen.as_array().unwrap();
    assert_eq!(cases.len(),51);
    let root =
        std::env::temp_dir().join(format!("intake-golden-{}",std::process::id()));
    fs::create_dir(&root).unwrap();
    let mut normal = Normalizer::default();
    for (i, case) in cases.iter().enumerate() {
        // Reopen on every operation: no receipt map or tombstone can live only in RAM.
        let mut store =
            Intake::open(&root.join("intake.db"),
                    json!({"A":["app"],"B":["app"]})).unwrap();
        let request = normal.resolve(&case["request"]);
        let result =
            if request["op"] == "erase" {
                let ids: Vec<String> =
                    uasf_json::from_value(request["receipt_ids"].clone()).unwrap();
                store.erase(&ids, None).unwrap()
            } else { store.execute(&request, "app").unwrap() };
        assert_eq!(normal.normalize(&result),case["response"],"golden operation {i}");
    }
    fs::remove_dir_all(root).unwrap();
}
