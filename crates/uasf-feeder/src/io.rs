//! Descriptor-relative nofollow I/O; inbox publication needs search, not listing.
use crate::{check, Error, Result, uuid, LIMIT};
use std::{
    ffi::CString, fs::{self, File, Metadata},
    io::{Read, Write, Seek, SeekFrom},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
pub fn relative(s: &str) -> Result<()> {
    check(!s.is_empty() && s.len() <= 2048 && !s.starts_with('/') &&
                    !s.contains('\\') &&
                s.chars().all(|c| c >= ' ' && c != '\u{7f}'),
            "invalid_relative_path")?;
    check(s.split('/').all(|p| !p.is_empty() && p != "." && p != ".."),
        "noncanonical_path")
}
pub fn absolute(p: &Path) -> Result<()> {
    let s = p.to_str().ok_or(Error("invalid_relative_path"))?;
    check(s.starts_with('/') && s != "/", "absolute_nonroot_path_required")?;
    relative(&s[1..])
}
fn name(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error("invalid_relative_path"))
}
fn opened(fd: i32) -> Result<File> {
    if fd < 0 {
        Err(Error("OSError"))
    } else { Ok(unsafe { File::from_raw_fd(fd) }) }
}
pub fn directory(p: &Path, traverse: bool) -> Result<File> {
    absolute(p)?;
    let flags =
        libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC |
            if traverse { libc::O_PATH } else { libc::O_RDONLY };
    let mut f = opened(unsafe { libc::open(c"/".as_ptr(), flags) })?;
    for part in p.to_str().unwrap()[1..].split('/') {
        let n = name(part)?;
        f =
            opened(unsafe {
                        libc::openat(f.as_raw_fd(), n.as_ptr(), flags)
                    })?;
    }
    Ok(f)
}
pub fn file(p: &Path) -> Result<File> {
    absolute(p)?;
    let parent = directory(p.parent().unwrap(), true)?;
    let n =
        name(p.file_name().unwrap().to_str().ok_or(Error("invalid_relative_path"))?)?;
    opened(unsafe {
            libc::openat(parent.as_raw_fd(), n.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK |
                    libc::O_CLOEXEC)
        })
}
pub fn facts(m: &Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec(), m.ctime(),
        m.ctime_nsec())
}
pub fn read(p: &Path, limit: u64, owner: Option<u32>,
    admit: &mut dyn FnMut(&Metadata) -> Result<()>,
    hook: &mut dyn FnMut() -> Result<()>) -> Result<(Vec<u8>, Metadata)> {
    let mut f = file(p)?;
    let before = f.metadata()?;
    check(before.is_file(), "not_regular")?;
    check(owner.is_none_or(|u| u == before.uid()), "wrong_owner")?;
    check(before.len() <= limit, "over_size_limit")?;
    admit(&before)?;
    let mut first = Vec::new();
    (&mut f).take(limit + 1).read_to_end(&mut first)?;
    check(first.len() as u64 <= limit, "over_size_limit")?;
    let middle = f.metadata()?;
    hook()?;
    f.seek(SeekFrom::Start(0))?;
    let mut second = Vec::new();
    (&mut f).take(limit + 1).read_to_end(&mut second)?;
    let after = f.metadata()?;
    let named = file(p).map_err(|_| Error("unstable"))?.metadata()?;
    check(facts(&before) == facts(&middle) && facts(&before) == facts(&after)
                        && facts(&before) == facts(&named) &&
                    first.len() as u64 == before.len() && first == second,
            "unstable")?;
    Ok((first, before))
}
pub fn bytes(p: &Path, limit: u64) -> Result<(Vec<u8>, Metadata)> {
    read(p, limit, None, &mut |_| Ok(()), &mut || Ok(()))
}
pub fn sync(p: &Path) -> Result<()> {
    directory(p, false)?.sync_all()?;
    Ok(())
}
pub fn mkdirs(p: &Path) -> Result<()> {
    absolute(p)?;
    uasf_staging::files::directory(p, true)?;
    Ok(())
}
pub fn private(p: &Path) -> Result<PathBuf> {
    let m = directory(p, false)?.metadata()?;
    check(m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o7777 == 0o700,
            "private_directory_owner_or_mode")?;
    Ok(p.to_owned())
}
pub fn atomic(p: &Path, data: &[u8], mode: u32, replace: bool, dropbox: bool)
    -> Result<()> {
    absolute(p)?;
    let parent = directory(p.parent().unwrap(), dropbox)?;
    let temporary = name(&format!(".part-{}",uuid()?))?;
    let dest =
        name(p.file_name().unwrap().to_str().ok_or(Error("invalid_relative_path"))?)?;
    let mut f =
        opened(unsafe {
                    libc::openat(parent.as_raw_fd(), temporary.as_ptr(),
                        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL |
                                libc::O_NOFOLLOW | libc::O_CLOEXEC, mode)
                })?;
    let result =
        (||
                    {
                        f.set_permissions(fs::Permissions::from_mode(mode))?;
                        f.write_all(data)?;
                        f.sync_all()?;
                        let code =
                            unsafe {
                                if replace {
                                    libc::renameat(parent.as_raw_fd(), temporary.as_ptr(),
                                        parent.as_raw_fd(), dest.as_ptr())
                                } else {
                                    libc::linkat(parent.as_raw_fd(), temporary.as_ptr(),
                                        parent.as_raw_fd(), dest.as_ptr(), 0)
                                }
                            };
                        check(code == 0, "OSError")?;
                        if !replace {
                            check(unsafe {
                                            libc::unlinkat(parent.as_raw_fd(), temporary.as_ptr(), 0)
                                        } == 0, "OSError")?;
                        }
                        if !dropbox { parent.sync_all()?; }
                        Ok(())
                    })();
    unsafe { libc::unlinkat(parent.as_raw_fd(), temporary.as_ptr(), 0); };
    result
}
pub struct Lock {
    _file: File,
}
impl Lock {
    pub fn acquire(p: &Path, shared: bool) -> Result<Self> {
        let f = file(p)?;
        check(f.metadata()?.is_file(), "lock_not_regular")?;
        check(unsafe {
                        libc::flock(f.as_raw_fd(),
                            if shared { libc::LOCK_SH } else { libc::LOCK_EX })
                    } == 0, "OSError")?;
        Ok(Self { _file: f })
    }
}
pub fn remove(p: &Path) -> Result<()> {
    match fs::remove_file(p) {
        Ok(()) => sync(p.parent().unwrap()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
pub fn json(p: &Path) -> Result<uasf_json::Value> {
    crate::parse(&bytes(p, LIMIT)?.0)
}
