//! Descriptor-relative capture: no symlink traversal, two complete bounded reads.
use crate::{Result, Error, LIMIT};
use uasf_json::{json, Value};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{Read, Write, Seek, SeekFrom},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::{MetadataExt, OpenOptionsExt}},
    },
    path::{Path, Component},
};
pub fn absolute(path: &Path) -> Result<()> {
    if !path.is_absolute() ||
            path.components().any(|c| c == Component::ParentDir) {
        Err(Error::Value)
    } else { Ok(()) }
}
pub fn relative(path: &Path) -> Result<()> {
    if path.is_absolute() ||
                !path.components().any(|c| matches!(c,Component::Normal(_)))
            || path.components().any(|c| c == Component::ParentDir) {
        Err(Error::Value)
    } else { Ok(()) }
}
fn name(s: &OsStr) -> Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| Error::Value)
}
pub fn open_at(parent: &File, part: &OsStr, dir: bool) -> Result<File> {
    let n = name(part)?;
    let flags =
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW |
            if dir { libc::O_DIRECTORY } else { libc::O_NONBLOCK };
    let fd = unsafe { libc::openat(parent.as_raw_fd(), n.as_ptr(), flags) };
    if fd < 0 { return Err(std::io::Error::last_os_error().into()) }
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub fn directory(path: &Path, create: bool) -> Result<File> {
    absolute(path)?;
    let mut fd = File::open("/")?;
    for part in path.components() {
        if let Component::Normal(p) = part {
            if create {
                let n = name(p)?;
                if unsafe { libc::mkdirat(fd.as_raw_fd(), n.as_ptr(), 0o700) }
                        < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(e.into())
                    }
                } else { fd.sync_all()?; }
            }
            fd = open_at(&fd, p, true)?;
        }
    }
    Ok(fd)
}
pub fn facts(m: &fs::Metadata) -> Value {
    json!([m.dev(),m.ino(),m.len(),m.mtime() as
        i128*1_000_000_000+m.mtime_nsec() as i128,m.ctime() as
        i128*1_000_000_000+m.ctime_nsec() as i128])
}
pub struct Capture {
    pub data: Vec<u8>,
    pub metadata: fs::Metadata,
    pub at: String,
}
pub fn snapshot(root: &Path, rel: &Path,
    hook: &mut dyn FnMut(&str) -> Result<()>)
    -> std::result::Result<Capture, Value> {
    let mut opened = false;
    let read =
        (|| -> Result<std::result::Result<Capture, Value>>
                    {
                        relative(rel)?;
                        let mut dir = directory(root, false)?;
                        let parts: Vec<_> =
                            rel.components().filter_map(|c|
                                        if let Component::Normal(p) = c {
                                            Some(p)
                                        } else { None }).collect();
                        for part in &parts[..parts.len() - 1] {
                            dir = open_at(&dir, part, true)?;
                        }
                        let part = parts.last().unwrap();
                        let mut source = open_at(&dir, part, false)?;
                        opened = true;
                        let before = source.metadata()?;
                        if !before.is_file() {
                            return Ok(Err(json!({"status":"rejected","reason":"not_regular"})))
                        }
                        if before.len() > LIMIT {
                            return Ok(Err(json!({"status":"unsupported","reason":"over_32_MiB"})))
                        }
                        let mut data = Vec::new();
                        let mut block = vec![0;256*1024];
                        loop {
                            let remaining =
                                (LIMIT + 1 - data.len() as u64).min(block.len() as u64) as
                                    usize;
                            let n = source.read(&mut block[..remaining])?;
                            if n == 0 { break }
                            data.extend_from_slice(&block[..n]);
                            hook("copy_chunk")?;
                            if data.len() as u64 > LIMIT {
                                return Ok(Err(json!({"status":"unstable","reason":"grew_over_limit"})))
                            }
                        }
                        let middle = source.metadata()?;
                        source.seek(SeekFrom::Start(0))?;
                        let mut second = Vec::new();
                        (&mut source).take(LIMIT + 1).read_to_end(&mut second)?;
                        let after = source.metadata()?;
                        let n = name(part)?;
                        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
                        if unsafe {
                                    libc::fstatat(dir.as_raw_fd(), n.as_ptr(), st.as_mut_ptr(),
                                        libc::AT_SYMLINK_NOFOLLOW)
                                } < 0 {
                            return Err(std::io::Error::last_os_error().into())
                        }
                        let st = unsafe { st.assume_init() };
                        let named =
                            json!([st.st_dev,st.st_ino,st.st_size,st.st_mtime as
                                i128*1_000_000_000+st.st_mtime_nsec as i128,st.st_ctime as
                                i128*1_000_000_000+st.st_ctime_nsec as i128]);
                        if second.len() as u64 > LIMIT {
                            return Ok(Err(json!({"status":"unstable","reason":"grew_over_limit"})))
                        }
                        if facts(&before) != facts(&middle) ||
                                            facts(&before) != facts(&after) || facts(&after) != named ||
                                    data.len() as u64 != before.len() || data != second {
                            return Ok(Err(json!({"status":"unstable","reason":"stat_or_full_hash_changed"})))
                        }
                        Ok(Ok(Capture { data, metadata: before, at: crate::now() }))
                    })();
    match read {
        Ok(v) => v,
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound =>
            Err(json!({"status":if opened
                        {"unstable"}else{"not_found"},"reason":"path_missing"})),
        Err(Error::Io(e)) =>
            Err(json!({"status":"rejected","reason":"unsafe_or_unreadable_path","errno":e.raw_os_error()})),
        Err(_) => Err(json!({"status":"unsupported","reason":"ValueError"})),
    }
}
pub struct Lock(File);
impl Lock {
    pub fn acquire(path: &Path, wait: bool) -> Result<Self> {
        let f =
            OpenOptions::new().read(true).write(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW
                            | libc::O_CLOEXEC).open(path)?;
        if !f.metadata()?.is_file() { return Err(Error::Value) }
        if unsafe {
                    libc::flock(f.as_raw_fd(),
                        libc::LOCK_EX | if wait { 0 } else { libc::LOCK_NB })
                } < 0 {
            return Err(std::io::Error::last_os_error().into())
        }
        Ok(Self(f))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN); }
    }
}
pub fn json(path: &Path) -> Result<Value> {
    Ok(uasf_json::from_slice(&fs::read(path)?)?)
}
pub fn save(root: &Path, state: &Value) -> Result<()> {
    let path = root.join("tmp").join(format!("state-{}",crate::uuid()?));
    let mut f =
        OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    f.write_all(uasf_json::to_string_pretty(state)?.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    fs::rename(path, root.join("state.json"))?;
    directory(root, false)?.sync_all()?;
    Ok(())
}
