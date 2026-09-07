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
    let s = base::hook::hook_failure_summary(dir.path()).expect("a failure must be reported");
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
