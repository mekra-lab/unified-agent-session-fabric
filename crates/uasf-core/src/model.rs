use serde::{Deserialize, Serialize};
use uasf_json::{json, Value};
use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("response delivery interrupted")]
    Delivery(std::io::Error),

    #[error("{0}")]
    Invalid(&'static str),

    #[error("store unavailable")]
    Sql(
        #[from]
        rusqlite::Error),

    #[error("invalid JSON")]
    Json(
        #[from]
        uasf_json::Error),

    #[error("file unavailable")]
    Io(
        #[from]
        std::io::Error),
}
pub fn sha(data: &[u8]) -> String { format!("{:x}", Sha256::digest(data)) }
pub use uasf_json::canonical;
pub fn key(kind: &str, parts: Value) -> String {
    format!("{kind}:{}", sha(canonical(&parts).as_bytes()))
}
pub fn session_key(source: &str, session: &str) -> String {
    key("session", json!([source, session]))
}
pub fn digest_valid(s: &str) -> bool {
    s.len() == 64 &&
        s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn nonempty(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}
pub fn truth(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
pub fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub fn strings(v: &Value) -> Vec<String> {
    arr(v).iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()
}
pub fn sorted_unique(mut xs: Vec<String>) -> Vec<String> {
    xs.sort();
    xs.dedup();
    xs
}
pub fn worst<'a>(states: impl IntoIterator<Item = &'a str>) -> &'static str {
    let states: Vec<_> = states.into_iter().collect();
    ["deleted", "unsupported", "incomplete",
                        "unknown"].into_iter().find(|s|
                states.contains(s)).unwrap_or("complete")
}
pub fn merge(mut base: Value, extra: Value) -> Value {
    base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    base
}
pub fn issue(v: &mut Value, state: &str, reason: &str) {
    v["status"] =
        json!(worst([v["status"].as_str().unwrap_or("complete"), state]));
    if !arr(&v["reasons"]).contains(&json!(reason)) {
        v["reasons"].as_array_mut().unwrap().push(json!(reason));
    }
}
fn fixture() -> String { "fixture".into() }
fn yes() -> bool { true }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub engine: String,
    #[serde(default="fixture")]
    pub observation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub source: String,
    pub capture: String,
    pub generation: String,
    pub length: u64,
    pub sha256: String,
    pub session: String,
    #[serde(default)]
    pub window: Option<String>,
    #[serde(default)]
    pub file_generation: Option<String>,
    #[serde(default="yes")]
    pub native_session: bool,
}
impl Capture {
    pub fn validate(&self) -> Result<()> {
        if [&self.source, &self.capture, &self.generation,
                            &self.session].iter().any(|s| s.is_empty()) {
            return Err(Error::Invalid("capture_identity"));
        }
        if self.window.as_ref().is_some_and(String::is_empty) ||
                self.file_generation.as_ref().is_some_and(String::is_empty) {
            return Err(Error::Invalid("capture_optional_identity"));
        }
        if !digest_valid(&self.sha256) {
            return Err(Error::Invalid("capture_digest"));
        }
        Ok(())
    }
    pub fn id(&self) -> String {
        key("capture", json!([self.source, self.generation, self.capture]))
    }
    pub fn session_id(&self) -> String {
        session_key(&self.source, &self.session)
    }
    pub fn anchor(&self) -> Value {
        self.range_anchor(0, self.length, &self.sha256)
    }
    pub fn range_anchor(&self, start: u64, end: u64, digest: &str) -> Value {
        json!({"source":self.source,"capture":self.capture,"generation":self.generation,"file_generation":self.file_generation,"start":start,"end":end,"sha256":digest})
    }
}
pub fn anchor_id(a: &Value) -> String {
    key("event",
        json!([a["source"],a["generation"],a["capture"],a["start"],a["end"]]))
}
pub fn validate_anchor(a: &Value) -> Result<()> {
    let ok =
        a.as_object().is_some_and(|o|
                                o.keys().all(|k|
                                        ["source", "capture", "generation", "file_generation",
                                                    "start", "end", "sha256"].contains(&k.as_str()))) &&
                        ["source", "capture",
                                        "generation"].iter().all(|k| nonempty(&a[k])) &&
                    a["start"].as_u64().zip(a["end"].as_u64()).is_some_and(|(s,
                                e)| s < e) && a["sha256"].as_str().is_some_and(digest_valid)
            &&
            (a["file_generation"].is_null() ||
                    a["file_generation"].is_string());
    if ok { Ok(()) } else { Err(Error::Invalid("invalid_original_anchor")) }
}

impl uasf_json::FromValue for Source {
    fn from_value(v: Value) -> uasf_json::Result<Self> {
        use uasf_json::from_value as field;
        uasf_json::fields(&v, &["id", "engine", "observation"])?;
        Ok(Self { id: field(v["id"].clone())?, engine: field(v["engine"].clone())?,
            observation: field(v.get("observation").cloned().unwrap_or(json!("fixture")))? })
    }
}
impl uasf_json::FromValue for Capture {
    fn from_value(v: Value) -> uasf_json::Result<Self> {
        use uasf_json::from_value as field;
        uasf_json::fields(&v, &["source", "capture", "generation", "length", "sha256", "session", "window", "file_generation", "native_session"])?;
        Ok(Self {
            source: field(v["source"].clone())?, capture: field(v["capture"].clone())?,
            generation: field(v["generation"].clone())?, length: field(v["length"].clone())?,
            sha256: field(v["sha256"].clone())?, session: field(v["session"].clone())?,
            window: field(v["window"].clone())?, file_generation: field(v["file_generation"].clone())?,
            native_session: field(v.get("native_session").cloned().unwrap_or(json!(true)))?,
        })
    }
}
