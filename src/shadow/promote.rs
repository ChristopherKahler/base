//! Promotion (K9f, K9i), rollback (K9g) and the announcement (K9h).
//!
//! WHEN. At session start, only while a state file exists, after the index refresh, under the shadow lock taken with
//! `try_lock`: a second session start at the same moment does nothing. `base shadow promote` and `base shadow
//! rollback` always work by hand.
//!
//! WHAT PROMOTION WRITES (G0 question 3, approved with conditions). A matcher candidate's `[match]` keys go into
//! `~/.base-gbl/base.toml` through the one line-by-line writer (`measure::set_section_keys`), which edits only those
//! keys and refuses anything else; the file is copied to `base.toml.BAK-<date>-pre-<version>` first. A proposals
//! candidate's proposals go through BO-16's approval (`review::run`, `Approve`), each tier's graph snapshotted first
//! (`store::snapshot`, BO-12's backup path) and each `domains.toml` copied; each approval's undo is recorded from the
//! store before and after it: the quads it added and removed on rule, decision, domain and proposal records, and the
//! `domains.toml` text it changed.
//!
//! ROLLBACK (lynx's Q6 ruling, and its edge ruled 2026-10-03). Once `[shadow] watch_prompts` typed prompts have passed
//! since a promotion, the share of them whose reply drew a correction (Q4 mapping), per 100, is compared with the share
//! over the `watch_prompts` typed prompts before it, both read from the logs of the folder the promotion was judged in.
//! The promotion is rolled back only when the rise is more than `[shadow] rollback_sd` times the standard deviation the
//! difference of two windows of those sizes shows by chance: `rollback_sd x sqrt(p (1 - p) (1 / n_before + 1 / n_after))
//! x 100`, p the rate pooled over both windows, so a before rate of 0 does not make one correction a rollback. With no
//! typed prompts before it there is nothing to compare, so no automatic rollback; the promotion line and the report say
//! so. On BO-19's replayed log, with nothing changed, the next 100 prompts were higher than the 100 before at 44% of
//! points; past 2.5 of those deviations (15 per 100 at its 23.6), at 5.5%. A smaller real regression is caught by hand:
//! every promotion is announced with its undo, and the report shows both rates.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::BaseConfig;
use crate::crud;
use crate::domain::tier::Tier;

use super::report::{self, Judged, Status};
use super::{State, Version};

/// The last promotion: what it made live, what was live before, its watch, and how to undo it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Promotion {
    pub version: String,
    pub previous: String,
    /// When it was made live (RFC 3339).
    pub at: String,
    /// `automatically` or `by hand`.
    pub by: String,
    pub wins: usize,
    pub losses: usize,
    pub prompts: usize,
    /// The typed prompts before it, and how many drew a correction.
    pub before: Rate,
    /// The watch is still running.
    pub watching: bool,
    /// When it was rolled back, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolled_back: Option<String>,
    pub undo: Undo,
    /// The folder it was judged in: its tiers' logs gave the rate before, and the watch reads the same ones.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
}

impl Promotion {
    /// The folder whose logs the watch reads: the one the promotion was judged in, while it is there.
    fn folder_or(&self, cwd: &Path) -> PathBuf {
        let f = PathBuf::from(&self.folder);
        if !self.folder.is_empty() && f.is_dir() { f } else { cwd.to_path_buf() }
    }

    /// No typed prompts before it: nothing to compare the watch with, so it is never rolled back automatically.
    pub fn unwatched(&self) -> bool {
        self.before.prompts == 0
    }
}

/// What a promotion line says when it has no automatic rollback.
const UNWATCHED: &str = "no automatic rollback (no typed prompts before it to compare with)";

/// Typed prompts, and how many of them drew a correction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rate {
    pub corrected: usize,
    pub prompts: usize,
}

impl Rate {
    /// Per 100 typed prompts.
    pub fn per_100(&self) -> f64 {
        if self.prompts == 0 { 0.0 } else { self.corrected as f64 * 100.0 / self.prompts as f64 }
    }
}

/// How to put back what a promotion wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Undo {
    /// The `[match]` keys of `file` as they were written before: a literal, or `None` for a key that was not there.
    Match { file: String, keys: Vec<(String, Option<String>)> },
    /// Each approval, in the order it ran.
    Proposals { steps: Vec<Step> },
}

/// What one approval wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub proposal: String,
    /// Per tier graph: the quads it added and removed, as N-Triples terms and the graph's IRI.
    pub graphs: Vec<GraphDiff>,
    /// Per `domains.toml` it changed: its text before and after.
    pub tomls: Vec<(String, String, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphDiff {
    /// A working directory in the tier whose graph it is.
    pub tier_cwd: String,
    pub added: Vec<[String; 4]>,
    pub removed: Vec<[String; 4]>,
}

/// The margin a rise in corrections per 100 must pass before a rollback (lynx's Q6 ruling and its edge): `sd` standard
/// deviations of the difference two windows of these sizes show by chance, at the rate pooled over both. `None` when
/// either window holds no typed prompt: nothing to compare.
pub fn noise_margin(before: &Rate, after: &Rate, sd: f64) -> Option<f64> {
    if before.prompts == 0 || after.prompts == 0 {
        return None;
    }
    let p = (before.corrected + after.corrected) as f64 / (before.prompts + after.prompts) as f64;
    let spread = p * (1.0 - p) * (1.0 / before.prompts as f64 + 1.0 / after.prompts as f64);
    Some(sd * spread.sqrt() * 100.0)
}

/// The typed prompts of `typed` (in log order) whose reply drew a correction.
fn rate_of(typed: &[(u32, u32, i64)], corrections: &HashSet<(u32, u32)>) -> Rate {
    Rate { corrected: typed.iter().filter(|(s, n, _)| corrections.contains(&(*s, *n))).count(), prompts: typed.len() }
}

/// The last `n` typed prompts before `at`, and the first `n` from it.
fn windows(sh: &crate::usage::ShadowScan, at: i64, n: usize) -> (Rate, Rate) {
    let corrections: HashSet<(u32, u32)> = sh.corrections.iter().map(|(s, r, _)| (*s, *r)).collect();
    let before: Vec<(u32, u32, i64)> = sh.typed.iter().filter(|t| t.2 < at).copied().collect();
    let before = &before[before.len().saturating_sub(n)..];
    let after: Vec<(u32, u32, i64)> = sh.typed.iter().filter(|t| t.2 >= at).take(n).copied().collect();
    (rate_of(before, &corrections), rate_of(&after, &corrections))
}

// ─── Session start ─────────────────────────────────────────────────────────

/// Session start's pass (K9f, K9g, K9h): judge the watch of the last promotion, judge the candidate, and return the
/// lines to print once. Nothing at all without a state file (the seamless upgrade), and nothing when another session
/// start holds the lock.
pub fn session_start(config: &BaseConfig, cwd: &Path) -> Vec<String> {
    let Some(dir) = super::dir() else { return Vec::new() };
    if !dir.join(super::STATE).is_file() {
        return Vec::new();
    }
    let Some(_lock) = super::lock(&dir, std::time::Duration::ZERO) else { return Vec::new() };
    let mut state = State::load(&dir);
    let before = state.clone();
    if let Some(p) = state.promotion.clone().filter(|p| p.watching) {
        watch(config, cwd, &mut state, &p);
    }
    if let Some(running) = state.candidate.clone()
        && let Some(version) = Version::load(&dir, &running.name)
    {
        let since = report::epoch(&running.since);
        let sh = report::scan(cwd, &running.name, since);
        let mut sorter = None;
        let j = report::judge_for_session_start(config, cwd, &running, &version, &sh, &mut sorter);
        match &j.status {
            // Tried once: an automatic promotion that failed is not tried again at every session start (each try would
            // back up the graph and each domains.toml again, and say so again); by hand it always is.
            Status::Promotes if state.not_promoted.as_deref() != Some(version.name.as_str()) => {
                if let Err(why) = promote(config, cwd, &mut state, &version, &j, "automatically", false, Some(&sh)) {
                    state.announce.push(format!(
                        "matcher: candidate {} was not promoted: {why} · not tried again automatically; by hand: base \
                         shadow promote",
                        version.name
                    ));
                    state.not_promoted = Some(version.name.clone());
                }
            }
            Status::Ready if state.ready_told.as_deref() != Some(version.name.as_str()) => {
                state.announce.push(format!(
                    "matcher: candidate {} ready to promote (wins {}, losses {}) · base shadow promote",
                    version.name,
                    j.wins.len(),
                    j.losses.len()
                ));
                state.ready_told = Some(version.name.clone());
            }
            _ => {}
        }
    }
    let lines = std::mem::take(&mut state.announce);
    if state != before && let Err(why) = state.save(&dir) {
        eprintln!("base: could not save the shadow state: {why}");
        return Vec::new();
    }
    // A promotion or a rollback changed what live is: the indexes it scores with, counted once the state that names it
    // is saved (before, the refresh read the old state and rebuilt the index of a candidate no longer running).
    if state.live != before.live {
        super::run::refresh_indexes(&BaseConfig::load(cwd), cwd);
    }
    lines
}

/// The watch (K9g): once `watch_prompts` typed prompts have passed since the promotion, keep it or roll it back.
fn watch(config: &BaseConfig, cwd: &Path, state: &mut State, p: &Promotion) {
    let Some(at) = report::epoch(&p.at) else { return };
    let n = config.shadow.watch_prompts.max(1);
    // The logs the rate before was read from: a session start in another folder reads other tiers.
    let sh = report::scan(&p.folder_or(cwd), "", Some(at));
    let after: Vec<(u32, u32, i64)> = sh.typed.iter().filter(|t| t.2 >= at).take(n).copied().collect();
    if after.len() < n {
        return;
    }
    let corrections: HashSet<(u32, u32)> = sh.corrections.iter().map(|(s, r, _)| (*s, *r)).collect();
    let rate = rate_of(&after, &corrections);
    let rise = rate.per_100() - p.before.per_100();
    // `None` with no typed prompts before it: nothing to compare, so the watch ends without a rollback (its line said so).
    if let Some(margin) = noise_margin(&p.before, &rate, config.shadow.rollback_sd).filter(|m| rise > *m) {
        match rollback(config, state) {
            Ok(_) => state.announce.push(format!(
                "matcher: {} rolled back (corrections rose from {:.0} to {:.0} per 100 prompts, over the {:.0} noise margin) · \
                 redo: base shadow promote {}",
                p.version,
                p.before.per_100(),
                rate.per_100(),
                margin,
                p.version
            )),
            Err(why) => state.announce.push(format!(
                "matcher: {} should be rolled back (corrections rose from {:.0} to {:.0} per 100 prompts) and could not be: \
                 {why}. Run base shadow rollback",
                p.version,
                p.before.per_100(),
                rate.per_100()
            )),
        }
    } else if let Some(cur) = state.promotion.as_mut() {
        cur.watching = false;
    }
}

// ─── Promote ───────────────────────────────────────────────────────────────

/// `base shadow promote [<version>] [--broad-ok]`.
pub fn by_hand(config: &BaseConfig, cwd: &Path, name: Option<&str>, broad_ok: bool) -> Result<String, String> {
    let dir = super::dir().ok_or("no home directory, so no shadow")?;
    let _lock = super::lock(&dir, std::time::Duration::from_secs(10)).ok_or("the shadow state is locked by another base; try again")?;
    let mut state = State::load(&dir);
    let name = match (name, &state.candidate) {
        (Some(n), _) => n.to_string(),
        (None, Some(c)) => c.name.clone(),
        (None, None) => return Err("no candidate is running: name the version to make live (base shadow promote <version>)".to_string()),
    };
    let version = Version::load(&dir, &name).ok_or_else(|| format!("no version {name} (versions are named by base shadow start)"))?;
    if state.live.as_deref() == Some(name.as_str()) && state.candidate.as_ref().is_none_or(|c| c.name != name) {
        return Err(format!("{name} is live already"));
    }
    let j = match &state.candidate {
        Some(c) if c.name == name => {
            let since = report::epoch(&c.since);
            let sh = report::scan(cwd, &c.name, since);
            let mut sorter = None;
            Some(report::judge_scan_quiet(config, cwd, c, &version, &sh, &mut sorter))
        }
        _ => None,
    };
    let empty = empty_judged(&name, &state);
    let line = promote(config, cwd, &mut state, &version, j.as_ref().unwrap_or(&empty), "by hand", broad_ok, None)?;
    state.save(&dir)?;
    super::run::refresh_indexes(&BaseConfig::load(cwd), cwd);
    Ok(format!("{line}\n"))
}

fn empty_judged(name: &str, state: &State) -> Judged {
    Judged {
        candidate: name.to_string(),
        live: state.live.clone().unwrap_or_default(),
        since: String::new(),
        events: 0,
        machine: 0,
        files: 0,
        skipped: 0,
        max_ms: 0,
        differ_prompts: 0,
        differ_files: 0,
        adds: Vec::new(),
        drops: Vec::new(),
        wins: Vec::new(),
        losses: Vec::new(),
        protected_losses: Vec::new(),
        ms: (0, 0, 0),
        notes: Vec::new(),
        status: Status::Collecting { have: 0, need: 0 },
        shown: HashMap::new(),
    }
}

/// Make `version` live: write it, record the undo and the watch, and queue the announcement. `scan` is the log as the
/// judge read it, when it has it.
#[allow(clippy::too_many_arguments)]
fn promote(
    config: &BaseConfig,
    cwd: &Path,
    state: &mut State,
    version: &Version,
    j: &Judged,
    by: &str,
    broad_ok: bool,
    scan: Option<&crate::usage::ShadowScan>,
) -> Result<String, String> {
    let previous = state.live.clone().unwrap_or_else(|| "(unnamed)".to_string());
    let undo = if version.is_proposals() {
        approve_all(config, cwd, version, broad_ok)?
    } else {
        write_match(cwd, version)?
    };
    let now = super::now();
    let at = report::epoch(&now).unwrap_or(0);
    // The rate before: the watch compares the same number of typed prompts after.
    let owned;
    let sh = match scan {
        Some(s) if s.typed.len() >= config.shadow.watch_prompts => s,
        _ => {
            owned = report::scan(cwd, "", None);
            &owned
        }
    };
    let (before, _) = windows(sh, at, config.shadow.watch_prompts.max(1));
    let promotion = Promotion {
        version: version.name.clone(),
        previous: previous.clone(),
        at: now,
        by: by.to_string(),
        wins: j.wins.len(),
        losses: j.losses.len(),
        prompts: j.events,
        before,
        // With no typed prompts before it there is nothing to watch against (lynx's Q6 edge ruling).
        watching: before.prompts > 0,
        rolled_back: None,
        undo,
        folder: cwd.display().to_string(),
    };
    let unwatched = if promotion.unwatched() { format!(" · {UNWATCHED}") } else { String::new() };
    state.promotion = Some(promotion);
    state.live = Some(version.name.clone());
    if state.candidate.as_ref().is_some_and(|c| c.name == version.name) {
        state.candidate = None;
    }
    state.ready_told = None;
    state.not_promoted = None;
    let line = if by == "automatically" || j.events > 0 {
        format!(
            "matcher: candidate {} promoted (wins {}, losses {}, {} prompts){unwatched} · undo: base shadow rollback",
            version.name,
            j.wins.len(),
            j.losses.len(),
            j.events
        )
    } else {
        format!("matcher: {} promoted by hand over {previous}{unwatched} · undo: base shadow rollback", version.name)
    };
    state.announce.push(line.clone());
    // The indexes the hooks score with are refreshed by the caller once this state is saved.
    Ok(line)
}

/// A matcher version's `[match]` keys written into the global base.toml, after a backup; the keys as they were, for the
/// undo.
fn write_match(cwd: &Path, version: &Version) -> Result<Undo, String> {
    if let Some(file) = super::workspace_match_override(cwd) {
        return Err(format!("{file} sets its own [match]; remove it there first, or promote from another folder"));
    }
    let path = crate::measure::global_config_path().ok_or("no home directory, so no ~/.base-gbl/base.toml")?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let table: toml::Table = text.parse().map_err(|e| format!("{} does not parse: {e}", path.display()))?;
    let written = table.get("match").and_then(toml::Value::as_table);
    let effective: crate::config::MatchConfig =
        written.cloned().map(toml::Value::Table).and_then(|t| t.try_into().ok()).unwrap_or_default();
    let mut keys: Vec<(&str, Option<String>)> = Vec::new();
    let mut before: Vec<(String, Option<String>)> = Vec::new();
    for ((k, want), (_, now)) in version.settings.keys().into_iter().zip(effective.keys()) {
        // Only the keys the candidate set at its start, so a key edited by hand since then stays as it is. A live
        // snapshot sets none: promoting one puts back the whole `[match]` it was.
        if want == now || (!version.sets.is_empty() && !version.sets.iter().any(|s| s.as_str() == k)) {
            continue;
        }
        before.push((k.to_string(), written.and_then(|t| t.get(k)).map(|v| v.to_string())));
        keys.push((k, want.map(|v| v.to_string())));
    }
    if !keys.is_empty() {
        write_keys(&path, &text, &keys, &version.name)?;
    }
    Ok(Undo::Match { file: path.display().to_string(), keys: before })
}

/// `[match]` keys set or removed in `path` (whose text is `text`), after a copy to `base.toml.BAK-<date>-pre-<version>`.
fn write_keys(path: &Path, text: &str, keys: &[(&str, Option<String>)], version: &str) -> Result<(), String> {
    let after = crate::measure::set_section_keys(text, "match", keys, &|k: &str| vec![k.to_string()]).map_err(|e| format!("{e:#}"))?;
    if path.is_file() {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup = path.with_file_name(format!("base.toml.BAK-{stamp}-pre-{version}"));
        std::fs::copy(path, &backup).map_err(|e| format!("backing up {} to {}: {e}", path.display(), backup.display()))?;
    }
    let tmp = path.with_extension("toml.shadow-tmp");
    std::fs::write(&tmp, &after).map_err(|e| format!("{}: {e}", tmp.display()))?;
    crate::store::rename_with_retry(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

// ─── Rollback ──────────────────────────────────────────────────────────────

/// `base shadow rollback`.
pub fn rollback_by_hand(config: &BaseConfig, cwd: &Path) -> Result<String, String> {
    let dir = super::dir().ok_or("no home directory, so no shadow")?;
    let _lock = super::lock(&dir, std::time::Duration::from_secs(10)).ok_or("the shadow state is locked by another base; try again")?;
    let mut state = State::load(&dir);
    let name = rollback(config, &mut state)?;
    let line = format!("matcher: {name} rolled back by hand · redo: base shadow promote {name}");
    state.announce.push(line.clone());
    state.save(&dir)?;
    super::run::refresh_indexes(&BaseConfig::load(cwd), cwd);
    Ok(format!("{line}\n"))
}

/// Undo the last promotion that stands; the version it replaced is live again. Its name. The caller refreshes the
/// indexes once the state is saved.
fn rollback(config: &BaseConfig, state: &mut State) -> Result<String, String> {
    let Some(p) = state.promotion.clone().filter(|p| p.rolled_back.is_none()) else {
        return Err("nothing to roll back: no promotion stands".to_string());
    };
    match &p.undo {
        Undo::Match { file, keys } => {
            let path = PathBuf::from(file);
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
            let keys: Vec<(&str, Option<String>)> = keys.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            if !keys.is_empty() {
                write_keys(&path, &text, &keys, &p.previous)?;
            }
        }
        Undo::Proposals { steps } => undo_steps(config, steps, &p.previous)?,
    }
    if let Some(cur) = state.promotion.as_mut() {
        cur.rolled_back = Some(super::now());
        cur.watching = false;
    }
    state.live = Some(p.previous.clone());
    Ok(p.version)
}

/// The last promotion in one line, for a report with no candidate running.
pub fn describe_last(p: &Promotion) -> String {
    match &p.rolled_back {
        Some(when) => format!(
            "last promotion: {} over {} on {}, rolled back on {}",
            p.version,
            p.previous,
            super::day_of(&p.at),
            super::day_of(when)
        ),
        None if p.unwatched() => format!(
            "last promotion: {} over {} on {} ({}) · {UNWATCHED} · undo: base shadow rollback",
            p.version,
            p.previous,
            super::day_of(&p.at),
            p.by
        ),
        None if p.watching => format!(
            "last promotion: {} over {} on {} ({}), watched for [shadow] watch_prompts typed prompts; corrections before it: \
             {:.0} per 100 · undo: base shadow rollback",
            p.version,
            p.previous,
            super::day_of(&p.at),
            p.by,
            p.before.per_100()
        ),
        None => format!("last promotion: {} over {} on {}, kept after its watch", p.version, p.previous, super::day_of(&p.at)),
    }
}

// ─── Proposals: approve, and undo ──────────────────────────────────────────

/// The tiers whose graph and `domains.toml` an approval can write: the workspace's and the global one's.
fn tiers(cwd: &Path) -> Vec<(Tier, PathBuf)> {
    crud::rule::tier_cwds(cwd)
}

/// Every quad of `tier_cwd`'s graph on a rule, decision, domain or proposal record, as N-Triples terms and graph: what
/// an approval writes, and nothing a concurrent hook stamps on a project (`lastActive`).
fn record_quads(ns: &crate::config::NamespaceConfig, tier_cwd: &Path) -> HashSet<[String; 4]> {
    let Ok(store) = crud::load_workspace_graph(tier_cwd) else { return HashSet::new() };
    let kinds: Vec<String> = ["rule", "decision", "domain", "proposal"].iter().map(|k| format!("<{}{k}/", ns.uri)).collect();
    store
        .iter()
        .filter_map(Result::ok)
        .filter(|q| !q.object.is_blank_node())
        .map(|q| [q.subject.to_string(), q.predicate.to_string(), q.object.to_string(), q.graph_name.to_string()])
        .filter(|q| kinds.iter().any(|k| q[0].starts_with(k.as_str())))
        .collect()
}

/// Each tier's `domains.toml` text, by path.
fn toml_texts(cwd: &Path) -> Vec<(PathBuf, String)> {
    tiers(cwd)
        .into_iter()
        .filter_map(|(t, c)| crate::domain::tier::domains_toml_for(&c, t))
        .filter_map(|f| std::fs::read_to_string(&f).ok().map(|t| (f, t)))
        .collect()
}

/// Approve each of `version`'s proposals through BO-16's review, recording what each wrote. A failure undoes what the
/// earlier ones wrote, and says so.
fn approve_all(config: &BaseConfig, cwd: &Path, version: &Version, broad_ok: bool) -> Result<Undo, String> {
    // Every proposal still pending here, or nothing is written: one approved or rejected since the start (or not held by
    // this folder's tiers) would fail part way, after the backups.
    if let Some(gone) = super::report::not_pending(config, cwd, version).first() {
        return Err(format!("{gone} is no longer pending here (approved or rejected since the start, or not in this folder's tiers)"));
    }
    // BO-12's backup path for store edits, and a copy of each domains.toml, before anything is written. The snapshot is
    // taken under the graph lock, as every other graph backup is, so no locked writer is part way through it.
    let op = format!("pre-{}", version.name);
    for (_, c) in tiers(cwd) {
        if let Ok(path) = crud::workspace_graph_path(&c)
            && path.is_file()
        {
            crate::store::with_graph_lock(&path, || crate::store::snapshot(&path, &op))
                .map_err(|e| format!("backing up {}: {e:#}", path.display()))?;
        }
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    for (f, _) in toml_texts(cwd) {
        let backup = f.with_file_name(format!("domains.toml.BAK-{stamp}-{op}"));
        std::fs::copy(&f, &backup).map_err(|e| format!("backing up {}: {e}", f.display()))?;
    }
    let mut steps: Vec<Step> = Vec::new();
    for p in &version.proposals {
        let graphs_before: Vec<(PathBuf, HashSet<[String; 4]>)> = tiers(cwd)
            .into_iter()
            .map(|(_, c)| {
                let q = record_quads(&config.namespace, &c);
                (c, q)
            })
            .collect();
        let tomls_before = toml_texts(cwd);
        let action = crate::corrections::review::Action::Approve { id: p.id.clone(), broad_ok };
        if let Err(why) = crate::corrections::review::run(config, cwd, &action) {
            let undone = undo_steps(config, &steps, &version.name);
            return Err(match undone {
                Ok(()) => format!("{} could not be approved ({why}); the {} approved before it were undone", p.id, steps.len()),
                Err(back) => format!("{} could not be approved ({why}), and undoing the earlier ones failed: {back}", p.id),
            });
        }
        let mut step = Step { proposal: p.id.clone(), graphs: Vec::new(), tomls: Vec::new() };
        for (c, before) in graphs_before {
            let after = record_quads(&config.namespace, &c);
            let added: Vec<[String; 4]> = after.difference(&before).cloned().collect();
            let removed: Vec<[String; 4]> = before.difference(&after).cloned().collect();
            if !added.is_empty() || !removed.is_empty() {
                step.graphs.push(GraphDiff { tier_cwd: c.display().to_string(), added, removed });
            }
        }
        for (f, after) in toml_texts(cwd) {
            let before = tomls_before.iter().find(|(g, _)| *g == f).map(|(_, t)| t.clone()).unwrap_or_default();
            if before != after {
                step.tomls.push((f.display().to_string(), before, after));
            }
        }
        steps.push(step);
    }
    Ok(Undo::Proposals { steps })
}

/// Put back what `steps` wrote, last first: the `domains.toml` texts, then the graph quads (a supersede edge pair through
/// `supersede::unlink_update`, the rest through one `DELETE DATA` and one `INSERT DATA` per tier). Each tier's graph is
/// snapshotted first, as before an approval.
fn undo_steps(config: &BaseConfig, steps: &[Step], restoring: &str) -> Result<(), String> {
    let ns = &config.namespace;
    let op = format!("pre-{restoring}");
    let mut snapshotted: HashSet<String> = HashSet::new();
    for step in steps.iter().rev() {
        for (file, before, after) in &step.tomls {
            restore_toml(Path::new(file), before, after)?;
        }
        for g in &step.graphs {
            let tier_cwd = PathBuf::from(&g.tier_cwd);
            let mut sparql: Vec<String> = Vec::new();
            // A supersede edge pair the approval wrote goes through `unlink_update`, which removes exactly the three quads
            // `link_update` writes (lynx's Q3 condition); the old record's status before is in `removed`, if it had one.
            let sup = format!("<{}{}>", ns.uri, crate::supersede::PRED_SUPERSEDES);
            let sup_by = format!("<{}{}>", ns.uri, crate::supersede::PRED_SUPERSEDED_BY);
            let status = format!("<{}status>", ns.uri);
            let superseded = format!("\"{}\"", crate::supersede::STATUS_SUPERSEDED);
            // (old, new, graph) of each pair.
            let pairs: Vec<(&str, &str, &str)> =
                g.added.iter().filter(|q| q[1] == sup).map(|q| (q[2].as_str(), q[0].as_str(), q[3].as_str())).collect();
            for (old, new, graph) in &pairs {
                sparql.push(crate::supersede::unlink_update(ns, unbracket(graph), unbracket(old), unbracket(new)));
            }
            let in_pair = |q: &[String; 4]| {
                pairs.iter().any(|(old, new, _)| {
                    (q[1] == sup && q[0] == *new && q[2] == *old)
                        || (q[1] == sup_by && q[0] == *old && q[2] == *new)
                        || (q[1] == status && q[0] == *old && q[2] == superseded)
                })
            };
            let rest: Vec<&[String; 4]> = g.added.iter().filter(|q| !in_pair(q)).collect();
            if !rest.is_empty() {
                sparql.push(format!("DELETE DATA {{\n{}}}", data_block(&rest)));
            }
            let removed: Vec<&[String; 4]> = g.removed.iter().collect();
            if !removed.is_empty() {
                sparql.push(format!("INSERT DATA {{\n{}}}", data_block(&removed)));
            }
            if sparql.is_empty() {
                continue;
            }
            let (store, trig_path, _lock) = crud::lock_and_load(&tier_cwd).map_err(|e| format!("{e:#}"))?;
            // Each tier's graph is backed up once per undo, under the lock just taken, before its first write.
            if snapshotted.insert(g.tier_cwd.clone()) && trig_path.is_file() {
                crate::store::snapshot(&trig_path, &op).map_err(|e| format!("backing up {}: {e:#}", trig_path.display()))?;
            }
            crate::store::update_and_write(
                &store,
                &trig_path,
                &format!("{}\n{}", crud::prefixes(ns), sparql.join(" ;\n")),
                crate::store::Scope::Wide,
                crate::store::Intent::Knowledge,
            )
            .map_err(|e| format!("{e:#}"))?;
        }
    }
    Ok(())
}

fn unbracket(t: &str) -> &str {
    t.trim_start_matches('<').trim_end_matches('>')
}

/// `GRAPH <g> { s p o . }` lines for a `DELETE DATA` or `INSERT DATA`.
fn data_block(quads: &[&[String; 4]]) -> String {
    let mut by_graph: Vec<(&str, Vec<String>)> = Vec::new();
    for q in quads {
        let line = format!("    {} {} {} .", q[0], q[1], q[2]);
        match by_graph.iter_mut().find(|(g, _)| *g == q[3]) {
            Some((_, v)) => v.push(line),
            None => by_graph.push((&q[3], vec![line])),
        }
    }
    by_graph.into_iter().map(|(g, lines)| format!("  GRAPH {g} {{\n{}\n  }}\n", lines.join("\n"))).collect()
}

/// A `domains.toml` put back: the text before when the file still holds what the approval wrote; otherwise the approval's
/// changes taken back out of what it holds now (keywords it added dropped and those it dropped added back, rules it
/// added removed and those it removed put back), so an edit made since stays.
fn restore_toml(path: &Path, before: &str, after: &str) -> Result<(), String> {
    let now = std::fs::read_to_string(path).unwrap_or_default();
    let text = if now == after {
        before.to_string()
    } else {
        use crate::domain::DomainsFile;
        let parse = |t: &str| toml::from_str::<DomainsFile>(t).map_err(|e| format!("{}: {e}", path.display()));
        let (b, a, mut cur) = (parse(before)?, parse(after)?, parse(&now)?);
        for d in cur.domain.iter_mut() {
            let slug = crud::slugify(&d.name);
            let find = |f: &DomainsFile| f.domain.iter().find(|x| crud::slugify(&x.name) == slug).cloned();
            let (Some(db), Some(da)) = (find(&b), find(&a)) else { continue };
            let added: Vec<String> = da.prompt_keywords.iter().filter(|k| !db.prompt_keywords.contains(k)).cloned().collect();
            let dropped: Vec<String> = db.prompt_keywords.iter().filter(|k| !da.prompt_keywords.contains(k)).cloned().collect();
            d.prompt_keywords = crate::domain::replay::edit_list(&d.prompt_keywords, &dropped, &added);
            let id = |r: &crate::domain::RuleEntry| crate::domain::rules::rule_id(&d.name, r.text());
            let ids_b: Vec<String> = db.rules.iter().map(|r| crate::domain::rules::rule_id(&db.name, r.text())).collect();
            let ids_a: Vec<String> = da.rules.iter().map(|r| crate::domain::rules::rule_id(&da.name, r.text())).collect();
            d.rules.retain(|r| ids_b.contains(&id(r)) || !ids_a.contains(&id(r)));
            for r in db.rules.iter().filter(|r| !ids_a.contains(&crate::domain::rules::rule_id(&db.name, r.text()))) {
                if !d.rules.iter().any(|x| x.text() == r.text()) {
                    d.rules.push(r.clone());
                }
            }
        }
        toml::to_string_pretty(&cur).map_err(|e| format!("{}: {e}", path.display()))?
    };
    let tmp = path.with_extension("toml.shadow-tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    crate::store::rename_with_retry(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The rates a report shows beside a watched promotion: before it, over the typed prompts since (read from the folder
/// it was judged in), and the margin a rise must pass when there is one to compare.
pub fn watched_rates(config: &BaseConfig, cwd: &Path, p: &Promotion) -> Option<(Rate, Rate, Option<f64>)> {
    let at = report::epoch(&p.at)?;
    let sh = report::scan(&p.folder_or(cwd), "", Some(at));
    let n = config.shadow.watch_prompts.max(1);
    let (_, after) = windows(&sh, at, n);
    Some((p.before, after, noise_margin(&p.before, &after, config.shadow.rollback_sd)))
}

/// The watch so far, in one line for the report (lynx's Q6 ruling: the report shows both rates).
pub fn watch_line(config: &BaseConfig, cwd: &Path, p: &Promotion) -> Option<String> {
    let (before, since, margin) = watched_rates(config, cwd, p)?;
    Some(format!(
        "watch: corrections {:.0} per 100 over the {} typed prompts before it, {:.0} per 100 over {} of [shadow] watch_prompts \
         = {} since{}",
        before.per_100(),
        before.prompts,
        since.per_100(),
        since.prompts,
        config.shadow.watch_prompts,
        margin.map(|m| format!("; it is rolled back past a rise of {m:.0}")).unwrap_or_default()
    ))
}
