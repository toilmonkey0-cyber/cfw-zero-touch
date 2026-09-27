//! Serve-sidecar lifecycle (PR 6): `needle.exe --serve` process owned by
//! the app, spawned on first doctor use and killed on idle or app exit.
//!
//! Phases, each cheap to verify: a free port is picked by binding
//! `127.0.0.1:0` in Rust, then the sidecar is spawned with `--port` (the
//! flag semantics are verified against the real binary on first smoke);
//! health is one `POST /reset` round-trip; telemetry env vars are set on
//! the child. Two processes serve two features: `cfw-embed` (Phase 1)
//! and the serve sidecar (Phase 2+) are never both resident.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::client::{ClientError, ServeClient, ServeTurn};

/// Idle sidecars are reaped after this long (~81 MB RSS goes back).
pub const SERVE_IDLE_KILL_AFTER: Duration = Duration::from_secs(5 * 60);

/// Picks a free loopback port by binding port 0 and reading the result
/// back. The close-then-spawn race is accepted; callers retry once on a
/// bind failure (harmless on localhost).
pub fn pick_free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("could not pick a port: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("could not read the picked port: {error}"))?
        .port();
    drop(listener);
    Ok(port)
}

struct ManagedServe {
    child: Child,
    port: u16,
    engine: PathBuf,
    weights: PathBuf,
    last_use: Instant,
}

static SERVE: Mutex<Option<ManagedServe>> = Mutex::new(None);

fn spawn_sidecar(engine: &Path, weights: &Path, port: u16) -> Result<Child, String> {
    let mut command = Command::new(engine);
    command
        .args(["--model"])
        .arg(weights)
        .args(["--tools"])
        .arg(serve_tools_path())
        .args(["--serve", "--port"])
        .arg(port.to_string())
        .env("NEEDLE_TELEMETRY", "0")
        .env("DO_NOT_TRACK", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes beside the app.
        command.creation_flags(0x0800_0000);
    }
    command
        .spawn()
        .map_err(|error| format!("could not start the serve sidecar: {error}"))
}

/// Health is one reset round-trip within the client timeouts.
fn health_check(client: &ServeClient) -> Result<(), String> {
    client
        .health_reset()
        .map_err(|error| format!("serve sidecar unhealthy: {error}"))
}

/// Tool schema for the sidecar, embedded from the repo so the shipped
/// app always pairs code and schema.
const SERVE_TOOLS: &str = include_str!("tools/card_doctor.json");

/// Writes the tool schema next to the sidecar invocation. A fresh temp
/// file per spawn keeps concurrent app installs from racing one path.
fn serve_tools_path() -> PathBuf {
    let name = format!(
        "card-doctor-tools-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let path = std::env::temp_dir().join(name);
    // Best effort: spawn fails honestly below if this write did not land.
    let _ = std::fs::write(&path, SERVE_TOOLS);
    path
}

/// Ensures a live sidecar for `(engine, weights)`, reusing a warm,
/// fresh one keyed on both paths or spawning + health-checking a
/// replacement. A stale executable serves nothing.
fn ensure(engine: &Path, weights: &Path) -> Result<u16, String> {
    {
        let mut guard = SERVE.lock().map_err(|_| "serve lock poisoned")?;
        if let Some(managed) = guard.as_mut() {
            let paths_match = managed.engine == engine && managed.weights == weights;
            if paths_match && managed.last_use.elapsed() <= SERVE_IDLE_KILL_AFTER {
                managed.last_use = Instant::now();
                return Ok(managed.port);
            }
            let _ = managed.child.kill();
            let _ = managed.child.wait();
            *guard = None;
        }
    }

    let port = pick_free_port()?;
    let mut child = spawn_sidecar(engine, weights, port)?;
    let client = ServeClient::new(port);
    match health_check(&client) {
        Ok(()) => {
            let mut guard = SERVE.lock().map_err(|_| "serve lock poisoned")?;
            *guard = Some(ManagedServe {
                child,
                port,
                engine: engine.to_path_buf(),
                weights: weights.to_path_buf(),
                last_use: Instant::now(),
            });
            Ok(port)
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

/// Asks through a warm sidecar (spawning one when needed).
/// The caller binds the pinned engine + verified weights paths.
pub fn ask_through_sidecar(
    engine: &Path,
    weights: &Path,
    input: &str,
) -> Result<ServeTurn, ClientError> {
    let port = ensure(engine, weights).map_err(ClientError::Http)?;
    let client = ServeClient::new(port);
    let turn = client.ask(input)?;
    if let Ok(mut guard) = SERVE.lock() {
        if let Some(managed) = guard.as_mut() {
            if managed.port == port {
                managed.last_use = Instant::now();
            }
        }
    }
    Ok(turn)
}

/// Kills the shared sidecar, if any. Wired to the app exit event.
pub fn kill_all() {
    if let Ok(mut guard) = SERVE.lock() {
        if let Some(mut managed) = guard.take() {
            let _ = managed.child.kill();
            let _ = managed.child.wait();
        }
    }
}
