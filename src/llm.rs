//! No-key LLM access: shell out to headless Claude Code (`claude -p`), reusing
//! the user's existing auth. base stays Rust — the LLM is a subprocess, not a
//! bolted-on SDK or an API-key dependency. ~20-30s per call, so callers must
//! cache by content hash and treat extraction as a batch operation.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

/// Run a single completion. `model` is an optional Claude Code model alias
/// (e.g. "haiku" for cheap bulk extraction, "opus" for hard reasoning); None
/// uses the Claude Code default.
pub fn complete(prompt: &str, model: Option<&str>) -> Result<String> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(prompt).arg("--output-format").arg("text");
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    let out = cmd
        .output()
        .context("failed to spawn `claude` — is Claude Code on PATH?")?;
    if !out.status.success() {
        bail!(
            "claude -p failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// [`complete`] with extra `claude` arguments, run in `cwd`, with stdin closed and a time limit. `base doctor --measure`
/// uses it to load one settings file and nothing else. Stdin is closed because `claude -p` otherwise waits 3 s for piped
/// input it will never get. The environment is inherited on purpose: `claude` finds its own login through it.
pub fn complete_with(
    prompt: &str,
    model: Option<&str>,
    extra: &[String],
    cwd: Option<&Path>,
    limit: Duration,
) -> Result<String> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(prompt).arg("--output-format").arg("text");
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    cmd.args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd
        .spawn()
        .context("failed to spawn `claude` — is Claude Code on PATH?")?;
    // Drained on their own threads so a full pipe can never stall the child while this one waits on it.
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout = drain(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting on `claude`")? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("claude -p gave no answer within {} s and was stopped", limit.as_secs());
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if !status.success() {
        bail!("claude -p failed: {}", String::from_utf8_lossy(&stderr).trim());
    }
    Ok(String::from_utf8_lossy(&stdout).trim().to_string())
}

/// A completion that can see an image. `claude -p` is text-only on stdin, so we
/// hand it the absolute path and whitelist the `Read` tool — Claude Code's Read
/// renders images, giving the headless model vision without an API key or an
/// image-upload channel. Used by the multimodal extractor's image adapter.
pub fn complete_with_image(prompt: &str, image_path: &Path, model: Option<&str>) -> Result<String> {
    let full = format!(
        "{prompt}\n\nThe image is at this absolute path: {}\nUse the Read tool to view it, then answer.",
        image_path.display()
    );
    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg(full)
        .arg("--output-format").arg("text")
        .arg("--allowedTools").arg("Read");
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    let out = cmd
        .output()
        .context("failed to spawn `claude` for vision — is Claude Code on PATH?")?;
    if !out.status.success() {
        bail!(
            "claude -p (vision) failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
