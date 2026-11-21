use uasf_json::Value;
use std::{
    collections::BTreeMap, fs, io::{self, Read},
    path::PathBuf,
};
use uasf_intake::{Intake, Error, Result, policy, status};
fn run() -> Result<(Value, bool)> {
    let mut args = std::env::args().skip(1);
    let mut options = BTreeMap::new();
    let mut unmanaged = false;
    while let Some(key) = args.next() {
        if key == "--unmanaged-test-mode" { unmanaged = true; continue }
        if !["--database", "--registry", "--principal", "--deletion-root",
                            "--collector-uid",
                            "--coordinator-lock-fd"].contains(&key.as_str()) {
            return Err(Error::Invalid)
        }
        if options.insert(key, args.next().ok_or(Error::Invalid)?).is_some() {
            return Err(Error::Invalid)
        }
    }
    let database =
        PathBuf::from(options.get("--database").ok_or(Error::Invalid)?);
    let registry =
        PathBuf::from(options.get("--registry").ok_or(Error::Invalid)?);
    let principal = options.get("--principal").ok_or(Error::Invalid)?;
    let uid = unsafe { libc::geteuid() };
    let collector =
        options.get("--collector-uid").map(|s|
                            s.parse::<u32>().map_err(|_|
                                    Error::Invalid)).transpose()?.unwrap_or(uid);
    if collector != uid { return Err(Error::Denied("wrong_collector_uid")) }
    let expected = options.get("--deletion-root").map(PathBuf::from);
    policy::binding(&database, expected.as_deref(), !unmanaged)?;
    let fd =
        options.get("--coordinator-lock-fd").map(|s|
                        s.parse::<i32>().map_err(|_| Error::Invalid)).transpose()?;
    // Before opening SQLite, enforce either normal admission or a proven coordinator lease.
    let normal =
        if fd.is_none() { Some(policy::access(&database)?) } else { None };
    let coordinator =
        if fd.is_some() { policy::coordinator(&database, fd)? } else { None };
    let grants =
        uasf_json::from_slice(&fs::read(registry)?).map_err(|_|
                    Error::Denied("deletion_state_unavailable"))?;
    let mut store =
        Intake::open(&database,
                    grants).map_err(|_|
                    Error::Denied("deletion_state_unavailable"))?;
    drop(normal); // execute rechecks binding and acquires its own full operation lease.
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let request: Value = uasf_json::from_str(&input)?;
    let erase = request.get("op").and_then(Value::as_str) == Some("erase");
    let response =
        if erase {
            let obj = request.as_object().ok_or(Error::Invalid)?;
            if obj.keys().any(|k| k != "op" && k != "receipt_ids") {
                return Err(Error::Invalid)
            }
            let ids: Vec<String> =
                uasf_json::from_value(request.get("receipt_ids").cloned().ok_or(Error::Invalid)?)?;
            store.erase(&ids, fd)?
        } else if coordinator.is_some() {
            status("coordinator_required")
        } else { store.execute(&request, principal)? };
    let success =
        response["status"] == "accepted" || response["status"] == "ok" ||
            (erase && response["status"] == "deleted");
    Ok((response, success))
}
fn main() {
    let (response, success) =
        match run() { Ok(v) => v, Err(e) => (e.response(), false), };
    println!("{}",response);
    if !success { std::process::exit(2) }
}
