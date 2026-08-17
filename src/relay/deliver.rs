use std::path::Path;

use super::{age_str, list_projects, relay_root, RelayStore};

// ─── Hook delivery (the push path) ───────────────────────────
//
// Session-start, prompt-submit AND pre-tool-use inject pending relay messages
// addressed to the current session. The mid-turn (tool) path exists because
// autonomous runs — Cadre members, PAUL workers — can go a whole milestone
// without hitting a prompt boundary; a question must interrupt the turn, not
// queue behind it. Polling is only for explicit `wait` gates — a session
// should never burn tokens checking an empty inbox.
//
// Identity: BASE_RELAY_AS env (Cadre Pulse contracts, routines) or registry
// binding by session id (interactive sessions that ran `base relay register`).
// Unregistered sessions get a one-line notice at session-start only, so an
// operator opening the workspace knows a relay is live without being spammed.

/// Collect and consume pending messages for the current session across all
/// relay stores in this workspace. Returns the injection block, if any.
/// `mid_turn` marks the pre-tool-use path: it skips the idle heartbeat write
/// (a locked registry mutation on every tool call would be pure contention)
/// and never emits the unregistered notice.
pub fn deliver(
    cwd: &Path,
    session_id: Option<&str>,
    notice_when_unregistered: bool,
    mid_turn: bool,
) -> Option<String> {
    let root = relay_root(cwd)?;
    let projects = list_projects(&root);
    if projects.is_empty() {
        return None;
    }

    let mut out = String::new();
    let mut unregistered: Vec<(String, usize, usize)> = Vec::new();

    for p in &projects {
        let store = RelayStore { root: root.join(p), project: p.clone() };
        match store.identity(session_id) {
            Some(title) => {
                let pending = store.pending_for(&title);
                if pending.is_empty() {
                    if !mid_turn {
                        store.heartbeat(&title);
                    }
                    continue;
                }
                store.heartbeat(&title);
                out.push_str(&format!("<relay project=\"{p}\" you=\"{title}\">\n"));
                let ids: Vec<String> = pending.iter().map(|m| m.id.clone()).collect();
                for m in &pending {
                    let refs = if m.refs.is_empty() {
                        String::new()
                    } else {
                        format!(" [refs: {}]", m.refs.join(", "))
                    };
                    out.push_str(&format!(
                        "[RELAY] from {} · {} · {} ago: {}{refs}\n",
                        m.from,
                        m.mtype,
                        age_str(&m.ts),
                        m.msg
                    ));
                }
                let needs_reply = pending.iter().any(|m| {
                    matches!(m.mtype.as_str(), "question" | "contract-change")
                });
                if needs_reply {
                    out.push_str(
                        "⚠️ REPLY REQUIRED NOW — before your next action, send: \
                         base relay send --to <sender> --type answer --msg \"...\" \
                         A one-line ack (\"on it\") counts if the full answer needs time. \
                         Never leave a question unanswered — the sender is blocked on you. \
                         Then resume your work.\n",
                    );
                }
                out.push_str("</relay>\n");
                let _ = store.mark_seen(&title, &ids);
            }
            None => {
                let reg = store.load_registry();
                let msgs = store.all_messages().len();
                unregistered.push((p.clone(), reg.sessions.len(), msgs));
            }
        }
    }

    if out.is_empty() && notice_when_unregistered && !unregistered.is_empty() {
        let (p, sessions, msgs) = &unregistered[0];
        out.push_str(&format!(
            "<relay-notice>Relay store '{p}' active ({sessions} sessions, {msgs} messages). \
             Join with: base relay register --as <title> · view: base relay board</relay-notice>\n"
        ));
    }

    (!out.is_empty()).then_some(out)
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
        let _lock = ENV_LOCK.lock().unwrap();
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

        let block = deliver(tmp.path(), Some("sess-abc"), true, false).expect("delivery expected");
        assert!(block.contains("you=\"quill\""));
        assert!(block.contains("3 units queued for you"));

        // Consumed — second delivery is empty (registered → no notice either).
        assert!(deliver(tmp.path(), Some("sess-abc"), true, false).is_none());
    }

    #[test]
    fn mid_turn_delivers_questions_between_tool_calls() {
        let _guard = env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(tmp.path());
        s.register("worker", Some("sess-w"), "/wt", None).unwrap();
        s.send("orch", "worker", "question", "which schema version?", &[]).unwrap();

        // Pre-tool-use path: the question interrupts the running turn.
        let block = deliver(tmp.path(), Some("sess-w"), false, true).expect("mid-turn delivery");
        assert!(block.contains("which schema version?"));
        assert!(block.contains("REPLY REQUIRED NOW"));

        // Empty inbox mid-turn: silent, no notice.
        assert!(deliver(tmp.path(), Some("sess-w"), false, true).is_none());
    }

    #[test]
    fn question_prompts_reply_instruction() {
        let _guard = env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(tmp.path());
        s.register("orch", Some("sess-1"), "/main", None).unwrap();
        s.send("worker", "orch", "contract-change", "need col rename", &[]).unwrap();

        let block = deliver(tmp.path(), Some("sess-1"), true, false).unwrap();
        assert!(block.contains("base relay send --to <sender> --type answer"));
        assert!(block.contains("REPLY REQUIRED NOW"));
    }

    #[test]
    fn unregistered_session_gets_notice_only_when_asked() {
        let _guard = env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(tmp.path());
        s.send("a", "b", "notify", "x", &[]).unwrap();

        // session-start path: notice.
        let block = deliver(tmp.path(), Some("unknown-sess"), true, false).unwrap();
        assert!(block.contains("<relay-notice>"));

        // prompt-submit path: silent.
        assert!(deliver(tmp.path(), Some("unknown-sess"), false, false).is_none());
    }

    #[test]
    fn env_identity_overrides_registry() {
        let _guard = env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let s = setup(tmp.path());
        s.send("sterling", "quill", "notify", "for the env-bound member", &[]).unwrap();

        // SAFETY: test-local env mutation; no parallel test reads this var.
        unsafe { std::env::set_var("BASE_RELAY_AS", "quill") };
        let block = deliver(tmp.path(), None, false, false);
        unsafe { std::env::remove_var("BASE_RELAY_AS") };

        let block = block.expect("env identity should receive delivery");
        assert!(block.contains("for the env-bound member"));
    }
}
