//! #22: `hook-events.jsonl` is bounded by its writer, not by whoever opens the dashboard.

#[test]
fn the_writer_trims_the_log_once_it_is_over_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let base_dir = dir.path();
    let log = base_dir.join("hook-events.jsonl");
    let line = "{\"ts\":\"2026-09-07T00:00:00-05:00\",\"hook\":\"user-prompt-submit\",\"success\":true}\n";
    let mut body = String::new();
    while (body.len() as u64) <= base::hook::HOOK_LOG_CAP_BYTES {
        body.push_str(line);
    }
    std::fs::write(&log, &body).unwrap();
    let before = std::fs::metadata(&log).unwrap().len();
    assert!(before > base::hook::HOOK_LOG_CAP_BYTES);

    base::hook::append_hook_event(base_dir, &serde_json::json!({"hook": "stop", "success": true, "marker": "the-new-event"}));

    let after = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = after.lines().collect();
    assert!(lines.len() <= base::hook::HOOK_LOG_KEEP_LINES + 1, "kept {} lines", lines.len());
    assert!(lines.last().unwrap().contains("the-new-event"), "the new event is the last line");
    assert!((after.len() as u64) < before, "the file shrank");
}

#[test]
fn under_the_cap_the_writer_only_appends() {
    let dir = tempfile::tempdir().unwrap();
    let base_dir = dir.path();
    base::hook::append_hook_event(base_dir, &serde_json::json!({"hook": "stop", "success": true}));
    base::hook::append_hook_event(base_dir, &serde_json::json!({"hook": "stop", "success": false}));
    let s = std::fs::read_to_string(base_dir.join("hook-events.jsonl")).unwrap();
    assert_eq!(s.lines().count(), 2);
}
