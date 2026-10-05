//! `base doctor --measure` (F6): how much hook text the running Claude Code really delivers to the model, measured per
//! hook, and the `[budget]` keys set from it.
//!
//! WHY THIS EXISTS. The budgets were measured once, on Claude Code 2.1.278 (2026-09-20), and nothing re-measured them as
//! the host moved. The prompt hook's 4,000-byte cap is where 59% of its context was lost (fork base-0160), and nobody
//! knew whether a host limit stood behind that number or only base's own.
//!
//! HOW (locked under D12: payloads with unique markers; a session reports which markers it saw). For each hook and each
//! size, a temporary settings file registers one hook, `base doctor --measure emit`, which prints a payload of exactly
//! that many bytes: a BEGIN line, numbered marker lines every 100 bytes, and an END line, all carrying a fresh nonce. One
//! headless `claude -p` call on a cheap model is asked for the highest marker it can see and whether it sees the END
//! line. Over the host's limit the model is handed a 2,000-character preview instead, so it answers near `0019 NO`.
//!
//! ISOLATION. The call loads no user, project or local settings (`--setting-sources ""`) and no MCP servers, so neither
//! base's own hooks nor any plugin's can add text to what is being counted: only the measure hook fires. Checked on
//! Claude Code 2.1.287 with `--output-format stream-json --include-hook-events`: one hook event per call, and the init
//! event lists no plugin but Claude Code's built-in ones. This holds with or without F27's headless marker (BO-08),
//! because the emitter is not a regular hook and the regular hooks are never loaded into the call at all.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::emit::prompt::thousands;

/// The hooks base budgets, by the names `base hook` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
}

impl Hook {
    pub const ALL: [Hook; 3] = [Hook::SessionStart, Hook::UserPromptSubmit, Hook::PreToolUse];

    pub fn name(self) -> &'static str {
        match self {
            Hook::SessionStart => "session-start",
            Hook::UserPromptSubmit => "user-prompt-submit",
            Hook::PreToolUse => "pre-tool-use",
        }
    }

    pub fn parse(s: &str) -> Option<Hook> {
        Self::ALL.into_iter().find(|h| h.name() == s)
    }

    /// Claude Code's name for the event.
    fn event(self) -> &'static str {
        match self {
            Hook::SessionStart => "SessionStart",
            Hook::UserPromptSubmit => "UserPromptSubmit",
            Hook::PreToolUse => "PreToolUse",
        }
    }

    /// The `[budget]` key this hook's measurement sets.
    pub fn budget_key(self) -> &'static str {
        match self {
            Hook::SessionStart => "session_start_bytes",
            Hook::UserPromptSubmit => "prompt_bytes",
            Hook::PreToolUse => "pre_tool_bytes",
        }
    }
}

// ─── The payload ─────────────────────────────────────────────

/// The BEGIN line and every marker line, newline included, are this many bytes, so marker `k` starts at byte `k * 100`
/// and a reply of `0019 NO` means delivery stopped near byte 1,900 (Example 1: 8,192 bytes carry markers 0001 to 0080).
pub const LINE_BYTES: usize = 100;

/// The BEGIN line, dots included, without its newline: `LINE_BYTES - 1` bytes.
fn begin_line(nonce: &str, bytes: usize) -> String {
    let head = format!("MEASURE {nonce} BEGIN {bytes} ");
    let dots = (LINE_BYTES - 1).saturating_sub(head.len());
    format!("{head}{}", ".".repeat(dots))
}

fn end_line(nonce: &str, bytes: usize) -> String {
    format!("MEASURE {nonce} END {bytes}")
}

/// A nonce is 1 to 16 ASCII letters and digits with at least one letter, so it can never be read back as a marker.
fn check_nonce(nonce: &str) -> Result<()> {
    if nonce.is_empty()
        || nonce.len() > 16
        || !nonce.chars().all(|c| c.is_ascii_alphanumeric())
        || !nonce.chars().any(|c| c.is_ascii_alphabetic())
    {
        bail!("nonce {nonce:?} must be 1 to 16 ASCII letters and digits, at least one a letter");
    }
    Ok(())
}

/// How many numbered marker lines a payload of `bytes` carries.
pub fn markers_in(bytes: usize, nonce: &str) -> usize {
    let fixed = begin_line(nonce, bytes).len() + 1 + end_line(nonce, bytes).len() + 1;
    bytes.saturating_sub(fixed) / LINE_BYTES
}

/// Exactly `bytes` bytes of ASCII: the BEGIN line and marker lines `0001` upward, [`LINE_BYTES`] each, one line of
/// dots for the remainder when there is one, and the END line. Every line ends in `\n`.
pub fn payload(bytes: usize, nonce: &str) -> Result<String> {
    check_nonce(nonce)?;
    let begin = begin_line(nonce, bytes);
    let end = end_line(nonce, bytes);
    let fixed = begin.len() + 1 + end.len() + 1;
    if bytes < fixed {
        bail!("{bytes} bytes cannot hold the BEGIN and END lines ({fixed} bytes)");
    }
    let markers = (bytes - fixed) / LINE_BYTES;
    if markers > 9999 {
        bail!("{bytes} bytes needs more than 9,999 four-digit markers");
    }
    let rest = (bytes - fixed) % LINE_BYTES;
    let mut s = String::with_capacity(bytes);
    s.push_str(&begin);
    s.push('\n');
    for k in 1..=markers {
        let head = format!("MEASURE {nonce} {k:04} ");
        s.push_str(&head);
        s.push_str(&".".repeat(LINE_BYTES - 1 - head.len()));
        s.push('\n');
    }
    if rest > 0 {
        s.push_str(&".".repeat(rest - 1));
        s.push('\n');
    }
    s.push_str(&end);
    s.push('\n');
    debug_assert_eq!(s.len(), bytes);
    Ok(s)
}

/// What the measure hook prints for `hook`: the payload as plain stdout on session start and prompt submit, and inside
/// the JSON `additionalContext` envelope on pre-tool, exactly as base's own hooks print on each event
/// (`src/hook/mod.rs`). Plain stdout on pre-tool never reaches the model. On pre-tool the measured size is the
/// `additionalContext` string's, because that is the text a pre-tool budget governs.
pub fn emit(hook: &str, bytes: usize, nonce: &str) -> Result<String> {
    let Some(h) = Hook::parse(hook) else {
        let names: Vec<&str> = Hook::ALL.iter().map(|h| h.name()).collect();
        bail!("unknown hook {hook:?}: expected one of {}", names.join(", "));
    };
    let text = payload(bytes, nonce)?;
    Ok(match h {
        Hook::PreToolUse => format!(
            "{}\n",
            serde_json::json!({
                "hookSpecificOutput": { "hookEventName": "PreToolUse", "additionalContext": text }
            })
        ),
        Hook::SessionStart | Hook::UserPromptSubmit => text,
    })
}

// ─── Reading the answer ──────────────────────────────────────

/// The model's reply, read: the highest marker it reports and whether it reports the END line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Saw { marker: usize, end: bool },
    /// Not exactly one four-digit number and exactly one YES or NO. Never read as a number.
    Unreadable,
}

/// Reads a reply such as `0080 YES`, `0019, NO` or `Highest: 0040. END: YES`. Anything with no four-digit number, no
/// YES or NO, or two different ones of either, is [`Answer::Unreadable`].
pub fn parse_answer(reply: &str) -> Answer {
    let mut numbers: Vec<usize> = Vec::new();
    let mut flags: Vec<bool> = Vec::new();
    for token in reply.split(|c: char| !c.is_ascii_alphanumeric()) {
        if token.len() == 4 && token.bytes().all(|b| b.is_ascii_digit()) {
            let n: usize = token.parse().unwrap_or_default();
            if !numbers.contains(&n) {
                numbers.push(n);
            }
        } else if token.eq_ignore_ascii_case("yes") || token.eq_ignore_ascii_case("no") {
            let f = token.eq_ignore_ascii_case("yes");
            if !flags.contains(&f) {
                flags.push(f);
            }
        }
    }
    match (numbers.as_slice(), flags.as_slice()) {
        ([marker], [end]) => Answer::Saw { marker: *marker, end: *end },
        _ => Answer::Unreadable,
    }
}

/// One payload's fate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every marker and the END line arrived.
    Whole,
    /// The model saw markers up to `marker` and no END line.
    Cut { marker: usize },
    /// The reply could not be read, or contradicts the payload (END seen without the last marker, a marker the payload
    /// never had, or no marker at all, which means the hook did not reach the model).
    Unreadable,
}

/// Judges an answer against the payload it was about. A cut between the last marker and the END line is a real cut.
pub fn judge(answer: &Answer, bytes: usize, nonce: &str) -> Verdict {
    let last = markers_in(bytes, nonce);
    match *answer {
        Answer::Saw { marker, end: true } if marker == last => Verdict::Whole,
        Answer::Saw { marker, end: false } if marker >= 1 && marker <= last => Verdict::Cut { marker },
        _ => Verdict::Unreadable,
    }
}

// ─── The search ──────────────────────────────────────────────

/// One headless call: `hook` registered to print a payload of `bytes` under `nonce`, and the model's reply returned.
/// Behind a trait so the search is tested with a fake; [`ClaudeRunner`] is the real one.
pub trait Runner {
    fn ask(&mut self, hook: Hook, bytes: usize, nonce: &str) -> Result<String>;
}

/// The first size tried. Doubling from here: 4,000, 8,000, 16,000, 32,000, 64,000.
pub const START_BYTES: usize = 4000;
/// Budgets are set in steps of this many bytes; the search stops once the gap between the largest size delivered whole
/// and the smallest cut is one step, so the largest size delivered whole IS the measured value rounded down to 500.
pub const STEP_BYTES: usize = 500;
/// The largest size tried. A host that delivers this much whole is reported as delivering at least this much.
pub const MAX_BYTES: usize = 64_000;
/// Calls per hook, retries included. A limit near 10 KB takes 7; one in the 16 to 32 KB doubling gap takes 9.
pub const MAX_CALLS: usize = 12;
/// An unreadable answer is asked again this many times at the same size, with a fresh nonce, and no more.
pub const RETRIES: usize = 1;
/// The highest marker a payload over the host's limit can show: the host hands the model a 2,000-character preview,
/// and markers start every 100 bytes after a 100-byte BEGIN line. A cut reported past it means the model miscounted or
/// the host truncated in place, so it is asked once more before it is believed (review finding 1: one miscounted
/// `0078 NO` at 8,000 bytes would otherwise have set the budget to 7,500).
pub const PREVIEW_MARKERS: usize = 20;

/// One call of the search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub bytes: usize,
    pub nonce: String,
    pub reply: String,
    pub verdict: Verdict,
}

impl Probe {
    /// One line for the operator: `  8,000 bytes: 0079 of 0079, END seen`.
    pub fn line(&self) -> String {
        let last = markers_in(self.bytes, &self.nonce);
        let what = match self.verdict {
            Verdict::Whole => format!("{last:04} of {last:04}, END seen"),
            Verdict::Cut { marker } => format!("{marker:04} of {last:04}, END missing"),
            Verdict::Unreadable => format!("unreadable reply {:?}", clip(&self.reply, 80)),
        };
        format!("{:>8} bytes: {what}", thousands(self.bytes))
    }
}

/// What the search found for one hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookMeasure {
    pub hook: Hook,
    pub probes: Vec<Probe>,
    /// Why the search stopped before it finished, when it did. A failed hook sets no budget.
    pub failure: Option<String>,
    /// Why the narrowing stopped early after a size had been delivered whole and a larger one cut. The budget is
    /// still the largest size delivered whole, so it is true, only wider than one step from the limit.
    pub stopped: Option<String>,
}

impl HookMeasure {
    /// The largest payload delivered whole.
    pub fn seen(&self) -> Option<usize> {
        self.probes.iter().filter(|p| p.verdict == Verdict::Whole).map(|p| p.bytes).max()
    }

    /// The smallest payload that was cut.
    pub fn missed(&self) -> Option<usize> {
        self.probes
            .iter()
            .filter(|p| matches!(p.verdict, Verdict::Cut { .. }))
            .map(|p| p.bytes)
            .min()
    }

    /// The doubling bracket, for the summary row: the last size seen whole and the first cut while doubling.
    fn doubling(&self) -> (Option<usize>, Option<usize>) {
        // Doubling ends at the first cut, so everything up to it is the doubling phase.
        let first_cut = self.probes.iter().position(|p| matches!(p.verdict, Verdict::Cut { .. }));
        let doubling = &self.probes[..first_cut.map_or(self.probes.len(), |i| i + 1)];
        let seen = doubling.iter().filter(|p| p.verdict == Verdict::Whole).map(|p| p.bytes).max();
        (seen, first_cut.map(|i| self.probes[i].bytes))
    }

    /// The budget this measurement supports: the largest size delivered whole, a multiple of [`STEP_BYTES`]. `None`
    /// when the search failed or nothing arrived whole.
    pub fn budget(&self) -> Option<usize> {
        if self.failure.is_some() {
            return None;
        }
        self.seen().map(|b| b / STEP_BYTES * STEP_BYTES)
    }

    /// The summary row: `session-start      sees 8,000 · misses 16,000 · narrowed to 10,000 bytes (7 calls)`.
    pub fn row(&self) -> String {
        let name = format!("{:<18}", self.hook.name());
        let calls = self.probes.len();
        if let Some(why) = &self.failure {
            return format!("{name} NOT MEASURED after {calls} call(s): {why}");
        }
        let (seen, missed) = self.doubling();
        let seen = seen.map_or("nothing".to_string(), thousands);
        let stopped = self.stopped.as_ref().map_or(String::new(), |why| format!("; stopped narrowing: {why}"));
        match (missed, self.budget()) {
            (Some(m), Some(b)) => format!(
                "{name} sees {seen} · misses {} · narrowed to {} bytes ({calls} calls{stopped})",
                thousands(m),
                thousands(b)
            ),
            (None, Some(b)) => format!(
                "{name} sees {} whole, the largest size tried, so at least {} bytes ({calls} calls)",
                thousands(b),
                thousands(b)
            ),
            (_, None) => format!("{name} nothing arrived whole, even {} bytes ({calls} calls)", thousands(STEP_BYTES)),
        }
    }
}

/// Doubles from [`START_BYTES`] until a payload is cut, then halves the gap on multiples of [`STEP_BYTES`] until it is
/// one step wide. Each call gets a fresh nonce from `nonces`, so no answer can be satisfied by an earlier payload.
/// An unreadable reply is asked again [`RETRIES`] time(s) at the same size; a runner error stops the hook at once.
/// `progress` sees each probe as it lands.
pub fn search(
    runner: &mut dyn Runner,
    hook: Hook,
    nonces: &mut dyn FnMut() -> String,
    progress: &mut dyn FnMut(&Probe),
) -> HookMeasure {
    let mut m = HookMeasure { hook, probes: Vec::new(), failure: None, stopped: None };
    // `ask` returns Some(true) for whole, Some(false) for cut, None when the search must stop (m.failure says why).
    let mut ask = |m: &mut HookMeasure, bytes: usize| -> Option<bool> {
        for attempt in 0..=RETRIES {
            if m.probes.len() >= MAX_CALLS {
                m.failure = Some(format!("stopped at the limit of {MAX_CALLS} calls"));
                return None;
            }
            let nonce = nonces();
            let reply = match runner.ask(hook, bytes, &nonce) {
                Ok(r) => r,
                Err(e) => {
                    m.failure = Some(format!("{e:#}"));
                    return None;
                }
            };
            let verdict = judge(&parse_answer(&reply), bytes, &nonce);
            let probe = Probe { bytes, nonce, reply, verdict: verdict.clone() };
            progress(&probe);
            m.probes.push(probe);
            match verdict {
                Verdict::Whole => return Some(true),
                Verdict::Cut { marker } if marker > PREVIEW_MARKERS && attempt < RETRIES => continue,
                Verdict::Cut { .. } => return Some(false),
                Verdict::Unreadable => continue,
            }
        }
        m.failure = Some(format!("the reply at {} bytes was unreadable {} times", thousands(bytes), RETRIES + 1));
        None
    };

    let mut lo = 0usize;
    let mut hi = None;
    let mut size = START_BYTES;
    loop {
        match ask(&mut m, size) {
            Some(true) => {
                lo = size;
                if size >= MAX_BYTES {
                    break;
                }
                size = (size * 2).min(MAX_BYTES);
            }
            Some(false) => {
                hi = Some(size);
                break;
            }
            None => return m,
        }
    }
    while let Some(h) = hi
        && h - lo > STEP_BYTES
    {
        let mid = lo + (h - lo) / STEP_BYTES / 2 * STEP_BYTES;
        match ask(&mut m, mid) {
            Some(true) => lo = mid,
            Some(false) => hi = Some(mid),
            // A size was delivered whole and a larger one cut: what is known stands, it is only less narrow
            // (review finding 8). With nothing delivered whole there is nothing to stand on.
            None if lo > 0 => {
                m.stopped = m.failure.take();
                return m;
            }
            None => return m,
        }
    }
    m
}

/// F6e: does the model see session-start text past the first 2,000 characters? Answered from the session-start
/// probes; the payload is ASCII, so its bytes are its characters. `None` when no session-start reply was readable.
pub fn first_screen(m: &HookMeasure) -> Option<String> {
    const FIRST_SCREEN: usize = 2000;
    if let Some(p) = m
        .probes
        .iter()
        .filter(|p| p.verdict == Verdict::Whole && p.bytes > FIRST_SCREEN)
        .max_by_key(|p| p.bytes)
    {
        let last = markers_in(p.bytes, &p.nonce);
        return Some(format!(
            "first screen: the model saw session-start text past character 2,000 ({} bytes arrived whole, marker {last:04} of {last:04})",
            thousands(p.bytes)
        ));
    }
    let cut = m
        .probes
        .iter()
        .filter_map(|p| match p.verdict {
            Verdict::Cut { marker } => Some((p, marker)),
            _ => None,
        })
        .max_by_key(|(_, marker)| *marker)?;
    let (p, marker) = cut;
    Some(format!(
        "first screen: the model saw session-start text only to marker {marker:04} of {:04}, about character {}",
        markers_in(p.bytes, &p.nonce),
        thousands(marker * LINE_BYTES)
    ))
}

// ─── The real runner ─────────────────────────────────────────

/// The cheap model every measure call runs on (round 2: Haiku off the hot path only; this is a doctor command).
pub const MODEL: &str = "claude-haiku-4-5";

/// How long one call may take before it is stopped. A timed probe call took 7 s on 2.1.287; `src/llm.rs` says 20 to 30.
pub const CALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(180);

/// The question for `hook`. Pre-tool needs a tool call to fire, so it asks for one harmless command first.
pub fn question(hook: Hook, nonce: &str) -> String {
    let ask = format!(
        "In this session's hook context there are lines starting \"MEASURE {nonce}\". The numbered lines carry a \
four-digit number right after \"MEASURE {nonce} \". Reply with exactly two values: the highest such line number you \
can see, and YES if you can see the line \"MEASURE {nonce} END\", otherwise NO."
    );
    match hook {
        Hook::PreToolUse => format!(
            "First run the shell command \"echo measure\" with the Bash tool, once, and run nothing else. Then answer \
from what is in your context. {ask} Read no file."
        ),
        Hook::SessionStart | Hook::UserPromptSubmit => format!("{ask} Do not use any tools and read no file."),
    }
}

/// The settings file one call loads: one hook, nothing else. Pre-tool matches only Bash, the one tool the call has.
pub fn settings(hook: Hook, command: &str) -> serde_json::Value {
    let mut entry = serde_json::json!({ "hooks": [{ "type": "command", "command": command, "timeout": 30 }] });
    if hook == Hook::PreToolUse {
        entry["matcher"] = serde_json::json!("Bash");
    }
    let mut events = serde_json::Map::new();
    events.insert(hook.event().to_string(), serde_json::json!([entry]));
    serde_json::json!({ "hooks": events })
}

/// The `claude` arguments after the prompt and model: the settings file, nothing else loaded, and the tools the
/// question needs. Pre-tool gets Bash with exactly `echo measure` allowed and everything else refused without a prompt
/// (`dontAsk`), so the model cannot read the file the host saves an over-limit output to.
pub fn claude_args(hook: Hook, settings_path: &Path) -> Vec<String> {
    let mut args: Vec<String> = [
        "--settings",
        &settings_path.display().to_string(),
        "--setting-sources",
        "",
        "--strict-mcp-config",
        "--no-session-persistence",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let tools: &[&str] = match hook {
        Hook::PreToolUse => &["--tools", "Bash", "--allowedTools", "Bash(echo measure)", "--permission-mode", "dontAsk"],
        Hook::SessionStart | Hook::UserPromptSubmit => &["--tools", ""],
    };
    args.extend(tools.iter().map(|s| s.to_string()));
    args
}

/// The real runner: one headless `claude -p` call per probe, in a scratch directory it owns.
pub struct ClaudeRunner {
    /// The base binary the hook runs: this one, so the emitter is always the build doing the measuring.
    exe: PathBuf,
    dir: PathBuf,
}

impl ClaudeRunner {
    /// `exe` is the base binary the measure hook runs. The scratch directory is made under the system temp dir and
    /// removed on drop.
    pub fn new(exe: PathBuf) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("base-measure-{}-{}", std::process::id(), nonce()));
        std::fs::create_dir_all(dir.join("cwd")).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self { exe, dir })
    }

    /// The hook command. Forward slashes and quotes, because Claude Code runs hook commands through a shell and on
    /// Windows that shell is bash, which eats unquoted backslashes.
    fn command(&self, hook: Hook, bytes: usize, nonce: &str) -> String {
        let exe = self.exe.display().to_string().replace('\\', "/");
        format!("\"{exe}\" doctor --measure emit --hook {} --bytes {bytes} --nonce {nonce}", hook.name())
    }
}

impl Runner for ClaudeRunner {
    fn ask(&mut self, hook: Hook, bytes: usize, nonce: &str) -> Result<String> {
        let path = self.dir.join("settings.json");
        let json = settings(hook, &self.command(hook, bytes, nonce));
        std::fs::write(&path, serde_json::to_string_pretty(&json)?)
            .with_context(|| format!("writing {}", path.display()))?;
        crate::llm::complete_with(
            &question(hook, nonce),
            Some(MODEL),
            &claude_args(hook, &path),
            Some(&self.dir.join("cwd")),
            CALL_LIMIT,
        )
    }
}

impl Drop for ClaudeRunner {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A fresh nonce: one letter then three letters or digits, from the standard library's per-process random hash keys.
/// Not cryptographic; it only has to differ from every other payload in the run.
pub fn nonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    let mut v = h.finish();
    const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    const ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut s = String::with_capacity(4);
    s.push(LETTERS[(v % 26) as usize] as char);
    v /= 26;
    for _ in 0..3 {
        s.push(ALNUM[(v % 36) as usize] as char);
        v /= 36;
    }
    s
}

// ─── Writing base.toml ───────────────────────────────────────

/// The user's base.toml, the file `base config set` writes: `~/.base-gbl/base.toml`.
pub fn global_config_path() -> Option<PathBuf> {
    crate::home::home_root().map(|h| h.join(".base-gbl").join("base.toml"))
}

/// Is this line a table header, and is it `[<section>]`?
fn header(line: &str, section: &str) -> Option<bool> {
    let t = line.trim_start();
    if !t.starts_with('[') {
        return None;
    }
    let body = t.split('#').next().unwrap_or("").trim();
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    Some(compact == format!("[{section}]"))
}

/// The key a `key = value` line sets, if it is one.
fn key_of(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.is_empty() || t.starts_with('#') || t.starts_with('[') {
        return None;
    }
    let (k, _) = t.split_once('=')?;
    // Basic and literal quoted keys are both valid TOML (review finding 9).
    Some(k.trim().trim_matches(['"', '\'']))
}

/// Where a line's trailing comment starts (the `#` outside any string), if it has one.
fn comment_at(line: &str) -> Option<usize> {
    let start = line.find('=')? + 1;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in line[start..].char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if q == '"' && c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '#' => return Some(start + i),
            _ => {}
        }
    }
    None
}

/// The spellings of a `[budget]` key: its own, and the old one it replaced (`prompt_chars`).
fn budget_spellings(key: &str) -> Vec<String> {
    let mut v = vec![key.to_string()];
    v.extend(crate::config::RENAMED_BUDGET_KEYS.iter().filter(|k| k.new == key).map(|k| k.old.to_string()));
    v
}

/// `[budget]` keys set in a base.toml's text, every other line left exactly as it was ([`set_section_keys`] on
/// `[budget]`).
///
/// A key spelled the old way (`prompt_chars`) is rewritten to the new spelling, because both spellings set one field and
/// a file holding both no longer parses.
///
/// `keys` holds TOML literals: `"10000"`, `"\"2.1.287\""`. The result is parsed back, and refused unless every key
/// reads back as written and every other value in the file is unchanged.
pub fn set_budget_keys(text: &str, keys: &[(&str, String)]) -> Result<String> {
    let keys: Vec<(&str, Option<String>)> = keys.iter().map(|(k, v)| (*k, Some(v.clone()))).collect();
    set_section_keys(text, "budget", &keys, &budget_spellings)
}

/// The keys of one section set or removed in a base.toml's text, every other line left exactly as it was: the one
/// writer that keeps an operator's file as they wrote it (`base doctor --measure` writes `[budget]` through it, a shadow
/// promotion and rollback `[match]`, BO-20).
///
/// A key the section already has keeps its line, its indentation and its trailing comment; only the value changes. A
/// key set to `None` loses its line. A key the section lacks goes after the section's last key line, so the comment
/// block that heads the next section stays with it. No such section: one is appended, when any key is set.
/// `spellings` gives every name a key is written under (its own first).
///
/// `keys` holds TOML literals. The result is parsed back, and refused unless every key reads back as asked (a removed
/// key absent) and every other value in the file is unchanged.
pub fn set_section_keys(
    text: &str,
    section: &str,
    keys: &[(&str, Option<String>)],
    spellings: &dyn Fn(&str) -> Vec<String>,
) -> Result<String> {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let start = lines.iter().position(|l| header(l, section) == Some(true));
    match start {
        None => {
            let set: Vec<(&str, &String)> = keys.iter().filter_map(|(k, v)| v.as_ref().map(|v| (*k, v))).collect();
            if !set.is_empty() {
                if lines.last().is_some_and(|l| !l.ends_with('\n')) {
                    lines.push(nl.to_string());
                }
                if !lines.is_empty() {
                    lines.push(nl.to_string());
                }
                lines.push(format!("[{section}]{nl}"));
                for (k, v) in set {
                    lines.push(format!("{k} = {v}{nl}"));
                }
            }
        }
        Some(start) => {
            let mut end = lines[start + 1..]
                .iter()
                .position(|l| header(l, section).is_some())
                .map_or(lines.len(), |i| start + 1 + i);
            let mut missing = Vec::new();
            for (k, v) in keys {
                let names = spellings(k);
                let mut found = false;
                let mut i = start + 1;
                while i < end {
                    let named = key_of(&lines[i]).is_some_and(|at| names.iter().any(|n| n == at));
                    if !named {
                        i += 1;
                        continue;
                    }
                    let Some(v) = v else {
                        lines.remove(i);
                        end -= 1;
                        continue;
                    };
                    let line = &mut lines[i];
                    let ending = if line.ends_with("\r\n") {
                        "\r\n"
                    } else if line.ends_with('\n') {
                        "\n"
                    } else {
                        ""
                    };
                    let body = line.trim_end_matches(['\r', '\n']);
                    let indent = &body[..body.len() - body.trim_start().len()];
                    let comment = comment_at(body).map(|i| {
                        let before = body[..i].trim_end();
                        format!("{}{}", &body[before.len()..i], &body[i..])
                    });
                    *line = format!("{indent}{k} = {v}{}{ending}", comment.unwrap_or_default());
                    found = true;
                    i += 1;
                }
                if let (false, Some(v)) = (found, v) {
                    missing.push(format!("{k} = {v}{nl}"));
                }
            }
            if !missing.is_empty() {
                let last_key = (start + 1..end).rev().find(|&i| key_of(&lines[i]).is_some()).unwrap_or(start);
                if !lines[last_key].ends_with('\n') {
                    lines[last_key].push_str(nl);
                }
                for (j, l) in missing.into_iter().enumerate() {
                    lines.insert(last_key + 1 + j, l);
                }
            }
        }
    }
    let out: String = lines.concat();
    check_written(text, &out, section, keys, spellings)?;
    Ok(out)
}

/// Refuses an edit unless the result parses, every key reads back as asked, and nothing else changed.
fn check_written(
    before: &str,
    after: &str,
    section: &str,
    keys: &[(&str, Option<String>)],
    spellings: &dyn Fn(&str) -> Vec<String>,
) -> Result<()> {
    let mut old: toml::Table = toml::from_str(before).context("the existing base.toml does not parse")?;
    let mut new: toml::Table = toml::from_str(after).context("the edited base.toml would not parse; nothing written")?;
    for (k, v) in keys {
        let got = new.get(section).and_then(|b| b.get(*k));
        match v {
            Some(v) => {
                let want: toml::Table = toml::from_str(&format!("x = {v}")).with_context(|| format!("{k} = {v} is not TOML"))?;
                if got != want.get("x") {
                    bail!("[{section}] {k} would read back as {got:?}, not {v}; nothing written");
                }
            }
            None => {
                if let Some(other) = spellings(k).iter().find(|n| new.get(section).and_then(|b| b.get(n.as_str())).is_some()) {
                    bail!("[{section}] {other} would still be set; nothing written");
                }
            }
        }
    }
    for t in [&mut old, &mut new] {
        if let Some(toml::Value::Table(b)) = t.get_mut(section) {
            for (k, _) in keys {
                for n in spellings(k) {
                    b.remove(&n);
                }
            }
            if b.is_empty() {
                t.remove(section);
            }
        }
    }
    if old != new {
        bail!("the edit would change more than the [{section}] keys it sets; nothing written");
    }
    Ok(())
}

/// The value a key has in the file's `[budget]` section, as written there, or `None`.
fn written(text: &str, key: &str) -> Option<String> {
    let t: toml::Table = toml::from_str(text).ok()?;
    let b = t.get("budget")?;
    let mut names = vec![key];
    names.extend(crate::config::RENAMED_BUDGET_KEYS.iter().filter(|k| k.new == key).map(|k| k.old));
    names.iter().find_map(|n| b.get(*n)).map(|v| v.to_string())
}

// ─── The command ─────────────────────────────────────────────

/// What the run wrote, for the caller's exit code.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every hook measured and base.toml written.
    Written,
    /// Some hook did not measure; base.toml untouched.
    NotWritten,
}

/// `base doctor --measure`: measure every hook on the running Claude Code, print each probe as it lands, then write
/// each hook's budget and `measured_on` to the user's base.toml and print them before and after. All or nothing: a
/// hook that fails to measure leaves the file untouched, because a `measured_on` beside one unmeasured number would
/// claim a measurement that never happened.
pub fn run(cwd: &Path, runner: &mut dyn Runner, host: &str, out: &mut dyn Write) -> Result<Outcome> {
    let path = global_config_path().context("no home directory, so no ~/.base-gbl/base.toml")?;
    let read = || {
        std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}; run `base install` first, or create it", path.display()))
    };
    // Fail before spending a call on a file the writer would refuse: the same edit, with placeholder values.
    let mut trial: Vec<(&str, String)> = Hook::ALL.iter().map(|h| (h.budget_key(), "1".to_string())).collect();
    trial.push(("measured_on", "\"0.0.0\"".to_string()));
    set_budget_keys(&read()?, &trial).with_context(|| format!("{} cannot take the measured values", path.display()))?;

    writeln!(
        out,
        "measuring on claude-code {host} (up to {MAX_CALLS} calls per hook, each a headless `claude -p` on {MODEL})"
    )?;
    let mut results = Vec::new();
    for hook in Hook::ALL {
        writeln!(out, "  {}", hook.name())?;
        out.flush()?;
        let mut nonces = nonce;
        let mut progress = |p: &Probe| {
            let _ = writeln!(out, "    {}", p.line());
            let _ = out.flush();
        };
        let m = search(runner, hook, &mut nonces, &mut progress);
        // All or nothing, so a hook that cannot be measured ends the run: every later call would be paid for and
        // thrown away (review finding 5).
        let failed = m.budget().is_none();
        results.push(m);
        if failed {
            break;
        }
    }
    for m in &results {
        writeln!(out, "  {}", m.row())?;
    }
    if let Some(line) = results.iter().find(|m| m.hook == Hook::SessionStart).and_then(first_screen) {
        writeln!(out, "  {line}")?;
    }

    let budgets: Vec<(Hook, usize)> = results.iter().filter_map(|m| m.budget().map(|b| (m.hook, b))).collect();
    if budgets.len() != Hook::ALL.len() {
        let missing: Vec<&str> = Hook::ALL
            .iter()
            .filter(|h| !budgets.iter().any(|(b, _)| b == *h))
            .map(|h| h.name())
            .collect();
        writeln!(out, "nothing written to {}: {} not measured", path.display(), missing.join(", "))?;
        return Ok(Outcome::NotWritten);
    }

    let mut keys: Vec<(&str, String)> = budgets.iter().map(|(h, b)| (h.budget_key(), b.to_string())).collect();
    keys.push(("measured_on", format!("\"{host}\"")));
    // Read again: the measurement takes minutes, and an edit made meanwhile must survive the write.
    let before = read()?;
    let after = set_budget_keys(&before, &keys)?;
    let tmp = path.with_extension("toml.measure-tmp");
    std::fs::write(&tmp, &after).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;

    let defaults = crate::config::BudgetConfig::default();
    let default_of = |key: &str| -> String {
        match key {
            "session_start_bytes" => defaults.session_start_bytes.to_string(),
            "prompt_bytes" => defaults.prompt_bytes.to_string(),
            "pre_tool_bytes" => defaults.pre_tool_bytes.to_string(),
            "measured_on" => format!("{:?}", defaults.measured_on),
            _ => "?".to_string(),
        }
    };
    writeln!(out, "base.toml [budget] ({}):", path.display())?;
    for (k, v) in &keys {
        let was = written(&before, k).unwrap_or_else(|| format!("(default {})", default_of(k)));
        writeln!(out, "  {k:<20} {was} -> {v}")?;
    }
    // A workspace base.toml overlays the global one key by key, so a budget set there wins over what was just written.
    let effective = crate::config::BaseConfig::load(cwd).budget;
    for (h, b) in &budgets {
        let now = match h {
            Hook::SessionStart => effective.session_start_bytes,
            Hook::UserPromptSubmit => effective.prompt_bytes,
            Hook::PreToolUse => effective.pre_tool_bytes,
        };
        if now != *b {
            writeln!(
                out,
                "  ⚠ in {} [budget] {} is {now}: the workspace's base.toml sets it and overrides the measured {b}",
                cwd.display(),
                h.budget_key()
            )?;
        }
    }
    // And `measured_on`, or doctor here goes on asking for a re-measure that cannot change it (review finding 6).
    if crate::doctor::version_in(&effective.measured_on).as_deref() != Some(host) {
        writeln!(
            out,
            "  ⚠ in {} [budget] measured_on is {:?}: the workspace's base.toml sets it and overrides the measured \"{host}\"",
            cwd.display(),
            effective.measured_on
        )?;
    }
    Ok(Outcome::Written)
}

fn clip(s: &str, n: usize) -> String {
    let t = s.trim();
    match t.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &t[..i]),
        None => t.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BO-20: the one writer sets and removes `[match]` keys line by line, as it sets `[budget]`: every other line, and
    /// the comment on a line it changes, stay; a section that is missing is added; anything that is not TOML is refused.
    #[test]
    fn set_section_keys_sets_and_removes_keys_and_keeps_every_other_line() {
        let own = |k: &str| vec![k.to_string()];
        let before = "[budget]\nprompt_bytes = 10000\n\n[match]\n# a note\nbm25 = false # why\nmin_score = 6.0\n\n[shadow]\nmax_ms = 50\n";
        let after = set_section_keys(
            before,
            "match",
            &[("bm25", Some("true".into())), ("min_score", None), ("relative", Some("0.5".into()))],
            &own,
        )
        .expect("written");
        assert_eq!(after, "[budget]\nprompt_bytes = 10000\n\n[match]\n# a note\nbm25 = true # why\nrelative = 0.5\n\n[shadow]\nmax_ms = 50\n");
        let back = set_section_keys(
            &after,
            "match",
            &[("bm25", Some("false".into())), ("min_score", Some("6.0".into())), ("relative", None)],
            &own,
        )
        .expect("written");
        assert_eq!(back, before, "set back, the file is byte for byte what it was");
        assert_eq!(
            set_section_keys("[budget]\nx = 1\n", "match", &[("bm25", Some("true".into()))], &own).expect("written"),
            "[budget]\nx = 1\n\n[match]\nbm25 = true\n"
        );
        assert_eq!(set_section_keys("[budget]\nx = 1\n", "match", &[("bm25", None)], &own).expect("written"), "[budget]\nx = 1\n");
        assert!(set_section_keys(before, "match", &[("bm25", Some("tr ue".into()))], &own).is_err(), "not TOML: refused");
        let crlf = before.replace('\n', "\r\n");
        let written = set_section_keys(&crlf, "match", &[("bm25", Some("true".into()))], &own).expect("written");
        assert!(written.contains("bm25 = true # why\r\n") && !written.replace("\r\n", "").contains('\n'), "CRLF kept");
    }

    /// A host that delivers `limit` bytes whole and hands anything larger over as the 2,000-character preview, answered
    /// the way the model answers: the highest marker whose line starts inside what it sees, and whether END is there.
    struct Fake {
        limit: usize,
        calls: usize,
        /// Replies served before the honest ones, one per call.
        garbled: Vec<&'static str>,
        /// Over the limit, show the first `limit` bytes instead of the 2,000-character preview.
        truncates: bool,
        /// After this many calls every reply is unreadable.
        fail_after: Option<usize>,
    }

    impl Fake {
        fn new(limit: usize) -> Self {
            Self { limit, calls: 0, garbled: Vec::new(), truncates: false, fail_after: None }
        }
    }

    impl Runner for Fake {
        fn ask(&mut self, _hook: Hook, bytes: usize, nonce: &str) -> Result<String> {
            self.calls += 1;
            if !self.garbled.is_empty() {
                return Ok(self.garbled.remove(0).to_string());
            }
            if self.fail_after.is_some_and(|n| self.calls > n) {
                return Ok("I could not tell.".to_string());
            }
            let visible = if bytes <= self.limit {
                bytes
            } else if self.truncates {
                self.limit
            } else {
                bytes.min(2000)
            };
            let last = markers_in(bytes, nonce);
            let marker = (1..=last).rev().find(|k| k * LINE_BYTES < visible).unwrap_or(0);
            Ok(format!("{marker:04} {}", if bytes <= visible { "YES" } else { "NO" }))
        }
    }

    fn measure(fake: &mut Fake) -> (HookMeasure, Vec<usize>) {
        let mut sizes = Vec::new();
        let mut nonces = nonce;
        let m = search(fake, Hook::UserPromptSubmit, &mut nonces, &mut |p| sizes.push(p.bytes));
        (m, sizes)
    }

    #[test]
    fn measure_parses_answers() {
        assert_eq!(parse_answer("0080 YES"), Answer::Saw { marker: 80, end: true });
        assert_eq!(parse_answer("0019 NO"), Answer::Saw { marker: 19, end: false });
        // The shapes Haiku gave on 2.1.287: `0040\nYES` and `0019, NO`.
        assert_eq!(parse_answer("0040\nYES"), Answer::Saw { marker: 40, end: true });
        assert_eq!(parse_answer("0019, NO"), Answer::Saw { marker: 19, end: false });
        assert_eq!(parse_answer("Highest: 0040. END line: yes"), Answer::Saw { marker: 40, end: true });
        // Unreadable is its own answer, never a number.
        for reply in [
            "",
            "YES",
            "0080",
            "I cannot see any MEASURE lines.",
            "0080 0081 YES",
            "0080 YES NO",
            "80 YES",
            "00800 YES",
            "MEASURE q7k2 END 8192 is there, YES, and the highest is 0080",
        ] {
            assert_eq!(parse_answer(reply), Answer::Unreadable, "{reply:?}");
        }
        // Judged against the payload it was about: 8,192 bytes carry markers 0001 to 0080 (Example 1).
        assert_eq!(markers_in(8192, "q7k2"), 80);
        assert_eq!(judge(&parse_answer("0080 YES"), 8192, "q7k2"), Verdict::Whole);
        assert_eq!(judge(&parse_answer("0019 NO"), 8192, "q7k2"), Verdict::Cut { marker: 19 });
        // A cut between the last marker and the END line is a real cut.
        assert_eq!(judge(&parse_answer("0080 NO"), 8192, "q7k2"), Verdict::Cut { marker: 80 });
        // Contradictions are unreadable: END without the last marker, a marker the payload never had, no marker.
        for reply in ["0079 YES", "0099 NO", "0000 NO", "0000 YES"] {
            assert_eq!(judge(&parse_answer(reply), 8192, "q7k2"), Verdict::Unreadable, "{reply:?}");
        }
    }

    #[test]
    fn measure_search_narrows() {
        // The brief's case: a host that sees up to 10,000 bytes.
        let mut fake = Fake::new(10_000);
        let (m, sizes) = measure(&mut fake);
        assert_eq!(m.failure, None);
        let b = m.budget().expect("a budget");
        assert!((9_500..=10_000).contains(&b), "ended at {b}");
        assert!(m.probes.len() <= MAX_CALLS, "{} calls", m.probes.len());
        assert_eq!(fake.calls, m.probes.len(), "every call is on record");
        assert_eq!(sizes, [4000, 8000, 16000, 12000, 10000, 11000, 10500]);
        assert_eq!((m.seen(), m.missed()), (Some(10_000), Some(10_500)));
        assert_eq!(
            m.row(),
            "user-prompt-submit sees 8,000 · misses 16,000 · narrowed to 10,000 bytes (7 calls)"
        );

        // Every limit ends on the largest multiple of 500 at or under it, inside the call limit.
        for limit in [2_345, 9_999, 10_957, 16_000, 31_999, 40_000] {
            let mut fake = Fake::new(limit);
            let (m, _) = measure(&mut fake);
            assert_eq!(m.budget(), Some(limit / STEP_BYTES * STEP_BYTES), "limit {limit}");
            assert!(m.probes.len() <= MAX_CALLS, "limit {limit}: {} calls", m.probes.len());
        }
        // A host that takes the largest size tried is reported as at least that, not as a number it never cut at.
        let mut fake = Fake::new(70_000);
        let (m, _) = measure(&mut fake);
        assert_eq!((m.budget(), m.missed()), (Some(MAX_BYTES), None));
        assert!(m.row().contains("at least 64,000 bytes"), "{}", m.row());
    }

    #[test]
    fn measure_retries_an_unreadable_reply_once_then_stops_without_a_budget() {
        // One unreadable reply: asked again at the same size with a fresh nonce, and the search carries on.
        let mut fake = Fake::new(10_000);
        fake.garbled = vec!["I see some lines."];
        let (m, sizes) = measure(&mut fake);
        assert_eq!(&sizes[..2], [4000, 4000]);
        assert_ne!(m.probes[0].nonce, m.probes[1].nonce, "a retry gets a fresh nonce");
        assert_eq!(m.budget(), Some(10_000));

        // Two at one size: the hook stops and sets no budget, rather than reading a guess as a number.
        let mut fake = Fake::new(10_000);
        fake.garbled = vec!["no idea", "still no idea"];
        let (m, _) = measure(&mut fake);
        assert_eq!(m.probes.len(), 2);
        assert_eq!(m.budget(), None);
        assert!(m.row().contains("NOT MEASURED"), "{}", m.row());

        // A runner that cannot run stops at once and says why.
        struct Broken;
        impl Runner for Broken {
            fn ask(&mut self, _: Hook, _: usize, _: &str) -> Result<String> {
                bail!("failed to spawn `claude`")
            }
        }
        let mut nonces = nonce;
        let m = search(&mut Broken, Hook::PreToolUse, &mut nonces, &mut |_| {});
        assert_eq!(m.budget(), None);
        assert!(m.failure.as_deref().is_some_and(|f| f.contains("spawn")), "{:?}", m.failure);
    }

    /// Review finding 1. A cut reported past the 2,000-character preview is asked once more before it is believed, so
    /// one miscount cannot lower a budget; a host that really truncates in place says the same twice and is believed.
    #[test]
    fn measure_rechecks_a_cut_past_the_preview_before_believing_it() {
        // Haiku miscounts at 4,000 (`0030 NO`, where every marker and END arrived): re-asked, whole, budget intact.
        let mut fake = Fake::new(10_000);
        fake.garbled = vec!["0030 NO"];
        let (m, sizes) = measure(&mut fake);
        assert_eq!(&sizes[..2], [4000, 4000], "the cut past the preview was asked again");
        assert_eq!(m.budget(), Some(10_000));

        // A host that truncates at its limit instead of previewing: the high cut repeats, so it stands.
        let mut fake = Fake::new(10_000);
        fake.truncates = true;
        let (m, _) = measure(&mut fake);
        assert_eq!(m.budget(), Some(10_000), "{:?}", m.probes);
        assert!(m.probes.len() <= MAX_CALLS, "{} calls", m.probes.len());
    }

    /// Review finding 8. Once a size arrived whole and a larger one was cut, a reply that stays unreadable stops the
    /// narrowing but keeps what is known: the largest size delivered whole, with the reason on the row.
    #[test]
    fn measure_keeps_a_proven_size_when_narrowing_stops_early() {
        let mut fake = Fake::new(10_000);
        fake.fail_after = Some(4); // 4,000 whole, 8,000 whole, 16,000 cut, 12,000 cut, then nothing readable
        let (m, _) = measure(&mut fake);
        assert_eq!(m.failure, None);
        assert_eq!(m.budget(), Some(8000), "the largest size delivered whole");
        assert!(m.row().contains("stopped narrowing: the reply at 10,000 bytes was unreadable 2 times"), "{}", m.row());

        // With nothing delivered whole there is nothing to keep: the hook is not measured.
        let mut fake = Fake::new(10_000);
        fake.fail_after = Some(0);
        let (m, _) = measure(&mut fake);
        assert_eq!(m.budget(), None);
        assert!(m.failure.is_some());
    }

    /// The install template pins `session_start_bytes` in every new base.toml, so it must ship the measured value and
    /// the version beside it, or a fresh install reads as unmeasured to `base doctor`.
    #[test]
    fn measure_install_template_ships_the_measured_values() {
        let src = include_str!("install.rs");
        let bytes = format!("session_start_bytes = {}  #", crate::config::MEASURED_HOOK_BYTES);
        let on = format!("measured_on = \"{}\"", crate::config::MEASURED_ON);
        assert!(src.contains(&bytes), "the install template does not write {bytes:?}");
        assert!(src.contains(&on), "the install template does not write {on:?}");
    }

    #[test]
    fn measure_first_screen_answers_from_session_start() {
        let mut fake = Fake::new(10_000);
        let mut nonces = nonce;
        let m = search(&mut fake, Hook::SessionStart, &mut nonces, &mut |_| {});
        let line = first_screen(&m).expect("an answer");
        assert!(line.contains("past character 2,000"), "{line}");
        assert!(line.contains("10,000 bytes arrived whole, marker 0098 of 0098"), "{line}");

        // Nothing past the first screen arrived whole: the answer says how far the model got instead.
        let cut = HookMeasure {
            hook: Hook::SessionStart,
            probes: vec![Probe {
                bytes: 4000,
                nonce: "q7k2".into(),
                reply: "0019 NO".into(),
                verdict: Verdict::Cut { marker: 19 },
            }],
            failure: None,
            stopped: None,
        };
        let line = first_screen(&cut).expect("an answer");
        assert!(line.contains("only to marker 0019 of 0038, about character 1,900"), "{line}");
    }

    #[test]
    fn measure_writes_budget_and_measured_on() {
        let keys = [
            ("session_start_bytes", "10000".to_string()),
            ("prompt_bytes", "10000".to_string()),
            ("pre_tool_bytes", "10000".to_string()),
            ("measured_on", "\"2.1.287\"".to_string()),
        ];
        // An existing section: values replaced in place with their comments, the legacy spelling renamed, missing
        // keys after the last key line so the next section's heading comment stays with it, nothing else touched.
        let before = "# my config\n[namespace]\nname = \"x\" # keep\n\n# ─── [budget] ───\n[budget]\n\
session_start_bytes = 9000   # everything session start prints\nmemory_chars = 4000\nprompt_chars = 4000 # legacy\n\n\
# ─── [sync] — globs\n[sync]\ninclude = [\"**/*.md\"]\n";
        let after = set_budget_keys(before, &keys).expect("written");
        assert_eq!(
            after,
            "# my config\n[namespace]\nname = \"x\" # keep\n\n# ─── [budget] ───\n[budget]\n\
session_start_bytes = 10000   # everything session start prints\nmemory_chars = 4000\nprompt_bytes = 10000 # legacy\n\
pre_tool_bytes = 10000\nmeasured_on = \"2.1.287\"\n\n# ─── [sync] — globs\n[sync]\ninclude = [\"**/*.md\"]\n"
        );
        let cfg: crate::config::BaseConfig = toml::from_str(&after).expect("parses");
        assert_eq!(
            (cfg.budget.session_start_bytes, cfg.budget.prompt_bytes, cfg.budget.pre_tool_bytes, cfg.budget.memory_chars),
            (10_000, 10_000, 10_000, 4000)
        );
        assert_eq!(cfg.budget.measured_on, "2.1.287");

        // No section: one is appended after everything else, here after an array of tables, as Chris's file ends.
        let before = "[signal]\nenabled = true\n\n[[workspace]]\npath = \"C:/x\"\n";
        let after = set_budget_keys(before, &keys).expect("written");
        assert_eq!(
            after,
            "[signal]\nenabled = true\n\n[[workspace]]\npath = \"C:/x\"\n\n[budget]\nsession_start_bytes = 10000\n\
prompt_bytes = 10000\npre_tool_bytes = 10000\nmeasured_on = \"2.1.287\"\n"
        );
        let t: toml::Table = toml::from_str(&after).expect("parses");
        assert_eq!(t["workspace"].as_array().map(Vec::len), Some(1), "the array of tables is untouched");

        // CRLF stays CRLF; a value holding a # is not mistaken for a comment.
        let before = "[budget]\r\nmeasured_on = \"claude-code #2.1.278\" # old\r\n";
        let after = set_budget_keys(before, &keys[3..]).expect("written");
        assert_eq!(after, "[budget]\r\nmeasured_on = \"2.1.287\" # old\r\n");

        // An edit that cannot be made cleanly is refused, not written: a dotted key elsewhere already defines it.
        let before = "budget.prompt_bytes = 4000\n";
        assert!(set_budget_keys(before, &keys[1..2]).is_err());

        // A literal-quoted key is valid TOML and is the same key (review finding 9).
        let before = "[budget]\n'session_start_bytes' = 9000\n";
        assert_eq!(set_budget_keys(before, &keys[..1]).expect("written"), "[budget]\nsession_start_bytes = 10000\n");
    }

    #[test]
    fn measure_run_writes_the_users_base_toml_only_when_every_hook_measured() {
        let home = tempfile::tempdir().expect("tempdir");
        let gbl = home.path().join(".base-gbl");
        std::fs::create_dir_all(&gbl).expect("mkdir");
        let path = gbl.join("base.toml");
        let original = "# operator's notes\n[signal]\nenabled = true\n";
        std::fs::write(&path, original).expect("write");

        crate::home::with_thread_home(home.path(), || {
            let mut out = Vec::new();
            let got = run(home.path(), &mut Fake::new(10_000), "2.1.287", &mut out).expect("run");
            assert_eq!(got, Outcome::Written);
            let text = String::from_utf8(out).expect("utf8");
            assert!(text.starts_with("measuring on claude-code 2.1.287"), "{text}");
            assert!(text.contains("  prompt_bytes         (default "), "{text}");
            assert!(text.contains("-> 10000"), "{text}");
            assert!(text.contains("measured_on          (default "), "{text}");
            assert!(text.contains("first screen: the model saw session-start text past character 2,000"), "{text}");
            let written = std::fs::read_to_string(&path).expect("read");
            assert!(written.starts_with(original), "the operator's lines come first, unchanged: {written}");
            assert!(written.ends_with(
                "[budget]\nsession_start_bytes = 10000\nprompt_bytes = 10000\npre_tool_bytes = 10000\nmeasured_on = \"2.1.287\"\n"
            ), "{written}");

            // A hook that does not measure leaves the file exactly as it was, and ends the run: the two hooks after
            // it are never asked, because their results could only be thrown away (review finding 5).
            std::fs::write(&path, original).expect("reset");
            let mut fake = Fake::new(10_000);
            fake.garbled = vec!["?", "?"];
            let mut out = Vec::new();
            let got = run(home.path(), &mut fake, "2.1.287", &mut out).expect("run");
            assert_eq!(got, Outcome::NotWritten);
            assert_eq!(fake.calls, 2, "calls were spent after the first hook failed");
            assert_eq!(std::fs::read_to_string(&path).expect("read"), original);
            let text = String::from_utf8(out).expect("utf8");
            assert!(
                text.contains("nothing written") && text.contains("session-start, user-prompt-submit, pre-tool-use not measured"),
                "{text}"
            );

            // A workspace that sets measured_on overrides what was written, and the run says so (review finding 6).
            std::fs::write(&path, original).expect("reset");
            let ws = home.path().join("ws");
            std::fs::create_dir_all(ws.join(".base")).expect("ws");
            std::fs::write(ws.join(".base").join("base.toml"), "[budget]\nmeasured_on = \"claude-code 2.1.278\"\n")
                .expect("ws base.toml");
            let mut out = Vec::new();
            run(&ws, &mut Fake::new(10_000), "2.1.287", &mut out).expect("run");
            let text = String::from_utf8(out).expect("utf8");
            assert!(text.contains("measured_on is \"claude-code 2.1.278\": the workspace's base.toml sets it"), "{text}");

            // A file the writer would refuse is refused before a single call is spent on it.
            std::fs::write(&path, "budget.prompt_bytes = 4000\n").expect("write");
            let mut fake = Fake::new(10_000);
            assert!(run(home.path(), &mut fake, "2.1.287", &mut Vec::new()).is_err());
            assert_eq!(fake.calls, 0, "a call was spent on a file that cannot take the result");
        });
    }

    #[test]
    fn measure_settings_load_one_hook_and_nothing_else() {
        let s = settings(Hook::PreToolUse, "\"C:/b/base.exe\" doctor --measure emit");
        assert_eq!(s["hooks"].as_object().map(|o| o.len()), Some(1));
        assert_eq!(s["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        let s = settings(Hook::SessionStart, "x");
        assert!(s["hooks"]["SessionStart"][0].get("matcher").is_none());
        let args = claude_args(Hook::UserPromptSubmit, Path::new("s.json"));
        let at = args.iter().position(|a| a == "--setting-sources").expect("setting sources");
        assert_eq!(args[at + 1], "", "no user, project or local settings, so base's own hooks never load");
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        let at = args.iter().position(|a| a == "--tools").expect("tools");
        assert_eq!(args[at + 1], "", "no tools on session start and prompt submit");
        let args = claude_args(Hook::PreToolUse, Path::new("s.json"));
        assert!(args.windows(2).any(|w| w == ["--allowedTools", "Bash(echo measure)"]));
        assert!(args.windows(2).any(|w| w == ["--permission-mode", "dontAsk"]));
        for _ in 0..50 {
            let n = nonce();
            assert!(check_nonce(&n).is_ok(), "{n}");
        }
    }
}
