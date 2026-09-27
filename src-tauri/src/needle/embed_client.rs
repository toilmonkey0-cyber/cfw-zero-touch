//! Client for the `cfw-embed` helper (tools/cfw-embed/cfw-embed.cpp).
//!
//! The helper is a small llvm-mingw clang++ binary that statically links
//! the Needle engine (the shipped `libneedle.a` is clang/libc++ and does
//! not link under MSVC — see the design doc's PR 2a amendment). This
//! client owns its whole lifecycle:
//!
//! - spawn: direct binary, unelevated, no shell; `NEEDLE_TELEMETRY=0` and
//!   `DO_NOT_TRACK=1` are set explicitly (defense in depth — the helper
//!   also clears them itself before the engine loads);
//! - protocol: line-based stdio — `PING` → `PONG <dim>`,
//!   `EMBED <len>\n<bytes>\n` → `OK <n>\n<float csv>`. Length-prefixed
//!   payloads avoid escaping rules entirely;
//! - lifecycle: one warm process, killed after 5 minutes idle (the ~81 MB
//!   RSS goes back) and respawned on the next request; `kill_all` is wired
//!   to the app's exit event so no orphan survives Card Studio.
//!
//! Stems are cleaned file-name fragments, never file contents, and are
//! validated here: non-empty, bounded, no control characters.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Hard cap on a stem payload sent to the helper.
pub const MAX_STEM_BYTES: usize = 4096;

/// A warm helper is killed after this long without a request.
pub const IDLE_KILL_AFTER: Duration = Duration::from_secs(5 * 60);

/// Validates a stem before it is written to the helper.
pub fn validate_stem(stem: &str) -> Result<(), String> {
    if stem.is_empty() {
        return Err("stem must not be empty".into());
    }
    if stem.len() > MAX_STEM_BYTES {
        return Err("stem exceeds the length cap".into());
    }
    if stem.bytes().any(|b| b.is_ascii_control()) {
        return Err("stem must not contain control characters".into());
    }
    Ok(())
}

/// Parses one CSV float line from the helper (`%.9g` values round-trip).
pub fn parse_floats(line: &str) -> Result<Vec<f32>, String> {
    line.split_whitespace()
        .map(|token| {
            token
                .parse::<f32>()
                .map_err(|error| format!("bad float {token:?}: {error}"))
        })
        .collect()
}

/// One live helper process.
pub struct EmbedHelper {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    dim: usize,
    last_use: Instant,
}

impl EmbedHelper {
    /// Spawns the helper with the given weights file. The caller is
    /// responsible for providing a verified weights path (PR 1 acquire or
    /// a developer-supplied file).
    pub fn spawn(helper_exe: &Path, weights: &Path) -> Result<Self, String> {
        let mut command = Command::new(helper_exe);
        command
            .arg("--weights")
            .arg(weights)
            .env("NEEDLE_TELEMETRY", "0")
            .env("DO_NOT_TRACK", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: never flash a console next to the app.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not start cfw-embed: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "cfw-embed stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "cfw-embed stdout unavailable".to_string())?;
        let mut helper = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            dim: 0,
            last_use: Instant::now(),
        };
        let dim = helper.ping()?;
        helper.dim = dim;
        Ok(helper)
    }

    /// The embedding dimension reported by the loaded weights.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Seconds since the last request.
    pub fn idle_for(&self) -> Duration {
        self.last_use.elapsed()
    }

    fn read_line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        let read = self
            .stdout
            .read_line(&mut line)
            .map_err(|error| format!("could not read from cfw-embed: {error}"))?;
        if read == 0 {
            return Err("cfw-embed exited".into());
        }
        self.last_use = Instant::now();
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.stdin
            .write_all(bytes)
            .and_then(|_| self.stdin.flush())
            .map_err(|error| format!("could not write to cfw-embed: {error}"))
    }

    /// Round-trips one `PING`; returns the embedding dimension.
    pub fn ping(&mut self) -> Result<usize, String> {
        self.write_all(b"PING\n")?;
        let line = self.read_line()?;
        let dim = line
            .strip_prefix("PONG ")
            .and_then(|rest| rest.trim().parse::<usize>().ok())
            .ok_or_else(|| format!("unexpected cfw-embed reply: {line:?}"))?;
        Ok(dim)
    }

    /// Embeds one stem (deterministic: a pure function of text + weights).
    pub fn embed(&mut self, stem: &str) -> Result<Vec<f32>, String> {
        validate_stem(stem)?;
        let payload = stem.as_bytes();
        let request = format!("EMBED {}\n", payload.len());
        self.write_all(request.as_bytes())?;
        self.write_all(payload)?;
        self.write_all(b"\n")?;
        let status = self.read_line()?;
        if let Some(error) = status.strip_prefix("ERR ") {
            return Err(error.to_string());
        }
        let wrote = status
            .strip_prefix("OK ")
            .and_then(|rest| rest.trim().parse::<usize>().ok())
            .ok_or_else(|| format!("unexpected cfw-embed reply: {status:?}"))?;
        let floats_line = self.read_line()?;
        let vector = parse_floats(&floats_line)?;
        if vector.len() != wrote {
            return Err(format!(
                "cfw-embed sent {wrote} floats but {} values",
                vector.len()
            ));
        }
        Ok(vector)
    }

    /// Embeds many stems over the same warm process.
    pub fn embed_many(&mut self, stems: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        stems.iter().map(|stem| self.embed(stem)).collect()
    }

    /// Kills the helper process (idempotent).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for EmbedHelper {
    fn drop(&mut self) {
        self.kill();
    }
}

struct ManagedHelper {
    helper: EmbedHelper,
    exe: PathBuf,
    weights: PathBuf,
}

static HELPER: Mutex<Option<ManagedHelper>> = Mutex::new(None);

/// Runs one interaction against the shared helper, reusing a warm process
/// when the paths match and it has not idled out; kills and respawns
/// otherwise.
pub fn with_helper<R>(
    helper_exe: &Path,
    weights: &Path,
    interaction: impl FnOnce(&mut EmbedHelper) -> Result<R, String>,
) -> Result<R, String> {
    let mut guard = HELPER.lock().map_err(|_| "embed helper lock poisoned")?;
    if let Some(managed) = guard.as_mut() {
        let fresh = managed.exe != helper_exe
            || managed.weights != weights
            || managed.helper.idle_for() > IDLE_KILL_AFTER;
        if fresh {
            managed.helper.kill();
            *managed = ManagedHelper {
                helper: EmbedHelper::spawn(helper_exe, weights)?,
                exe: helper_exe.to_path_buf(),
                weights: weights.to_path_buf(),
            };
        }
    } else {
        *guard = Some(ManagedHelper {
            helper: EmbedHelper::spawn(helper_exe, weights)?,
            exe: helper_exe.to_path_buf(),
            weights: weights.to_path_buf(),
        });
    }
    let managed = guard.as_mut().expect("helper was just ensured");
    interaction(&mut managed.helper)
}

/// Kills the shared helper, if any. Wired to the app exit event so no
/// orphan survives Card Studio.
pub fn kill_all() {
    if let Ok(mut guard) = HELPER.lock() {
        if let Some(mut managed) = guard.take() {
            managed.helper.kill();
        }
    }
}
