//! Authenticated owner observations, ordered inbox admission and deletion barriers.
pub mod io;
use std::{
    fs, path::{Path, PathBuf},
    os::unix::fs::MetadataExt,
};
use sha2::{Digest, Sha256};
use uasf_json::{json, Value};
pub const LIMIT: u64 = 32 * 1024 * 1024;
pub const HEADER: usize = 16 * 1024;
#[derive(Debug)]
pub struct Error(pub &'static str);
pub type Result<T> = std::result::Result<T, Error>;
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self { Self("OSError") }
}
impl From<uasf_json::Error> for Error {
    fn from(_: uasf_json::Error) -> Self { Self("invalid_json") }
}
impl From<uasf_staging::Error> for Error {
    fn from(e: uasf_staging::Error) -> Self {
        match e {
            uasf_staging::Error::Denied(s) => Self(s),
            _ => Self("staging_unavailable"),
        }
    }
}
pub fn check(b: bool, s: &'static str) -> Result<()> {
    if b { Ok(()) } else { Err(Error(s)) }
}
fn text(v: &Value) -> Result<&str> {
    v.as_str().ok_or(Error("invalid_string"))
}
fn arr(v: &Value) -> Result<&Vec<Value>> {
    v.as_array().ok_or(Error("invalid_array"))
}
fn fields(v: &Value, names: &[&str]) -> bool {
    v.as_object().is_some_and(|o|
            o.len() == names.len() &&
                names.iter().all(|n| o.contains_key(*n)))
}
fn integer(v: &Value, low: u64) -> Result<u64> {
    v.as_u64().filter(|n|
                *n >= low &&
                    *n <= i64::MAX as u64).ok_or(Error("invalid_integer"))
}
fn hex(s: &str, n: usize) -> bool {
    s.len() == n &&
        s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn generation(v: &str) -> Result<()> {
    check(hex(v, 32), "invalid_generation")
}
pub fn sha(b: &[u8]) -> String { format!("{:x}",Sha256::digest(b)) }
pub fn uuid() -> Result<String> { Ok(uasf_staging::uuid()?.replace('-', "")) }
pub fn parse(b: &[u8]) -> Result<Value> {
    uasf_json::from_slice_unique(b).map_err(|e|
            if e.to_string().contains("duplicate_json_key") {
                Error("duplicate_json_key")
            } else { Error("invalid_json") })
}
/// Python-compatible ASCII canonical header, including UTF-16 surrogate escapes.
pub fn canonical(v: &Value) -> Vec<u8> {
    let s = uasf_json::canonical(v);
    let mut out = String::new();
    for c in s.chars() {
        if c as u32 >= 127 {
            let mut units = [0; 2];
            for u in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        } else { out.push(c) }
    }
    out.into_bytes()
}
fn token(source: &str, path: &str) -> String {
    sha(&canonical(&json!([source,path])))
}
pub fn jsonl(data: &[u8]) -> Result<()> {
    check(data.is_empty() || data.ends_with(b"\n"), "incomplete_jsonl")?;
    for line in data.split_inclusive(|b| *b == b'\n') {
        check(parse(&line[..line.len() - 1])?.is_object(),
                "jsonl_object_required")?;
    }
    Ok(())
}
pub fn allowed(spec: &Value, path: &str) -> Result<bool> {
    io::relative(path)?;
    if let Some(files) = spec.get("files") {
        return Ok(arr(files)?.contains(&json!(path)) &&
                    path.ends_with(".jsonl"));
    }
    let parts: Vec<_> = path.split('/').collect();
    Ok(spec["pattern"] == "native-jsonl" && parts.len() >= 2 &&
                path.ends_with(".jsonl") &&
            ((spec["kind"] == "codex" && parts[0] == "sessions" &&
                            parts.last().unwrap().starts_with("rollout-")) ||
                    (spec["kind"] == "claude" && parts[0] == "projects")))
}
pub fn load_config(path: &Path) -> Result<Value> {
    let (raw, m) = io::bytes(path, (HEADER * 4) as u64)?;
    check((m.uid() == 0 || m.uid() == unsafe { libc::geteuid() }) &&
                m.mode() & 0o022 == 0, "config_owner_or_mode")?;
    let c = parse(&raw)?;
    check(fields(&c,
                    &["version", "stream", "producer_uid", "collector_uid",
                                "sources"]) && c["version"] == 1, "config_schema")?;
    generation(text(&c["stream"])?)?;
    integer(&c["producer_uid"], 0)?;
    integer(&c["collector_uid"], 0)?;
    let sources =
        c["sources"].as_object().filter(|o|
                        !o.is_empty()).ok_or(Error("sources_required"))?;
    for (source, spec) in sources {
        check(!source.is_empty() && source.len() <= 64 &&
                        source.as_bytes()[0].is_ascii_alphanumeric() &&
                    source.bytes().all(|b|
                            b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "source_id")?;
        check(fields(spec, &["root", "kind", "files"]) ||
                    fields(spec, &["root", "kind", "pattern"]),
                "explicit_files_or_pattern_required")?;
        io::absolute(Path::new(text(&spec["root"])?))?;
        check(spec["kind"] == "codex" || spec["kind"] == "claude",
                "source_kind")?;
        if let Some(files) = spec.get("files") {
            check(!arr(files)?.is_empty(), "files_required")?;
            for p in arr(files)? {
                io::relative(text(p)?)?;
                check(text(p)?.ends_with(".jsonl"), "jsonl_only")?;
            }
        } else {
            check(spec["pattern"] == "native-jsonl", "unsupported_pattern")?;
        }
    }
    Ok(c)
}
pub fn load_key(path: &Path) -> Result<Vec<u8>> {
    let (k, m) = io::bytes(path, 32)?;
    check(k.len() == 32 &&
                    (m.uid() == 0 || m.uid() == unsafe { libc::geteuid() }) &&
                m.mode() & 0o7777 == 0o600, "key_owner_mode_or_length")?;
    Ok(k)
}
fn mac(key: &[u8], meta: &Value, payload: &[u8]) -> Vec<u8> {
    let mut inner = [0x36u8; 64];
    let mut outer = [0x5cu8; 64];
    let k =
        if key.len() > 64 {
            Sha256::digest(key).to_vec()
        } else { key.to_vec() };
    for (i, b) in k.iter().enumerate() { inner[i] ^= b; outer[i] ^= b; }
    let mut h = Sha256::new();
    h.update(inner);
    h.update(b"UASF-FEEDER-1\0");
    h.update(canonical(meta));
    h.update(b"\0");
    h.update(payload);
    let mut out = Sha256::new();
    out.update(outer);
    out.update(h.finalize());
    out.finalize().to_vec()
}
fn mac_hex(k: &[u8], m: &Value, p: &[u8]) -> String {
    mac(k, m, p).iter().map(|b| format!("{b:02x}")).collect()
}
pub fn seal(meta: &Value, payload: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    let mut h =
        canonical(&json!({"meta":meta,"mac":mac_hex(key,meta,payload)}));
    check(h.len() <= HEADER, "header_too_large")?;
    h.push(b'\n');
    h.extend_from_slice(payload);
    check(h.len() as u64 <= LIMIT, "record_over_32_MiB")?;
    Ok(h)
}
pub fn unseal(raw: &[u8], key: &[u8], cfg: &Value)
    -> Result<(Value, Vec<u8>)> {
    check(raw.len() as u64 <= LIMIT, "record_over_32_MiB")?;
    let at =
        raw.iter().take(HEADER +
                            1).position(|b|
                        *b == b'\n').ok_or(Error("header_missing_or_oversized"))?;
    let e = parse(&raw[..at])?;
    let payload = &raw[at + 1..];
    check(fields(&e, &["meta", "mac"]), "envelope_schema")?;
    let m = &e["meta"];
    let tag = text(&e["mac"])?;
    check(m.is_object() && hex(tag, 64), "mac_shape")?;
    let expected = mac_hex(key, m, payload);
    check(tag.bytes().zip(expected.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b))
                == 0, "authentication_failed")?;
    check(fields(m,
                &["version", "stream", "config_sha256", "source", "path",
                            "kind", "generation", "sequence", "previous", "device",
                            "inode", "mtime_ns", "ctime_ns", "source_length",
                            "source_sha256", "length", "sha256", "withheld_length",
                            "policy_revision"]), "metadata_schema")?;
    check(m["version"] == 1 && m["stream"] == cfg["stream"] &&
                m["config_sha256"] == sha(&canonical(cfg)),
            "stream_or_config_mismatch")?;
    generation(text(&m["generation"])?)?;
    check(m["previous"].is_null() ||
                m["previous"].as_str().is_some_and(|s| hex(s, 32)),
            "previous_generation")?;
    for n in
        ["sequence", "device", "inode", "mtime_ns", "ctime_ns",
                "source_length", "length", "withheld_length",
                "policy_revision"] {
        integer(&m[n], if n == "sequence" { 1 } else { 0 })?;
    }
    let source = text(&m["source"])?;
    let path = text(&m["path"])?;
    io::relative(path)?;
    let spec =
        cfg["sources"].get(source).ok_or(Error("unregistered_source"))?;
    check(m["kind"] == spec["kind"] && allowed(spec, path)?,
            "unregistered_path")?;
    for n in ["sha256", "source_sha256"] {
        check(m[n].as_str().is_some_and(|s| hex(s, 64)), "hash_shape")?;
    }
    check(m["source_length"].as_u64().unwrap() <= LIMIT &&
                        m["length"] == payload.len() &&
                    m["source_length"].as_u64() ==
                        m["length"].as_u64().unwrap().checked_add(m["withheld_length"].as_u64().unwrap())
                && m["sha256"] == sha(payload), "length_or_hash_mismatch")?;
    check(m["withheld_length"] != 0 || m["source_sha256"] == m["sha256"],
            "full_hash_mismatch")?;
    jsonl(payload)?;
    Ok((m.clone(), payload.to_vec()))
}

pub fn initialize(root: &Path, cfg: &Value, key: &[u8], role: &str)
    -> Result<Value> {
    io::private(root)?;
    check(["producer", "collector"].contains(&role),
            "operation_role_mismatch")?;
    check(cfg[format!("{role}_uid")] == unsafe { libc::geteuid() },
            "wrong_role_uid")?;
    io::atomic(&root.join("lock"), b"", 0o600, false, false)?;
    for d in ["records", "validated", "tmp"] {
        fs::create_dir(root.join(d))?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.join(d), fs::Permissions::from_mode(0o700))?;
    }
    io::sync(root)?;
    let binding = sha(&[canonical(cfg), key.to_vec()].concat());
    io::atomic(&root.join("state.json"),
            &canonical(&json!({"version":1,"binding":binding,"role":role,"heads":{},"records":{},"policy_revision":0,"paths":[],"inodes":[]})),
            0o600, false, false)?;
    Ok(json!({"status":"initialized"}))
}
pub struct Endpoint {
    pub cfg: Value,
    key: Vec<u8>,
    pub root: PathBuf,
    pub policy: PathBuf,
    pub inbox: PathBuf,
    role: String,
}
impl Endpoint {
    pub fn new(config: &Path, key: &Path, state: &Path, policy: &Path,
        inbox: &Path, role: &str) -> Result<Self> {
        let cfg = load_config(config)?;
        let key = load_key(key)?;
        let root = io::private(state)?;
        check(["producer", "collector"].contains(&role),
                "operation_role_mismatch")?;
        check(cfg[format!("{role}_uid")] == unsafe { libc::geteuid() },
                "wrong_role_uid")?;
        let m = io::directory(policy, false)?.metadata()?;
        check(cfg["collector_uid"] == m.uid() && m.mode() & 0o022 == 0,
                "policy_directory_owner_or_mode")?;
        let (_, m) = io::bytes(&policy.join("lock"), 0)?;
        check(cfg["collector_uid"] == m.uid() && m.mode() & 0o022 == 0,
                "policy_lock_writable_by_others")?;
        let m = io::directory(inbox, true)?.metadata()?;
        check(cfg["collector_uid"] == m.uid() && m.mode() & 0o7777 == 0o3730,
                "inbox_owner_or_mode")?;
        for spec in cfg["sources"].as_object().unwrap().values() {
            let src = Path::new(text(&spec["root"])?);
            for dest in [&root, policy, inbox] {
                check(!src.starts_with(dest) && !dest.starts_with(src),
                        "source_runtime_overlap")?;
            }
        }
        Ok(Self {
                cfg,
                key,
                root,
                policy: policy.to_owned(),
                inbox: inbox.to_owned(),
                role: role.into(),
            })
    }
    fn require_role(&self, r: &str) -> Result<()> {
        check(self.role == r, "operation_role_mismatch")
    }
    fn policy_load(&self) -> Result<Value> {
        let (raw, m) =
            io::bytes(&self.policy.join("deleted.json"), 4 * 1024 * 1024)?;
        check(m.uid() as u64 == integer(&self.cfg["collector_uid"], 0)? &&
                    m.mode() & 0o022 == 0, "policy_writable_by_others")?;
        let p = parse(&raw)?;
        check(fields(&p,
                            &["version", "stream", "revision", "paths", "inodes"]) &&
                        p["version"] == 1 && p["stream"] == self.cfg["stream"],
                "policy_schema")?;
        integer(&p["revision"], 0)?;
        for pair in arr(&p["paths"])? {
            let a = arr(pair)?;
            check(a.len() == 2 &&
                        self.cfg["sources"].get(text(&a[0])?).is_some(),
                    "policy_path")?;
            io::relative(text(&a[1])?)?;
        }
        for pair in arr(&p["inodes"])? {
            let a = arr(pair)?;
            check(a.len() == 2, "policy_inode")?;
            for v in a { integer(v, 0)?; }
        }
        Ok(p)
    }
    fn held(&self, write: bool) -> Result<(io::Lock, Value)> {
        let l = io::Lock::acquire(&self.policy.join("lock"), !write)?;
        Ok((l, self.policy_load()?))
    }
    fn state(&self, p: &Value) -> Result<(io::Lock, Value)> {
        let l = io::Lock::acquire(&self.root.join("lock"), false)?;
        let (raw, m) = io::bytes(&self.root.join("state.json"), LIMIT)?;
        check(m.uid() == unsafe { libc::geteuid() } &&
                    m.mode() & 0o7777 == 0o600, "state_mode")?;
        let mut s = parse(&raw)?;
        check(s["version"] == 1 && s["role"] == self.role &&
                    s["binding"] ==
                        sha(&[canonical(&self.cfg), self.key.clone()].concat()),
                "state_binding")?;
        check(integer(&p["revision"], 0)? >=
                            integer(&s["policy_revision"], 0)? &&
                        arr(&s["paths"])?.iter().all(|v|
                                p["paths"].as_array().is_some_and(|a| a.contains(v))) &&
                    arr(&s["inodes"])?.iter().all(|v|
                            p["inodes"].as_array().is_some_and(|a| a.contains(v))),
                "policy_rollback")?;
        check(s["records"].is_object() && s["heads"].is_object(),
                "state_binding")?;
        for h in s["heads"].as_object().unwrap().values() {
            check(fields(h, &["sequence", "generation"]), "state_binding")?;
            integer(&h["sequence"], 1)?;
            generation(text(&h["generation"])?)?;
        }
        for (g, r) in s["records"].as_object().unwrap() {
            generation(g)?;
            check(r.is_object() && r["deleted"].is_boolean() &&
                        r["meta"].is_object(), "state_binding")?;
            let m = &r["meta"];
            check(m["generation"] == *g &&
                            self.cfg["sources"].get(text(&m["source"])?).is_some() &&
                        (m["kind"] == "claude" || m["kind"] == "codex"),
                    "state_binding")?;
            io::relative(text(&m["path"])?)?;
            integer(&m["device"], 0)?;
            integer(&m["inode"], 0)?;
            if let Some(link) = r.get("staging_link") {
                check(link.is_object(), "state_binding")?;
            }
        }
        s["policy_revision"] = p["revision"].clone();
        s["paths"] = p["paths"].clone();
        s["inodes"] = p["inodes"].clone();
        self.save(&s)?;
        for e in fs::read_dir(self.root.join("records"))? {
            let path = e?.path();
            let name = path.file_name().unwrap().to_string_lossy();
            if name.starts_with(".part-") ||
                    (name.ends_with(".rec") &&
                            s["records"].get(&name[..name.len() - 4]).is_none()) {
                fs::remove_file(path)?;
            }
        }
        for e in fs::read_dir(self.root.join("tmp"))? {
            fs::remove_file(e?.path())?;
        }
        io::sync(&self.root.join("records"))?;
        io::sync(&self.root.join("tmp"))?;
        Ok((l, s))
    }
    fn save(&self, s: &Value) -> Result<()> {
        io::atomic(&self.root.join("state.json"), &canonical(s), 0o600, true,
            false)
    }
    fn admitted(&self, source: &str, path: &str) -> Result<&Value> {
        io::relative(path)?;
        let spec =
            self.cfg["sources"].get(source).ok_or(Error("unregistered_source"))?;
        check(allowed(spec, path)?, "unregistered_path")?;
        Ok(spec)
    }
    fn policy_check(p: &Value, source: &str, path: &str, id: Option<Value>)
        -> Result<()> {
        check(!arr(&p["paths"])?.contains(&json!([source,path])) &&
                id.is_none_or(|v|
                        !p["inodes"].as_array().unwrap().contains(&v)), "deleted")
    }
    pub fn output(&self, m: &Value) -> Result<PathBuf> {
        let g = text(&m["generation"])?;
        generation(g)?;
        let source = text(&m["source"])?;
        check(self.cfg["sources"].get(source).is_some(),
                "unregistered_source")?;
        let base = self.root.join("validated").join(source);
        Ok(if m["kind"] == "codex" {
                base.join("sessions").join(g).join("rollout-feeder.jsonl")
            } else {
                base.join("projects").join(g).join(format!("{}-{}-{}-{}-{}.jsonl",&g[..8],&g[8..12],&g[12..16],&g[16..20],&g[20..]))
            })
    }
    fn purge_locked(&self, s: &mut Value, p: &Value) -> Result<()> {
        for r in s["records"].as_object_mut().unwrap().values_mut() {
            let m = &r["meta"];
            if arr(&p["paths"])?.contains(&json!([m["source"],m["path"]])) ||
                    arr(&p["inodes"])?.contains(&json!([m["device"],m["inode"]]))
                {
                r["deleted"] = true.into();
            }
        }
        self.save(s)?;
        for (g, r) in s["records"].as_object_mut().unwrap() {
            if r["deleted"] == true {
                generation(g)?;
                io::remove(&self.root.join("records").join(format!("{g}.rec")))?;
                io::remove(&self.output(&r["meta"])?)?;
                let minimal =
                    ["source", "path", "kind", "generation", "device",
                                        "inode"].into_iter().map(|k|
                                (k.to_owned(), r["meta"][k].clone())).collect();
                r["meta"] = Value::Object(minimal);
                if let Some(link) = r.get_mut("staging_link") {
                    *link =
                        Value::Object(["staging_root", "staging_generation",
                                                "staging_relative"].into_iter().map(|k|
                                        (k.to_owned(), link[k].clone())).collect());
                }
            }
        }
        self.save(s)
    }
    pub fn purge(&self) -> Result<Value> {
        let (_p, p) = self.held(false)?;
        let (_s, mut s) = self.state(&p)?;
        self.purge_locked(&mut s, &p)?;
        Ok(json!({"status":"purged","downstream_deletion":"required"}))
    }
    fn deliver(&self, g: &str, raw: &[u8]) -> Result<String> {
        let name = format!("{g}-{}.rec",uuid()?);
        io::atomic(&self.inbox.join(&name), raw, 0o640, false, true)?;
        Ok(name)
    }
    pub fn emit(&self, source: &str, path: &str,
        hook: &mut dyn FnMut(&str) -> Result<()>) -> Result<Value> {
        self.require_role("producer")?;
        let spec = self.admitted(source, path)?;
        let (_p, p) = self.held(false)?;
        let (_s, mut s) = self.state(&p)?;
        self.purge_locked(&mut s, &p)?;
        Self::policy_check(&p, source, path, None)?;
        let (raw, m) =
            io::read(&Path::new(text(&spec["root"])?).join(path), LIMIT,
                    Some(integer(&self.cfg["producer_uid"], 0)? as u32),
                    &mut |m|
                            Self::policy_check(&p, source, path,
                                Some(json!([m.dev(),m.ino()]))), &mut || hook("read"))?;
        let end = raw.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let prefix = &raw[..end];
        jsonl(prefix)?;
        if !raw.is_empty() && prefix.is_empty() {
            return Ok(json!({"status":"not_captured","reason":"no_complete_line"}));
        }
        let t = token(source, path);
        let old = &s["heads"][&t];
        let seq =
            if old.is_null() {
                1
            } else {
                integer(&old["sequence"],
                                    1)?.checked_add(1).ok_or(Error("invalid_integer"))?
            };
        check(seq <= i64::MAX as u64, "invalid_integer")?;
        let g = uuid()?;
        let meta =
            json!({"version":1,"stream":self.cfg["stream"],"config_sha256":sha(&canonical(&self.cfg)),"source":source,"path":path,"kind":spec["kind"],"generation":g,"sequence":seq,"previous":old["generation"],"device":m.dev(),"inode":m.ino(),"mtime_ns":m.mtime()
                    as i128*1_000_000_000+m.mtime_nsec()as
                    i128,"ctime_ns":m.ctime()as
                    i128*1_000_000_000+m.ctime_nsec()as
                    i128,"source_length":raw.len(),"source_sha256":sha(&raw),"length":end,"sha256":sha(prefix),"withheld_length":raw.len()-end,"policy_revision":p["revision"]});
        let wire = seal(&meta, prefix, &self.key)?;
        io::atomic(&self.root.join("records").join(format!("{g}.rec")), &wire,
                0o600, false, false)?;
        s["records"][&g] = json!({"meta":meta,"deleted":false});
        s["heads"][t] = json!({"sequence":seq,"generation":g});
        self.save(&s)?;
        hook("reserved")?;
        let name = self.deliver(&g, &wire)?;
        Ok(json!({"status":"sent","generation":g,"sequence":seq,"record":name,"withheld_length":raw.len()-end}))
    }
    pub fn retry(&self, g: &str) -> Result<Value> {
        self.require_role("producer")?;
        generation(g)?;
        let (_p, p) = self.held(false)?;
        let (_s, mut s) = self.state(&p)?;
        self.purge_locked(&mut s, &p)?;
        let r = s["records"].get(g).ok_or(Error("unknown_generation"))?;
        check(r["deleted"] == false, "deleted")?;
        let (raw, _) =
            io::bytes(&self.root.join("records").join(format!("{g}.rec")),
                    LIMIT)?;
        let (m, _) = unseal(&raw, &self.key, &self.cfg)?;
        check(m == r["meta"], "outbox_integrity")?;
        Ok(json!({"status":"sent","generation":g,"record":self.deliver(g,&raw)?}))
    }
    fn materialize_locked(&self, s: &Value, g: &str, p: &Value)
        -> Result<Value> {
        let r = s["records"].get(g).ok_or(Error("unknown_generation"))?;
        check(r["deleted"] == false, "deleted")?;
        let (raw, stat) =
            io::bytes(&self.root.join("records").join(format!("{g}.rec")),
                    LIMIT)?;
        check(stat.uid() == unsafe { libc::geteuid() }, "wrong_owner")?;
        let (m, payload) = unseal(&raw, &self.key, &self.cfg)?;
        check(m == r["meta"], "collector_record_integrity")?;
        Self::policy_check(p, text(&m["source"])?, text(&m["path"])?,
                Some(json!([m["device"],m["inode"]])))?;
        let out = self.output(&m)?;
        io::mkdirs(out.parent().unwrap())?;
        if out.exists() {
            let (current, st) = io::bytes(&out, LIMIT)?;
            check(st.uid() == unsafe { libc::geteuid() } &&
                        current == payload, "validated_copy_tampered")?;
        } else {
            let tmp = self.root.join("tmp").join(g);
            io::atomic(&tmp, &payload, 0o400, true, false)?;
            fs::rename(&tmp, &out)?;
            io::sync(out.parent().unwrap())?;
            io::sync(tmp.parent().unwrap())?;
        }
        Ok(json!({"status":"validated","generation":g,"path":out,"receipt":m,"provenance":"authenticated_owner_observation","staging_publication":"required"}))
    }
    pub fn materialize(&self, g: &str) -> Result<Value> {
        self.require_role("collector")?;
        generation(g)?;
        let (_p, p) = self.held(false)?;
        let (_s, mut s) = self.state(&p)?;
        self.purge_locked(&mut s, &p)?;
        self.materialize_locked(&s, g, &p)
    }
    pub fn accept(&self, name: &str, hook: &mut dyn FnMut(&str) -> Result<()>)
        -> Result<Value> {
        self.require_role("collector")?;
        check(name.len() == 69 && name.as_bytes()[32] == b'-' &&
                            name.ends_with(".rec") && hex(&name[..32], 32) &&
                    hex(&name[33..65], 32), "invalid_record_name")?;
        let (_p, p) = self.held(false)?;
        let (_s, mut s) = self.state(&p)?;
        self.purge_locked(&mut s, &p)?;
        let (raw, m) = io::bytes(&self.inbox.join(name), LIMIT)?;
        check(self.cfg["producer_uid"] == m.uid(), "wrong_owner")?;
        check(m.mode() & 0o7777 == 0o640 && m.nlink() == 1,
                "record_mode_or_links")?;
        let (m, _) = unseal(&raw, &self.key, &self.cfg)?;
        let g = text(&m["generation"])?;
        check(name.starts_with(&format!("{g}-")), "record_name_binding")?;
        check(integer(&m["policy_revision"], 0)? <=
                    integer(&p["revision"], 0)?, "future_policy_revision")?;
        Self::policy_check(&p, text(&m["source"])?, text(&m["path"])?,
                Some(json!([m["device"],m["inode"]])))?;
        let t = token(text(&m["source"])?, text(&m["path"])?);
        let head = &s["heads"][&t];
        check(m["sequence"].as_u64() ==
                        Some(if head.is_null() {
                                1
                            } else {
                                integer(&head["sequence"],
                                                    1)?.checked_add(1).ok_or(Error("invalid_integer"))?
                            }) && m["previous"] == head["generation"],
                "replay_or_out_of_order")?;
        check(s["records"].get(g).is_none(), "generation_reuse")?;
        let dest = self.root.join("records").join(format!("{g}.rec"));
        if dest.exists() {
            check(io::bytes(&dest, LIMIT)?.0 == raw, "orphan_conflict")?;
        } else { io::atomic(&dest, &raw, 0o600, false, false)?; }
        hook("copied")?;
        s["records"][g] = json!({"meta":m,"deleted":false});
        s["heads"][t] = json!({"sequence":m["sequence"],"generation":g});
        self.save(&s)?;
        hook("committed")?;
        let result = self.materialize_locked(&s, g, &p)?;
        io::remove(&self.inbox.join(name))?;
        Ok(result)
    }
    pub fn delete(&self, source: &str, path: &str) -> Result<Value> {
        self.require_role("collector")?;
        self.admitted(source, path)?;
        let (_p, mut p) = self.held(true)?;
        let (_s, mut s) = self.state(&p)?;
        let pair = json!([source,path]);
        if !arr(&p["paths"])?.contains(&pair) {
            p["paths"].as_array_mut().unwrap().push(pair);
        }
        for r in s["records"].as_object().unwrap().values() {
            let m = &r["meta"];
            let id = json!([m["device"],m["inode"]]);
            if m["source"] == source && m["path"] == path &&
                    !arr(&p["inodes"])?.contains(&id) {
                p["inodes"].as_array_mut().unwrap().push(id);
            }
        }
        for r in s["records"].as_object().unwrap().values() {
            let m = &r["meta"];
            let pair = json!([m["source"],m["path"]]);
            if arr(&p["inodes"])?.contains(&json!([m["device"],m["inode"]]))
                    && !arr(&p["paths"])?.contains(&pair) {
                p["paths"].as_array_mut().unwrap().push(pair);
            }
        }
        p["revision"] =
            json!(integer(&p["revision"],0)?.checked_add(1).ok_or(Error("invalid_integer"))?);
        io::atomic(&self.policy.join("deleted.json"), &canonical(&p), 0o640,
                true, false)?;
        self.purge_locked(&mut s, &p)?;
        for item in fs::read_dir(&self.inbox)? {
            let path = item?.path();
            if path.extension().is_some_and(|s| s == "rec") {
                if let Ok((raw, m)) = io::bytes(&path, LIMIT) {
                    if self.cfg["producer_uid"] == m.uid() {
                        if let Ok((m, _)) = unseal(&raw, &self.key, &self.cfg) {
                            if Self::policy_check(&p, text(&m["source"])?,
                                        text(&m["path"])?,
                                        Some(json!([m["device"],m["inode"]]))).is_err() {
                                io::remove(&path)?;
                            }
                        }
                    }
                }
            }
        }
        io::sync(&self.inbox)?;
        Ok(json!({"status":"deleted","policy_revision":p["revision"],"producer_purge":"required","downstream_deletion":"required"}))
    }
    pub fn registry(&self) -> Value {
        json!({"version":1,"sources":self.cfg["sources"].as_object().unwrap().iter().map(|(k,v)|(k.clone(),json!({"kind":v["kind"],"root":self.root.join("validated").join(k)}))).collect::<uasf_json::Map<_,_>>()})
    }
    pub fn publish(&self, g: &str, staging: &Path, unmanaged: bool)
        -> Result<Value> {
        self.require_role("collector")?;
        generation(g)?;
        uasf_staging::policy::binding(staging, None, !unmanaged)?;
        let store =
            uasf_staging::Store::new(staging.to_owned(), self.registry())?;
        store.with_capture(|lease|
                {
                    let (_p, p) = self.held(false)?;
                    let (_s, mut s) = self.state(&p)?;
                    self.purge_locked(&mut s, &p)?;
                    let accepted = self.materialize_locked(&s, g, &p)?;
                    let m = &accepted["receipt"];
                    let source = text(&m["source"])?;
                    let path = Path::new(text(&accepted["path"])?);
                    let source_root = self.root.join("validated").join(source);
                    let rel =
                        path.strip_prefix(source_root).map_err(|_|
                                                Error("staging_registry_mismatch"))?.to_str().ok_or(Error("invalid_relative_path"))?;
                    let result =
                        lease.capture(source, rel, integer(&m["length"], 0)?,
                                text(&m["sha256"])?)?;
                    if result["status"] != "published" &&
                            result["status"] != "unchanged" {
                        return Ok(json!({"status":result["status"],"staging":result}));
                    }
                    let r = &result["receipt"];
                    check(r["length"] == m["length"] &&
                                r["sha256"] == m["sha256"], "staging_bytes_mismatch")?;
                    let link =
                        json!({"feeder_generation":g,"staging_generation":r["generation"],"staging_root":staging,"staging_relative":r["staging_relative"],"original_provenance":m,"collector_snapshot":path,"sha256":m["sha256"]});
                    s["records"][g]["staging_link"] = link.clone();
                    self.save(&s)?;
                    Ok(json!({"status":"published","link":link,"staging":result}))
                })
    }
}
