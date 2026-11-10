//! One control connection to a container: blocking request/response with per-request
//! timeouts. A reader thread (owned by the supervisor) feeds [`Connection::dispatch`].

use std::collections::HashMap;
use std::os::fd::BorrowedFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::Mutex;
use std::time::Duration;

use stpc_transport::channel::{FrameKind, Stream};
use stpc_transport::messages::{Request, Response, ERR_UNKNOWN_FORMAT, ERR_UNSUPPORTED};
use stpc_transport::wire::Wire;

use crate::{Error, Result};

pub(crate) struct Connection {
    writer: Mutex<Stream>,
    pending: Mutex<HashMap<u64, SyncSender<Response>>>,
    next_id: AtomicU64,
    dead: AtomicBool,
    /// Wake modes the container announced.
    pub wake_support: u32,
    pub pid: u32,
}

impl Connection {
    pub fn new(writer: Stream, wake_support: u32, pid: u32) -> Connection {
        Connection {
            writer: Mutex::new(writer),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            dead: AtomicBool::new(false),
            wake_support,
            pid,
        }
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Acquire)
    }

    /// Sends `request` and waits for its response. Plug-in level failures come back as
    /// `Err`; `Ok` is never `Response::Error`.
    pub fn call(&self, request: &Request, fds: &[BorrowedFd<'_>], timeout: Duration) -> Result<Response> {
        if self.is_dead() {
            return Err(Error::ContainerDied);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = sync_channel(1);
        self.pending.lock().unwrap().insert(id, tx);
        if self.is_dead() {
            // `fail_all` may have run between the check and the insert.
            self.pending.lock().unwrap().remove(&id);
            return Err(Error::ContainerDied);
        }
        let sent = {
            let writer = self.writer.lock().unwrap();
            writer.send(FrameKind::Request, id, &request.encode(), fds)
        };
        if sent.is_err() {
            // A partial frame may be on the wire: the stream is unusable. Closing it makes
            // the reader thread run the death procedure.
            self.pending.lock().unwrap().remove(&id);
            self.writer.lock().unwrap().shutdown();
            return Err(Error::ContainerDied);
        }
        match rx.recv_timeout(timeout) {
            Ok(Response::Error { code, message }) => Err(match code {
                ERR_UNSUPPORTED => Error::Unsupported,
                ERR_UNKNOWN_FORMAT => Error::UnknownFormat(message),
                _ => Error::Plugin(message),
            }),
            Ok(response) => Ok(response),
            Err(RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                Err(Error::Timeout(timeout))
            }
            Err(RecvTimeoutError::Disconnected) => Err(Error::ContainerDied),
        }
    }

    /// Reader thread: a response arrived.
    pub fn dispatch(&self, id: u64, response: Response) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
            let _ = tx.try_send(response);
        }
    }

    /// The container is gone: every waiting and future call fails with `ContainerDied`.
    pub fn fail_all(&self) {
        self.dead.store(true, Ordering::Release);
        self.pending.lock().unwrap().clear();
    }

    pub fn close(&self) {
        self.writer.lock().unwrap().shutdown();
    }
}
