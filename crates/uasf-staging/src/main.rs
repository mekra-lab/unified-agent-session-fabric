use std::{path::PathBuf, collections::HashMap};
use uasf_staging::{Store, Error, files, policy};
fn main() {
    let result = run();
    let (value, good) =
        match result {
            Ok(v) => v,
            Err(e) => (uasf_staging::response(&e), false),
        };
    println!("{}",uasf_json::to_string_pretty(&value).unwrap());
    if !good { std::process::exit(2) }
}
fn run() -> uasf_staging::Result<(uasf_json::Value, bool)> {
    let mut options = HashMap::new();
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut unmanaged = false;
    while let Some(a) = args.next() {
        if a == "--help" {
            println!("uasf-staging --root PATH --registry PATH [--deletion-root PATH] [--collector-uid UID] [--unmanaged-test-mode] capture|scan|delete|inspect SOURCE [RELATIVE]");
            std::process::exit(0)
        } else if a == "--unmanaged-test-mode" {
            unmanaged = true
        } else if a.starts_with("--") {
            if !["--root", "--registry", "--deletion-root",
                                "--collector-uid"].contains(&a.as_str()) {
                return Err(Error::Value)
            }
            options.insert(a, args.next().ok_or(Error::Value)?);
        } else { positional.push(a) }
    }
    let get = |key: &str| options.get(key).ok_or(Error::Value);
    let root = PathBuf::from(get("--root")?);
    files::absolute(&root)?;
    let uid = unsafe { libc::geteuid() };
    if options.get("--collector-uid").is_some_and(|s|
                s.parse::<u32>().ok() != Some(uid)) {
        return Err(Error::Denied("wrong_collector_uid"))
    }
    let expected = options.get("--deletion-root").map(PathBuf::from);
    policy::binding(&root, expected.as_deref(), !unmanaged)?;
    let store =
        Store::new(root, files::json(&PathBuf::from(get("--registry")?))?)?;
    let action = positional.first().ok_or(Error::Value)?;
    let source = positional.get(1).map(String::as_str).unwrap_or("");
    let rel = positional.get(2).map(String::as_str).unwrap_or("");
    let value =
        match action.as_str() {
            "capture" => store.capture(source, rel)?,
            "delete" => store.delete(source, rel)?,
            "scan" => store.scan(source)?,
            "inspect" => return Ok((store.state()?, true)),
            _ => return Err(Error::Value),
        };
    let good =
        |v: &uasf_json::Value|
            v["status"] == "published" || v["status"] == "unchanged" ||
                action == "delete" && v["status"] == "deleted";
    let ok =
        if let Some(a) = value.as_array() {
            a.iter().all(good)
        } else { good(&value) };
    Ok((value, ok))
}
