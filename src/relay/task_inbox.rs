//! Session-targeted task inbox — the delivery channel for `*task <title> …`
//! and `*ping <title> …`.
//!
//! A task relayed to a live session lands as one JSON file under
//! `~/.base-gbl/.base/relay-inbox/<target-title>/<slug>.json`. The folder is
//! named for the receiver's TITLE, but each item carries the session id that held
//! the title when it was sent, and only that session is ever shown it (BO-05, F12).
//! A title that passes to another session (a restart, a /clear, a codename handed
//! out again) does not pass its inbox: the previous holder's items are archived
//! under `relay-inbox/.archive/<title>-<session>/` and their senders told. The
//! target's hooks resolve their session id → held titles and scan those dirs at
//! session start and on each prompt, never on a tool call (BO-04, F13b). A new
//! item is shown once, in full, as information; while a ping stays unanswered,
//! session start lists it in one line.
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
/// one that has waited a day is history. A delivered one stays in the inbox file for anyone who reads it;
/// one that no session was ever shown is archived and its sender told (BO-05, carried in from BO-00's code
/// review: it used to be skipped here forever, silently).
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
    /// The session that held `to_title` when this was sent (BO-05, F12a). Delivery shows the item only to that session
    /// ([`deliver_deferred`]); when the title passes to another session, the item is archived instead
    /// ([`settle`]). An item with this empty is placed by the title history ([`super::session_registry::holder_at`]).
    #[serde(default)]
    pub to_session: String,
    /// The sending session, when it held the `from` title at send time. A notice that this item was not delivered goes
    /// to that session only, never to a later holder of the sender's title (BO-05, F12d). Empty when unknown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from_session: String,
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

/// Which hook is asking to deliver. Relay content appears at session start and on a prompt, never at a turn's end, and
/// on a tool call only for a run that cannot keep an inbox watcher (BO-04, F13b as amended by lynx).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    SessionStart,
    Prompt,
    /// A tool call in a run with no inbox watcher ([`super::monitorless`]): new items only, nothing listed.
    Tool,
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
    // Both ends of the item now use their titles, so both are reminded to keep an inbox watcher (BO-27, V2). The
    // sender's session is recorded only when the sending process holds its `from` title (`relay::sending_session`).
    super::session_registry::mark_used(&[
        (task.to_title.as_str(), task.to_session.as_str()),
        (task.from.as_str(), task.from_session.as_str()),
    ]);

    // Durable graph mirror — best-effort. A failure here must never sink the
    // relay: the JSON inbox is what actually drives delivery.
    if let Err(e) = mirror_to_graph(ns, task) {
        eprintln!("base relay task: graph mirror skipped: {e:#}");
    }
    Ok(path)
}

// ─── Addressing (BO-05, F12) ─────────────────────────────────
//
// An inbox item is for the session that held its title when it was sent, never for whoever holds the title now. On
// 2026-10-01 session 5b860473 was auto-given the title `lynx`, inherited the inbox of the session that had held it, and
// was shown bison's pings about a doc it had never seen, while bison had no way to know they had gone to the wrong
// session. Now a title's folder holds only what was sent to its holder: anything sent to another session is moved to
// `relay-inbox/.archive/<title>-<that session>/`, and the sender of anything no session was shown is told, once.

/// The folder under the inbox root that holds archived inboxes. A dot name: no title sanitizes to it, and the readers
/// that walk the inbox root (`base relay tasks`, `base relay done`) never find an item directly inside it.
const ARCHIVE: &str = ".archive";

/// Who a notice that something was not delivered comes from. Never a thread (`is_chat` is false for its kind), so two
/// notices to one session are both shown.
pub const NOTICE_FROM: &str = "relay";

/// Where the items sent to `session` under `title` are archived: `relay-inbox/.archive/<title>-<session>/`.
pub fn archive_dir(title: &str, session: &str) -> Option<PathBuf> {
    inbox_root().map(|r| r.join(ARCHIVE).join(sanitize(&format!("{title}-{session}"))))
}

/// Why an item left its title's inbox, which decides what its sender is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Why {
    /// It was sent to a session that no longer holds the title.
    Passed,
    /// It carries no session id, and the title history has no line placing it (F12e).
    Unplaced,
    /// It was sent to the holder, which was never shown it in the day a ping stays live (BO-00's carry-in).
    Stale,
}

/// One item moved to an archive folder that no session was shown.
struct Archived {
    task: InboxTask,
    folder: PathBuf,
    /// The session the folder is named for.
    old: String,
    why: Why,
}

/// A title has just been claimed by `holder` (an explicit `relay register`, a tab reclaim after /clear, a wordlist
/// pick, or a launcher's `BASE_RELAY_AS`), taken from `previous` when another session held it. Archives everything in
/// the title's inbox that was sent to another session and tells the senders, so the new holder starts with an inbox
/// holding only what was sent to it. Fail-open: nothing here can stop a registration.
pub fn settle(title: &str, holder: &str, previous: Option<&str>) {
    let _ = sort_inbox(title, holder, previous);
}

/// Settle `title`'s inbox for `holder`, the session that holds the title now, and return the items addressed to it.
///
/// - An item stays when it was sent to `holder` or shown to it ([`addressee`], [`shown_to`]). Any other is moved to the
///   archive folder of the session that was shown it, or else the session it was sent to (F12b, F12c).
/// - An item with no session id is placed by the title history: read as sent to whoever held the title when it was
///   written. When the history cannot say, it is archived with `previous`'s folder, or under `unknown`, and never
///   delivered (F12e).
/// - A ping addressed to `holder` that no session was shown and that has waited past [`PING_STALE_SECS`] is archived in
///   `holder`'s folder. It used to be skipped forever, silently (BO-00's code review).
/// - What `holder` had been shown under the title and lost when the title passed away from it comes back
///   ([`restore_shown`]).
///
/// Each sender of an archived item that no session was shown is told, by one notice per folder ([`tell_senders`]).
/// The lock is taken only when something has to move; when another process holds it, this one moves nothing and still
/// returns only `holder`'s items.
fn sort_inbox(title: &str, holder: &str, previous: Option<&str>) -> Vec<(PathBuf, InboxTask)> {
    let Some(dir) = title_dir(title) else { return Vec::new() };
    restore_shown(title, holder, &dir);
    let items = read_tasks_in(&dir);
    if items.is_empty() {
        return items;
    }
    let mut kept = Vec::new();
    let mut leaving: Vec<(PathBuf, InboxTask, String, Why)> = Vec::new();
    for (path, mut task) in items {
        let sent_to = addressee(title, &task);
        let seen_by = shown_to(&task);
        if sent_to.as_deref() == Some(holder) || seen_by.as_deref() == Some(holder) {
            if task.status == "pending" && ping_is_stale(&task) {
                leaving.push((path, task, holder.to_string(), Why::Stale));
                continue;
            }
            if task.to_session.is_empty()
                && let Some(s) = sent_to
            {
                // Placed by the history: the item says so from now on.
                task.to_session = s;
                let _ = write_json_atomic(&path, &task);
            }
            kept.push((path, task));
            continue;
        }
        // Filed with the session that was shown it, if one was, so that session gets it back if it takes the title
        // back; otherwise with the session it was sent to.
        match seen_by.or(sent_to) {
            Some(s) => leaving.push((path, task, s, Why::Passed)),
            None => leaving.push((path, task, previous.unwrap_or("unknown").to_string(), Why::Unplaced)),
        }
    }
    if leaving.is_empty() {
        return kept;
    }
    let Some(lock) = SettleLock::take(&dir) else { return kept };
    let mut archived: Vec<Archived> = Vec::new();
    for (path, task, old, why) in leaving {
        let Some(folder) = archive_dir(title, &old) else { continue };
        if !move_into(&path, &folder) {
            continue;
        }
        if task.kind == "notify" && task.status == "pending" {
            // The spool holds back its copy only while the notify is in the inbox. Marked seen for this title, it is
            // never shown to the title's new holder either (F12b).
            mark_spool_seen(&task);
        }
        if never_shown(&task) {
            archived.push(Archived { task, folder, old, why });
        }
    }
    drop(lock);
    tell_senders(title, holder, &archived);
    kept
}

/// The session an item was sent to: its `to_session`, or for an item that records none, the history's holder of the
/// title when it was written.
fn addressee(title: &str, task: &InboxTask) -> Option<String> {
    if task.to_session.is_empty() {
        parse_ts(&task.created).and_then(|t| super::session_registry::holder_at(title, t))
    } else {
        Some(task.to_session.clone())
    }
}

/// The session an item was shown to, once it was. An item stays with the title's holder when it was sent to it OR
/// shown to it. After BO-05 a session is shown only what was sent to it, so the two agree; before, they could differ
/// both ways: BO-04 showed a session that took a title over the previous holder's open task and unanswered pings (it
/// keeps them), and a session that held a title for a while was shown what was sent to the holder before and after it
/// (the holder keeps those).
fn shown_to(task: &InboxTask) -> Option<String> {
    (matches!(task.status.as_str(), "delivered" | "superseded") && !task.last_loud_session.is_empty())
        .then(|| task.last_loud_session.clone())
}

/// A session that takes back a title it held gets back what it had been shown under it: an open task, an unanswered
/// ping. They were its own; they left only because the title passed to another session meanwhile (for one, a headless
/// run started from this session's terminal tab, which takes the tab's title: F27, BO-08). Items it was never shown
/// stay archived, since their senders were told they were not delivered.
fn restore_shown(title: &str, holder: &str, dir: &Path) {
    let Some(folder) = archive_dir(title, holder) else { return };
    if !folder.is_dir() {
        return;
    }
    for (path, task) in read_tasks_in(&folder) {
        if task.last_loud_session == holder && matches!(task.status.as_str(), "delivered" | "superseded") {
            let _ = move_into(&path, dir);
        }
    }
}

/// Work or a message that no session was shown: its sender is owed a notice when it is archived. A notice is never
/// itself the subject of one.
fn never_shown(task: &InboxTask) -> bool {
    task.status == "pending" && matches!(task.kind.as_str(), "ping" | "reply" | "task" | "notify")
}

/// Move one file into `folder`, keeping its name unless that name is already taken there, in which case a number is
/// added: a rename onto an existing name would replace that file on Windows.
fn move_into(path: &Path, folder: &Path) -> bool {
    if std::fs::create_dir_all(folder).is_err() {
        return false;
    }
    let Some(name) = path.file_name() else { return false };
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut dest = folder.join(name);
    let mut n = 1;
    while dest.exists() {
        if n > 1000 {
            return false;
        }
        dest = folder.join(format!("{stem}-{n}.json"));
        n += 1;
    }
    std::fs::rename(path, dest).is_ok()
}

/// One process at a time moves a title's items, so a sender gets one notice per folder rather than one per process.
/// A lock older than 30 seconds is a crashed holder's and is broken.
struct SettleLock(PathBuf);

impl SettleLock {
    fn take(dir: &Path) -> Option<Self> {
        let path = dir.join(".settle.lock");
        for _ in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Some(Self(path)),
                Err(_) => {
                    let old = std::fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|m| m.elapsed().ok())
                        .is_some_and(|e| e.as_secs() > 30);
                    if !old {
                        return None;
                    }
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        None
    }
}

impl Drop for SettleLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The session a notice about `task` goes to: the session that sent it, while that session still holds the sender's
/// title. Which session sent it is the item's `from_session`, or, for an item that does not record one (a script's
/// `--from`, a binary from before BO-05), the session the title history says held the sender's title when it was sent.
/// Nobody is told when that cannot be established, when the sending session no longer holds its title (a later holder
/// never sent it: telling it would repeat F12 the other way round), when the sender has no title, or when the
/// sender's title is the one that passed (its holder is the session the item did not reach).
fn notice_to(task: &InboxTask, title: &str) -> Option<String> {
    let from = task.from.as_str();
    if super::threadless(from) || from == NOTICE_FROM || from == title {
        return None;
    }
    let holder = super::session_registry::resolve(from)?.session_id;
    let sent_by = if task.from_session.is_empty() {
        parse_ts(&task.created).and_then(|t| super::session_registry::holder_at(from, t))
    } else {
        Some(task.from_session.clone())
    };
    (sent_by.as_deref() == Some(holder.as_str())).then_some(holder)
}

/// Tell each sender what of theirs was archived without being shown: one notice per sender, folder and reason, in the
/// sender's own inbox, addressed to the sending session (F12d). A sender that cannot be reached is not told; the
/// archive folder keeps everything either way.
fn tell_senders(title: &str, holder: &str, archived: &[Archived]) {
    let mut groups: std::collections::BTreeMap<(String, String, PathBuf, Why), Vec<&Archived>> =
        std::collections::BTreeMap::new();
    for a in archived {
        let Some(to) = notice_to(&a.task, title) else { continue };
        groups.entry((a.task.from.clone(), to, a.folder.clone(), a.why)).or_default().push(a);
    }
    for ((from, to, folder, why), items) in groups {
        let Some(dir) = title_dir(&from) else { continue };
        let notice = InboxTask {
            slug: super::ping_slug(),
            summary: notice_text(title, holder, &items, &folder, why),
            doc: String::new(),
            from: NOTICE_FROM.into(),
            to_title: from.clone(),
            to_session: to,
            from_session: String::new(),
            priority: "high".into(),
            created: now_iso(),
            status: "pending".into(),
            last_loud_session: String::new(),
            last_alert_ts: String::new(),
            kind: "undelivered".into(),
            refs: Vec::new(),
            spool_store: String::new(),
            spool_id: String::new(),
        };
        let path = dir.join(format!("{}.json", sanitize(&notice.slug)));
        if let Err(e) = write_json_atomic(&path, &notice) {
            eprintln!("base relay: could not tell {from} that its message to {title} was not delivered: {e:#}");
        }
    }
}

/// What a sender is told (F12d, example 1): what was not delivered, whose the title was, who holds it now, where the
/// items are, and what to do.
fn notice_text(title: &str, holder: &str, items: &[&Archived], folder: &Path, why: Why) -> String {
    let n = items.len();
    let noun = if items.iter().all(|a| a.task.kind == "task") {
        "task"
    } else if items.iter().all(|a| matches!(a.task.kind.as_str(), "ping" | "reply")) {
        "ping"
    } else {
        "message"
    };
    let what = if n == 1 { noun.to_string() } else { format!("{n} {noun}s") };
    let sent: Vec<String> = items.iter().map(|a| super::clock(&a.task.created)).collect();
    // Subject and object pronouns, and the verbs that agree with them.
    let (was, it, them, carries, apply) = if n == 1 {
        ("was", "it", "it", "carries", "applies")
    } else {
        ("were", "they", "them", "carry", "apply")
    };
    let path = folder.to_string_lossy().replace('\\', "/");
    let now_holds = match super::session_registry::held_since(title, holder) {
        Some(since) => format!("session {} (since {})", short(holder), super::clock(&since)),
        None => format!("session {}", short(holder)),
    };
    let old = items.first().map(|a| a.old.as_str()).unwrap_or_default();
    let opening = format!("your {what} to {title} (sent {}) {was} not delivered", sent.join(", "));
    match why {
        Why::Passed => {
            let gone = match super::session_registry::titles_for(old).first() {
                Some(t) => format!("which is now {t}"),
                None => "which no longer holds it".to_string(),
            };
            format!(
                "{opening}. {title} was session {}, {gone}; the title now belongs to {now_holds}, which has not seen \
                 {them}. Archived at {path}/. Re-send to the new holder if {it} still {apply}.",
                short(old)
            )
        }
        Why::Unplaced => format!(
            "{opening}: {it} {carries} no session id, and base has no record of which session held {title} when {it} \
             {was} sent. The title now belongs to {now_holds}. Archived at {path}/. Re-send to the new holder if {it} \
             still {apply}."
        ),
        Why::Stale => format!(
            "{opening}: {it} waited more than a day, and {now_holds}, which holds {title}, was never shown {them}. \
             Archived at {path}/. Re-send if {it} still {apply}."
        ),
    }
}

/// The first eight characters of a session id, as the registry prints it and a reader recognises it.
fn short(session: &str) -> String {
    session.chars().take(8).collect()
}

// ─── Delivery (target side, called from hooks) ───────────────

/// Scan this session's inbox and render the injection block, if any. Mutates
/// task state (marks delivered, deletes an announced reply or notify) as a side effect.
pub fn deliver(session_id: &str, phase: Phase) -> Option<String> {
    deliver_deferred(session_id, phase).map(super::Part::commit)
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

/// A ping still waiting for its answer: shown (`delivered`) or hidden behind a newer message from the same sender
/// (`superseded`). Hiding a ping never drops what it asks of the receiver; a reply to the sender clears both.
fn unanswered(task: &InboxTask) -> bool {
    task.kind == "ping" && matches!(task.status.as_str(), "delivered" | "superseded")
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
///   kept on disk, and the shown one carries a line naming the command that lists them. A sender with no title is
///   never treated as one thread ([`super::threadless`]).
/// - Items already shown to this session: listed in one line at session start (a compacted context), never on a
///   prompt.
/// - On a tool call ([`Phase::Tool`], only for a run with no inbox watcher, see [`super::monitorless`]): new items
///   only, the same way, and nothing listed.
///
/// BO-05 (F12b): only items addressed to THIS session are read. Anything in its titles' folders that was sent to
/// another session is archived first and its sender told ([`sort_inbox`]). BO-04 showed a session that took a title
/// over the previous holder's open task in full and its unanswered pings in a line; a session that takes a title now
/// starts with an inbox holding only what was sent to it, so there is nothing of another session's to show.
///
/// That settling runs at once, before the block is built, and is not held back with the commits: it shows this session
/// nothing, it only moves out what was never this session's (and a ping of its own that went a day unshown), and tells
/// those senders. Everything this session is shown, hidden from or marked as having seen still waits for its commit.
///
/// What this replaced: a loud block on every first sighting in ANY hook, pre-tool included, headed "REPLY REQUIRED
/// BEFORE YOUR NEXT ACTION", then a terse "Reply RIGHT NOW" nag every three minutes on prompts and tool calls until the
/// receiver answered.
pub fn deliver_deferred(session_id: &str, phase: Phase) -> Option<super::Part> {
    // Which titles does this session hold? A never-registered session can't be
    // a relay target, so it does zero filesystem work beyond the registry read.
    let titles = super::session_registry::titles_for(session_id);
    if titles.is_empty() {
        return None;
    }
    let mut tasks: Vec<(PathBuf, InboxTask)> = Vec::new();
    let mut addressed: Vec<&str> = Vec::new();
    for title in &titles {
        let mine = sort_inbox(title, session_id, None);
        if !mine.is_empty() {
            addressed.push(title.as_str());
        }
        tasks.extend(mine);
    }
    // A title whose folder holds an item addressed to this session is in use, whoever wrote the file (BO-27, V2):
    // `enqueue` marks what it writes, and this covers the rest. At once, not as a commit: the item was addressed to
    // this session whether or not this prompt prints it.
    let pairs: Vec<(&str, &str)> = addressed.into_iter().map(|t| (t, session_id)).collect();
    super::session_registry::mark_used(&pairs);
    // A delivered ping past a day is history: never listed again, and its file stays until its sender is answered.
    tasks.retain(|(_, t)| t.status != "done" && !ping_is_stale(t));
    tasks.sort_by(|a, b| order_key(&a.1).cmp(&order_key(&b.1)));
    if tasks.is_empty() {
        return None;
    }

    // Superseded: per (title, sender), every unshown chat message but the newest.
    let keys: Vec<Option<(&str, &str)>> = tasks
        .iter()
        .map(|(_, t)| {
            (t.status == "pending" && is_chat(t) && !super::threadless(&t.from))
                .then_some((t.to_title.as_str(), t.from.as_str()))
        })
        .collect();
    let behind = super::superseded_by(&keys);
    let mut hidden: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for newer in behind.iter().flatten() {
        *hidden.entry(*newer).or_default() += 1;
    }

    let now = now_iso();
    let mut shown: Vec<(usize, String)> = Vec::new();
    let mut open_pings: Vec<String> = Vec::new();
    let mut open_tasks: Vec<String> = Vec::new();
    let mut commits: Vec<super::Commit> = Vec::new();

    for (i, (path, task)) in tasks.iter().enumerate() {
        if task.status == "pending" {
            if behind[i].is_some() {
                commits.push(hide(path.clone(), task.clone(), session_id, &now));
            } else {
                shown.push((i, render(task)));
                commits.push(consume(path.clone(), task.clone(), session_id, &now));
            }
            continue;
        }
        if phase != Phase::SessionStart {
            continue;
        }
        if task.kind == "task" {
            open_tasks.push(format!("{} from {}", task.slug, sender(task)));
        } else if unanswered(task) {
            open_pings.push(format!("{} {}", sender(task), super::clock(&task.created)));
        }
    }

    if shown.is_empty() && open_pings.is_empty() && open_tasks.is_empty() {
        return None;
    }
    let mut text = String::new();
    for (i, rendered) in &shown {
        text.push_str(rendered);
        if let Some(n) = hidden.get(i) {
            let from = sender(&tasks[*i].1);
            text.push_str(&super::hidden_line(*n, from, &format!("base relay tasks --from {from}")));
        }
    }
    if !open_pings.is_empty() {
        text.push_str(&format!(
            "relay: {} unanswered ping{} ({}) · read: base relay tasks --from <sender> · reply: base relay ping --to <sender> --msg \"...\"\n",
            open_pings.len(),
            if open_pings.len() == 1 { "" } else { "s" },
            open_pings.join(", ")
        ));
    }
    if !open_tasks.is_empty() {
        text.push_str(&format!(
            "relay: {} open task{} ({}) · when finished: base relay done <slug>\n",
            open_tasks.len(),
            if open_tasks.len() == 1 { "" } else { "s" },
            open_tasks.join(", ")
        ));
    }
    let items = shown.len() + open_pings.len() + open_tasks.len();
    Some(super::Part { text, commits, items })
}

/// The order items are read in: when they were sent, then, inside one second, the milliseconds a ping's or notify's
/// slug carries, then the slug. `created` has one-second resolution, and the slugs alone sort every `notify-` before
/// every `ping-`, so a ping sent after a notify in the same second would read as the older of the two.
fn order_key(task: &InboxTask) -> (Option<chrono::DateTime<chrono::Local>>, u128, &str) {
    (parse_ts(&task.created), slug_millis(&task.slug), &task.slug)
}

/// The send time in milliseconds that `ping-<millis>-...` and `notify-<millis>-...` slugs open with; 0 for any other.
fn slug_millis(slug: &str) -> u128 {
    let rest = slug.strip_prefix("ping-").or_else(|| slug.strip_prefix("notify-")).unwrap_or("");
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

/// Write an item back where it was read, unless it is no longer there: another process may have archived it between
/// the read and this commit (BO-05), and writing it back would put a copy addressed to another session in the folder.
fn persist(path: PathBuf, task: InboxTask) -> super::Commit {
    Box::new(move || rewrite(&path, &task))
}

fn rewrite(path: &Path, task: &InboxTask) {
    if path.is_file() {
        let _ = write_json_atomic(path, task);
    }
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

/// What showing a message does to the inbox: a reply, notify or not-delivered notice is gone (a notify's spool copy is
/// marked seen, so the spool never shows it again); a ping or task is recorded as delivered to this session.
fn consume(path: PathBuf, mut task: InboxTask, session_id: &str, now: &str) -> super::Commit {
    if matches!(task.kind.as_str(), "reply" | "notify" | "undelivered") {
        return Box::new(move || {
            mark_spool_seen(&task);
            let _ = std::fs::remove_file(&path);
        });
    }
    task.status = "delivered".into();
    task.last_loud_session = session_id.to_string();
    task.last_alert_ts = now.to_string();
    persist(path, task)
}

/// What hiding a superseded message does: the file stays, readable by `base relay tasks --from <sender>`, and is never
/// shown by a hook. A hidden ping still counts as unanswered.
fn hide(path: PathBuf, mut task: InboxTask, session_id: &str, now: &str) -> super::Commit {
    task.status = "superseded".into();
    task.last_loud_session = session_id.to_string();
    task.last_alert_ts = now.to_string();
    Box::new(move || {
        mark_spool_seen(&task);
        rewrite(&path, &task);
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
        // BO-05 (F12d): base telling a sender that what it sent went to a session that no longer holds the title.
        "undelivered" => format!("relay: not delivered ({when}) · no answer needed"),
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
/// flipping each graph mirror to answered (best-effort), and with them every message from `peer` hidden as superseded.
/// Returns how many pings cleared.
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

/// A ping to `peer` that answers one: when `peer` has a ping waiting in one of `my_titles`' inboxes, everything `peer`
/// sent no later than the newest such ping is answered (BO-04, F13c, lynx's condition 3): the pings and superseded
/// messages are cleared ([`clear_pings_from`]), an unshown reply or notify from then or before is cleared with its spool
/// copy marked seen, and `peer`'s spool messages from then or before are marked seen. A ping that answers nothing marks
/// nothing, and nothing `peer` sent after its newest ping is touched. Returns how many pings cleared.
pub fn answer_from(ns: &NamespaceConfig, cwd: &Path, peer: &str, my_titles: &[String]) -> usize {
    let mut cutoff: Option<chrono::DateTime<chrono::Local>> = None;
    for title in my_titles {
        let Some(dir) = title_dir(title) else { continue };
        for (_, task) in read_tasks_in(&dir) {
            if task.kind == "ping" && task.from == peer {
                cutoff = cutoff.max(parse_ts(&task.created));
            }
        }
    }
    let Some(cutoff) = cutoff else { return 0 };
    let cleared = clear_pings_from(ns, peer, my_titles);
    for title in my_titles {
        let Some(dir) = title_dir(title) else { continue };
        for (path, task) in read_tasks_in(&dir) {
            if task.from == peer
                && matches!(task.kind.as_str(), "reply" | "notify")
                && parse_ts(&task.created).is_some_and(|t| t <= cutoff)
            {
                mark_spool_seen(&task);
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    super::deliver::mark_answered(cwd, peer, my_titles, cutoff);
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
            from_session: String::new(),
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
            from_session: String::new(),
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

    /// BO-05 (F12c) replaced "a new session re-announces the task loud": a restart that reclaims the title under a new
    /// session id starts with an empty inbox, and the task waits in the old session's archive folder. The same session
    /// starting again (a compaction) still gets its open task in one line.
    #[test]
    fn a_new_session_on_the_title_does_not_inherit_the_task() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            enqueue(&ns, &sample("sid-B")).unwrap();
            assert!(deliver("sid-B", Phase::SessionStart).is_some());
            let again = deliver("sid-B", Phase::SessionStart).expect("an open task is listed");
            assert_eq!(again, "relay: 1 open task (rebuild-auth-guard from api-session) · when finished: base relay done <slug>\n");
            // The receiver restarts and reclaims the SAME title under a new session id.
            bind("caddy-backend", "sid-C", home);
            assert!(deliver("sid-C", Phase::SessionStart).is_none(), "the new session is shown nothing of sid-B's");
            assert!(read_tasks_in(&title_dir("caddy-backend").unwrap()).is_empty(), "its inbox starts empty");
            let archived = read_tasks_in(&archive_dir("caddy-backend", "sid-B").unwrap());
            assert_eq!(archived.len(), 1, "the task is in sid-B's archive folder");
            assert_eq!(archived[0].1.slug, "rebuild-auth-guard");
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
            // Nothing is deleted: both were sent to sid-A, so both are readable in sid-A's archive folder (BO-05).
            assert!(read_tasks_in(&title_dir("cougar").unwrap()).is_empty());
            assert_eq!(read_tasks_in(&archive_dir("cougar", "sid-A").unwrap()).len(), 2);
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
            // An old TASK is assigned work and persists until done: never stale, never archived for its age.
            let mut task = sample("sid-B");
            task.to_title = "cougar".into();
            task.created = days_ago(25);
            enqueue(&ns, &task).unwrap();
            let block = deliver("sid-B", Phase::SessionStart).expect("an old task still announces");
            assert!(block.contains("relay: task rebuild-auth-guard"), "{block}");
            assert!(read_tasks_in(&archive_dir("cougar", "sid-B").unwrap()).is_empty(), "nothing archived");
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
            // The hidden ping is still unanswered, so it is listed (by sender and time, never its text).
            let listed = format!(
                "relay: 3 unanswered pings (bison {}, heron {}, bison {})",
                crate::relay::clock(&go.created),
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

    /// BO-04's review finding 1 showed a session that takes a title over mid-session (`base relay register`) the open
    /// task and unanswered pings another session had been shown. BO-05 (F12c) reverses it: they were sent to session
    /// A, so session B is shown nothing, they move to A's archive folder, and since A was shown both, nobody is told.
    #[test]
    fn a_title_taken_over_mid_session_shows_the_new_holder_nothing_of_the_old() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("heron", "sid-H", home);
            bind("caddy-backend", "sid-A", home);
            enqueue(&ns, &sample("sid-A")).unwrap();
            enqueue(&ns, &sample_ping("ping", "heron", "caddy-backend", "sid-A", "still waiting on you")).unwrap();
            assert!(deliver("sid-A", Phase::Prompt).is_some(), "control: session A was shown both");
            bind("caddy-backend", "sid-B", home);
            assert!(deliver("sid-B", Phase::Prompt).is_none(), "session B is shown nothing of A's");
            assert!(deliver("sid-B", Phase::SessionStart).is_none(), "nor listed at its start");
            assert_eq!(read_tasks_in(&archive_dir("caddy-backend", "sid-A").unwrap()).len(), 2);
            assert!(read_tasks_in(&title_dir("heron").unwrap()).is_empty(), "a delivered ping's sender is not told");
        });
    }

    /// Review finding 3: a newer message of another kind hides an older ping, but never what the ping asks: the hidden
    /// ping is still listed as unanswered at session start.
    #[test]
    fn a_ping_hidden_behind_a_newer_message_stays_unanswered() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("lynx", "sid-L", home);
            let mut question = sample_ping("ping", "bison", "lynx", "sid-L", "which schema?");
            question.slug = "ping-1".into();
            question.created = hours_ago(2);
            enqueue(&ns, &question).unwrap();
            let mut fyi = sample_ping("notify", "bison", "lynx", "sid-L", "build is green");
            fyi.slug = "notify-2".into();
            fyi.created = hours_ago(1);
            enqueue(&ns, &fyi).unwrap();
            let block = deliver("sid-L", Phase::Prompt).expect("the newest is shown");
            assert!(block.contains("build is green") && !block.contains("which schema?"), "{block}");
            let start = deliver("sid-L", Phase::SessionStart).expect("the hidden ping is still owed an answer");
            assert!(start.starts_with("relay: 1 unanswered ping (bison "), "{start}");
        });
    }

    /// Review finding 5: senders with no title are not one conversation; each of their messages is shown.
    #[test]
    fn senders_with_no_title_never_supersede_each_other() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("lynx", "sid-L", home);
            for (i, from) in ["unregistered", "unregistered", "", "unregistered-1234abcd"].iter().enumerate() {
                let mut p = sample_ping("ping", from, "lynx", "sid-L", &format!("script message {i}"));
                p.slug = format!("ping-{i}");
                enqueue(&ns, &p).unwrap();
            }
            let block = deliver("sid-L", Phase::Prompt).expect("all four are shown");
            for i in 0..4 {
                assert!(block.contains(&format!("script message {i}")), "{block}");
            }
            assert!(!block.contains("hidden"), "{block}");
        });
    }

    /// Review finding 6: inside one second, the order is the milliseconds the slugs carry, not `notify-` before `ping-`.
    #[test]
    fn inside_one_second_the_later_message_is_the_newer() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("lynx", "sid-L", home);
            let second = hours_ago(1);
            let mut go = sample_ping("ping", "bison", "lynx", "sid-L", "go: edit the doc");
            go.slug = "ping-1700000000100-0001-0".into();
            go.created = second.clone();
            enqueue(&ns, &go).unwrap();
            let mut stand_down = sample_ping("notify", "bison", "lynx", "sid-L", "stand down");
            stand_down.slug = "notify-1700000000900-bison-77-lynx".into();
            stand_down.created = second;
            enqueue(&ns, &stand_down).unwrap();
            let block = deliver("sid-L", Phase::Prompt).expect("delivery");
            assert!(block.contains("stand down") && !block.contains("go: edit the doc"), "{block}");
        });
    }

    /// Review finding 4: a ping that answers nothing marks nothing; a reply clears only what the sender sent up to its
    /// newest ping, and a message the sender sent after that is still shown.
    #[test]
    fn a_reply_answers_what_came_before_it_and_nothing_after() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("lynx", "sid-L", home);
            let cwd = home.join("ws");
            let mut later = sample_ping("notify", "bison", "lynx", "sid-L", "sent after the ping");
            later.slug = "notify-later".into();
            later.created = hours_ago(1);
            enqueue(&ns, &later).unwrap();
            assert_eq!(answer_from(&ns, &cwd, "bison", &["lynx".to_string()]), 0, "no ping from bison: not a reply");
            assert_eq!(list_all().len(), 1, "a ping that answers nothing marks nothing");
            let mut question = sample_ping("ping", "bison", "lynx", "sid-L", "which schema?");
            question.slug = "ping-q".into();
            question.created = hours_ago(2);
            enqueue(&ns, &question).unwrap();
            let mut earlier = sample_ping("notify", "bison", "lynx", "sid-L", "sent before the ping");
            earlier.slug = "notify-earlier".into();
            earlier.created = hours_ago(3);
            enqueue(&ns, &earlier).unwrap();
            assert_eq!(answer_from(&ns, &cwd, "bison", &["lynx".to_string()]), 1);
            let left: Vec<String> = list_all().into_iter().map(|t| t.slug).collect();
            assert_eq!(left, vec!["notify-later".to_string()], "only what came after the ping is left");
        });
    }

    /// Review finding 8: the block says how many items it carries, counted where they are rendered.
    #[test]
    fn a_block_counts_its_items_where_it_renders_them() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("lynx", "sid-L", home);
            enqueue(&ns, &sample_ping("ping", "bison", "lynx", "sid-L", "relay: this body starts like a header")).unwrap();
            let mut n = sample_ping("notify", "heron", "lynx", "sid-L", "[notify] spool news");
            n.slug = "notify-a".into();
            n.spool_store = home.join("crew").display().to_string();
            enqueue(&ns, &n).unwrap();
            let part = deliver_deferred("sid-L", Phase::Prompt).expect("delivery");
            assert_eq!(part.items, 2, "{}", part.text);
            assert!(part.text.contains("relay (crew): notify from heron ("), "{}", part.text);
        });
    }

    // ─── BO-05 (F12): addressed to a session, not a title ────────

    use crate::relay::session_registry::{self as registry, SessionEntry, SessionRegistry};

    /// Write the registry whole, the way the 2026-10-01 machine had it: every codename held by a live session except
    /// `dead`, which `dead_session` held until its heartbeat stopped two hours ago.
    fn registry_with_one_dead_title(dead: &str, dead_session: &str) {
        let now = crate::relay::now_iso();
        let mut reg = SessionRegistry::default();
        for word in [
            "falcon", "otter", "cobra", "lynx", "heron", "bison", "raven", "koala", "gecko", "panda", "tiger", "wolf",
            "moose", "crane", "viper", "badger", "ferret", "marlin", "osprey", "jackal", "mantis", "walrus", "puffin",
            "dingo", "quokka", "tapir", "narwhal", "cougar", "egret", "finch", "gopher", "hawk", "jaguar", "kestrel",
            "lemur", "meerkat", "ocelot", "pelican", "quail", "robin", "seal", "toucan", "vole", "weasel", "zebra",
            "wren", "stoat", "orca",
        ] {
            let (session, beat) = if word == dead { (dead_session.to_string(), hours_ago(2)) } else { (format!("sid-{word}"), now.clone()) };
            reg.sessions.insert(
                word.into(),
                SessionEntry {
                    title: word.into(),
                    session_id: session,
                    registered_at: hours_ago(5),
                    last_heartbeat: beat,
                    auto: true,
                    ..Default::default()
                },
            );
        }
        let dir = registry::global_base_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sessions.json"), serde_json::to_string_pretty(&reg).unwrap()).unwrap();
    }

    /// The notices in `title`'s inbox: base telling that session something it sent was not delivered.
    fn notices(title: &str) -> Vec<InboxTask> {
        read_tasks_in(&title_dir(title).unwrap()).into_iter().map(|(_, t)| t).filter(|t| t.kind == "undelivered").collect()
    }

    /// Example 1, the 2026-10-01 case on the path it took: the session holding `lynx` ended with two of bison's pings
    /// unshown, and a new session was handed the codename `lynx` from the wordlist. The new session starts with an empty
    /// inbox, the pings are in `.archive/lynx-<old session>/`, and bison is told, once, in its own inbox.
    #[test]
    fn a_codename_handed_out_again_archives_the_dead_holders_inbox() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            registry_with_one_dead_title("lynx", "aaaa1111-0000-4000-8000-000000000001");
            for (i, msg) in ["go: edit the Everything In Progress doc", "stand down on the doc"].iter().enumerate() {
                let mut p = sample_ping("ping", "bison", "lynx", "aaaa1111-0000-4000-8000-000000000001", msg);
                p.slug = format!("ping-{i}");
                p.from_session = "sid-bison".into();
                enqueue(&ns, &p).unwrap();
            }
            let new = "5b860473-5522-5a45-85ed-de1df8fca274";
            assert_eq!(registry::touch(new, home).as_deref(), Some("lynx"), "control: the wordlist hands out lynx");

            assert!(read_tasks_in(&title_dir("lynx").unwrap()).is_empty(), "the new holder's inbox starts empty");
            let archived = read_tasks_in(&archive_dir("lynx", "aaaa1111-0000-4000-8000-000000000001").unwrap());
            assert_eq!(archived.len(), 2, "both pings are in the old holder's archive folder");
            assert!(deliver(new, Phase::SessionStart).is_none() && deliver(new, Phase::Prompt).is_none());

            let told = notices("bison");
            assert_eq!(told.len(), 1, "bison is told once for both pings: {told:?}");
            assert_eq!(told[0].to_session, "sid-bison", "the notice is addressed to the session that sent them");
            let text = &told[0].summary;
            assert!(text.starts_with("your 2 pings to lynx (sent "), "{text}");
            assert!(text.contains(") were not delivered. lynx was session aaaa1111, which no longer holds it; "), "{text}");
            assert!(text.contains("the title now belongs to session 5b860473 (since "), "{text}");
            assert!(text.contains(", which has not seen them. Archived at "), "{text}");
            assert!(text.contains("/relay-inbox/.archive/lynx-aaaa1111-0000-4000-8000-000000000001/"), "{text}");
            assert!(text.ends_with("Re-send to the new holder if they still apply."), "{text}");

            // bison's next prompt shows it, once, as information.
            let block = deliver("sid-bison", Phase::Prompt).expect("bison is shown the notice");
            assert!(block.starts_with("relay: not delivered ("), "{block}");
            assert!(block.contains(") · no answer needed\nyour 2 pings to lynx"), "{block}");
            assert!(deliver("sid-bison", Phase::Prompt).is_none() && deliver("sid-bison", Phase::SessionStart).is_none());
        });
    }

    /// The /clear case: a new session in the same Windows Terminal tab reclaims the tab's title. It is still a new
    /// session, so the title's inbox is settled as on any other change of holder.
    #[test]
    fn a_tab_reclaim_after_clear_archives_the_previous_sessions_inbox() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("heron", "sid-H", home);
            // SAFETY: with_home holds ENV_LOCK, and scrub_shell_env has run.
            unsafe { std::env::set_var("WT_SESSION", "tab-1") };
            let title = registry::touch("sid-A", home).expect("an auto title");
            let mut p = sample_ping("ping", "heron", &title, "sid-A", "are you there?");
            p.from_session = "sid-H".into();
            enqueue(&ns, &p).unwrap();
            let reclaimed = registry::touch("sid-B", home);
            unsafe { std::env::remove_var("WT_SESSION") };
            assert_eq!(reclaimed.as_deref(), Some(title.as_str()), "control: the tab reclaims its title");
            assert!(deliver("sid-B", Phase::Prompt).is_none(), "sid-B is not shown sid-A's ping");
            assert_eq!(read_tasks_in(&archive_dir(&title, "sid-A").unwrap()).len(), 1);
            assert_eq!(notices("heron").len(), 1, "heron is told");
        });
    }

    /// F12d: the notice goes to the session that sent the ping. When that session no longer holds the sender's title,
    /// the title's later holder never sent it and is not told; when base cannot say which session sent it, nobody is
    /// told. The archive keeps the ping either way.
    #[test]
    fn a_notice_reaches_the_sending_session_never_a_later_holder_of_its_title() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("bison", "sid-bison-1", home);
            bind("lynx", "sid-L1", home);
            let mut p = sample_ping("ping", "bison", "lynx", "sid-L1", "first bison's question");
            p.from_session = "sid-bison-1".into();
            enqueue(&ns, &p).unwrap();
            bind("bison", "sid-bison-2", home);
            bind("lynx", "sid-L2", home);
            assert_eq!(read_tasks_in(&archive_dir("lynx", "sid-L1").unwrap()).len(), 1, "archived");
            assert!(notices("bison").is_empty(), "the second bison never sent it, so it is not told");
            // A ping that does not say which session sent it (a script's --from) is placed by the title history: sent
            // while sid-bison-2 held bison, so sid-bison-2 is told.
            bind("lynx", "sid-L3", home);
            enqueue(&ns, &sample_ping("ping", "bison", "lynx", "sid-L3", "from a script")).unwrap();
            bind("lynx", "sid-L4", home);
            let told = notices("bison");
            assert_eq!(told.len(), 1, "{told:?}");
            assert_eq!(told[0].to_session, "sid-bison-2");
            // One from before bison's first history line (a binary from before BO-05): which session sent it cannot
            // be known, so nobody is told, and the archive keeps it.
            let mut legacy = sample_ping("ping", "bison", "lynx", "sid-L4", "from before the history");
            legacy.slug = "ping-legacy".into();
            legacy.created = hours_ago(3);
            enqueue(&ns, &legacy).unwrap();
            bind("lynx", "sid-L5", home);
            assert_eq!(read_tasks_in(&archive_dir("lynx", "sid-L4").unwrap()).len(), 1, "archived");
            assert_eq!(notices("bison").len(), 1, "no second notice: {:?}", notices("bison"));
        });
    }

    /// F12e at the unit level: an item that carries no session id is read as sent to whoever held its title when it
    /// was written, by the title history; the history's earliest line is as far back as that reaches.
    #[test]
    fn title_history_places_an_item_by_when_it_was_written() {
        with_home(|home| {
            bind("lynx", "sid-1", home);
            std::thread::sleep(std::time::Duration::from_millis(1100));
            bind("lynx", "sid-2", home);
            let h = registry::holders("lynx");
            assert_eq!(h.iter().map(|h| h.session_id.as_str()).collect::<Vec<_>>(), ["sid-1", "sid-2"]);
            let at = |s: &str| crate::relay::parse_ts(s).unwrap();
            assert_eq!(registry::holder_at("lynx", at(&h[0].since)).as_deref(), Some("sid-1"));
            assert_eq!(registry::holder_at("lynx", chrono::Local::now()).as_deref(), Some("sid-2"));
            assert_eq!(registry::holder_at("lynx", at(&h[0].since) - chrono::Duration::seconds(5)), None, "before the first line");
            assert_eq!(registry::held_since("lynx", "sid-2"), Some(h[1].since.clone()));
            // Re-registering the same session adds no line.
            bind("lynx", "sid-2", home);
            assert_eq!(registry::holders("lynx").len(), 2);
        });
    }

    /// A ping no session was ever shown that has waited past a day is archived and its sender told, never skipped
    /// silently (carried into BO-05 from BO-00's code review of `ping_is_stale`).
    #[test]
    fn a_never_delivered_stale_ping_is_archived_and_its_sender_told() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("heron", "sid-H", home);
            bind("lynx", "sid-L", home);
            let mut old = sample_ping("ping", "heron", "lynx", "sid-L", "a question from the day before");
            old.created = days_ago(2);
            old.from_session = "sid-H".into();
            enqueue(&ns, &old).unwrap();
            assert!(deliver("sid-L", Phase::Prompt).is_none(), "a stale ping is not shown");
            assert_eq!(read_tasks_in(&archive_dir("lynx", "sid-L").unwrap()).len(), 1, "it is archived");
            let told = notices("heron");
            assert_eq!(told.len(), 1);
            assert!(told[0].summary.contains("was not delivered: it waited more than a day, and session sid-L"), "{}", told[0].summary);
        });
    }

    /// Review finding 1: before BO-05, a session that took a title over was shown the previous holder's open task in
    /// full (BO-04), so on disk the task still names the previous holder while its listing belongs to the session that
    /// was shown it. After the upgrade that session keeps it, and it leaves with that session's inbox, not the first's.
    #[test]
    fn what_bo_04_showed_a_takeover_holder_stays_that_holders() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("caddy-backend", "sid-B", home);
            let mut shown_to_b = sample("sid-A");
            shown_to_b.status = "delivered".into();
            shown_to_b.last_loud_session = "sid-B".into();
            enqueue(&ns, &shown_to_b).unwrap();
            // And the other way round: a ping sent to sid-B that a session holding the title for a while was shown.
            let mut sent_to_b = sample_ping("ping", "heron", "caddy-backend", "sid-B", "sent to B, shown to X");
            sent_to_b.status = "delivered".into();
            sent_to_b.last_loud_session = "sid-X".into();
            enqueue(&ns, &sent_to_b).unwrap();
            let listed = deliver("sid-B", Phase::SessionStart).expect("sid-B still lists its open task");
            assert!(listed.contains("relay: 1 open task (rebuild-auth-guard from api-session)"), "{listed}");
            assert!(listed.contains("relay: 1 unanswered ping (heron "), "what was sent to sid-B stays with it:
{listed}");
            bind("caddy-backend", "sid-C", home);
            assert!(deliver("sid-C", Phase::SessionStart).is_none());
            assert_eq!(read_tasks_in(&archive_dir("caddy-backend", "sid-B").unwrap()).len(), 1, "the task, in sid-B's folder");
            assert_eq!(read_tasks_in(&archive_dir("caddy-backend", "sid-X").unwrap()).len(), 1, "the ping, with the session shown it");
        });
    }

    /// Review finding 3: a session that takes back a title it held gets back what it had been shown under it, for one
    /// after a headless run started from its terminal tab took the tab's title (F27, BO-08). What it was never shown
    /// stays archived: its sender was told it was not delivered.
    #[test]
    fn a_session_that_takes_its_title_back_gets_back_what_it_was_shown() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("heron", "sid-H", home);
            bind("otter", "sid-P", home);
            enqueue(&ns, &sample_ping("ping", "heron", "otter", "sid-P", "seen before the child ran")).unwrap();
            let mut task = sample("sid-P");
            task.to_title = "otter".into();
            enqueue(&ns, &task).unwrap();
            assert!(deliver("sid-P", Phase::Prompt).is_some(), "control: sid-P was shown both");
            let mut unseen = sample_ping("ping", "heron", "otter", "sid-P", "sent while the child held otter");
            unseen.slug = "ping-unseen".into();
            unseen.from_session = "sid-H".into();
            enqueue(&ns, &unseen).unwrap();

            bind("otter", "sid-child", home);
            assert_eq!(read_tasks_in(&archive_dir("otter", "sid-P").unwrap()).len(), 3, "all of sid-P's left");
            bind("otter", "sid-P", home);
            let back = deliver("sid-P", Phase::SessionStart).expect("its open task and ping are back");
            assert!(back.contains("relay: 1 open task (rebuild-auth-guard"), "{back}");
            assert!(back.contains("relay: 1 unanswered ping (heron "), "{back}");
            let still: Vec<String> =
                read_tasks_in(&archive_dir("otter", "sid-P").unwrap()).into_iter().map(|(_, t)| t.slug).collect();
            assert_eq!(still, vec!["ping-unseen".to_string()], "the unshown ping stays archived");
            assert_eq!(notices("heron").len(), 1, "and its sender was told once");
        });
    }

    /// Review finding 5: a delivery's write-back runs after the block is printed. If the title passed and the item was
    /// archived in between, the write-back does not put a copy back in the folder.
    #[test]
    fn a_late_write_back_never_restores_an_archived_item() {
        with_home(|home| {
            let ns = NamespaceConfig::default();
            bind("lynx", "sid-A", home);
            enqueue(&ns, &sample_ping("ping", "bison", "lynx", "sid-A", "shown to A")).unwrap();
            let part = deliver_deferred("sid-A", Phase::Prompt).expect("a block for A");
            bind("lynx", "sid-B", home);
            let _ = part.commit();
            assert!(read_tasks_in(&title_dir("lynx").unwrap()).is_empty(), "nothing was written back into the folder");
            assert_eq!(read_tasks_in(&archive_dir("lynx", "sid-A").unwrap()).len(), 1);
        });
    }

    /// Review findings 7 and 9: the history is seeded once, by a marker, so a heartbeat never reads it; seed lines are
    /// never shown as the time a session took a title; and compaction keeps each registered title's newest line and
    /// the recent lines only, leaving the file well under the cap.
    #[test]
    fn the_history_is_seeded_once_and_compacted_to_what_a_lookup_needs() {
        with_home(|home| {
            let dir = registry::global_base_dir().unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            let mut reg = SessionRegistry::default();
            reg.sessions.insert(
                "kite".into(),
                SessionEntry { title: "kite".into(), session_id: "sid-K".into(), last_heartbeat: hours_ago(0), ..Default::default() },
            );
            std::fs::write(dir.join("sessions.json"), serde_json::to_string(&reg).unwrap()).unwrap();
            bind("lynx", "sid-L", home);
            let kite = registry::holders("kite");
            assert_eq!(kite.len(), 1, "the title held since before the install got a seed line");
            assert!(kite[0].observed);
            assert_eq!(registry::held_since("kite", "sid-K"), None, "a seed line is not when sid-K took kite");
            assert!(dir.join("title-history.seeded").is_file());
            // Seeded once: after the marker, a registry write (here a heartbeat) never reads or writes the history.
            let path = dir.join("title-history.jsonl");
            let before = std::fs::read_to_string(&path).unwrap();
            registry::heartbeat("sid-L");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "a heartbeat does not touch the history");

            // Over the cap: 3,000 lines for titles nobody holds now, all from two months ago.
            let mut big = before.clone();
            let old = (chrono::Local::now() - chrono::Duration::days(60)).format("%Y-%m-%dT%H:%M:%S%z").to_string();
            for i in 0..3000 {
                big.push_str(&format!("{{\"title\":\"gone-{i}\",\"session_id\":\"sid-{i}-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\",\"since\":\"{old}\"}}\n"));
            }
            std::fs::write(&path, big).unwrap();
            assert!(std::fs::metadata(&path).unwrap().len() > 256 * 1024, "control: over the cap");
            bind("lynx", "sid-L2", home);
            let len = std::fs::metadata(&path).unwrap().len();
            assert!(len < 128 * 1024, "compacted well under the cap: {len} bytes");
            assert_eq!(registry::holder_at("lynx", chrono::Local::now()).as_deref(), Some("sid-L2"));
            assert_eq!(registry::holders("kite").len(), 1, "a registered title keeps its newest line");
            assert!(registry::holders("gone-1").is_empty(), "an old line for a title nobody holds is dropped");
        });
    }
}
