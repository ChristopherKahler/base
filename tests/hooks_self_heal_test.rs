//! A release that adds a hook must not depend on the operator re-running
//! `base install`. The auto-update swaps the binary and touches nothing else,
//! and a hook that is not in settings.json never fires — silently. Measured
//! 2026-09-01: `base hook stop` existed since 0.13.5 and was wired on this
//! machine by hand four releases later; every auto-updated install in between
//! ran with no Stop hook and therefore no automatic map refresh.

use base::install::{ensure_hooks_wired, wire_hooks_quiet, HOOK_TABLE};

#[test]
fn a_release_that_adds_a_hook_wires_it_at_session_start_once() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join(".base-gbl")).unwrap();

    // An install from before the Stop hook existed: four of the five.
    let four: Vec<String> = HOOK_TABLE
        .iter()
        .filter(|(event, _)| *event != "Stop")
        .map(|(event, cmd)| format!(r#""{event}":[{{"hooks":[{{"type":"command","command":"{cmd}"}}]}}]"#))
        .collect();
    let settings = home.join(".claude").join("settings.json");
    std::fs::write(&settings, format!("{{\"hooks\":{{{}}}}}", four.join(","))).unwrap();

    base::home::with_thread_home(&home, || {
        assert_eq!(ensure_hooks_wired(), vec!["Stop"], "exactly the missing hook");
        let text = std::fs::read_to_string(&settings).unwrap();
        for (_, cmd) in HOOK_TABLE {
            assert_eq!(text.matches(cmd).count(), 1, "{cmd}: present exactly once");
        }
        // Once per version: the next session start adds nothing.
        assert!(ensure_hooks_wired().is_empty());
    });
}

#[test]
fn a_fresh_claude_install_with_no_settings_file_gets_one() {
    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();

    let added = wire_hooks_quiet(&settings).unwrap();
    assert_eq!(added.len(), HOOK_TABLE.len(), "every hook, into a file that did not exist");

    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    for (event, cmd) in HOOK_TABLE {
        assert_eq!(v["hooks"][event][0]["hooks"][0]["command"], serde_json::json!(cmd), "{event}");
    }
}

#[test]
fn no_claude_directory_means_nothing_to_wire() {
    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join(".claude").join("settings.json");
    assert!(wire_hooks_quiet(&settings).unwrap().is_empty());
    assert!(!settings.exists(), "base never invents a Claude Code install");
}
