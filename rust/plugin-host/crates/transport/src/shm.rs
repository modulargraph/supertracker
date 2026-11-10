//! Anonymous shared memory, handed to the other process as a file descriptor over the
//! control channel (`SCM_RIGHTS`). No name outlives the creating call, so nothing can leak
//! into `/dev/shm` (or the macOS POSIX shm namespace) whatever way either process dies.
//!
//! * Linux: `memfd_create`, sealed against resizing so the plug-in process cannot shrink
//!   the object under the host (which would turn host reads into SIGBUS).
//! * Elsewhere (macOS): `shm_open(O_CREAT|O_EXCL)` with a unique name, unlinked immediately.
//!   On Linux this path is used when `STPC_SHM=posix` is set, which is how it is exercised
//!   by the tests. macOS refuses a second `ftruncate` on a POSIX shm object, which gives the
//!   same no-shrink guarantee (by reading the xnu sources; not verified by running).

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU32, Ordering};

/// A shared mapping; unmapped on drop.
pub struct Mapping {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is plain memory; all access goes through the region protocol.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: `ptr`/`len` describe a mapping created by `mmap` below.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

fn map(fd: BorrowedFd<'_>, len: usize) -> io::Result<Mapping> {
    // SAFETY: a fresh shared mapping of `len` bytes; the kernel picks the address.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(Mapping { ptr: ptr.cast(), len })
}

/// Creates a zero-filled shared object of `len` bytes and maps it.
pub fn create(len: usize) -> io::Result<(OwnedFd, Mapping)> {
    let fd = create_fd(len)?;
    let mapping = map(fd.as_fd_compat(), len)?;
    Ok((fd, mapping))
}

/// Maps an object received from the other process. Fails if it is smaller than `len`
/// (touching pages past the end of the object would raise SIGBUS).
pub fn attach(fd: BorrowedFd<'_>, len: usize) -> io::Result<Mapping> {
    // SAFETY: `st` is a valid out-parameter for fstat.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_size as u64) < len as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("shared object has {} bytes, {} expected", st.st_size, len),
        ));
    }
    map(fd, len)
}

trait AsFdCompat {
    fn as_fd_compat(&self) -> BorrowedFd<'_>;
}
impl AsFdCompat for OwnedFd {
    fn as_fd_compat(&self) -> BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(self)
    }
}

fn create_fd(len: usize) -> io::Result<OwnedFd> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        if std::env::var_os("STPC_SHM").is_none_or(|v| v != "posix") {
            return create_memfd(len);
        }
    }
    create_posix_shm(len)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn create_memfd(len: usize) -> io::Result<OwnedFd> {
    // SAFETY: NUL-terminated literal name; the flags are valid.
    let raw = unsafe {
        libc::memfd_create(c"stpc-region".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING)
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a new descriptor we own.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: plain ftruncate/fcntl on our descriptor.
    unsafe {
        if libc::ftruncate(raw, len as libc::off_t) != 0 {
            return Err(io::Error::last_os_error());
        }
        // Best effort: the object can no longer be resized by either process.
        libc::fcntl(raw, libc::F_ADD_SEALS, libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL);
    }
    Ok(fd)
}

/// POSIX shared memory with an immediately unlinked name. Runs on Linux only under
/// `STPC_SHM=posix`; on macOS it is the only path and is TYPE-CHECKED ONLY there.
fn create_posix_shm(len: usize) -> io::Result<OwnedFd> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    for _ in 0..64 {
        // macOS limits shm names to 31 bytes (PSHMNAMLEN): "/stpc-" + 8 + "-" + 8 hex digits.
        let nonce = (crate::monotonic_ns() as u32) ^ COUNTER.fetch_add(0x9E37_79B9, Ordering::Relaxed);
        let name = CString::new(format!("/stpc-{:08x}-{:08x}", std::process::id(), nonce)).unwrap();
        // SAFETY: valid NUL-terminated name. `mode` is passed as an int-sized variadic argument.
        let raw = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            )
        };
        if raw < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(err);
        }
        // SAFETY: `raw` is a new descriptor we own; the name is ours to unlink.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        unsafe { libc::shm_unlink(name.as_ptr()) };
        // SAFETY: plain ftruncate on our descriptor.
        if unsafe { libc::ftruncate(raw, len as libc::off_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(fd);
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free shared-memory name"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn create_attach_share_memory() {
        let (fd, a) = create(8192).unwrap();
        let b = attach(fd.as_fd(), 8192).unwrap();
        // SAFETY: both mappings are 8192 bytes long.
        unsafe {
            *a.as_ptr().add(5000) = 42;
            assert_eq!(*b.as_ptr().add(5000), 42);
        }
        assert!(attach(fd.as_fd(), 8193).is_err());
    }

    #[test]
    fn posix_shm_path_leaves_no_name() {
        let fd = create_posix_shm(4096).unwrap();
        let m = attach(fd.as_fd(), 4096).unwrap();
        assert_eq!(m.len(), 4096);
        #[cfg(target_os = "linux")]
        {
            let pid = format!("stpc-{:08x}-", std::process::id());
            let leaked: Vec<_> = std::fs::read_dir("/dev/shm")
                .map(|dir| dir.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&pid)).collect())
                .unwrap_or_default();
            assert!(leaked.is_empty(), "leaked: {leaked:?}");
        }
    }
}
