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

/// `claude -p --output-format text [--model <model>]`, the prompt to come on stdin: [`complete_stdin`]'s command.
fn claude_stdin(model: Option<&str>) -> Command {
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg("--output-format").arg("text");
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    cmd.env(HEADLESS_ENV, "1");
    cmd
}

// ─── The test seam (BO-17) ───────────────────────────────────────────────────
//
// `BASE_LLM_FAKE` stands in for `claude` in every call this module makes: a path to a JSON file of answers, or `fail`.
// `BASE_LLM_FAKE_LOG` names a file every call is appended to, answered or not. The integration tests drive the real
// binary, so this is how they give `base tune` a judge and how `no_hook_calls_llm` proves that no hook path reaches
// this module: with `fail` set, any call is written to the log before it fails, whatever the caller does with the
// error. Unset (as it is for everyone but the tests), it is never read past one variable lookup.

/// The fake's answers: the first whose `when` is in the prompt (or that has none) answers it.
#[derive(serde::Deserialize)]
struct FakeAnswers {
    answers: Vec<FakeAnswer>,
}

#[derive(serde::Deserialize)]
struct FakeAnswer {
    #[serde(default)]
    when: Option<String>,
    answer: String,
}

/// The fake's answer to `prompt`, or `None` when `BASE_LLM_FAKE` is not set.
fn fake(prompt: &str) -> Option<Result<String>> {
    let spec = std::env::var_os("BASE_LLM_FAKE")?;
    if let Some(log) = std::env::var_os("BASE_LLM_FAKE_LOG") {
        use std::io::Write;
        let line = serde_json::json!({ "prompt_chars": prompt.chars().count(), "prompt_head": prompt.chars().take(200).collect::<String>() });
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "{line}");
        }
    }
    if spec == "fail" {
        return Some(Err(anyhow::anyhow!("BASE_LLM_FAKE=fail: this call should not have been made")));
    }
    let text = match std::fs::read_to_string(&spec) {
        Ok(t) => t,
        Err(e) => return Some(Err(anyhow::anyhow!("BASE_LLM_FAKE {}: {e}", Path::new(&spec).display()))),
    };
    let answers: FakeAnswers = match serde_json::from_str(&text) {
        Ok(a) => a,
        Err(e) => return Some(Err(anyhow::anyhow!("BASE_LLM_FAKE {}: {e}", Path::new(&spec).display()))),
    };
    Some(
        answers
            .answers
            .into_iter()
            .find(|a| a.when.as_deref().is_none_or(|w| prompt.contains(w)))
            .map(|a| a.answer)
            .ok_or_else(|| anyhow::anyhow!("BASE_LLM_FAKE: no answer for this prompt")),
    )
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
    if let Some(faked) = fake(prompt) {
        return faked;
    }
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
    if let Some(faked) = fake(prompt) {
        return faked;
    }
    run_limited(claude(prompt, model), extra, cwd, None, limit)
}

/// [`complete_with`] with the prompt written to `claude`'s standard input instead of its command line (BO-17). Windows
/// caps a command line at 32,767 characters, and one session's batch for `base tune` runs 10 to 60 KB.
pub fn complete_stdin(
    prompt: &str,
    model: Option<&str>,
    extra: &[String],
    cwd: Option<&Path>,
    limit: Duration,
) -> Result<String> {
    if let Some(faked) = fake(prompt) {
        return faked;
    }
    run_limited(claude_stdin(model), extra, cwd, Some(prompt), limit)
}

/// Run `cmd` with `extra` arguments in `cwd` under `limit`: `input` on its stdin, or stdin closed.
fn run_limited(mut cmd: Command, extra: &[String], cwd: Option<&Path>, input: Option<&str>, limit: Duration) -> Result<String> {
    const PIPE_GRACE: Duration = Duration::from_secs(10);
    cmd.args(extra)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd
        .spawn()
        .context("failed to spawn `claude` — is Claude Code on PATH?")?;
    // The prompt is written on its own thread and the pipe closed after it, so a child that answers before it has read
    // all of it can never stall this one.
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let bytes = text.as_bytes().to_vec();
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = stdin.write_all(&bytes);
        });
    }
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
    if let Some(faked) = fake(&full) {
        return faked;
    }
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
