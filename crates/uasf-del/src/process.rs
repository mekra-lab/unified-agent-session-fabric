//! Explicit subprocess interfaces. No shell, archive crate, or implicit daemon.
use crate::{Coordinator, Error, Executor, Lease, Result};
use uasf_json::{json, Value};
use std::{io::Write, path::PathBuf, process::{Command, Stdio}};

pub struct Commands {
    pub completion: Option<Vec<String>>,
    pub intake: Vec<String>,
    pub archive: Vec<String>,
    pub timeout: std::time::Duration,
}
fn command(words: &[String]) -> Result<Command> {
    let (exe, args) = words.split_first().ok_or(Error("ValueError"))?;
    if !PathBuf::from(exe).is_absolute() { return Err(Error("ValueError")); }
    let mut cmd = Command::new(exe);
    cmd.args(args);
    Ok(cmd)
}
/// Bound the complete pipe exchange, including a child that never reads stdin.
/// Each child has a private process group. Drop kills that group and reaps the
/// direct child on every success/error path, including inherited pipe holders.
struct Running(std::process::Child);
impl Drop for Running {
    fn drop(&mut self) {
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL); }
        let _ = self.0.wait();
    }
}
fn nonblocking(fd: i32) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(Error("OSError"));
    }
    Ok(())
}
fn exchange(mut cmd: Command, input: &[u8], fd: Option<i32>, timeout: std::time::Duration) -> Result<Vec<u8>> {
    use std::{io::Read, os::{fd::AsRawFd, unix::process::CommandExt}, time::{Duration, Instant}};
    if timeout.is_zero() || timeout > Duration::from_secs(3600) { return Err(Error("ValueError")); }
    let deadline = Instant::now() + timeout;
    let parent = unsafe { libc::getpid() };
    unsafe {
        cmd.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent { libc::_exit(125); }
            // All other descriptors remain CLOEXEC. Only the intake capability
            // crosses this exec; intake restores CLOEXEC before processing input.
            if let Some(fd) = fd {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let mut running = Running(cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?);
    let mut stdin = running.0.stdin.take();
    let mut stdout = running.0.stdout.take().ok_or(Error("RuntimeError"))?;
    nonblocking(stdin.as_ref().ok_or(Error("RuntimeError"))?.as_raw_fd())?;
    nonblocking(stdout.as_raw_fd())?;
    let mut sent = 0;
    let mut output = Vec::new();
    let mut eof = false;
    loop {
        if Instant::now() >= deadline { return Err(Error("TimeoutError")); }
        if let Some(pipe) = stdin.as_mut() {
            if sent == input.len() { stdin.take(); }
            else {
                match pipe.write(&input[sent..]) {
                    Ok(0) => return Err(Error("RuntimeError")),
                    Ok(n) => sent += n,
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {},
                    Err(e) => return Err(e.into()),
                }
            }
        }
        let mut buf = [0u8; 8192];
        match stdout.read(&mut buf) {
            Ok(0) => eof = true,
            Ok(n) => {
                output.extend_from_slice(&buf[..n]);
                if output.len() > 1024 * 1024 { return Err(Error("RuntimeError")); }
            },
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {},
            Err(e) => return Err(e.into()),
        }
        if let Some(status) = running.0.try_wait()? {
            if !status.success() { return Err(Error("RuntimeError")); }
            if eof && sent == input.len() { return Ok(output); }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
impl Executor for Commands {
    fn complete_integration(&mut self, job:&Value)->Result<()> {
        let words=self.completion.as_ref().ok_or(Error("integration_executor_required"))?;
        let out=exchange(command(words)?, &uasf_json::to_vec(&json!({"op":"complete_deletion","job":job}))?,None,self.timeout)?;
        if uasf_json::from_slice(&out)?!=json!({"status":"complete"}) {return Err(Error("integration_pending"));}
        Ok(())
    }

    fn erase(&mut self, receipts: &Value, lease: &Lease) -> Result<()> {
        let fd = lease.fd();
        let words: Vec<_> = self.intake.iter().map(|s| s.replace("{lock_fd}", &fd.to_string())).collect();
        let bytes = uasf_json::to_vec(&json!({"op":"erase","receipt_ids":receipts}))?;
        let out = exchange(command(&words)?, &bytes, Some(fd), self.timeout)?;
        let v = uasf_json::from_slice(&out).map_err(|_| Error("RuntimeError"))?;
        if v != json!({"status":"deleted"}) { return Err(Error("RuntimeError")); }
        Ok(())
    }
    fn excise(&mut self, scope: &str) -> Result<()> {
        let words: Vec<_> = self.archive.iter().map(|s| s.replace("{scope}", scope)).collect();
        if !self.archive.iter().any(|s| s == "{scope}") { return Err(Error("ValueError")); }
        let out = exchange(command(&words)?, &[], None, self.timeout)?;
        let v = uasf_json::from_slice(&out).map_err(|_| Error("RuntimeError"))?;
        if !v.is_object() || v["dry_run"] != false || v["queued"] == true ||
                v["status"] == "queued" || v.get("error").is_some_and(|v| !v.is_null() && v != false) {
            return Err(Error("RuntimeError"));
        }
        Ok(())
    }
}
pub fn config(value: &Value, initialize: bool) -> Result<Coordinator> {
    let path =
        |k: &str|
            value[k].as_str().map(PathBuf::from).ok_or(Error("ValueError"));
    Coordinator::integrated(path("root")?,path("staging")?,value["registry"].clone(),path("intake")?,value.get("integration").cloned().unwrap_or(Value::Null),initialize)
}
pub fn commands(value: &Value) -> Result<Commands> {
    let words =
        |k: &str| -> Result<Vec<String>>
            {
                value[k].as_array().ok_or(Error("ValueError"))?.iter().map(|v|
                            v.as_str().map(String::from).ok_or(Error("ValueError"))).collect()
            };
    let milliseconds = match value.get("command_timeout_ms") {
        None => 90000,
        Some(v) => v.as_u64().filter(|n| (1..=3600000).contains(n)).ok_or(Error("ValueError"))?,
    };
    Ok(Commands {
            completion: value.get("integration").map(|v| v["completion_command"].as_array().ok_or(Error("ValueError")).and_then(|a|a.iter().map(|s|s.as_str().map(str::to_owned).ok_or(Error("ValueError"))).collect())).transpose()?,
            timeout: std::time::Duration::from_millis(milliseconds),
            intake: words("intake_command")?,
            archive: words("archive_command")?,
        })
}
pub fn execute(q: &Coordinator, request: &Value, executor: &mut dyn Executor,
    hook: &mut dyn FnMut(&str) -> Result<()>) -> Result<Value> {
    let text = |k: &str| request[k].as_str().ok_or(Error("ValueError"));
    match text("op")? {
        "init" => Ok(json!({"status":"ok"})),
        "state" => q.state(),
        "request" =>
            q.request(&request["selector"], &request["input_map"],
                request["request_id"].as_str(), hook),
        "resume" => q.resume(executor, hook),
        "link_receipt" =>
            q.link_receipt(text("receipt_id")?, text("source")?,
                text("relative")?),
        "read_cache" => q.cache(text("key")?, None),
        "put_cache" => {
            let hex = text("hex")?;
            if hex.len() % 2 != 0 { return Err(Error("ValueError")); }
            let data =
                (0..hex.len()).step_by(2).map(|i|
                                hex.get(i..i +
                                                    2).ok_or(Error("ValueError")).and_then(|v|
                                        u8::from_str_radix(v,
                                                16).map_err(|_|
                                                Error("ValueError")))).collect::<Result<Vec<_>>>()?;
            q.cache(text("key")?, Some(&data))
        }
        _ => Err(Error("ValueError")),
    }
}
