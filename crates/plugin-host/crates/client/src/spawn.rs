//! Starting a container process and performing the handshake with it. Shared by the
//! [`crate::Supervisor`] (containers that host instances) and the [`crate::Scanner`]
//! (sacrificial containers that scan plug-in files).

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use stpc_transport::channel::{FrameKind, Listener, Stream};
use stpc_transport::messages::{Hello, Welcome};
use stpc_transport::wire::Wire;
use stpc_transport::{monotonic_ns, ARG_ENDPOINT, ARG_TOKEN, HELLO_MAGIC, PROTOCOL_VERSION};

use crate::{Error, Result, CONTAINER_BINARY_NAME, ENV_CONTAINER_PATH};

/// The container executable: `explicit`, else `$STPC_CONTAINER_PATH`, else next to the
/// current executable.
pub(crate) fn resolve_container_path(explicit: Option<&PathBuf>) -> Result<PathBuf> {
    let path = match explicit {
        Some(path) => path.clone(),
        None => match std::env::var_os(ENV_CONTAINER_PATH) {
            Some(path) => PathBuf::from(path),
            None => std::env::current_exe()
                .map_err(Error::Io)?
                .parent()
                .map(|dir| dir.join(CONTAINER_BINARY_NAME))
                .ok_or_else(|| Error::Invalid("the current executable has no directory".into()))?,
        },
    };
    if !path.is_file() {
        return Err(Error::Spawn(format!("{} does not exist", path.display())));
    }
    Ok(path)
}

/// Creates this process's private socket directory (`stpc-<pid>-<random>`, mode 0700)
/// under `socket_dir`, `$XDG_RUNTIME_DIR` or the temporary directory, after removing the
/// directories of processes that no longer exist.
pub(crate) fn create_private_dir(socket_dir: Option<&PathBuf>) -> Result<PathBuf> {
    let base = socket_dir
        .cloned()
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()))
        .unwrap_or_else(std::env::temp_dir);
    sweep_stale_dirs(&base);
    let dir = base.join(format!("stpc-{}-{}", std::process::id(), random_hex(4)));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(Error::Io)?;
    // sockaddr_un holds about 104 bytes on macOS: fail early with a usable message.
    if dir.as_os_str().len() + 24 > 100 {
        let _ = std::fs::remove_dir(&dir);
        return Err(Error::Invalid(format!("socket directory path too long for a Unix socket: {}", dir.display())));
    }
    Ok(dir)
}

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut raw)).is_ok();
    if !ok {
        let seed = monotonic_ns() ^ ((std::process::id() as u64) << 32);
        for (i, byte) in raw.iter_mut().enumerate() {
            *byte = (seed.rotate_left(i as u32 * 7) ^ (i as u64 * 0x9E)) as u8;
        }
    }
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

/// Removes socket directories left behind by processes that no longer exist.
fn sweep_stale_dirs(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix("stpc-")) else { continue };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<i32>().ok()) else { continue };
        if pid <= 0 || pid as u32 == std::process::id() {
            continue;
        }
        // SAFETY: signal 0 only checks for the existence of the process.
        let gone =
            unsafe { libc::kill(pid, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

pub(crate) struct Launch<'a> {
    pub container_path: &'a Path,
    pub args: &'a [OsString],
    pub env: &'a [(OsString, OsString)],
    /// The private socket directory.
    pub dir: &'a Path,
    /// File name of the socket inside `dir`; unique per launch.
    pub socket: &'a str,
    pub timeout: Duration,
}

/// Starts a container and completes the handshake: binds a fresh socket, spawns the
/// process with the endpoint and a random token, accepts its one connection (the socket
/// path is removed at once), checks its `Hello` and answers. On any failure the process
/// is killed and reaped.
pub(crate) fn launch(launch: &Launch<'_>) -> Result<(Child, Stream, Hello)> {
    let listener = Listener::bind(launch.dir, launch.socket).map_err(|e| Error::Spawn(format!("socket: {e}")))?;
    let token = random_hex(16);
    let mut command = Command::new(launch.container_path);
    command
        .args(launch.args)
        .arg(ARG_ENDPOINT)
        .arg(listener.endpoint())
        .arg(ARG_TOKEN)
        .arg(&token)
        .envs(launch.env.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str())))
        .stdin(Stdio::null());
    let mut child = command.spawn().map_err(|e| Error::Spawn(format!("{}: {e}", launch.container_path.display())))?;
    let reject = |child: &mut Child, error: Error| -> Error {
        let _ = child.kill();
        let _ = child.wait();
        error
    };
    let stream = match listener.accept(launch.timeout, || matches!(child.try_wait(), Ok(None))) {
        Ok(stream) => stream,
        Err(err) => return Err(reject(&mut child, Error::Spawn(err.to_string()))),
    };
    drop(listener); // removes the socket path: nobody else can connect
    let hello = stream
        .set_read_timeout(Some(launch.timeout))
        .and_then(|_| stream.recv())
        .map_err(|e| Error::Spawn(format!("handshake: {e}")))
        .and_then(|frame| {
            if frame.kind != FrameKind::Handshake {
                return Err(Error::Protocol("expected a handshake frame".into()));
            }
            Hello::decode(&frame.payload).map_err(|e| Error::Protocol(e.to_string()))
        });
    let hello = match hello {
        Ok(hello) => hello,
        Err(err) => return Err(reject(&mut child, err)),
    };
    let refusal = if hello.magic != HELLO_MAGIC {
        Some("not a plug-in container".to_string())
    } else if hello.protocol_version != PROTOCOL_VERSION {
        Some(format!(
            "protocol version mismatch: the container speaks {}, this host {}",
            hello.protocol_version, PROTOCOL_VERSION
        ))
    } else if hello.token != token {
        Some("wrong connection token".to_string())
    } else {
        None
    };
    let welcome = Welcome {
        accepted: refusal.is_none(),
        protocol_version: PROTOCOL_VERSION,
        reason: refusal.clone().unwrap_or_default(),
    };
    let sent = stream.send(FrameKind::Handshake, 0, &welcome.encode(), &[]);
    if let Some(reason) = refusal {
        return Err(reject(&mut child, Error::Rejected(reason)));
    }
    if let Err(err) = sent.and_then(|_| stream.set_read_timeout(None)) {
        return Err(reject(&mut child, Error::Io(err)));
    }
    Ok((child, stream, hello))
}
