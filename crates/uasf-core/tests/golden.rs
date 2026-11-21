//! Exact synthetic oracle bytes, replayed without an interpreter or daemon.
use std::{
    collections::{BTreeMap, BTreeSet}, fs, io::Write, path::{Path, PathBuf},
    process::{Command, Stdio},
};
use uasf_json::Value;
use uasf_core::{json::from_slice, model::{canonical, sha}};
fn decode_pack(manifest: &Value, pack: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    if manifest["schema"] != "uasf-core.golden/2" || manifest["pack"]["path"] != "objects.jsonl"
        || manifest["pack"]["length"].as_u64() != Some(pack.len() as u64)
        || manifest["pack"]["sha256"] != sha(pack) {
        return Err("pack hash, length or format mismatch".into());
    }
    let mut objects = BTreeMap::new();
    for line in pack.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let row = from_slice(line).map_err(|e| e.to_string())?;
        if row.as_object().map(|o| o.len()) != Some(3) {
            return Err("invalid object fields".into());
        }
        let data = if let Some(text) = row.get("utf8").and_then(Value::as_str) {
            text.as_bytes().to_vec()
        } else if let Some(bytes) = row.get("bytes").and_then(Value::as_array) {
            let data: Vec<u8> = bytes.iter().map(|b| b.as_u64()
                .and_then(|n| u8::try_from(n).ok()).ok_or("invalid byte value"))
                .collect::<Result<_, _>>()?;
            if std::str::from_utf8(&data).is_ok() {
                return Err("UTF-8 data must use a string".into());
            }
            data
        } else {
            return Err("invalid object encoding".into());
        };
        let digest = row["sha256"].as_str().ok_or("missing object address")?;
        if sha(&data) != digest || row["length"].as_u64() != Some(data.len() as u64) {
            return Err("object hash or length mismatch".into());
        }
        if objects.insert(digest.to_string(), data).is_some() {
            return Err("duplicate content address".into());
        }
    }
    let mut required = BTreeSet::new();
    for op in manifest["operations"].as_array().ok_or("missing operations")? {
        let fields: &[&str] = match op["action"].as_str() {
            Some("fixture") => &["sha256"],
            Some("cli") => &["request_sha256", "response_sha256"],
            Some("remove_index") => &[],
            _ => return Err("unknown golden action".into()),
        };
        for field in fields {
            required.insert(op[field].as_str().ok_or("missing content address")?.to_string());
        }
    }
    if objects.keys().cloned().collect::<BTreeSet<_>>() != required
        || manifest["pack"]["objects"].as_u64() != Some(objects.len() as u64) {
        return Err("object inventory mismatch".into());
    }
    Ok(objects)
}
fn bundle() -> (Value, Vec<u8>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    (from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap(),
        fs::read(root.join("objects.jsonl")).unwrap())
}
#[test]
fn rejects_corrupt_pack_and_objects() {
    let (mut manifest, mut pack) = bundle();
    // A changed wire spelling must fail even if the pack-level digest is updated.
    let position = pack.windows(4).position(|w| w == b":1e0").unwrap() + 1;
    pack[position] = b'2';
    assert!(decode_pack(&manifest, &pack).is_err());
    manifest["pack"]["sha256"] = Value::String(sha(&pack));
    assert!(decode_pack(&manifest, &pack).is_err());
    let (mut manifest, mut pack) = bundle();
    let row = pack.split(|b| *b == b'\n').next().unwrap().to_vec();
    pack.extend_from_slice(&row); pack.push(b'\n');
    manifest["pack"]["sha256"] = Value::String(sha(&pack));
    manifest["pack"]["length"] = (pack.len() as u64).into();
    assert!(decode_pack(&manifest, &pack).is_err());
}
#[test]
fn preserves_malformed_utf8_and_original_wire_spelling() {
    let (manifest, pack) = bundle();
    let objects = decode_pack(&manifest, &pack).unwrap();
    assert_eq!(objects.len(), 803);
    assert!(objects.values().any(|v| v == &[255, 10]));
    for spelling in [b"1e0".as_slice(), b"1.00".as_slice(), b"-0.0".as_slice()] {
        assert!(objects.values().any(|v| v.windows(spelling.len()).any(|w| w == spelling)));
    }
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
#[test]
fn frozen_synthetic_cli_workflows() {
    let (manifest, pack) = bundle();
    let objects = decode_pack(&manifest, &pack).unwrap();
    let mut scenarios = BTreeSet::new();
    let temp =
        std::env::temp_dir().join(format!("uasf-golden-{}",std::process::id()));
    fs::create_dir(&temp).unwrap();
    let _scratch = Scratch(temp.clone());
    let mut comparisons = 0;
    for op in manifest["operations"].as_array().unwrap() {
        let case = op["case"].as_str().unwrap();
        assert!(!case.starts_with("e3-"));
        let db = temp.join(case).join("store");
        fs::create_dir_all(&db).unwrap();
        match op["action"].as_str().unwrap() {
            "fixture" => {
                let data = &objects[op["sha256"].as_str().unwrap()];
                assert_eq!(sha(&data),op["sha256"]);
                let path = temp.join(op["path"].as_str().unwrap());
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, data).unwrap();
            }
            "remove_index" =>
                fs::remove_file(db.join("index.sqlite")).unwrap(),
            "cli" => {
                let request = &objects[op["request_sha256"].as_str().unwrap()];
                let response = &objects[op["response_sha256"].as_str().unwrap()];
                assert_eq!(sha(&request),op["request_sha256"]);
                assert_eq!(sha(&response),op["response_sha256"]);
                let mut child =
                    Command::new(env!("CARGO_BIN_EXE_uasf-core")).arg("--index").arg(db.join("index.sqlite")).arg("--persistent").arg(db.join("persistent.sqlite")).arg(op["command"].as_str().unwrap()).current_dir(&temp).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
                child.stdin.take().unwrap().write_all(&request).unwrap();
                let output = child.wait_with_output().unwrap();
                assert_eq!(output.status.code().map(i64::from),op["exit_code"].as_i64(),"{case} {}",op["label"]);
                let actual: Value = from_slice(&output.stdout).unwrap();
                let expected = from_slice(&response).unwrap();
                assert_eq!(canonical(&actual),canonical(&expected),"{case} {}",op["label"]);
                if op["command"] == "query" && output.status.success() {
                    let core = uasf_core::Core::open(db.join("index.sqlite"), db.join("persistent.sqlite")).unwrap();
                    let request = from_slice(request).unwrap();
                    let expected_bytes = uasf_json::to_vec(&core.query(&request).unwrap()).unwrap();
                    assert_eq!(output.stdout.strip_suffix(b"\n").unwrap(), expected_bytes, "stream bytes: {case} {}", op["label"]);
                }
                comparisons += 1;
                scenarios.insert(case);
            }
            _ => panic!("unknown golden action"),
        }
    }
    assert_eq!(comparisons,863);
    assert_eq!(scenarios.len(),34);
    assert_eq!(manifest["counts"]["comparisons"], comparisons);
    assert_eq!(manifest["counts"]["scenarios"], scenarios.len());
    println!("golden: {comparisons} comparisons, {} scenarios, {} unique objects", scenarios.len(), objects.len());
}
