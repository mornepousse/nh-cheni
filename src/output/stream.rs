//! Subprocess runner with merged stdout+stderr and live prettification.
//!
//! We need three things from `cheni upgrade` / `cheni build` /
//! `cheni self-update` when they shell out to `nh` or `nix`:
//!
//! 1. **Live output**, so the user sees progress on a multi-minute
//!    rebuild rather than waiting in silence.
//! 2. **In-order merging of stdout and stderr**. `nh` interleaves the
//!    two streams deliberately; a `select!` over two pipes can reorder
//!    them by a few milliseconds under load, which breaks error
//!    messages that straddle the boundary.
//! 3. **Full capture**, so downstream parsers (the build error parser,
//!    the diagnose pattern library) see the raw text for pattern
//!    matching.
//!
//! The trick: give the child process the *same write end* of a single
//! pipe for both its stdout and stderr. The kernel then serialises
//! every write (POSIX atomic up to PIPE_BUF, and toolchain output is
//! line-buffered well below that), so the parent reads one merged
//! stream in natural emission order. No `select!`, no reordering, no
//! edge cases.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result};
use os_pipe::pipe;

use super::prettify::prettify_line;

/// Outcome of a `run_streaming` call.
pub struct StreamOutput {
    /// Exit status reported by the child.
    pub status: ExitStatus,
    /// Full raw output (stdout + stderr merged, **not** prettified)
    /// for downstream structured parsers.
    pub raw_buffer: String,
}

/// Spawn `program` with `args`, merge stdout+stderr through a single
/// OS pipe, print each line to the terminal via [`prettify_line`], and
/// accumulate the raw lines into `raw_buffer`.
///
/// When `cwd` is `Some`, runs the child in that directory; otherwise
/// inherits the caller's cwd.
///
/// Returns only when the child has exited. The `Result` is `Err` for
/// spawn/io failures; a non-zero exit is reported in
/// `StreamOutput::status` and left to the caller to handle.
pub fn run_streaming(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
) -> Result<StreamOutput> {
    let (reader, writer) = pipe().context("creating merge pipe")?;
    let writer_clone = writer.try_clone().context("cloning pipe writer")?;

    let mut cmd = Command::new(program);
    cmd.args(args).stdout(writer).stderr(writer_clone);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| crate::nix::tools::tool_error(program, e))?;

    // Dropping `cmd` is essential: it owns the remaining parent-side
    // writer handles, and the reader only sees EOF once every writer
    // end is closed. Without this drop we'd block forever on the
    // child exit even after it's done.
    drop(cmd);

    let mut buffer = String::new();
    for line in BufReader::new(reader).lines() {
        let line = line.context("reading merged pipe")?;
        println!("{}", prettify_line(&line));
        buffer.push_str(&line);
        buffer.push('\n');
    }

    let status = child
        .wait()
        .with_context(|| format!("waiting for {}", program))?;
    Ok(StreamOutput {
        status,
        raw_buffer: buffer,
    })
}
