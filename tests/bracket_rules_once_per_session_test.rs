//! BO-03, F3 — each bracket rule is sent once per session, and never when a CLAUDE.md Claude Code loads
//! already carries it.
//!
//! WHAT CHANGED. Until BO-03 the bracket block went out once per TIER (K1, Chris 2026-09-12: "inject one
//! time, then no more, inject only when bracket changes the rules for that bracket"), whole. So every tier
//! change sent the `always` rules again, and the T1 to T6 rules went out although `~/.claude/CLAUDE.md` carries
//! them: on 2026-10-01 they took about 2,400 of 3,864 bytes of a prompt in session `5b860473`.
//!
//! NOW (scope F3, locked under D12; per rule, not per block, ruled by lynx 2026-10-02):
//!   - a rule whose `covered_by` text is in a loaded CLAUDE.md is never sent;
//!   - every other rule is sent once per session, on the first prompt where its tier applies, and never
//!     again, a tier change included; the DEPLETED and CRITICAL buckets go out once when their tier arrives;
//!   - a rule the budget dropped was not sent, so it stays due (D15); an edited text is a new rule.
//!
//! This file was `bracket_rules_once_per_tier_test.rs`. Two of its tests asserted the per-tier behaviour F3
//! replaces and are gone: `the_gate_claims_a_tier_once_and_reopens_on_a_change` and
//! `the_prompt_hook_sends_the_block_once_per_tier` (its prompt 3 re-sent the `always` rule at MODERATE).
//! The other four hold under F3 and are kept here on the per-rule record.

use std::path::Path;

use base::config::{BaseConfig, BracketConfig, BracketRule, BracketRules};
use base::domain::session::{bracket_rule_key, Bracket, SessionState};
use base::hook::user_prompt_submit;

const ALWAYS_ON_DOMAIN: &str = r#"
[[domain]]
name = "probe"
mode = "always"
rules = ["the probe rule"]
"#;

/// The heading lines of the operator's real `~/.claude/CLAUDE.md` "Trait Routing" section (Example 1).
const CLAUDE_MD: &str = "# Instructions\n\n## Trait Routing — Always On\n\n\
### T1 — Confidence is numeric, never prose\n\nNever hedge in sentences.\n\n\
### T2 — Position changes are audited (deference check)\n\nUPDATED, MISREAD or DEFERRED.\n\n\
### T3 — Scope has a drain, not a leash\n\nAdjacent: five lines.\n\n\
### T4 — Objections are one line, logged once\n\nObjection: once.\n\n\
### T5 — Emotional context never lowers the bar\n\nTechnical answer first.\n\n\
### T6 — Underdetermined asks declare their target\n\nAiming at: one line.\n";

fn covered(text: &str, markers: &[&str]) -> BracketRule {
    BracketRule { text: text.to_string(), covered_by: markers.iter().map(|m| m.to_string()).collect() }
}

/// Example 1: the operator's bracket rules, as base.toml carries them, with the BO-03 markers.
fn example_one_rules() -> BracketRules {
    BracketRules {
        always: vec![
            "CODENAME_RULE register first, then read your row back".into(),
            covered("T1_RULE never hedge in prose", &["### T1 — Confidence is numeric, never prose"]),
            covered("T2_RULE when you change a stated position", &["### T2 — Position changes are audited"]),
        ],
        fresh: vec![
            covered("T3_RULE answer only what was asked", &["### T3 — Scope has a drain, not a leash"]),
            covered("T4_RULE build it as specified", &["### T4 — Objections are one line, logged once"]),
            covered("T5_RULE emotional context is background", &["### T5 — Emotional context never lowers the bar"]),
            covered("T6_RULE declare the assumed target", &["### T6 — Underdetermined asks declare their target"]),
        ],
        moderate: vec![
            covered("T3T4_RULE adjacent drain still applies", &["### T3 —", "### T4 —"]),
            covered("T5T6_RULE emotional context never lowers the bar", &["### T5 —", "### T6 —"]),
        ],
        depleted: vec!["DEPLETED_RULE write your handoff now".into()],
        critical: vec!["CRITICAL_RULE stop and hand over".into()],
    }
}

/// Turn-counted brackets: FRESH for prompt 1, MODERATE for 2, DEPLETED for 3-4, CRITICAL from 5. No transcript
/// is passed, so the turn thresholds decide; the force-refresh is off, it is a separate mechanism.
fn config(rules: BracketRules) -> BaseConfig {
    BaseConfig {
        bracket: BracketConfig {
            enabled: true,
            fresh_until: 1,
            moderate_until: 2,
            depleted_until: 4,
            refresh_interval: 0,
            mode: None,
            rules,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn workspace(root: &Path) {
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
    std::fs::write(root.join(".base").join("domains.toml"), ALWAYS_ON_DOMAIN).unwrap();
    SessionState::clear(&root.join(".base"));
}

fn user_claude_md(home: &Path, text: &str) {
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude").join("CLAUDE.md"), text).unwrap();
}

/// One prompt; what the hook printed.
fn prompt(config: &BaseConfig, root: &Path, text: &str) -> String {
    let event = serde_json::json!({ "prompt": text, "session_id": "sid-bracket" });
    let mut out = String::new();
    user_prompt_submit::handle(config, root, &event, &mut out).unwrap();
    out
}

/// The rule labels a prompt's output carries, in order.
fn sent(out: &str) -> Vec<&'static str> {
    // BO-31: a rule the budget held back is listed by title at the end of the output (`  0. CODENAME_RULE …`); a title
    // is not the rule sent, so the scan stops at the list's first line.
    let out = match out.find(" for this message did not fit in its ") {
        Some(at) => &out[..out[..at].rfind('\n').unwrap_or(0)],
        None => out,
    };
    const LABELS: [&str; 11] = [
        "CODENAME_RULE", "T1_RULE", "T2_RULE", "T3_RULE", "T4_RULE", "T5_RULE", "T6_RULE", "T3T4_RULE", "T5T6_RULE",
        "DEPLETED_RULE", "CRITICAL_RULE",
    ];
    LABELS.into_iter().filter(|l| out.contains(&format!(". {l} "))).collect()
}

// ── Example 1: covered by CLAUDE.md ─────────────────────────────────────────

#[test]
fn bracket_rule_covered_by_claude_md_is_not_sent() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        user_claude_md(root, CLAUDE_MD);
        let cfg = config(example_one_rules());

        let first = prompt(&cfg, root, "prompt one");
        assert_eq!(sent(&first), ["CODENAME_RULE"], "prompt 1 carries rule 0 and nothing CLAUDE.md covers:\n{first}");
        assert!(first.contains("[BRACKET RULES — FRESH]\n  0. CODENAME_RULE"), "renumbered from 0:\n{first}");
        let second = prompt(&cfg, root, "prompt two");
        assert!(sent(&second).is_empty(), "MODERATE: both of its rules are covered, rule 0 was sent:\n{second}");
        assert!(!second.contains("[BRACKET RULES"), "no block at all when nothing is due:\n{second}");
    });
}

#[test]
fn without_the_claude_md_headings_every_rule_is_sent_once() {
    // "A user without these CLAUDE.md headings gets all the rules once per session."
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        user_claude_md(root, "# Instructions\n\nNothing about traits here.\n");
        let cfg = config(example_one_rules());

        let first = prompt(&cfg, root, "prompt one");
        assert_eq!(
            sent(&first),
            ["CODENAME_RULE", "T1_RULE", "T2_RULE", "T3_RULE", "T4_RULE", "T5_RULE", "T6_RULE"],
            "{first}"
        );
        let second = prompt(&cfg, root, "prompt two");
        assert_eq!(sent(&second), ["T3T4_RULE", "T5T6_RULE"], "MODERATE sends only its own rules:\n{second}");
    });
}

#[test]
fn a_rule_listing_two_markers_needs_both() {
    let rule = covered("T3T4_RULE", &["### T3 —", "### T4 —"]);
    assert!(rule.covered_in(CLAUDE_MD));
    assert!(!rule.covered_in("### T3 — Scope has a drain"), "one of two markers is not covered");
    assert!(!covered("X", &["", "  "]).covered_in(CLAUDE_MD), "a blank marker covers nothing");
    assert!(!BracketRule::from("X").covered_in(CLAUDE_MD), "no marker, never covered");
}

#[test]
fn a_claude_md_in_the_working_folder_or_above_covers_too() {
    // Claude Code loads CLAUDE.md, .claude/CLAUDE.md and CLAUDE.local.md in the working folder and every folder
    // above it (code.claude.com/docs/en/memory, read 2026-10-02).
    for (place, file) in [
        ("working folder", "CLAUDE.md"),
        ("working folder", "CLAUDE.local.md"),
        ("parent", ".claude/CLAUDE.md"),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        // The home is its own folder, so the parent's `.claude/CLAUDE.md` is not also the user's file.
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        base::home::with_thread_home(&home, || {
            let root = tmp.path().join("project").join("work");
            workspace(&root);
            let dir = if place == "parent" { tmp.path().join("project") } else { root.clone() };
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, CLAUDE_MD).unwrap();
            let cfg = config(example_one_rules());
            let first = prompt(&cfg, &root, "prompt one");
            assert_eq!(sent(&first), ["CODENAME_RULE"], "{file} in the {place}:\n{first}");
        });
    }
}

#[test]
fn coverage_is_read_once_per_session_as_claude_code_reads_claude_md() {
    // Claude Code reads CLAUDE.md at launch and again at `/compact`, which is a session start, and session start
    // clears this session's records. An edit in between is not in the reader's context, so it does not change
    // what this session is sent; the next session reads the file again.
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        user_claude_md(root, CLAUDE_MD);
        let rules = BracketRules {
            always: vec![covered("T1_RULE never hedge in prose", &["### T1 — Confidence is numeric, never prose"])],
            depleted: vec![covered("T2_RULE when you change a stated position", &["### T2 — Position changes are audited"])],
            ..Default::default()
        };
        let cfg = config(rules);
        assert!(sent(&prompt(&cfg, root, "one")).is_empty(), "covered at the session's first prompt");
        let s = SessionState::load(&root.join(".base"));
        assert_eq!(s.bracket_coverage("T1_RULE never hedge in prose"), Some(true), "the verdict is recorded");
        assert_eq!(
            s.bracket_coverage("T2_RULE when you change a stated position"),
            Some(true),
            "for every marked rule of every tier, read once"
        );

        user_claude_md(root, "# Instructions\n\nThe trait section was removed.\n");
        for i in 2..=4 {
            let out = prompt(&cfg, root, &format!("prompt {i}"));
            assert!(sent(&out).is_empty(), "prompt {i}: the session keeps the CLAUDE.md it started with:\n{out}");
        }

        SessionState::clear(&root.join(".base")); // what session start does for the next session
        assert_eq!(sent(&prompt(&cfg, root, "new session")), ["T1_RULE"], "a new session reads the file again");
    });
}

#[test]
fn the_same_text_covered_in_one_bucket_is_covered_in_all() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        user_claude_md(root, CLAUDE_MD);
        let cfg = config(BracketRules {
            always: vec![covered("T1_RULE never hedge", &["### T1 — Confidence is numeric, never prose"])],
            depleted: vec!["T1_RULE never hedge".into()],
            ..Default::default()
        });
        for i in 1..=4 {
            let out = prompt(&cfg, root, &format!("prompt {i}"));
            assert!(sent(&out).is_empty(), "prompt {i}: one text, one rule, covered:\n{out}");
        }
    });
}

// ── Once per session ────────────────────────────────────────────────────────

#[test]
fn uncovered_bracket_rules_sent_once_per_session() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let rules = BracketRules {
            always: vec!["CODENAME_RULE register first".into()],
            fresh: vec!["T3_RULE answer only what was asked".into()],
            ..Default::default()
        };
        // Prompts 1-2 FRESH, 3 MODERATE: the tier changes and no MODERATE rule exists.
        let cfg = BaseConfig {
            bracket: BracketConfig { fresh_until: 2, moderate_until: 4, ..config(rules).bracket },
            ..Default::default()
        };

        let outs: Vec<String> = (1..=3).map(|i| prompt(&cfg, root, &format!("prompt {i}"))).collect();
        assert_eq!(sent(&outs[0]), ["CODENAME_RULE", "T3_RULE"], "the first prompt has them:\n{}", outs[0]);
        assert!(sent(&outs[1]).is_empty(), "the second prompt does not:\n{}", outs[1]);
        assert!(outs[2].contains("[MODERATE]"), "control: prompt 3 is MODERATE:\n{}", outs[2]);
        assert!(sent(&outs[2]).is_empty(), "and the bracket change does not:\n{}", outs[2]);
    });
}

#[test]
fn depleted_and_critical_rules_are_sent_once_when_their_tier_arrives() {
    // lynx, 2026-10-02: per rule, so the DEPLETED rules (write the handoff) and the CRITICAL rules (stop) still
    // go out, once each, and the header names the tier on every prompt.
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        user_claude_md(root, CLAUDE_MD);
        let cfg = config(example_one_rules());
        let tiers = ["FRESH", "MODERATE", "DEPLETED", "DEPLETED", "CRITICAL", "CRITICAL"];
        let want: [&[&str]; 6] = [&["CODENAME_RULE"], &[], &["DEPLETED_RULE"], &[], &["CRITICAL_RULE"], &[]];
        for (i, (tier, want)) in tiers.iter().zip(want).enumerate() {
            let out = prompt(&cfg, root, &format!("prompt {}", i + 1));
            assert!(
                out.starts_with(&format!("<context-bracket>[{tier}] (prompt {})</context-bracket>", i + 1)),
                "the header names the tier on prompt {}:\n{out}",
                i + 1
            );
            assert_eq!(sent(&out), want, "prompt {} ({tier}):\n{out}", i + 1);
        }
    });
}

#[test]
fn a_bracket_rule_the_budget_drops_stays_due() {
    // D15: a rule is sent only when it was printed whole. Prompt 1 matches a keyword domain whose rule fills the
    // budget, so the bracket block (priority 5, last) is dropped; prompt 2 has room and carries it.
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".base")).unwrap();
        std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
        let big = "x".repeat(900);
        std::fs::write(
            root.join(".base").join("domains.toml"),
            format!("[[domain]]\nname = \"bulky\"\nprompt_keywords = [\"bulky\"]\nrules = [\"BULKY {big}\"]\n"),
        )
        .unwrap();
        SessionState::clear(&root.join(".base"));
        // 300 bytes of bracket rule beside a 900-byte matched rule: together over 1,200, the matched rule and the
        // bracket block's pointer line under it.
        let rule = format!("CODENAME_RULE {}", "y".repeat(300));
        let mut cfg = config(BracketRules { always: vec![rule.as_str().into()], ..Default::default() });
        cfg.budget.prompt_bytes = 1200;

        let first = prompt(&cfg, root, "the bulky one");
        assert!(first.contains("BULKY"), "control: the matched rule printed:\n{first}");
        assert!(sent(&first).is_empty(), "the bracket block did not fit:\n{first}");
        assert!(first.contains("withheld bracket-rules"), "and its pointer line says so:\n{first}");
        let second = prompt(&cfg, root, "a quiet one");
        assert_eq!(sent(&second), ["CODENAME_RULE"], "it stayed due and goes now:\n{second}");
        assert!(sent(&prompt(&cfg, root, "another")).is_empty(), "and only once");
    });
}

#[test]
fn an_edited_bracket_rule_is_sent_again() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let before = config(BracketRules { always: vec!["CODENAME_RULE first wording".into()], ..Default::default() });
        let after = config(BracketRules { always: vec!["CODENAME_RULE second wording".into()], ..Default::default() });

        assert!(prompt(&before, root, "one").contains("CODENAME_RULE first wording"));
        let edited = prompt(&after, root, "two");
        assert!(edited.contains("CODENAME_RULE second wording"), "the edited text is a new rule:\n{edited}");
        assert!(sent(&prompt(&after, root, "three")).is_empty(), "sent once, like any rule");
    });
}

#[test]
fn the_same_text_in_two_buckets_is_one_rule() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let cfg = config(BracketRules {
            always: vec!["CODENAME_RULE twice".into()],
            fresh: vec!["CODENAME_RULE twice".into()],
            moderate: vec!["CODENAME_RULE twice".into()],
            ..Default::default()
        });
        let first = prompt(&cfg, root, "one");
        assert_eq!(first.matches("CODENAME_RULE twice").count(), 1, "{first}");
        assert!(sent(&prompt(&cfg, root, "two")).is_empty(), "MODERATE carries the same text, already sent");
    });
}

// ── Kept from the per-tier file: they hold under F3 ─────────────────────────

#[test]
fn the_gate_is_per_session_not_per_workspace() {
    // `.session` is one file per workspace and several Claude sessions share a workspace, so a record that was
    // not per session would let one session's first prompt silence another session's first prompt.
    let (id, content) = bracket_rule_key("ALWAYS_RULE");
    let mut s = SessionState::default();

    s.set_active(Some("sid-a"));
    assert!(s.bracket_rule_due("ALWAYS_RULE"));
    s.mark_rule_shown(&id, content, Bracket::Fresh, None, 1);
    assert!(!s.bracket_rule_due("ALWAYS_RULE"));

    s.set_active(Some("sid-b"));
    assert!(s.bracket_rule_due("ALWAYS_RULE"), "a different Claude session has not been sent anything yet");

    s.set_active(Some("sid-a"));
    assert!(!s.bracket_rule_due("ALWAYS_RULE"), "and the first session is still silenced");
    for tier in [Bracket::Moderate, Bracket::Depleted, Bracket::Critical] {
        s.record_tier(tier);
        assert!(!s.bracket_rule_due("ALWAYS_RULE"), "{tier}: a tier change does not re-open a sent rule");
    }
}

#[test]
fn a_refresh_starts_from_nothing_and_a_live_session_is_not_disturbed() {
    // J7 item 9: a new Claude Code session id resets the once-per-session state. A refresh is a new session, and
    // Chris refreshes rather than compacting, so this is the path that actually runs on his machine.
    //
    // Proved on the STATE, not through `handle`: `handle` scopes its record through `SessionState::load`, which
    // binds to `PROCESS_SESSION`, a `OnceLock` set once at hook entry. In production that is exact (one hook, one
    // process, one session); inside one test process every call shares the scope whatever the event's
    // `session_id`, so the state API is the honest place to show two sessions.
    let (id, content) = bracket_rule_key("ALWAYS_RULE");
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();

    let mut live = SessionState::load_for(&base_dir, Some("sid-one"));
    assert!(live.bracket_rule_due("ALWAYS_RULE"), "first prompt of the live session");
    live.mark_rule_shown(&id, content, Bracket::Fresh, None, 1);
    live.save(&base_dir).unwrap();

    let mut successor = SessionState::load_for(&base_dir, Some("sid-two"));
    assert!(successor.bracket_rule_due("ALWAYS_RULE"), "a refresh is a new session id and starts from nothing");
    successor.mark_rule_shown(&id, content, Bracket::Fresh, None, 2);
    successor.save(&base_dir).unwrap();

    let live_again = SessionState::load_for(&base_dir, Some("sid-one"));
    assert!(!live_again.bracket_rule_due("ALWAYS_RULE"), "the original session stays silent");
}

#[test]
fn the_no_domains_path_is_gated_too() {
    // One of four return sites in `handle` prints the block: the empty-domains return, the star-command return,
    // the no-match return, and the main path. A gate at three of them is one a user routes around by having no
    // domains configured.
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".base")).unwrap();
        std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
        SessionState::clear(&root.join(".base"));
        let cfg = config(BracketRules { always: vec!["CODENAME_RULE no domains".into()], ..Default::default() });

        assert_eq!(sent(&prompt(&cfg, root, "hello")), ["CODENAME_RULE"], "first prompt carries it");
        assert!(sent(&prompt(&cfg, root, "again")).is_empty(), "the second does not");
    });
}

#[test]
fn a_tier_with_no_configured_rules_records_nothing() {
    // base ships no bracket rules, so a default install renders nothing, and nothing may be recorded as sent:
    // that would silence the first real rule an operator configures mid-session.
    let empty = BracketRules::default();
    let texts: Vec<&str> = Bracket::Fresh.entries(&empty).iter().map(|r| r.text.as_str()).collect();
    let rendered = base::domain::session::render_bracket_rules(Bracket::Fresh, &texts);
    assert!(rendered.is_empty(), "a default install renders nothing: {rendered:?}");

    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let out = prompt(&config(BracketRules::default()), root, "one");
        assert!(!out.contains("[BRACKET RULES"), "{out}");
        let s = SessionState::load(&root.join(".base"));
        assert!(
            s.rules_shown.keys().all(|k| !k.contains("bracket:")),
            "nothing recorded for a block never printed: {:?}",
            s.rules_shown.keys().collect::<Vec<_>>()
        );
        let cfg = config(BracketRules { always: vec!["CODENAME_RULE configured later".into()], ..Default::default() });
        assert_eq!(sent(&prompt(&cfg, root, "two")), ["CODENAME_RULE"], "the first configured rule is sent");
    });
}

#[test]
fn bracket_rules_parse_as_strings_or_tables() {
    let cfg: BracketRules = toml::from_str(
        "always = [\"plain\", { text = \"covered\", covered_by = \"### T1 —\" }]\n\
         moderate = [{ text = \"two\", covered_by = [\"### T3 —\", \"### T4 —\"] }]\n\
         fresh = [{ text = \"bare table\" }]\n",
    )
    .unwrap();
    assert_eq!(cfg.always[0], BracketRule::from("plain"));
    assert_eq!(cfg.always[1].covered_by, ["### T1 —"]);
    assert_eq!(cfg.moderate[0].covered_by, ["### T3 —", "### T4 —"]);
    assert!(cfg.fresh[0].covered_by.is_empty());
    // Serialized in the shape it was read: a string without a marker, so a config written before BO-03 round-trips.
    assert_eq!(serde_json::to_value(&cfg.always[0]).unwrap(), serde_json::json!("plain"));
    assert_eq!(
        serde_json::to_value(&cfg.moderate[0]).unwrap(),
        serde_json::json!({ "text": "two", "covered_by": ["### T3 —", "### T4 —"] })
    );
}
