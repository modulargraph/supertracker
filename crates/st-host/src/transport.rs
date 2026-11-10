//! How messages reach the interface. The bridge core knows only [`Transport`]; the window's web view
//! channel and the WebSocket of `--serve` implement it in `st-app`, both on top of [`Outbox`].

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// One message for the interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// The JSON text (a reply or an event, see [`crate::protocol`]).
    pub json: String,
    /// Set for events of which only the newest matters (`EventSink::emit_latest`): the event's name.
    /// A transport that has not delivered the previous one yet replaces it instead of queueing both.
    pub latest_only: Option<String>,
}

/// Carries messages to one interface (one page). Replies and events arrive in the order they were sent.
pub trait Transport: Send + Sync + 'static {
    /// Accepts a message for delivery. Called from any thread, including service threads that must not
    /// wait: an implementation queues and returns.
    fn send(&self, message: Outgoing);
}

/// The queue between the bridge and whatever writes to the interface. It is a [`Transport`].
///
/// Events marked `latest_only` replace their undelivered predecessor in place, so a page that is busy
/// (or hung) finds one transport status waiting instead of thousands.
pub struct Outbox {
    state: Mutex<State>,
    arrived: Condvar,
    wake: Option<Box<dyn Fn() + Send + Sync>>,
}

struct State {
    queue: VecDeque<Outgoing>,
    closed: bool,
}

/// What [`Outbox::wait`] returned.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    Message(String),
    /// Nothing arrived within the timeout.
    Idle,
    /// The outbox was closed and everything queued before that has been taken.
    Closed,
}

impl Outbox {
    /// An outbox whose consumer blocks in [`Outbox::wait`] (the WebSocket's writer thread).
    pub fn new() -> Arc<Self> {
        Arc::new(Outbox { state: Mutex::new(State { queue: VecDeque::new(), closed: false }), arrived: Condvar::new(), wake: None })
    }

    /// An outbox that calls `wake` whenever it goes from empty to non-empty, for a consumer that runs
    /// in an event loop and takes messages with [`Outbox::drain`].
    pub fn with_wake(wake: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Outbox {
            state: Mutex::new(State { queue: VecDeque::new(), closed: false }),
            arrived: Condvar::new(),
            wake: Some(Box::new(wake)),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // The queue stays consistent even if a thread panicked while holding the lock.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Queues a message. After [`Outbox::close`] messages are discarded: nobody is listening.
    pub fn push(&self, message: Outgoing) {
        let was_empty;
        {
            let mut state = self.lock();
            if state.closed {
                return;
            }
            was_empty = state.queue.is_empty();
            let replaced = message.latest_only.is_some()
                && state.queue.iter_mut().any(|queued| {
                    let same = queued.latest_only == message.latest_only;
                    if same {
                        queued.json.clone_from(&message.json);
                    }
                    same
                });
            if !replaced {
                state.queue.push_back(message);
            }
        }
        self.arrived.notify_one();
        if was_empty {
            if let Some(wake) = &self.wake {
                wake();
            }
        }
    }

    /// Takes everything queued, oldest first.
    pub fn drain(&self) -> Vec<String> {
        self.lock().queue.drain(..).map(|message| message.json).collect()
    }

    /// Takes the oldest message, waiting up to `timeout` for one.
    pub fn wait(&self, timeout: Duration) -> Next {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if let Some(message) = state.queue.pop_front() {
                return Next::Message(message.json);
            }
            if state.closed {
                return Next::Closed;
            }
            let now = Instant::now();
            if now >= deadline {
                return Next::Idle;
            }
            state = self.arrived.wait_timeout(state, deadline - now).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
    }

    /// Stops accepting messages. What is already queued can still be taken.
    pub fn close(&self) {
        self.lock().closed = true;
        self.arrived.notify_all();
    }

    /// The number of undelivered messages.
    pub fn len(&self) -> usize {
        self.lock().queue.len()
    }

    /// True when nothing waits for delivery.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Transport for Outbox {
    fn send(&self, message: Outgoing) {
        self.push(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn plain(json: &str) -> Outgoing {
        Outgoing { json: json.to_string(), latest_only: None }
    }

    fn latest(name: &str, json: &str) -> Outgoing {
        Outgoing { json: json.to_string(), latest_only: Some(name.to_string()) }
    }

    #[test]
    fn messages_leave_in_the_order_they_arrived() {
        let outbox = Outbox::new();
        outbox.push(plain("1"));
        outbox.push(plain("2"));
        assert_eq!(outbox.drain(), ["1", "2"]);
        assert!(outbox.is_empty());
    }

    #[test]
    fn a_newer_snapshot_replaces_the_undelivered_one_in_place() {
        let outbox = Outbox::new();
        outbox.push(latest("status", "status-1"));
        outbox.push(plain("reply"));
        outbox.push(latest("sampler.playheads", "heads-1"));
        outbox.push(latest("status", "status-2"));
        outbox.push(latest("status", "status-3"));
        assert_eq!(outbox.drain(), ["status-3", "reply", "heads-1"]);
        // Once delivered, the next one queues again.
        outbox.push(latest("status", "status-4"));
        assert_eq!(outbox.drain(), ["status-4"]);
    }

    #[test]
    fn wake_runs_only_when_the_queue_becomes_non_empty() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        let outbox = Outbox::with_wake(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        outbox.push(plain("1"));
        outbox.push(plain("2"));
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        outbox.drain();
        outbox.push(plain("3"));
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn wait_returns_messages_then_idle_then_closed() {
        let outbox = Outbox::new();
        assert_eq!(outbox.wait(Duration::from_millis(5)), Next::Idle);
        let producer = outbox.clone();
        let thread = std::thread::spawn(move || producer.push(plain("late")));
        assert_eq!(outbox.wait(Duration::from_secs(5)), Next::Message("late".to_string()));
        thread.join().unwrap();
        outbox.push(plain("last"));
        outbox.close();
        outbox.push(plain("ignored"));
        assert_eq!(outbox.wait(Duration::from_millis(5)), Next::Message("last".to_string()));
        assert_eq!(outbox.wait(Duration::from_millis(5)), Next::Closed);
    }
}
