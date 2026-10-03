//! Context-window depletion, read from the live Claude Code transcript.
//!
//! The UserPromptSubmit hook event carries `transcript_path`, and every assistant
//! message in that JSONL records a `usage` block. Reading the most recent one gives
//! true context depletion for free — no API call, no estimation.
//!
//! This exists because turn counting is a poor proxy for depletion: a build turn
//! that reads three large files consumes an order of magnitude more context than a
//! discussion turn, so a fixed prompt-count threshold fires early in conversation
//! and late in heavy work — exactly backwards from what the bracket is for.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How much of the transcript tail to scan. Usage blocks appear on every assistant
/// message, so the last 256 KB reliably contains one, and a bounded read keeps the
/// hook cheap on multi-megabyte transcripts.
const TAIL_BYTES: u64 = 256 * 1024;

/// Tokens currently occupying the context window, from the newest `usage` block.
///
/// Sums the three input figures: `cache_read_input_tokens` carries the bulk of it
/// (a cached prompt still occupies the window), plus fresh cache writes and
/// uncached input. Output tokens are excluded — they are not resident context.
///
/// Returns `None` when the transcript is missing, unreadable, or has no usage yet
/// (the first prompt of a session), so callers fall back to turn counting.
pub fn context_tokens(transcript_path: &str) -> Option<u32> {
    let mut file = std::fs::File::open(Path::new(transcript_path)).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;

    // Read as bytes, not str: a mid-file seek can split a multibyte character,
    // which would make a UTF-8 read fail outright rather than lose one line.
    let mut raw = Vec::new();
    file.take(TAIL_BYTES + 1024).read_to_end(&mut raw).ok()?;
    let buf = String::from_utf8_lossy(&raw);

    // Scan newest-first. A seek landing mid-line leaves one unparseable fragment,
    // which simply fails serde and is skipped.
    buf.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|v| usage_total(&v))
}

fn usage_total(v: &serde_json::Value) -> Option<u32> {
    let usage = v.get("message")?.get("usage")?;
    let field = |k: &str| usage.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let total = field("input_tokens")
        + field("cache_read_input_tokens")
        + field("cache_creation_input_tokens");
    (total > 0).then_some(total as u32)
}

// ─── What the correction detector reads (BO-15) ─────────────────────────────
//
// The Stop hook payload carries `transcript_path` as the prompt hook's does. The detector reads the lines a session
// added since it last read (`read_events`), or a whole transcript (`read_all`), and keeps only what a correction can
// show: the prompts, the AI's own text, the files it wrote, the user stopping a turn or refusing a tool call, and
// Claude Code noticing a file it had changed outside the AI's own write.
//
// SHAPES, read on Claude Code 2.1.288 transcripts (2026-10-03). A typed prompt is a `user` line with
// `turnOrigin: "human"`; a queued task notification has `task_notification`. The AI's text is an `assistant` line's
// `text` block; hook output, CLAUDE.md and system reminders arrive as `attachment` lines or `isMeta` user lines, never
// as assistant text, so a marker quoted in them is never read as the AI's. An interrupt is a user line with
// `interruptedMessageId`, or a text block starting `[Request interrupted by user`. A refused tool call is a user line
// with `toolDenialKind: "user-rejected"` (`permission-rule` is a hook or rule, `cancelled` a classifier: not the
// user). A tool result that merely CONTAINS those strings, as a Read of a document about them does, is neither.

/// The tools whose `file_path` is a file the AI wrote.
pub const WRITE_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// The start of the tool result Claude Code writes when the user refuses a tool call, for transcripts written before
/// `toolDenialKind` existed.
const DENIAL_TEXT: &str = "The user doesn't want to proceed with this tool use.";

/// The start of the user line Claude Code writes when the user stops a turn.
const INTERRUPT_TEXT: &str = "[Request interrupted by user";

/// A session with no reading yet starts this far from the end of a longer transcript: a session that upgrades base
/// half way through does not read its own history as new.
pub const FIRST_READ_MAX: u64 = 4 * 1024 * 1024;

/// One thing a transcript line says, as the correction detector reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A prompt: typed by the user (`human`), or not (a task notification, a local command).
    Prompt { human: bool, text: String },
    /// A text block the AI wrote in the main thread: not thinking, not a subagent's.
    Text(String),
    /// A file the AI wrote with one of [`WRITE_TOOLS`].
    Wrote(String),
    /// The user stopped the turn.
    Interrupt,
    /// The user refused a tool call.
    Denial,
    /// Claude Code saw a file the AI had touched change outside the AI's own write (`edited_text_file`).
    FileChanged(String),
}

/// What one read found, and where the next read starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stretch {
    pub events: Vec<Event>,
    /// The byte offset after the last whole line read. A line still being written is left for the next read.
    pub next: u64,
}

/// A prompt text that no person typed: a queued task notification or a local command's echo.
pub fn machine_prompt(text: &str) -> bool {
    let t = text.trim_start();
    ["<task-notification>", "<command-", "<local-command-", "<bash-"].iter().any(|p| t.starts_with(p))
}

/// The events of one transcript line. A line that cannot matter is passed over before any JSON is parsed: an ordinary
/// tool result (often most of a transcript's bytes) and every line kind the detector does not read.
pub fn events_of_line(line: &str) -> Vec<Event> {
    let interesting = line.contains("\"assistant\"")
        || line.contains("\"user\"")
        || line.contains("edited_text_file");
    if !interesting {
        return Vec::new();
    }
    if line.contains("\"toolUseResult\"") && !line.contains("toolDenialKind") && !line.contains(DENIAL_TEXT) {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    if v.get("isSidechain").and_then(serde_json::Value::as_bool) == Some(true) {
        return Vec::new();
    }
    match v.get("type").and_then(serde_json::Value::as_str) {
        Some("user") => user_events(&v),
        Some("assistant") => assistant_events(&v),
        Some("attachment") => {
            let a = v.get("attachment");
            let kind = a.and_then(|a| a.get("type")).and_then(serde_json::Value::as_str);
            match (kind, a.and_then(|a| a.get("filename")).and_then(serde_json::Value::as_str)) {
                (Some("edited_text_file"), Some(file)) => vec![Event::FileChanged(file.to_string())],
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

fn user_events(v: &serde_json::Value) -> Vec<Event> {
    match v.get("toolDenialKind").and_then(serde_json::Value::as_str) {
        Some("user-rejected") => return vec![Event::Denial],
        Some(_) => return Vec::new(),
        None => {}
    }
    if v.get("interruptedMessageId").is_some() {
        return vec![Event::Interrupt];
    }
    let content = v.get("message").and_then(|m| m.get("content"));
    let text = match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(blocks)) => {
            if let Some(result) = blocks.iter().find(|b| b.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")) {
                // A transcript older than `toolDenialKind`: the refusal is an error result in Claude Code's own words.
                let is_error = result.get("is_error").and_then(serde_json::Value::as_bool) == Some(true);
                let said = match result.get("content") {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(serde_json::Value::Array(parts)) => parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(serde_json::Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                return if is_error && said.trim_start().starts_with(DENIAL_TEXT) { vec![Event::Denial] } else { Vec::new() };
            }
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(serde_json::Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => return Vec::new(),
    };
    if text.trim_start().starts_with(INTERRUPT_TEXT) {
        return vec![Event::Interrupt];
    }
    // A system reminder, a skill's expanded text, an image note: Claude Code's, not a prompt (465 such lines in Chris's
    // 80 newest transcripts, 2026-10-03).
    if v.get("isMeta").and_then(serde_json::Value::as_bool) == Some(true) {
        return Vec::new();
    }
    let human = match v.get("turnOrigin").and_then(serde_json::Value::as_str) {
        Some(origin) => origin == "human",
        None => match v.get("origin").and_then(|o| o.get("kind")).and_then(serde_json::Value::as_str) {
            Some(kind) => kind == "human",
            None => !machine_prompt(&text),
        },
    };
    vec![Event::Prompt { human: human && !machine_prompt(&text), text }]
}

fn assistant_events(v: &serde_json::Value) -> Vec<Event> {
    let Some(blocks) = v.get("message").and_then(|m| m.get("content")).and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for b in blocks {
        match b.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => {
                if let Some(t) = b.get("text").and_then(serde_json::Value::as_str) {
                    out.push(Event::Text(t.to_string()));
                }
            }
            Some("tool_use") => {
                let name = b.get("name").and_then(serde_json::Value::as_str).unwrap_or_default();
                if WRITE_TOOLS.contains(&name) {
                    let input = b.get("input");
                    let file = input
                        .and_then(|i| i.get("file_path").or_else(|| i.get("notebook_path")))
                        .and_then(serde_json::Value::as_str);
                    if let Some(f) = file.filter(|f| !f.is_empty()) {
                        out.push(Event::Wrote(f.to_string()));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Read the whole lines of `path` from byte `from` to the end, and their events. A line still being written (no
/// newline yet) is not read; `next` stops before it. A start inside a line (a first read [`FIRST_READ_MAX`] from the
/// end) goes on to the next line start. An offset past the end (a file that was replaced) starts again
/// [`FIRST_READ_MAX`] from the end.
pub fn read_events(path: &Path, from: u64) -> std::io::Result<Stretch> {
    use std::io::BufRead;
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = if from > len { len.saturating_sub(FIRST_READ_MAX) } else { from };
    let mut skip_partial = false;
    if start > 0 {
        file.seek(SeekFrom::Start(start - 1))?;
        let mut before = [0u8; 1];
        file.read_exact(&mut before)?;
        skip_partial = before[0] != b'\n';
    }
    file.seek(SeekFrom::Start(start))?;
    let mut reader = std::io::BufReader::new(file);
    let mut at = start;
    let mut events = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line)?;
        if n == 0 || line.last() != Some(&b'\n') {
            break;
        }
        at += n as u64;
        if skip_partial {
            skip_partial = false;
            continue;
        }
        events.extend(events_of_line(&String::from_utf8_lossy(&line)));
    }
    Ok(Stretch { events, next: at })
}

/// Where a session's first read starts: the beginning, or [`FIRST_READ_MAX`] from the end of a longer transcript.
/// [`read_events`] reads from the first line start at or after it.
pub fn first_offset(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0).saturating_sub(FIRST_READ_MAX)
}

/// Every event of a whole transcript, in order.
pub fn read_all(path: &Path) -> std::io::Result<Vec<Event>> {
    read_events(path, 0).map(|s| s.events)
}

/// Above this, the configured window is not believable — real usage cannot exceed
/// the window by half again. Treated as misconfiguration rather than depletion.
const IMPLAUSIBLE_PCT: f64 = 150.0;

/// Context depletion as a percentage of the configured window.
/// `None` propagates from `context_tokens` and means "fall back to turns".
///
/// Also returns `None` when the result is implausibly high, which means
/// `context_window` is set below the model's real window — most likely a config
/// carried over from a smaller-context model. Falling back to turns is strictly
/// better than pinning the session to CRITICAL from its first prompt.
pub fn context_pct(transcript_path: &str, context_window: u32) -> Option<f64> {
    if context_window == 0 {
        return None;
    }
    let used = context_tokens(transcript_path)?;
    let pct = (f64::from(used) / f64::from(context_window)) * 100.0;
    (pct <= IMPLAUSIBLE_PCT).then_some(pct)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_transcript(dir: &Path, lines: &[&str]) -> String {
        let path = dir.join("transcript.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn reads_newest_usage_block() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_transcript(
            tmp.path(),
            &[
                r#"{"message":{"usage":{"input_tokens":10,"cache_read_input_tokens":1000,"cache_creation_input_tokens":0}}}"#,
                r#"{"type":"user","message":{"content":"no usage here"}}"#,
                r#"{"message":{"usage":{"input_tokens":2,"cache_read_input_tokens":5000,"cache_creation_input_tokens":500}}}"#,
            ],
        );
        assert_eq!(context_tokens(&p), Some(5502));
    }

    #[test]
    fn percent_of_window() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_transcript(
            tmp.path(),
            &[r#"{"message":{"usage":{"input_tokens":0,"cache_read_input_tokens":50000,"cache_creation_input_tokens":0}}}"#],
        );
        let pct = context_pct(&p, 200_000).unwrap();
        assert!((pct - 25.0).abs() < 0.001, "expected 25%, got {pct}");
    }

    #[test]
    fn missing_transcript_is_none() {
        assert_eq!(context_tokens("/nonexistent/transcript.jsonl"), None);
        assert_eq!(context_pct("/nonexistent/transcript.jsonl", 200_000), None);
    }

    #[test]
    fn no_usage_yet_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_transcript(tmp.path(), &[r#"{"type":"user","message":{"content":"hi"}}"#]);
        assert_eq!(context_tokens(&p), None);
    }

    #[test]
    fn implausible_window_falls_back() {
        // 900k used against a 200k window = 450% — the window is misconfigured,
        // so the caller must fall back to turns rather than pin CRITICAL.
        let tmp = tempfile::tempdir().unwrap();
        let p = write_transcript(
            tmp.path(),
            &[r#"{"message":{"usage":{"cache_read_input_tokens":900000}}}"#],
        );
        assert_eq!(context_pct(&p, 200_000), None);
        // Same reading against the correct 1M window is a normal 90%.
        let pct = context_pct(&p, 1_000_000).unwrap();
        assert!((pct - 90.0).abs() < 0.001, "expected 90%, got {pct}");
    }

    #[test]
    fn zero_window_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_transcript(
            tmp.path(),
            &[r#"{"message":{"usage":{"cache_read_input_tokens":100}}}"#],
        );
        assert_eq!(context_pct(&p, 0), None);
    }
}
