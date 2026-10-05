use std::path::Path;

use super::{list_projects, relay_root, RelayStore};

// ─── Hook delivery (the push path) ───────────────────────────
//
// Session start and prompt-submit inject pending relay messages addressed to the
// current session. Never pre-tool (BO-04, F13b): relay text on tool calls competed
// with the user's own prompt, and a session that must hear a message mid-turn runs
// the inbox watcher (`base relay arm`), whose Monitor wakes it. Polling is only for
// explicit `wait` gates — a session should never burn tokens checking an empty inbox.
//
// Identity: BASE_RELAY_AS env (Cadre Pulse contracts, routines) or registry
// binding by session id (interactive sessions that ran `base relay register`).
// Unregistered sessions get a one-line notice at session-start only, so an
// operator opening the workspace knows a relay is live without being spammed.

/// How the unregistered notice opens. Session start ranks the notice apart from delivered
/// messages: a message for this session is due now, an invitation to join is not (BO-00 B4).
pub const NOTICE_OPEN: &str = "<relay-notice>";

/// True when `block`, as [`deliver`] returned it, is the unregistered notice rather than messages.
/// The two never share a block: the notice is written only when no message was.
pub fn is_notice(block: &str) -> bool {
    block.starts_with(NOTICE_OPEN)
}

/// Collect and consume pending messages for the current session across all
/// relay stores in this workspace. Returns the injection block, if any.
pub fn deliver(cwd: &Path, session_id: Option<&str>, notice_when_unregistered: bool) -> Option<String> {
    deliver_deferred(cwd, session_id, notice_when_unregistered).map(super::Part::commit)
}

/// [`deliver`] on a tool call, for a run with no inbox watcher ([`super::monitorless`]): new messages only, no join
/// notice, and no liveness write (a locked registry write on every tool call would be pure contention).
pub fn deliver_mid_turn(cwd: &Path, session_id: Option<&str>) -> Option<String> {
    collect(cwd, session_id, false, false).map(super::Part::commit)
}

/// [`deliver`] with the messages NOT yet marked seen: the marks come back as commits, for the prompt hook to run only
/// if it prints the block (BO-01). The liveness heartbeat is not held back; it says the session is alive, not that it
/// read anything.
///
/// BO-04 (F13): each message is shown as information, once: a header (store, type, sender, time, and for a question
/// the answer command), then the message on its own line. Of one sender's unseen messages only the newest is shown; the
/// older ones are marked seen with it, and a line names the command that still lists them. A message whose wake notify
/// is still in this session's inbox is left to that notify, so one message is never shown twice.
pub fn deliver_deferred(cwd: &Path, session_id: Option<&str>, notice_when_unregistered: bool) -> Option<super::Part> {
    collect(cwd, session_id, notice_when_unregistered, true)
}

fn collect(
    cwd: &Path,
    session_id: Option<&str>,
    notice_when_unregistered: bool,
    heartbeat: bool,
) -> Option<super::Part> {
    let root = relay_root(cwd)?;
    let projects = list_projects(&root);
    if projects.is_empty() {
        return None;
    }

    let mut text = String::new();
    let mut items = 0usize;
    let mut commits: Vec<super::Commit> = Vec::new();
    let mut unregistered: Vec<(String, usize, usize)> = Vec::new();

    for p in &projects {
        let store = RelayStore { root: root.join(p), project: p.clone() };
        match store.identity(session_id) {
            Some(title) => {
                if heartbeat {
                    store.heartbeat(&title);
                }
                let pending: Vec<_> = store
                    .pending_for(&title)
                    .into_iter()
                    .filter(|m| !super::task_inbox::has_notify(&title, &m.id))
                    .collect();
                if pending.is_empty() {
                    continue;
                }
                // `pending_for` is oldest first, and every message here is to this one title.
                let keys: Vec<Option<&str>> =
                    pending.iter().map(|m| (!super::threadless(&m.from)).then_some(m.from.as_str())).collect();
                let behind = super::superseded_by(&keys);
                for (i, m) in pending.iter().enumerate() {
                    if behind[i].is_some() {
                        continue;
                    }
                    let answer = super::task_inbox::answer_command(p, &m.mtype, &m.from);
                    text.push_str(&format!(
                        "relay ({p}): {} from {} ({}){answer}\n{}\n",
                        m.mtype,
                        m.from,
                        super::clock(&m.ts),
                        m.msg
                    ));
                    if !m.refs.is_empty() {
                        text.push_str(&format!("refs: {}\n", m.refs.join(", ")));
                    }
                    let earlier = behind.iter().filter(|b| **b == Some(i)).count();
                    if earlier > 0 {
                        let command = format!("base relay poll --project {p} --peek --all --from {}", m.from);
                        text.push_str(&super::hidden_line(earlier, &m.from, &command));
                    }
                    items += 1;
                }
                let ids: Vec<String> = pending.iter().map(|m| m.id.clone()).collect();
                commits.push(Box::new(move || {
                    let _ = store.mark_seen(&title, &ids);
                }));
            }
            None => {
                let reg = store.load_registry();
                let msgs = store.all_messages().len();
                unregistered.push((p.clone(), reg.sessions.len(), msgs));
            }
        }
    }

    if text.is_empty() && notice_when_unregistered && !unregistered.is_empty() {
        let (p, sessions, msgs) = &unregistered[0];
        text.push_str(&format!(
            "{NOTICE_OPEN}Relay store '{p}' active ({sessions} sessions, {msgs} messages). \
             Join with: base relay register --as <title> · view: base relay board</relay-notice>\n"
        ));
    }

    (!text.is_empty()).then_some(super::Part { text, commits, items })
}

/// A reply to `peer` answers everything `peer` sent before it (BO-04, F13c): every unseen spool message from `peer` to
/// one of `my_titles`, sent no later than `cutoff` (the newest ping being answered), in every store of this workspace,
/// is marked seen. Returns how many.
pub fn mark_answered(
    cwd: &Path,
    peer: &str,
    my_titles: &[String],
    cutoff: chrono::DateTime<chrono::Local>,
) -> usize {
    let Some(root) = relay_root(cwd) else { return 0 };
    let mut marked = 0;
    for p in list_projects(&root) {
        let store = RelayStore { root: root.join(&p), project: p };
        for title in my_titles {
            let ids: Vec<String> = store
                .pending_for(title)
                .into_iter()
                .filter(|m| m.from == peer && super::parse_ts(&m.ts).is_some_and(|t| t <= cutoff))
                .map(|m| m.id)
                .collect();
            if !ids.is_empty() && store.mark_seen(title, &ids).is_ok() {
                marked += ids.len();
            }
        }
    }
    marked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::RelayStore;

    /// Tests that set or depend on the absence of BASE_RELAY_AS serialize
    /// through this — cargo test is multi-threaded and env is process-global.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Holds ENV_LOCK *and* scrubs BASE_RELAY_AS for the test's duration,
    /// restoring the shell's value on drop.
    ///
    /// Serializing alone was never enough: the lock stopped tests from racing
    /// each other but nothing cleared what the *shell* exported. A
    /// wrapper-launched session (`cc work` sets BASE_RELAY_AS) made `deliver()`
    /// resolve identity from the env instead of the registry binding, so every
    /// message addressed to a registered title looked undeliverable.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: Option<std::ffi::OsString>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: ENV_LOCK is still held — no other test reads this var.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("BASE_RELAY_AS", v),
                    None => std::env::remove_var("BASE_RELAY_AS"),
                }
            }
        }
    }

    fn env_guard() -> EnvGuard {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Before capturing `prev`, so a failing run cannot restore the shell's
        // contaminated value into a sibling module's tests.
        crate::relay::scrub_shell_env();
        let prev = std::env::var_os("BASE_RELAY_AS");
        // SAFETY: guarded by ENV_LOCK — no other test reads this var concurrently.
        unsafe { std::env::remove_var("BASE_RELAY_AS") };
        EnvGuard { _lock, prev }
    }

    fn setup(tmp: &Path) -> RelayStore {
        let base = tmp.join(".base");
        let s = RelayStore {
            root: base.join("relay").join("proj"),
            project: "proj".into(),
        };
        s.init().unwrap();
        s
    }

    #[test]
    fn delivers_to_registered_session_and_consumes() {
        let _guard = env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(tmp.path());
        s.register("quill", Some("sess-abc"), "/firm", None).unwrap();
        s.send("sterling", "quill", "notify", "3 units queued for you", &[]).unwrap();

        let block = deliver(tmp.path(), Some("sess-abc"), true).expect("delivery expected");
        assert!(block.starts_with("relay (proj): notify from sterling ("), "{block}");
        assert!(block.contains("3 units queued for you"));

        // Consumed — second delivery is empty (registered → no notice either).
        assert!(deliver(tmp.path(), Some("sess-abc"), true).is_none());
    }

    /// BO-04, F13c with lynx's conditions: of one sender's unseen messages only the newest is shown, the line under it
    /// names the command that still lists the older ones, nothing is deleted, and a second delivery shows nothing.
    #[test]
    fn the_newest_message_per_sender_is_shown_and_the_older_ones_listed() {
        let _guard = env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(tmp.path());
        s.register("lynx", Some("sess-l"), "/main", None).unwrap();
        s.send("bison", "lynx", "answer", "go: edit the doc", &[]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.send("heron", "lynx", "notify", "unrelated news", &[]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.send("bison", "lynx", "answer", "stand down, the go is withdrawn", &[]).unwrap();

        let block = deliver(tmp.path(), Some("sess-l"), false).expect("delivery");
        assert!(block.contains("stand down, the go is withdrawn"), "{block}");
        assert!(!block.contains("go: edit the doc"), "a superseded message was shown:\n{block}");
        assert!(block.contains("unrelated news"), "another sender's message is not superseded:\n{block}");
        assert!(
            block.contains("(1 earlier message from bison hidden, this one is newer: base relay poll --project proj --peek --all --from bison)"),
            "{block}"
        );
        assert_eq!(s.all_messages().len(), 3, "superseding hides, never deletes");
        assert!(deliver(tmp.path(), Some("sess-l"), false).is_none(), "a delivered message is never shown again");
    }

    #[test]
    fn question_prompts_reply_instruction() {
        let _guard = env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(tmp.path());
        s.register("orch", Some("sess-1"), "/main", None).unwrap();
        s.send("worker", "orch", "contract-change", "need col rename", &[]).unwrap();

        let block = deliver(tmp.path(), Some("sess-1"), true).unwrap();
        assert!(block.contains("answer: base relay send --project proj --to worker --type answer"), "{block}");
        assert!(!block.contains("REPLY REQUIRED"), "relay text never claims priority over the user:\n{block}");
        assert!(block.lines().any(|l| l == "need col rename"), "the message starts its own line:\n{block}");
    }

    /// F13c condition 3: a reply to a sender marks every earlier message from that sender seen.
    #[test]
    fn a_reply_marks_everything_earlier_from_that_sender_seen() {
        let _guard = env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(tmp.path());
        s.register("lynx", Some("sess-r"), "/main", None).unwrap();
        s.send("bison", "lynx", "answer", "go: edit the doc", &[]).unwrap();
        s.send("heron", "lynx", "notify", "from someone else", &[]).unwrap();
        assert_eq!(mark_answered(tmp.path(), "bison", &["lynx".to_string()], chrono::Local::now()), 1);
        let block = deliver(tmp.path(), Some("sess-r"), false).expect("heron's message is still due");
        assert!(!block.contains("go: edit the doc"), "{block}");
        assert!(block.contains("from someone else"), "{block}");
    }

    #[test]
    fn unregistered_session_gets_notice_only_when_asked() {
        let _guard = env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(tmp.path());
        s.send("a", "b", "notify", "x", &[]).unwrap();

        // session-start path: notice.
        let block = deliver(tmp.path(), Some("unknown-sess"), true).unwrap();
        assert!(block.contains("<relay-notice>"));

        // prompt-submit path: silent.
        assert!(deliver(tmp.path(), Some("unknown-sess"), false).is_none());
    }

    #[test]
    fn env_identity_overrides_registry() {
        let _guard = env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(tmp.path());
        s.send("sterling", "quill", "notify", "for the env-bound member", &[]).unwrap();

        // SAFETY: test-local env mutation; no parallel test reads this var.
        unsafe { std::env::set_var("BASE_RELAY_AS", "quill") };
        let block = deliver(tmp.path(), None, false);
        unsafe { std::env::remove_var("BASE_RELAY_AS") };

        let block = block.expect("env identity should receive delivery");
        assert!(block.contains("for the env-bound member"));
    }
}
