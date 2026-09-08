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

// ─── Issue #93 ──────────────────────────────────────────────
//
// `ensure_hooks_wired` returns an empty vec for TWO different states: "already
// fully wired" and "there is no ~/.claude directory to write into". It stamped
// the version for both. So on a machine where base was installed before Claude
// Code, the first call disarmed the repair for the rest of that version's life
// — which is what turns a delay into the permanent inertness #93 reports.
//
// The stamp means "reconciled against a real Claude Code config", not "tried".

/// The stamp file `ensure_hooks_wired` gates itself on.
fn stamp_of(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".base-gbl")
        .join(format!(".hooks-wired-{}", env!("CARGO_PKG_VERSION")))
}

#[test]
fn no_claude_code_yet_leaves_the_stamp_absent_so_the_next_run_retries() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    // `.base-gbl` exists on every real install (step 2 runs before step 3), and
    // it has to exist here too: without it the stamp write fails for an
    // unrelated reason and this test would pass without proving anything.
    std::fs::create_dir_all(home.join(".base-gbl")).unwrap();
    assert!(home.join(".base-gbl").is_dir(), "the stamp has somewhere to land");
    // No ~/.claude at all: base installed before Claude Code.
    assert!(!home.join(".claude").exists());

    base::home::with_thread_home(&home, || {
        assert!(ensure_hooks_wired().is_empty(), "nothing to wire yet");
        assert!(
            !stamp_of(&home).exists(),
            "no Claude Code config was reconciled, so nothing is stamped"
        );

        // Claude Code arrives. The very next call repairs, with no re-install.
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let added = ensure_hooks_wired();
        assert_eq!(added.len(), HOOK_TABLE.len(), "every hook, on the next run");

        let text = std::fs::read_to_string(home.join(".claude").join("settings.json")).unwrap();
        for (_, cmd) in HOOK_TABLE {
            assert_eq!(text.matches(cmd).count(), 1, "{cmd}: present exactly once");
        }
        assert!(stamp_of(&home).exists(), "now there was something to stamp");
        assert!(ensure_hooks_wired().is_empty(), "and once per version after that");
    });
}

#[test]
fn an_already_wired_home_is_stamped_and_left_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join(".base-gbl")).unwrap();

    let settings = home.join(".claude").join("settings.json");
    base::home::with_thread_home(&home, || {
        // Wire it once, then assert the second pass changes nothing at all.
        assert_eq!(ensure_hooks_wired().len(), HOOK_TABLE.len());
        std::fs::remove_file(stamp_of(&home)).unwrap();
        let before = std::fs::read(&settings).unwrap();

        assert!(ensure_hooks_wired().is_empty(), "nothing left to add");
        assert_eq!(std::fs::read(&settings).unwrap(), before, "not one byte rewritten");
        assert!(stamp_of(&home).exists(), "a real config WAS reconciled");
    });
}
