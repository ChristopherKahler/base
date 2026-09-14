//! F9 — dedup per RULE, not per domain block.
//!
//! This is the half of rank 01 that Chris's complaint is actually about: *"type a
//! keyword or touch a filepath, matches the domain and then DUMPS EVERY SINGLE RULE IN
//! THAT DOMAIN ALL AT ONCE EVERY TIME A MATCH HITS ... that just doesn't feel right."*
//!
//! Before this commit the unit of dedup is the domain block. Add one rule to a
//! seventeen-rule domain and the reader is handed all seventeen again, sixteen of which
//! it has already been told this session. Measured live on 2026-09-14: `auk`'s prompt
//! hook emitted 28,394 bytes because one word matched one domain, and editing
//! `BASE-WORK-ORDER.md` served all thirteen basemode rules for an edit to a base work
//! order.
//!
//! After it the unit is the rule. A rule is served once per session, again when the
//! bracket changes tier, and again if its own text or rationale changes.

use std::path::Path;

use base::config::{BaseConfig, BracketConfig, NamespaceConfig};
use base::crud;
use base::domain::session::{Bracket, SessionState};
use base::hook::{pre_tool_use, user_prompt_submit};

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

fn workspace(root: &Path) {
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
    std::fs::create_dir_all(root.join("work")).unwrap();
    std::fs::write(
        root.join(".base").join("domains.toml"),
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nprompt_keywords = [\"probe\"]\npaths = [\"work\"]\n",
    )
    .unwrap();
}

fn touch(config: &BaseConfig, root: &Path, file: &str) -> String {
    let event = serde_json::json!({
        "tool_name": "Read",
        "tool_input": { "file_path": root.join("work").join(file).display().to_string() },
    });
    pre_tool_use::handle(config, root, &event).unwrap().1
}

// ── The record itself ────────────────────────────────────────────────────────

#[test]
fn a_rule_is_claimed_once_per_session_and_again_on_a_tier_change() {
    let mut s = SessionState::default();
    s.set_active(Some("sid-a"));

    assert!(s.claim_rule("rule-1", 100, Bracket::Fresh, None), "first time this session");
    assert!(!s.claim_rule("rule-1", 100, Bracket::Fresh, None), "already served");
    assert!(
        s.claim_rule("rule-1", 100, Bracket::Moderate, None),
        "the tier changed, so the rules in force are served again once"
    );
    assert!(!s.claim_rule("rule-1", 100, Bracket::Moderate, None), "and then stays silent");
    assert!(
        s.claim_rule("rule-1", 101, Bracket::Moderate, None),
        "its text or rationale changed, so it is shown again (F8)"
    );
}

#[test]
fn place_and_action_scopes_are_counted_separately() {
    // F8: a place rule is once per session PER PLACE, and an action rule fires every
    // time its action runs. Putting the scope in the key is what makes both work
    // without a second map.
    let mut s = SessionState::default();
    s.set_active(Some("sid-b"));

    assert!(s.claim_rule("r", 1, Bracket::Fresh, Some("folder-a")), "first place");
    assert!(!s.claim_rule("r", 1, Bracket::Fresh, Some("folder-a")), "same place again");
    assert!(
        s.claim_rule("r", 1, Bracket::Fresh, Some("folder-b")),
        "a different place is a different occasion for the same rule"
    );
    assert!(
        !s.claim_rule("r", 1, Bracket::Fresh, Some("folder-a")),
        "and the first place is still claimed"
    );
}

#[test]
fn one_session_does_not_silence_another() {
    let mut s = SessionState::default();
    s.set_active(Some("sid-one"));
    assert!(s.claim_rule("r", 1, Bracket::Fresh, None));
    s.set_active(Some("sid-two"));
    assert!(s.claim_rule("r", 1, Bracket::Fresh, None), "a second Claude session saw nothing yet");
}

// ── Through the real hooks ───────────────────────────────────────────────────

#[test]
fn the_tool_hook_serves_only_the_rule_that_is_new() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let config = BaseConfig { namespace: ns(), ..Default::default() };
        SessionState::clear(&root.join(".base"));

        crud::rule::add(root, &ns(), "probe", "FIRST RULE", None).unwrap();
        let first = touch(&config, root, "x.md");
        assert!(first.contains("FIRST RULE"), "positive control: {first}");

        crud::rule::add(root, &ns(), "probe", "SECOND RULE", None).unwrap();
        let second = touch(&config, root, "y.md");

        assert!(
            second.contains("SECOND RULE"),
            "the new rule has to arrive: {second}"
        );
        assert!(
            !second.contains("FIRST RULE"),
            "and the rule this session was already told must NOT arrive again. Before \
             F9 the unit of dedup is the whole domain block, so adding one rule to a \
             seventeen-rule domain hands the reader all seventeen: {second}"
        );
    });
}

#[test]
fn the_prompt_hook_serves_only_the_rule_that_is_new() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        // Turn-counted brackets with a wide FRESH band, so this test measures per-rule
        // dedup and not a tier change.
        let config = BaseConfig {
            namespace: ns(),
            bracket: BracketConfig { fresh_until: 50, ..Default::default() },
            ..Default::default()
        };
        SessionState::clear(&root.join(".base"));

        crud::rule::add(root, &ns(), "probe", "FIRST RULE", None).unwrap();
        crud::rule::add(root, &ns(), "probe", "SECOND RULE", None).unwrap();

        let ev = |n: u32| serde_json::json!({ "prompt": format!("probe {n}"), "session_id": "sid-p" });
        let one = user_prompt_submit::handle(&config, root, &ev(1)).unwrap();
        assert_eq!(
            one.rules_injected, 2,
            "positive control: the first matching prompt carries both rules"
        );

        crud::rule::add(root, &ns(), "probe", "THIRD RULE", None).unwrap();
        let two = user_prompt_submit::handle(&config, root, &ev(2)).unwrap();
        assert_eq!(
            two.rules_injected, 1,
            "the second prompt carries ONE rule, the new one. Before F9 the block hash \
             changes when any rule changes, so all three are re-served."
        );
    });
}

#[test]
fn a_domain_whose_rules_were_all_already_served_emits_no_header() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let config = BaseConfig { namespace: ns(), ..Default::default() };
        SessionState::clear(&root.join(".base"));

        crud::rule::add(root, &ns(), "probe", "ONLY RULE", None).unwrap();
        assert!(touch(&config, root, "x.md").contains("ONLY RULE"), "positive control");

        let again = touch(&config, root, "y.md");
        assert!(
            !again.contains("[FILE MATCH: probe]"),
            "a header with nothing under it costs a line and tells the reader nothing: \
             {again}"
        );
    });
}
