//! No-key LLM access: shell out to headless Claude Code (`claude -p`), reusing
//! the user's existing auth. base stays Rust — the LLM is a subprocess, not a
//! bolted-on SDK or an API-key dependency. ~20-30s per call, so callers must
//! cache by content hash and treat extraction as a batch operation.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

/// The variable every headless call base makes carries, set to `1` (F27, BO-08).
///
/// Claude Code runs the user's hooks inside a `claude -p` session, base's own among them, so without it a
/// `base graph extract` fired base's session start inside base's own call: the call took a relay codename (or, started
/// from a Windows Terminal tab, the tab's title: BO-05), wrote hook log rows and session files, and added base's context
/// to the extraction prompt. Every base hook returns at once, printing and writing nothing, when it is set
/// ([`headless`], checked first in `hook::dispatch`).
pub const HEADLESS_ENV: &str = "BASE_HEADLESS";

/// True inside one of base's own headless calls: [`HEADLESS_ENV`] is `1`, the value [`claude`] sets. Only that value
/// counts, so `BASE_HEADLESS=0` or an empty export meant as "off" never turns every base hook off unseen.
pub fn headless() -> bool {
    std::env::var_os(HEADLESS_ENV).is_some_and(|v| v == "1")
}

/// `claude -p <prompt> --output-format text [--model <model>]` with [`HEADLESS_ENV`] set: the one place every call here
/// is built.
fn claude(prompt: &str, model: Option<&str>) -> Command {
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(prompt).arg("--output-format").arg("text");
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    cmd.env(HEADLESS_ENV, "1");
    cmd
}

/// A finished call's answer, or its stderr as the error. `what` names the call in the error.
fn answer(success: bool, stdout: &[u8], stderr: &[u8], what: &str) -> Result<String> {
    if !success {
        bail!("{what} failed: {}", String::from_utf8_lossy(stderr).trim());
    }
    Ok(String::from_utf8_lossy(stdout).trim().to_string())
}

/// Run a single completion. `model` is an optional Claude Code model alias
/// (e.g. "haiku" for cheap bulk extraction, "opus" for hard reasoning); None
/// uses the Claude Code default.
pub fn complete(prompt: &str, model: Option<&str>) -> Result<String> {
    let out = claude(prompt, model)
        .output()
        .context("failed to spawn `claude` — is Claude Code on PATH?")?;
    answer(out.status.success(), &out.stdout, &out.stderr, "claude -p")
}

/// [`complete`] with extra `claude` arguments, run in `cwd`, with stdin closed and a time limit. `base doctor --measure`
/// uses it to load one settings file and nothing else. Stdin is closed because `claude -p` otherwise waits 3 s for piped
/// input it will never get. The environment is inherited on purpose: `claude` finds its own login through it.
///
/// THE LIMIT COVERS THE OUTPUT TOO. `claude` exiting is not the end of the call: a child it started can still hold the
/// output pipes, and waiting for them to close would have no deadline. After the exit the pipes get a short grace and
/// then the call fails. On the time limit the whole process tree is stopped, not only `claude`.
pub fn complete_with(
    prompt: &str,
    model: Option<&str>,
    extra: &[String],
    cwd: Option<&Path>,
    limit: Duration,
) -> Result<String> {
    const PIPE_GRACE: Duration = Duration::from_secs(10);
    let mut cmd = claude(prompt, model);
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
    // Drained on their own threads so a full pipe can never stall the child while this one waits on it. Each sends
    // its bytes when its pipe closes, so the wait for them can have a deadline.
    let (tx, rx) = mpsc::channel::<(usize, Vec<u8>)>();
    let pipes: [Option<Box<dyn Read + Send>>; 2] = [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
    ];
    for (i, pipe) in pipes.into_iter().enumerate() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = pipe {
                let _ = r.read_to_end(&mut buf);
            }
            let _ = tx.send((i, buf));
        });
    }
    drop(tx);
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting on `claude`")? {
            break status;
        }
        if Instant::now() >= deadline {
            kill_tree(&mut child);
            bail!("claude -p gave no answer within {} s and was stopped", limit.as_secs());
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let mut out: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
    for _ in 0..2 {
        match rx.recv_timeout(PIPE_GRACE) {
            Ok((i, buf)) => out[i] = buf,
            Err(_) => bail!(
                "claude -p exited but its output stayed open {} s later: a process it started still holds it",
                PIPE_GRACE.as_secs()
            ),
        }
    }
    answer(status.success(), &out[0], &out[1], "claude -p")
}

/// Stops `child` and everything it started. On Windows `Child::kill` ends one process, so the tree goes through
/// `taskkill /T` first; elsewhere the kill is what there is.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
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
    let out = claude(&full, model)
        .arg("--allowedTools")
        .arg("Read")
        .output()
        .context("failed to spawn `claude` for vision — is Claude Code on PATH?")?;
    answer(out.status.success(), &out.stdout, &out.stderr, "claude -p (vision)")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F27a (BO-08): every headless call is built by [`claude`], and the command it builds carries the marker, so the
    /// child's environment has `BASE_HEADLESS=1` and base's own hooks inside the call return at once.
    #[test]
    fn headless_calls_set_marker() {
        for model in [None, Some("haiku")] {
            let cmd = claude("a prompt", model);
            let marker: Vec<_> = cmd.get_envs().filter(|(k, _)| *k == HEADLESS_ENV).collect();
            assert_eq!(marker, [(std::ffi::OsStr::new(HEADLESS_ENV), Some(std::ffi::OsStr::new("1")))], "model {model:?}");
            assert_eq!(cmd.get_program(), "claude");
            assert_eq!(cmd.get_args().take(2).collect::<Vec<_>>(), ["-p", "a prompt"]);
        }
    }
}
