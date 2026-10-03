//! When the rule pass is due (BO-17: D5, D7, K4c), and the marks the hooks and `base tune` leave for each other.
//!
//! NO HOOK EVER CALLS THE LLM (K4c). The hooks only count and print one line; the AI runs `base tune` as a normal
//! command in its turn ([`super::tune_pass`]). Everything in this file is what a hook may do: read and write a few small
//! files under `~/.base-gbl/corrections/`.
//!
//! THE COUNTS live in the session's cursor file (BO-15's [`super::State`], its `tune` part), written only by the hooks,
//! where the correction signals are found: the prompt hook (C1, and the C2 of a turn that ended with no Stop) and the
//! Stop hook (C3). A flagged turn is one with any C1, C2 or C3 signal, a C3 `DEFERRED` alone excepted.
//!
//! THE PASS'S MARK is `<session>.tuned`, written only by `base tune` for each session it read: when, and how far. The
//! hooks read it on each prompt (one small file, or none) and start their counts again when a new pass read the session.
//! Two writers never share a file, so no write can lose another's.
//!
//! SESSION END (D7d) writes `<session>.ended` and nothing else. Session start (D7e) counts the earlier sessions that
//! ended, or sat untouched for a day, with flagged turns no pass has read.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::config::TuneConfig;
use crate::emit::match_log::Signal;

/// The prompt block that asks the AI to run `base tune`, as `base hooks show` names it.
pub const DUE_BLOCK: &str = "tune-due";
/// The session-start block for earlier sessions with unreviewed corrections.
pub const CATCH_UP_BLOCK: &str = "rule-pass";
/// The command both lines name.
pub const COMMAND: &str = "base tune";
/// Flagged turns kept per session, at most.
const MAX_FLAGGED: usize = 50;
/// A session untouched this long is abandoned: its corrections count for the catch-up line (D7e).
pub const IDLE: Duration = Duration::from_secs(24 * 60 * 60);

/// `~/.base-gbl/corrections`, when `~/.base-gbl` exists: the cursor files and each session's `.ended` and `.tuned`
/// marks. `None` without it: then nothing here counts or prints (the detector itself falls back to the cwd's tier).
pub fn dir() -> Option<PathBuf> {
    crate::home::home_root()
        .map(|h| h.join(".base-gbl"))
        .filter(|p| p.is_dir())
        .map(|g| g.join(super::STATE_DIR))
}

/// `~/.base-gbl/corrections/tune`: the pass's own files (its lock, cache, log, store-check time and saved cases). A
/// folder of its own, so the cursor-file prune never takes them.
pub fn own_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("tune"))
}

fn mark_path(session: &str, ext: &str) -> Option<PathBuf> {
    if !crate::crud::handoff_show::is_file_safe_session_id(session) {
        return None;
    }
    dir().map(|d| d.join(format!("{session}.{ext}")))
}

/// `<session>.tuned`: what the last pass that read this session judged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tuned {
    /// When that pass ran.
    pub at: String,
    /// The transcript's turns it judged, counting every prompt: the next pass starts after them.
    pub turns: u32,
    /// The last judged turn's number as the hooks count prompts.
    pub prompt: u32,
}

pub fn read_tuned(session: &str) -> Option<Tuned> {
    let path = mark_path(session, "tuned")?;
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Write `<session>.tuned` (temp file, then rename). Only `base tune` calls it.
pub fn write_tuned(session: &str, t: &Tuned) -> Result<(), String> {
    let path = mark_path(session, "tuned").ok_or_else(|| format!("no place for session {session}'s pass mark"))?;
    write_json(&path, t)
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(err)?;
    }
    let text = serde_json::to_string(value).map_err(|e| format!("{}: {e}", path.display()))?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).map_err(err)?;
    crate::store::rename_with_retry(&tmp, path).map_err(err)
}

/// The hooks' counts for one session, kept in its cursor file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marks {
    /// The `.tuned` time these counts start from.
    #[serde(default)]
    pub pass_seen: Option<String>,
    /// Prompts a person typed since then.
    #[serde(default)]
    pub prompts: u32,
    /// Turns flagged since then, by the hooks' prompt number.
    #[serde(default)]
    pub flagged: Vec<u32>,
    /// Typed prompts since then that matched no domain by keyword, path or star command.
    #[serde(default)]
    pub unmatched: u32,
    /// `prompts` when the due line last printed.
    #[serde(default)]
    pub shown_at: Option<u32>,
}

/// Why a pass is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// D7a: this many flagged corrections since the last pass.
    Corrections(u32),
    /// D7b: this many typed prompts with no pass, and some evidence among them.
    Prompts(u32),
}

impl Marks {
    /// A pass read this session since the hooks last looked: the counts start again, keeping the turns flagged after
    /// the last one it judged.
    pub fn sync_with_pass(&mut self, session: &str) {
        let Some(t) = read_tuned(session) else { return };
        if self.pass_seen.as_deref() == Some(t.at.as_str()) {
            return;
        }
        self.flagged.retain(|n| *n > t.prompt);
        self.prompts = 0;
        self.unmatched = 0;
        self.shown_at = None;
        self.pass_seen = Some(t.at);
    }

    /// Count `turn` as flagged, once.
    pub fn flag(&mut self, turn: u32) {
        if !self.flagged.contains(&turn) {
            self.flagged.push(turn);
            if self.flagged.len() > MAX_FLAGGED {
                self.flagged.remove(0);
            }
        }
    }

    /// Is a pass due on these counts, whether or not the line was shown. `[tune] corrections = 0` turns the evidence
    /// trigger off, `[tune] turns = 0` the safety net.
    pub fn due(&self, t: &TuneConfig) -> Option<Due> {
        let flagged = self.flagged.len() as u32;
        if t.corrections > 0 && flagged >= t.corrections {
            return Some(Due::Corrections(flagged));
        }
        if t.turns > 0 && self.prompts >= t.turns && (flagged > 0 || self.unmatched > 0) {
            return Some(Due::Prompts(self.prompts));
        }
        None
    }

    /// The line is due on this prompt: a pass is due and the line has not been shown since the pass, or was shown
    /// `[tune] turns` typed prompts ago (G0 question 7).
    pub fn line_due(&self, t: &TuneConfig) -> Option<Due> {
        let due = self.due(t)?;
        match self.shown_at {
            Some(at) if self.prompts.saturating_sub(at) < t.turns.max(1) => None,
            _ => Some(due),
        }
    }
}

/// A turn's signals flag it as a possible correction: any C1, C2 or C3 signal, a C3 `DEFERRED` alone excepted (the AI
/// held its position: a disagreement, not a missing rule).
pub fn flags(signals: &[Signal]) -> bool {
    signals.iter().any(|s| !(s.layer == "C3" && s.kind == "DEFERRED"))
}

/// `rule pass due: 3 corrections since the last one · run base tune`.
pub fn due_line(d: Due) -> String {
    match d {
        Due::Corrections(n) => format!(
            "rule pass due: {n} {} since the last one · run {COMMAND}",
            if n == 1 { "correction" } else { "corrections" }
        ),
        Due::Prompts(n) => format!("rule pass due: {n} prompts since the last one · run {COMMAND}"),
    }
}

// ─── Session end (D7d) ───────────────────────────────────────────────────────

/// `<session>.ended`: the session ended (Claude Code's SessionEnd), and no pass has read what it left.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ended {
    pub ts: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub transcript: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// `base hook session-end`: mark the session ended and do nothing else (a hook must be fast; the pass runs later, as a
/// command). `Ok(false)` when there is no session or no `~/.base-gbl` to mark it in.
pub fn mark_ended(event: &serde_json::Value, session: Option<&str>, cwd: &Path) -> Result<bool, String> {
    let Some(path) = session.and_then(|s| mark_path(s, "ended")) else { return Ok(false) };
    let field = |k: &str| event.get(k).and_then(serde_json::Value::as_str).map(String::from);
    let ended = Ended {
        ts: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        reason: field("reason"),
        transcript: field("transcript_path"),
        cwd: Some(cwd.display().to_string()),
    };
    write_json(&path, &ended)?;
    Ok(true)
}

pub fn read_ended(session: &str) -> Option<Ended> {
    let path = mark_path(session, "ended")?;
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

// ─── Catch-up at session start (D7e) ─────────────────────────────────────────

/// One session as the corrections folder lists it.
#[derive(Debug, Clone, Default)]
pub struct Listed {
    pub session: String,
    /// The cursor file's last write.
    pub state_at: Option<SystemTime>,
    pub ended: bool,
    /// The `.tuned` mark's last write.
    pub tuned_at: Option<SystemTime>,
}

impl Listed {
    /// Ended, or untouched for [`IDLE`].
    pub fn abandoned(&self, now: SystemTime) -> bool {
        self.ended || self.state_at.is_some_and(|m| now.duration_since(m).unwrap_or_default() > IDLE)
    }

    /// The hooks wrote after the last pass that read it, or no pass ever did.
    pub fn written_since_pass(&self) -> bool {
        match (self.state_at, self.tuned_at) {
            (Some(s), Some(t)) => s > t,
            (Some(_), None) => true,
            _ => false,
        }
    }
}

/// Every session the folder holds a cursor file or a mark for, from one listing.
pub fn list() -> Vec<Listed> {
    let Some(d) = dir() else { return Vec::new() };
    let mut by: std::collections::BTreeMap<String, Listed> = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((stem, ext)) = name.rsplit_once('.') else { continue };
        if !crate::crud::handoff_show::is_file_safe_session_id(stem) {
            continue;
        }
        let modified = entry.metadata().and_then(|m| m.modified()).ok();
        let l = by.entry(stem.to_string()).or_insert_with(|| Listed { session: stem.to_string(), ..Listed::default() });
        match ext {
            "json" => l.state_at = modified,
            "ended" => l.ended = true,
            "tuned" => l.tuned_at = modified,
            _ => {}
        }
    }
    by.into_values().filter(|l| l.state_at.is_some()).collect()
}

/// The flagged turns of a session's cursor file that no pass has read.
pub fn unpassed_flags(session: &str) -> Vec<u32> {
    let Some(path) = dir().map(|d| d.join(format!("{session}.json"))) else { return Vec::new() };
    let state = super::load_state(&path);
    let after = read_tuned(session).map(|t| t.prompt).unwrap_or(0);
    state.tune.flagged.into_iter().filter(|n| *n > after).collect()
}

/// How many earlier sessions (not `current`) ended or sat untouched for a day with flagged turns no pass has read. A
/// cursor file is opened only when its times say it may count.
pub fn catch_up(current: Option<&str>) -> usize {
    let now = SystemTime::now();
    list()
        .into_iter()
        .filter(|l| Some(l.session.as_str()) != current)
        .filter(|l| l.abandoned(now) && l.written_since_pass())
        .filter(|l| !unpassed_flags(&l.session).is_empty())
        .count()
}

/// `rule pass due: 2 earlier sessions have unreviewed corrections · run base tune`.
pub fn catch_up_line(n: usize) -> String {
    if n == 1 {
        format!("rule pass due: 1 earlier session has unreviewed corrections · run {COMMAND}")
    } else {
        format!("rule pass due: {n} earlier sessions have unreviewed corrections · run {COMMAND}")
    }
}

/// Remove `.ended` and `.tuned` marks not written for `days` days (`[log] prompt_days`), as the cursor files are.
pub fn prune_marks(days: u64) -> usize {
    let Some(d) = dir() else { return 0 };
    let keep = Duration::from_secs(days.max(1) * 24 * 60 * 60);
    let mut removed = 0;
    for entry in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let path = entry.path();
        let mark = path.extension().is_some_and(|x| x == "ended" || x == "tuned");
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > keep);
        if mark && old && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(corrections: u32, turns: u32) -> TuneConfig {
        TuneConfig { corrections, turns, ..TuneConfig::default() }
    }

    #[test]
    fn due_on_evidence_then_on_turns() {
        let t = cfg(3, 15);
        let mut m = Marks::default();
        for n in [4, 7] {
            m.flag(n);
        }
        assert_eq!(m.due(&t), None, "two corrections");
        m.flag(9);
        m.flag(9);
        assert_eq!(m.due(&t), Some(Due::Corrections(3)), "the same turn counts once");
        let m = Marks { prompts: 15, unmatched: 1, ..Marks::default() };
        assert_eq!(m.due(&t), Some(Due::Prompts(15)));
        let m = Marks { prompts: 15, ..Marks::default() };
        assert_eq!(m.due(&t), None, "15 prompts with no evidence");
        let m = Marks { prompts: 40, unmatched: 3, flagged: vec![1, 2, 3], ..Marks::default() };
        assert_eq!(m.due(&cfg(0, 0)), None, "both triggers off");
    }

    #[test]
    fn the_line_waits_a_full_count_after_it_was_shown() {
        let t = cfg(3, 15);
        let mut m = Marks { flagged: vec![1, 2, 3], prompts: 9, ..Marks::default() };
        assert!(m.line_due(&t).is_some());
        m.shown_at = Some(9);
        m.prompts = 10;
        assert_eq!(m.line_due(&t), None, "not on the next prompt");
        m.prompts = 24;
        assert!(m.line_due(&t).is_some(), "again after another 15 prompts with no pass");
    }

    #[test]
    fn lines_read_as_the_scope_gives_them() {
        assert_eq!(due_line(Due::Corrections(3)), "rule pass due: 3 corrections since the last one · run base tune");
        assert_eq!(due_line(Due::Prompts(15)), "rule pass due: 15 prompts since the last one · run base tune");
        assert_eq!(catch_up_line(2), "rule pass due: 2 earlier sessions have unreviewed corrections · run base tune");
        assert_eq!(catch_up_line(1), "rule pass due: 1 earlier session has unreviewed corrections · run base tune");
    }

    #[test]
    fn deferred_alone_is_not_a_flag() {
        let s = |l: &str, k: &str| Signal { layer: l.into(), kind: k.into(), value: None };
        assert!(!flags(&[s("C3", "DEFERRED")]));
        assert!(flags(&[s("C3", "DEFERRED"), s("C1", "phrase")]));
        assert!(flags(&[s("C2", "interrupt")]));
        assert!(!flags(&[]));
    }
}
