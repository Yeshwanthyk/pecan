//! Shared, timed-out `git`/`gh` subprocess execution.
//!
//! Every command spawned here is `kill_on_drop` and wrapped in an explicit
//! timeout, so a hung `git`/`gh` process cannot hang a request indefinitely.
//! This consolidates what used to be near-identical subprocess plumbing
//! duplicated between the read-only git-diff review path (`api.rs`) and the
//! git/gh mutation path (`ship.rs`).

use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::api_errors::ApiError;

/// Timeout for read-only status/diff/query commands.
pub(crate) const STATUS_TIMEOUT: Duration = Duration::from_secs(8);
/// Timeout for commands that mutate repository or remote state.
pub(crate) const ACTION_TIMEOUT: Duration = Duration::from_secs(90);

/// Runs `program args` in `cwd` under `timeout`, with no output size limit.
/// Reserved for commands whose output is always small (`rev-parse`,
/// `symbolic-ref`, `show-ref`, ...).
///
/// # Errors
/// Returns [`ApiError::bad_gateway`] when the process cannot be spawned or
/// does not finish within `timeout`.
pub(crate) async fn run(
    program: &str,
    cwd: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, ApiError> {
    tokio::time::timeout(
        timeout,
        Command::new(program).current_dir(cwd).args(args).kill_on_drop(true).output(),
    )
    .await
    .map_err(|_elapsed| ApiError::bad_gateway(format!("{program} command timed out")))?
    .map_err(|error| ApiError::bad_gateway(format!("could not run {program}: {error}")))
}

/// A subprocess result whose stdout/stderr were capped at caller-chosen
/// limits, with each stream flagged when it hit its cap.
pub(crate) struct BoundedOutput {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    /// Whether stdout hit `max_stdout_bytes` and was cut off.
    pub(crate) stdout_truncated: bool,
    /// Whether stderr hit `max_stderr_bytes` and was cut off.
    pub(crate) stderr_truncated: bool,
}

/// Runs `program args` in `cwd` under `timeout`, capping stdout at
/// `max_stdout_bytes` and stderr at `max_stderr_bytes` rather than letting
/// either grow unbounded. Overflow is reported via the `*_truncated` flags
/// instead of failing the call, so callers choose whether truncation is
/// acceptable (a diff preview) or should be turned into a hard error (a Ship
/// mutation).
///
/// # Errors
/// Returns [`ApiError::bad_gateway`] when the process cannot be spawned, its
/// pipes cannot be read, or it does not finish within `timeout`. Returns
/// [`ApiError::internal`] if the child's stdout/stderr pipes are missing,
/// which should not happen given they are always requested below.
pub(crate) async fn run_bounded(
    program: &str,
    cwd: &str,
    args: &[&str],
    timeout: Duration,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
) -> Result<BoundedOutput, ApiError> {
    let mut child = Command::new(program)
        .current_dir(cwd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| ApiError::bad_gateway(format!("could not run {program}: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::internal(format!("{program} stdout pipe was unavailable")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::internal(format!("{program} stderr pipe was unavailable")))?;
    let read = async move {
        let (stdout_bytes, stdout_truncated) =
            read_capped(stdout, max_stdout_bytes).await.map_err(|error| {
                ApiError::bad_gateway(format!("could not read {program} output: {error}"))
            })?;
        let (stderr_bytes, stderr_truncated) =
            read_capped(stderr, max_stderr_bytes).await.map_err(|error| {
                ApiError::bad_gateway(format!("could not read {program} error: {error}"))
            })?;
        let status = child.wait().await.map_err(|error| {
            ApiError::bad_gateway(format!("could not wait for {program}: {error}"))
        })?;
        Ok(BoundedOutput {
            status,
            stdout: stdout_bytes,
            stderr: stderr_bytes,
            stdout_truncated,
            stderr_truncated,
        })
    };
    tokio::time::timeout(timeout, read)
        .await
        .map_err(|_elapsed| ApiError::bad_gateway(format!("{program} command timed out")))?
}

/// Reads up to `limit` bytes from `reader`, draining the remainder so the
/// process can still exit; returns the captured prefix and whether more
/// bytes were discarded.
async fn read_capped(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut chunk = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok((output, truncated));
        }
        let remaining = limit.saturating_sub(output.len());
        if remaining > 0
            && let Some(bytes) = chunk.get(..read.min(remaining))
        {
            output.extend_from_slice(bytes);
        }
        truncated |= read > remaining;
    }
}

/// Maps a failed command's stderr into a bad-gateway [`ApiError`], keeping
/// only the first line for a compact message.
pub(crate) fn failure(action: &str, stderr: &[u8]) -> ApiError {
    let detail = String::from_utf8_lossy(stderr);
    ApiError::bad_gateway(format!(
        "{action} failed: {}",
        detail.lines().next().unwrap_or("unknown error")
    ))
}
