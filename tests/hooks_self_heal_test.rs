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
        .join(base::install::hooks_wired_stamp())
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

// ─── BO-17: SessionEnd on a home wired before it existed ─────────────────────
//
// A home like Chris's: his own hooks beside base's five, written the way Claude Code writes the file, and stamped as
// wired by a build whose version string never changed. The new hook goes in once, as text, after a backup, and every
// other byte stays (lynx's G0 ruling on question 5).

#[test]
fn session_end_is_added_once_and_the_rest_of_settings_stays_byte_for_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join(".base-gbl")).unwrap();
    // The old stamp, named by version only: it must not stop the new hook.
    std::fs::write(home.join(".base-gbl").join(format!(".hooks-wired-{}", env!("CARGO_PKG_VERSION"))), b"").unwrap();
    let entry = |cmd: &str| {
        format!("      {{\n        \"hooks\": [\n          {{\n            \"type\": \"command\",\n            \"command\": \"{cmd}\"\n          }}\n        ]\n      }}")
    };
    // The user's own Stop hook first, base's after it, in the one Stop array.
    let five: Vec<String> = HOOK_TABLE
        .iter()
        .filter(|(event, _)| *event != "SessionEnd")
        .map(|(event, cmd)| {
            let mine = if *event == "Stop" { format!("{},\n", entry("python ~/.claude/hooks/style-guard.py")) } else { String::new() };
            format!("    \"{event}\": [\n{mine}{}\n    ]", entry(cmd))
        })
        .collect();
    let original = format!(
        "{{\n  \"model\": \"opus\",\n  \"permissions\": {{\n    \"allow\": [\n      \"Bash(git status)\"\n    ],\n    \"deny\": [\n      \"mcp__claude-in-chrome__*\"\n    ]\n  }},\n  \"hooks\": {{\n{}\n  }},\n  \"statusLine\": {{\n    \"type\": \"command\",\n    \"command\": \"bash ~/.claude/statusline.sh\"\n  }}\n}}\n",
        five.join(",\n")
    );
    let settings = home.join(".claude").join("settings.json");
    std::fs::write(&settings, &original).unwrap();

    base::home::with_thread_home(&home, || {
        assert_eq!(ensure_hooks_wired(), vec!["SessionEnd"], "exactly the new hook");
        let after = std::fs::read_to_string(&settings).unwrap();
        let at = after.find(",\n    \"SessionEnd\"").expect("added after the last event");
        assert_eq!(&after[..at], &original[..at], "every byte before it is the original's");
        assert!(after.ends_with(&original[at..]), "every byte after it is the original's");
        let v: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["hooks"]["SessionEnd"][0]["hooks"][0]["command"], "base hook session-end");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "python ~/.claude/hooks/style-guard.py", "the user's own hook kept");
        assert_eq!(v["hooks"]["Stop"][1]["hooks"][0]["command"], "base hook stop");
        // The backup: the file as it was, beside it.
        let backups: Vec<std::path::PathBuf> = std::fs::read_dir(home.join(".claude"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("settings.json.bak-base-")))
            .collect();
        assert_eq!(backups.len(), 1, "one backup");
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original, "the backup is the file before the change");

        // Never twice: the stamp holds it, and with the stamp gone the command in the file does.
        assert!(ensure_hooks_wired().is_empty());
        std::fs::remove_file(stamp_of(&home)).unwrap();
        assert!(ensure_hooks_wired().is_empty(), "already there, so nothing added");
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), after, "and nothing rewritten");
        assert_eq!(after.matches("base hook session-end").count(), 1);
    });
}
