//! #20: a hook that failed is named by the failure summary doctor and session start read.

fn dir_with(lines: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hook-events.jsonl"), lines.join("\n") + "\n").unwrap();
    dir
}

#[test]
fn a_failed_hook_in_the_window_is_reported_with_its_error() {
    let dir = dir_with(&[
        r#"{"ts":"2026-09-07T09:00:00-05:00","hook":"session-start","success":true}"#,
        r#"{"ts":"2026-09-07T09:01:00-05:00","hook":"user-prompt-submit","success":false,"error":"Failed to parse graph"}"#,
        r#"{"ts":"2026-09-07T09:02:00-05:00","hook":"stop","success":true}"#,
    ]);
    let s = base::hook::hook_failure_summary(dir.path()).expect("a failure must be reported").summary;
    assert!(s.contains("1 failed of the last 3"), "{s}");
    assert!(s.contains("user-prompt-submit at 2026-09-07T09:01:00-05:00: Failed to parse graph"), "{s}");
}

#[test]
fn a_clean_trail_reports_nothing() {
    let dir = dir_with(&[r#"{"ts":"t","hook":"stop","success":true}"#]);
    assert_eq!(base::hook::hook_failure_summary(dir.path()), None);
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(base::hook::hook_failure_summary(empty.path()), None, "no log, nothing to say");
}

#[test]
fn only_the_window_counts() {
    let mut lines: Vec<String> = vec![r#"{"ts":"old","hook":"stop","success":false,"error":"ancient"}"#.into()];
    for _ in 0..base::hook::HOOK_FAILURE_WINDOW {
        lines.push(r#"{"ts":"t","hook":"stop","success":true}"#.into());
    }
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let dir = dir_with(&refs);
    assert_eq!(base::hook::hook_failure_summary(dir.path()), None, "a failure older than the window is not a live fault");
}

#[test]
fn a_hook_whose_latest_event_failed_is_broken_now() {
    // The failure is the last word on `user-prompt-submit`: nothing has succeeded
    // for that hook since, so hooks are broken today and `doctor` must go red.
    let dir = dir_with(&[
        r#"{"ts":"2026-09-07T09:00:00-05:00","hook":"user-prompt-submit","success":true}"#,
        r#"{"ts":"2026-09-07T09:01:00-05:00","hook":"stop","success":true}"#,
        r#"{"ts":"2026-09-07T09:02:00-05:00","hook":"user-prompt-submit","success":false,"error":"store would not open"}"#,
    ]);
    let t = base::hook::hook_failure_summary(dir.path()).expect("a failure must be reported");
    assert!(t.broken_now, "the latest user-prompt-submit event failed: {}", t.summary);
    assert!(t.summary.contains("has not succeeded since"), "{}", t.summary);
}

#[test]
fn an_older_failure_with_successes_after_it_is_history_not_a_fault() {
    // Same failure, but the hook has run cleanly since. It is still reported, so
    // an operator can see it happened, and it does NOT hold `doctor` red.
    let dir = dir_with(&[
        r#"{"ts":"2026-09-07T09:00:00-05:00","hook":"user-prompt-submit","success":false,"error":"store would not open"}"#,
        r#"{"ts":"2026-09-07T09:01:00-05:00","hook":"user-prompt-submit","success":true}"#,
        r#"{"ts":"2026-09-07T09:02:00-05:00","hook":"stop","success":true}"#,
    ]);
    let t = base::hook::hook_failure_summary(dir.path()).expect("the failure is still reported");
    assert!(!t.broken_now, "it succeeded after the failure: {}", t.summary);
    assert!(t.summary.contains("1 failed of the last 3"), "{}", t.summary);
    assert!(t.summary.contains("history rather than a live fault"), "{}", t.summary);
}

#[test]
fn session_start_is_silent_about_a_hook_that_has_recovered() {
    // `doctor` still reports the old failure; session start must not, or one
    // transient miss becomes a banner on every session from then on.
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    let ws = tempfile::tempdir().unwrap();
    let base = ws.path().join(".base");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(
        base.join("hook-events.jsonl"),
        concat!(
            r#"{"ts":"t1","hook":"user-prompt-submit","success":false,"error":"transient"}"#,
            "\n",
            r#"{"ts":"t2","hook":"user-prompt-submit","success":true}"#,
            "\n",
        ),
    )
    .unwrap();

    let trail = base::hook::hook_failure_summary(&base).expect("the failure is still on record");
    assert!(!trail.broken_now, "it recovered: {}", trail.summary);

    // The session-start line is gated on exactly that flag.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_base"))
        .args(["hook", "session-start"])
        .current_dir(ws.path())
        .env("BASE_HOME", home.path())
        .env("BASE_NO_AUTO_UPDATE", "1")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !text.contains("[hooks]"),
        "a recovered hook must not announce itself at session start: {text}"
    );
}

#[test]
fn diagnose_reads_the_hook_log_under_the_cwd_it_was_given() {
    // It called `std::env::current_dir()` while taking a `cwd` param, so the
    // section was untestable and wrong for any caller that does not chdir.
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    let ws = tempfile::tempdir().unwrap();
    let base = ws.path().join(".base");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("graph.nq"), "").unwrap();
    std::fs::write(
        base.join("hook-events.jsonl"),
        "{\"ts\":\"t1\",\"hook\":\"user-prompt-submit\",\"success\":false,\"error\":\"planted under the param\"}\n",
    )
    .unwrap();

    // Never chdir: the param is the only way this log can be found.
    let report = base::doctor::diagnose(ws.path());
    assert!(
        report.warnings.iter().any(|w| w.contains("planted under the param")),
        "diagnose ignored its cwd param; warnings were {:?}",
        report.warnings
    );
    assert!(!report.healthy, "a hook failing now is not healthy");
}
