pub mod board;
pub mod deliver;
pub mod session_registry;
pub mod task_inbox;
pub mod wake;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

// ─── Relay: session-to-session message bus ───────────────────
//
// Ephemeral per-project store for coordinating parallel Claude sessions —
// PAUL orchestrator/worker fan-outs and Cadre firm Members alike. The
// workspace graph stays clean: relay comms live in a disposable spool under
// `.base/relay/<project>/`, torn down at milestone end. Durable outcomes get
// PROMOTED to the real graph explicitly, never auto-leaked.
//
// Storage is maildir-style: one JSON file per message, written tmp+rename
// (atomic on POSIX), so concurrent senders never corrupt anything and no
// daemon is needed. Registry and claims are small JSON files guarded by a
// lockfile. NO direct concurrent .nq appends — export to inbox.nq is an
// explicit read-only snapshot.
//
// Identity model: sessions register a stable TITLE (worker-phase-11, quill).
// Binding to the live Claude session happens via CLAUDE_CODE_SESSION_ID
// (exported to Bash tool processes) or the BASE_RELAY_AS env override —
// Cadre Pulse contracts set BASE_RELAY_AS=<member> so scheduled one-shot
// member runs inherit their inbox with zero prompt surgery.

pub const MESSAGE_TYPES: &[&str] = &[
    "claim",
    "release",
    "notify",
    "unblock",
    "contract-change",
    "ready-to-merge",
    "question",
    "answer",
];

/// Heartbeats older than this render as DEAD on the board.
pub const DEAD_AFTER_SECS: i64 = 15 * 60;
/// Default advisory-claim TTL.
pub const DEFAULT_CLAIM_TTL_SECS: i64 = 60 * 60;

// ─── Data model ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub ts: String,
    pub from: String,
    /// Recipient: a title, a session id, "phase:<n>", or "all".
    pub to: String,
    #[serde(rename = "type")]
    pub mtype: String,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub worktree: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub registered_at: String,
    pub last_heartbeat: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub resource: String,
    pub by: String,
    #[serde(default)]
    pub note: String,
    pub ts: String,
    pub ttl_secs: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub sessions: BTreeMap<String, RegistryEntry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Claims {
    #[serde(default)]
    pub claims: BTreeMap<String, Claim>,
}

// ─── Store handle ────────────────────────────────────────────

pub struct RelayStore {
    pub root: PathBuf,
    pub project: String,
}

impl RelayStore {
    fn messages_dir(&self) -> PathBuf {
        self.root.join("messages")
    }
    fn seen_dir(&self) -> PathBuf {
        self.root.join("seen")
    }
    fn registry_path(&self) -> PathBuf {
        self.root.join("registry.json")
    }
    fn claims_path(&self) -> PathBuf {
        self.root.join("claims.json")
    }
    fn lock_path(&self) -> PathBuf {
        self.root.join(".lock")
    }

    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }

    /// Create the store skeleton (idempotent).
    pub fn init(&self) -> Result<()> {
        std::fs::create_dir_all(self.messages_dir())?;
        std::fs::create_dir_all(self.seen_dir())?;
        if !self.registry_path().exists() {
            write_json_atomic(&self.registry_path(), &Registry::default())?;
        }
        if !self.claims_path().exists() {
            write_json_atomic(&self.claims_path(), &Claims::default())?;
        }
        Ok(())
    }

    // ── Registry ────────────────────────────────────────────

    pub fn load_registry(&self) -> Registry {
        read_json(&self.registry_path()).unwrap_or_default()
    }

    /// Register (or re-bind) a titled session. Re-registering the same title
    /// is the normal path for scheduled members — each run binds its fresh
    /// session id to the stable title.
    pub fn register(
        &self,
        title: &str,
        session_id: Option<&str>,
        worktree: &str,
        phase: Option<&str>,
    ) -> Result<()> {
        self.with_lock(|| {
            let mut reg = self.load_registry();
            let now = now_iso();
            let entry = reg
                .sessions
                .entry(title.to_string())
                .or_insert_with(|| RegistryEntry {
                    title: title.to_string(),
                    registered_at: now.clone(),
                    ..Default::default()
                });
            entry.session_id = session_id.map(String::from).or(entry.session_id.take());
            entry.worktree = worktree.to_string();
            if phase.is_some() {
                entry.phase = phase.map(String::from);
            }
            entry.last_heartbeat = now;
            write_json_atomic(&self.registry_path(), &reg)
        })
    }

    pub fn heartbeat(&self, title: &str) {
        let _ = self.with_lock(|| {
            let mut reg = self.load_registry();
            if let Some(e) = reg.sessions.get_mut(title) {
                e.last_heartbeat = now_iso();
                write_json_atomic(&self.registry_path(), &reg)?;
            }
            Ok(())
        });
    }

    /// Resolve the current session's title: BASE_RELAY_AS env override first,
    /// then registry lookup by bound session id.
    pub fn identity(&self, session_id: Option<&str>) -> Option<String> {
        if let Ok(t) = std::env::var("BASE_RELAY_AS")
            && !t.is_empty()
        {
            return Some(t);
        }
        let sid = session_id?;
        self.load_registry()
            .sessions
            .values()
            .find(|e| e.session_id.as_deref() == Some(sid))
            .map(|e| e.title.clone())
    }

    // ── Messages ────────────────────────────────────────────

    /// Append a message — atomic file create, safe under concurrent senders.
    pub fn send(&self, from: &str, to: &str, mtype: &str, msg: &str, refs: &[String]) -> Result<Message> {
        if !MESSAGE_TYPES.contains(&mtype) {
            bail!(
                "Unknown message type '{mtype}'. Valid: {}",
                MESSAGE_TYPES.join(", ")
            );
        }
        let ts_ms = epoch_ms();
        let id = format!("{ts_ms}-{}-{}", sanitize(from), std::process::id());
        let message = Message {
            id: id.clone(),
            ts: now_iso(),
            from: from.to_string(),
            to: to.to_string(),
            mtype: mtype.to_string(),
            msg: msg.to_string(),
            refs: refs.to_vec(),
        };
        let path = self.messages_dir().join(format!("{id}.json"));
        // Uniqueness backstop: same sender + same pid + same millisecond.
        let path = if path.exists() {
            self.messages_dir().join(format!("{id}-{}.json", nanos_suffix()))
        } else {
            path
        };
        write_json_atomic(&path, &message)?;
        Ok(message)
    }

    /// All messages, chronological (filename-ordered: epoch-ms prefix).
    pub fn all_messages(&self) -> Vec<Message> {
        let Ok(entries) = std::fs::read_dir(self.messages_dir()) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        files.iter().filter_map(|p| read_json::<Message>(p)).collect()
    }

    /// Whether a message is addressed to a recipient identity.
    pub fn addressed_to(msg: &Message, title: &str, session_id: Option<&str>, phase: Option<&str>) -> bool {
        if msg.to == "all" || msg.to == title {
            return true;
        }
        if let Some(sid) = session_id
            && msg.to == sid
        {
            return true;
        }
        if let Some(ph) = phase
            && msg.to == format!("phase:{ph}")
        {
            return true;
        }
        false
    }

    /// Registry titles that should be woken for a message addressed to `to`.
    ///
    /// `relay send` writes into this workspace spool, but armed wake monitors
    /// watch the GLOBAL relay-inbox — two different directories, so a send
    /// could never fire a monitor (issue #9). The sender uses this to fan the
    /// recipient spec (title, session id, "phase:<n>", "all") out to concrete
    /// titles and drop a wake notification in each one's global inbox.
    pub fn wake_targets(&self, to: &str, sender: &str) -> Vec<String> {
        let probe = Message {
            id: String::new(),
            ts: String::new(),
            from: sender.to_string(),
            to: to.to_string(),
            mtype: String::new(),
            msg: String::new(),
            refs: Vec::new(),
        };
        let reg = self.load_registry();
        let mut titles: Vec<String> = reg
            .sessions
            .values()
            .filter(|e| e.title != sender)
            .filter(|e| Self::addressed_to(&probe, &e.title, e.session_id.as_deref(), e.phase.as_deref()))
            .map(|e| e.title.clone())
            .collect();
        // A direct title send can target a session registered globally but not
        // in this workspace store (cross-project --project sends). Let the
        // caller try the global registry for it.
        if titles.is_empty() && to != "all" && !to.starts_with("phase:") {
            titles.push(to.to_string());
        }
        titles
    }

    /// Unseen messages addressed to `title`. Does NOT mark seen.
    pub fn pending_for(&self, title: &str) -> Vec<Message> {
        let reg = self.load_registry();
        let entry = reg.sessions.get(title);
        let session_id = entry.and_then(|e| e.session_id.as_deref());
        let phase = entry.and_then(|e| e.phase.as_deref());
        let seen = self.load_seen(title);
        self.all_messages()
            .into_iter()
            .filter(|m| m.from != title)
            .filter(|m| Self::addressed_to(m, title, session_id, phase))
            .filter(|m| !seen.contains(&m.id))
            .collect()
    }

    pub fn mark_seen(&self, title: &str, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut seen = self.load_seen(title);
        seen.extend(ids.iter().cloned());
        write_json_atomic(&self.seen_dir().join(format!("{}.json", sanitize(title))), &seen)
    }

    fn load_seen(&self, title: &str) -> HashSet<String> {
        read_json(&self.seen_dir().join(format!("{}.json", sanitize(title)))).unwrap_or_default()
    }

    /// Block until a matching unseen message arrives (or timeout). Burns zero
    /// session tokens — the CLI process does the waiting, not the model.
    pub fn wait(
        &self,
        title: &str,
        from: Option<&str>,
        mtype: Option<&str>,
        timeout_secs: u64,
    ) -> Option<Message> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        loop {
            let hit = self
                .pending_for(title)
                .into_iter()
                .find(|m| {
                    from.is_none_or(|f| m.from == f) && mtype.is_none_or(|t| m.mtype == t)
                });
            if let Some(m) = hit {
                let _ = self.mark_seen(title, &[m.id.clone()]);
                return Some(m);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    // ── Claims (advisory — git worktrees are the safety layer) ──

    pub fn load_claims(&self) -> Claims {
        read_json(&self.claims_path()).unwrap_or_default()
    }

    /// Take an advisory claim. Fails (advisorily) if another session holds an
    /// unexpired claim on the resource.
    pub fn claim(&self, resource: &str, by: &str, note: &str, ttl_secs: i64) -> Result<()> {
        self.with_lock(|| {
            let mut claims = self.load_claims();
            if let Some(existing) = claims.claims.get(resource)
                && existing.by != by
                && !claim_expired(existing)
            {
                bail!(
                    "'{resource}' already claimed by {} ({} ago): {}",
                    existing.by,
                    age_str(&existing.ts),
                    existing.note
                );
            }
            claims.claims.insert(
                resource.to_string(),
                Claim {
                    resource: resource.to_string(),
                    by: by.to_string(),
                    note: note.to_string(),
                    ts: now_iso(),
                    ttl_secs,
                },
            );
            write_json_atomic(&self.claims_path(), &claims)
        })
    }

    pub fn release(&self, resource: &str, by: &str, force: bool) -> Result<()> {
        self.with_lock(|| {
            let mut claims = self.load_claims();
            match claims.claims.get(resource) {
                None => bail!("No claim on '{resource}'"),
                Some(c) if c.by != by && !force => {
                    bail!("'{resource}' is claimed by {} — use --force to override", c.by)
                }
                Some(_) => {
                    claims.claims.remove(resource);
                    write_json_atomic(&self.claims_path(), &claims)
                }
            }
        })
    }

    // ── Teardown ────────────────────────────────────────────

    pub fn dispose(&self) -> Result<()> {
        std::fs::remove_dir_all(&self.root)
            .with_context(|| format!("Failed to remove {}", self.root.display()))
    }

    /// Export the spool as an ephemeral named graph (inbox.nq) for
    /// inspection — read-only snapshot, never the live medium.
    pub fn export_nq(&self, ns_base: &str) -> Result<PathBuf> {
        let graph = format!("{ns_base}/relay/{}", sanitize(&self.project));
        let mut out = String::new();
        for m in self.all_messages() {
            let s = format!("{ns_base}/relay-msg/{}", m.id);
            let mut triple = |p: &str, o: &str| {
                out.push_str(&format!("<{s}> <{ns_base}/{p}> \"{}\" <{graph}> .\n", escape_nq(o)));
            };
            triple("from", &m.from);
            triple("to", &m.to);
            triple("messageType", &m.mtype);
            triple("text", &m.msg);
            triple("sentAt", &m.ts);
            for r in &m.refs {
                triple("references", r);
            }
        }
        let path = self.root.join("inbox.nq");
        std::fs::write(&path, out)?;
        Ok(path)
    }

    // ── Lock ────────────────────────────────────────────────

    /// Simple lockfile mutex for registry/claims mutations. Message sends
    /// don't need it (atomic file-per-message). Stale locks (>30s) are broken.
    fn with_lock<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let lock = self.lock_path();
        std::fs::create_dir_all(&self.root)?;
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
                        bail!("Relay store locked for >10s: {}", lock.display());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        let result = f();
        let _ = std::fs::remove_file(&lock);
        result
    }
}

// ─── Store resolution ────────────────────────────────────────

/// The relay root for a workspace: `<base>/relay/`. In a git worktree the
/// store lives in the MAIN repo's .base so all workers share one spool —
/// resolved via the `.git` file's gitdir pointer, pure-fs, no git exec.
pub fn relay_root(cwd: &Path) -> Option<PathBuf> {
    if let Ok(store) = std::env::var("BASE_RELAY_STORE")
        && !store.is_empty()
    {
        // Points directly at a project store; root is its parent.
        return PathBuf::from(store).parent().map(PathBuf::from);
    }
    if let Some(base) = crate::config::find_workspace_base(cwd) {
        let root = base.join("relay");
        if root.is_dir() {
            return Some(root);
        }
    }
    // Worktree fallback: .git FILE → gitdir: <main>/.git/worktrees/<name>
    let mut dir = cwd.to_path_buf();
    loop {
        let dotgit = dir.join(".git");
        if dotgit.is_file()
            && let Ok(content) = std::fs::read_to_string(&dotgit)
            && let Some(gitdir) = content.trim().strip_prefix("gitdir:")
        {
            let gitdir = PathBuf::from(gitdir.trim());
            // <main>/.git/worktrees/<name> → <main>
            if let Some(main_root) = gitdir.ancestors().find(|a| a.ends_with(".git")).and_then(|g| g.parent()) {
                let root = main_root.join(".base").join("relay");
                if root.is_dir() {
                    return Some(root);
                }
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Resolve a project store. With no --project and exactly one store, use it.
/// An explicit --project always beats the BASE_RELAY_STORE env default.
pub fn resolve_store(cwd: &Path, project: Option<&str>) -> Result<RelayStore> {
    if project.is_none()
        && let Ok(store) = std::env::var("BASE_RELAY_STORE")
        && !store.is_empty()
    {
        let root = PathBuf::from(&store);
        let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        return Ok(RelayStore { root, project: name });
    }
    let base = crate::config::find_workspace_base(cwd)
        .or_else(|| relay_root(cwd).and_then(|r| r.parent().map(PathBuf::from)))
        .ok_or_else(|| anyhow::anyhow!("No .base directory found — run from a base workspace"))?;
    let root = base.join("relay");

    match project {
        Some(p) => {
            let store = RelayStore { root: root.join(p), project: p.to_string() };
            // Fail LOUD on a missing store — silently waiting/polling against
            // nothing (wrong cwd, typo'd project) wastes a blocked session.
            if !store.exists() {
                let others = list_projects(&root);
                bail!(
                    "Relay store '{p}' not found under {}{}",
                    root.display(),
                    if others.is_empty() {
                        format!(" — create it: base relay init --project {p}")
                    } else {
                        format!(" — existing stores: {}", others.join(", "))
                    }
                );
            }
            Ok(store)
        }
        None => {
            let projects = list_projects(&root);
            match projects.len() {
                0 => bail!("No relay stores exist. Create one: base relay init --project <name>"),
                1 => {
                    let p = projects.into_iter().next().unwrap();
                    Ok(RelayStore { root: root.join(&p), project: p })
                }
                _ => bail!(
                    "Multiple relay stores ({}) — pass --project",
                    projects.join(", ")
                ),
            }
        }
    }
}

pub fn list_projects(relay_root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(relay_root) else {
        return Vec::new();
    };
    let mut projects: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    projects.sort();
    projects
}

/// The current session id, as exposed to Bash tool subprocesses.
pub fn env_session_id() -> Option<String> {
    std::env::var("CLAUDE_CODE_SESSION_ID").ok().filter(|s| !s.is_empty())
}

// ─── Helpers ─────────────────────────────────────────────────

pub fn now_iso() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z").to_string()
}

fn epoch_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn nanos_suffix() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect()
}

pub fn claim_expired(c: &Claim) -> bool {
    parse_ts(&c.ts).is_some_and(|t| (chrono::Local::now() - t).num_seconds() > c.ttl_secs)
}

pub fn parse_ts(ts: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%z")
        .ok()
        .map(|t| t.with_timezone(&chrono::Local))
}

pub fn age_str(ts: &str) -> String {
    let Some(t) = parse_ts(ts) else {
        return "?".into();
    };
    let secs = (chrono::Local::now() - t).num_seconds().max(0);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

fn escape_nq(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> RelayStore {
        let s = RelayStore {
            root: dir.join("relay").join("testproj"),
            project: "testproj".into(),
        };
        s.init().unwrap();
        s
    }

    #[test]
    fn init_register_send_poll_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());

        s.register("orchestrator", Some("sess-1"), "/main", None).unwrap();
        s.register("worker-p11", Some("sess-2"), "/wt/p11", Some("11")).unwrap();

        s.send("orchestrator", "worker-p11", "notify", "contracts frozen", &[]).unwrap();
        s.send("orchestrator", "all", "notify", "kickoff", &[]).unwrap();
        s.send("worker-p11", "orchestrator", "question", "schema Q", &[]).unwrap();

        // worker sees: direct + broadcast, NOT its own message.
        let pending = s.pending_for("worker-p11");
        assert_eq!(pending.len(), 2);
        assert!(pending.iter().any(|m| m.msg == "contracts frozen"));
        assert!(pending.iter().any(|m| m.msg == "kickoff"));

        // mark seen → gone from pending.
        let ids: Vec<String> = pending.iter().map(|m| m.id.clone()).collect();
        s.mark_seen("worker-p11", &ids).unwrap();
        assert!(s.pending_for("worker-p11").is_empty());

        // orchestrator sees only the worker's question — its own direct
        // message and its own broadcast are filtered (from == title).
        let orch = s.pending_for("orchestrator");
        assert_eq!(orch.len(), 1);
        assert_eq!(orch[0].mtype, "question");
    }

    #[test]
    fn own_messages_never_delivered_back() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.register("solo", Some("sess-1"), "/main", None).unwrap();
        s.send("solo", "all", "notify", "hello everyone", &[]).unwrap();
        assert!(s.pending_for("solo").is_empty());
    }

    #[test]
    fn wake_targets_fan_out_matches_addressing() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.register("orchestrator", Some("sess-1"), "/main", None).unwrap();
        s.register("worker-p11", Some("sess-2"), "/wt/p11", Some("11")).unwrap();
        s.register("worker-p12", Some("sess-3"), "/wt/p12", Some("12")).unwrap();

        // Direct title → that title only.
        assert_eq!(s.wake_targets("worker-p11", "orchestrator"), vec!["worker-p11"]);
        // Session id → the title bound to it.
        assert_eq!(s.wake_targets("sess-3", "orchestrator"), vec!["worker-p12"]);
        // Phase → the member in that phase.
        assert_eq!(s.wake_targets("phase:11", "orchestrator"), vec!["worker-p11"]);
        // Broadcast → everyone except the sender.
        let all = s.wake_targets("all", "orchestrator");
        assert_eq!(all.len(), 2);
        assert!(!all.contains(&"orchestrator".to_string()));
        // Unknown direct title falls through for a global-registry lookup;
        // unknown phase and broadcast never do.
        assert_eq!(s.wake_targets("elsewhere", "orchestrator"), vec!["elsewhere"]);
        assert!(s.wake_targets("phase:99", "orchestrator").is_empty());
    }

    #[test]
    fn phase_addressing_routes_to_phase_member() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.register("worker-a", None, "/wt/a", Some("11")).unwrap();
        s.register("worker-b", None, "/wt/b", Some("12")).unwrap();
        s.send("orch", "phase:11", "unblock", "go", &[]).unwrap();

        assert_eq!(s.pending_for("worker-a").len(), 1);
        assert!(s.pending_for("worker-b").is_empty());
    }

    #[test]
    fn invalid_message_type_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        assert!(s.send("a", "b", "gossip", "x", &[]).is_err());
    }

    #[test]
    fn concurrent_sends_lose_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        let root = s.root.clone();

        let handles: Vec<_> = (0..3)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let s = RelayStore { root, project: "testproj".into() };
                    for n in 0..50 {
                        s.send(&format!("sender-{i}"), "all", "notify", &format!("m{n}"), &[])
                            .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(s.all_messages().len(), 150);
    }

    #[test]
    fn wait_unblocks_on_matching_message() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.register("waiter", None, "/main", None).unwrap();
        let root = s.root.clone();

        let sender = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let s = RelayStore { root, project: "testproj".into() };
            s.send("other", "waiter", "answer", "42", &[]).unwrap();
        });

        let got = s.wait("waiter", Some("other"), Some("answer"), 10);
        sender.join().unwrap();
        let m = got.expect("wait should unblock");
        assert_eq!(m.msg, "42");
        // consumed by wait — not pending anymore.
        assert!(s.pending_for("waiter").is_empty());
    }

    #[test]
    fn wait_times_out_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.register("waiter", None, "/main", None).unwrap();
        // 0-second timeout: single check, no match → None.
        assert!(s.wait("waiter", None, Some("answer"), 0).is_none());
    }

    #[test]
    fn claims_conflict_and_ttl_expiry() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());

        s.claim("src/models/user.rs", "worker-a", "editing", 3600).unwrap();
        // Conflict: other holder, unexpired.
        assert!(s.claim("src/models/user.rs", "worker-b", "want it", 3600).is_err());
        // Same holder re-claims fine.
        assert!(s.claim("src/models/user.rs", "worker-a", "still editing", 3600).is_ok());

        // Expired claim is claimable by another.
        s.claim("phase:12", "worker-a", "quick", 0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(s.claim("phase:12", "worker-b", "taking over", 3600).is_ok());

        // Release: wrong holder without force fails, force works.
        assert!(s.release("src/models/user.rs", "worker-b", false).is_err());
        assert!(s.release("src/models/user.rs", "worker-b", true).is_ok());
    }

    #[test]
    fn dispose_removes_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.send("a", "all", "notify", "x", &[]).unwrap();
        assert!(s.exists());
        s.dispose().unwrap();
        assert!(!s.exists());
    }

    #[test]
    fn export_nq_snapshots_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.send("a", "b", "notify", "line1\nline2 \"quoted\"", &[String::from("src/x.rs")]).unwrap();
        let path = s.export_nq("https://ops.example/ns").unwrap();
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("line1\\nline2 \\\"quoted\\\""));
        assert!(content.contains("references"));
    }

    #[test]
    fn worktree_gitdir_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        // main repo: <tmp>/main with .git dir + .base/relay
        let main = tmp.path().join("main");
        std::fs::create_dir_all(main.join(".git").join("worktrees").join("wt1")).unwrap();
        std::fs::create_dir_all(main.join(".base").join("relay").join("proj")).unwrap();
        // worktree: <tmp>/wt1 with .git FILE pointing into main
        let wt = tmp.path().join("wt1");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}", main.join(".git").join("worktrees").join("wt1").display()),
        )
        .unwrap();

        let root = relay_root(&wt).expect("worktree should resolve main relay root");
        assert_eq!(root, main.join(".base").join("relay"));
    }
}

/// Clear the two shell variables that contaminate relay tests — once for the
/// whole test process, and never restored.
///
/// `BASE_RELAY_AS` pins a session's codename and `WT_SESSION` drives tab
/// continuity, so a suite run from a wrapper-launched shell (`cc work` exports
/// both) hands every fake session the same title and the uniqueness assertions
/// fail. Two modules used to scrub-and-restore these behind two *different*
/// mutexes, which do not serialize against each other: `deliver`'s guard
/// restored the shell's value while `task_inbox`'s tests were mid-run, putting
/// the contamination back. Nothing in the suite wants the shell's value back,
/// so the fix is to stop restoring it.
#[cfg(test)]
pub(crate) fn scrub_shell_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: runs exactly once, and no test sets these except under its
        // own lock, after this has already run.
        unsafe {
            std::env::remove_var("BASE_RELAY_AS");
            std::env::remove_var("WT_SESSION");
        }
    });
}
