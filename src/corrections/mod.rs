//! Corrections (BO-15: K3, C1 to C4, C3 for every user, D4, D10): base notices when the user corrects the AI, and
//! turns each correction into a rule proposal.
//!
//! NO ONE SIGNAL DECIDES (D4). Chris: "sometimes you being corrected might not actually trigger the thing that needs
//! to be triggered for that correction." So four layers each flag a turn, and none of them decides:
//!
//! | layer | what flags a turn | where it is read |
//! |---|---|---|
//! | C1 | a phrase in the prompt (`[corrections] phrases`) | the prompt hook |
//! | C2 | the user stopped the turn, refused a tool call, changed a file the AI wrote after its turn, or sent the same request again | the prompt hook and the Stop hook |
//! | C3 | the AI's own marker at the start of a reply line (`UPDATED:`, `CORRECTED:`, `MISREAD:`, `DEFERRED:`) | the Stop hook, and the prompt hook for what Stop could not read |
//! | C4 | C1 on this prompt, or a C2 not yet answered: the prompt carries one line asking the AI to run `base rule propose --from-turn` if it was a correction | the prompt hook |
//!
//! Every flag is a row in the match log (`event: "signal"`), so BO-17 (the backstop) and BO-19 (doctor) can count.
//!
//! WHY C2 IS ALSO READ AT THE PROMPT. Claude Code runs no Stop hook after the user interrupts a turn or refuses a tool
//! call: across Chris's 222 transcripts from 2026-09-23 on, a Stop came before the next prompt after 0 of 36
//! interrupts and 0 of 21 refusals. A detector that read only at Stop would see Example 4's interrupt one turn late.
//! So one reader with a cursor per session runs at a turn's end AND when the next prompt arrives: every transcript
//! line is read once, by whichever comes first, and what the prompt hook reads belongs to the turn before it.
//!
//! THE CURSOR BELONGS TO THE SESSION, NOT A TIER: `~/.base-gbl/corrections/<session>.json`. A session that changes its
//! cwd into another workspace keeps one cursor and never reads a line twice.

pub mod claude_md;
pub mod markers;
pub mod phrases;
pub mod propose;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{BaseConfig, CorrectionsConfig};
use crate::domain::transcript::{self, Event};
use crate::emit::match_log::{self, Item, Row, Signal};
use crate::emit::prompt::{Priority, PromptBlock};

/// The C4 block's name, as `base hooks show` and the match log name it.
pub const CHECK_BLOCK: &str = "correction-check";
/// C4, word for word as the scope gives it.
pub const CHECK_LINE: &str = "This may be a correction. If it is, run base rule propose --from-turn after answering.";

/// The folder under `~/.base-gbl` that holds one cursor file per session.
const STATE_DIR: &str = "corrections";
/// The files a turn wrote that are watched until the next prompt, at most.
const MAX_WRITTEN: usize = 50;

/// A session's reading of its own transcript, kept between hook runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// The transcript the hooks were given.
    #[serde(default)]
    pub transcript: Option<String>,
    /// The byte offset of the next line to read. `None` until the first read.
    #[serde(default)]
    pub offset: Option<u64>,
    /// The last human prompt: its number, the hashes of its words, a hash of its text. No text.
    #[serde(default)]
    pub last_human: Option<LastHuman>,
    /// Files the AI wrote in the turn running now (Write, Edit, MultiEdit, NotebookEdit).
    #[serde(default)]
    pub turn_writes: Vec<String>,
    /// Those files as the turn's Stop left them, checked again by the next prompt.
    #[serde(default)]
    pub written: Vec<Written>,
    /// C2 signals no C4 line has answered yet.
    #[serde(default)]
    pub pending: Vec<Signal>,
    /// C3 kinds already counted, per turn, so a marker read at Stop and again later counts once.
    #[serde(default)]
    pub c3_seen: Vec<(u32, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastHuman {
    pub num: u32,
    pub words: Vec<u64>,
    pub text_hash: String,
}

/// A file the AI wrote, as its turn's Stop left it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Written {
    pub path: String,
    /// `None` when the file was not there.
    pub len: Option<u64>,
    pub modified_ns: Option<u64>,
}

impl Written {
    fn now(path: &str) -> Self {
        let meta = std::fs::metadata(path).ok();
        Written {
            path: path.to_string(),
            len: meta.as_ref().map(std::fs::Metadata::len),
            modified_ns: meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64),
        }
    }

    /// The file is not as Stop left it: changed, created or removed since.
    fn changed(&self) -> bool {
        Written::now(&self.path) != *self
    }
}

impl State {
    /// Count a C3 kind for `turn` once.
    fn c3_new(&mut self, turn: u32, kind: &str) -> bool {
        if self.c3_seen.iter().any(|(t, k)| *t == turn && k == kind) {
            return false;
        }
        self.c3_seen.push((turn, kind.to_string()));
        true
    }

    /// Forget C3 kinds of turns more than two back.
    fn forget_old_c3(&mut self, turn: u32) {
        self.c3_seen.retain(|(t, _)| t + 2 >= turn);
    }
}

/// Where a session's state is kept: `~/.base-gbl/corrections/<session>.json`, or, with no `~/.base-gbl`, the session's
/// own folder in the cwd's tier. `None` for an id that cannot be a file name.
pub fn state_path(cwd: &Path, session: &str) -> Option<PathBuf> {
    if !crate::crud::handoff_show::is_file_safe_session_id(session) {
        return None;
    }
    if let Some(gbl) = crate::home::home_root().map(|h| h.join(".base-gbl")).filter(|p| p.is_dir()) {
        return Some(gbl.join(STATE_DIR).join(format!("{session}.json")));
    }
    let dir = crate::crud::handoff_show::session_start_dir(cwd)?;
    crate::emit::session_files::session_dir(&dir, session).map(|d| d.join("corrections.json"))
}

pub fn load_state(path: &Path) -> State {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_state(path: &Path, state: &State) -> Result<(), String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let text = serde_json::to_string(state).map_err(|e| format!("{}: {e}", path.display()))?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).map_err(err)?;
    crate::store::rename_with_retry(&tmp, path).map_err(err)
}

/// Remove state files not written for `days` days (`[log] prompt_days`, read as at least 1). Session start runs it.
pub fn prune_state(days: u64) -> usize {
    let Some(dir) = crate::home::home_root().map(|h| h.join(".base-gbl").join(STATE_DIR)) else {
        return 0;
    };
    let keep = std::time::Duration::from_secs(days.max(1) * 24 * 60 * 60);
    let mut removed = 0;
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > keep);
        if old && path.extension().is_some_and(|x| x == "json") && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Push `signal` unless one of the same layer and kind is there.
fn push_once(list: &mut Vec<Signal>, signal: Signal) {
    if !list.iter().any(|s| s.layer == signal.layer && s.kind == signal.kind) {
        list.push(signal);
    }
}

/// What the prompt hook's corrections step found, held until the output is fitted.
pub struct PromptCheck {
    /// The C4 line, when this prompt gets one.
    pub block: Option<PromptBlock>,
    state: State,
    path: PathBuf,
    rows: Vec<Row>,
    /// Every unanswered C2 signal, this prompt's included: kept for the next prompt unless the C4 line prints.
    pending: Vec<Signal>,
    row_dir: Option<PathBuf>,
}

impl PromptCheck {
    /// Save the state and write the signal rows, after the print. `printed`: the C4 block was in what printed, which
    /// answers every pending C2 signal (D15's rule, as the relay blocks follow it).
    pub fn commit(mut self, printed: bool) {
        self.state.pending = if printed && self.block.is_some() { Vec::new() } else { std::mem::take(&mut self.pending) };
        if let Err(why) = save_state(&self.path, &self.state) {
            eprintln!("base: the prompt hook could not keep its corrections state: {why}");
        }
        if let Some(dir) = &self.row_dir {
            for row in &self.rows {
                if let Err(why) = match_log::append(dir, row) {
                    eprintln!("base: the prompt hook could not write a correction signal row: {why}");
                }
            }
        }
    }
}

/// The prompt hook's corrections step (C1, C2, C4, and C3 for a turn whose Stop did not read it). `prompt_num` is this
/// prompt's number, as the hook counted it. `None` when corrections are off or the session is unknown.
pub fn on_prompt(
    config: &BaseConfig,
    cwd: &Path,
    event: &serde_json::Value,
    session: Option<&str>,
    prompt_num: Option<u32>,
    prompt: &str,
) -> Option<PromptCheck> {
    let cc = &config.corrections;
    if !cc.enabled {
        return None;
    }
    let (session, num) = (session?, prompt_num?);
    let path = state_path(cwd, session)?;
    let mut state = load_state(&path);
    // What the session wrote since the last read belongs to the turn before this prompt: an interrupted or refused
    // turn ends with no Stop, and Stop can run before the turn's last lines are on disk.
    let prev = num.saturating_sub(1).max(1);
    let mut prev_signals: Vec<Signal> = Vec::new();
    let transcript_path = event
        .get("transcript_path")
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .or_else(|| state.transcript.clone());
    if let Some(tp) = transcript_path {
        let from = state.offset.unwrap_or_else(|| transcript::first_offset(Path::new(&tp)));
        if let Ok(stretch) = transcript::read_events(Path::new(&tp), from) {
            state.offset = Some(stretch.next);
            for e in &stretch.events {
                match e {
                    Event::Interrupt => push_once(&mut prev_signals, Signal::new("C2", "interrupt", None)),
                    Event::Denial => push_once(&mut prev_signals, Signal::new("C2", "denial", None)),
                    Event::Text(t) => {
                        for f in markers::find(t, &cc.markers) {
                            if state.c3_new(prev, &f.kind) {
                                prev_signals.push(Signal::new("C3", &f.kind, Some(&f.line)));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        state.transcript = Some(tp);
    }
    // C2: a file the AI wrote last turn changed after that turn's Stop.
    for w in std::mem::take(&mut state.written) {
        if w.changed() {
            prev_signals.push(Signal::new("C2", "file-edited", Some(&w.path)));
        }
    }
    state.turn_writes.clear();

    // This prompt: C1 and the repeat check, for a prompt a person typed and not a session's first.
    let mut now_signals: Vec<Signal> = Vec::new();
    let human = !transcript::machine_prompt(prompt);
    let mut c1 = false;
    if human {
        let words = phrases::word_hashes(prompt);
        if let Some(last) = &state.last_human {
            let hit = phrases::matched(prompt, &cc.phrases);
            if !hit.is_empty() {
                c1 = true;
                now_signals.push(Signal::new("C1", "phrase", Some(&hit.join(", "))));
            }
            if let Some(sim) = phrases::similarity(&last.words, &words)
                && sim >= cc.repeat_similarity
            {
                now_signals.push(Signal::new("C2", "repeat", Some(&format!("{sim:.2}"))));
            }
        }
        state.last_human = Some(LastHuman { num, words, text_hash: phrases::text_hash(prompt) });
    }

    let mut pending = std::mem::take(&mut state.pending);
    for s in prev_signals.iter().chain(now_signals.iter()).filter(|s| s.layer == "C2") {
        pending.push(s.clone());
    }
    // A task notification never carries the line: it is not the user, and the signals wait for the user's next prompt.
    let block = (human && (c1 || !pending.is_empty())).then(|| {
        PromptBlock::new(CHECK_BLOCK, Priority::Matched, CHECK_LINE, 1, "line").with_logged([Item::of_kind(CHECK_BLOCK, "check")])
    });
    let mut rows = Vec::new();
    if !prev_signals.is_empty() {
        rows.push(match_log::signal_row(Some(session), Some(prev), prev_signals));
    }
    if !now_signals.is_empty() {
        rows.push(match_log::signal_row(Some(session), Some(num), now_signals));
    }
    state.forget_old_c3(num);
    Some(PromptCheck { block, state, path, rows, pending, row_dir: crate::crud::handoff_show::session_start_dir(cwd) })
}

/// The Stop hook's corrections step: C3 in what the turn's AI wrote (the transcript, and the payload's
/// `last_assistant_message`), C2 if the turn held one, and the files the turn wrote, recorded for the next prompt.
/// Fail-open: every error goes to stderr.
pub fn on_stop(config: &BaseConfig, cwd: &Path, event: &serde_json::Value, session: Option<&str>) {
    let cc = &config.corrections;
    if !cc.enabled {
        return;
    }
    let Some(session) = session else { return };
    let Some(path) = state_path(cwd, session) else { return };
    let mut state = load_state(&path);
    let num = current_prompt(cwd, session).max(state.last_human.as_ref().map(|l| l.num).unwrap_or(0)).max(1);
    let mut signals: Vec<Signal> = Vec::new();
    let transcript_path = event
        .get("transcript_path")
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .or_else(|| state.transcript.clone());
    if let Some(tp) = transcript_path {
        let from = state.offset.unwrap_or_else(|| transcript::first_offset(Path::new(&tp)));
        match transcript::read_events(Path::new(&tp), from) {
            Ok(stretch) => {
                state.offset = Some(stretch.next);
                for e in stretch.events {
                    match e {
                        Event::Interrupt => push_once(&mut signals, Signal::new("C2", "interrupt", None)),
                        Event::Denial => push_once(&mut signals, Signal::new("C2", "denial", None)),
                        Event::Text(t) => {
                            for f in markers::find(&t, &cc.markers) {
                                if state.c3_new(num, &f.kind) {
                                    signals.push(Signal::new("C3", &f.kind, Some(&f.line)));
                                }
                            }
                        }
                        Event::Wrote(f) => {
                            if !state.turn_writes.contains(&f) && state.turn_writes.len() < MAX_WRITTEN {
                                state.turn_writes.push(f);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => eprintln!("base: the Stop hook could not read the transcript {tp}: {e}"),
        }
        state.transcript = Some(tp);
    }
    // The payload's last reply: the transcript can trail the hook by its last lines.
    if let Some(last) = event.get("last_assistant_message").and_then(serde_json::Value::as_str) {
        for f in markers::find(last, &cc.markers) {
            if state.c3_new(num, &f.kind) {
                signals.push(Signal::new("C3", &f.kind, Some(&f.line)));
            }
        }
    }
    for s in signals.iter().filter(|s| s.layer == "C2") {
        state.pending.push(s.clone());
    }
    // As the turn leaves them: a later Stop in the same turn (a Stop hook that blocked) records them again.
    state.written = state.turn_writes.iter().map(|f| Written::now(f)).collect();
    state.forget_old_c3(num);
    if let Err(why) = save_state(&path, &state) {
        eprintln!("base: the Stop hook could not keep its corrections state: {why}");
    }
    if !signals.is_empty()
        && let Some(dir) = crate::crud::handoff_show::session_start_dir(cwd)
        && let Err(why) = match_log::append(&dir, &match_log::signal_row(Some(session), Some(num), signals))
    {
        eprintln!("base: the Stop hook could not write a correction signal row: {why}");
    }
}

/// This session's prompt count, as the prompt hook keeps it.
fn current_prompt(cwd: &Path, session: &str) -> u32 {
    crate::config::find_workspace_base(cwd)
        .or_else(|| crate::config::global_base_dir().filter(|d| d.is_dir()))
        .map(|dir| crate::domain::session::SessionState::load(&dir).prompt_count_for(Some(session)))
        .unwrap_or(0)
}

// ─── A whole transcript, turn by turn ────────────────────────────────────────

/// One prompt's turn, read from a whole transcript: the prompt and everything up to the next prompt.
#[derive(Debug, Clone)]
pub struct Turn {
    /// 1 for the transcript's first prompt, counting every prompt (the prompt hook counts them all).
    pub num: u32,
    /// 1 for the first prompt a person typed; `None` for a task notification.
    pub human_num: Option<u32>,
    pub prompt: String,
    pub events: Vec<Event>,
    /// The signals that belong to this turn: C1 and repeat on its prompt; C2 and C3 in what followed it; C2
    /// file-edited when a file this turn wrote changed before the next turn's reply began.
    pub signals: Vec<Signal>,
    /// The prompt hook would print the C4 line on this prompt.
    pub check: bool,
}

/// The turns of `events` (a whole transcript, [`transcript::read_all`]) with their signals, as the hooks would have
/// found them. One difference, named wherever this is printed: a file the AI wrote that changed before the next reply
/// is read from Claude Code's `edited_text_file` line, since the hooks' size-and-time check needs the file as it was.
pub fn turns(events: Vec<Event>, cc: &CorrectionsConfig) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    let mut human_count = 0u32;
    for e in events {
        match e {
            Event::Prompt { human, text } => {
                let human_num = human.then(|| {
                    human_count += 1;
                    human_count
                });
                out.push(Turn {
                    num: out.len() as u32 + 1,
                    human_num,
                    prompt: text,
                    events: Vec::new(),
                    signals: Vec::new(),
                    check: false,
                });
            }
            other => {
                if let Some(t) = out.last_mut() {
                    t.events.push(other);
                }
            }
        }
    }
    let mut last_human_words: Option<Vec<u64>> = None;
    let mut pending: Vec<Signal> = Vec::new();
    for i in 0..out.len() {
        let mut signals: Vec<Signal> = Vec::new();
        let human = out[i].human_num.is_some();
        let mut c1 = false;
        if human {
            let words = phrases::word_hashes(&out[i].prompt);
            if let Some(last) = &last_human_words {
                let hit = phrases::matched(&out[i].prompt, &cc.phrases);
                if !hit.is_empty() {
                    c1 = true;
                    signals.push(Signal::new("C1", "phrase", Some(&hit.join(", "))));
                }
                if let Some(sim) = phrases::similarity(last, &words)
                    && sim >= cc.repeat_similarity
                {
                    signals.push(Signal::new("C2", "repeat", Some(&format!("{sim:.2}"))));
                }
            }
            last_human_words = Some(words);
        }
        for e in &out[i].events {
            match e {
                Event::Interrupt => push_once(&mut signals, Signal::new("C2", "interrupt", None)),
                Event::Denial => push_once(&mut signals, Signal::new("C2", "denial", None)),
                Event::Text(t) => {
                    for f in markers::find(t, &cc.markers) {
                        if !signals.iter().any(|s| s.layer == "C3" && s.kind == f.kind) {
                            signals.push(Signal::new("C3", &f.kind, Some(&f.line)));
                        }
                    }
                }
                _ => {}
            }
        }
        // A file the turn before wrote, changed between that turn and this one's first reply.
        if i > 0 {
            let wrote: Vec<String> = out[i - 1]
                .events
                .iter()
                .filter_map(|e| match e {
                    Event::Wrote(f) => Some(same_path_key(f)),
                    _ => None,
                })
                .collect();
            let mut edited: Vec<Signal> = Vec::new();
            for e in &out[i].events {
                match e {
                    Event::FileChanged(f) if wrote.contains(&same_path_key(f)) => {
                        if !edited.iter().any(|s| s.value.as_deref() == Some(f.as_str())) {
                            edited.push(Signal::new("C2", "file-edited", Some(f)));
                        }
                    }
                    Event::Text(_) | Event::Wrote(_) => break,
                    _ => {}
                }
            }
            for s in &edited {
                pending.push(s.clone());
            }
            out[i - 1].signals.extend(edited);
        }
        // The previous turn's C2 (an interrupt or a refusal ends a turn with no Stop) waits for this prompt.
        if i > 0 {
            for s in out[i - 1].signals.iter().filter(|s| s.layer == "C2" && s.kind != "repeat" && s.kind != "file-edited") {
                pending.push(s.clone());
            }
        }
        for s in signals.iter().filter(|s| s.layer == "C2" && s.kind == "repeat") {
            pending.push(s.clone());
        }
        let check = human && (c1 || !pending.is_empty());
        if check {
            pending.clear();
        }
        out[i].signals.extend(signals);
        out[i].check = check;
    }
    out
}

/// A path as two writes of one file compare: separators one way, case ignored on Windows.
fn same_path_key(p: &str) -> String {
    let p = p.replace('\\', "/");
    if cfg!(windows) { p.to_lowercase() } else { p }
}

/// `C1 phrase "quit" · C3 UPDATED`.
pub fn labels(signals: &[Signal]) -> String {
    signals.iter().map(Signal::label).collect::<Vec<_>>().join(" · ")
}

/// One turn as `base log corrections --transcript --json` prints it.
pub fn turn_json(t: &Turn) -> serde_json::Value {
    serde_json::json!({
        "turn": t.num,
        "human_turn": t.human_num,
        "prompt": clip(&crate::scrub::scrub(&t.prompt), 120),
        "signals": t.signals,
        "check": t.check,
    })
}

fn clip(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    format!("{}...", one.chars().take(max).collect::<String>().trim_end())
}

/// Counts over a transcript's turns, for the summary line and BO-15's gate 4.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tally {
    pub turns: usize,
    pub human: usize,
    /// Turns flagged by each layer.
    pub c1: usize,
    pub c2: usize,
    pub c3: usize,
    /// Turns per C2 and C3 kind, sorted by kind.
    pub kinds: Vec<(String, usize)>,
    /// Turns whose reply carried a marker that C3 counts as a correction (not DEFERRED), and how many of those the
    /// detector had flagged by the time the prompt arrived (the C4 line printed on it).
    pub c3_corrections: usize,
    pub c3_caught: usize,
    pub checks: usize,
}

pub fn tally(turns: &[Turn]) -> Tally {
    let mut t = Tally { turns: turns.len(), ..Tally::default() };
    let mut kinds: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for turn in turns {
        t.human += usize::from(turn.human_num.is_some());
        t.checks += usize::from(turn.check);
        let has = |layer: &str| turn.signals.iter().any(|s| s.layer == layer);
        t.c1 += usize::from(has("C1"));
        t.c2 += usize::from(has("C2"));
        t.c3 += usize::from(has("C3"));
        let mut seen: Vec<String> = Vec::new();
        for s in turn.signals.iter().filter(|s| s.layer != "C1") {
            let k = format!("{} {}", s.layer, s.kind);
            if !seen.contains(&k) {
                seen.push(k.clone());
                *kinds.entry(k).or_default() += 1;
            }
        }
        if turn.signals.iter().any(|s| s.layer == "C3" && s.kind != "DEFERRED") {
            t.c3_corrections += 1;
            t.c3_caught += usize::from(turn.check);
        }
    }
    t.kinds = kinds.into_iter().collect();
    t
}

/// `base log corrections --transcript`: each flagged turn, then the counts.
pub fn render_turns(turns: &[Turn], path: &str) -> String {
    let mut s = String::new();
    for t in turns.iter().filter(|t| !t.signals.is_empty() || t.check) {
        let who = match t.human_num {
            Some(n) => format!("human prompt {n}"),
            None => "task notification".to_string(),
        };
        let check = if t.check { " · check line" } else { "" };
        s.push_str(&format!("turn {} ({who}) \"{}\": {}{check}\n", t.num, clip(&t.prompt, 60), labels(&t.signals)));
    }
    let n = tally(turns);
    let kinds: Vec<String> = n.kinds.iter().map(|(k, c)| format!("{k} {c}")).collect();
    s.push_str(&format!(
        "{path}: {} turns ({} typed by a person) · flagged by C1 {}, C2 {}, C3 {} · {} · C3 corrections the detector \
         flagged at the prompt: {} of {} · check lines: {}\n",
        n.turns,
        n.human,
        n.c1,
        n.c2,
        n.c3,
        if kinds.is_empty() { "no C2 or C3 kinds".to_string() } else { kinds.join(", ") },
        n.c3_caught,
        n.c3_corrections,
        n.checks
    ));
    s.push_str("(read from the transcript: a file changed after the AI wrote it is Claude Code's edited_text_file line here, \
                where the hooks check the file's size and time)\n");
    s
}
