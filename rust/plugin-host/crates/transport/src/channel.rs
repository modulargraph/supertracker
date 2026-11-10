//! The control channel: length-prefixed frames over a Unix domain stream socket, with file
//! descriptors (the shared region, wake pipes) attached to a frame via `SCM_RIGHTS`.
//!
//! Frame: `[u32 payload length][u8 kind][u64 id][payload]`, little-endian.
//!
//! The supervisor binds a [`Listener`] at a fresh path inside its private socket directory,
//! passes the path (the *endpoint*) to the container on the command line, accepts exactly
//! one connection and removes the path. A Windows implementation would keep this API and
//! use a named pipe as the endpoint; descriptors would become duplicated handles or named
//! mappings carried in the payload.

use std::io::{self, Read};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Handshake = 0,
    Request = 1,
    Response = 2,
    Notification = 3,
}

impl FrameKind {
    fn from_u8(value: u8) -> io::Result<FrameKind> {
        Ok(match value {
            0 => FrameKind::Handshake,
            1 => FrameKind::Request,
            2 => FrameKind::Response,
            3 => FrameKind::Notification,
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown frame kind")),
        })
    }
}

pub struct Frame {
    pub kind: FrameKind,
    pub id: u64,
    pub payload: Vec<u8>,
    pub fds: Vec<OwnedFd>,
}

const HEADER_LEN: usize = 13;
/// Most descriptors one frame may carry.
pub const MAX_FDS: usize = 4;

/// One end of the control channel. Sending and receiving may happen on different threads
/// (use [`Stream::try_clone`]); concurrent senders must serialise themselves.
pub struct Stream {
    sock: UnixStream,
}

impl Stream {
    pub fn connect(endpoint: &str) -> io::Result<Stream> {
        Stream::from_std(UnixStream::connect(endpoint)?)
    }

    fn from_std(sock: UnixStream) -> io::Result<Stream> {
        #[cfg(target_os = "macos")]
        {
            // macOS has no MSG_NOSIGNAL: suppress SIGPIPE per socket. TYPE-CHECKED ONLY.
            let one: libc::c_int = 1;
            // SAFETY: valid socket and option buffer.
            unsafe {
                libc::setsockopt(
                    sock.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_NOSIGPIPE,
                    (&one as *const libc::c_int).cast(),
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
            }
        }
        Ok(Stream { sock })
    }

    /// A connected pair, for tests.
    pub fn pair() -> io::Result<(Stream, Stream)> {
        let (a, b) = UnixStream::pair()?;
        Ok((Stream::from_std(a)?, Stream::from_std(b)?))
    }

    pub fn try_clone(&self) -> io::Result<Stream> {
        Ok(Stream { sock: self.sock.try_clone()? })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.sock.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.sock.set_write_timeout(timeout)
    }

    /// Closes both directions; a thread blocked in [`Stream::recv`] on any clone returns.
    pub fn shutdown(&self) {
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
    }

    pub fn send(&self, kind: FrameKind, id: u64, payload: &[u8], fds: &[BorrowedFd<'_>]) -> io::Result<()> {
        if payload.len() > crate::MAX_FRAME_BYTES || fds.len() > MAX_FDS {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame too large"));
        }
        let mut header = [0u8; HEADER_LEN];
        header[0..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        header[4] = kind as u8;
        header[5..13].copy_from_slice(&id.to_le_bytes());

        // The descriptors ride on the first sendmsg call (which always carries at least
        // the first header byte); whatever that call did not write follows with plain sends.
        let mut iov = [
            libc::iovec { iov_base: header.as_ptr() as *mut libc::c_void, iov_len: HEADER_LEN },
            libc::iovec { iov_base: payload.as_ptr() as *mut libc::c_void, iov_len: payload.len() },
        ];
        let mut control = [0u64; 8]; // u64 for alignment; 64 bytes hold MAX_FDS descriptors
        // SAFETY: zeroed msghdr is valid; the fields set below point to live buffers.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = iov.as_mut_ptr();
        msg.msg_iovlen = 2;
        if !fds.is_empty() {
            let raw: Vec<RawFd> = fds.iter().map(|fd| fd.as_raw_fd()).collect();
            let data_len = std::mem::size_of_val(&raw[..]);
            // SAFETY: `control` is large enough for CMSG_SPACE(4 * sizeof(int)) (asserted),
            // aligned for cmsghdr, and outlives the sendmsg call.
            unsafe {
                let space = libc::CMSG_SPACE(data_len as u32) as usize;
                assert!(space <= std::mem::size_of_val(&control));
                msg.msg_control = control.as_mut_ptr().cast();
                msg.msg_controllen = space as _;
                let cmsg = libc::CMSG_FIRSTHDR(&msg);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len = libc::CMSG_LEN(data_len as u32) as _;
                std::ptr::copy_nonoverlapping(raw.as_ptr().cast::<u8>(), libc::CMSG_DATA(cmsg), data_len);
            }
        }
        let total = HEADER_LEN + payload.len();
        let mut sent = loop {
            // SAFETY: `msg` points to valid buffers for the duration of the call.
            let n = unsafe { libc::sendmsg(self.sock.as_raw_fd(), &msg, SEND_FLAGS) };
            if n >= 0 {
                break n as usize;
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(err);
            }
        };
        while sent < total {
            let chunk: &[u8] = if sent < HEADER_LEN { &header[sent..] } else { &payload[sent - HEADER_LEN..] };
            // SAFETY: `chunk` is a valid buffer.
            let n = unsafe { libc::send(self.sock.as_raw_fd(), chunk.as_ptr().cast(), chunk.len(), SEND_FLAGS) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            sent += n as usize;
        }
        Ok(())
    }

    /// Blocks for the next frame. `UnexpectedEof` when the peer closed the connection.
    pub fn recv(&self) -> io::Result<Frame> {
        let mut header = [0u8; HEADER_LEN];
        let mut fds = Vec::new();
        let mut got = 0;
        while got < HEADER_LEN {
            let mut iov = libc::iovec {
                iov_base: header[got..].as_mut_ptr().cast(),
                iov_len: HEADER_LEN - got,
            };
            let mut control = [0u64; 8];
            // SAFETY: zeroed msghdr is valid; buffers are live for the call.
            let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = std::mem::size_of_val(&control) as _;
            // SAFETY: as above.
            let n = unsafe { libc::recvmsg(self.sock.as_raw_fd(), &mut msg, RECV_FLAGS) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            // SAFETY: the kernel filled `msg_controllen` bytes of `control` with cmsghdrs.
            unsafe {
                let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
                while !cmsg.is_null() {
                    if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                        let data_len = (*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                        let count = data_len / std::mem::size_of::<RawFd>();
                        let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                        for i in 0..count {
                            let fd = OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i)));
                            set_cloexec(fd.as_raw_fd());
                            fds.push(fd);
                        }
                    }
                    cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
                }
            }
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            got += n as usize;
        }
        let len = u32::from_le_bytes(header[0..4].try_into().unwrap()) as usize;
        if len > crate::MAX_FRAME_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
        }
        let kind = FrameKind::from_u8(header[4])?;
        let id = u64::from_le_bytes(header[5..13].try_into().unwrap());
        let mut payload = Vec::new();
        // Grow as data arrives instead of trusting the announced length for one allocation.
        let mut reader = (&self.sock).take(len as u64);
        reader.read_to_end(&mut payload)?;
        if payload.len() != len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(Frame { kind, id, payload, fds })
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const SEND_FLAGS: libc::c_int = libc::MSG_NOSIGNAL;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const SEND_FLAGS: libc::c_int = 0;

#[cfg(any(target_os = "linux", target_os = "android"))]
const RECV_FLAGS: libc::c_int = libc::MSG_CMSG_CLOEXEC;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const RECV_FLAGS: libc::c_int = 0;

fn set_cloexec(fd: RawFd) {
    // SAFETY: plain fcntl on a descriptor we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// The supervisor's side of an endpoint that one container will connect to.
pub struct Listener {
    inner: UnixListener,
    path: PathBuf,
}

impl Listener {
    /// Binds `dir/name`. The path must be short enough for `sockaddr_un` (104 bytes on macOS).
    pub fn bind(dir: &Path, name: &str) -> io::Result<Listener> {
        let path = dir.join(name);
        let inner = UnixListener::bind(&path)?;
        inner.set_nonblocking(true)?;
        Ok(Listener { inner, path })
    }

    /// What the container passes to [`Stream::connect`].
    pub fn endpoint(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    /// Waits for the connection. `keep_waiting` is polled every few milliseconds and can
    /// abort the wait (e.g. the child process exited).
    pub fn accept(&self, timeout: Duration, mut keep_waiting: impl FnMut() -> bool) -> io::Result<Stream> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.inner.accept() {
                Ok((sock, _)) => {
                    sock.set_nonblocking(false)?;
                    return Stream::from_std(sock);
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
            if !keep_waiting() {
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "the container exited before connecting"));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the container did not connect in time"));
            }
            let mut pfd = libc::pollfd { fd: self.inner.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let ms = (deadline - now).as_millis().clamp(1, 5) as libc::c_int;
            // SAFETY: one valid pollfd.
            unsafe { libc::poll(&mut pfd, 1, ms) };
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn frames_with_descriptors_and_large_payloads() {
        let (a, b) = Stream::pair().unwrap();
        let (fd, map) = crate::shm::create(4096).unwrap();
        // SAFETY: the mapping is 4096 bytes.
        unsafe { *map.as_ptr() = 99 };
        let big: Vec<u8> = (0..5_000_000u32).map(|i| (i * 7) as u8).collect();
        let big_copy = big.clone();
        let sender = std::thread::spawn(move || {
            a.send(FrameKind::Request, 7, b"hello", &[fd.as_fd()]).unwrap();
            a.send(FrameKind::Response, 8, &big_copy, &[]).unwrap();
            a.send(FrameKind::Notification, 9, &[], &[]).unwrap();
        });
        let first = b.recv().unwrap();
        assert_eq!((first.kind, first.id, first.payload.as_slice()), (FrameKind::Request, 7, &b"hello"[..]));
        assert_eq!(first.fds.len(), 1);
        let other = crate::shm::attach(first.fds[0].as_fd(), 4096).unwrap();
        // SAFETY: the mapping is 4096 bytes.
        assert_eq!(unsafe { *other.as_ptr() }, 99);
        let second = b.recv().unwrap();
        assert_eq!(second.id, 8);
        assert!(second.payload == big);
        assert!(second.fds.is_empty());
        let third = b.recv().unwrap();
        assert_eq!((third.kind, third.payload.len()), (FrameKind::Notification, 0));
        sender.join().unwrap();
        assert_eq!(b.recv().err().unwrap().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn listener_accepts_once_and_removes_its_path() {
        let dir = std::env::temp_dir().join(format!("stpc-test-{}-{}", std::process::id(), crate::monotonic_ns()));
        std::fs::create_dir(&dir).unwrap();
        let listener = Listener::bind(&dir, "c1.sock").unwrap();
        let endpoint = listener.endpoint();
        assert_eq!(
            listener.accept(Duration::from_millis(30), || true).err().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            listener.accept(Duration::from_secs(5), || false).err().unwrap().kind(),
            io::ErrorKind::ConnectionAborted
        );
        let client = std::thread::spawn(move || {
            let stream = Stream::connect(&endpoint).unwrap();
            stream.send(FrameKind::Handshake, 0, b"x", &[]).unwrap();
        });
        let stream = listener.accept(Duration::from_secs(5), || true).unwrap();
        assert_eq!(stream.recv().unwrap().payload, b"x");
        client.join().unwrap();
        drop(listener);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir(&dir).unwrap();
    }
}
