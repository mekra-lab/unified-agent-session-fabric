//! Collector-owned files, descriptor-relative snapshots and shared writer locks.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt, fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Component, Path},
    ffi::CString,
};
use uasf_json::Value;
use crate::{Error, Result, LIMIT};

pub fn uid() -> u32 { unsafe { libc::getuid() } }
pub fn absolute(p: &Path) -> Result<()> {
    if !p.is_absolute() || p.components().any(|c| c == Component::ParentDir) {
        return Err(Error);
    }
    Ok(())
}
pub fn relative(p: &Path) -> Result<()> {
    if p.is_absolute() || p.as_os_str().is_empty() ||
            p.components().any(|c| c == Component::ParentDir) {
        return Err(Error);
    }
    Ok(())
}
pub fn trusted_parents(p: &Path) -> Result<()> {
    let platform = fs::metadata("/")?.uid();
    for parent in p.ancestors().skip(1) {
        let m = fs::symlink_metadata(parent)?;
        let sticky = m.uid() == platform && m.mode() & libc::S_ISVTX != 0;
        if m.file_type().is_symlink() || ![platform, uid()].contains(&m.uid())
                || (m.mode() & 0o022 != 0 && !sticky) {
            return Err(Error);
        }
    }
    Ok(())
}
pub fn private(p: &Path, directory: bool) -> Result<()> {
    absolute(p)?;
    trusted_parents(p)?;
    let m = fs::symlink_metadata(p)?;
    if (if directory { !m.is_dir() } else { !m.is_file() }) ||
                m.uid() != uid() || m.mode() & 0o077 != 0 {
        return Err(Error);
    }
    Ok(())
}
fn open_at(parent: &File, part: &std::ffi::OsStr, directory: bool)
    -> Result<File> {
    let name = CString::new(part.as_bytes()).map_err(|_| Error)?;
    let flags =
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW |
            if directory { libc::O_DIRECTORY } else { libc::O_NONBLOCK };
    let fd =
        unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 { return Err(Error); }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn directory(path: &Path) -> Result<File> {
    absolute(path)?;
    let mut fd = File::open("/")?;
    for component in path.components() {
        if let Component::Normal(part) = component {
            fd = open_at(&fd, part, true)?;
        }
    }
    Ok(fd)
}
fn facts(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec(), m.ctime(),
        m.ctime_nsec())
}
pub fn snapshot(root: &Path, rel: &Path) -> Result<Vec<u8>> {
    relative(rel)?;
    let mut dir = directory(root)?;
    let parts: Vec<_> =
        rel.components().filter_map(|c|
                    if let Component::Normal(s) = c {
                        Some(s)
                    } else { None }).collect();
    if parts.is_empty() { return Err(Error); }
    for part in &parts[..parts.len() - 1] {
        dir = open_at(&dir, part, true)?;
    }
    let mut file = open_at(&dir, parts.last().unwrap(), false)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() > LIMIT { return Err(Error); }
    let mut data = Vec::new();
    (&mut file).take(LIMIT + 1).read_to_end(&mut data)?;
    let middle = file.metadata()?;
    file.seek(SeekFrom::Start(0))?;
    // Read the complete second prefix, never a tail-only comparison.
    let mut second = Vec::new();
    (&mut file).take(LIMIT + 1).read_to_end(&mut second)?;
    let after = file.metadata()?;
    let named = open_at(&dir, parts.last().unwrap(), false)?.metadata()?;
    if facts(&before) != facts(&middle) || facts(&before) != facts(&after) ||
                    facts(&after) != facts(&named) ||
                data.len() as u64 != before.len() || second != data {
        return Err(Error);
    }
    Ok(data)
}
pub fn json(path: &Path) -> Result<Value> {
    Ok(uasf_json::from_slice(&fs::read(path)?)?)
}
pub struct Lock(File);
impl Lock {
    pub fn acquire(path: &Path, wait: bool) -> Result<Self> {
        let f =
            OpenOptions::new().read(true).write(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW
                            | libc::O_CLOEXEC).open(path)?;
        if !f.metadata()?.is_file() { return Err(Error); }
        let flags = libc::LOCK_EX | if wait { 0 } else { libc::LOCK_NB };
        if unsafe { libc::flock(f.as_raw_fd(), flags) } != 0 {
            return Err(Error);
        }
        Ok(Self(f))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN); }
    }
}
pub fn atomic_json(root: &Path, state: &Value) -> Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path =
        root.join("tmp").join(format!("state-rust-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
    let mut f =
        OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    let result =
        (||
                    {
                        f.write_all(uasf_json::to_string_pretty(state)?.as_bytes())?;
                        f.write_all(b"\n")?;
                        f.sync_all()?;
                        fs::rename(&path, root.join("state.json"))?;
                        File::open(root)?.sync_all()?;
                        Ok(())
                    })();
    if result.is_err() { let _ = fs::remove_file(path); }
    result
}
pub fn mkdir(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) =>
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Error),
    }
    private(path, true)
}
