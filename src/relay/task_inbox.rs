//! Session-targeted task inbox — the delivery channel for `*task <title> …`
//! and `*ping <title> …`.
//!
//! A task relayed to a live session lands as one JSON file under
//! `~/.base-gbl/.base/relay-inbox/<target-title>/<slug>.json`. The inbox is keyed
//! by the receiver's stable TITLE, not its session id — so a receiver that
//! restarts (fresh session id) still picks the task up the moment it reclaims
//! its title with `base relay register`. The target's hooks resolve their
//! session id → held titles and scan those dirs at session start and on each
//! prompt, never on a tool call (BO-04, F13b). A new item is shown once, in full,
//! as information; while a ping stays unanswered, session start lists it in one line.
//!
//! Pings ride the same rail with IM semantics instead of work semantics: no
//! briefing doc (the message IS the payload, mirrored to the graph), and the
//! obligation is a REPLY, not completion. The receiver's ping back to the sender
//! clears it. The reply (`kind == "reply"`) is announced once and consumed on
//! delivery, so an ack never demands its own ack.
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

/// A ping older than this never alerts again, loud or terse. A ping is a live message between sessions;
/// one that has waited a day is history, and it stays in the inbox file for anyone who reads it.
/// Measured 2026-09-23: a ping sent 2026-08-29 to another session, already delivered, fired as
/// "REPLY REQUIRED BEFORE YOUR NEXT ACTION" on every prompt of an unrelated session that took the title
/// by tab reclaim. Tasks are not covered: a task is assigned work and persists until `relay done`.
const PING_STALE_SECS: i64 = 24 * 60 * 60;

/// True for a ping whose `created` stamp is older than [`PING_STALE_SECS`]. An unparseable stamp is not
/// stale, so a malformed file keeps the old behaviour rather than going quiet.
fn ping_is_stale(task: &InboxTask) -> bool {
    task.kind == "ping"
        && parse_ts(&task.created)
            .is_some_and(|t| (chrono::Local::now() - t).num_seconds() > PING_STALE_SECS)
}

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
    /// "task" (briefed work, cleared by `base relay done`), "ping" (instant
    /// message, cleared by the receiver's reply), or "reply" (announced once,
    /// consumed on delivery — no further obligation).
    #[serde(default = "default_kind")]
    pub kind: String,
    /// File paths / entity ids the message references (pings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    /// A notify's spool: the store root and the message id `relay send` wrote there. Showing the notify marks that
    /// message seen in that store, so one message is never shown twice (BO-04, F13c). Empty for every other kind.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spool_store: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spool_id: String,
}

fn default_priority() -> String {
    "high".into()
}

fn default_kind() -> String {
    "task".into()
}

/// Which hook is asking to deliver. Relay content appears at session start and on a prompt only, never on a tool call
/// or at a turn's end (BO-04, F13b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    SessionStart,
    Prompt,
}

// ─── Paths ───────────────────────────────────────────────────

fn inbox_root() -> Option<PathBuf> {
    super::session_registry::global_base_dir().map(|d| d.join("relay-inbox"))
}

pub(crate) fn title_dir(title: &str) -> Option<PathBuf> {
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
    out.sort_by(|a, b| (&a.1.created, &a.1.slug).cmp(&(&b.1.created, &b.1.slug)));
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
/// task state (marks delivered, deletes an announced reply or notify) as a side effect.
pub fn deliver(session_id: &str, phase: Phase) -> Option<String> {
    let (block, commits) = deliver_deferred(session_id, phase)?;
    super::run_commits(commits);
    Some(block)
}

/// The slug of the wake notify `relay send` drops for one spool message and one recipient title.
pub fn notify_slug(message_id: &str, title: &str) -> String {
    format!("notify-{message_id}-{title}")
}

/// Whether `title`'s inbox still holds the wake notify for spool message `message_id`. The spool delivery leaves such a
/// message to the notify, so one message is never shown twice on one prompt (BO-04, F13c).
pub fn has_notify(title: &str, message_id: &str) -> bool {
    title_dir(title).is_some_and(|d| d.join(format!("{}.json", sanitize(&notify_slug(message_id, title)))).is_file())
}

/// True for the kinds that are messages between sessions rather than assigned work. One sender's newer message hides
/// its older unshown ones; a task is never hidden.
fn is_chat(task: &InboxTask) -> bool {
    matches!(task.kind.as_str(), "ping" | "reply" | "notify")
}

/// [`deliver`] with the inbox NOT yet changed: recording a task or ping as delivered, hiding a superseded message and
/// deleting an announced reply or notify come back as commits, for the prompt hook to run only if it prints the block
/// (BO-01). Until they run, the inbox reads as it did, so a dropped block is announced at the next prompt.
///
/// BO-04 (F13), WHAT EACH EVENT SHOWS:
/// - A message no session has been shown (`pending`) is shown once, in full, on the first prompt or session start
///   after it arrives. It is shown as information: who, when, the message, and the reply command. Nothing in it claims
///   to come before the user's own prompt.
/// - Of one sender's unshown messages to one title, only the newest is shown; the older ones are marked `superseded`,
///   kept on disk, and the shown one carries a line naming the command that lists them.
/// - A ping already shown, while unanswered, is listed in one line at session start and is never repeated on a prompt.
/// - A task already shown to another session is shown in full at this session's start (it is work this session now
///   holds); one already shown to this session is listed in one line.
///
/// What this replaced: a loud block on every first sighting in ANY hook, pre-tool included, headed "REPLY REQUIRED
/// BEFORE YOUR NEXT ACTION", then a terse "Reply RIGHT NOW" nag every three minutes on prompts and tool calls until the
/// receiver answered.
pub fn deliver_deferred(session_id: &str, phase: Phase) -> Option<(String, Vec<super::Commit>)> {
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
    tasks.retain(|(_, t)| t.status != "done" && !ping_is_stale(t));
    if tasks.is_empty() {
        return None;
    }

    // Superseded: per (title, sender), every unshown chat message but the newest. `read_tasks_in` sorts oldest first.
    let mut newest: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
    for (i, (_, t)) in tasks.iter().enumerate() {
        if t.status == "pending" && is_chat(t) {
            newest.insert((t.to_title.clone(), t.from.clone()), i);
        }
    }
    let mut hidden: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();

    let now = now_iso();
    let mut shown: Vec<(usize, String)> = Vec::new();
    let mut open_pings: Vec<String> = Vec::new();
    let mut open_tasks: Vec<String> = Vec::new();
    let mut commits: Vec<super::Commit> = Vec::new();

    for (i, (path, task)) in tasks.iter().enumerate() {
        let mut task = task.clone();
        let path = path.clone();
        if task.status == "pending" {
            if is_chat(&task) {
                let keep = newest[&(task.to_title.clone(), task.from.clone())];
                if keep != i {
                    *hidden.entry(keep).or_default() += 1;
                    commits.push(hide(path, task, session_id, &now));
                    continue;
                }
            }
            shown.push((i, render(&task)));
            commits.push(consume(path, task, session_id, &now));
            continue;
        }
        if phase != Phase::SessionStart {
            continue;
        }
        match (task.kind.as_str(), task.status.as_str()) {
            ("task", _) if task.last_loud_session != session_id => {
                shown.push((i, render(&task)));
                task.last_loud_session = session_id.to_string();
                task.last_alert_ts = now.clone();
                commits.push(write_back(path, task));
            }
            ("task", _) => open_tasks.push(format!("{} from {}", task.slug, sender(&task))),
            ("ping", "delivered") => open_pings.push(format!("{} {}", sender(&task), super::clock(&task.created))),
            _ => {}
        }
    }

    if shown.is_empty() && open_pings.is_empty() && open_tasks.is_empty() {
        return None;
    }
    let mut out = String::new();
    for (i, text) in &shown {
        out.push_str(text);
        if let Some(n) = hidden.get(i) {
            let from = sender(&tasks[*i].1);
            out.push_str(&format!(
                "({n} earlier message{s} from {from} hidden, this one is newer: base relay tasks --from {from})\n",
                s = if *n == 1 { "" } else { "s" },
            ));
        }
    }
    if !open_pings.is_empty() {
        out.push_str(&format!(
            "relay: {} unanswered ping{} ({}) · read: base relay tasks --from <sender> · reply: base relay ping --to <sender> --msg \"...\"\n",
            open_pings.len(),
            if open_pings.len() == 1 { "" } else { "s" },
            open_pings.join(", ")
        ));
    }
    if !open_tasks.is_empty() {
        out.push_str(&format!(
            "relay: {} open task{} ({}) · when finished: base relay done <slug>\n",
            open_tasks.len(),
            if open_tasks.len() == 1 { "" } else { "s" },
            open_tasks.join(", ")
        ));
    }
    Some((out, commits))
}

fn write_back(path: PathBuf, task: InboxTask) -> super::Commit {
    Box::new(move || {
        let _ = write_json_atomic(&path, &task);
    })
}

/// Mark a shown message as seen in the spool it came from, for a notify that carries one.
fn mark_spool_seen(task: &InboxTask) {
    if task.spool_store.is_empty() || task.spool_id.is_empty() {
        return;
    }
    let root = PathBuf::from(&task.spool_store);
    let project = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let store = super::RelayStore { root, project };
    if store.exists() {
        let _ = store.mark_seen(&task.to_title, std::slice::from_ref(&task.spool_id));
    }
}

/// What showing a message does to the inbox: a reply or notify is gone (a notify's spool copy is marked seen, so the
/// spool never shows it again); a ping or task is recorded as delivered to this session.
fn consume(path: PathBuf, mut task: InboxTask, session_id: &str, now: &str) -> super::Commit {
    if task.kind == "reply" || task.kind == "notify" {
        return Box::new(move || {
            mark_spool_seen(&task);
            let _ = std::fs::remove_file(&path);
        });
    }
    task.status = "delivered".into();
    task.last_loud_session = session_id.to_string();
    task.last_alert_ts = now.to_string();
    write_back(path, task)
}

/// What hiding a superseded message does: the file stays, readable by `base relay tasks --from <sender>`, and is never
/// shown by a hook.
fn hide(path: PathBuf, mut task: InboxTask, session_id: &str, now: &str) -> super::Commit {
    task.status = "superseded".into();
    task.last_loud_session = session_id.to_string();
    task.last_alert_ts = now.to_string();
    Box::new(move || {
        mark_spool_seen(&task);
        let _ = write_json_atomic(&path, &task);
    })
}

fn sender(task: &InboxTask) -> &str {
    if task.from.is_empty() { "another session" } else { &task.from }
}

fn refs_line(task: &InboxTask) -> String {
    if task.refs.is_empty() {
        String::new()
    } else {
        format!("refs: {}\n", task.refs.join(", "))
    }
}

/// One relay item as information: a header line (what, who, when, what to run), then the message on its own line.
///
/// The message starts its own line, and that is load-bearing rather than cosmetic (#101): `hook/mod.rs` matches star
/// commands against this rendered block, and a command only activates in the leading star-run of a line. `cli.rs`
/// marks a send `kind = "reply"` whenever the target has an unanswered ping from the sender, so the operator ANSWERING
/// a session's question arrives here. Pinned by `every_renderer_puts_the_operator_message_on_its_own_line`.
///
/// No header claims priority over the user (BO-04, F13a): no "REPLY REQUIRED", no "DIRECTIVE", no "RIGHT NOW", no
/// urgency marks. Pinned by `relay_text_has_no_priority_claims`.
fn render(task: &InboxTask) -> String {
    let from = sender(task);
    let when = super::clock(&task.created);
    let header = match task.kind.as_str() {
        "ping" if task.from.is_empty() => format!(
            "relay: ping from {from} ({when}) · it has no relay title, so it cannot be answered; clear it: base relay done {}",
            task.slug
        ),
        "ping" => format!("relay: ping from {from} ({when}) · reply: base relay ping --to {from} --msg \"...\""),
        "reply" => format!("relay: reply from {from} ({when}) · no answer needed"),
        "notify" => match spool_parts(task) {
            // The wake notify for a spool message reads exactly as the spool delivery would show it: the store,
            // the message type, and for a question the command that answers it.
            Some((project, mtype, _)) => format!(
                "relay ({project}): {mtype} from {from} ({when}){answer}",
                answer = answer_command(&project, mtype, from),
            ),
            None => format!("relay: message from {from} ({when})"),
        },
        _ => format!(
            "relay: task {slug} [{pri}] from {from} ({when}) · when finished: base relay done {slug}",
            slug = task.slug,
            pri = task.priority,
        ),
    };
    let brief = if task.kind == "task" && !task.doc.is_empty() {
        format!("brief: {}\n", task.doc)
    } else {
        String::new()
    };
    let msg = match spool_parts(task) {
        Some((_, _, body)) if task.kind == "notify" => body,
        _ => task.summary.as_str(),
    };
    format!("{header}\n{msg}\n{brief}{refs}", refs = refs_line(task))
}

/// A spool notify's store name, message type and message body. `relay send` writes the notify's summary as
/// `[<type>] <message>`; a notify without a spool, or a summary in another shape, gives `None`.
fn spool_parts(task: &InboxTask) -> Option<(String, &str, &str)> {
    if task.spool_store.is_empty() {
        return None;
    }
    let project = std::path::Path::new(&task.spool_store).file_name()?.to_string_lossy().into_owned();
    let rest = task.summary.strip_prefix('[')?;
    let (mtype, body) = rest.split_once("] ")?;
    Some((project, mtype, body))
}

/// The command that answers a spool question, for the two message types that ask for one; empty for the others.
pub fn answer_command(project: &str, mtype: &str, from: &str) -> String {
    if matches!(mtype, "question" | "contract-change") {
        format!(" · answer: base relay send --project {project} --to {from} --type answer --msg \"...\"")
    } else {
        String::new()
    }
}

/// The receiver's reply IS the ack: clear every inbound ping FROM `peer` sitting in any of `my_titles`' inboxes,
/// flipping each graph mirror to answered (best-effort), and with them every message from `peer` hidden as superseded,
/// so a reply leaves nothing earlier from that sender to show (BO-04, F13c). Returns how many pings cleared.
pub fn clear_pings_from(ns: &NamespaceConfig, peer: &str, my_titles: &[String]) -> usize {
    let mut cleared = 0;
    for title in my_titles {
        let Some(dir) = title_dir(title) else { continue };
        for (path, task) in read_tasks_in(&dir) {
            if task.from != peer {
                continue;
            }
            if task.kind == "ping" && std::fs::remove_file(&path).is_ok() {
                cleared += 1;
                if let Err(e) = answer_in_graph(ns, &task.slug) {
                    eprintln!("base relay ping: graph mirror update skipped: {e:#}");
                }
            } else if task.status == "superseded" {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    cleared
}

// ─── Completion + listing ────────────────────────────────────

/// Mark a relayed task done: remove every matching inbox entry (across all
/// session dirs — the receiver may not know which subdir it landed in) and flip
/// the durable graph mirror to completed. Returns how many inbox files cleared.
/// Also the escape hatch for pings whose sender is untitled (can't be replied
/// to) — their mirror flips to answered instead of completed.
pub fn done(ns: &NamespaceConfig, slug: &str) -> Result<usize> {
    let target = format!("{}.json", sanitize(slug));
    let mut cleared = 0;
    let mut was_ping = false;
    if let Some(root) = inbox_root()
        && let Ok(sessions) = std::fs::read_dir(&root)
    {
        for s in sessions.filter_map(|e| e.ok()) {
            let f = s.path().join(&target);
            if !f.is_file() {
                continue;
            }
            was_ping |= read_json::<InboxTask>(&f).is_some_and(|t| t.kind != "task");
            if std::fs::remove_file(&f).is_ok() {
                cleared += 1;
            }
        }
    }
    let mirror = if was_ping { answer_in_graph(ns, slug) } else { complete_in_graph(ns, slug) };
    if let Err(e) = mirror {
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

/// Human-readable row for a task (used by `base relay tasks`): the facts on one line, the message on the next, so a
/// superseded or already-shown message can be read again.
pub fn format_row(t: &InboxTask) -> String {
    let kind = if t.kind == "task" { String::new() } else { format!(" ({})", t.kind) };
    format!(
        "  {slug}{kind} → {title} [{status}, {pri}] · {age} · from {from}\n      {summary}",
        slug = t.slug,
        title = t.to_title,
        status = t.status,
        pri = t.priority,
        age = age_str(&t.created),
        from = if t.from.is_empty() { "?" } else { &t.from },
        summary = t.summary,
    )
}

// ─── Graph mirror (global tier, best-effort) ─────────────────

fn global_cwd() -> Option<PathBuf> {
    crate::home::home_root().map(|h| h.join(".base-gbl"))
}

fn mirror_to_graph(ns: &NamespaceConfig, task: &InboxTask) -> Result<()> {
    let cwd = global_cwd().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let p = &ns.prefix;
    let ws_slug = crate::crud::workspace_slug(&cwd);
    let graph = crate::crud::workspace_graph_iri(ns, &ws_slug);
    let now = crate::crud::now_iso();
    let name = crate::crud::escape_sparql_literal(&task.summary);
    let from = crate::crud::escape_sparql_literal(&task.from);
    let to = crate::crud::escape_sparql_literal(&task.to_title);
    let sid = crate::crud::escape_sparql_literal(&task.to_session);

    // Pings mirror as their own entity type — the message body IS the durable
    // record (no doc), and they must never pollute `base task list`.
    let sparql = if task.kind == "task" {
        let iri = crate::crud::build_iri(ns, "task", &task.slug);
        let doc = crate::crud::escape_sparql_literal(&task.doc);
        let pri = crate::crud::escape_sparql_literal(&task.priority);
        format!(
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
        )
    } else {
        let iri = crate::crud::build_iri(ns, "ping", &task.slug);
        // A reply's obligation is already met the moment it's sent.
        let status = if task.kind == "reply" { "answered" } else { "open" };
        let refs = task
            .refs
            .iter()
            .map(|r| format!(" ;\n               {p}:references \"{}\"", crate::crud::escape_sparql_literal(r)))
            .collect::<String>();
        format!(
            "INSERT DATA {{\n\
               GRAPH <{graph}> {{\n\
                 <{iri}> rdf:type {p}:Ping ;\n\
                   {p}:message \"{name}\" ;\n\
                   {p}:pingKind \"{kind}\" ;\n\
                   {p}:status \"{status}\" ;\n\
                   {p}:relayFrom \"{from}\" ;\n\
                   {p}:assignedTo \"{to}\" ;\n\
                   {p}:assignedSession \"{sid}\"{refs} ;\n\
                   {p}:createdAt \"{now}\"^^xsd:dateTime .\n\
               }}\n\
             }}",
            kind = task.kind,
        )
    };
    crate::crud::load_and_mutate(&cwd, ns, &sparql)
}

/// Flip a mirrored ping to answered (reply sent, or operator-cleared).
fn answer_in_graph(ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let cwd = global_cwd().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let p = &ns.prefix;
    let iri = crate::crud::build_iri(ns, "ping", slug);
    let ws_slug = crate::crud::workspace_slug(&cwd);
    let graph = crate::crud::workspace_graph_iri(ns, &ws_slug);
    let update = crate::crud::field_update(&graph, &iri, &format!("{p}:status"), "\"answered\"");
    crate::crud::load_and_mutate(&cwd, ns, &update)
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

    // Each test gets its OWN fake home, on its own thread, via the seam's
    // per-thread override. `cargo test` runs each test on a separate thread, so
    // thread-local IS the isolation boundary — no shared root for the sessions
    // in one test to collide with the sessions in another. The previous version
    // set `$HOME`, which the seam deliberately no longer reads: `dirs` ignores
    // `$HOME` on Windows, which is the whole reason the override exists.
    //
    // The lock survives for one reason only: `no_autoname_env_opts_out` sets
    // BASE_NO_AUTONAME, which `auto_register` reads, and env is process-global.
    // Taken with `unwrap_or_else(into_inner)` so a failing test fails ALONE —
    // the old `unwrap()` is why a single assertion failure surfaced as nine,
    // eight of them PoisonError noise pointing at the lock instead of the bug.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::relay::scrub_shell_env();
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || f(tmp.path()))
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
            kind: "task".into(),
            refs: Vec::new(),
            spool_store: String::new(),
            spool_id: String::new(),
        }
    }

    fn sample_ping(kind: &str, from: &str, to_title: &str, session: &str, msg: &str) -> InboxTask {
        InboxTask {
            slug: format!("ping-{}", msg.len()),
            summary: msg.into(),
            doc: String::new(),
            from: from.into(),
            to_title: to_title.into(),
            to_session: session.into(),
            priority: "high".into(),
            created: crate::relay::now_iso(),
            status: "pending".into(),
            last_loud_session: String::new(),
            last_alert_ts: String::new(),
            kind: kind.into(),
            refs: Vec::new(),
            spool_store: String::new(),
            spool_id: String::new(),
        }
    }

    /// Bind the receiver title to a session id in the global registry so
    /// deliver()'s session→titles resolution finds the inbox.
    fn bind(title: &str, session: &str, home: &Path) {
        crate::relay::session_registry::register(title, session, home, None).unwrap();
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
    fn a_pending_task_is_shown_in_full_on_a_prompt() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            enqueue(&ns, &sample("sid-B")).unwrap();
            let block = deliver("sid-B", Phase::Prompt).expect("pending task must deliver");
            assert!(block.starts_with("relay: task rebuild-auth-guard [high] from api-session ("), "{block}");
            assert!(block.contains("when finished: base relay done rebuild-auth-guard"), "{block}");
            assert!(block.contains("\nRebuild the auth guard\n"), "{block}");
            assert!(block.contains("brief: /tmp/rebuild-auth-guard.md"), "{block}");
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
            assert!(deliver("sid-B", Phase::Prompt).is_none(), "and never repeats on later prompts");
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
            assert!(block.starts_with("relay: task rebuild-auth-guard"), "{block}");
            // The same session starting again (a compaction) gets the one-line listing, not the brief again.
            let again = deliver("sid-C", Phase::SessionStart).expect("an open task is listed");
            assert_eq!(again, "relay: 1 open task (rebuild-auth-guard from api-session) · when finished: base relay done <slug>\n");
        });
    }

    fn days_ago(days: i64) -> String {
        (chrono::Local::now() - chrono::Duration::days(days)).format("%Y-%m-%dT%H:%M:%S%z").to_string()
    }

    #[test]
    fn stale_ping_never_alerts_a_new_holder_of_the_title() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("cougar", "sid-B", home);
            // The 2026-09-23 case: delivered long ago to a session that is gone.
            let mut delivered = sample_ping("ping", "px-depth", "cougar", "sid-A", "ack. Ended.");
            delivered.created = days_ago(25);
            delivered.status = "delivered".into();
            delivered.last_loud_session = "sid-A".into();
            enqueue(&ns, &delivered).unwrap();
            // And one that never reached anyone, from two days ago.
            let mut pending = sample_ping("ping", "heron", "cougar", "sid-A", "old question");
            pending.created = days_ago(2);
            enqueue(&ns, &pending).unwrap();
            assert!(deliver("sid-B", Phase::SessionStart).is_none(), "stale pings must not fire loud");
            assert!(deliver("sid-B", Phase::Prompt).is_none(), "nor on a prompt");
            // Nothing is deleted: the files stay readable in the inbox.
            assert_eq!(read_tasks_in(&title_dir("cougar").unwrap()).len(), 2);
        });
    }

    #[test]
    fn fresh_ping_and_stale_task_still_fire() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("cougar", "sid-B", home);
            // A ping just inside the window still alerts.
            let mut fresh = sample_ping("ping", "heron", "cougar", "sid-B", "still live");
            fresh.created = (chrono::Local::now() - chrono::Duration::hours(23))
                .format("%Y-%m-%dT%H:%M:%S%z")
                .to_string();
            enqueue(&ns, &fresh).unwrap();
            let block = deliver("sid-B", Phase::SessionStart).expect("a ping under a day old fires");
            assert!(block.contains("still live"));
            // An old TASK is assigned work and persists until done.
            let mut task = sample("sid-B");
            task.to_title = "cougar".into();
            task.created = days_ago(25);
            enqueue(&ns, &task).unwrap();
            bind("cougar", "sid-C", home);
            let block = deliver("sid-C", Phase::SessionStart).expect("an old task still re-announces");
            assert!(block.contains("relay: task rebuild-auth-guard"), "{block}");
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
    fn explicit_register_retires_auto_codename_ghost() {
        with_home(|home| {
            // Boot order in every real session: auto-name first, explicit second.
            let ghost = crate::relay::session_registry::touch("sess-g", home).unwrap();
            crate::relay::session_registry::register("realname", "sess-g", home, None).unwrap();
            let titles = crate::relay::session_registry::titles_for("sess-g");
            assert_eq!(titles, vec!["realname".to_string()], "ghost '{ghost}' must be retired");
            // Re-touch must NOT resurrect a new codename — an explicit title exists.
            assert_eq!(crate::relay::session_registry::touch("sess-g", home).unwrap(), "realname");
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

    #[test]
    fn legacy_task_json_without_kind_still_parses() {
        with_home(|home| {
            bind("caddy-backend", "sid-B", home);
            // A task written by a pre-ping binary has no `kind`/`refs` fields.
            let dir = title_dir("caddy-backend").unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("old-task.json"),
                r#"{"slug":"old-task","summary":"legacy","doc":"","from":"x","to_title":"caddy-backend","to_session":"sid-B","priority":"high","created":"2026-07-01T00:00:00-0500","status":"pending","last_loud_session":"","last_alert_ts":""}"#,
            )
            .unwrap();
            let block = deliver("sid-B", Phase::Prompt).expect("legacy task must deliver");
            assert!(block.starts_with("relay: task old-task"), "defaults to kind=task: {block}");
        });
    }

    #[test]
    fn a_ping_is_information_with_its_reply_command() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            enqueue(&ns, &sample_ping("ping", "orchestrator", "caddy-backend", "sid-B", "auth guard status?")).unwrap();

            let block = deliver("sid-B", Phase::Prompt).expect("a new ping is shown on the next prompt");
            let header = block.lines().next().unwrap();
            assert!(header.starts_with("relay: ping from orchestrator ("), "{block}");
            assert!(header.ends_with(") · reply: base relay ping --to orchestrator --msg \"...\""), "{block}");
            assert!(block.lines().any(|l| l == "auth guard status?"), "{block}");
            assert!(deliver("sid-B", Phase::Prompt).is_none(), "shown once, never repeated on a prompt");

            // Unanswered → still in the inbox (unlike a reply, which consumes).
            assert_eq!(list_all().len(), 1);
        });
    }

    #[test]
    fn reply_announces_once_then_consumes() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("orchestrator", "sid-A", home);
            enqueue(&ns, &sample_ping("reply", "caddy-backend", "orchestrator", "sid-A", "ack — on it")).unwrap();

            let block = deliver("sid-A", Phase::Prompt).expect("reply must announce");
            assert!(block.starts_with("relay: reply from caddy-backend ("), "{block}");
            assert!(block.contains("ack — on it"));
            assert!(!block.contains("REPLY REQUIRED"), "a reply never demands its own ack");

            // Consumed on delivery — inbox empty, nothing re-fires.
            assert!(list_all().is_empty());
            assert!(deliver("sid-A", Phase::SessionStart).is_none());
        });
    }

    #[test]
    fn notify_announces_once_then_consumes() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("worker-p11", "sid-N", home);
            enqueue(&ns, &sample_ping("notify", "orchestrator", "worker-p11", "sid-N", "[notify] contracts frozen")).unwrap();

            let block = deliver("sid-N", Phase::Prompt).expect("notify must announce");
            assert!(block.starts_with("relay: message from orchestrator ("), "{block}");
            assert!(block.contains("contracts frozen"));
            assert!(!block.contains("REPLY REQUIRED"), "a notify never demands an ack");

            // Consumed on delivery — inbox empty, nothing re-fires.
            assert!(list_all().is_empty());
            assert!(deliver("sid-N", Phase::SessionStart).is_none());
        });
    }

    #[test]
    fn reply_send_clears_inbound_pings_from_peer() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            enqueue(&ns, &sample_ping("ping", "orchestrator", "caddy-backend", "sid-B", "status?")).unwrap();
            enqueue(&ns, &sample_ping("ping", "someone-else", "caddy-backend", "sid-B", "unrelated")).unwrap();
            let mut hidden = sample_ping("reply", "orchestrator", "caddy-backend", "sid-B", "an older answer");
            hidden.slug = "hidden-reply".into();
            hidden.status = "superseded".into();
            enqueue(&ns, &hidden).unwrap();

            // caddy-backend pings orchestrator back → only orchestrator's ping clears.
            let cleared = clear_pings_from(&ns, "orchestrator", &["caddy-backend".to_string()]);
            assert_eq!(cleared, 1);
            let remaining = list_all();
            assert_eq!(remaining.len(), 1);
            assert_eq!(remaining[0].from, "someone-else");
        });
    }

    #[test]
    fn untitled_sender_ping_falls_back_to_done() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            let mut p = sample_ping("ping", "", "caddy-backend", "sid-B", "fire and forget");
            p.slug = "ping-anon".into();
            enqueue(&ns, &p).unwrap();

            let block = deliver("sid-B", Phase::Prompt).expect("ping must fire");
            assert!(block.contains("base relay done ping-anon"), "untitled sender → done escape hatch");
            assert_eq!(done(&ns, "ping-anon").unwrap(), 1);
            assert!(list_all().is_empty());
        });
    }

    /// #101: `hook/mod.rs` matches star commands against the RENDERED block,
    /// so where each renderer puts the operator's message decides whether a
    /// command in it can be addressed at all. Under the leading-star-run rule
    /// a message interpolated mid-line can never activate one — and
    /// `cli.rs`'s `kind = if answered > 0 { "reply" }` means the operator
    /// ANSWERING a session's question always takes the reply path.
    ///
    /// So the four renderers must agree: the message starts its own line.
    /// Enumerated from the codebase rather than from this test's assumptions
    /// (law 31) — every `kind` `deliver` can render is a row here.
    #[test]
    fn every_renderer_puts_the_operator_message_on_its_own_line() {
        let kinds = ["task", "ping", "reply", "notify"];
        let mut visited = 0usize;
        for kind in kinds {
            with_home(|home| {
                let ns = NamespaceConfig::default();
                let session = format!("sid-{kind}");
                bind("caddy-backend", &session, home);
                let msg = "MESSAGE-BODY-MARKER";
                let mut t = sample_ping(kind, "chris", "caddy-backend", &session, msg);
                t.slug = format!("slug-{kind}");
                enqueue(&ns, &t).unwrap();

                let block = deliver(&session, Phase::Prompt)
                    .unwrap_or_else(|| panic!("kind={kind} must deliver"));
                assert!(block.contains(msg), "kind={kind}: block must carry the message");
                assert!(
                    block.lines().any(|l| l.starts_with(msg)),
                    "kind={kind}: the message must BEGIN a line, not sit after a prefix.\n\
                     Block was:\n{block}"
                );
            });
            visited += 1;
        }
        // Law 23 — a loop that visited nothing is not a pass.
        assert_eq!(visited, kinds.len(), "visited {visited} of {} kinds", kinds.len());
        assert!(visited > 0, "visited ZERO renderers — this proves nothing");
    }

    fn hours_ago(h: i64) -> String {
        (chrono::Local::now() - chrono::Duration::hours(h)).format("%Y-%m-%dT%H:%M:%S%z").to_string()
    }

    /// BO-04 F13c, with lynx's three conditions: one sender's older unshown messages are hidden behind its newest;
    /// the shown one names the command that lists them; the files stay; a reply to that sender clears them.
    #[test]
    fn a_senders_older_messages_are_hidden_behind_its_newest_never_deleted() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("lynx", "sid-L", home);
            let mut go = sample_ping("ping", "bison", "lynx", "sid-L", "go: edit the doc");
            go.slug = "ping-0910".into();
            go.created = hours_ago(3);
            enqueue(&ns, &go).unwrap();
            let mut other = sample_ping("ping", "heron", "lynx", "sid-L", "an unrelated question");
            other.slug = "ping-1000".into();
            other.created = hours_ago(2);
            enqueue(&ns, &other).unwrap();
            let mut stand_down = sample_ping("ping", "bison", "lynx", "sid-L", "stand down, the go is withdrawn");
            stand_down.slug = "ping-1405".into();
            stand_down.created = hours_ago(1);
            enqueue(&ns, &stand_down).unwrap();

            let block = deliver("sid-L", Phase::Prompt).expect("the newest ping is shown");
            assert!(block.contains("stand down, the go is withdrawn"), "{block}");
            assert!(!block.contains("go: edit the doc"), "a superseded message was shown:\n{block}");
            assert!(block.contains("an unrelated question"), "another sender is its own thread:\n{block}");
            assert!(
                block.contains("(1 earlier message from bison hidden, this one is newer: base relay tasks --from bison)"),
                "{block}"
            );
            // Hidden, never deleted: the file stays and the named listing shows its text.
            let all = list_all();
            let older = all.iter().find(|t| t.slug == "ping-0910").expect("the superseded file stays");
            assert_eq!(older.status, "superseded");
            assert!(format_row(older).contains("go: edit the doc"), "the listing shows the hidden message's text");
            // Never shown again, on a prompt or at session start (the listing line names only delivered pings).
            assert!(deliver("sid-L", Phase::Prompt).is_none());
            let start = deliver("sid-L", Phase::SessionStart).expect("unanswered pings are listed");
            assert!(!start.contains("go: edit the doc"), "{start}");
            let listed = format!(
                "relay: 2 unanswered pings (heron {}, bison {})",
                crate::relay::clock(&other.created),
                crate::relay::clock(&stand_down.created)
            );
            assert!(start.starts_with(&listed), "{start}");
            // Lynx replies to bison: everything earlier from bison is answered.
            assert_eq!(clear_pings_from(&ns, "bison", &["lynx".to_string()]), 2);
            assert!(list_all().iter().all(|t| t.from != "bison"), "nothing from bison is left to show");
        });
    }

    /// Tasks are work, never superseded: two tasks from one sender are both shown.
    #[test]
    fn tasks_are_never_superseded() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            let mut one = sample("sid-B");
            one.slug = "task-one".into();
            enqueue(&ns, &one).unwrap();
            let mut two = sample("sid-B");
            two.slug = "task-two".into();
            enqueue(&ns, &two).unwrap();
            let block = deliver("sid-B", Phase::Prompt).expect("tasks deliver");
            assert!(block.contains("relay: task task-one") && block.contains("relay: task task-two"), "{block}");
            assert!(!block.contains("hidden"), "{block}");
        });
    }

    /// F13a at the source: no renderer and no listing line claims to come before the user's prompt.
    #[test]
    fn no_renderer_claims_priority_over_the_user() {
        let banned = [
            "REPLY REQUIRED", "DIRECTIVE", "RIGHT NOW", "PAUSE", "IMMEDIATELY", "BEFORE YOUR NEXT ACTION",
            "\u{1F6A8}", "\u{2757}", "\u{26A0}", "\u{1F514}", "\u{1F4E8}", "\u{23F3}",
        ];
        let mut texts: Vec<String> = Vec::new();
        for kind in ["task", "ping", "reply", "notify"] {
            texts.push(render(&sample_ping(kind, "bison", "lynx", "sid", "a message")));
            texts.push(render(&sample_ping(kind, "", "lynx", "sid", "a message")));
        }
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("lynx", "sid-1", home);
            enqueue(&ns, &sample_ping("ping", "bison", "lynx", "sid-1", "first")).unwrap();
            let mut t = sample("sid-1");
            t.to_title = "lynx".into();
            enqueue(&ns, &t).unwrap();
            texts.push(deliver("sid-1", Phase::Prompt).unwrap());
            texts.push(deliver("sid-1", Phase::SessionStart).unwrap());
        });
        assert_eq!(texts.len(), 10, "control: every renderer and both listings were read");
        for text in &texts {
            for phrase in banned {
                assert!(!text.contains(phrase), "{phrase:?} in relay text:\n{text}");
            }
        }
    }
}
