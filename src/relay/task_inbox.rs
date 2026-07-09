//! Session-targeted task inbox — the delivery channel for `*task <title> …`.
//!
//! A task relayed to a live session lands as one JSON file under
//! `~/.base-gbl/.base/relay-inbox/<target-title>/<slug>.json`. The inbox is keyed
//! by the receiver's stable TITLE, not its session id — so a receiver that
//! restarts (fresh session id) still picks the task up the moment it reclaims
//! its title with `base relay register`. The target's hooks resolve their
//! session id → held titles, scan those dirs on every event (session-start,
//! prompt, tool-use, stop), and inject a loud "CONSUME AND EXECUTE" block the
//! first time a given session sees it, then a throttled reminder until done.
//!
//! Why a filesystem inbox and not the graph as the live medium: `base task
//! list` is workspace-scoped, so two sessions on adjacent projects can't see
//! each other's task graphs. The inbox is the shared, cross-workspace channel;
//! the hot delivery path is a cheap dir-stat, never a graph load. A durable
//! copy is ALSO mirrored into the global graph tier (best-effort) so the task
//! is visible in the graph — but the JSON is the source of truth for delivery.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{age_str, now_iso, parse_ts, read_json, write_json_atomic};
use crate::config::NamespaceConfig;

/// Terse reminders re-fire at most this often (per task) outside a fresh
/// session — keeps a long autonomous run nudged without spamming every tool call.
const TERSE_THROTTLE_SECS: i64 = 600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxTask {
    pub slug: String,
    /// One-line summary shown in the alert.
    pub summary: String,
    /// Absolute path to the full briefing doc the receiver should read.
    #[serde(default)]
    pub doc: String,
    /// Who relayed it (origin title or session id).
    #[serde(default)]
    pub from: String,
    /// Friendly title the task was addressed to.
    pub to_title: String,
    /// Resolved target session id (also the inbox subdir name).
    pub to_session: String,
    #[serde(default = "default_priority")]
    pub priority: String,
    pub created: String,
    /// pending → delivered → done. `pending` has never been shown loud.
    pub status: String,
    /// The session id that last received the LOUD alert (empty = never).
    #[serde(default)]
    pub last_loud_session: String,
    /// ISO timestamp of the last alert of any kind (loud or terse).
    #[serde(default)]
    pub last_alert_ts: String,
}

fn default_priority() -> String {
    "high".into()
}

/// Which hook is asking to deliver — controls loud vs terse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    SessionStart,
    Prompt,
    Tool,
    Stop,
}

impl Phase {
    /// Prompt boundaries (session-start, user prompt) may re-announce loudly on
    /// a new session; mid-turn phases (tool, stop) only ever nudge tersely.
    fn is_boundary(self) -> bool {
        matches!(self, Phase::SessionStart | Phase::Prompt)
    }
}

// ─── Paths ───────────────────────────────────────────────────

fn inbox_root() -> Option<PathBuf> {
    super::session_registry::global_base_dir().map(|d| d.join("relay-inbox"))
}

fn title_dir(title: &str) -> Option<PathBuf> {
    inbox_root().map(|r| r.join(sanitize(title)))
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect()
}

fn read_tasks_in(dir: &Path) -> Vec<(PathBuf, InboxTask)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, InboxTask)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| read_json::<InboxTask>(&p).map(|t| (p, t)))
        .collect();
    out.sort_by(|a, b| a.1.created.cmp(&b.1.created));
    out
}

// ─── Enqueue (origin side) ───────────────────────────────────

/// Drop a task into the target session's inbox. `ns` drives the best-effort
/// durable graph mirror. Returns the inbox file path written.
pub fn enqueue(ns: &NamespaceConfig, task: &InboxTask) -> Result<PathBuf> {
    let dir = title_dir(&task.to_title)
        .ok_or_else(|| anyhow::anyhow!("no home directory — cannot resolve ~/.base-gbl"))?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", sanitize(&task.slug)));
    write_json_atomic(&path, task)?;

    // Durable graph mirror — best-effort. A failure here must never sink the
    // relay: the JSON inbox is what actually drives delivery.
    if let Err(e) = mirror_to_graph(ns, task) {
        eprintln!("base relay task: graph mirror skipped: {e:#}");
    }
    Ok(path)
}

// ─── Delivery (target side, called from hooks) ───────────────

/// Scan this session's inbox and render the injection block, if any. Mutates
/// task state (marks delivered, stamps alert timestamps) as a side effect so
/// the loud alert fires once per session and terse reminders stay throttled.
pub fn deliver(session_id: &str, phase: Phase) -> Option<String> {
    // Which titles does this session hold? A never-registered session can't be
    // a relay target, so it does zero filesystem work beyond the registry read.
    let titles = super::session_registry::titles_for(session_id);
    if titles.is_empty() {
        return None;
    }
    let mut tasks: Vec<(PathBuf, InboxTask)> = Vec::new();
    for title in &titles {
        if let Some(dir) = title_dir(title) {
            tasks.extend(read_tasks_in(&dir));
        }
    }
    if tasks.is_empty() {
        return None;
    }

    let now = now_iso();
    let mut loud_blocks: Vec<String> = Vec::new();
    let mut terse_slugs: Vec<String> = Vec::new();

    for (path, mut task) in tasks {
        if task.status == "done" {
            continue;
        }
        let is_pending = task.status == "pending";
        let is_new_session = task.last_loud_session != session_id;
        // Loud on the very first sighting (any phase), or when a NEW session
        // hits a prompt boundary — that's the "re-announce at session start"
        // behavior. Otherwise it's a throttled terse nudge.
        let loud = is_pending || (phase.is_boundary() && is_new_session);

        if loud {
            loud_blocks.push(render_loud(&task));
            task.status = "delivered".into();
            task.last_loud_session = session_id.to_string();
            task.last_alert_ts = now.clone();
            let _ = write_json_atomic(&path, &task);
        } else if terse_due(&task.last_alert_ts, &now) {
            terse_slugs.push(task.slug.clone());
            task.last_alert_ts = now.clone();
            let _ = write_json_atomic(&path, &task);
        }
    }

    if loud_blocks.is_empty() && terse_slugs.is_empty() {
        return None;
    }

    let mut out = String::new();
    for b in &loud_blocks {
        out.push_str(b);
    }
    if !terse_slugs.is_empty() {
        out.push_str(&format!(
            "<relay-task-open>⏳ Relayed task(s) still assigned to this session and not yet done: {}. \
             Finish, then run `base relay done <slug>`.</relay-task-open>\n",
            terse_slugs.join(", ")
        ));
    }
    Some(out)
}

fn terse_due(last_alert_ts: &str, _now: &str) -> bool {
    match parse_ts(last_alert_ts) {
        Some(t) => (chrono::Local::now() - t).num_seconds() >= TERSE_THROTTLE_SECS,
        None => true, // never alerted (or unparseable) — due now
    }
}

fn render_loud(task: &InboxTask) -> String {
    let doc_line = if task.doc.is_empty() {
        String::new()
    } else {
        format!("Full brief: {}  ← read it in full before acting.\n", task.doc)
    };
    format!(
        "<relay-task-inbound to=\"{title}\">\n\
         🔔 NEW TASK TO IMMEDIATELY CONSUME AND EXECUTE — relayed from {from} to THIS session ({title}).\n\
         Task: {slug} [{pri}]\n\
         {summary}\n\
         {doc_line}\
         DIRECTIVE: Acknowledge in one line that you see it and are picking it up, then execute it now — this is operator-approved. \
         Ask a question ONLY if you are genuinely blocked and cannot proceed; the point is to keep the workflow rolling without pulling the operator back in. \
         When finished, run: base relay done {slug}\n\
         </relay-task-inbound>\n",
        title = task.to_title,
        from = if task.from.is_empty() { "another session" } else { &task.from },
        slug = task.slug,
        pri = task.priority,
        summary = task.summary,
        doc_line = doc_line,
    )
}

// ─── Completion + listing ────────────────────────────────────

/// Mark a relayed task done: remove every matching inbox entry (across all
/// session dirs — the receiver may not know which subdir it landed in) and flip
/// the durable graph mirror to completed. Returns how many inbox files cleared.
pub fn done(ns: &NamespaceConfig, slug: &str) -> Result<usize> {
    let target = format!("{}.json", sanitize(slug));
    let mut cleared = 0;
    if let Some(root) = inbox_root()
        && let Ok(sessions) = std::fs::read_dir(&root)
    {
        for s in sessions.filter_map(|e| e.ok()) {
            let f = s.path().join(&target);
            if f.is_file() && std::fs::remove_file(&f).is_ok() {
                cleared += 1;
            }
        }
    }
    if let Err(e) = complete_in_graph(ns, slug) {
        eprintln!("base relay done: graph mirror update skipped: {e:#}");
    }
    Ok(cleared)
}

/// Every inbound relay task across all sessions (for `base relay tasks`).
pub fn list_all() -> Vec<InboxTask> {
    let Some(root) = inbox_root() else {
        return Vec::new();
    };
    let Ok(sessions) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for s in sessions.filter_map(|e| e.ok()) {
        if s.path().is_dir() {
            out.extend(read_tasks_in(&s.path()).into_iter().map(|(_, t)| t));
        }
    }
    out.sort_by(|a, b| a.created.cmp(&b.created));
    out
}

/// Human-readable one-liner for a task (used by `base relay tasks`).
pub fn format_row(t: &InboxTask) -> String {
    format!(
        "  {slug} → {title} [{status}, {pri}] · {age} · from {from}",
        slug = t.slug,
        title = t.to_title,
        status = t.status,
        pri = t.priority,
        age = age_str(&t.created),
        from = if t.from.is_empty() { "?" } else { &t.from },
    )
}

// ─── Graph mirror (global tier, best-effort) ─────────────────

fn global_cwd() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".base-gbl"))
}

fn mirror_to_graph(ns: &NamespaceConfig, task: &InboxTask) -> Result<()> {
    let cwd = global_cwd().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let p = &ns.prefix;
    let iri = crate::crud::build_iri(ns, "task", &task.slug);
    let ws_slug = crate::crud::workspace_slug(&cwd);
    let graph = crate::crud::workspace_graph_iri(ns, &ws_slug);
    let now = crate::crud::now_iso();
    let name = crate::crud::escape_sparql_literal(&task.summary);
    let doc = crate::crud::escape_sparql_literal(&task.doc);
    let from = crate::crud::escape_sparql_literal(&task.from);
    let to = crate::crud::escape_sparql_literal(&task.to_title);
    let sid = crate::crud::escape_sparql_literal(&task.to_session);
    let pri = crate::crud::escape_sparql_literal(&task.priority);

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> rdf:type {p}:Task ;\n\
               {p}:name \"{name}\" ;\n\
               {p}:status \"active\" ;\n\
               {p}:priority \"{pri}\" ;\n\
               {p}:relayInbound \"true\" ;\n\
               {p}:assignedTo \"{to}\" ;\n\
               {p}:assignedSession \"{sid}\" ;\n\
               {p}:relayFrom \"{from}\" ;\n\
               {p}:brief \"{doc}\" ;\n\
               {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
               {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
           }}\n\
         }}"
    );
    crate::crud::load_and_mutate(&cwd, ns, &sparql)
}

fn complete_in_graph(ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let cwd = global_cwd().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let p = &ns.prefix;
    let iri = crate::crud::build_iri(ns, "task", slug);
    let ws_slug = crate::crud::workspace_slug(&cwd);
    let graph = crate::crud::workspace_graph_iri(ns, &ws_slug);
    let now = crate::crud::now_iso();
    let updates = [
        crate::crud::field_update(&graph, &iri, &format!("{p}:status"), "\"completed\""),
        crate::crud::field_update(
            &graph,
            &iri,
            &format!("{p}:lastActive"),
            &format!("\"{now}\"^^xsd:dateTime"),
        ),
    ];
    crate::crud::load_and_mutate(&cwd, ns, &updates.join(" ;\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NamespaceConfig;
    use std::path::Path;

    // HOME is process-global and cargo test is multi-threaded — every test that
    // repoints ~/.base-gbl serializes through this.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _g = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("HOME");
        // SAFETY: guarded by ENV_LOCK — no other test reads HOME concurrently.
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let out = f(tmp.path());
        unsafe {
            match prev {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
        }
        out
    }

    fn sample(session: &str) -> InboxTask {
        InboxTask {
            slug: "rebuild-auth-guard".into(),
            summary: "Rebuild the auth guard".into(),
            doc: "/tmp/rebuild-auth-guard.md".into(),
            from: "api-session".into(),
            to_title: "caddy-backend".into(),
            to_session: session.into(),
            priority: "high".into(),
            created: crate::relay::now_iso(),
            status: "pending".into(),
            last_loud_session: String::new(),
            last_alert_ts: String::new(),
        }
    }

    /// Bind the receiver title to a session id in the global registry so
    /// deliver()'s session→titles resolution finds the inbox.
    fn bind(title: &str, session: &str, home: &Path) {
        crate::relay::session_registry::register(title, session, home).unwrap();
    }

    #[test]
    fn unregistered_session_gets_nothing() {
        with_home(|_| {
            let ns = NamespaceConfig::default();
            enqueue(&ns, &sample("sid-B")).unwrap();
            // sid-B never claimed the "caddy-backend" title → not a target.
            assert!(deliver("sid-B", Phase::SessionStart).is_none());
        });
    }

    #[test]
    fn pending_fires_loud_on_any_phase() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            enqueue(&ns, &sample("sid-B")).unwrap();
            // Mid-turn (Tool) still fires loud when the task has never been seen.
            let block = deliver("sid-B", Phase::Tool).expect("pending task must deliver");
            assert!(block.contains("NEW TASK TO IMMEDIATELY CONSUME AND EXECUTE"));
            assert!(block.contains("rebuild-auth-guard"));
            assert!(block.contains("base relay done rebuild-auth-guard"));
        });
    }

    #[test]
    fn loud_once_then_quiet_within_same_session() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            enqueue(&ns, &sample("sid-B")).unwrap();
            assert!(deliver("sid-B", Phase::SessionStart).is_some(), "first sighting loud");
            // Same session, right after: no re-loud, terse throttled → silent.
            assert!(deliver("sid-B", Phase::Prompt).is_none(), "must not re-fire same session");
            assert!(deliver("sid-B", Phase::Tool).is_none(), "must not nudge every tool call");
        });
    }

    #[test]
    fn new_session_re_announces_loud() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            enqueue(&ns, &sample("sid-B")).unwrap();
            assert!(deliver("sid-B", Phase::SessionStart).is_some());
            // Receiver restarts and reclaims the SAME title under a new session id.
            // The one inbox entry re-fires loud for the new session — the core of
            // "persist until done, re-loud at session-start".
            bind("caddy-backend", "sid-C", home);
            let block = deliver("sid-C", Phase::SessionStart).expect("new session must see it");
            assert!(block.contains("NEW TASK TO IMMEDIATELY CONSUME"));
        });
    }

    #[test]
    fn done_clears_the_inbox() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            enqueue(&ns, &sample("sid-B")).unwrap();
            assert_eq!(list_all().len(), 1);
            let cleared = done(&ns, "rebuild-auth-guard").unwrap();
            assert_eq!(cleared, 1);
            assert!(list_all().is_empty());
            assert!(deliver("sid-B", Phase::SessionStart).is_none(), "no alert after done");
        });
    }

    #[test]
    fn touch_auto_names_stable_and_unique() {
        with_home(|home| {
            let n1 = crate::relay::session_registry::touch("sess-1", home).unwrap();
            assert!(!n1.is_empty());
            // Same session keeps its name (idempotent).
            assert_eq!(crate::relay::session_registry::touch("sess-1", home).unwrap(), n1);
            // A second live session gets a different codename.
            let n2 = crate::relay::session_registry::touch("sess-2", home).unwrap();
            assert_ne!(n1, n2);
        });
    }

    #[test]
    fn no_autoname_env_opts_out() {
        with_home(|home| {
            // SAFETY: guarded by ENV_LOCK via with_home.
            unsafe { std::env::set_var("BASE_NO_AUTONAME", "1") };
            let r = crate::relay::session_registry::touch("sess-x", home);
            unsafe { std::env::remove_var("BASE_NO_AUTONAME") };
            assert!(r.is_none());
        });
    }

    #[test]
    fn slug_sanitized_matches_between_enqueue_and_done() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            let mut t = sample("sid-B");
            t.slug = "Rebuild Auth Guard".into(); // raw; done() matches via filename sanitization
            enqueue(&ns, &t).unwrap();
            assert_eq!(done(&ns, "Rebuild Auth Guard").unwrap(), 1);
        });
    }
}
