//! The command table: every command the interface can send is one registration here.
//!
//! See the crate documentation for the model and an example.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::marker::PhantomData;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::bridge::RawReply;

/// Where a command's handler runs when the interface sends the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// On the bridge thread, one command at a time, in the order the interface sent them.
    Ordered,
    /// On a worker thread of its own, concurrently with everything else.
    Blocking,
}

pub(crate) type Run = dyn Fn(Value, RawReply) + Send + Sync;

pub(crate) struct Entry {
    pub lane: Lane,
    pub run: Arc<Run>,
}

/// The table of commands, filled before the bridge starts.
///
/// | Method | Handler | Runs |
/// |---|---|---|
/// | [`register`](Commands::register) | `Fn(P) -> Result<R, E>` | in arrival order on the bridge thread; must return quickly |
/// | [`register_blocking`](Commands::register_blocking) | `Fn(P) -> Result<R, E>` | on a worker thread; may take as long as it needs |
/// | [`register_deferred`](Commands::register_deferred) | `Fn(P, Reply<R>)` | in arrival order; answers later, from any thread |
///
/// `P` is the payload (`serde::Deserialize`; use [`NoPayload`] when there is none, `serde_json::Value`
/// to take it as sent), `R` the result (`serde::Serialize`), and `E` anything that implements
/// `Display` with a message the user can read ([`crate::Error`], another crate's error, a `String`).
///
/// [`Commands::with`] registers methods of a shared service without writing closures.
#[derive(Default)]
pub struct Commands {
    entries: BTreeMap<String, Entry>,
}

impl Commands {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a quick command. Quick commands run one at a time in the order the interface sent
    /// them (a `note.on` is handled before the `note.off` that follows it), so a handler that waits
    /// holds up every other quick command: anything that can take long belongs in
    /// [`register_blocking`](Commands::register_blocking).
    ///
    /// # Panics
    /// If `name` is already registered. Registration happens at startup, where this is a programming
    /// error that should not survive the first run.
    pub fn register<P, R, E>(&mut self, name: &str, handler: impl Fn(P) -> Result<R, E> + Send + Sync + 'static)
    where
        P: DeserializeOwned,
        R: Serialize,
        E: Display,
    {
        self.insert(name, Lane::Ordered, answering(name, handler));
    }

    /// Registers a command that may take long (a file dialog, an export, a scan, loading a file).
    /// Each request runs on a worker thread, concurrently with other commands and with other
    /// requests for the same command; the reply is sent when the handler returns.
    ///
    /// # Panics
    /// If `name` is already registered.
    pub fn register_blocking<P, R, E>(&mut self, name: &str, handler: impl Fn(P) -> Result<R, E> + Send + Sync + 'static)
    where
        P: DeserializeOwned,
        R: Serialize,
        E: Display,
    {
        self.insert(name, Lane::Blocking, answering(name, handler));
    }

    /// Registers a command whose answer comes later from somewhere else: the handler runs in arrival
    /// order like a quick command, hands the [`Reply`] to whoever will finish the work (the window's
    /// event loop, an export thread) and returns. A `Reply` that is dropped unanswered still answers,
    /// with an error.
    ///
    /// # Panics
    /// If `name` is already registered.
    pub fn register_deferred<P, R>(&mut self, name: &str, handler: impl Fn(P, Reply<R>) + Send + Sync + 'static)
    where
        P: DeserializeOwned,
        R: Serialize,
    {
        let command = name.to_string();
        self.insert(
            name,
            Lane::Ordered,
            Arc::new(move |payload, raw| match decode::<P>(&command, payload) {
                Ok(payload) => handler(payload, Reply::new(raw)),
                Err(message) => raw.error(message),
            }),
        );
    }

    /// Registers methods of `state` as commands: `Fn(&S, P) -> Result<R, E>`.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use st_host::{Commands, NoPayload, Result};
    /// struct Counter(std::sync::atomic::AtomicU64);
    /// impl Counter {
    ///     fn next(&self, _: NoPayload) -> Result<u64> {
    ///         Ok(self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
    ///     }
    /// }
    /// let mut commands = Commands::new();
    /// let counter = Arc::new(Counter(Default::default()));
    /// commands.with(&counter).register("counter.next", Counter::next);
    /// ```
    pub fn with<S: Send + Sync + 'static>(&mut self, state: &Arc<S>) -> Scoped<'_, S> {
        Scoped { commands: self, state: state.clone() }
    }

    /// True when `name` is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// The registered names, sorted.
    pub fn names(&self) -> Vec<&str> {
        self.entries.keys().map(String::as_str).collect()
    }

    /// The lane `name` was registered for.
    pub fn lane(&self, name: &str) -> Option<Lane> {
        self.entries.get(name).map(|entry| entry.lane)
    }

    fn insert(&mut self, name: &str, lane: Lane, run: Arc<Run>) {
        let previous = self.entries.insert(name.to_string(), Entry { lane, run });
        assert!(previous.is_none(), "the command `{name}` is registered twice");
    }

    pub(crate) fn into_entries(self) -> BTreeMap<String, Entry> {
        self.entries
    }
}

/// [`Commands`] with a service attached: handlers are the service's methods. See [`Commands::with`].
pub struct Scoped<'a, S> {
    commands: &'a mut Commands,
    state: Arc<S>,
}

impl<S: Send + Sync + 'static> Scoped<'_, S> {
    /// Like [`Commands::register`], for `Fn(&S, P) -> Result<R, E>`.
    pub fn register<P, R, E>(&mut self, name: &str, handler: impl Fn(&S, P) -> Result<R, E> + Send + Sync + 'static) -> &mut Self
    where
        P: DeserializeOwned,
        R: Serialize,
        E: Display,
    {
        let state = self.state.clone();
        self.commands.register(name, move |payload: P| handler(&state, payload));
        self
    }

    /// Like [`Commands::register_blocking`], for `Fn(&S, P) -> Result<R, E>`.
    pub fn register_blocking<P, R, E>(&mut self, name: &str, handler: impl Fn(&S, P) -> Result<R, E> + Send + Sync + 'static) -> &mut Self
    where
        P: DeserializeOwned,
        R: Serialize,
        E: Display,
    {
        let state = self.state.clone();
        self.commands.register_blocking(name, move |payload: P| handler(&state, payload));
        self
    }

    /// Like [`Commands::register_deferred`], for `Fn(&S, P, Reply<R>)`.
    pub fn register_deferred<P, R>(&mut self, name: &str, handler: impl Fn(&S, P, Reply<R>) + Send + Sync + 'static) -> &mut Self
    where
        P: DeserializeOwned,
        R: Serialize,
    {
        let state = self.state.clone();
        self.commands.register_deferred(name, move |payload: P, reply: Reply<R>| handler(&state, payload, reply));
        self
    }
}

/// The payload of a command that takes none. Whatever the interface sent is ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoPayload;

impl<'de> serde::Deserialize<'de> for NoPayload {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer).map(|_| NoPayload)
    }
}

/// The pending answer to one request, for handlers registered with
/// [`Commands::register_deferred`]. It can move to any thread. Answering consumes it; dropping it
/// unanswered sends an error reply, so the interface never waits forever.
pub struct Reply<R> {
    raw: RawReply,
    result: PhantomData<fn(R)>,
}

impl<R: Serialize> Reply<R> {
    fn new(raw: RawReply) -> Self {
        Reply { raw, result: PhantomData }
    }

    /// The command being answered.
    pub fn command(&self) -> &str {
        self.raw.command()
    }

    /// Answers with a result.
    pub fn ok(self, result: R) {
        let encoded = encode(self.raw.command(), &result);
        self.raw.send(encoded);
    }

    /// Answers with an error message.
    pub fn error(self, message: impl Display) {
        self.raw.error(message);
    }

    /// Answers with whichever the `Result` holds.
    pub fn send<E: Display>(self, result: Result<R, E>) {
        match result {
            Ok(result) => self.ok(result),
            Err(error) => self.error(error),
        }
    }
}

fn answering<P, R, E>(name: &str, handler: impl Fn(P) -> Result<R, E> + Send + Sync + 'static) -> Arc<Run>
where
    P: DeserializeOwned,
    R: Serialize,
    E: Display,
{
    let command = name.to_string();
    Arc::new(move |payload, raw: RawReply| {
        let outcome = match decode::<P>(&command, payload) {
            Ok(payload) => match handler(payload) {
                Ok(result) => encode(&command, &result),
                Err(error) => Err(error.to_string()),
            },
            Err(message) => Err(message),
        };
        raw.send(outcome);
    })
}

fn decode<P: DeserializeOwned>(command: &str, payload: Value) -> Result<P, String> {
    // Unknown fields are ignored and missing optional fields defaulted by the payload type itself
    // (`#[serde(default)]`); what remains is a request the handler cannot understand.
    serde_json::from_value(payload).map_err(|error| format!("`{command}` was sent with an unusable payload: {error}."))
}

fn encode<R: Serialize>(command: &str, result: &R) -> Result<Value, String> {
    serde_json::to_value(result).map_err(|error| format!("The result of `{command}` could not be sent to the interface: {error}."))
}
