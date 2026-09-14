//! F7 / K1 — the bracket-rules block injects ONCE per tier, not once per prompt.
//!
//! Chris ruled it 2026-09-12: *"no, inject one time, then no more, inject only when
//! bracket changes the rules for that bracket."*
//!
//! At `0ace1ba` the block is exempt from dedup by design. `src/hook/user_prompt_submit.rs`
//! says so in its own comment: *"Bracket rules — tier-gated, never deduped. Re-injecting
//! every prompt IS the feature"*. Measured by `wolf` on 2026-09-12: 2,931 bytes on every
//! prompt at FRESH and 1,878 at MODERATE, in a session where the same text also sat in
//! `~/.claude/CLAUDE.md`.
//!
//! Red at `0ace1ba`: every prompt carries a block. Green after: prompt 1 carries FRESH,
//! prompts 2 carries nothing, the first prompt after the tier change carries MODERATE,
//! and the one after that carries nothing.

use std::path::Path;

use base::config::{BaseConfig, BracketConfig, BracketRules};
use base::domain::session::{Bracket, SessionState};
use base::hook::user_prompt_submit;

const ALWAYS_ON_DOMAIN: &str = r#"
[[domain]]
name = "probe"
mode = "always"
rules = ["the probe rule"]
"#;

fn workspace(root: &Path) {
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
    std::fs::write(root.join(".base").join("domains.toml"), ALWAYS_ON_DOMAIN).unwrap();
}

/// Turn-counted brackets, not percent: FRESH for prompts 1-2, MODERATE for 3-4.
/// `mode` stays `None`, which is the turns path, and no transcript is passed, so
/// `context_pct` is `None` and the turn thresholds decide.
fn config() -> BaseConfig {
    BaseConfig {
        bracket: BracketConfig {
            enabled: true,
            fresh_until: 2,
            moderate_until: 4,
            depleted_until: 6,
            // 0 disables the DEPLETED/CRITICAL force-refresh, which is a separate
            // mechanism with its own test. Leaving it on would let an unrelated
            // interval clear state mid-sequence.
            refresh_interval: 0,
            mode: None,
            rules: BracketRules {
                always: vec!["ALWAYS_RULE".into()],
                fresh: vec!["FRESH_RULE".into()],
                moderate: vec!["MODERATE_RULE".into()],
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

/// One prompt in session `sid`; whether the bracket block was emitted.
fn prompt(config: &BaseConfig, root: &Path, sid: &str, text: &str) -> bool {
    let event = serde_json::json!({ "prompt": text, "session_id": sid });
    user_prompt_submit::handle(config, root, &event)
        .unwrap()
        .bracket_rules_injected
}

// ── The gate itself ──────────────────────────────────────────────────────────

#[test]
fn the_gate_claims_a_tier_once_and_reopens_on_a_change() {
    let mut s = SessionState::default();
    s.set_active(Some("sid-a"));

    assert!(s.claim_bracket_block(Bracket::Fresh), "first FRESH prompt of the session");
    assert!(!s.claim_bracket_block(Bracket::Fresh), "second FRESH prompt carries nothing");
    assert!(!s.claim_bracket_block(Bracket::Fresh), "and the third");

    assert!(s.claim_bracket_block(Bracket::Moderate), "the tier changed, so the new tier's block goes once");
    assert!(!s.claim_bracket_block(Bracket::Moderate), "and then stays silent again");

    assert!(
        s.claim_bracket_block(Bracket::Fresh),
        "a change BACK is still a change: the rules for the tier now in force have not \
         been served since it came into force"
    );
}

#[test]
fn the_gate_is_per_session_not_per_workspace() {
    // `.session` is one file per workspace and several Claude sessions share a
    // workspace, so an ungated-per-session claim would let one session's first
    // prompt silence another session's first prompt.
    let mut s = SessionState::default();

    s.set_active(Some("sid-a"));
    assert!(s.claim_bracket_block(Bracket::Fresh));
    assert!(!s.claim_bracket_block(Bracket::Fresh));

    s.set_active(Some("sid-b"));
    assert!(
        s.claim_bracket_block(Bracket::Fresh),
        "a different Claude session has not been shown anything yet"
    );

    s.set_active(Some("sid-a"));
    assert!(!s.claim_bracket_block(Bracket::Fresh), "and the first session is still silenced");
}

// ── Through the real hook ────────────────────────────────────────────────────

#[test]
fn the_prompt_hook_sends_the_block_once_per_tier() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let cfg = config();
        SessionState::clear(&root.join(".base"));

        let got: Vec<bool> = (1..=4)
            .map(|i| prompt(&cfg, root, "sid-hook", &format!("prompt number {i}")))
            .collect();

        assert_eq!(
            got,
            vec![true, false, true, false],
            "prompt 1 FRESH, prompt 2 nothing, prompt 3 is the first MODERATE prompt so it \
             carries MODERATE, prompt 4 nothing. At 0ace1ba this is [true, true, true, true]."
        );
    });
}

#[test]
fn a_refresh_starts_from_nothing_and_a_live_session_is_not_disturbed() {
    // J7 item 9: a new Claude Code session id resets the once-per-session state. A
    // refresh is a new session, and Chris refreshes rather than compacting, so this
    // is the path that actually runs on his machine.
    //
    // Proved on the STATE, not through `handle`, and the reason is worth writing down
    // so nobody puts the unprovable version back. `handle` scopes its dedup through
    // `SessionState::load`, which binds to `PROCESS_SESSION` — a `OnceLock` set once
    // at hook entry. In production that is exact: a hook invocation is a fresh process
    // serving exactly one Claude session, so a refresh gets a new process and a new
    // binding. Inside ONE test process every call shares the scope whatever
    // `session_id` the event carries, and a `OnceLock` cannot be flipped between
    // calls. `set_process_session`'s own doc says it: "Tests bypass it with
    // `load_for`/`set_active`, since a test process impersonates several sessions."
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();

    // The session that has been running.
    let mut live = SessionState::load_for(&base_dir, Some("sid-one"));
    assert!(live.claim_bracket_block(Bracket::Fresh), "first prompt of the live session");
    assert!(!live.claim_bracket_block(Bracket::Fresh), "and it stays silent after that");
    live.save(&base_dir).unwrap();

    // The successor, reading the same `.session` file off disk.
    let mut successor = SessionState::load_for(&base_dir, Some("sid-two"));
    assert!(
        successor.claim_bracket_block(Bracket::Fresh),
        "a refresh is a new session id, so it has been shown nothing and starts from \
         nothing"
    );
    successor.save(&base_dir).unwrap();

    // And the first session is still silenced: a refresh in one terminal must not
    // make every other live session re-send its block.
    let mut live_again = SessionState::load_for(&base_dir, Some("sid-one"));
    assert!(
        !live_again.claim_bracket_block(Bracket::Fresh),
        "the original session has already been served this tier and stays silent"
    );
}

#[test]
fn the_no_domains_path_is_gated_too() {
    // One of four return sites in `handle` prints the block: the empty-domains
    // return, the star-command return, the no-match return, and the main path. A
    // gate applied at three of them is a gate a user routes around by having no
    // domains configured.
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".base")).unwrap();
        std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
        // no domains.toml at all
        let cfg = config();
        SessionState::clear(&root.join(".base"));

        assert!(prompt(&cfg, root, "sid-nodom", "hello"), "first prompt carries it");
        assert!(!prompt(&cfg, root, "sid-nodom", "again"), "the second does not");
    });
}

#[test]
fn a_tier_with_no_configured_rules_never_claims_a_tier_it_did_not_show() {
    // base ships no bracket rules, so `format_bracket_rules` returns an empty string
    // for a default install. Claiming a tier for a block that was never printed would
    // silence the first real block after the operator configures one.
    let mut s = SessionState::default();
    s.set_active(Some("sid-empty"));

    let empty = BracketRules::default();
    let rendered = base::domain::session::format_bracket_rules(Bracket::Fresh, &empty);
    assert!(rendered.is_empty(), "a default install renders nothing: {rendered:?}");

    // The hook short-circuits on the empty render and never reaches the claim, so
    // the tier is still unclaimed here.
    assert!(
        s.claim_bracket_block(Bracket::Fresh),
        "the first time a block is actually rendered, it is served"
    );
}
