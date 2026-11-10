//! `--serve`: the interface over HTTP and the bridge over a WebSocket, on 127.0.0.1 only.
//!
//! - `GET /…` serves the interface's files (the `--ui` folder). They are the same files for
//!   everyone and need no token.
//! - `GET /bridge?token=<token>` upgrades to the WebSocket that carries the bridge protocol, one
//!   JSON text per message. The token is random per run and printed once, inside the URL to open.
//!
//! What protects the bridge: the listener is bound to the loopback address; the token (compared in
//! constant time); the `Host` header must name this server (a page on another site that resolves
//! its own name to 127.0.0.1 is refused); and the `Origin` of the WebSocket request must be this
//! server (another site's script cannot open the socket even if it learned the token).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use st_host::{Bridge, Next, Outbox};

use crate::websocket::{accept_key, Received, Receiver, Sender};

/// Connections handled at once; a browser loading the interface uses six.
const MAX_CONNECTIONS: usize = 64;
const MAX_HEAD_BYTES: usize = 16 * 1024;
/// An idle keep-alive connection is closed after this long.
const IDLE: Duration = Duration::from_secs(15);
/// A WebSocket that has nothing to send is pinged this often, which also notices dead peers.
const PING_EVERY: Duration = Duration::from_secs(20);

/// The running server.
pub struct Server {
    address: SocketAddr,
    token: String,
    shared: Arc<Shared>,
}

struct Shared {
    root: PathBuf,
    token: String,
    port: u16,
    bridge: Arc<Bridge>,
    stopping: AtomicBool,
    connections: AtomicUsize,
    /// Open WebSockets, to close them at shutdown.
    sockets: Mutex<Vec<(Arc<Outbox>, TcpStream)>>,
}

impl Server {
    /// Binds 127.0.0.1:`port` (0 picks a free port) and starts accepting.
    pub fn start(bridge: Arc<Bridge>, root: PathBuf, port: u16) -> Result<Server, String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| format!("Could not listen on 127.0.0.1:{port}: {error}."))?;
        let address = listener.local_addr().map_err(|error| format!("Could not read the listening address: {error}."))?;
        let token = random_token()?;
        let shared = Arc::new(Shared {
            root,
            token: token.clone(),
            port: address.port(),
            bridge,
            stopping: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
            sockets: Mutex::new(Vec::new()),
        });
        let accepting = shared.clone();
        std::thread::Builder::new()
            .name("st-app http".to_string())
            .spawn(move || accept(listener, accepting))
            .map_err(|error| format!("Could not start the server thread: {error}."))?;
        Ok(Server { address, token, shared })
    }

    /// The URL that opens the interface connected to this host.
    pub fn url(&self) -> String {
        format!("http://{}/?token={}", self.address, self.token)
    }

    /// Stops accepting, lets each WebSocket send what is queued, and closes them.
    pub fn stop(&self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Wake the accept loop.
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(200));
        let sockets: Vec<(Arc<Outbox>, TcpStream)> = self.shared.sockets.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).drain(..).collect();
        for (outbox, _) in &sockets {
            outbox.close();
        }
        // The writer threads drain their outboxes and send the close frame; give them a moment.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while sockets.iter().any(|(outbox, _)| !outbox.is_empty()) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(20));
        for (_, stream) in sockets {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| format!("The system gave no random numbers for the access token: {error}."))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn accept(listener: TcpListener, shared: Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.stopping.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
            let _ = respond(&stream, 503, "text/plain; charset=utf-8", b"Too many connections.", false, true);
            continue;
        }
        let connection = shared.clone();
        let spawned = std::thread::Builder::new().name("st-app connection".to_string()).spawn(move || {
            let _ = serve_connection(stream, &connection);
            connection.connections.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

struct Head {
    method: String,
    /// The path, percent-encoded as sent, without the query.
    path: String,
    query: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(header, _)| header.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }

    fn query_value(&self, name: &str) -> Option<&str> {
        self.query.split('&').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
    }
}

/// Reads one request head. `Ok(None)` when the peer closed the connection between requests.
fn read_head(reader: &mut BufReader<TcpStream>) -> io::Result<Option<Head>> {
    let mut lines: Vec<String> = Vec::new();
    let mut total = 0;
    loop {
        let mut line = String::new();
        let read = reader.by_ref().take((MAX_HEAD_BYTES - total + 1) as u64).read_line(&mut line)?;
        total += read;
        if read == 0 {
            return if lines.is_empty() { Ok(None) } else { Err(io::ErrorKind::UnexpectedEof.into()) };
        }
        if total > MAX_HEAD_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "request head too large"));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if lines.is_empty() {
                continue; // Stray blank line before a request.
            }
            break;
        }
        lines.push(line.to_string());
    }
    let malformed = || io::Error::new(io::ErrorKind::InvalidData, "malformed request");
    let mut request = lines[0].split(' ');
    let (method, target) = (request.next().ok_or_else(malformed)?, request.next().ok_or_else(malformed)?);
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers = lines[1..]
        .iter()
        .filter_map(|line| line.split_once(':').map(|(name, value)| (name.trim().to_string(), value.trim().to_string())))
        .collect();
    Ok(Some(Head { method: method.to_string(), path: path.to_string(), query: query.to_string(), headers }))
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        426 => "Upgrade Required",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn respond(mut stream: &TcpStream, status: u16, content_type: &str, body: &[u8], head_only: bool, close: bool) -> io::Result<()> {
    let mut response = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n",
        reason(status),
        body.len()
    );
    if close {
        response.push_str("Connection: close\r\n");
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes())?;
    if !head_only {
        stream.write_all(body)?;
    }
    stream.flush()
}

fn refuse(stream: &TcpStream, status: u16, message: &str) -> io::Result<()> {
    respond(stream, status, "text/plain; charset=utf-8", message.as_bytes(), false, true)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && left.iter().zip(right).fold(0u8, |difference, (a, b)| difference | (a ^ b)) == 0
}

impl Shared {
    /// The names a browser on this machine may use for this server.
    fn is_own_authority(&self, authority: &str) -> bool {
        [format!("127.0.0.1:{}", self.port), format!("localhost:{}", self.port)].iter().any(|own| own.eq_ignore_ascii_case(authority))
    }
}

fn serve_connection(stream: TcpStream, shared: &Arc<Shared>) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE))?;
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    while let Some(head) = read_head(&mut reader)? {
        if !head.header("Host").is_some_and(|host| shared.is_own_authority(host)) {
            return refuse(&stream, 403, "This server only answers to its own address on this computer.");
        }
        if head.path == "/bridge" {
            return serve_bridge(stream, reader, &head, shared);
        }
        if head.method != "GET" && head.method != "HEAD" {
            return refuse(&stream, 405, "Only GET is supported.");
        }
        let head_only = head.method == "HEAD";
        match resolve(&shared.root, &head.path).and_then(|file| std::fs::read(&file).ok().map(|body| (file, body))) {
            Some((file, body)) => respond(&stream, 200, content_type(&file), &body, head_only, false)?,
            None => respond(&stream, 404, "text/plain; charset=utf-8", b"Not found.", head_only, false)?,
        }
    }
    Ok(())
}

/// The file a request path names inside `root`, if the path is one this server is willing to read.
pub fn resolve(root: &Path, request_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(request_path)?;
    let relative = decoded.strip_prefix('/')?;
    let relative = if relative.is_empty() { "index.html" } else { relative };
    if relative.contains('\\') || relative.contains('\0') {
        return None;
    }
    let mut file = root.to_path_buf();
    for part in Path::new(relative).components() {
        match part {
            Component::Normal(name) if !name.to_string_lossy().starts_with('.') => file.push(name),
            _ => return None, // "..", absolute parts, hidden files
        }
    }
    // A symbolic link inside the folder must not lead out of it.
    let (file, root) = (file.canonicalize().ok()?, root.canonicalize().ok()?);
    (file.starts_with(&root) && file.is_file()).then_some(file)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

pub fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|extension| extension.to_str()).unwrap_or_default().to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "wav" => "audio/wav",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn serve_bridge(stream: TcpStream, reader: BufReader<TcpStream>, head: &Head, shared: &Arc<Shared>) -> io::Result<()> {
    let upgrade = head.header("Upgrade").is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let Some(key) = head.header("Sec-WebSocket-Key").filter(|_| upgrade && head.method == "GET") else {
        return refuse(&stream, 426, "This address is the bridge's WebSocket.");
    };
    if !head.query_value("token").is_some_and(|token| constant_time_eq(token.as_bytes(), shared.token.as_bytes())) {
        return refuse(&stream, 403, "The access token is missing or wrong. Open the URL the host printed when it started.");
    }
    let own_origin = head.header("Origin").and_then(|origin| origin.strip_prefix("http://")).is_some_and(|authority| shared.is_own_authority(authority));
    if !own_origin {
        return refuse(&stream, 403, "The bridge only accepts the interface this host serves.");
    }
    let accept = accept_key(key);
    (&stream).write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes())?;
    // From here on the socket waits as long as the interface is quiet.
    stream.set_read_timeout(None)?;

    let sender = Sender::new(stream.try_clone()?);
    let outbox = Outbox::new();
    let connection = shared.bridge.connect(outbox.clone());
    shared.sockets.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((outbox.clone(), stream.try_clone()?));

    let writer = {
        let (outbox, sender, stream) = (outbox.clone(), sender.clone(), stream.try_clone()?);
        std::thread::Builder::new().name("st-app bridge writer".to_string()).spawn(move || {
            loop {
                let sent = match outbox.wait(PING_EVERY) {
                    Next::Message(text) => sender.text(&text),
                    Next::Idle => sender.ping(),
                    Next::Closed => {
                        let _ = sender.close(1001);
                        break;
                    }
                };
                if sent.is_err() {
                    // The peer is gone: stop the reader too.
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    break;
                }
            }
        })?
    };

    let mut receiver = Receiver::new(reader, sender);
    while let Ok(Received::Text(text)) = receiver.next() {
        connection.receive(&text);
    }
    connection.close();
    outbox.close();
    let _ = writer.join();
    let _ = stream.shutdown(std::net::Shutdown::Both);
    shared.sockets.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|(open, _)| !Arc::ptr_eq(open, &outbox));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_host::testing::TempDir;

    #[test]
    fn request_paths_resolve_inside_the_folder_only() {
        let folder = TempDir::new("serve");
        let root = folder.path().join("ui");
        std::fs::create_dir_all(root.join("images")).unwrap();
        std::fs::write(root.join("index.html"), "<html>").unwrap();
        std::fs::write(root.join("app bridge.js"), "//").unwrap();
        std::fs::write(root.join("images/logo.svg"), "<svg/>").unwrap();
        std::fs::write(root.join(".secret"), "no").unwrap();
        std::fs::write(folder.path().join("outside.txt"), "no").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(folder.path().join("outside.txt"), root.join("link.txt")).unwrap();

        let found = |path: &str| resolve(&root, path).map(|file| file.strip_prefix(root.canonicalize().unwrap()).unwrap().to_string_lossy().into_owned());
        assert_eq!(found("/").as_deref(), Some("index.html"));
        assert_eq!(found("/index.html").as_deref(), Some("index.html"));
        assert_eq!(found("/app%20bridge.js").as_deref(), Some("app bridge.js"));
        assert_eq!(found("/images/logo.svg").as_deref(), Some("images/logo.svg"));
        for refused in ["/../outside.txt", "/%2e%2e/outside.txt", "/images/../../outside.txt", "//etc/passwd", "/.secret", "/missing.js", "/images", "/link.txt", "/a%zz", "/images\\logo.svg", "index.html"] {
            assert_eq!(found(refused), None, "{refused}");
        }
    }

    #[test]
    fn content_types_cover_what_the_interface_loads() {
        assert_eq!(content_type(Path::new("index.html")), "text/html; charset=utf-8");
        assert_eq!(content_type(Path::new("APP.JS")), "text/javascript; charset=utf-8");
        assert_eq!(content_type(Path::new("styles.css")), "text/css; charset=utf-8");
        assert_eq!(content_type(Path::new("unknown.bin")), "application/octet-stream");
    }

    #[test]
    fn tokens_are_random_hex_and_compared_whole() {
        let (first, second) = (random_token().unwrap(), random_token().unwrap());
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|character| character.is_ascii_hexdigit()));
        assert_ne!(first, second);
        assert!(constant_time_eq(first.as_bytes(), first.as_bytes()));
        assert!(!constant_time_eq(first.as_bytes(), second.as_bytes()));
        assert!(!constant_time_eq(first.as_bytes(), &first.as_bytes()[..31]));
    }
}
