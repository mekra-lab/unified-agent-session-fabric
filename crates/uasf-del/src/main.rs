use std::{io::Read, fs};
use uasf_json::json;
fn run() -> uasf_del::Result<uasf_json::Value> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 || args[0] != "--config" {
        return Err(uasf_del::Error("ValueError"));
    }
    let cfg = uasf_json::from_slice(&fs::read(&args[1])?)?;
    let mut input = String::new();
    std::io::stdin().take(1024 * 1024 + 1).read_to_string(&mut input)?;
    if input.len() > 1024 * 1024 {
        return Err(uasf_del::Error("ValueError"));
    }
    let request = uasf_json::from_str(&input)?;
    let q = uasf_del::process::config(&cfg, request["op"] == "init")?;
    let mut commands = uasf_del::process::commands(&cfg)?;
    uasf_del::process::execute(&q, &request, &mut commands, &mut |_| Ok(()))
}
fn main() {
    let (v, code) =
        match run() {
            Ok(v) => {
                let code = if v["status"] == "pending" { 2 } else { 0 };
                (v, code)
            }
            Err(e) => (json!({"status":"unsupported","reason":e.0}), 2),
        };
    println!("{}",uasf_json::to_string(&v).unwrap());
    std::process::exit(code)
}
