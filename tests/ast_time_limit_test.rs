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

/// `base sync --ast --yes` on `app` with this limit, its output sent to files: what is timed is base's own exit, not
/// how long something it started keeps the caller's pipes open (every unattended caller gives it no pipes at all).
fn sync_ast(app: &std::path::Path, home: &std::path::Path, limit_secs: &str) -> (Duration, String, String) {
    let logs = tempfile::tempdir().unwrap();
    let (out_path, err_path) = (logs.path().join("out"), logs.path().join("err"));
    let started = Instant::now();
    Command::new(BIN)
        .args(["sync", "--ast", "--yes", "--target"])
        .arg(app)
        .current_dir(app)
        .env("BASE_HOME", home)
        .env("BASE_AST_LIMIT_SECS", limit_secs)
        .env("BASE_AST_SKIP_REGISTER", "1")
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env_remove("CLAUDECODE")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&out_path).unwrap())
        .stderr(std::fs::File::create(&err_path).unwrap())
        .status()
        .unwrap();
    let took = started.elapsed();
    let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap_or_default();
    (took, read(&out_path), read(&err_path))
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

    let (took, _, err) = sync_ast(&app, home.path(), "3");

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

    let (took, stdout, err) = sync_ast(&app, home.path(), "120");
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
    // A build ran within the debounce window, so session start starts none: the Stop hook is the next to try.
    assert_eq!(
        line(&record),
        Some(format!(
            "[AST] base stopped updating the code map for {} after 10 minutes because it was stuck. \
             Your old map is still there. It will try again the next time Claude finishes a reply. To build it with no time limit, run `base sync --ast --yes --no-time-limit --target {}`.",
            app.display(),
            app.display()
        ))
    );
    // Past the window and with no build running, session start has already started the next one when it says so.
    let age_out = || {
        let _ = std::fs::remove_file(base_ast.join(".building"));
        let old = std::time::SystemTime::now() - Duration::from_secs(120);
        std::fs::File::options().write(true).open(base_ast.join(".last-sync")).unwrap().set_modified(old).unwrap();
    };
    age_out();
    assert_eq!(
        line(&record),
        Some(format!(
            "[AST] base stopped updating the code map for {} after 10 minutes because it was stuck. \
             Your old map is still there. It is trying again now, in the background. To build it with no time limit, run `base sync --ast --yes --no-time-limit --target {}`.",
            app.display(),
            app.display()
        ))
    );
    // A build already running (its `.building` is fresh) is said as such.
    age_out();
    std::fs::write(base_ast.join(".building"), b"").unwrap();
    assert_eq!(
        line(&record),
        Some(format!(
            "[AST] base stopped updating the code map for {} after 10 minutes because it was stuck. \
             Your old map is still there. Another build is running now. To build it with no time limit, run `base sync --ast --yes --no-time-limit --target {}`.",
            app.display(),
            app.display()
        ))
    );
    // With no map yet, the same record says so.
    std::fs::remove_file(base_ast.join("ast.ttl")).unwrap();
    age_out();
    assert_eq!(
        line(&record),
        Some(format!(
            "[AST] base stopped building the code map for {} after 10 minutes because it was stuck. \
             There is no map yet. It is trying again now, in the background. To build it with no time limit, run `base sync --ast --yes --no-time-limit --target {}`.",
            app.display(),
            app.display()
        ))
    );
}

/// Every `--yes` run has the limit; `--no-time-limit` lifts it; without `--yes` there is none.
#[test]
fn only_no_time_limit_lifts_the_limit() {
    use base::hook::automap::{limit_for, unattended_limit};
    assert_eq!(limit_for(true, false), Some(unattended_limit()));
    assert_eq!(limit_for(true, true), None);
    assert_eq!(limit_for(false, false), None);
    assert_eq!(limit_for(false, true), None);
}

/// The command the stopped-build line names really builds past the limit.
#[test]
fn no_time_limit_lets_a_long_build_finish() {
    const SLOW: &str = r##"import sys, time
from pathlib import Path
out = Path(sys.argv[sys.argv.index("--out") + 1])
time.sleep(5)
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text("# a map\n")
"##;
    let home = tempfile::tempdir().unwrap();
    let app = home.path().join("dev").join("app");
    std::fs::create_dir_all(app.join(".git")).unwrap();
    std::fs::write(app.join("a.py"), "def f():\n    pass\n").unwrap();
    let scripts = app.join("scripts").join("ast");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("onto_ast.py"), SLOW).unwrap();
    let out = Command::new(BIN)
        .args(["sync", "--ast", "--yes", "--no-time-limit", "--target"])
        .arg(&app)
        .current_dir(&app)
        .env("BASE_HOME", home.path())
        .env("BASE_AST_LIMIT_SECS", "2")
        .env("BASE_AST_SKIP_REGISTER", "1")
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env_remove("CLAUDECODE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("AST extraction complete"), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(app.join(".base-ast").join("ast.ttl").is_file(), "a 5 s build finished past the 2 s limit");
}

#[test]
fn the_limit_is_fifteen_minutes_and_under_the_build_lock() {
    // At least five times the longest unattended build measured on a real tree, and under the 30-minute build lock.
    assert_eq!(base::hook::automap::UNATTENDED_LIMIT_SECS, 900);
    assert_eq!(
        base::hook::automap::stopped_record(Duration::from_secs(900), b""),
        "base stopped the code map build after 15 minutes because it was stuck.\n"
    );
    assert_eq!(base::hook::automap::spoken(Duration::from_secs(60)), "1 minute");
    assert_eq!(base::hook::automap::spoken(Duration::from_secs(90)), "90 seconds");
}

