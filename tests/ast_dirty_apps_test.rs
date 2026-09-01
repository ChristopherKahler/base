//! A dirty-app mark must survive the session's cwd moving between the edit and
//! the Stop hook. Measured 2026-09-01: a session running from the home dir
//! edited dev/logos-wall with cwd inside the app; the mark landed in
//! logos-wall/.base/.session, the Stop hook (cwd back at home) drained
//! C:\Users\Chris\.base\.session, and the map never refreshed. The fix keeps a
//! second copy of the mark in one place, keyed per session, that any Stop can
//! drain. These helpers take the directory explicitly so the test never
//! touches the real global tier.
//!
//! The process binds to ONE session id (a hook process serves exactly one
//! session), so a second session is impersonated by writing its scoped key
//! into the file by hand rather than by rebinding.

use base::domain::session::{set_process_session, SessionState};

#[test]
fn a_mark_is_drained_from_the_dir_it_was_written_under_by_its_own_session_once() {
    let tmp = tempfile::tempdir().unwrap();
    let shared = tmp.path().join("shared-base");
    std::fs::create_dir_all(&shared).unwrap();
    set_process_session(Some("sess-a"));

    assert!(SessionState::mark_dirty_app_in(&shared, "C:/apps/logos-wall"));
    assert!(!SessionState::mark_dirty_app_in(&shared, "C:/apps/logos-wall"), "a repeat mark is not new");

    // Another session's pending refresh, written the way the file stores it:
    // "<session>\u{1}<root>". Draining as sess-a must leave it alone.
    let path = shared.join(".session");
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    doc["dirty_apps"].as_array_mut().unwrap().push(serde_json::Value::String("sess-b\u{1}C:/apps/other".into()));
    std::fs::write(&path, serde_json::to_string(&doc).unwrap()).unwrap();

    assert_eq!(SessionState::take_dirty_apps_in(&shared), vec!["C:/apps/logos-wall".to_string()]);
    assert!(SessionState::take_dirty_apps_in(&shared).is_empty(), "drained twice");

    let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let left: Vec<&str> = after["dirty_apps"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(left, vec!["sess-b\u{1}C:/apps/other"], "sess-b's mark was stolen or dropped");
}
