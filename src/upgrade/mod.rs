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
//! Every change is backed up first and announced once with its undo, `base doctor --restore <backup>` (U5), in a
//! person's words under [`LEAD`] (BO-30). One line names another way back: the 0.15 installer's `max_chars`, which
//! nothing reads any more, so its restore would change nothing; that line names the setting that does. The
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

/// The first words of every line the upgrade prints (BO-30): base and the version that made the change, so a person and
/// their Claude can tell where the line came from and which release notes explain it. The lines after it are written for
/// a person: what changed, why that is good for them, and how to undo it, with every command exact.
pub const LEAD: &str = concat!("base ", env!("CARGO_PKG_VERSION"), ":");

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
        "{LEAD} The cleanup of {} base data did not finish: {why}. What it could not fix was left as it was, and base \
         will not try again by itself. To run it yourself: `base doctor --fix --yes`.",
        whose(tier)
    )
}

/// How a line names a tier: "your global" or "this workspace's". A workspace's lines are printed only by a session start
/// in that workspace ([`announcements`] reads the tiers of its own folder), so "this" is the one the person is in.
fn whose(tier: &Tier) -> &'static str {
    if tier.label == "global" { "your global" } else { "this workspace's" }
}

/// `1 entry`, `2 entries`.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `4000` as `4,000`, the way a person reads a number.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
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
            failed = Some(format!("the old max_chars setting in {} was not removed", base_toml.display()));
        }
        progress(tier, &record)?;
    }

    // U2: relative triggers written out, and Q2b's one line.
    let domains = tier.config.join("domains.toml");
    match triggers::rewrite_relative(&domains, &tier.root, VERSION) {
        Ok(r) if !r.changed.is_empty() => announce(&mut record, relative_line(tier, &r)),
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
    if let Some(line) = newly_firing_line(&inert) {
        announce(&mut record, line);
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

/// The sentence that ends a line with its undo, `base doctor --restore "<backup>"` exactly, in backticks so the period
/// after it is never copied into the command. Nothing when there is no backup.
fn undo(lead: &str, backup: Option<&Path>) -> String {
    backup.map(|b| format!(" {lead}, run `base doctor --restore \"{}\"`.", b.display())).unwrap_or_default()
}

/// U2's line: the relative triggers written out as the full paths they already resolve to, the first three shown.
fn relative_line(tier: &Tier, r: &triggers::Rewrite) -> String {
    let mut shown: Vec<String> = r.changed.iter().take(3).map(|(_, t, f)| format!("`{t}` is now `{f}`")).collect();
    let more = r.changed.len().saturating_sub(shown.len());
    if more > 0 {
        shown.push(format!("{more} more"));
    }
    format!(
        "{LEAD} Your rules tied to folders and files now spell out the whole path ({} in {} domains.toml), so {}. They \
         cover the same places as before.{}",
        r.changed.len(),
        whose(tier),
        and_list(&shown),
        undo("To undo it", r.backup.as_deref())
    )
}

/// Q2b's line: the rules of a folder that holds more than one project, which 0.15 ignored, and that load now.
fn newly_firing_line(inert: &[triggers::Firing]) -> Option<String> {
    let narrow = "To narrow them, run `base domain paths --suggest`.";
    match inert {
        [] => None,
        [one] => Some(format!(
            "{LEAD} The rules of `{}` tied to `{}` now load for files in that folder that sit outside the {} projects in it. \
             0.15 ignored a folder that held more than one project. {narrow}",
            one.domain, one.trigger, one.projects
        )),
        many => {
            let each: Vec<String> =
                many.iter().map(|f| format!("`{}` (rules of `{}`, {} projects)", f.trigger, f.domain, f.projects)).collect();
            Some(format!(
                "{LEAD} Rules tied to a folder that holds more than one project now load for files in it that sit outside \
                 those projects: {}. 0.15 ignored such a folder. {narrow}",
                and_list(&each)
            ))
        }
    }
}

/// One line for what the store repair changed in this tier, and one for its base.toml. Nothing when it changed nothing
/// (G0 condition 3). Written for a person (BO-30): entries, not records or quads; where the moved ones went and whether
/// they still show up; one short clause for each of the rest; the undo last.
fn repair_lines(tier: &Tier, report: &crate::fix::Report, toml_backup: Option<&Path>) -> Vec<String> {
    use crate::fix::Dest;
    let mut out = Vec::new();
    // Where moved entries stop showing up: a workspace's data shows in that workspace, the global data in every one.
    let here = if tier.label == "global" { "in every workspace" } else { "here" };
    for t in report.tiers.iter().filter(|t| t.error.is_none() && t.skipped.is_none()) {
        // Into a workspace registered and reachable here, where they show up now; or into a file of their own beside
        // this tier's graph, which nothing reads.
        let (mut back, mut folders, mut aside, mut files) = (0usize, Vec::new(), 0usize, Vec::new());
        for f in &t.foreign {
            match &f.dest {
                Dest::Workspace { path } => {
                    back += f.records.len();
                    folders.push(workspace_folder(path));
                }
                Dest::File { path, .. } => {
                    aside += f.records.len();
                    files.push(path.as_str());
                }
                Dest::Left { .. } => {}
            }
        }
        folders.sort();
        folders.dedup();
        files.sort();
        files.dedup();
        let mut sentences: Vec<String> = Vec::new();
        if back > 0 {
            let at: Vec<String> = folders.iter().map(|f| format!("`{f}`")).collect();
            let (whom, into) = if at.len() == 1 {
                (format!("your workspace at {}", at[0]), "back into it")
            } else {
                (format!("your workspaces at {}", and_list(&at)), "back into them")
            };
            let (belongs, shows) = if back == 1 { ("belongs", "it shows") } else { ("belong", "they show") };
            sentences.push(format!(
                "We moved {} that {belongs} to {whom} {into}; {shows} up there now, not {here}.",
                count(back, "entry", "entries")
            ));
        }
        if aside > 0 {
            let owner = if files.len() == 1 { "another workspace" } else { "other workspaces" };
            let (belongs, they, shows) =
                if aside == 1 { ("belongs", "It is", "it no longer shows") } else { ("belong", "They are", "they no longer show") };
            let file = if files.len() == 1 { "a separate file" } else { "separate files" };
            sentences.push(format!(
                "We set aside {} that {belongs} to {owner} we could not find on this computer. {they} saved in {file} in \
                 {} .base folder, so nothing was lost, but {shows} up {here}.",
                count(aside, "entry", "entries"),
                whose(tier)
            ));
        }
        let mut changes = usize::from(back > 0) + usize::from(aside > 0);
        let mut also: Vec<String> = Vec::new();
        let mixups = t.corrections.linked.len() + t.supersession.len();
        if mixups > 0 {
            also.push(format!(
                "fixed {} in how entries point to the ones they replace",
                count(mixups, "small mix-up", "small mix-ups")
            ));
            changes += 1;
        }
        let mut snapshot = t.snapshot.clone();
        if let Some(c) = &t.compact
            && c.how == "base graph compact"
        {
            also.push("removed repeated lines".to_string());
            snapshot = snapshot.or_else(|| c.backup.clone());
            changes += 1;
        }
        let deleted = t.backups.removed.len();
        if deleted > 0 {
            also.push(format!(
                "deleted {}, keeping the newest {}",
                count(deleted, "older backup copy", "older backup copies"),
                t.backups.keep
            ));
        }
        if !also.is_empty() {
            sentences.push(if sentences.is_empty() {
                format!("In {} base data, we {}.", whose(tier), and_list(&also))
            } else {
                format!("We also {}.", and_list(&also))
            });
        }
        if sentences.is_empty() {
            continue;
        }
        // A restore puts the graph back; it cannot bring back a deleted backup copy.
        let lead = if deleted > 0 {
            "To undo the rest"
        } else if changes > 1 {
            "To undo all of it"
        } else {
            "To undo it"
        };
        out.push(format!("{LEAD} {}{}", sentences.join(" "), undo(lead, snapshot.as_deref().map(Path::new))));
    }
    for c in report.config.iter().filter(|c| c.error.is_none()) {
        let put_back = undo("To put the old setting back", toml_backup);
        out.push(match c.max_chars {
            None => format!(
                "{LEAD} We removed an old setting from {} base.toml that was not a number; this version does not read \
                 it.{put_back}",
                whose(tier)
            ),
            // U6: 0.16 reads the old key nowhere, so a restore brings back a line nothing reads. The real way back to a
            // smaller memory block is its own setting (lynx's G0 ruling, row 1).
            Some(v) if c.installer => format!(
                "{LEAD} Claude now gets up to {} characters of your saved notes when a session starts. An old setting from \
                 the 0.15 installer held them, with a few other items, to {}. To set a lower limit again, run \
                 `base config set budget.memory_chars {v}`.",
                thousands(c.memory_chars),
                thousands(v)
            ),
            Some(v) if c.moved => format!(
                "{LEAD} Your own limit of {} characters, from an old setting in {} base.toml, now applies to your saved \
                 notes alone; it used to cover them and a few other items.{put_back}",
                thousands(v),
                whose(tier)
            ),
            Some(v) => format!(
                "{LEAD} We removed an old setting ({} characters) from {} base.toml that this version does not read; your \
                 saved notes keep the limit of {} characters you set.{put_back}",
                thousands(v),
                whose(tier),
                thousands(c.memory_chars)
            ),
        });
    }
    out
}

/// The folder of the workspace whose graph is at `graph` (`<folder>/.base/graph.nq`), as the line names it.
fn workspace_folder(graph: &str) -> String {
    let p = Path::new(graph);
    p.parent().and_then(Path::parent).unwrap_or(p).display().to_string()
}

/// The lines for one `commands.toml` (U3, Example 2 and Example 3).
fn command_lines(tier: &Tier, c: &commands::Outcome) -> Vec<String> {
    let stars = |names: &[String]| and_list(&names.iter().map(|n| format!("*{n}")).collect::<Vec<_>>());
    let mut out = Vec::new();
    if !c.updated.is_empty() {
        let (noun, has, them) = if c.updated.len() == 1 { ("command", "has", "it") } else { ("commands", "have", "them") };
        out.push(format!(
            "{LEAD} Your {} {noun} in {} commands.toml now {has} this version's text. You had not changed {them} from what \
             an older base shipped, so nothing of yours was lost.{}",
            stars(&c.updated),
            whose(tier),
            undo("To undo it", c.backup.as_deref())
        ));
    }
    if !c.linked_old.is_empty() {
        let (still, show) = match c.linked_old.as_slice() {
            [one] => ("command still has", format!("base commands show {one} --shipped")),
            _ => ("commands still have", "base commands show <name> --shipped".to_string()),
        };
        out.push(format!(
            "{LEAD} Your {} {still} an older base's text. We left {} commands.toml as it is because it is a link to another \
             file. To see this version's text, run `{show}`.",
            stars(&c.linked_old),
            whose(tier)
        ));
    }
    if c.old_rule_add {
        out.push(format!(
            "{LEAD} Your *base command still has the old rule add line. We left it as it is because you changed that \
             command. Inside a Claude Code session, rule add now needs --keywords and --fires-on; to see this version's \
             *base, run `base commands show base --shipped`."
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

    // ─── BO-30: the lines are written for a person ─────────────────────────

    use crate::fix::{Compaction, Dest, Disagreement, ForeignGraph, TierFix};

    fn at(label: &'static str) -> Tier {
        let (store, config) = if label == "global" {
            ("/home/u/.base-gbl/.base", "/home/u/.base-gbl")
        } else {
            ("/home/u/work/.base", "/home/u/work/.base")
        };
        Tier { label, store: store.into(), config: config.into(), root: "/home/u/work".into() }
    }

    const SNAP: &str = "/home/u/work/.base/graph.nq.bak-fix-2026-10-05-090000";
    const BAK: &str = "/home/u/.base-gbl/base.toml.BAK-20261005-090000-pre-0.16.0";

    fn tier_fix(snapshot: Option<&str>) -> TierFix {
        TierFix {
            tier: "workspace".into(),
            path: "/home/u/work/.base/graph.nq".into(),
            skipped: None,
            foreign: Vec::new(),
            corrections: Default::default(),
            supersession: Vec::new(),
            compact: None,
            backups: Default::default(),
            snapshot: snapshot.map(String::from),
            error: None,
        }
    }

    fn foreign(records: usize, dest: Dest) -> ForeignGraph {
        ForeignGraph {
            graph: "http://base/graph/ws/gone".into(),
            workspace: "gone".into(),
            quads: records * 2,
            kinds: Vec::new(),
            records: (0..records).map(|i| format!("note/n{i}")).collect(),
            dest,
            left_behind: 0,
        }
    }

    fn aside(name: &str) -> Dest {
        Dest::File { path: format!("/home/u/work/.base/foreign-{name}.nq"), why: "no registered workspace".into() }
    }

    fn into_workspace(folder: &str) -> Dest {
        Dest::Workspace { path: format!("{folder}/.base/graph.nq") }
    }

    fn config(max_chars: Option<i64>, moved: bool, installer: bool, memory_chars: i64) -> crate::fix::ConfigFix {
        crate::fix::ConfigFix { file: "/home/u/.base-gbl/base.toml".into(), max_chars, moved, installer, memory_chars, error: None }
    }

    fn store_line(tier: &Tier, t: TierFix) -> String {
        let report = crate::fix::Report { applied: true, tiers: vec![t], config: Vec::new() };
        let lines = repair_lines(tier, &report, None);
        assert_eq!(lines.len(), 1, "{lines:?}");
        lines[0].clone()
    }

    fn config_line(c: crate::fix::ConfigFix) -> String {
        let report = crate::fix::Report { applied: true, tiers: Vec::new(), config: vec![c] };
        repair_lines(&at("global"), &report, Some(Path::new(BAK))).remove(0)
    }

    /// Every line the upgrade, its developer-mode step and the reminder pass can print, one for each case each builder
    /// has (the rows of BO-30's G0 table), with the command the person can run.
    fn every_line() -> Vec<(&'static str, String)> {
        let (ws, gbl) = (at("workspace"), at("global"));
        let mut out: Vec<(&'static str, String)> = vec![
            ("1 the installer's max_chars", config_line(config(Some(2000), false, true, 4000))),
            ("2 a max_chars the user chose, moved", config_line(config(Some(3000), true, false, 3000))),
            ("3 max_chars removed, memory_chars set", config_line(config(Some(3000), false, false, 5000))),
            ("3b max_chars not a number", config_line(config(None, true, false, 4000))),
        ];

        let mut t = tier_fix(Some(SNAP));
        t.foreign = vec![foreign(13, aside("gone"))];
        t.corrections.linked = vec![("correction/c1".into(), "note/n1".into())];
        t.supersession = vec![Disagreement::EdgeAdded { record: "note/a".into(), by: "note/b".into() }];
        out.push(("4 set aside, two mix-ups", store_line(&ws, t)));
        let mut t = tier_fix(Some(SNAP));
        t.foreign = vec![foreign(1, aside("gone"))];
        out.push(("4 one entry set aside", store_line(&ws, t)));
        let mut t = tier_fix(Some(SNAP));
        t.foreign = vec![foreign(4, aside("gone")), foreign(2, aside("left"))];
        out.push(("4 set aside into two files, global", store_line(&gbl, t)));
        let mut t = tier_fix(Some(SNAP));
        t.foreign = vec![foreign(2, into_workspace("/home/u/other"))];
        out.push(("4a back into a workspace", store_line(&ws, t)));
        let mut t = tier_fix(Some(SNAP));
        t.foreign = vec![foreign(1, into_workspace("/home/u/a")), foreign(3, into_workspace("/home/u/b")), foreign(5, aside("gone"))];
        out.push(("4a back into two workspaces, and set aside", store_line(&ws, t)));
        let mut t = tier_fix(None);
        t.compact = Some(Compaction { how: "base graph compact", lines_before: 1200, lines_after: 1100, backup: Some(SNAP.into()) });
        out.push(("4b repeated lines alone", store_line(&ws, t)));
        let mut t = tier_fix(Some(SNAP));
        t.foreign = vec![foreign(2, aside("gone"))];
        t.backups.keep = 3;
        t.backups.removed = vec![("graph.nq.bak-1".into(), 10), ("graph.nq.bak-2".into(), 10)];
        out.push(("4c set aside, old copies deleted", store_line(&ws, t)));
        let mut t = tier_fix(None);
        t.backups.keep = 3;
        t.backups.removed = vec![("graph.nq.bak-1".into(), 10)];
        out.push(("4c one old copy deleted alone", store_line(&gbl, t)));

        out.push(("5 the cleanup did not finish", failure_line(&ws, "the old max_chars setting in /home/u/work/.base/base.toml was not removed")));
        out.push(("5 the cleanup did not finish, global", failure_line(&gbl, "permission denied")));

        let one = triggers::Rewrite {
            changed: vec![("notes".into(), "notes".into(), "/home/u/work/notes".into())],
            backup: Some("/home/u/work/.base/domains.toml.BAK-20261005-090000-pre-0.16.0".into()),
        };
        out.push(("6 one path written out", relative_line(&ws, &one)));
        let many = triggers::Rewrite {
            changed: (0..5).map(|i| ("d".to_string(), format!("p{i}"), format!("/home/u/work/p{i}"))).collect(),
            backup: Some("/home/u/work/.base/domains.toml.BAK-20261005-090000-pre-0.16.0".into()),
        };
        out.push(("6 five paths written out", relative_line(&ws, &many)));

        let firing = |trigger: &str, domain: &str, projects| triggers::Firing { trigger: trigger.into(), domain: domain.into(), projects };
        out.push(("7 one folder now loads", newly_firing_line(&[firing("/home/u/tools", "toolbox", 2)]).unwrap()));
        out.push((
            "7 two folders now load",
            newly_firing_line(&[firing("/home/u/tools", "toolbox", 2), firing("/home/u/docs", "notes", 3)]).unwrap(),
        ));

        let updated = commands::Outcome {
            updated: vec!["handoff".into(), "base".into()],
            backup: Some("/home/u/.base-gbl/commands.toml.BAK-20261005-090000-pre-0.16.0".into()),
            ..Default::default()
        };
        out.push(("8 starter commands updated", command_lines(&gbl, &updated).remove(0)));
        let one_updated = commands::Outcome { updated: vec!["base".into()], backup: updated.backup.clone(), ..Default::default() };
        out.push(("8 one starter command updated", command_lines(&ws, &one_updated).remove(0)));
        let linked = commands::Outcome { linked_old: vec!["handoff".into(), "base".into()], ..Default::default() };
        out.push(("9 linked commands.toml", command_lines(&gbl, &linked).remove(0)));
        let linked_one = commands::Outcome { linked_old: vec!["base".into()], ..Default::default() };
        out.push(("9 linked commands.toml, one command", command_lines(&gbl, &linked_one).remove(0)));
        let old = commands::Outcome { old_rule_add: true, ..Default::default() };
        out.push(("10 the old rule add line", command_lines(&gbl, &old).remove(0)));

        let file = Path::new("/home/u/.base-gbl/base.toml");
        out.push(("12 the developer-mode paragraph", devmode::paragraph(&gbl, file, Path::new(BAK), VERSION)));
        let ws_file = Path::new("/home/u/work/.base/base.toml");
        out.push(("12a the paragraph, a workspace's file", devmode::paragraph(&ws, ws_file, Path::new(BAK), VERSION)));
        out.push(("13 a linked base.toml", devmode::linked_line(&gbl, file)));
        out.push(("13 a linked base.toml, a workspace's", devmode::linked_line(&ws, ws_file)));
        out.push(("14 developer mode left on", devmode::failed_line(&gbl, file, "access is denied")));
        out.push(("14 developer mode left on, a workspace's", devmode::failed_line(&ws, ws_file, "access is denied")));

        let archived = crate::crud::reminder::AutoArchived {
            slug: "renew-the-domain".into(),
            name: "renew the domain".into(),
            days: 12,
            warned_on: "2026-10-03".into(),
        };
        out.push(("17 a reminder archived", archived.line()));
        out
    }

    /// The line with every backticked part taken out: commands and paths, which the person copies as they are.
    fn words_outside_backticks(line: &str) -> String {
        line.split('`').step_by(2).collect::<Vec<_>>().join(" ")
    }

    /// BO-30: no line holds base's internal words (quads, `.nq` files, tiers, the graph or the store, `record(s)`), and
    /// no `[section] key` a person is not told to set. Commands and paths in backticks are left out of the check: the
    /// person copies those exactly as they are, `.nq` in a backup's name included.
    #[test]
    fn upgrade_lines_use_no_internal_terms() {
        let lines = every_line();
        assert!(lines.len() >= 30, "every row is reached: {}", lines.len());
        for (row, line) in &lines {
            let prose = words_outside_backticks(line);
            let words: Vec<String> =
                prose.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
            for internal in ["quad", "quads", "nq", "tier", "tiers", "graph", "graphs", "store", "stores", "record", "records"] {
                assert!(!words.iter().any(|w| w == internal), "row {row}: {internal:?} in\n{line}");
            }
            assert!(!line.contains("(s)"), "row {row}: a (s) plural in\n{line}");
            assert!(!line.contains(" · "), "row {row}: a log-style separator in\n{line}");
            // A `[section]`, with or without a key after it, only where the line tells the person to set one.
            for (i, _) in line.match_indices('[') {
                let inner: String = line[i + 1..].chars().take_while(|c| *c != ']').collect();
                if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                    assert!(line[..i].ends_with("under `"), "row {row}: [{inner}] is not something the person sets:\n{line}");
                }
            }
            // Where it came from, first: base and this version, the reminder pass's `base:`, or the paragraph's own
            // first words.
            let lead = if row.starts_with("17") {
                "base: "
            } else if row.starts_with("12") {
                "base "
            } else {
                LEAD
            };
            assert!(line.starts_with(lead), "row {row}: does not start with {lead:?}:\n{line}");
            assert!(line.ends_with('.'), "row {row}: not a finished sentence:\n{line}");
        }
    }

    /// BO-30: every line that had a command to undo or act on still carries it byte for byte, now in backticks so the
    /// period after it is never copied with it. The commands are the ones each line printed at `07750bf`.
    ///
    /// Row 1 is the one line whose way back changed (lynx's G0 ruling): 0.16 reads the old `max_chars` nowhere, so its
    /// restore brought back a line nothing reads and was no way back at all. The line now names the setting that is.
    #[test]
    fn every_upgrade_line_keeps_its_undo() {
        let lines = every_line();
        let line = |row: &str| -> &str {
            &lines.iter().find(|(r, _)| *r == row).unwrap_or_else(|| panic!("no row {row}")).1
        };
        let restore = |path: &str| format!("base doctor --restore \"{path}\"");
        let domains_bak = restore("/home/u/work/.base/domains.toml.BAK-20261005-090000-pre-0.16.0");
        let commands_bak = restore("/home/u/.base-gbl/commands.toml.BAK-20261005-090000-pre-0.16.0");
        let cases: Vec<(&str, String)> = vec![
            ("2 a max_chars the user chose, moved", restore(BAK)),
            ("3 max_chars removed, memory_chars set", restore(BAK)),
            ("4 set aside, two mix-ups", restore(SNAP)),
            ("4a back into a workspace", restore(SNAP)),
            ("4b repeated lines alone", restore(SNAP)),
            ("4c set aside, old copies deleted", restore(SNAP)),
            ("5 the cleanup did not finish", "base doctor --fix --yes".into()),
            ("6 one path written out", domains_bak.clone()),
            ("6 five paths written out", domains_bak),
            ("7 one folder now loads", "base domain paths --suggest".into()),
            ("7 two folders now load", "base domain paths --suggest".into()),
            ("8 starter commands updated", commands_bak),
            ("9 linked commands.toml", "base commands show <name> --shipped".into()),
            ("10 the old rule add line", "base commands show base --shipped".into()),
            ("12 the developer-mode paragraph", "base log matches".into()),
            ("12 the developer-mode paragraph", "base config set devmode.enabled true".into()),
            ("12 the developer-mode paragraph", restore(BAK)),
            ("13 a linked base.toml", "base config set devmode.enabled false".into()),
            ("14 developer mode left on", "base config set devmode.enabled false".into()),
            ("17 a reminder archived", "base reminder unarchive renew-the-domain".into()),
        ];
        for (row, command) in &cases {
            assert!(line(row).contains(&format!("`{command}`")), "row {row} lost `{command}`:\n{}", line(row));
        }
        // Rows 12a, 13 and 14 for a workspace's file: the line says which file to edit and what to set in it.
        assert!(line("12a the paragraph, a workspace's file").contains("set `enabled = true` under `[devmode]` in /home/u/work/.base/base.toml"));
        assert!(line("13 a linked base.toml, a workspace's").contains("set `enabled = false` under `[devmode]` in that file."));
        assert!(line("14 developer mode left on, a workspace's").contains("set `enabled = false` under `[devmode]` in /home/u/work/.base/base.toml."));
        // A deleted backup copy cannot be restored, so a line that deleted some says the restore undoes the rest.
        assert!(line("4c set aside, old copies deleted").contains(&format!("To undo the rest, run `{}`.", restore(SNAP))));
        assert!(!line("4c one old copy deleted alone").contains("--restore"), "{}", line("4c one old copy deleted alone"));

        let row1 = line("1 the installer's max_chars");
        assert!(row1.contains("`base config set budget.memory_chars 2000`"), "{row1}");
        assert!(!row1.contains("--restore"), "the restore brings back a line 0.16 never reads:\n{row1}");
    }

    /// The facts each line states, by case: how many, where they went, and whether they still show up.
    #[test]
    fn the_lines_say_what_happened_in_plain_words() {
        let lines = every_line();
        let line = |row: &str| lines.iter().find(|(r, _)| *r == row).map(|(_, l)| l.clone()).unwrap();
        assert_eq!(
            line("1 the installer's max_chars"),
            format!(
                "{LEAD} Claude now gets up to 4,000 characters of your saved notes when a session starts. An old setting \
                 from the 0.15 installer held them, with a few other items, to 2,000. To set a lower limit again, run \
                 `base config set budget.memory_chars 2000`."
            )
        );
        assert_eq!(
            line("4 set aside, two mix-ups"),
            format!(
                "{LEAD} We set aside 13 entries that belong to another workspace we could not find on this computer. They \
                 are saved in a separate file in this workspace's .base folder, so nothing was lost, but they no longer \
                 show up here. We also fixed 2 small mix-ups in how entries point to the ones they replace. To undo all \
                 of it, run `base doctor --restore \"{SNAP}\"`."
            )
        );
        assert!(line("4 one entry set aside").contains("We set aside 1 entry that belongs to another workspace"));
        assert!(line("4 one entry set aside").contains("It is saved in a separate file") && line("4 one entry set aside").contains("it no longer shows up here. To undo it,"));
        assert!(line("4 set aside into two files, global").contains("6 entries that belong to other workspaces"));
        assert!(line("4 set aside into two files, global").contains("in separate files in your global .base folder"));
        assert!(line("4 set aside into two files, global").contains("no longer show up in every workspace"));
        assert!(line("4a back into a workspace").contains(
            "We moved 2 entries that belong to your workspace at `/home/u/other` back into it; they show up there now, not here."
        ));
        assert!(line("4a back into two workspaces, and set aside").contains("your workspaces at `/home/u/a` and `/home/u/b` back into them"));
        assert!(line("4a back into two workspaces, and set aside").contains("We set aside 5 entries"));
        assert!(line("4b repeated lines alone").starts_with(&format!("{LEAD} In this workspace's base data, we removed repeated lines.")));
        assert!(line("4c one old copy deleted alone").contains("we deleted 1 older backup copy, keeping the newest 3."));
        assert!(line("6 five paths written out").contains("so `p0` is now `/home/u/work/p0`, `p1` is now `/home/u/work/p1`, `p2` is now `/home/u/work/p2` and 2 more."));
        assert!(line("8 starter commands updated").contains("Your *handoff and *base commands in your global commands.toml now have"));
        assert!(line("8 one starter command updated").contains("Your *base command in this workspace's commands.toml now has"));
        assert!(line("9 linked commands.toml, one command").contains("`base commands show base --shipped`"));
        assert_eq!(thousands(4000), "4,000");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(999), "999");
    }
}
