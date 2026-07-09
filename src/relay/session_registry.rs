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

use super::{now_iso, parse_ts, write_json_atomic, read_json, DEAD_AFTER_SECS};

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
}

impl SessionEntry {
    pub fn alive(&self) -> bool {
        parse_ts(&self.last_heartbeat)
            .map(|t| (chrono::Local::now() - t).num_seconds() < DEAD_AFTER_SECS)
            .unwrap_or(false)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionRegistry {
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionEntry>,
}

/// `~/.base-gbl/.base/` — the global tier base dir. `None` only if there is no home dir.
pub fn global_base_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".base-gbl").join(".base"))
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
pub fn register(title: &str, session_id: &str, cwd: &Path) -> Result<()> {
    with_lock(|| {
        let mut reg = load();
        let now = now_iso();
        let workspace = workspace_name(cwd);
        let entry = reg.sessions.entry(title.to_string()).or_insert_with(|| SessionEntry {
            title: title.to_string(),
            registered_at: now.clone(),
            ..Default::default()
        });
        entry.session_id = session_id.to_string();
        entry.cwd = cwd.to_string_lossy().to_string();
        entry.workspace = workspace;
        entry.last_heartbeat = now;
        save(&reg)
    })
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

pub fn list() -> Vec<SessionEntry> {
    load().sessions.into_values().collect()
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
    if let Some(t) = titles_for(session_id).into_iter().next() {
        if should_heartbeat(session_id) {
            heartbeat(session_id);
        }
        return Some(t);
    }
    if std::env::var_os("BASE_NO_AUTONAME").is_some() {
        return None;
    }
    auto_register(session_id, cwd).ok()
}

fn auto_register(session_id: &str, cwd: &Path) -> Result<String> {
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
            return Ok(t);
        }
        let name = pick_name(session_id, &reg);
        let now = now_iso();
        reg.sessions.insert(
            name.clone(),
            SessionEntry {
                title: name.clone(),
                session_id: session_id.to_string(),
                cwd: cwd.to_string_lossy().to_string(),
                workspace: workspace_name(cwd),
                registered_at: now.clone(),
                last_heartbeat: now,
            },
        );
        save(&reg)?;
        Ok(name)
    })
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
            Some(e) if e.session_id == session_id || !e.alive() => return cand.to_string(),
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
    write_json_atomic(&path, reg)
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
