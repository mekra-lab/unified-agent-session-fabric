use std::{collections::BTreeMap, fs::File, io::Read, path::PathBuf};
use crate::model::{sha, Capture};
pub const LIMIT: u64 = 32 * 1024 * 1024;
#[derive(Debug, Clone)]
pub struct ByteResult {
    pub data: Option<Vec<u8>>,
    pub sha256: Option<String>,
    pub verification: String,
    pub reason: Option<String>,
}
impl ByteResult {
    pub fn denied(state: &str, reason: &str) -> Self {
        Self {
            data: None,
            sha256: None,
            verification: state.into(),
            reason: Some(reason.into()),
        }
    }
}
pub trait ByteProvider {
    fn read(&self, capture: &Capture, start: u64, end: u64)
    -> ByteResult;
}
#[derive(Clone)]
pub enum Binding { File(PathBuf), Bytes(Vec<u8>), }
pub struct LocalProvider {
    pub bindings: BTreeMap<String, Binding>,
    pub verified: bool,
}
impl LocalProvider {
    pub fn new(bindings: BTreeMap<String, Binding>) -> Self {
        Self { bindings, verified: true }
    }
}
impl ByteProvider for LocalProvider {
    fn read(&self, c: &Capture, start: u64, end: u64) -> ByteResult {
        if start > end || end > c.length || c.length > LIMIT {
            return ByteResult::denied("unsupported", "range_or_size_limit");
        }
        let data =
            match self.bindings.get(&c.id()) {
                Some(Binding::Bytes(b)) =>
                    b[..b.len().min(c.length as usize)].to_vec(),
                Some(Binding::File(p)) => {
                    let mut b = Vec::new();
                    match File::open(p).and_then(|f|
                                f.take(c.length).read_to_end(&mut b)) {
                        Ok(_) => b,
                        Err(_) =>
                            return ByteResult::denied("unverified",
                                    "fixture_unavailable"),
                    }
                }
                None =>
                    return ByteResult::denied("unverified",
                            "fixture_unavailable"),
            };
        if data.len() as u64 != c.length || sha(&data) != c.sha256 {
            return ByteResult::denied("mismatch", "pinned_capture_mismatch");
        }
        let part = data[start as usize..end as usize].to_vec();
        ByteResult {
            sha256: Some(sha(&part)),
            data: Some(part),
            verification: if self.verified {
                    "verified"
                } else { "unverified" }.into(),
            reason: (!self.verified).then(||
                    "independent_verification_not_supplied".into()),
        }
    }
}
