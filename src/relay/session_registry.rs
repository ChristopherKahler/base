//! Global session title registry — the cross-workspace rendezvous that makes
//! session-targeted task relay possible.
//!
//! The per-project [`RelayStore`](super::RelayStore) registry binds titles to
//! session ids WITHIN one workspace's disposable spool. This registry is its
//! global sibling: a single JSON file at `~/.base-gbl/.base/sessions.json` that
//! any session, in any workspace, can read and write. That's the whole point —
//! session A (toolbox) must be able to resolve a friendly title to session B's
//! id even though B lives in a different workspace graph entirely.
//!
//! A session claims a title with `base relay register --as <title>`; its hooks
//! then heartbeat the binding (throttled) so liveness stays fresh. `*task
//! <title> …` resolves the title here to find the live target session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{now_iso, parse_ts, write_json_atomic, read_json, IDLE_AFTER_SECS};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionEntry {
    pub title: String,
    pub session_id: String,
    /// The session's working directory when it registered (for the board + disambiguation).
    #[serde(default)]
    pub cwd: String,
    /// Derived workspace name for display.
    #[serde(default)]
    pub workspace: String,
    pub registered_at: String,
    pub last_heartbeat: String,
    /// Windows Terminal tab id (WT_SESSION env) captured at bind time. It
    /// survives /clear — the new session in the same tab reclaims this title
    /// instead of drawing a fresh codename (Chris's handoff→clear flow).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub wt_session: String,
    /// True when this title was auto-assigned from the codename wordlist.
    /// An explicit `relay register` drops the session's auto titles — the
    /// boot-order ghost (auto-name lands before the explicit register in
    /// every boot sequence) would otherwise shadow the session forever.
    #[serde(default)]
    pub auto: bool,
    /// Every project that EVER touched this session (Chris spec 2026-08-17):
    /// `register --project X` appends, nothing removes — the ping hub filters
    /// session cards by these keywords.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    /// The session that used this title: it sent an inbox item under it, was sent one under it, or was launched with
    /// it pinned by `BASE_RELAY_AS` (BO-27, V2). It counts only while that session holds the title, so a title handed
    /// to another session starts unused, as its inbox does (BO-05).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub used_by: String,
}

impl SessionEntry {
    pub fn alive(&self) -> bool {
        parse_ts(&self.last_heartbeat)
            .map(|t| (chrono::Local::now() - t).num_seconds() < IDLE_AFTER_SECS)
            .unwrap_or(false)
    }

    /// The title is in use: set by hand, or the session holding it sent or was sent an inbox item under it (BO-27,
    /// V2). Only a title in use is reminded to start an inbox watcher. Before BO-27 every session start drew a title
    /// for every session and the hooks told each one to arm a watcher, relay user or not.
    pub fn in_use(&self) -> bool {
        !self.auto || (!self.used_by.is_empty() && self.used_by == self.session_id)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionRegistry {
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionEntry>,
}

/// `~/.base-gbl/.base/` — the global tier base dir. `None` only if there is no home dir.
pub fn global_base_dir() -> Option<PathBuf> {
    crate::home::home_root().map(|h| h.join(".base-gbl").join(".base"))
}

fn registry_path() -> Option<PathBuf> {
    global_base_dir().map(|d| d.join("sessions.json"))
}

pub fn load() -> SessionRegistry {
    registry_path()
        .and_then(|p| read_json(&p))
        .unwrap_or_default()
}

/// Bind (or re-bind) a title to the current session. Re-registering the same
/// title with a fresh session id is the normal path — a new Claude session
/// reclaims its stable title.
///
/// BO-05 (F12): a title that passes to a new session does not pass its inbox. What the previous holder was sent is
/// archived, and the senders of anything it never saw are told ([`super::task_inbox::settle`]).
pub fn register(title: &str, session_id: &str, cwd: &Path, project: Option<&str>) -> Result<()> {
    let previous = with_lock(|| {
        let mut reg = load();
        let now = now_iso();
        let workspace = workspace_name(cwd);
        let entry = reg.sessions.entry(title.to_string()).or_insert_with(|| SessionEntry {
            title: title.to_string(),
            registered_at: now.clone(),
            ..Default::default()
        });
        if let Some(p) = project
            && !p.is_empty()
            && !entry.projects.iter().any(|x| x == p)
        {
            entry.projects.push(p.to_string());
        }
        let previous = (!entry.session_id.is_empty() && entry.session_id != session_id).then(|| entry.session_id.clone());
        if entry.session_id != session_id {
            record_holder(title, session_id, &now);
        }
        entry.session_id = session_id.to_string();
        entry.cwd = cwd.to_string_lossy().to_string();
        entry.workspace = workspace;
        entry.last_heartbeat = now;
        entry.auto = false;
        entry.wt_session = std::env::var("WT_SESSION").unwrap_or_default();
        // Claiming a real title retires this session's auto-codename ghosts —
        // otherwise every registered session carries a wordlist title it never
        // asked for (and, under the wake contract, gets nudged to arm it).
        reg.sessions
            .retain(|_, e| !(e.session_id == session_id && e.auto && e.title != title));
        save(&reg)?;
        Ok(previous)
    })?;
    super::task_inbox::settle(title, session_id, previous.as_deref());
    Ok(())
}

/// Refresh the heartbeat for whatever title(s) the current session holds.
/// No-op when this session hasn't claimed a title. Callers throttle this so it
/// doesn't rewrite the file on every hook — see [`should_heartbeat`].
pub fn heartbeat(session_id: &str) {
    let _ = with_lock(|| {
        let mut reg = load();
        let now = now_iso();
        let mut dirty = false;
        for e in reg.sessions.values_mut() {
            if e.session_id == session_id {
                e.last_heartbeat = now.clone();
                dirty = true;
            }
        }
        if dirty {
            save(&reg)?;
        }
        Ok(())
    });
}

/// True if the session's binding is stale enough to warrant a heartbeat write
/// (>60s). Keeps hook writes rare — liveness doesn't need per-tool granularity.
pub fn should_heartbeat(session_id: &str) -> bool {
    load()
        .sessions
        .values()
        .filter(|e| e.session_id == session_id)
        .any(|e| {
            parse_ts(&e.last_heartbeat)
                .map(|t| (chrono::Local::now() - t).num_seconds() >= 60)
                .unwrap_or(true)
        })
}

/// Resolve a title to its bound session entry, if the title exists.
pub fn resolve(title: &str) -> Option<SessionEntry> {
    load().sessions.get(title).cloned()
}

/// Every title currently bound to this session id. A session can hold more than
/// one; a task addressed to any of them is for this session. Empty when the
/// session has never registered — such a session can't be a relay target.
pub fn titles_for(session_id: &str) -> Vec<String> {
    load()
        .sessions
        .values()
        .filter(|e| e.session_id == session_id)
        .map(|e| e.title.clone())
        .collect()
}

/// The titles this session holds that are in use ([`SessionEntry::in_use`]): the ones the hooks remind to start an
/// inbox watcher. The same single read of the registry as [`titles_for`].
pub fn titles_in_use_for(session_id: &str) -> Vec<String> {
    load()
        .sessions
        .values()
        .filter(|e| e.session_id == session_id && e.in_use())
        .map(|e| e.title.clone())
        .collect()
}

/// Record that each `(title, session)` pair used the title, where that session holds it and it is not in use yet
/// (BO-27, V2). Reads the registry first and takes the lock only when something changes, so a send or a delivery to
/// a title already in use costs one read. Best-effort: a registry that cannot be written leaves the reminder off,
/// never a ping unsent.
pub fn mark_used(pairs: &[(&str, &str)]) {
    let due = |reg: &SessionRegistry, title: &str, sid: &str| {
        !sid.is_empty() && reg.sessions.get(title).is_some_and(|e| e.session_id == sid && !e.in_use())
    };
    let reg = load();
    if !pairs.iter().any(|(t, s)| due(&reg, t, s)) {
        return;
    }
    let _ = with_lock(|| {
        let mut reg = load();
        let mut dirty = false;
        for (title, sid) in pairs {
            if due(&reg, title, sid)
                && let Some(e) = reg.sessions.get_mut(*title)
            {
                e.used_by = (*sid).to_string();
                dirty = true;
            }
        }
        if dirty {
            save(&reg)?;
        }
        Ok(())
    });
}

pub fn list() -> Vec<SessionEntry> {
    load().sessions.into_values().collect()
}

/// The title this session goes by: one it registered rather than one auto-assigned, the newest first. `None` when
/// the session holds no title. A handoff's default lane is this name when nothing nearer names its author (BO-11).
pub fn title_of(session_id: &str) -> Option<String> {
    let mut held: Vec<SessionEntry> = list().into_iter().filter(|e| e.session_id == session_id).collect();
    held.sort_by(|a, b| a.auto.cmp(&b.auto).then_with(|| b.registered_at.cmp(&a.registered_at)));
    held.into_iter().next().map(|e| e.title)
}

// ─── Title history (BO-05, F12e) ─────────────────────────────
//
// `sessions.json` holds who has a title NOW. This file holds who had it before: one line each time a title binds to
// a different session, and one seed line for each title already held when this build first writes the registry. It
// is what lets an inbox item that carries no session id be read as addressed to the session that held its title when
// it was written, what says which session sent an item that does not record its sender's session, and what a notice
// names as the time the new holder took the title. Nothing before a title's first line can be placed: an item from
// then is archived, never delivered, and its sender is not told, since base cannot say which session sent it.

/// One line of the title history: from `since`, `title` belonged to `session_id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Holder {
    pub title: String,
    pub session_id: String,
    pub since: String,
    /// A seed line: `since` is when base first saw this holder (the first registry write after the install), not when
    /// it took the title. Good for placing what came after it; never shown as the time a session took a title.
    #[serde(default, skip_serializing_if = "is_false")]
    pub observed: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Past this size the history is rewritten to the lines a lookup can still need ([`compact_history`]).
const HISTORY_CAP_BYTES: u64 = 256 * 1024;

/// How far back the history keeps lines once it is over [`HISTORY_CAP_BYTES`]. A ping older than a day is stale and
/// archived anyway; a task is not, so the history covers a month of them, at most [`HISTORY_KEEP_LINES`] lines.
const HISTORY_KEEP_DAYS: i64 = 30;

/// The most recent lines compaction keeps, besides each registered title's newest. About 130 KB, half the cap, so a
/// compacted file has room to grow before the next compaction instead of being rewritten on every line.
const HISTORY_KEEP_LINES: usize = 1_000;

fn history_path() -> Option<PathBuf> {
    global_base_dir().map(|d| d.join("title-history.jsonl"))
}

/// Append that `title` now belongs to `session_id`. Called under the registry lock, so lines never interleave.
/// Best-effort: a history that cannot be written makes old items unplaceable, never a registration fail.
fn record_holder(title: &str, session_id: &str, since: &str) {
    append_holder(&Holder { title: title.into(), session_id: session_id.into(), since: since.into(), observed: false });
}

fn append_holder(line: &Holder) {
    use std::io::Write as _;
    let Some(path) = history_path() else { return };
    let Ok(json) = serde_json::to_string(line) else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{json}");
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > HISTORY_CAP_BYTES) {
        compact_history(&path);
    }
}

/// Keep each registered title's newest line, and of the lines from the last [`HISTORY_KEEP_DAYS`] days the newest
/// [`HISTORY_KEEP_LINES`]; drop the rest. A title no longer in the registry loses its lines with age, so the file stays
/// bounded however many codenames come and go.
fn compact_history(path: &Path) {
    let lines = read_history(path);
    let registered: std::collections::BTreeSet<String> = load().sessions.into_keys().collect();
    let cutoff = chrono::Local::now() - chrono::Duration::days(HISTORY_KEEP_DAYS);
    let mut newest: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, h) in lines.iter().enumerate() {
        newest.insert(&h.title, i);
    }
    let recent_from = lines.len().saturating_sub(HISTORY_KEEP_LINES);
    let kept: String = lines
        .iter()
        .enumerate()
        .filter(|(i, h)| {
            (registered.contains(&h.title) && newest.get(h.title.as_str()) == Some(i))
                || (*i >= recent_from && parse_ts(&h.since).is_some_and(|t| t >= cutoff))
        })
        .filter_map(|(_, h)| serde_json::to_string(h).ok())
        .map(|j| j + "\n")
        .collect();
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&tmp, kept).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn read_history(path: &Path) -> Vec<Holder> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

/// Every title the history has a line for, whoever holds it now.
pub fn history_titles() -> Vec<String> {
    history_path().map(|p| read_history(&p)).unwrap_or_default().into_iter().map(|h| h.title).collect()
}

/// Every recorded holder of `title`, oldest first.
pub fn holders(title: &str) -> Vec<Holder> {
    let mut out: Vec<Holder> =
        history_path().map(|p| read_history(&p)).unwrap_or_default().into_iter().filter(|h| h.title == title).collect();
    out.sort_by_key(|h| parse_ts(&h.since));
    out
}

/// The session that held `title` at `at`, from the history: the newest line for the title from no later than `at`.
/// `None` when the history has no line for the title that early, which means it cannot be known.
pub fn holder_at(title: &str, at: chrono::DateTime<chrono::Local>) -> Option<String> {
    holders(title)
        .into_iter()
        .rev()
        .find(|h| parse_ts(&h.since).is_some_and(|t| t <= at))
        .map(|h| h.session_id)
}

/// When `session_id` took `title`, if the history recorded it: the newest such line that is not a seed line.
pub fn held_since(title: &str, session_id: &str) -> Option<String> {
    holders(title).into_iter().rev().find(|h| h.session_id == session_id && !h.observed).map(|h| h.since)
}

/// Short, distinct, easy-to-type codenames auto-assigned to unnamed sessions.
/// Kept to memorable single words so `*task <name> …` stays frictionless.
const WORDLIST: &[&str] = &[
    "falcon", "otter", "cobra", "lynx", "heron", "bison", "raven", "koala",
    "gecko", "panda", "tiger", "wolf", "moose", "crane", "viper", "badger",
    "ferret", "marlin", "osprey", "jackal", "mantis", "walrus", "puffin", "dingo",
    "quokka", "tapir", "narwhal", "cougar", "egret", "finch", "gopher", "hawk",
    "jaguar", "kestrel", "lemur", "meerkat", "ocelot", "pelican", "quail", "robin",
    "seal", "toucan", "vole", "weasel", "zebra", "wren", "stoat", "orca",
];

/// Ensure this session holds a title (auto-assigning a codename if it doesn't)
/// and keep its liveness fresh. Returns the session's primary title. Called from
/// boundary hooks (session-start, prompt). Honors `BASE_NO_AUTONAME` to opt out.
pub fn touch(session_id: &str, cwd: &Path) -> Option<String> {
    touch_with(session_id, cwd, true)
}

/// `touch` with the auto-codename switchable: `auto_name = false` (the
/// `[relay] enabled = false` setting) refreshes a title the session already
/// holds and never draws one for it.
pub fn touch_with(session_id: &str, cwd: &Path, auto_name: bool) -> Option<String> {
    let held = titles_for(session_id);
    if let Some(t) = held.first().cloned() {
        if should_heartbeat(session_id) {
            heartbeat(session_id);
        }
        // A launcher's pinned title is set by hand (BO-27, V2), including for a session that took it before this
        // build wrote `used_by`.
        if let Some(pinned) = relay_as().filter(|p| held.contains(p)) {
            mark_used(&[(pinned.as_str(), session_id)]);
        }
        return Some(t);
    }
    if !auto_name || std::env::var_os("BASE_NO_AUTONAME").is_some() {
        return None;
    }
    let (title, previous) = auto_register(session_id, cwd).ok()?;
    // BO-05 (F12): the title's inbox is settled for its new holder, as on an explicit register.
    super::task_inbox::settle(&title, session_id, previous.as_deref());
    Some(title)
}

/// Draw or reclaim a title for an unnamed session. Returns the title and, when it was taken from another session,
/// that session's id.
fn auto_register(session_id: &str, cwd: &Path) -> Result<(String, Option<String>)> {
    with_lock(|| {
        let mut reg = load();
        prune_dead(&mut reg, session_id);
        // Re-check under the lock — a concurrent boundary hook may have named us.
        if let Some(t) = reg
            .sessions
            .values()
            .find(|e| e.session_id == session_id)
            .map(|e| e.title.clone())
        {
            return Ok((t, None));
        }
        // Same-tab continuity: /clear starts a NEW session id in the SAME
        // Windows Terminal tab (WT_SESSION persists). A title whose tab id
        // matches ours but whose session id differs is this tab's
        // predecessor — reclaim it instead of drawing a fresh codename, so
        // Chris's handoff → /clear → resume flow keeps the window's name.
        if let Ok(wt) = std::env::var("WT_SESSION")
            && !wt.is_empty()
            && let Some(prev) = reg
                .sessions
                .values_mut()
                .find(|e| e.wt_session == wt && e.session_id != session_id)
        {
            let previous = (!prev.session_id.is_empty()).then(|| prev.session_id.clone());
            let now = now_iso();
            // Same tab, same title: peers still address it, so a title in use stays in use (BO-27, V2).
            prev.used_by = if prev.in_use() { session_id.to_string() } else { String::new() };
            prev.session_id = session_id.to_string();
            prev.last_heartbeat = now.clone();
            // The row describes the session that holds it now. Without this a reclaimed title kept its
            // predecessor's folder: cougar read Documents/std-video-engine on 2026-09-23 while its new
            // session ran in the home folder.
            prev.cwd = cwd.to_string_lossy().to_string();
            prev.workspace = workspace_name(cwd);
            let t = prev.title.clone();
            record_holder(&t, session_id, &now);
            save(&reg)?;
            return Ok((t, previous));
        }
        // BASE_RELAY_AS pins the codename at launch (e.g. `cc work` wrapper);
        // otherwise fall back to the random wordlist pick. A pinned title was chosen by hand, so it is in use from the
        // start (BO-27, V2); a wordlist pick is not until a ping goes to or from it.
        let pinned = relay_as();
        let used_by = if pinned.is_some() { session_id.to_string() } else { String::new() };
        let name = pinned.unwrap_or_else(|| pick_name(session_id, &reg));
        // The pick may be a title another session held (a dead holder, or a launcher's BASE_RELAY_AS).
        let previous = reg.sessions.get(&name).map(|e| e.session_id.clone()).filter(|s| !s.is_empty());
        let now = now_iso();
        record_holder(&name, session_id, &now);
        reg.sessions.insert(
            name.clone(),
            SessionEntry {
                title: name.clone(),
                session_id: session_id.to_string(),
                cwd: cwd.to_string_lossy().to_string(),
                workspace: workspace_name(cwd),
                registered_at: now.clone(),
                last_heartbeat: now,
                auto: true,
                wt_session: std::env::var("WT_SESSION").unwrap_or_default(),
                used_by,
                ..Default::default()
            },
        );
        save(&reg)?;
        Ok((name, previous))
    })
}

/// The title a launcher pinned with `BASE_RELAY_AS`, when it set one.
fn relay_as() -> Option<String> {
    std::env::var("BASE_RELAY_AS").ok().filter(|t| !t.is_empty())
}

/// Pick a codename for a session: hash its id to a start index, then walk the
/// list until we find one that's free or held by a dead/self session. Falls back
/// to a suffixed name only if every word is taken by a live session.
fn pick_name(session_id: &str, reg: &SessionRegistry) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut h);
    let start = (h.finish() as usize) % WORDLIST.len();
    for k in 0..WORDLIST.len() {
        let cand = WORDLIST[(start + k) % WORDLIST.len()];
        match reg.sessions.get(cand) {
            None => return cand.to_string(),
            Some(e) if e.session_id == session_id => return cand.to_string(),
            // A stale heartbeat alone does not free a title: heartbeats only
            // refresh at boundary hooks, so a long autonomous turn looks dead
            // for many minutes while its wake monitor is provably alive. The
            // sentinel is the stronger liveness signal — a spawned tab stole
            // "heron" from a mid-build session exactly this way (2026-08-17).
            Some(e) if !e.alive() && !super::wake::is_watching(cand) => {
                return cand.to_string()
            }
            _ => {}
        }
    }
    let suffix: String = session_id.chars().filter(|c| c.is_alphanumeric()).take(3).collect();
    format!("{}-{}", WORDLIST[start], suffix)
}

/// Drop registry entries whose sessions have been dead longer than 24h, so the
/// registry stays bounded no matter how many sessions come and go. Never prunes
/// the caller's own session.
fn prune_dead(reg: &mut SessionRegistry, keep: &str) {
    const CUTOFF_SECS: i64 = 24 * 3600;
    reg.sessions.retain(|_, e| {
        e.session_id == keep
            || parse_ts(&e.last_heartbeat)
                .map(|t| (chrono::Local::now() - t).num_seconds() < CUTOFF_SECS)
                .unwrap_or(false)
    });
}

fn workspace_name(cwd: &Path) -> String {
    crate::config::find_workspace_base(cwd)
        .and_then(|b| b.parent().map(PathBuf::from))
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default()
}

fn save(reg: &SessionRegistry) -> Result<()> {
    let path = registry_path()
        .ok_or_else(|| anyhow::anyhow!("no home directory — cannot resolve ~/.base-gbl"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_json_atomic(&path, reg)?;
    seed_history(reg);
    Ok(())
}

/// Once, at the first registry write after the install: give each title the history has no line for a seed line, its
/// holder from now. The history records only changes of holder, so a title one session has held since before this
/// build would never get a line, and nothing sent to or by it could be placed by time. Every title created later gets
/// its line from [`register`] or [`auto_register`]. A marker file makes it once, so a heartbeat never reads the history
/// (BO-05, F12e). Called under the registry lock, from [`save`].
fn seed_history(reg: &SessionRegistry) {
    let Some(marker) = global_base_dir().map(|d| d.join("title-history.seeded")) else { return };
    if marker.exists() {
        return;
    }
    let Some(path) = history_path() else { return };
    let known: std::collections::BTreeSet<String> = read_history(&path).into_iter().map(|h| h.title).collect();
    let now = now_iso();
    for e in reg.sessions.values() {
        if !e.session_id.is_empty() && !known.contains(&e.title) {
            let line = Holder { title: e.title.clone(), session_id: e.session_id.clone(), since: now.clone(), observed: true };
            append_holder(&line);
        }
    }
    let _ = std::fs::write(&marker, &now);
}

/// Lockfile mutex over the registry file. Mirrors [`RelayStore::with_lock`] —
/// short critical sections, stale locks (>30s) are broken.
fn with_lock<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let Some(dir) = global_base_dir() else {
        // No global tier — run unlocked (single-writer degenerate case).
        return f();
    };
    std::fs::create_dir_all(&dir)?;
    let lock = dir.join(".sessions.lock");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(_) => break,
            Err(_) => {
                if let Ok(meta) = std::fs::metadata(&lock)
                    && meta.modified().ok().and_then(|m| m.elapsed().ok())
                        .is_some_and(|e| e.as_secs() > 30)
                {
                    let _ = std::fs::remove_file(&lock);
                    continue;
                }
                if std::time::Instant::now() >= deadline {
                    anyhow::bail!("session registry locked for >10s: {}", lock.display());
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    let result = f();
    let _ = std::fs::remove_file(&lock);
    result
}
