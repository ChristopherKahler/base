//! `base tune` (BO-17: K4a, K4b, C5, D7c): the rule pass. It reads the sessions since the last pass and writes rule
//! proposals into BO-15's store for BO-16's review. Nothing is applied.
//!
//! WHAT IT READS (G0 question 1). The session running it, its last turn left out (the one in progress); sessions that
//! ended or sat untouched for a day with turns no pass has read; other live sessions only when they are due
//! themselves. `--session` and `--transcript` name sessions instead. Each session's transcript is read turn by turn as
//! the hooks read it (`corrections::turns`), only past the turns its last pass judged (`<session>.tuned`).
//!
//! ONE HAIKU CALL PER SESSION (K4b). `claude-haiku-4-5` through `llm`, the prompt on stdin, isolated as `doctor
//! --measure` is (no settings, no MCP servers, no tools), with BO-08's marker so base's own hooks stay silent inside
//! it. It says which typed prompts corrected the AI and what rule would have prevented it, which flagged prompts were
//! not corrections, and which domain a prompt that matched none belongs to. Answers are cached by the SHA-256 of model
//! and prompt, so a rerun on the same turns costs nothing. Once a day (or with `--store`) one more call judges the
//! store: rules to merge, rules to split, keywords that bring a domain where it does not belong. A rule that no row of a
//! 30-day match log ever served is proposed for retirement with no call.
//!
//! WHAT IT WRITES. Proposals, pending, each with its evidence: a judged correction goes through BO-15's own sort (a
//! keyword gap, a rewrite or a new rule) with the judge's wording and keywords; the rest are this pass's kinds (keyword
//! gap from prompts that matched no domain, drop keyword, merge, split, retire). A correction the detector did not flag
//! is saved as a detector case (C5). The pass's counts go to `tune/log.jsonl`, and each session read gets its mark.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::BaseConfig;
use crate::corrections::{self, propose, tune, Turn};
use crate::domain::transcript::{self, Event};
use crate::emit::match_log::Signal;

/// The judge (Round 2: Haiku off the hot path).
pub const MODEL: &str = "claude-haiku-4-5";
/// One call's time limit.
pub const CALL_LIMIT: Duration = Duration::from_secs(300);
/// Calls one pass makes at most, unless `--max-calls` says otherwise (G0 question 11).
pub const DEFAULT_MAX_CALLS: usize = 10;
/// A session's turns go to the judge in chunks of at most this many characters.
const CHUNK_CHARS: usize = 60_000;
const BEFORE_CHARS: usize = 400;
const PROMPT_CHARS: usize = 1_000;
const AFTER_CHARS: usize = 300;

/// What `base tune` was given.
#[derive(Debug, Clone, Default)]
pub struct Args {
    pub dry_run: bool,
    pub sessions: Vec<String>,
    pub transcripts: Vec<String>,
    pub store: bool,
    pub max_calls: usize,
}

// ─── The judge ───────────────────────────────────────────────────────────────

/// Who answers a prompt. The real one is [`Haiku`]; the tests drive the binary with `BASE_LLM_FAKE`.
pub trait Judge {
    fn ask(&mut self, prompt: &str) -> Result<String, String>;
}

/// Headless Claude Code on Haiku, in a scratch folder of its own, loading nothing but its login.
pub struct Haiku {
    scratch: PathBuf,
}

impl Haiku {
    pub fn new() -> Self {
        let scratch = std::env::temp_dir().join(format!("base-tune-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&scratch);
        Haiku { scratch }
    }
}

impl Default for Haiku {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Haiku {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// The isolation `doctor --measure` uses: no user, project or local settings (so no hook of the user's runs inside the
/// call), no MCP servers, no session file, no tools.
pub fn isolation() -> Vec<String> {
    ["--setting-sources", "", "--strict-mcp-config", "--no-session-persistence", "--tools", ""].iter().map(|s| s.to_string()).collect()
}

impl Judge for Haiku {
    fn ask(&mut self, prompt: &str) -> Result<String, String> {
        crate::llm::complete_stdin(prompt, Some(MODEL), &isolation(), Some(&self.scratch), CALL_LIMIT).map_err(|e| format!("{e:#}"))
    }
}

fn cache_path(prompt: &str) -> Option<PathBuf> {
    let key: String = Sha256::digest(format!("{MODEL}\n{prompt}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    tune::own_dir().map(|d| d.join("cache").join(format!("{key}.txt")))
}

fn cached(prompt: &str) -> Option<String> {
    std::fs::read_to_string(cache_path(prompt)?).ok()
}

pub(crate) fn keep_cached(prompt: &str, answer: &str) {
    if let Some(p) = cache_path(prompt) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, answer);
    }
}

/// Remove cached answers not used for `days` days.
fn prune_cache(days: u64) {
    let Some(d) = tune::own_dir().map(|d| d.join("cache")) else { return };
    let keep = Duration::from_secs(days.max(1) * 24 * 60 * 60);
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        if e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > keep) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The JSON object in a judge's answer: from its first `{` to its last `}` (a model may wrap it in a code fence).
pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(answer: &str) -> Option<T> {
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    (end > start).then(|| serde_json::from_str(&answer[start..=end]).ok()).flatten()
}

/// The calls a pass makes, and its budget.
pub(crate) struct Calls<'j> {
    judge: &'j mut dyn Judge,
    made: usize,
    cached: usize,
    pub(crate) max: usize,
    dry_run: bool,
    /// In a dry run: the calls it would make.
    would: usize,
}

impl Calls<'_> {
    /// Calls left in the budget for `n` more prompts, cached ones free.
    pub(crate) fn can(&self, prompts: &[String]) -> bool {
        let fresh = prompts.iter().filter(|p| cached(p).is_none()).count();
        self.made + self.would + fresh <= self.max
    }

    /// Ask, or answer from the cache. `None` in a dry run (counted).
    pub(crate) fn ask(&mut self, prompt: &str) -> Option<Result<String, String>> {
        if let Some(a) = cached(prompt) {
            self.cached += 1;
            return Some(Ok(a));
        }
        if self.dry_run {
            self.would += 1;
            return None;
        }
        self.made += 1;
        Some(self.judge.ask(prompt))
    }
}

// ─── Which sessions ──────────────────────────────────────────────────────────

/// A session the pass reads.
#[derive(Debug, Clone)]
struct Picked {
    session: String,
    transcript: PathBuf,
    running: bool,
    flagged: usize,
    state_at: Option<SystemTime>,
}

/// The sessions to read (G0 question 1), the running one first, then the most flagged, then the oldest; and notes.
fn pick(config: &BaseConfig, cwd: &Path, args: &Args, running: Option<&str>) -> (Vec<Picked>, Vec<String>) {
    let mut out: Vec<Picked> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for t in &args.transcripts {
        let path = PathBuf::from(t);
        let Some(sid) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else {
            notes.push(format!("{t}: not a transcript file name"));
            continue;
        };
        if !path.is_file() {
            notes.push(format!("{t}: no such transcript"));
            continue;
        }
        out.push(Picked { session: sid, transcript: path, running: false, flagged: 0, state_at: None });
    }
    for s in &args.sessions {
        match propose::transcript_for(cwd, None, Some(s)) {
            Ok(path) => out.push(Picked {
                session: s.clone(),
                transcript: path,
                running: Some(s.as_str()) == running,
                flagged: 0,
                state_at: None,
            }),
            Err(e) => notes.push(e),
        }
    }
    if !args.transcripts.is_empty() || !args.sessions.is_empty() {
        return (out, notes);
    }
    let now = SystemTime::now();
    for l in tune::list() {
        let Some(path) = tune::dir().map(|d| d.join(format!("{}.json", l.session))) else { continue };
        let state = corrections::load_state(&path);
        let Some(tp) = state.transcript.clone().map(PathBuf::from).filter(|p| p.is_file()) else { continue };
        let is_running = Some(l.session.as_str()) == running;
        let mut marks = state.tune.clone();
        marks.sync_with_pass(&l.session);
        let due_itself = marks.due(&config.tune).is_some();
        if is_running || (l.written_since_pass() && (l.abandoned(now) || due_itself)) {
            out.push(Picked {
                session: l.session.clone(),
                transcript: tp,
                running: is_running,
                flagged: marks.flagged.len(),
                state_at: l.state_at,
            });
        }
    }
    out.sort_by(|a, b| b.running.cmp(&a.running).then(b.flagged.cmp(&a.flagged)).then(a.state_at.cmp(&b.state_at)));
    (out, notes)
}

// ─── One session, read ───────────────────────────────────────────────────────

/// A session's turns and the stretch of them this pass judges.
struct Read {
    picked: Picked,
    turns: Vec<Turn>,
    /// Indexes into `turns`: `from..to` is judged.
    from: usize,
    to: usize,
    /// Add to a transcript turn number to get the hooks' prompt number (they agree for a session that had base from
    /// its first prompt).
    offset: i64,
}

impl Read {
    fn hooks_num(&self, t: &Turn) -> u32 {
        (i64::from(t.num) + self.offset).max(1) as u32
    }

    /// The typed turns judged.
    fn typed(&self) -> Vec<usize> {
        (self.from..self.to).filter(|i| self.turns[*i].human_num.is_some()).collect()
    }

    fn short(&self) -> String {
        self.picked.session.chars().take(8).collect()
    }
}

fn read(config: &BaseConfig, p: Picked) -> Result<Read, String> {
    let events = transcript::read_all(&p.transcript).map_err(|e| format!("{}: {e}", p.transcript.display()))?;
    let turns = corrections::turns(events, &config.corrections);
    let tuned = tune::read_tuned(&p.session);
    let from = tuned.as_ref().map(|t| t.turns as usize).unwrap_or(0).min(turns.len());
    let to = if p.running { turns.len().saturating_sub(1) } else { turns.len() }.max(from);
    // The hooks' numbering, from the session's last typed prompt as its cursor file knows it.
    let mut offset = 0i64;
    if let Some(last) = tune::dir()
        .map(|d| corrections::load_state(&d.join(format!("{}.json", p.session))))
        .and_then(|s| s.last_human)
        && let Some(t) = turns.iter().rev().find(|t| corrections::phrases::text_hash(&t.prompt) == last.text_hash)
    {
        offset = i64::from(last.num) - i64::from(t.num);
    }
    Ok(Read { picked: p, turns, from, to, offset })
}

/// The detector flagged turn `i`: the C4 line printed on its prompt, or its reply carried a correction marker.
fn flagged(turns: &[Turn], i: usize) -> bool {
    let t = &turns[i];
    t.check || t.signals.iter().any(|s| s.layer == "C3" && s.kind != "DEFERRED")
}

/// The signals that flagged turn `i`: its own, and the turn before's C2 that its prompt answers.
fn turn_signals(turns: &[Turn], i: usize) -> Vec<Signal> {
    let mut out: Vec<Signal> = turns[i].signals.clone();
    if i > 0 {
        for s in turns[i - 1].signals.iter().filter(|s| propose::carried_over(s)) {
            corrections::push_once(&mut out, s.clone());
        }
    }
    out
}

/// The layers among `signals`, each once: `C1`, `C2`, `C3`.
fn layers(signals: &[Signal]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in signals.iter().filter(|s| !(s.layer == "C3" && s.kind == "DEFERRED")) {
        if !out.contains(&s.layer) {
            out.push(s.layer.clone());
        }
    }
    out.sort();
    out
}

fn last_text(events: &[Event]) -> Option<&str> {
    events.iter().rev().find_map(|e| match e {
        Event::Text(t) => Some(t.as_str()),
        _ => None,
    })
}

fn first_text(events: &[Event]) -> Option<&str> {
    events.iter().find_map(|e| match e {
        Event::Text(t) => Some(t.as_str()),
        _ => None,
    })
}

const SESSION_HEADER: &str = "You are reviewing part of a Claude Code session for base, a tool that gives the AI short \
rules at the right moment. Read the turns below and answer with one JSON object and nothing else.

1. \"corrections\": every turn where the USER corrects the AI: says it got something wrong, repeats an instruction it \
ignored, tells it to stop doing something, or undoes what it did. A new request, a question, an approval or thanks is \
not a correction. For each: \"turn\"; \"why\" (what the AI got wrong, one sentence); \"rule\" (one sentence the AI \
should follow from now on, written to the AI); \"keywords\" (2 to 4 words or short phrases copied exactly from the \
USER's prompt that should bring the rule back); \"domain\" (one name from DOMAINS, or null).
2. \"not_corrections\": the numbers of the turns base flagged that are not corrections.
3. \"unmatched\": for each turn listed under NO DOMAIN MATCHED that belongs to one of the DOMAINS: \"turn\", \
\"domain\", \"keywords\" (words copied exactly from that prompt that should have brought the domain).";

const SESSION_FOOTER: &str = "Answer in this shape, with your own values:
{\"corrections\":[{\"turn\":3,\"why\":\"...\",\"rule\":\"...\",\"keywords\":[\"...\"],\"domain\":\"...\"}],\
\"not_corrections\":[5],\"unmatched\":[{\"turn\":12,\"domain\":\"...\",\"keywords\":[\"...\"]}]}";

/// `DOMAINS (name: some of its keywords):` and one line per domain.
fn domains_block(domains: &[crate::domain::DomainDef]) -> String {
    let mut s = String::from("DOMAINS (name: some of its keywords):\n");
    for d in domains {
        if d.is_always() {
            s.push_str(&format!("- {} (always on)\n", d.name));
            continue;
        }
        let kw: Vec<&str> = d.prompt_keywords.iter().take(8).map(String::as_str).collect();
        s.push_str(&format!("- {}: {}\n", d.name, if kw.is_empty() { "(no keywords)".to_string() } else { kw.join(", ") }));
    }
    s
}

pub(crate) fn quote(text: &str, max: usize) -> String {
    format!("\"{}\"", corrections::clip(&crate::scrub::scrub(text), max).replace('"', "'"))
}

/// One typed turn as the judge reads it.
fn turn_block(r: &Read, i: usize) -> String {
    let t = &r.turns[i];
    let mut s = format!("[turn {}]\n", t.num);
    if i > 0
        && let Some(b) = last_text(&r.turns[i - 1].events)
    {
        s.push_str(&format!("AI before: {}\n", quote(b, BEFORE_CHARS)));
    }
    s.push_str(&format!("USER: {}\n", quote(&t.prompt, PROMPT_CHARS)));
    if let Some(a) = first_text(&t.events) {
        s.push_str(&format!("AI after: {}\n", quote(a, AFTER_CHARS)));
    }
    let sig = turn_signals(&r.turns, i);
    if flagged(&r.turns, i) {
        s.push_str(&format!("base flagged it: {}\n", corrections::labels(&sig)));
    }
    s
}

/// The session's prompts for the judge, in chunks under [`CHUNK_CHARS`]: each the header, the domains, its turns, the
/// turns among them that matched no domain, the answer's shape.
fn session_prompts(r: &Read, domains: &str, unmatched: &HashSet<u32>) -> Vec<String> {
    let mut chunks: Vec<Vec<usize>> = vec![Vec::new()];
    let mut size = 0usize;
    for i in r.typed() {
        let n = turn_block(r, i).len();
        if size + n > CHUNK_CHARS && !chunks.last().is_some_and(Vec::is_empty) {
            chunks.push(Vec::new());
            size = 0;
        }
        size += n;
        if let Some(c) = chunks.last_mut() {
            c.push(i);
        }
    }
    chunks
        .into_iter()
        .filter(|c| !c.is_empty())
        .map(|c| {
            let body: String = c.iter().map(|i| turn_block(r, *i)).collect::<Vec<_>>().join("\n");
            let none: Vec<String> =
                c.iter().map(|i| r.turns[*i].num).filter(|n| unmatched.contains(n)).map(|n| n.to_string()).collect();
            let none = if none.is_empty() { "none".to_string() } else { format!("turns {}", none.join(", ")) };
            format!("{SESSION_HEADER}\n\n{domains}\nTURNS:\n{body}\nNO DOMAIN MATCHED: {none}\n\n{SESSION_FOOTER}\n")
        })
        .collect()
}

#[derive(Debug, Default, Deserialize)]
struct SessionAnswer {
    #[serde(default)]
    corrections: Vec<JudgedCorrection>,
    #[serde(default)]
    not_corrections: Vec<u32>,
    #[serde(default)]
    unmatched: Vec<JudgedUnmatched>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct JudgedCorrection {
    turn: u32,
    #[serde(default)]
    why: String,
    #[serde(default)]
    rule: String,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    domain: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct JudgedUnmatched {
    turn: u32,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
}

// ─── The pass ────────────────────────────────────────────────────────────────

/// One proposal the pass wrote, as its report lists it.
#[derive(Debug, Clone)]
pub struct Written {
    pub id: String,
    pub kind: propose::Kind,
    pub domain: String,
    pub what: String,
    pub evidence: String,
}

/// The detector's record on the turns judged (C5): per layer, the corrections it flagged and the flags that were not.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Detector {
    /// Typed turns the judge read.
    pub judged: usize,
    /// Of those, flagged by the detector.
    pub flagged: usize,
    /// Judged corrections.
    pub corrections: usize,
    /// Judged corrections the detector had flagged, by layer (one correction can count under two).
    pub hits: BTreeMap<String, usize>,
    /// Judged corrections it had not flagged.
    pub misses: usize,
    /// Flagged turns judged not corrections, by layer.
    pub false_flags: BTreeMap<String, usize>,
}

/// What the store check found or would look at.
#[derive(Debug, Clone, Default)]
pub struct StoreCheck {
    pub due: bool,
    pub ran: bool,
    pub merge: usize,
    pub split: usize,
    pub keywords: usize,
    pub retire: usize,
    pub note: Option<String>,
}

/// What `base tune` did, or would do with `--dry-run`.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub dry_run: bool,
    /// Each session read: its short id, typed prompts judged, flagged turns, prompts that matched no domain.
    pub sessions: Vec<(String, usize, usize, usize)>,
    pub backstop: usize,
    pub calls: usize,
    pub cached: usize,
    pub would_call: usize,
    pub store: StoreCheck,
    pub written: Vec<Written>,
    pub missed_saved: usize,
    pub detector: Detector,
    pub notes: Vec<String>,
}

impl Report {
    fn prompts(&self) -> usize {
        self.sessions.iter().map(|s| s.1).sum()
    }
    fn flagged(&self) -> usize {
        self.sessions.iter().map(|s| s.2).sum()
    }
    fn unmatched(&self) -> usize {
        self.sessions.iter().map(|s| s.3).sum()
    }
}

/// A domain's name as `domains.toml` spells it, when the judge named one that exists and is not always on.
fn known_domain(domains: &[crate::domain::DomainDef], name: Option<&str>) -> Option<String> {
    let want = crate::crud::slugify(name?.trim());
    domains.iter().find(|d| crate::crud::slugify(&d.name) == want).map(|d| d.name.clone())
}

/// The judge's keywords that are in `prompt` as whole words, each once.
fn in_prompt(keywords: &[String], prompt: &str) -> Vec<String> {
    let lower = prompt.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for k in keywords.iter().map(|k| k.trim()).filter(|k| !k.is_empty()) {
        if crate::domain::matcher::contains_word(&lower, &k.to_lowercase()) && !out.iter().any(|o| o.eq_ignore_ascii_case(k)) {
            out.push(k.to_string());
        }
    }
    out
}

/// BO-15 named item 5: a keyword no other recent prompt carries will never fire again. Such keywords go when at least
/// one that others carry is left.
fn keep_carried(keywords: Vec<String>, recent: &[String], own: &[&str]) -> Vec<String> {
    let others: Vec<String> = recent.iter().filter(|r| !own.contains(&r.as_str())).map(|r| r.to_lowercase()).collect();
    let carried: Vec<String> = keywords
        .iter()
        .filter(|k| others.iter().any(|r| crate::domain::matcher::contains_word(r, &k.to_lowercase())))
        .cloned()
        .collect();
    if carried.is_empty() { keywords } else { carried }
}

/// Run the pass from `cwd`. `running` is the session running it (its last turn is in progress).
pub fn run(config: &BaseConfig, cwd: &Path, args: &Args, judge: &mut dyn Judge, running: Option<&str>) -> Result<Report, String> {
    let own = tune::own_dir().ok_or_else(|| "no ~/.base-gbl here: the rule pass keeps its marks there".to_string())?;
    std::fs::create_dir_all(&own).map_err(|e| format!("{}: {e}", own.display()))?;
    let lock_path = own.join("lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("{}: {e}", lock_path.display()))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err("a rule pass is running in another session; nothing was done".to_string());
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(format!("{}: {e}", lock_path.display())),
    }

    let mut report = Report { dry_run: args.dry_run, ..Report::default() };
    let max = if args.max_calls == 0 { DEFAULT_MAX_CALLS } else { args.max_calls };
    let mut calls = Calls { judge, made: 0, cached: 0, max, dry_run: args.dry_run, would: 0 };
    let (picked, notes) = pick(config, cwd, args, running);
    report.notes.extend(notes);

    let domains = crate::domain::load_domains(cwd);
    let bench = crate::domain::rule_test::Bench::load(config, cwd);
    let mut sorter = propose::Sorter::load(config, cwd);
    let recent: Vec<String> =
        crate::domain::replay::recent_prompts(cwd, config.tune.replay_prompts).rows.into_iter().map(|(_, t)| t).collect();
    let dblock = domains_block(&domains);
    // Turns that already have a proposal (the AI ran `rule propose` after the C4 line): the pass leaves them alone.
    let proposed: HashSet<(String, String)> = corrections::review::load(config, cwd)
        .into_iter()
        .filter_map(|p| Some((p.session?, p.turn?)))
        .collect();
    let mut gaps: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
    let mut marks: Vec<(String, tune::Tuned)> = Vec::new();
    let mut missed: Vec<serde_json::Value> = Vec::new();
    let now = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);

    for p in picked {
        let label = p.session.chars().take(8).collect::<String>();
        let r = match read(config, p) {
            Ok(r) => r,
            Err(e) => {
                report.notes.push(format!("{label}: {e}"));
                continue;
            }
        };
        let typed = r.typed();
        let mark = tune::Tuned {
            at: now.clone(),
            turns: r.to as u32,
            prompt: r.to.checked_sub(1).map(|i| r.hooks_num(&r.turns[i])).unwrap_or(0),
        };
        if typed.is_empty() {
            if r.to > r.from && !args.dry_run {
                marks.push((r.picked.session.clone(), mark));
            }
            continue;
        }
        let unmatched: HashSet<u32> = typed
            .iter()
            .map(|i| &r.turns[*i])
            .filter(|t| bench.star_commands(&t.prompt).is_empty() && bench.matched_domains(&t.prompt).is_empty())
            .map(|t| t.num)
            .collect();
        let flagged_n = typed.iter().filter(|i| flagged(&r.turns, **i)).count();
        // Nothing a judge could find: the only typed turn is the session's first (a spawned session's boot prompt,
        // whose later turns are task notifications), so no reply came before it to correct; nothing flagged it; and
        // it matched a domain. Read with no call. On Chris's day of 2026-10-02 this was 9 of 19 sessions (gate 4).
        if typed.iter().all(|i| *i == 0) && flagged_n == 0 && unmatched.is_empty() {
            report.sessions.push((r.short(), typed.len(), 0, 0));
            if !args.dry_run {
                marks.push((r.picked.session.clone(), mark));
            }
            continue;
        }
        let prompts = session_prompts(&r, &dblock, &unmatched);
        if !calls.can(&prompts) {
            report.notes.push(format!("{}: left for the next pass ({} calls at most per pass)", r.short(), calls.max));
            continue;
        }
        report.sessions.push((r.short(), typed.len(), flagged_n, unmatched.len()));
        let mut answer = SessionAnswer::default();
        let mut whole = true;
        for prompt in &prompts {
            match calls.ask(prompt) {
                None => whole = false,
                Some(Ok(text)) => match parse_json::<SessionAnswer>(&text) {
                    Some(a) => {
                        keep_cached(prompt, &text);
                        answer.corrections.extend(a.corrections);
                        answer.not_corrections.extend(a.not_corrections);
                        answer.unmatched.extend(a.unmatched);
                    }
                    None => {
                        whole = false;
                        report.notes.push(format!("{}: the judge's answer was not the JSON asked for; this session waits for the next pass", r.short()));
                    }
                },
                Some(Err(e)) => {
                    whole = false;
                    report.notes.push(format!("{}: the judge's call failed ({}); this session waits for the next pass", r.short(), corrections::clip(&e, 160)));
                }
            }
        }
        if args.dry_run || !whole {
            continue;
        }
        let at: HashMap<u32, usize> = typed.iter().map(|i| (r.turns[*i].num, *i)).collect();

        // The detector's record (C5).
        report.detector.judged += typed.len();
        report.detector.flagged += flagged_n;
        let judged: HashSet<u32> = answer.corrections.iter().map(|c| c.turn).filter(|n| at.contains_key(n)).collect();
        for n in answer.not_corrections.iter().filter(|n| !judged.contains(n)) {
            if let Some(i) = at.get(n).copied().filter(|i| flagged(&r.turns, *i)) {
                for l in layers(&turn_signals(&r.turns, i)) {
                    *report.detector.false_flags.entry(l).or_default() += 1;
                }
            }
        }

        let mut seen_turns: HashSet<u32> = HashSet::new();
        for c in &answer.corrections {
            let Some(&i) = at.get(&c.turn) else { continue };
            if !seen_turns.insert(c.turn) {
                continue;
            }
            let t = &r.turns[i];
            let signals = turn_signals(&r.turns, i);
            report.detector.corrections += 1;
            let was_flagged = flagged(&r.turns, i);
            if was_flagged {
                for l in layers(&signals) {
                    *report.detector.hits.entry(l).or_default() += 1;
                }
            } else {
                report.detector.misses += 1;
                report.backstop += 1;
                missed.push(missed_case(&r, i, &c.why, config));
            }
            let hooks_turn = r.hooks_num(t);
            if proposed.contains(&(r.picked.session.clone(), hooks_turn.to_string())) {
                continue;
            }
            let rule = c.rule.trim();
            if rule.is_empty() {
                report.notes.push(format!("{} turn {}: judged a correction with no rule wording; not proposed", r.short(), t.num));
                continue;
            }
            let kw = keep_carried(in_prompt(&c.keywords, &t.prompt), &recent, &[t.prompt.as_str()]);
            let pargs = propose::Args {
                from_turn: true,
                text: Some(rule.to_string()),
                keywords: (!kw.is_empty()).then(|| kw.join(", ")),
                domain: known_domain(&domains, c.domain.as_deref()),
                ..propose::Args::default()
            };
            let ev = propose::Evidence {
                prompt: t.prompt.clone(),
                signals,
                session: Some(r.picked.session.clone()),
                turn: Some(hooks_turn),
                manual: false,
            };
            let mut prop = match sorter.sort(&pargs, ev) {
                Ok(p) => p,
                Err(e) => {
                    report.notes.push(format!("{} turn {}: {}", r.short(), t.num, corrections::clip(&e, 200)));
                    continue;
                }
            };
            prop.origin = "tune";
            prop.evidence.push(format!("judged a correction: {}", c.why.trim()));
            if !was_flagged {
                prop.evidence.push("base's detector did not flag it: the backstop found it".to_string());
            }
            if let Some(w) = write_one(&mut sorter, config, &mut prop, &mut report) {
                let what = prop.text.as_deref().map(|t| corrections::clip(t, 48)).unwrap_or_default();
                let marker = prop.signals.iter().find(|s| s.layer == "C3").map(|s| format!(" ({})", s.kind)).unwrap_or_default();
                report.written.push(Written { id: w, kind: prop.kind, domain: prop.target.domain.clone(), what, evidence: format!("1 correction{marker}") });
            }
        }
        for u in &answer.unmatched {
            let Some(&i) = at.get(&u.turn) else { continue };
            if !unmatched.contains(&u.turn) {
                continue;
            }
            let Some(d) = known_domain(&domains, u.domain.as_deref()).filter(|d| domains.iter().any(|x| &x.name == d && !x.is_always())) else {
                continue;
            };
            let kw = in_prompt(&u.keywords, &r.turns[i].prompt);
            if !kw.is_empty() {
                gaps.entry(d).or_default().push((r.turns[i].prompt.clone(), kw));
            }
        }
        marks.push((r.picked.session.clone(), mark));
    }

    // Keyword gaps from prompts that matched no domain, per domain, across the sessions read.
    if !args.dry_run {
        for (domain, list) in &gaps {
            let mut counts: Vec<(String, usize)> = Vec::new();
            for (_, kws) in list {
                for k in kws {
                    match counts.iter_mut().find(|(c, _)| c.eq_ignore_ascii_case(k)) {
                        Some(c) => c.1 += 1,
                        None => counts.push((k.clone(), 1)),
                    }
                }
            }
            counts.sort_by_key(|c| std::cmp::Reverse(c.1));
            let own: Vec<&str> = list.iter().map(|(p, _)| p.as_str()).collect();
            let kw = keep_carried(counts.into_iter().take(5).map(|(k, _)| k).collect(), &recent, &own);
            let mut evidence = vec![format!("{} {} matched no domain; the judge put {} in {domain}", list.len(), plural(list.len(), "prompt", "prompts"), if list.len() == 1 { "it" } else { "them" })];
            for (p, _) in list.iter().take(2) {
                evidence.push(format!("prompt: \"{}\"", corrections::clip(p, 120)));
            }
            let target = propose::Target { kind: "domain", id: domain.clone(), domain: domain.clone(), what: String::new() };
            let parts = propose::PatternParts {
                keywords: kw.clone(),
                example: list.first().map(|(p, _)| p.clone()),
                evidence,
                ..propose::PatternParts::default()
            };
            let mut prop = propose::pattern(propose::Kind::KeywordGap, target, format!("prompts about {domain} matched no domain"), parts);
            if let Some(w) = write_one(&mut sorter, config, &mut prop, &mut report) {
                report.written.push(Written {
                    id: w,
                    kind: prop.kind,
                    domain: domain.clone(),
                    what: format!("add {}", kw.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", ")),
                    evidence: format!("{} {}", list.len(), plural(list.len(), "prompt", "prompts")),
                });
            }
        }
    }

    // The store check (G0 question 2).
    report.store = super::tune_store::check(config, cwd, args, &bench, &mut calls, &mut sorter, &mut report.written, &mut report.notes);

    report.calls = calls.made;
    report.cached = calls.cached;
    report.would_call = calls.would;
    if args.dry_run {
        return Ok(report);
    }
    for (session, mark) in &marks {
        if let Err(e) = tune::write_tuned(session, mark) {
            report.notes.push(e);
        }
    }
    if !missed.is_empty() {
        match save_cases(&missed) {
            Ok(n) => report.missed_saved = n,
            Err(e) => report.notes.push(e),
        }
    }
    log_pass(&report);
    prune_cache(config.log.prompt_days);
    drop(lock);
    Ok(report)
}

pub(crate) fn plural<'s>(n: usize, one: &'s str, many: &'s str) -> &'s str {
    if n == 1 { one } else { many }
}

/// Write `prop` unless a proposal with its fingerprint is already in either tier (pending, approved, edited, or
/// rejected: K5e). Its id, or `None` with a note.
pub(crate) fn write_one(sorter: &mut propose::Sorter<'_>, config: &BaseConfig, prop: &mut propose::Proposal, report_written: &mut Report) -> Option<String> {
    write_proposal(sorter, config, prop, &mut report_written.notes)
}

pub(crate) fn write_proposal(sorter: &mut propose::Sorter<'_>, config: &BaseConfig, prop: &mut propose::Proposal, notes: &mut Vec<String>) -> Option<String> {
    if let Some((id, status)) = sorter.store().and_then(|s| corrections::review::fingerprint_known(s, &config.namespace, &prop.fingerprint)) {
        notes.push(format!("{} {} on {} is already {id} ({status}); not written again", prop.kind.label(), prop.target.kind, prop.target.id));
        return None;
    }
    match sorter.write(prop) {
        Ok(()) => prop.id.clone(),
        Err(e) => {
            notes.push(format!("could not write a {} proposal: {}", prop.kind.label(), corrections::clip(&e, 200)));
            None
        }
    }
}

// ─── C5: the detector's misses as test cases ─────────────────────────────────

/// A detector case for a correction the detector missed, in `tests/fixtures/corrections/cases.json`'s schema: the turn
/// before and the turn itself as events, what the detector finds on them today (`expect`, `checks`), and the label.
fn missed_case(r: &Read, i: usize, why: &str, config: &BaseConfig) -> serde_json::Value {
    let mut events: Vec<Event> = Vec::new();
    let mut json: Vec<serde_json::Value> = Vec::new();
    let clip = |t: &str| crate::scrub::scrub(&t.chars().take(2_000).collect::<String>());
    let start = i.saturating_sub(1);
    for t in &r.turns[start..=i] {
        let text = clip(&t.prompt);
        let human = t.human_num.is_some();
        events.push(Event::Prompt { human, text: text.clone() });
        json.push(if human { serde_json::json!({ "prompt": text }) } else { serde_json::json!({ "notification": text }) });
        for e in &t.events {
            match e {
                Event::Text(x) => {
                    let x = clip(x);
                    events.push(Event::Text(x.clone()));
                    json.push(serde_json::json!({ "text": x }));
                }
                Event::Wrote(f) => {
                    events.push(Event::Wrote(f.clone()));
                    json.push(serde_json::json!({ "write": f }));
                }
                Event::Interrupt => {
                    events.push(Event::Interrupt);
                    json.push(serde_json::json!("interrupt"));
                }
                Event::Denial => {
                    events.push(Event::Denial);
                    json.push(serde_json::json!("denial"));
                }
                Event::FileChanged(f) => {
                    events.push(Event::FileChanged(f.clone()));
                    json.push(serde_json::json!({ "file_changed": f }));
                }
                Event::Prompt { .. } => {}
            }
        }
    }
    let turns = corrections::turns(events, &config.corrections);
    let mut expect = serde_json::Map::new();
    for t in &turns {
        if !t.signals.is_empty() {
            let mut labels: Vec<String> = t.signals.iter().map(|s| format!("{} {}", s.layer, s.kind)).collect();
            labels.sort();
            expect.insert(t.num.to_string(), serde_json::json!(labels));
        }
    }
    let checks: Vec<u32> = turns.iter().filter(|t| t.check).map(|t| t.num).collect();
    serde_json::json!({
        "name": format!("missed-{}-turn-{}", r.short(), r.turns[i].num),
        "events": json,
        "expect": expect,
        "checks": checks,
        "missed": { "turn": turns.len(), "why": crate::scrub::scrub(why.trim()) },
        "source": { "session": r.short(), "turn": r.turns[i].num, "date": chrono::Local::now().format("%Y-%m-%d").to_string() },
    })
}

/// Where the missed cases are kept (G0 question 8): this machine only, never the public repo.
pub fn cases_path() -> Option<PathBuf> {
    tune::own_dir().map(|d| d.join("missed-cases.json"))
}

/// Add `cases` to [`cases_path`], a case already there by name replaced. How many were new.
fn save_cases(cases: &[serde_json::Value]) -> Result<usize, String> {
    let path = cases_path().ok_or_else(|| "no place for the missed cases".to_string())?;
    let mut doc: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({
            "about": "Corrections base's detector missed, found by base tune's backstop (BO-17, C5), in the schema of base's tests/fixtures/corrections/cases.json. `expect` and `checks` are what the detector finds on the case today; `missed.turn` is the turn the judge called a correction. Real prompts: kept on this machine only.",
            "cases": []
        }));
    let list = doc.get_mut("cases").and_then(serde_json::Value::as_array_mut).ok_or_else(|| format!("{}: no cases list", path.display()))?;
    let mut added = 0;
    for c in cases {
        let name = c["name"].as_str().unwrap_or_default();
        match list.iter().position(|x| x["name"] == name) {
            Some(i) => list[i] = c.clone(),
            None => {
                list.push(c.clone());
                added += 1;
            }
        }
    }
    tune::write_json(&path, &doc)?;
    Ok(added)
}

// ─── The log, and the report ─────────────────────────────────────────────────

/// One row per pass in `tune/log.jsonl`: what it read, what it cost, what it found, and the detector's record, for
/// BO-19's doctor.
fn log_pass(r: &Report) {
    let Some(path) = tune::own_dir().map(|d| d.join("log.jsonl")) else { return };
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for w in &r.written {
        *kinds.entry(w.kind.slug()).or_default() += 1;
    }
    let row = serde_json::json!({
        "ts": chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "sessions": r.sessions.len(),
        "prompts": r.prompts(),
        "flagged": r.flagged(),
        "unmatched": r.unmatched(),
        "calls": r.calls,
        "cached": r.cached,
        "store": r.store.ran,
        "proposals": kinds,
        "detector": r.detector,
        "missed_saved": r.missed_saved,
    });
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{row}");
    }
}

/// The detector's counts over every pass logged: what BO-19's doctor shows.
pub fn detector_totals() -> Detector {
    let mut out = Detector::default();
    let Some(path) = tune::own_dir().map(|d| d.join("log.jsonl")) else { return out };
    for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let d = &v["detector"];
        let n = |k: &str| d[k].as_u64().unwrap_or(0) as usize;
        out.judged += n("judged");
        out.flagged += n("flagged");
        out.corrections += n("corrections");
        out.misses += n("misses");
        for (field, into) in [("hits", &mut out.hits), ("false_flags", &mut out.false_flags)] {
            if let Some(m) = d[field].as_object() {
                for (k, x) in m {
                    *into.entry(k.clone()).or_default() += x.as_u64().unwrap_or(0) as usize;
                }
            }
        }
    }
    out
}

fn layer_counts(m: &BTreeMap<String, usize>) -> String {
    if m.is_empty() {
        return "none".to_string();
    }
    m.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")
}

/// What `base tune` prints (Example 1's shape).
pub fn render(r: &Report) -> String {
    let mut s = String::new();
    let sessions: Vec<String> = r.sessions.iter().map(|(id, ..)| id.clone()).collect();
    let what = format!(
        "{} {}{}, {} {}, {} flagged {}",
        r.sessions.len(),
        plural(r.sessions.len(), "session", "sessions"),
        if sessions.is_empty() { String::new() } else { format!(" ({})", sessions.join(", ")) },
        r.prompts(),
        plural(r.prompts(), "prompt", "prompts"),
        r.flagged(),
        plural(r.flagged(), "correction", "corrections"),
    );
    if r.dry_run {
        s.push_str(&format!("would read: {what}, {} with no domain match\n", r.unmatched()));
        s.push_str(&format!("store check: {}\n", store_line(&r.store)));
        s.push_str(&format!("haiku: would make {} {} ({} cached)\n", r.would_call, plural(r.would_call, "call", "calls"), r.cached));
        for n in &r.notes {
            s.push_str(&format!("  note: {n}\n"));
        }
        s.push_str("dry run: no call made, nothing written\n");
        return s;
    }
    s.push_str(&format!("read: {what}, {} unflagged found by the backstop\n", r.backstop));
    s.push_str(&format!("haiku: {} {} (cached: {})\n", r.calls, plural(r.calls, "call", "calls"), r.cached));
    s.push_str(&format!("store check: {}\n", store_line(&r.store)));
    if r.written.is_empty() {
        s.push_str("proposals written: 0\n");
    } else {
        s.push_str(&format!("proposals written: {} (see base rule review)\n", r.written.len()));
        let kind_w = r.written.iter().map(|w| w.kind.label().len()).max().unwrap_or(0);
        for w in &r.written {
            let head = format!("{} {:<kind_w$} · {} · {}", w.id, w.kind.label(), w.domain, w.what);
            s.push_str(&format!("  {head:<72}  evidence: {}\n", w.evidence));
        }
    }
    let d = &r.detector;
    if d.judged > 0 {
        s.push_str(&format!(
            "detector: {} of {} judged corrections flagged (by layer: {}), {} missed, false flags by layer: {}\n",
            d.corrections - d.misses,
            d.corrections,
            layer_counts(&d.hits),
            d.misses,
            layer_counts(&d.false_flags)
        ));
    }
    if r.missed_saved > 0 || r.backstop > 0 {
        let path = cases_path().map(|p| p.display().to_string()).unwrap_or_default();
        s.push_str(&format!(
            "detector: {} missed {} saved as {} ({path})\n",
            r.backstop,
            plural(r.backstop, "correction", "corrections"),
            plural(r.backstop, "a test case", "test cases")
        ));
    }
    for n in &r.notes {
        s.push_str(&format!("  note: {n}\n"));
    }
    s
}

fn store_line(c: &StoreCheck) -> String {
    let mut s = if c.ran {
        "ran".to_string()
    } else if c.due {
        "due".to_string()
    } else {
        "not due (once a day; --store runs it)".to_string()
    };
    if c.due || c.ran {
        s.push_str(&format!(" · {} merge, {} split, {} keyword, {} retire candidates", c.merge, c.split, c.keywords, c.retire));
    }
    if let Some(n) = &c.note {
        s.push_str(&format!(" · {n}"));
    }
    s
}
