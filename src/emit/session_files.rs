//! Each session's own hook-output files (BO-06, F11), in `.base/hook-output/<session id>/` beside the prompt hook's
//! `prompt-blocks.json` (BO-01):
//!
//! - `session-start.md`: the session's untrimmed session start, the file its header names on line 1;
//! - `prompt-submit.md`: the untrimmed output of the session's latest prompt;
//! - `letters.json`: the handoff letters and DUE NOW numbers its session start printed, which `base handoff show <letter>`
//!   and `base reminder archive|snooze <number>` read back inside that session.
//!
//! WHY. Until BO-06 each of these was one file per workspace, rewritten by whichever session ran last. On 2026-10-01
//! six sessions wrote `last-session-start.md`; when lynx opened it at about 14:15 it held robin's start (session
//! 1bf80fa9, 14:01), not lynx's (12:57), and the line telling the reader "never guess; run it" sent it there. The
//! letters file did the same to `base handoff show A`: one session's A was whatever the last session start lettered A.
//!
//! The workspace files `last-session-start.md`, `last-prompt-submit.md` and `last-session-start-letters.json` are still
//! written (F11c), as the latest of any session, for a reader that has no session: a terminal, `base doctor`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::FullOutput;

/// A session's untrimmed session start.
pub const SESSION_START_FILE: &str = "session-start.md";
/// The untrimmed output of a session's latest prompt.
pub const PROMPT_FILE: &str = "prompt-submit.md";
/// The letters and DUE NOW numbers a session's start printed.
pub const LETTERS_FILE: &str = "letters.json";
/// The workspace's latest-of-any-session copy of [`SESSION_START_FILE`] (F11c).
pub const LATEST_SESSION_START: &str = "last-session-start.md";

/// Days a legacy `.base/due-now/<session>.json` is kept, as its writer kept it until BO-06 merged those numbers into
/// [`LETTERS_FILE`]. Nothing writes the folder now; this only clears what an earlier build left.
const LEGACY_DUE_NOW_KEEP_DAYS: u64 = 7;

/// `<base>/hook-output/<session id>/`, or `None` when the id cannot be a folder name: Claude Code's ids are UUIDs, and
/// anything else arriving on a hook's stdin is refused rather than joined into a path.
pub fn session_dir(base: &Path, session_id: &str) -> Option<PathBuf> {
    crate::crud::handoff_show::is_file_safe_session_id(session_id)
        .then(|| base.join(super::prompt::SESSION_DIR).join(session_id))
}

/// Write `text` as this session's `name` and as the workspace's latest copy `latest`. The outcome is the one the hook
/// names: the session's own file when the host named a session with a usable id and it was written, else the latest
/// copy. Never panics; a failure comes back as a value carrying its reason.
pub fn write(base: &Path, session: Option<&str>, name: &str, latest: &str, text: &str) -> FullOutput {
    let own = session
        .and_then(|s| session_dir(base, s))
        .map(|dir| super::write_full_output(&dir.join(name), text));
    let shared = super::write_full_output(&base.join(latest), text);
    match own {
        Some(own) if own.written_path().is_some() || shared.written_path().is_none() => own,
        _ => shared,
    }
}

/// When a session folder was last written: the newest modification time of the files in it, or the folder's own when
/// it holds none. Every file here is written through a temp file and a rename, so a file's time is its last write.
fn last_written(dir: &Path) -> Option<SystemTime> {
    let newest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.metadata().and_then(|m| m.modified()).ok())
        .max();
    newest.or_else(|| std::fs::metadata(dir).and_then(|m| m.modified()).ok())
}

fn older_than(t: Option<SystemTime>, keep: Duration) -> bool {
    t.and_then(|t| t.elapsed().ok()).is_some_and(|age| age > keep)
}

/// Remove every session folder under `<base>/hook-output/` not written for `days` days (F11e: `[log] prompt_days`,
/// read as at least 1), and the legacy `<base>/due-now/*.json` files past their seven days. Only folders named like a
/// session id are touched, and never through a link. Returns how many session folders were removed. Fail-open: a
/// folder that cannot be read or removed stays, and session start goes on.
pub fn prune(base: &Path, days: u64) -> usize {
    let keep = Duration::from_secs(days.max(1) * 24 * 60 * 60);
    let mut removed = 0;
    if let Ok(entries) = std::fs::read_dir(base.join(super::prompt::SESSION_DIR)) {
        for entry in entries.flatten() {
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            let named = entry
                .file_name()
                .to_str()
                .is_some_and(crate::crud::handoff_show::is_file_safe_session_id);
            if is_dir && named && older_than(last_written(&entry.path()), keep)
                && std::fs::remove_dir_all(entry.path()).is_ok()
            {
                removed += 1;
            }
        }
    }
    let legacy = base.join(crate::crud::handoff_show::DUE_NOW_DIR);
    if let Ok(entries) = std::fs::read_dir(&legacy) {
        let keep = Duration::from_secs(LEGACY_DUE_NOW_KEEP_DAYS * 24 * 60 * 60);
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|x| x == "json")
                && older_than(entry.metadata().and_then(|m| m.modified()).ok(), keep)
            {
                let _ = std::fs::remove_file(path);
            }
        }
        // Removed only once empty: a file this build does not know is left alone.
        let _ = std::fs::remove_dir(&legacy);
    }
    removed
}
