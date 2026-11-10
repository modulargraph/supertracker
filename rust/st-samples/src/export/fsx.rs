//! Filesystem operations the standard library lacks: publishing a file or folder under a name
//! only if that name is free, and (on Unix) operations relative to an open directory, which keep
//! working on the same directory object even if its path is renamed or replaced meanwhile.

use std::path::Path;

/// Renames `from` to `to`, failing if `to` exists. Atomic: `to` is either absent or complete.
pub(crate) fn rename_exclusive(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        unix::rename_exclusive_raw(libc::AT_FDCWD, &from, libc::AT_FDCWD, &to, false)
    }
    #[cfg(not(unix))]
    {
        // A hard link fails when the target exists and appears atomically.
        std::fs::hard_link(from, to)?;
        std::fs::remove_file(from)
    }
}

#[cfg(unix)]
pub(crate) use unix::*;

#[cfg(unix)]
mod unix {
    use std::ffi::{CStr, CString, OsString};
    use std::io;
    use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;

    /// What `stat` says about a file, reduced to what identifies it and reveals a change.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Identity {
        pub device: u64,
        pub inode: u64,
        pub mode: u32,
        pub size: i64,
        modified: (i64, i64),
        changed: (i64, i64),
    }

    /// File type bits of a mode.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Kind {
        File,
        Directory,
        Other,
    }

    // The field types of `stat` differ between platforms; the casts are not redundant on all.
    #[allow(clippy::unnecessary_cast)]
    impl Identity {
        fn new(value: &libc::stat) -> Identity {
            Identity {
                device: value.st_dev as u64,
                inode: value.st_ino as u64,
                mode: value.st_mode as u32,
                size: value.st_size as i64,
                modified: (value.st_mtime as i64, value.st_mtime_nsec as i64),
                changed: (value.st_ctime as i64, value.st_ctime_nsec as i64),
            }
        }

        pub fn kind(&self) -> Kind {
            match self.mode & libc::S_IFMT as u32 {
                x if x == libc::S_IFREG as u32 => Kind::File,
                x if x == libc::S_IFDIR as u32 => Kind::Directory,
                _ => Kind::Other,
            }
        }

        /// Same filesystem object (whatever happened to its content).
        pub fn same_object(&self, other: &Identity) -> bool {
            self.device == other.device && self.inode == other.inode && self.kind() == other.kind()
        }
    }

    fn name(value: &str) -> io::Result<CString> {
        CString::new(value).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))
    }

    fn check(result: libc::c_int) -> io::Result<()> {
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    const DIRECTORY_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    /// Opens a directory by path without following a symbolic link in the last component.
    pub(crate) fn open_dir(path: &Path) -> io::Result<OwnedFd> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: `path` is a valid NUL-terminated string; the returned descriptor is owned here.
        let fd = unsafe { libc::open(path.as_ptr(), DIRECTORY_FLAGS) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Opens the directory `leaf` inside `dir`, not following a symbolic link.
    pub(crate) fn open_dir_at(dir: BorrowedFd<'_>, leaf: &str) -> io::Result<OwnedFd> {
        let leaf = name(leaf)?;
        // SAFETY: valid descriptor and NUL-terminated string.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), leaf.as_ptr(), DIRECTORY_FLAGS) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    pub(crate) fn mkdir_at(dir: BorrowedFd<'_>, leaf: &str, mode: u32) -> io::Result<()> {
        let leaf = name(leaf)?;
        // SAFETY: valid descriptor and NUL-terminated string.
        check(unsafe { libc::mkdirat(dir.as_raw_fd(), leaf.as_ptr(), mode as libc::mode_t) })
    }

    /// Removes the entry `leaf` of `dir`: an empty directory when `directory`, else a file or
    /// symbolic link (which is removed itself, never followed).
    pub(crate) fn unlink_at(dir: BorrowedFd<'_>, leaf: &str, directory: bool) -> io::Result<()> {
        let leaf = name(leaf)?;
        // SAFETY: valid descriptor and NUL-terminated string.
        check(unsafe { libc::unlinkat(dir.as_raw_fd(), leaf.as_ptr(), if directory { libc::AT_REMOVEDIR } else { 0 }) })
    }

    pub(crate) fn chmod_fd(fd: BorrowedFd<'_>, mode: u32) -> io::Result<()> {
        // SAFETY: valid descriptor.
        check(unsafe { libc::fchmod(fd.as_raw_fd(), mode as libc::mode_t) })
    }

    pub(crate) fn stat_fd(fd: BorrowedFd<'_>) -> io::Result<Identity> {
        // SAFETY: `value` is a properly sized, writable `stat`; all-zero is a valid bit pattern.
        let mut value: libc::stat = unsafe { std::mem::zeroed() };
        check(unsafe { libc::fstat(fd.as_raw_fd(), &mut value) })?;
        Ok(Identity::new(&value))
    }

    fn optional(result: libc::c_int, value: &libc::stat) -> io::Result<Option<Identity>> {
        if result == 0 {
            return Ok(Some(Identity::new(value)));
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(None)
        } else {
            Err(error)
        }
    }

    /// The entry `leaf` of `dir` itself (a symbolic link is not followed); `None` when absent.
    pub(crate) fn stat_at(dir: BorrowedFd<'_>, leaf: &str) -> io::Result<Option<Identity>> {
        let leaf = name(leaf)?;
        // SAFETY: as in `stat_fd`, plus a valid NUL-terminated string.
        let mut value: libc::stat = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::fstatat(dir.as_raw_fd(), leaf.as_ptr(), &mut value, libc::AT_SYMLINK_NOFOLLOW) };
        optional(result, &value)
    }

    /// `path` itself (a symbolic link is not followed); `None` when absent.
    pub(crate) fn lstat(path: &Path) -> io::Result<Option<Identity>> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: as in `stat_at`.
        let mut value: libc::stat = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::lstat(path.as_ptr(), &mut value) };
        optional(result, &value)
    }

    pub(crate) fn rename_at(from_dir: BorrowedFd<'_>, from: &str, to_dir: BorrowedFd<'_>, to: &str) -> io::Result<()> {
        let (from, to) = (name(from)?, name(to)?);
        // SAFETY: valid descriptors and NUL-terminated strings.
        check(unsafe { libc::renameat(from_dir.as_raw_fd(), from.as_ptr(), to_dir.as_raw_fd(), to.as_ptr()) })
    }

    /// Renames without replacing: fails with "already exists" when `to` is taken.
    pub(crate) fn rename_exclusive_at(from_dir: BorrowedFd<'_>, from: &str, to_dir: BorrowedFd<'_>, to: &str, directory: bool) -> io::Result<()> {
        rename_exclusive_raw(from_dir.as_raw_fd(), &name(from)?, to_dir.as_raw_fd(), &name(to)?, directory)
    }

    pub(super) fn rename_exclusive_raw(from_dir: RawFd, from: &CStr, to_dir: RawFd, to: &CStr, directory: bool) -> io::Result<()> {
        // The kernel's atomic "rename unless the target exists".
        #[cfg(any(target_os = "linux", target_os = "android"))]
        // SAFETY: renameat2 takes two (descriptor, NUL-terminated path) pairs and a flag word.
        let result = unsafe { libc::syscall(libc::SYS_renameat2, from_dir, from.as_ptr(), to_dir, to.as_ptr(), libc::RENAME_NOREPLACE) as libc::c_int };
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        // SAFETY: renameatx_np has the same shape as renameat2.
        let result = unsafe { libc::renameatx_np(from_dir, from.as_ptr(), to_dir, to.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos", target_os = "ios"))]
        {
            if result == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            // Only a filesystem (or kernel) without the call falls through to the portable way.
            if !matches!(error.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::ENOTSUP)) {
                return Err(error);
            }
        }
        if directory {
            // Creating the target directory is exclusive; renaming a directory onto an empty
            // directory then replaces it atomically.
            // SAFETY: valid descriptors and NUL-terminated strings.
            check(unsafe { libc::mkdirat(to_dir, to.as_ptr(), 0o700) })?;
            let renamed = check(unsafe { libc::renameat(from_dir, from.as_ptr(), to_dir, to.as_ptr()) });
            if renamed.is_err() {
                unsafe { libc::unlinkat(to_dir, to.as_ptr(), libc::AT_REMOVEDIR) };
            }
            renamed
        } else {
            // A hard link fails when the target exists and appears atomically.
            // SAFETY: valid descriptors and NUL-terminated strings.
            check(unsafe { libc::linkat(from_dir, from.as_ptr(), to_dir, to.as_ptr(), 0) })?;
            check(unsafe { libc::unlinkat(from_dir, from.as_ptr(), 0) })
        }
    }

    /// The names in a directory, without "." and "..".
    pub(crate) fn names(dir: BorrowedFd<'_>) -> io::Result<Vec<OsString>> {
        // A fresh open file description: the listing must not share a read offset with `dir`.
        // SAFETY: valid descriptor and a static NUL-terminated string.
        let copy = unsafe { libc::openat(dir.as_raw_fd(), c".".as_ptr(), DIRECTORY_FLAGS) };
        if copy < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `copy` is an open directory descriptor; on success the stream owns it.
        let stream = unsafe { libc::fdopendir(copy) };
        if stream.is_null() {
            let error = io::Error::last_os_error();
            unsafe { libc::close(copy) };
            return Err(error);
        }
        let mut result = Vec::new();
        let outcome = loop {
            clear_errno();
            // SAFETY: `stream` is a valid directory stream until `closedir` below.
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                break if !ERRNO_CLEARED || error.raw_os_error().unwrap_or(0) == 0 { Ok(()) } else { Err(error) };
            }
            // SAFETY: `d_name` is a NUL-terminated string inside the entry `readdir` returned.
            let leaf = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if leaf != b"." && leaf != b".." {
                result.push(OsString::from_vec(leaf.to_vec()));
            }
        };
        // SAFETY: `stream` came from `fdopendir` and is closed exactly once.
        unsafe { libc::closedir(stream) };
        outcome.map(|()| result)
    }

    /// Whether `clear_errno` works here, so that a null `readdir` result can be told apart
    /// from an error. Elsewhere a null result is taken as the end of the directory.
    const ERRNO_CLEARED: bool = cfg!(any(target_os = "linux", target_os = "macos", target_os = "ios", target_os = "freebsd"));

    fn clear_errno() {
        // SAFETY: the errno location is a valid thread-local integer.
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = 0
        };
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
        unsafe {
            *libc::__error() = 0
        };
    }
}
