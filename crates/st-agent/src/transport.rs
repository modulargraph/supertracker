//! The local byte-stream transport under the control protocol.
//!
//! Everything above this module works with two traits, [`Channel`] (a connected stream) and
//! [`Listener`] (the application's end), and the two functions [`listen`] and [`connect`]. Unix
//! domain sockets implement them today. A Windows named pipe is one more implementation of the same
//! two traits behind the same two functions; until it exists they fail there with a message.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::ControlError;

/// A connected stream. Handles are shared between threads: one thread reads, another may write or
/// close.
pub trait Channel: Send + Sync {
    /// Reads available bytes; `Ok(0)` means the other side closed the stream.
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize>;
    /// Writes all of `bytes`.
    fn write_all(&self, bytes: &[u8]) -> io::Result<()>;
    /// Limits how long one `read` waits (`None`: forever).
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    /// Limits how long a `write_all` may make no progress (`None`: forever).
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    /// Closes both directions now. Reads and writes blocked on other threads return at once; the
    /// other side sees the end of the stream. Closing twice is harmless.
    fn close(&self);
}

/// The listening end.
pub trait Listener: Send + Sync {
    /// Waits for the next client of the same user. Returns `Ok(None)` once the listener is closed.
    fn accept(&self) -> io::Result<Option<Arc<dyn Channel>>>;
    /// Stops accepting and removes the endpoint. `accept` returns `Ok(None)` shortly after.
    fn close(&self);
}

/// Opens the control endpoint at `path` for this user only.
///
/// An endpoint another running application answers on is left alone (an error says so); a stale
/// one is replaced.
pub fn listen(path: &Path) -> Result<Arc<dyn Listener>, ControlError> {
    platform::listen(path)
}

/// Connects to the application's control endpoint.
pub fn connect(path: &Path) -> Result<Arc<dyn Channel>, ControlError> {
    platform::connect(path)
}

#[cfg(unix)]
mod platform {
    use std::io::{self, Read, Write};
    use std::net::Shutdown;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::{Channel, Listener};
    use crate::ControlError;

    struct UnixChannel(UnixStream);

    impl Channel for UnixChannel {
        fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
            loop {
                match (&self.0).read(buffer) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    other => return other,
                }
            }
        }

        fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
            (&self.0).write_all(bytes)
        }

        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.0.set_read_timeout(timeout)
        }

        fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.0.set_write_timeout(timeout)
        }

        fn close(&self) {
            // The descriptor itself stays open until the last handle is dropped, so a thread that
            // is still writing can never write into an unrelated descriptor that reused the number.
            let _ = self.0.shutdown(Shutdown::Both);
        }
    }

    struct UnixControlListener {
        listener: UnixListener,
        path: PathBuf,
        closed: AtomicBool,
    }

    impl Listener for UnixControlListener {
        fn accept(&self) -> io::Result<Option<Arc<dyn Channel>>> {
            loop {
                if self.closed.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                // The listener is non-blocking: wait in short slices so that `close` is noticed on
                // every Unix (shutting a listening socket down does not wake `accept` everywhere).
                let mut poller = libc::pollfd { fd: self.listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                // SAFETY: `poller` is one valid pollfd for the duration of the call.
                let ready = unsafe { libc::poll(&mut poller, 1, 200) };
                if ready <= 0 {
                    continue;
                }
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        if self.closed.load(Ordering::SeqCst) {
                            return Ok(None);
                        }
                        if peer_is_this_user(&stream) && stream.set_nonblocking(false).is_ok() {
                            return Ok(Some(Arc::new(UnixChannel(stream))));
                        }
                    }
                    Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted) => {}
                    Err(error) => return Err(error),
                }
            }
        }

        fn close(&self) {
            if !self.closed.swap(true, Ordering::SeqCst) {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    impl Drop for UnixControlListener {
        fn drop(&mut self) {
            Listener::close(self);
        }
    }

    fn peer_is_this_user(stream: &UnixStream) -> bool {
        // SAFETY: plain system calls on a descriptor this function borrows; the out-parameters are
        // valid for the duration of each call.
        unsafe {
            let me = libc::getuid();
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                let mut credentials: libc::ucred = std::mem::zeroed();
                let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
                let status = libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut credentials as *mut libc::ucred).cast(), &mut length);
                status == 0 && credentials.uid == me
            }
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            {
                let mut uid: libc::uid_t = 0;
                let mut gid: libc::gid_t = 0;
                libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) == 0 && uid == me
            }
        }
    }

    fn address(path: &Path) -> Result<(libc::sockaddr_un, libc::socklen_t), ControlError> {
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: an all-zero `sockaddr_un` is a valid (empty) address.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
            return Err(ControlError::Transport(format!("The control socket path is too long: {}", path.display())));
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
            *target = *byte as libc::c_char;
        }
        Ok((address, std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t))
    }

    fn system_error(action: &str) -> ControlError {
        ControlError::Transport(format!("{action}: {}.", io::Error::last_os_error()))
    }

    pub fn listen(path: &Path) -> Result<Arc<dyn Listener>, ControlError> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)
                .map_err(|error| ControlError::Transport(format!("Could not create the folder of the control socket ({}): {error}.", folder.display())))?;
        }
        if std::fs::symlink_metadata(path).is_ok() {
            // A live socket belongs to another running application; a stale one is replaced.
            if UnixStream::connect(path).is_ok() {
                return Err(ControlError::Transport("Another SuperTracker window is already accepting AI control.".into()));
            }
            let _ = std::fs::remove_file(path);
        }
        let (address, length) = address(path)?;
        // The socket is created by hand rather than with `UnixListener::bind`, which starts
        // listening before the file's permissions can be narrowed.
        // SAFETY: ordinary socket calls; the descriptor is owned by `OwnedFd` from its creation, so
        // every early return closes it.
        let listener = unsafe {
            let raw = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
            if raw < 0 {
                return Err(system_error("Could not create the control socket"));
            }
            let owned = OwnedFd::from_raw_fd(raw);
            libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC);
            if libc::bind(raw, (&address as *const libc::sockaddr_un).cast(), length) != 0 {
                return Err(system_error("Could not open the control socket"));
            }
            UnixListener::from(owned)
        };
        // User-only access before any connection can be accepted.
        let secured = std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .map_err(|error| error.to_string())
            // SAFETY: `listen` on the socket this function just bound.
            .and_then(|()| if unsafe { libc::listen(listener.as_raw_fd(), 8) } == 0 { Ok(()) } else { Err(io::Error::last_os_error().to_string()) })
            .and_then(|()| listener.set_nonblocking(true).map_err(|error| error.to_string()));
        if let Err(error) = secured {
            let _ = std::fs::remove_file(path);
            return Err(ControlError::Transport(format!("Could not secure the control socket: {error}.")));
        }
        Ok(Arc::new(UnixControlListener { listener, path: path.to_path_buf(), closed: AtomicBool::new(false) }))
    }

    pub fn connect(path: &Path) -> Result<Arc<dyn Channel>, ControlError> {
        address(path)?;
        match UnixStream::connect(path) {
            Ok(stream) => Ok(Arc::new(UnixChannel(stream))),
            Err(error) => Err(ControlError::NotRunning(format!("{error} at {}", path.display()))),
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use std::path::Path;
    use std::sync::Arc;

    use super::{Channel, Listener};
    use crate::ControlError;

    const UNSUPPORTED: &str = "AI control is not available on this system yet (it needs a named-pipe transport).";

    pub fn listen(_path: &Path) -> Result<Arc<dyn Listener>, ControlError> {
        Err(ControlError::Transport(UNSUPPORTED.into()))
    }

    pub fn connect(_path: &Path) -> Result<Arc<dyn Channel>, ControlError> {
        Err(ControlError::NotRunning(UNSUPPORTED.into()))
    }
}
