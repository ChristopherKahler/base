//! R7 — a real graph write reaches a real listener.
//!
//! Unix only: the Windows half is a named pipe, which has no portable stand-in
//! here. Its contract (attempt the open, never probe for existence, absent
//! listener is a no-op) is held by the same `ring` seam and verified on the
//! MSVC run.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

use base::config::NamespaceConfig;
use base::crud;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// Accept, or fail loudly — never block forever.
///
/// A bare `accept()` here hangs when base does not ring, and a hung test is
/// strictly worse than a failing one: it reports nothing, and in CI it burns
/// the whole timeout instead of naming the regression. Found by red-checking
/// this file with the `ring` call removed.
fn accept_within(listener: &UnixListener, budget: Duration) -> UnixStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + budget;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(budget)).unwrap();
                return stream;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "base never rang the doorbell within {budget:?}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("accept failed: {e}"),
        }
    }
}

/// How long a test waits for a poke that should already have happened.
const WAIT: Duration = Duration::from_secs(5);

/// A workspace under an isolated home, with the app's address published.
fn publish(home: &Path, addr: &Path) -> std::path::PathBuf {
    let ws = home.join("proj");
    std::fs::create_dir_all(ws.join(".base")).unwrap();
    let tier = home.join(".base-gbl");
    std::fs::create_dir_all(&tier).unwrap();
    std::fs::write(tier.join("doorbell"), addr.display().to_string()).unwrap();
    ws
}

#[test]
fn a_graph_write_rings_a_listening_app() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = tmp.path().join("app.sock");
    // Bound before the write: the connection lands in the backlog, so this
    // needs no accept-racing thread to be deterministic.
    let listener = UnixListener::bind(&addr).unwrap();

    let graph = base::home::with_thread_home(tmp.path(), || {
        let ws = publish(tmp.path(), &addr);
        crud::note::learn(&ws, &ns(), "wake the app", "insight", None, None, None).unwrap();
        ws.join(".base").join("graph.nq")
    });

    let stream = accept_within(&listener, WAIT);
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();

    let v: serde_json::Value = serde_json::from_str(line.trim()).expect("the poke is one JSON line");
    assert_eq!(v["v"], serde_json::json!(1));
    assert_eq!(v["event"], serde_json::json!("graph-write"));
    assert_eq!(v["tier"], serde_json::json!("workspace"));
    assert_eq!(v["path"], serde_json::json!(graph.display().to_string()));
}

#[test]
fn a_global_tier_write_says_global() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = tmp.path().join("app.sock");
    let listener = UnixListener::bind(&addr).unwrap();

    base::home::with_thread_home(tmp.path(), || {
        publish(tmp.path(), &addr);
        // `-g` resolves the global tier; the doorbell must name it as such or an
        // app cannot tell which store to re-read.
        let tier_root = tmp.path().join(".base-gbl");
        crud::note::learn(&tier_root, &ns(), "global write", "insight", None, None, None).unwrap();
    });

    let stream = accept_within(&listener, WAIT);
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["tier"], serde_json::json!("global"));
}

/// The app crashed and left its address behind. Every base command must keep
/// working, and keep working *promptly*.
#[test]
fn a_dead_listener_does_not_fail_or_stall_the_write() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = tmp.path().join("app.sock");
    // Bind then drop + unlink: the address file survives, the listener does not.
    drop(UnixListener::bind(&addr).unwrap());
    std::fs::remove_file(&addr).unwrap();

    base::home::with_thread_home(tmp.path(), || {
        let ws = publish(tmp.path(), &addr);
        let t = Instant::now();
        crud::note::learn(&ws, &ns(), "nobody home", "insight", None, None, None)
            .expect("a stale doorbell must never fail the user's command");
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "a dead listener must not stall the write: took {:?}",
            t.elapsed()
        );

        // The write itself still landed.
        let graph = std::fs::read_to_string(ws.join(".base").join("graph.nq")).unwrap();
        assert!(graph.contains("nobody home"));
    });
}

/// A listener that accepts and then never reads must not hold a write hostage.
#[test]
fn a_listener_that_never_reads_does_not_hold_the_write_hostage() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = tmp.path().join("app.sock");
    let _listener = UnixListener::bind(&addr).unwrap(); // bound, never accepts

    base::home::with_thread_home(tmp.path(), || {
        let ws = publish(tmp.path(), &addr);
        let t = Instant::now();
        crud::note::learn(&ws, &ns(), "silent listener", "insight", None, None, None).unwrap();
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "the 50ms write budget must bound this: took {:?}",
            t.elapsed()
        );
    });
}
