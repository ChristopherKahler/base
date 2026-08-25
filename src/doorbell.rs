//! Doorbell — tell a running desktop app that a graph write landed.
//!
//! base is the only writer of `graph.nq`, so an app that wants to be current
//! either polls or gets told. This tells it.
//!
//! **Not a sync gate.** `<home>/.base-gbl/sync-enabled` says whether deltas are
//! captured *at all*; this file says where to poke *right now*. An app that is
//! paused or crashed still owes deltas — collapsing the two would silently drop
//! writes that cannot be reconstructed afterwards. Two files, two jobs.
//!
//! **Not network code.** A Unix domain socket and a Windows named pipe are IPC:
//! no TCP, no HTTP, no DNS, and no new dependency.
//!
//! Cost when no app is running: **715 ns per write** (median of 1000, release)
//! — one failed `open()`. Unlike the sync gate there is no marker to switch this
//! off, so every user pays it on every write and it is measured rather than
//! assumed. See `doorbell::cost`.
//!
//! The app writes its address here at startup and removes it on clean exit.
//! base never enumerates and never guesses a name — if the file is absent,
//! unreadable or empty, there is nobody to tell and that is not an error.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The longest base will spend telling the app. A write must never wait on a
/// listener that has stopped reading.
const BUDGET: Duration = Duration::from_millis(50);

/// Where the app publishes the socket/pipe it is listening on — one line.
pub fn address_file() -> Option<PathBuf> {
    crate::home::home_root().map(|h| h.join(".base-gbl").join("doorbell"))
}

/// The address the app published, if any.
fn address() -> Option<String> {
    let text = std::fs::read_to_string(address_file()?).ok()?;
    let line = text.lines().next()?.trim().to_string();
    (!line.is_empty()).then_some(line)
}

/// What base tells the app. The tier is derived the same way the change log
/// derives it, from the resolved path, so the two can never disagree.
pub fn payload(graph_path: &Path) -> String {
    let tier = if crate::changelog::ws_slug(graph_path) == "base-gbl" {
        "global"
    } else {
        "workspace"
    };
    serde_json::json!({
        "v": 1,
        "event": "graph-write",
        "tier": tier,
        "path": graph_path.display().to_string(),
    })
    .to_string()
}

/// Poke the app, if one is listening.
///
/// Best-effort by contract, like [`crate::changelog::append`]: returns `()`,
/// never fails the user's command. A missed doorbell costs the app one stale
/// render until its next poll; a failed `base learn` costs the user their work.
pub fn ring(graph_path: &Path) {
    let Some(addr) = address() else { return };
    let _ = poke(&addr, &payload(graph_path));
}

#[cfg(unix)]
fn poke(addr: &str, payload: &str) -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;

    // Connect fails immediately when nothing is bound (ENOENT / ECONNREFUSED),
    // so the absent-listener path costs a syscall. The timeout is for the other
    // case: a listener that accepted and then stopped reading.
    let mut sock = UnixStream::connect(addr)?;
    sock.set_write_timeout(Some(BUDGET))?;
    sock.write_all(payload.as_bytes())?;
    sock.write_all(b"\n")
}

#[cfg(windows)]
fn poke(addr: &str, payload: &str) -> std::io::Result<()> {
    // No existence probe first: `Path::exists()` on a live named pipe lies.
    // Attempt the open and let "nobody home" arrive as an error — which is the
    // no-op path, not a failure. A busy server returns ERROR_PIPE_BUSY straight
    // away rather than blocking, because blocking would require an explicit
    // WaitNamedPipe this deliberately does not call.
    //
    // std exposes no write timeout for a File, so the BUDGET is not enforced on
    // this side: the app's pipe server must read promptly. Documented rather
    // than papered over with a thread that could outlive the process.
    let mut pipe = std::fs::OpenOptions::new().write(true).open(addr)?;
    pipe.write_all(payload.as_bytes())?;
    pipe.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_payload_names_the_tier_the_change_log_would_name() {
        let global = Path::new("/home/u/.base-gbl/.base/graph.nq");
        let v: serde_json::Value = serde_json::from_str(&payload(global)).unwrap();
        assert_eq!(v["v"], serde_json::json!(1));
        assert_eq!(v["event"], serde_json::json!("graph-write"));
        assert_eq!(v["tier"], serde_json::json!("global"));
        assert_eq!(v["path"], serde_json::json!("/home/u/.base-gbl/.base/graph.nq"));

        let ws = Path::new("/home/u/proj/.base/graph.nq");
        let v: serde_json::Value = serde_json::from_str(&payload(ws)).unwrap();
        assert_eq!(v["tier"], serde_json::json!("workspace"));
    }

    #[test]
    fn no_address_file_is_silence_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            assert!(address().is_none(), "nothing published, nothing to read");
            ring(Path::new("/anywhere/.base/graph.nq")); // must not panic
        });
    }

    #[test]
    fn an_empty_or_blank_address_file_is_also_silence() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let dir = tmp.path().join(".base-gbl");
            std::fs::create_dir_all(&dir).unwrap();
            for junk in ["", "\n", "   \n"] {
                std::fs::write(dir.join("doorbell"), junk).unwrap();
                assert!(address().is_none(), "blank address must not be dialled: {junk:?}");
                ring(Path::new("/anywhere/.base/graph.nq"));
            }
        });
    }

    /// The case that must never fail a command: the app died without cleaning up.
    #[test]
    fn a_stale_address_pointing_at_nothing_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let dir = tmp.path().join(".base-gbl");
            std::fs::create_dir_all(&dir).unwrap();
            let dead = tmp.path().join("no-such.sock");
            std::fs::write(dir.join("doorbell"), dead.display().to_string()).unwrap();

            assert!(address().is_some(), "the stale file still parses");
            ring(Path::new("/anywhere/.base/graph.nq")); // must not panic or hang
        });
    }
}

#[cfg(test)]
mod cost {
    use super::*;

    /// Unlike the sync gate, this cost is paid by EVERY user on EVERY write —
    /// there is no marker to switch it off — so it is measured, not assumed.
    ///
    /// `cargo test --release --lib -- --ignored --nocapture doorbell::cost`
    #[test]
    #[ignore = "benchmark"]
    fn bench_absent_app_overhead_per_write() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let graph = Path::new("/w/.base/graph.nq");
            let mut v: Vec<u128> = (0..1000)
                .map(|_| {
                    let t = std::time::Instant::now();
                    ring(graph);
                    t.elapsed().as_nanos()
                })
                .collect();
            v.sort_unstable();
            println!("\n  doorbell, no app listening: {} ns/write (median of 1000)\n", v[500]);
        });
    }
}
