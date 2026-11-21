//! Nonblocking per-store admission. Independent processes share two slots;
//! descriptor locks are released on crash and are never inherited by children.
use std::{fs::{self, File, OpenOptions}, os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt}, path::Path};
pub const SLOTS: usize = 2;
pub const QUERY_ADDRESS_SPACE: u64 = 1536 * 1024 * 1024;
pub struct Lease { _file: File }
#[derive(Debug, PartialEq)]
pub enum Refusal { Capacity, Unavailable }
pub fn acquire(root: &Path) -> Result<Lease, Refusal> {
    acquire_slots(root, SLOTS)
}
pub fn acquire_gate(root: &Path) -> Result<Lease, Refusal> { acquire_slots(root, 2) }
fn acquire_slots(root: &Path, slots: usize) -> Result<Lease, Refusal> {
    fn attempt(root: &Path, slots: usize) -> std::io::Result<Option<Lease>> {
        let dir = root.join(".uasf-query-slots");
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}, Err(e) => return Err(e),
        }
        let meta = fs::symlink_metadata(&dir)?;
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o777 != 0o700 {
            return Err(std::io::Error::other("untrusted admission directory"));
        }
        // Hold the validated directory descriptor while opening child slots.
        let directory = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC).open(&dir)?;
        use std::os::fd::{AsRawFd, FromRawFd};
        let opened = directory.metadata()?;
        if opened.dev() != meta.dev() || opened.ino() != meta.ino() { return Err(std::io::Error::other("admission directory changed")); }
        for n in 0..slots {
            let name = std::ffi::CString::new(n.to_string()).unwrap();
            let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600) };
            if fd < 0 { return Err(std::io::Error::last_os_error()); }
            let file = unsafe { File::from_raw_fd(fd) }; let meta = file.metadata()?;
            if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o777 != 0o600 || meta.nlink() != 1 { return Err(std::io::Error::other("untrusted admission slot")); }
            if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 { return Ok(Some(Lease { _file: file })); }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EWOULDBLOCK) { return Err(error); }
        }
        Ok(None)
    }
    attempt(root, slots).map_err(|_| Refusal::Unavailable)?.ok_or(Refusal::Capacity)
}
/// Apply before query allocations in the dedicated CLI process. A stricter
/// caller limit is preserved. Library users control their own process limits.
pub fn limit_address_space() -> std::io::Result<()> {
    limit_address_space_to(QUERY_ADDRESS_SPACE)
}
pub fn limit_gate_address_space() -> std::io::Result<()> { limit_address_space_to(768 * 1024 * 1024) }
fn limit_address_space_to(maximum: u64) -> std::io::Result<()> {
    let mut old = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    if unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut old) } != 0 { return Err(std::io::Error::last_os_error()); }
    let limit = libc::rlimit { rlim_cur: old.rlim_cur.min(maximum), rlim_max: old.rlim_max };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) } != 0 { return Err(std::io::Error::last_os_error()); }
    Ok(())
}
