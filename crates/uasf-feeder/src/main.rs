use std::{collections::BTreeMap, path::Path};
use uasf_feeder::{
    check, initialize, load_config, load_key, Endpoint, Error, Result,
};
use uasf_json::{json, Value};
fn run() -> Result<Value> {
    let mut flags = BTreeMap::new();
    let mut words = Vec::new();
    let mut args = std::env::args().skip(1);
    let mut unmanaged = false;
    while let Some(a) = args.next() {
        if a == "--unmanaged-synthetic" {
            unmanaged = true;
        } else if a.starts_with("--") {
            check(["--config", "--key", "--state", "--policy", "--inbox",
                                "--role", "--staging"].contains(&a.as_str()),
                    "invalid_arguments")?;
            check(flags.insert(a,
                            args.next().ok_or(Error("invalid_arguments"))?).is_none(),
                    "invalid_arguments")?;
        } else { words.push(a); }
    }
    let f =
        |k: &str|
            flags.get(k).map(String::as_str).ok_or(Error("invalid_arguments"));
    let w =
        |n: usize|
            words.get(n).map(String::as_str).ok_or(Error("invalid_arguments"));
    let op = w(0)?;
    let count =
        match op {
            "init" | "purge" => 1,
            "emit" | "delete" => 3,
            "retry" | "materialize" | "accept" | "publish" => 2,
            _ => return Err(Error("invalid_arguments")),
        };
    check(words.len() == count, "invalid_arguments")?;
    let config = Path::new(f("--config")?);
    let key = Path::new(f("--key")?);
    let state = Path::new(f("--state")?);
    let role = f("--role")?;
    if op == "init" {
        return initialize(state, &load_config(config)?, &load_key(key)?,
                role);
    }
    let e =
        Endpoint::new(config, key, state, Path::new(f("--policy")?),
                Path::new(f("--inbox")?), role)?;
    match op {
        "emit" => e.emit(w(1)?, w(2)?, &mut |_| Ok(())),
        "delete" => e.delete(w(1)?, w(2)?),
        "accept" => e.accept(w(1)?, &mut |_| Ok(())),
        "retry" => e.retry(w(1)?),
        "materialize" => e.materialize(w(1)?),
        "purge" => e.purge(),
        "publish" => e.publish(w(1)?, Path::new(f("--staging")?), unmanaged),
        _ => Err(Error("invalid_arguments")),
    }
}
fn main() {
    let (v, code) =
        match run() {
            Ok(v) => {
                let code = if v["status"] == "not_captured" { 2 } else { 0 };
                (v, code)
            }
            Err(e) => (json!({"status":"rejected","reason":e.0}), 2),
        };
    println!("{}",v);
    std::process::exit(code);
}
