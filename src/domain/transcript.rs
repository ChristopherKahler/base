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
