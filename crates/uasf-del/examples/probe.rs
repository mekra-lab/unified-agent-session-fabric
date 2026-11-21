//! Synthetic test driver: hook rendezvous and archive effects are explicit IPC.
//! Production callers use uasf-del, which exposes no crash or callback controls.
use std::io::{self, BufRead, Write};
use uasf_json::{json, Value};
use uasf_del::{Error, Executor, Lease, Result};
fn line() -> Result<Value> {
    let mut s = String::new();
    io::stdin().lock().read_line(&mut s)?;
    Ok(uasf_json::from_str(&s)?)
}
fn send(v: Value) -> Result<()> {
    println!("{}",v);
    io::stdout().flush()?;
    Ok(())
}
fn rendezvous(v: Value) -> Result<()> {
    send(v)?;
    let r = line()?;
    match r["error"].as_str() {
        Some("ConnectionError") => Err(Error("ConnectionError")),
        Some(_) => Err(Error("RuntimeError")),
        None => Ok(()),
    }
}
struct Remote(uasf_del::process::Commands, bool);
impl Executor for Remote {
    fn erase(&mut self, ids: &Value, lease: &Lease) -> Result<()> {
        self.0.erase(ids, lease)
    }
    fn excise(&mut self, scope: &str) -> Result<()> {
        if self.1 {
            self.0.excise(scope)
        } else { rendezvous(json!({"event":"archive","scope":scope})) }
    }
}
fn run() -> Result<Value> {
    let input = line()?;
    let cfg = &input["config"];
    let req = &input["request"];
    let q = uasf_del::process::config(cfg, req["op"] == "init")?;
    let mut executor =
        Remote(uasf_del::process::commands(cfg)?,
            cfg["archive_command"][0] != "/bin/false");
    uasf_del::process::execute(&q, req, &mut executor,
        &mut |point| rendezvous(json!({"event":"hook","point":point})))
}
fn main() {
    unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL); }
    if unsafe { libc::getppid() } == 1 { std::process::exit(78); }
    let value =
        match run() {
            Ok(v) => json!({"result":v}),
            Err(e) => json!({"error":e.0}),
        };
    send(value).unwrap();
}
