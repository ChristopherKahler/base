//! #174: an unattended `base sync --ast --yes` can never wait for ever.
//!
//! The Stop hook's refresh, first contact and the WSL delegate all run `sync --ast --yes`, which used to wait with no
//! limit for the extractor to exit and close its stderr. A hung worker, a stalled read, or a grandchild holding the pipe
//! kept it waiting at no CPU until someone killed it. Now the run stops at a time limit, takes everything it started
//! with it, leaves `.base-ast/.last-error` saying so, and the next session start shows that record even for an app that
//! already has a map.
//!
//! The stub extractor is real Python (CI's runners and this crate's AST tests already need it): it starts a grandchild
//! that sleeps and holds the inherited stderr, writes the grandchild's pid down, then sleeps itself.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base::hook::automap::{session_start_notice, STOPPED};

const BIN: &str = env!("CARGO_BIN_EXE_base");

const STUB: &str = r##"import subprocess, sys, time
from pathlib import Path
out = Path(sys.argv[sys.argv.index("--out") + 1])
out.parent.mkdir(parents=True, exist_ok=True)
print("# Extracting 1 files from somewhere", file=sys.stderr, flush=True)
child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(300)"])
(out.parent / "grandchild.pid").write_text(str(child.pid))
time.sleep(300)
"##;

/// The process with this pid is still running.
fn alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out = Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        return String::from_utf8_lossy(&out.stdout).split_whitespace().any(|w| w == pid.to_string());
    }
    // A killed process can sit as a zombie until it is reaped, and a zombie still answers `kill -0`: on Linux its
    // state in /proc is what says it has stopped running.
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        let state = stat.rsplit(')').next().and_then(|rest| rest.split_whitespace().next());
        return !matches!(state, Some("Z" | "X"));
    }
    if Path::new("/proc/self").exists() {
        return false;
    }
    Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

#[test]
fn unattended_ast_run_stops_at_its_limit() {
    let home = tempfile::tempdir().unwrap();
    let app = home.path().join("dev").join("app");
    std::fs::create_dir_all(app.join(".git")).unwrap();
    std::fs::write(app.join("a.py"), "def f():\n    pass\n").unwrap();
    // `base sync --ast` looks for the extractor under $BASE_HOME/.base-gbl/scripts/ast, then cwd/scripts/ast.
    let scripts = app.join("scripts").join("ast");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("onto_ast.py"), STUB).unwrap();
    let base_ast = app.join(".base-ast");
    std::fs::create_dir_all(&base_ast).unwrap();
    std::fs::write(base_ast.join(".building"), b"").unwrap();

    let started = Instant::now();
    let out = Command::new(BIN)
        .args(["sync", "--ast", "--yes", "--target"])
        .arg(&app)
        .current_dir(&app)
        .env("BASE_HOME", home.path())
        .env("BASE_AST_LIMIT_SECS", "3")
        .env("BASE_AST_SKIP_REGISTER", "1")
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env_remove("CLAUDECODE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let took = started.elapsed();
    let err = String::from_utf8_lossy(&out.stderr);

    assert!(took >= Duration::from_secs(3), "it waited for the limit, not less: {took:?}\n{err}");
    assert!(took < Duration::from_secs(60), "it ended at the limit, not when the 300 s sleep did: {took:?}\n{err}");
    assert!(err.contains("base stopped the code map build for "), "{err}");
    assert!(err.contains("after 3 seconds because it was stuck."), "{err}");

    let record = std::fs::read_to_string(base_ast.join(".last-error")).expect(".last-error written");
    let last = record.lines().rev().find(|l| !l.trim().is_empty()).unwrap();
    assert_eq!(last, "base stopped the code map build after 3 seconds because it was stuck.");
    assert!(last.starts_with(STOPPED));
    assert!(record.contains("# Extracting 1 files"), "what it printed before the limit is kept: {record}");
    assert!(!base_ast.join(".building").exists(), "the build lock is released");
    assert!(!base_ast.join("ast.ttl").exists(), "no map was written");

    // Everything it started went with it: the grandchild that held the pipe is gone.
    let pid: u32 = std::fs::read_to_string(base_ast.join("grandchild.pid")).unwrap().trim().parse().unwrap();
    let gone_by = Instant::now() + Duration::from_secs(10);
    while alive(pid) && Instant::now() < gone_by {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(!alive(pid), "grandchild {pid} outlived the stopped build");
}

/// The extractor writes its map and exits, but leaves a process holding its stderr: the build is done, so it ends in
/// seconds with the map kept, and the leftover is stopped, instead of waiting out that process or the limit.
#[test]
fn a_finished_build_does_not_wait_for_what_it_left_running() {
    const LEAVES_ONE: &str = r##"import subprocess, sys
from pathlib import Path
out = Path(sys.argv[sys.argv.index("--out") + 1])
out.parent.mkdir(parents=True, exist_ok=True)
child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(300)"])
(out.parent / "leftover.pid").write_text(str(child.pid))
out.write_text("# a map\n")
print("# Extracting 1 files from somewhere", file=sys.stderr, flush=True)
"##;
    let home = tempfile::tempdir().unwrap();
    let app = home.path().join("dev").join("app");
    std::fs::create_dir_all(app.join(".git")).unwrap();
    std::fs::write(app.join("a.py"), "def f():\n    pass\n").unwrap();
    let scripts = app.join("scripts").join("ast");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("onto_ast.py"), LEAVES_ONE).unwrap();

    let started = Instant::now();
    let out = Command::new(BIN)
        .args(["sync", "--ast", "--yes", "--target"])
        .arg(&app)
        .current_dir(&app)
        .env("BASE_HOME", home.path())
        .env("BASE_AST_LIMIT_SECS", "120")
        .env("BASE_AST_SKIP_REGISTER", "1")
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env_remove("CLAUDECODE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let took = started.elapsed();
    let err = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(took < Duration::from_secs(60), "it waited for the leftover: {took:?}\n{err}");
    assert!(stdout.contains("AST extraction complete"), "{stdout}\n{err}");
    let base_ast = app.join(".base-ast");
    assert!(base_ast.join("ast.ttl").is_file(), "the map it wrote is kept");
    assert!(!base_ast.join(".last-error").exists(), "a finished build is not a failure");
    let pid: u32 = std::fs::read_to_string(base_ast.join("leftover.pid")).unwrap().trim().parse().unwrap();
    let gone_by = Instant::now() + Duration::from_secs(10);
    while alive(pid) && Instant::now() < gone_by {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(!alive(pid), "the leftover {pid} was not stopped");
}

#[test]
fn a_stopped_refresh_shows_at_session_start_even_with_a_map() {
    // SAFETY: single-process test env; nothing here may spawn a build.
    unsafe { std::env::set_var("BASE_AST_NO_SPAWN", "1") };
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let app = home.join("dev").join("mapped");
    let base_ast = app.join(".base-ast");
    std::fs::create_dir_all(app.join(".git")).unwrap();
    std::fs::create_dir_all(&base_ast).unwrap();
    std::fs::write(app.join("a.py"), "def f():\n    pass\n").unwrap();
    std::fs::write(base_ast.join("ast.ttl"), "# a map\n").unwrap();
    std::fs::write(base_ast.join(".last-sync"), b"").unwrap();

    let line = |record: &str| {
        std::fs::write(base_ast.join(".last-error"), record).unwrap();
        base::home::with_thread_home(&home, || session_start_notice(&app))
    };

    // Control: any other refresh failure on a mapped app stays quiet, as before.
    assert_eq!(line("Traceback (most recent call last):\nValueError: boom\n"), None);

    let record = base::hook::automap::stopped_record(Duration::from_secs(600), b"worker output\n");
    assert_eq!(
        line(&record),
        Some(format!(
            "[AST] base stopped updating the code map for {} after 10 minutes because it was stuck. \
             Your old map is still there. It will try again after your next reply.",
            app.display()
        ))
    );
    // With no map yet, the same record says so.
    std::fs::remove_file(base_ast.join("ast.ttl")).unwrap();
    assert_eq!(
        line(&record),
        Some(format!(
            "[AST] base stopped building the code map for {} after 10 minutes because it was stuck. \
             There is no map yet. It will try again after your next reply.",
            app.display()
        ))
    );
}

#[test]
fn the_limit_is_ten_minutes_and_under_the_build_lock() {
    assert_eq!(base::hook::automap::UNATTENDED_LIMIT_SECS, 600);
    assert_eq!(
        base::hook::automap::stopped_record(Duration::from_secs(600), b""),
        "base stopped the code map build after 10 minutes because it was stuck.\n"
    );
    assert_eq!(base::hook::automap::spoken(Duration::from_secs(60)), "1 minute");
    assert_eq!(base::hook::automap::spoken(Duration::from_secs(90)), "90 seconds");
}

