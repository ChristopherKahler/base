//! The upgrade needs no action from the user or their Claude (BO-26): after the auto-update, nothing a user has is
//! damaged, nothing asks them or their Claude to run a command, and the store comes out in better shape than it went in.
//!
//! The auto-update swaps the binary and touches nothing else, so the new binary's first session start is where the rest
//! of an upgrade happens. It does three things, once per version and tier, under lynx's G0 rulings:
//!
//! - **U1, the store repair**: [`crate::fix::run_tier`], the function `base doctor --fix --yes` runs, with its snapshot
//!   before any write. It costs 4.5 s on a copy of Chris's store, so it runs in a background process: the first session
//!   start on the new version starts `base hook upgrade` with no stdin, stdout or stderr and returns; the next session
//!   start prints what it did.
//! - **U2's mechanical part**: a path trigger written relative is written out as the full path it resolves to
//!   ([`triggers`]), and the triggers 0.15.2 left inert and that fire now are named once.
//! - **U3**: a starter command still as an earlier base shipped it gets this build's text ([`commands`]).
//! - **BO-28**: a base.toml still holding the 0.15 installer's `[devmode] enabled = true` line is marked, and the next
//!   session start turns developer mode off and says so first on its screen ([`devmode`]).
//!
//! Every change is backed up first and announced once with its undo, `base doctor --restore <backup>` (U5). The
//! announcements wait in each tier's record until a session start prints them. Nothing a user wrote is deleted.
//!
//! ## When it runs
//!
//! Each tier's `.base/upgrade.json` names the version it was last brought up to. A tier is due when its record names
//! another version and the home was upgraded, which the global record says, or, before there is one, a version stamp an
//! older base left in `~/.base-gbl` (`.hooks-wired-<version>`, written by every session start since 0.13). A home with
//! neither is new: the global record is written as `fresh` and nothing else happens, so a new user's first session
//! start prints nothing (G0 condition 4), and a test's freshly built home is left alone.
//!
//! The record is written as the last step of a tier's upgrade, with the version, so a process stopped part way leaves
//! the tier without this version's record and the next session start tries again (G0 condition 5). The lines it had
//! produced by then are kept in the record without the version, so none is lost. A store repair that fails is recorded
//! as failed, said once with the command to run it by hand, and not tried again for this version.
//!
//! `[graph] auto_migrate = false` turns all of it off.

pub mod commands;
pub mod devmode;
pub mod triggers;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The version this build brings a store up to.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Each tier's record, in its `.base` folder.
pub const RECORD: &str = "upgrade.json";

/// The lock one upgrade process holds, in the global tier's `.base` folder.
const LOCK: &str = "upgrade.lock";

/// Set and not empty: base starts no background process, and session start runs the upgrade itself before it returns.
/// Fake homes and tests set it, so what they measure is decided when the hook exits.
pub const NO_SPAWN_ENV: &str = "BASE_NO_SPAWN";

/// One tier's record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The version this tier was last brought up to; empty while an upgrade of it has not finished.
    #[serde(default)]
    pub version: String,
    /// The version the home was upgraded from. Absent on a home base came to new (`status = "fresh"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default)]
    pub at: String,
    /// `fresh`, `done` or `failed`.
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Lines no session start has printed yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub announce: Vec<String>,
    /// The developer-mode step (BO-28): absent unless this tier's base.toml held the installer's line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devmode_off: Option<devmode::Off>,
}

/// A tier as the upgrade sees it.
#[derive(Debug, Clone)]
pub struct Tier {
    /// `global` or `workspace`, as doctor names it.
    pub label: &'static str,
    /// The tier's `.base` folder: its graph and its record.
    pub store: PathBuf,
    /// The folder of its `domains.toml`, `commands.toml` and `base.toml`.
    pub config: PathBuf,
    /// What its relative path triggers resolve against: home for the global tier, the workspace root for a workspace.
    pub root: PathBuf,
}

/// What is due from `cwd`.
#[derive(Debug, Clone)]
pub struct Due {
    /// The version the home was upgraded from.
    pub from: String,
    /// The tiers without this version's record, global first.
    pub tiers: Vec<Tier>,
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The global tier, then the workspace `cwd` stands in when it is another folder.
pub fn tiers(cwd: &Path) -> Vec<Tier> {
    let mut out = Vec::new();
    let Some(home) = crate::home::home_root() else { return out };
    let gbl = home.join(".base-gbl");
    if gbl.is_dir() {
        out.push(Tier { label: "global", store: gbl.join(".base"), config: gbl.clone(), root: home.clone() });
    }
    if let Some(base_dir) = crate::config::find_workspace_base(cwd)
        && canonical(&base_dir) != canonical(&gbl.join(".base"))
        && let Some(root) = base_dir.parent()
    {
        out.push(Tier { label: "workspace", store: base_dir.clone(), config: base_dir.clone(), root: root.to_path_buf() });
    }
    out
}

/// A tier's record, when it has one that parses.
pub fn read_record(store: &Path) -> Option<Record> {
    serde_json::from_str(&std::fs::read_to_string(store.join(RECORD)).ok()?).ok()
}

fn write_record(store: &Path, record: &Record) -> Result<()> {
    std::fs::create_dir_all(store).with_context(|| format!("creating {}", store.display()))?;
    let path = store.join(RECORD);
    let tmp = store.join(format!("{RECORD}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(record)?).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))
}

/// `0.15.2` before `0.16.0`, part by part; a part that is not a number compares as text.
fn version_order(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |v: &str| -> Vec<(u64, String)> {
        v.split(['.', '-']).map(|p| (p.parse().unwrap_or(0), p.to_string())).collect()
    };
    parts(a).cmp(&parts(b))
}

/// The newest version an older base stamped in `gbl` (`~/.base-gbl`): `.hooks-wired-<version>[-<hash>]` other than this
/// build's own stamp, or `.update-noticed-<version>` of another version. `None` when no older base ran here.
pub fn older_stamp(gbl: &Path) -> Option<String> {
    let own = crate::install::hooks_wired_stamp();
    let mut found: Vec<String> = Vec::new();
    for e in std::fs::read_dir(gbl).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(rest) = name.strip_prefix(".hooks-wired-") {
            if name != own && !rest.ends_with(".failed") {
                // `0.15.2`, or `0.15.2-1a2b3c4d` from a build that hashes its hook table.
                let v = match rest.rsplit_once('-') {
                    Some((v, hash)) if hash.len() == 8 && hash.chars().all(|c| c.is_ascii_hexdigit()) => v,
                    _ => rest,
                };
                found.push(v.to_string());
            }
        } else if let Some(v) = name.strip_prefix(".update-noticed-")
            && v != VERSION
        {
            found.push(v.to_string());
        }
    }
    found.into_iter().max_by(|a, b| version_order(a, b))
}

/// What is due from `cwd`, or `None`. A new home (no record, no older stamp) gets its global record here, as `fresh`.
pub fn plan(cwd: &Path) -> Option<Due> {
    let tiers = tiers(cwd);
    let global = tiers.iter().find(|t| t.label == "global")?;
    let record = read_record(&global.store);
    let from = match &record {
        Some(r) if r.version == VERSION => r.from.clone(),
        Some(r) if !r.version.is_empty() => Some(r.version.clone()),
        Some(r) => r.from.clone().or_else(|| older_stamp(&global.config)),
        None => older_stamp(&global.config),
    };
    let Some(from) = from else {
        // Only into a global tier that exists: creating `~/.base-gbl/.base` would make it one.
        if global.store.is_dir() && record.as_ref().is_none_or(|r| r.version != VERSION) {
            let fresh = Record { version: VERSION.to_string(), at: now(), status: "fresh".into(), ..Default::default() };
            let _ = write_record(&global.store, &fresh);
        }
        return None;
    };
    let due: Vec<Tier> = tiers
        .into_iter()
        .filter(|t| read_record(&t.store).is_none_or(|r| r.version != VERSION))
        .collect();
    (!due.is_empty()).then_some(Due { from, tiers: due })
}

fn now() -> String {
    chrono::Local::now().to_rfc3339()
}

/// How long an upgrade process waits for another to finish before it leaves the work to the next session start.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// The upgrade lock, waiting up to `wait`: `None` while another process still holds it. An OS lock, released when the
/// process ends however it ends (the shadow module's pattern).
fn lock(store: &Path, wait: std::time::Duration) -> Option<std::fs::File> {
    std::fs::create_dir_all(store).ok()?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(store.join(LOCK)).ok()?;
    let until = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Some(file),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

/// Copy `file` aside as `<name>.BAK-<date>-pre-<version>`, the backup `base doctor --restore` takes back.
pub fn backup(file: &Path, version: &str) -> Result<PathBuf> {
    let name = file.file_name().and_then(|n| n.to_str()).context("a config file has no name")?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut to = file.with_file_name(format!("{name}.BAK-{stamp}-pre-{version}"));
    let mut n = 2;
    while to.exists() {
        to = file.with_file_name(format!("{name}.BAK-{stamp}-{n}-pre-{version}"));
        n += 1;
    }
    std::fs::copy(file, &to).with_context(|| format!("backing up {} to {}", file.display(), to.display()))?;
    Ok(to)
}

// ─── Session start ─────────────────────────────────────────────────────────

/// What earlier upgrades did and no session start has printed, global tier first, taken out of the records so the next
/// session start prints none of it again (U5). Nothing while an upgrade process holds the lock: its lines wait.
pub fn announcements(cwd: &Path) -> Vec<String> {
    let tiers = tiers(cwd);
    let Some(global) = tiers.iter().find(|t| t.label == "global") else { return Vec::new() };
    if !tiers.iter().any(|t| read_record(&t.store).is_some_and(|r| !r.announce.is_empty())) {
        return Vec::new();
    }
    let Some(_lock) = lock(&global.store, std::time::Duration::ZERO) else { return Vec::new() };
    let mut out = Vec::new();
    for t in &tiers {
        let Some(mut r) = read_record(&t.store) else { continue };
        if r.announce.is_empty() {
            continue;
        }
        let lines = std::mem::take(&mut r.announce);
        if write_record(&t.store, &r).is_ok() {
            out.extend(lines);
        }
    }
    out
}

/// After session start has printed: start the upgrade when it is due (U1). The background process gets no stdin, stdout
/// or stderr, so the hook exits at once and Claude Code waits on nothing. With [`NO_SPAWN_ENV`] set, or when this process
/// is not base itself (a test calling session start in process), it runs here.
pub fn after_session_start(config: &crate::config::BaseConfig, cwd: &Path) {
    if !config.graph.auto_migrate || plan(cwd).is_none() {
        return;
    }
    let inline = || {
        if let Err(e) = run(cwd) {
            eprintln!("base: the upgrade could not run: {e:#}");
        }
    };
    if std::env::var_os(NO_SPAWN_ENV).is_some_and(|v| !v.is_empty()) {
        return inline();
    }
    let Ok(exe) = std::env::current_exe() else { return };
    if exe.file_stem().and_then(|s| s.to_str()) != Some("base") {
        return inline();
    }
    // `detached::spawn`: no stdio, and on Windows not this hook's own pipes either, or Claude Code would wait for the
    // whole upgrade as if it were the hook (U7).
    let _ = crate::detached::spawn(std::process::Command::new(exe).args(["hook", "upgrade"]).current_dir(cwd));
}

// ─── The upgrade process ───────────────────────────────────────────────────

/// `base hook upgrade`: bring every due tier up to this version. One process at a time: another waits for it (up to
/// [`LOCK_WAIT`]) and then finds nothing due.
pub fn run(cwd: &Path) -> Result<()> {
    let Some(global) = tiers(cwd).into_iter().find(|t| t.label == "global") else { return Ok(()) };
    let Some(_lock) = lock(&global.store, LOCK_WAIT) else { return Ok(()) };
    if !crate::config::BaseConfig::load(cwd).graph.auto_migrate {
        return Ok(());
    }
    let Some(due) = plan(cwd) else { return Ok(()) };
    let mut ctx: Option<crate::domain::matcher::TriggerContext> = None;
    for tier in &due.tiers {
        // One tier's error is that tier's: recorded and said once, and the next tier still runs. The process has no
        // stderr, so an error returned from here would reach nobody.
        if let Err(e) = upgrade_tier(cwd, tier, &due.from, &mut ctx) {
            record_failed(tier, &due.from, &format!("{e:#}"));
        }
        // BO-28, after the tier's record is written whatever it came to: a store repair that failed does not keep the
        // installer's developer mode on.
        devmode::mark(tier);
    }
    Ok(())
}

/// Save the lines so far without the version: a process stopped after this keeps them and the tier stays due.
fn progress(tier: &Tier, record: &Record) -> Result<()> {
    write_record(&tier.store, record)
}

/// Add a line to say, unless it is already waiting: a tier retried after a stop part way keeps the lines it had saved and
/// produces the same ones again.
fn announce(record: &mut Record, line: String) {
    if !record.announce.contains(&line) {
        record.announce.push(line);
    }
}

fn failure_line(tier: &Tier, why: &str) -> String {
    format!(
        "upgrade: the store repair for {VERSION} did not finish on the {} tier: {why}; what it could not repair was left \
         as it was · not tried again automatically; by hand: base doctor --fix --yes",
        tier.label
    )
}

/// A tier whose upgrade stopped on an error: recorded as failed for this version, with the lines it had produced and one
/// naming the error, so it is said once and not tried again automatically (G0, Q1). When even the record cannot be
/// written there is nowhere to say it, and the next session start tries again.
fn record_failed(tier: &Tier, from: &str, why: &str) {
    let mut record = read_record(&tier.store).unwrap_or_default();
    record.from = Some(from.to_string());
    announce(&mut record, failure_line(tier, why));
    record.version = VERSION.to_string();
    record.at = now();
    record.status = "failed".into();
    record.error = Some(why.to_string());
    let _ = write_record(&tier.store, &record);
}

fn upgrade_tier(cwd: &Path, tier: &Tier, from: &str, ctx: &mut Option<crate::domain::matcher::TriggerContext>) -> Result<()> {
    let graph = tier.store.join("graph.nq");
    // A graph that does not parse: session start already says so every time, and `--repair` comes first. No record, so
    // the next session start tries again.
    if graph.exists() && !matches!(crate::store::graph_health(&graph), crate::store::GraphHealth::Healthy) {
        return Ok(());
    }
    let mut record = read_record(&tier.store).unwrap_or_default();
    record.from = Some(from.to_string());
    let mut failed: Option<String> = None;

    // U1 (and U6): the store repair, the one `base doctor --fix --yes` runs, on this tier and its base.toml. The base.toml
    // is copied aside first, which `--fix` does not do (G0, Q4).
    let base_toml = tier.config.join("base.toml");
    let legacy = crate::config::BaseConfig::legacy_keys(cwd)
        .into_iter()
        .any(|k| k.section == "signal" && k.key == "max_chars" && canonical(&k.file) == canonical(&base_toml));
    let toml_backup = if legacy { Some(backup(&base_toml, VERSION)?) } else { None };
    if graph.exists() || legacy {
        let report = crate::fix::run_tier(cwd, true, tier.label);
        for line in repair_lines(tier, &report, toml_backup.as_deref()) {
            announce(&mut record, line);
        }
        if report.has_errors() {
            let why: Vec<String> = report
                .tiers
                .iter()
                .filter_map(|t| t.error.clone())
                .chain(report.config.iter().filter_map(|c| c.error.clone()))
                .collect();
            failed = Some(why.join("; "));
        } else if legacy && report.config.is_empty() {
            // The key was there and nothing planned its repair: the backup stays, and says so.
            failed = Some(format!("[signal] max_chars in {} was not repaired", base_toml.display()));
        }
        progress(tier, &record)?;
    }

    // U2: relative triggers written out, and Q2b's one line.
    let domains = tier.config.join("domains.toml");
    match triggers::rewrite_relative(&domains, &tier.root, VERSION) {
        Ok(r) if !r.changed.is_empty() => {
            let shown: Vec<String> = r.changed.iter().take(3).map(|(_, t, f)| format!("`{t}` -> `{f}`")).collect();
            let more = r.changed.len().saturating_sub(shown.len());
            let tail = if more > 0 { format!(" and {more} more") } else { String::new() };
            announce(&mut record, format!(
                "upgrade: {} path trigger(s) in the {} domains.toml written as their full paths, the same folders ({}{tail}){}",
                r.changed.len(),
                tier.label,
                shown.join(", "),
                undo(r.backup.as_deref())
            ));
        }
        Ok(_) => {}
        // Not a store failure: the triggers still fire as written, and doctor's advice line stays.
        Err(e) => eprintln!("base: the upgrade left the relative triggers in {} as they were: {e:#}", domains.display()),
    }
    // Q2b, for a home coming from 0.15 or older: the triggers F29 left inert there fire since BO-10.
    let inert = if version_order(from, "0.16.0").is_lt() {
        let ctx = ctx.get_or_insert_with(|| crate::domain::trigger_context(cwd));
        triggers::newly_firing(&domains, &tier.root, ctx)
    } else {
        Vec::new()
    };
    if !inert.is_empty() {
        announce(&mut record, format!(
            "upgrade: path triggers 0.15 left inert now fire on files outside the projects they hold: {}; to narrow them: \
             base domain paths --suggest",
            inert.join(", ")
        ));
    }
    progress(tier, &record)?;

    // U3: the starter commands.
    let file = tier.config.join("commands.toml");
    match commands::upgrade(&file, VERSION) {
        Ok(c) => {
            for line in command_lines(tier, &c) {
                announce(&mut record, line);
            }
        }
        Err(e) => eprintln!("base: the upgrade left {} as it was: {e:#}", file.display()),
    }

    record.version = VERSION.to_string();
    record.at = now();
    record.status = if failed.is_some() { "failed".into() } else { "done".into() };
    if let Some(why) = &failed {
        announce(&mut record, failure_line(tier, why));
    }
    record.error = failed;
    write_record(&tier.store, &record)
}

fn undo(backup: Option<&Path>) -> String {
    backup.map(|b| format!(" · undo: base doctor --restore \"{}\"", b.display())).unwrap_or_default()
}

/// One line for what the store repair changed in this tier, and one for its base.toml. Nothing when it changed nothing
/// (G0 condition 3).
fn repair_lines(tier: &Tier, report: &crate::fix::Report, toml_backup: Option<&Path>) -> Vec<String> {
    use crate::fix::Dest;
    let mut out = Vec::new();
    for t in report.tiers.iter().filter(|t| t.error.is_none() && t.skipped.is_none()) {
        let mut parts: Vec<String> = Vec::new();
        let moved: Vec<&crate::fix::ForeignGraph> =
            t.foreign.iter().filter(|f| !matches!(f.dest, Dest::Left { .. })).collect();
        if !moved.is_empty() {
            let quads: usize = moved.iter().map(|f| f.quads).sum();
            let records: usize = moved.iter().map(|f| f.records.len()).sum();
            let mut to: Vec<String> = moved
                .iter()
                .map(|f| match &f.dest {
                    Dest::Workspace { path } => path.clone(),
                    Dest::File { .. } => ".base/foreign-*.nq".to_string(),
                    Dest::Left { .. } => String::new(),
                })
                .collect();
            to.sort();
            to.dedup();
            parts.push(format!("{records} record(s) of other workspaces ({quads} quads) moved to {}", to.join(", ")));
        }
        if !t.corrections.linked.is_empty() {
            parts.push(format!("{} correction(s) linked to the record they correct", t.corrections.linked.len()));
        }
        if !t.supersession.is_empty() {
            parts.push(format!("{} supersession disagreement(s) settled", t.supersession.len()));
        }
        let mut snapshot = t.snapshot.clone();
        if let Some(c) = &t.compact
            && c.how == "base graph compact"
        {
            parts.push(format!("compacted, {} -> {} lines", c.lines_before, c.lines_after));
            snapshot = snapshot.or_else(|| c.backup.clone());
        }
        if !t.backups.removed.is_empty() {
            parts.push(format!(
                "{} of base's own older graph snapshots removed ([graph] keep_backups = {})",
                t.backups.removed.len(),
                t.backups.keep
            ));
        }
        if parts.is_empty() {
            continue;
        }
        out.push(format!(
            "upgrade: the {} store for {VERSION}: {}{}",
            tier.label,
            parts.join("; "),
            undo(snapshot.as_deref().map(Path::new))
        ));
    }
    for c in report.config.iter().filter(|c| c.error.is_none()) {
        let value = c.max_chars.map_or("(not a number)".to_string(), |v| v.to_string());
        let what = if c.installer {
            format!(
                "[signal] max_chars = {value} removed (the 0.15 installer's value; the memory block keeps its default, \
                 [budget] memory_chars = {})",
                c.memory_chars
            )
        } else if c.moved {
            format!("[signal] max_chars = {value} moved to [budget] memory_chars")
        } else {
            format!("[signal] max_chars = {value} removed ([budget] memory_chars = {} is set)", c.memory_chars)
        };
        out.push(format!("upgrade: {} base.toml: {what}{}", tier.label, undo(toml_backup)));
    }
    out
}

/// The lines for one `commands.toml` (U3, Example 2 and Example 3).
fn command_lines(tier: &Tier, c: &commands::Outcome) -> Vec<String> {
    let stars = |names: &[String]| names.iter().map(|n| format!("*{n}")).collect::<Vec<_>>().join(", ");
    let mut out = Vec::new();
    if !c.updated.is_empty() {
        out.push(format!(
            "upgrade: {} in the {} commands.toml updated to this version's text (they were as an older base shipped them){}",
            stars(&c.updated),
            tier.label,
            undo(c.backup.as_deref())
        ));
    }
    if !c.linked_old.is_empty() {
        out.push(format!(
            "upgrade: the {} commands.toml is a link, so it was left as it is; its {} still has an older base's text \
             (this version's: base commands show <name> --shipped)",
            tier.label,
            stars(&c.linked_old)
        ));
    }
    if c.old_rule_add {
        out.push(format!(
            "upgrade: your *base command still uses the old rule add line; {VERSION}'s rule add takes --keywords and \
             --fires-on (this version's *base: base commands show base --shipped)"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_by_number_not_by_text() {
        assert_eq!(version_order("0.9.1", "0.15.2"), std::cmp::Ordering::Less);
        assert_eq!(version_order("0.15.2", "0.16.0"), std::cmp::Ordering::Less);
        assert_eq!(version_order("0.16.0", "0.16.0"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn an_older_stamp_is_any_stamp_but_this_builds_own() {
        let dir = tempfile::tempdir().unwrap();
        let gbl = dir.path();
        std::fs::write(gbl.join(crate::install::hooks_wired_stamp()), b"").unwrap();
        std::fs::write(gbl.join(format!(".update-noticed-{VERSION}")), b"").unwrap();
        assert_eq!(older_stamp(gbl), None, "this build's own stamps say nothing about an older base");
        std::fs::write(gbl.join(".hooks-wired-0.14.2"), b"").unwrap();
        std::fs::write(gbl.join(".hooks-wired-0.15.2-1a2b3c4d"), b"").unwrap();
        std::fs::write(gbl.join(".hooks-wired-0.15.2-1a2b3c4d.failed"), b"").unwrap();
        assert_eq!(older_stamp(gbl).as_deref(), Some("0.15.2"));
    }
}
