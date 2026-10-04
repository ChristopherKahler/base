//! `base shadow report` (K9e), and the judgement promotion reads (K9f).
//!
//! THE EVENTS. The match-log rows that carry the running candidate's `shadow` entry, read by BO-19's one reader
//! (`usage::scan_for_shadow`): typed prompts, task notifications and the like, and file touches. "Events", as
//! `[shadow] min_prompts` counts them, are the typed prompts the candidate ran on and was not stopped on (K9d); the rest
//! are reported beside them.
//!
//! WINS AND LOSSES (G0 question 4, approved). A correction is about one reply (lynx's Q4 mapping, `usage::about`): C1,
//! the C2 repeat and a C3 `UPDATED`/`CORRECTED` logged on prompt N+1 answer reply N; an interrupt, a refusal or an
//! edited file logged on prompt N is about reply N. It is sorted by BO-15's own sort (`propose::Sorter::fit_for`) on
//! prompt N+1's text and its C3 marker line: the rule or decision it fits, or none (no stand-out fit: neither a win nor
//! a loss). A rule served on prompt N counts signals about reply N or N+1 (BO-19's window), so a correction about reply
//! N looks at the candidate's events of that session at prompts N and N-1: a WIN when the rule it fits is in their
//! `adds` (the candidate would have served it, live did not), a LOSS when it is in their `drops` (live served it, the
//! candidate would not have). In both, it is neither. Each correction counts once; a loss on a rule marked protected
//! (`base rule update <rule> --protected`) is a protected loss.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::Serialize;

use crate::config::BaseConfig;
use crate::usage;

use super::{Running, State, Version};

/// Where a candidate stands against K9f.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Status {
    /// Fewer typed prompts than `[shadow] min_prompts`.
    Collecting { have: usize, need: usize },
    /// Wins under `win_ratio` times the losses, zero losses counting as one (lynx's Q5 ruling).
    NotClear { wins: usize, losses: usize, need: usize },
    /// A loss on a protected rule: never promoted automatically.
    ProtectedLoss { rules: Vec<String> },
    /// A cwd whose own base.toml sets `[match]`: promotion would write a `[match]` its hooks do not read.
    WorkspaceOverride { file: String },
    /// A proposal its replay flags TOO BROAD (BO-16 refuses it without `--broad-ok`, lynx's Q10 pick).
    TooBroad { proposals: Vec<String> },
    /// A proposal of the candidate's approved or rejected since it started, or held by no tier of this folder: approving
    /// it would fail, so it is never tried.
    NotPending { proposals: Vec<String> },
    /// Its automatic promotion failed at a session start, which said why; it is not tried again automatically.
    NotPromoted,
    /// Every condition holds and `auto_promote = false`: `base shadow promote` does it.
    Ready,
    /// Every condition holds: the next session start promotes it.
    Promotes,
}

impl Status {
    /// Every condition of K9f holds.
    pub fn met(&self) -> bool {
        matches!(self, Status::Ready | Status::Promotes)
    }
}

/// What the log says of one running candidate.
#[derive(Debug, Clone, Serialize)]
pub struct Judged {
    pub candidate: String,
    pub live: String,
    pub since: String,
    /// Typed prompts the candidate ran on to the end: the events K9f counts.
    pub events: usize,
    pub machine: usize,
    pub files: usize,
    /// Events of any kind the candidate was stopped on (K9d).
    pub skipped: usize,
    pub max_ms: u64,
    /// Typed prompts and file touches whose picks differ.
    pub differ_prompts: usize,
    pub differ_files: usize,
    /// Each id the candidate adds or drops, with the events it does on, most first.
    pub adds: Vec<(String, usize)>,
    pub drops: Vec<(String, usize)>,
    pub wins: Vec<Attributed>,
    pub losses: Vec<Attributed>,
    pub protected_losses: Vec<Attributed>,
    /// `ms` per event the candidate ran on: median, p90, max.
    pub ms: (u64, u64, u64),
    /// One line each: what a reader must know to trust the numbers.
    pub notes: Vec<String>,
    pub status: Status,
    /// Each id as a reader names it: `<domain>.<id>` for a rule, the slug for a decision.
    #[serde(skip)]
    pub shown: HashMap<String, String>,
}

/// One event's ids: what the candidate adds, and what it drops.
type Picks<'a> = (HashSet<&'a str>, HashSet<&'a str>);

/// One correction a win or a loss was counted from.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Attributed {
    pub session: u32,
    /// The reply it is about.
    pub reply: u32,
    pub kind: &'static str,
    pub id: String,
}

/// Seconds since the epoch of an RFC 3339 time.
pub fn epoch(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|t| t.timestamp())
}

/// The log's rows for `candidate`, read from `cwd`'s tiers: files last written before `since` are passed over.
pub fn scan(cwd: &Path, candidate: &str, since: Option<i64>) -> usage::ShadowScan {
    let dirs = usage::log_dirs(cwd);
    let today = chrono::Local::now().date_naive();
    usage::scan_for_shadow(&dirs, today, &usage::Current::default(), candidate, usage::Reading::from_env(), since)
        .shadow
        .unwrap_or_default()
}

/// Judge the running candidate (K9e, K9f).
pub fn judge(config: &BaseConfig, cwd: &Path, running: &Running, version: &Version) -> Judged {
    let since = epoch(&running.since);
    let sh = scan(cwd, &running.name, since);
    judge_scan(config, cwd, running, version, &sh, &mut None)
}

/// [`judge_scan`] without the notes, for session start's pass: the live-changed note counts the whole configuration's
/// hash, which promotion does not need.
pub fn judge_scan_quiet<'c>(
    config: &'c BaseConfig,
    cwd: &Path,
    running: &Running,
    version: &Version,
    sh: &usage::ShadowScan,
    sorter: &mut Option<crate::corrections::propose::Sorter<'c>>,
) -> Judged {
    judge_with(config, cwd, running, version, sh, sorter, false, false)
}

/// [`judge_scan_quiet`] for session start's own pass: while the candidate has fewer typed prompts than `[shadow]
/// min_prompts` it is collecting whatever its wins, so its corrections are not sorted and the protected rules not read
/// (no store load on that hook for the first `min_prompts` prompts).
pub fn judge_for_session_start<'c>(
    config: &'c BaseConfig,
    cwd: &Path,
    running: &Running,
    version: &Version,
    sh: &usage::ShadowScan,
    sorter: &mut Option<crate::corrections::propose::Sorter<'c>>,
) -> Judged {
    judge_with(config, cwd, running, version, sh, sorter, false, true)
}

/// [`judge`] on a scan already read. `sorter` is loaded the first time a correction needs sorting.
pub fn judge_scan<'c>(
    config: &'c BaseConfig,
    cwd: &Path,
    running: &Running,
    version: &Version,
    sh: &usage::ShadowScan,
    sorter: &mut Option<crate::corrections::propose::Sorter<'c>>,
) -> Judged {
    judge_with(config, cwd, running, version, sh, sorter, true, false)
}

#[allow(clippy::too_many_arguments)]
fn judge_with<'c>(
    config: &'c BaseConfig,
    cwd: &Path,
    running: &Running,
    version: &Version,
    sh: &usage::ShadowScan,
    sorter: &mut Option<crate::corrections::propose::Sorter<'c>>,
    notes: bool,
    collecting_stops: bool,
) -> Judged {
    let since = epoch(&running.since).unwrap_or(i64::MIN);
    let events: Vec<&usage::ShadowEvent> = sh.events.iter().filter(|e| e.at >= since).collect();
    let mut j = Judged {
        candidate: running.name.clone(),
        live: running.against.clone(),
        since: super::day_of(&running.since),
        events: 0,
        machine: 0,
        files: 0,
        skipped: 0,
        max_ms: config.shadow.max_ms,
        differ_prompts: 0,
        differ_files: 0,
        adds: Vec::new(),
        drops: Vec::new(),
        wins: Vec::new(),
        losses: Vec::new(),
        protected_losses: Vec::new(),
        ms: (0, 0, 0),
        notes: Vec::new(),
        status: Status::Collecting { have: 0, need: config.shadow.min_prompts },
        shown: HashMap::new(),
    };
    let mut adds: BTreeMap<&str, usize> = BTreeMap::new();
    let mut drops: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ms: Vec<u64> = Vec::new();
    // Each (session, prompt)'s adds and drops, for the corrections.
    let mut by_turn: HashMap<(u32, u32), Picks<'_>> = HashMap::new();
    for e in &events {
        if e.entry.skipped.is_some() {
            j.skipped += 1;
            continue;
        }
        ms.push(e.entry.ms);
        match e.kind {
            "typed" => j.events += 1,
            "file" => j.files += 1,
            _ => j.machine += 1,
        }
        let differs = !e.entry.adds.is_empty() || !e.entry.drops.is_empty();
        if differs {
            if e.kind == "file" {
                j.differ_files += 1;
            } else {
                j.differ_prompts += 1;
            }
        }
        for a in &e.entry.adds {
            *adds.entry(a).or_default() += 1;
        }
        for d in &e.entry.drops {
            *drops.entry(d).or_default() += 1;
        }
        if let (Some(s), Some(t)) = (e.session, e.turn) {
            let slot = by_turn.entry((s, t)).or_default();
            slot.0.extend(e.entry.adds.iter().map(String::as_str));
            slot.1.extend(e.entry.drops.iter().map(String::as_str));
        }
    }
    let ranked = |m: BTreeMap<&str, usize>| {
        let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    };
    j.adds = ranked(adds);
    j.drops = ranked(drops);
    ms.sort_unstable();
    if !ms.is_empty() {
        let at = |q: f64| ms[((ms.len() as f64 - 1.0) * q).round() as usize];
        j.ms = (at(0.5), at(0.9), *ms.last().unwrap_or(&0));
    }
    if collecting_stops && j.events < config.shadow.min_prompts {
        j.status = Status::Collecting { have: j.events, need: config.shadow.min_prompts };
        return j;
    }

    // The corrections since the start whose replies the candidate had events on.
    let protected = crate::crud::rule::protected_ids(cwd, &config.namespace);
    let mut counted: HashSet<(u32, u32)> = HashSet::new();
    for &(s, reply, at) in sh.corrections.iter().filter(|c| c.2 >= since) {
        if !counted.insert((s, reply)) {
            continue;
        }
        let mut added: HashSet<&str> = HashSet::new();
        let mut dropped: HashSet<&str> = HashSet::new();
        for t in [Some(reply), reply.checked_sub(1)].into_iter().flatten() {
            if let Some((a, d)) = by_turn.get(&(s, t)) {
                added.extend(a);
                dropped.extend(d);
            }
        }
        let _ = at;
        if added.is_empty() && dropped.is_empty() {
            continue;
        }
        let text = sh.texts.get(&(s, reply + 1)).map(String::as_str).unwrap_or("");
        let marker = sh.markers.get(&(s, reply + 1)).map(String::as_str);
        if text.is_empty() && marker.is_none() {
            continue;
        }
        let sorter = sorter.get_or_insert_with(|| crate::corrections::propose::Sorter::load(config, cwd));
        let Some((kind, id)) = sorter.fit_for(text, marker) else { continue };
        let a = Attributed { session: s, reply, kind, id: id.clone() };
        match (added.contains(id.as_str()), dropped.contains(id.as_str())) {
            (true, false) => j.wins.push(a),
            (false, true) => {
                if kind == "rule" && protected.contains(&id) {
                    j.protected_losses.push(a.clone());
                }
                j.losses.push(a);
            }
            _ => {}
        }
    }
    if let Some(sorter) = sorter.as_ref() {
        for (id, _) in j.adds.iter().chain(&j.drops) {
            if let Some(show) = sorter.shown(id) {
                j.shown.insert(id.clone(), show.to_string());
            }
        }
    }

    // What a reader must know.
    let idf = version.settings.prompt_idf || config.matching.prompt_idf;
    if notes && idf && config.log.prompt_text_mode() != crate::emit::match_log::PromptText::Full {
        j.notes.push(
            "prompt_idf acts as off: [log] prompt_text keeps no whole prompt, so there is nothing to count".to_string(),
        );
    }
    if notes && super::content_hash(config, cwd, &config.matching, &[]) != running.live_hash {
        j.notes.push(format!(
            "live changed since {} (rules, keywords or [match] edited while the candidate ran): later events compare \
             against live as it is now",
            j.since
        ));
    }
    j.status = status(config, cwd, version, &j);
    j
}

/// The candidate's proposals that are no longer pending as `cwd` reads the store: approved or rejected since it started,
/// or held by no tier here. Empty for a matcher candidate.
pub fn not_pending(config: &BaseConfig, cwd: &Path, version: &Version) -> Vec<String> {
    if !version.is_proposals() {
        return Vec::new();
    }
    let all = crate::corrections::review::load(config, cwd);
    version
        .proposals
        .iter()
        .filter(|p| !all.iter().any(|q| q.id == p.id && q.status == "pending"))
        .map(|p| p.id.clone())
        .collect()
}

/// K9f, in order: a proposals candidate's proposals still pending (else nothing can be promoted), enough typed prompts,
/// then the protected rules, then wins against losses (lynx's Q5: zero losses count as one), then what would stop a
/// write (a workspace `[match]`, a TOO BROAD proposal).
fn status(config: &BaseConfig, cwd: &Path, version: &Version, j: &Judged) -> Status {
    let gone = not_pending(config, cwd, version);
    if !gone.is_empty() {
        return Status::NotPending { proposals: gone };
    }
    let need = config.shadow.min_prompts;
    if j.events < need {
        return Status::Collecting { have: j.events, need };
    }
    if !j.protected_losses.is_empty() {
        let mut rules: Vec<String> = j.protected_losses.iter().map(|a| a.id.clone()).collect();
        rules.dedup();
        return Status::ProtectedLoss { rules };
    }
    let (wins, losses) = (j.wins.len(), j.losses.len());
    let need_wins = (config.shadow.win_ratio * losses.max(1) as f64).ceil() as usize;
    if (wins as f64) < config.shadow.win_ratio * losses.max(1) as f64 {
        return Status::NotClear { wins, losses, need: need_wins };
    }
    if !version.is_proposals()
        && let Some(file) = super::workspace_match_override(cwd)
    {
        return Status::WorkspaceOverride { file };
    }
    if version.is_proposals() {
        let replayer = crate::domain::replay::Replayer::load(config, cwd);
        let broad: Vec<String> = version
            .proposals
            .iter()
            .filter(|p| replayer.run(&p.change()).is_ok_and(|o| o.too_broad()))
            .map(|p| p.id.clone())
            .collect();
        if !broad.is_empty() {
            return Status::TooBroad { proposals: broad };
        }
    }
    if config.shadow.auto_promote { Status::Promotes } else { Status::Ready }
}

/// The status line.
pub fn status_line(s: &Status, config: &BaseConfig) -> String {
    match s {
        Status::Collecting { have, need } => {
            format!("status: collecting: {have} of [shadow] min_prompts = {need} typed prompts")
        }
        Status::NotClear { wins, losses, need } => format!(
            "status: not clear: wins {wins} · losses {losses}; promotion needs wins >= [shadow] win_ratio {} x max(losses, \
             1) = {need}",
            config.shadow.win_ratio
        ),
        Status::ProtectedLoss { rules } => format!(
            "status: not promoted: a loss on protected rule {} (a protected rule's loss always blocks promotion)",
            rules.join(", ")
        ),
        Status::WorkspaceOverride { file } => format!(
            "status: not promoted here: {file} sets its own [match], which a promotion of ~/.base-gbl/base.toml would not \
             change"
        ),
        Status::TooBroad { proposals } => format!(
            "status: no automatic promotion: {} TOO BROAD on replay; with the user's yes: base shadow promote --broad-ok",
            proposals.join(", ")
        ),
        Status::NotPending { proposals } => format!(
            "status: not promoted: {} no longer pending here (approved or rejected since the start, or not in this folder's \
             tiers); end it: base shadow stop",
            proposals.join(", ")
        ),
        Status::NotPromoted => "status: its automatic promotion failed (the session start said why) and is not tried again \
                                automatically; by hand: base shadow promote"
            .to_string(),
        Status::Ready => "status: ready to promote: base shadow promote".to_string(),
        Status::Promotes => {
            "status: promotion conditions met (auto_promote = true): promotes at the next session start".to_string()
        }
    }
}

/// How many of a list a line names before `+N more`.
const LISTED: usize = 3;

fn list_line(items: &[(String, usize)], shown: &HashMap<String, String>) -> String {
    if items.is_empty() {
        return "none".to_string();
    }
    let parts: Vec<String> = items
        .iter()
        .take(LISTED)
        .enumerate()
        .map(|(i, (id, n))| {
            let name = shown.get(id).map(String::as_str).unwrap_or(id);
            if i == 0 { format!("{name} ({n} events)") } else { format!("{name} ({n})") }
        })
        .collect();
    let more = items.len().saturating_sub(LISTED);
    let mut line = parts.join(", ");
    if more > 0 {
        line.push_str(&format!(" · +{more} more (--json lists them)"));
    }
    line
}

/// The report as `base shadow report` prints it (G0's drawing of K9e).
pub fn render(j: &Judged, config: &BaseConfig) -> String {
    let mut out = format!(
        "candidate {} vs live {} · since {}: {} typed prompts, {} task notifications, {} file touches",
        j.candidate, j.live, j.since, j.events, j.machine, j.files
    );
    if j.skipped > 0 {
        out.push_str(&format!(" · {} skipped (over [shadow] max_ms = {})", j.skipped, j.max_ms));
    }
    out.push('\n');
    out.push_str(&format!(
        "  differ on {} events ({} prompts, {} file touches) · candidate time per event: median {} ms, p90 {} ms, max {} ms\n",
        j.differ_prompts + j.differ_files,
        j.differ_prompts,
        j.differ_files,
        j.ms.0,
        j.ms.1,
        j.ms.2
    ));
    out.push_str(&format!("  candidate adds:   {}\n", list_line(&j.adds, &j.shown)));
    out.push_str(&format!("  candidate drops:  {}\n", list_line(&j.drops, &j.shown)));
    out.push_str(&format!(
        "  wins {} · losses {} · protected losses {}\n",
        j.wins.len(),
        j.losses.len(),
        j.protected_losses.len()
    ));
    for n in &j.notes {
        out.push_str(&format!("  note: {n}\n"));
    }
    out.push_str(&format!("  {}\n", status_line(&j.status, config)));
    out
}

/// `base shadow report [--json]`.
pub fn run(config: &BaseConfig, cwd: &Path, json: bool) -> Result<String, String> {
    let dir = super::dir().ok_or("no home directory, so no shadow")?;
    let state = State::load(&dir);
    let Some(running) = state.candidate.as_ref() else {
        let mut out = "no shadow is running. Start one: base shadow start --matcher bm25 (or keyword-only), or \
                       base shadow start --from-proposals <ids>\n"
            .to_string();
        if let Some(p) = &state.promotion {
            out.push_str(&format!("{}\n", super::promote::describe_last(p)));
            // Both rates while the watch runs (lynx's Q6 ruling).
            if p.watching
                && let Some(line) = super::promote::watch_line(config, cwd, p)
            {
                out.push_str(&format!("  {line}\n"));
            }
        }
        return Ok(out);
    };
    let version = Version::load(&dir, &running.name).ok_or_else(|| format!("version {} is missing from {}", running.name, dir.display()))?;
    let mut j = judge(config, cwd, running, &version);
    if j.status == Status::Promotes && state.not_promoted.as_deref() == Some(running.name.as_str()) {
        j.status = Status::NotPromoted;
    }
    if json {
        return serde_json::to_string_pretty(&j).map(|s| s + "\n").map_err(|e| e.to_string());
    }
    Ok(render(&j, config))
}
