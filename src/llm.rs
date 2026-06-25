//! No-key LLM access: shell out to headless Claude Code (`claude -p`), reusing
//! the user's existing auth. base stays Rust — the LLM is a subprocess, not a
//! bolted-on SDK or an API-key dependency. ~20-30s per call, so callers must
//! cache by content hash and treat extraction as a batch operation.

use std::path::Path;
use std::process::Command;

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
