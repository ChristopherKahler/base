//! `petrel`'s FINDING 1 on `5c099d1`: the two hooks must serve a rule at the SAME bracket tier.
//!
//! The prompt hook reads its tier from the transcript's real percentage. The tool hook passed `None` and so fell
//! back to the prompt count. Both write one per-rule record, and a record re-opens whenever its stored tier
//! differs. In percent mode the two tiers disagree for most of a session, so every switch between a prompt and a
//! tool call served the same rules again: the per-rule dedup of F9 undone through the tier.
//!
//! The operator's live `base.toml` is percent mode, with tiers at 20, 40 and 60 percent and a prompt-count
//! fallback of 7, 20 and 30 prompts. At prompt 3 with 25 percent used, the prompt hook says MODERATE and the tool
//! hook said FRESH.
//!
//! Red at `5c099d1`, where the tool hook computes its own tier. Green once the tool hook serves at the tier the
//! prompt hook stored.

use std::path::Path;

use base::config::{BaseConfig, BracketConfig, NamespaceConfig};
use base::crud;
use base::domain::session::SessionState;
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

/// A transcript whose newest usage block puts `used` tokens in the window.
fn transcript(root: &Path, used: u64) -> String {
    let path = root.join("transcript.jsonl");
    let line = serde_json::json!({ "message": { "usage": { "input_tokens": used } } });
    std::fs::write(&path, format!("{line}\n")).unwrap();
    path.display().to_string()
}

#[test]
fn before_any_prompt_the_tool_hook_serves_at_the_prompt_count_tier() {
    // `auk`'s added case: a tool call before the session's first prompt, so no prompt has stored a tier. The tool
    // hook falls back to the prompt count, which is what it always did. GREEN in both arms by design: it pins the
    // fallback so the fix cannot silently change it.
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let config = BaseConfig {
            namespace: ns(),
            bracket: BracketConfig { mode: Some("percent".into()), context_window: 1000, ..Default::default() },
            ..Default::default()
        };
        SessionState::clear(&root.join(".base"));
        crud::rule::add(root, &ns(), "probe", "EARLY RULE", None).unwrap();

        let tool = serde_json::json!({
            "tool_name": "Read",
            "session_id": "sid-early",
            "tool_input": { "file_path": root.join("work").join("x.md").display().to_string() },
        });
        let out = pre_tool_use::handle(&config, root, &tool).unwrap().1;
        assert!(out.contains("EARLY RULE"), "control: a tool call before any prompt still serves the rule: {out}");

        let state = SessionState::load(&root.join(".base"));
        let tiers: Vec<&str> = state.rules_shown.values().map(|r| r.tier.as_str()).collect();
        assert_eq!(tiers, ["FRESH"], "no prompt has stored a tier, so the prompt count (0) decides: {tiers:?}");
    });
}

#[test]
fn the_tool_hook_serves_at_the_tier_the_prompt_hook_computed() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        // Percent mode. 250 of 1,000 tokens is 25 percent: MODERATE by percent (20 < 25 <= 40).
        // The prompt-count bands keep prompt 1 FRESH, so the two ways of computing a tier disagree.
        let config = BaseConfig {
            namespace: ns(),
            bracket: BracketConfig {
                mode: Some("percent".into()),
                context_window: 1000,
                fresh_until_pct: 20.0,
                moderate_until_pct: 40.0,
                depleted_until_pct: 60.0,
                fresh_until: 7,
                moderate_until: 20,
                depleted_until: 30,
                ..Default::default()
            },
            ..Default::default()
        };
        SessionState::clear(&root.join(".base"));
        crud::rule::add(root, &ns(), "probe", "ONLY RULE", None).unwrap();

        let tpath = transcript(root, 250);
        let prompt = serde_json::json!({ "prompt": "probe", "session_id": "sid-tier", "transcript_path": tpath });
        let served = user_prompt_submit::handle(&config, root, &prompt).unwrap();
        assert_eq!(served.rules_injected, 1, "positive control: the prompt hook serves the rule once");

        // Positive control that the PERCENT path was taken, read off the record the prompt hook wrote. Without
        // it, a transcript the reader could not parse would fall back to turns, both hooks would agree on FRESH,
        // and the assertion below would pass over a case that never occurred.
        let state = SessionState::load(&root.join(".base"));
        let tiers: Vec<&str> = state.rules_shown.values().map(|r| r.tier.as_str()).collect();
        assert_eq!(tiers, ["MODERATE"], "the prompt hook must have served at the percent tier: {tiers:?}");

        // A second rule, so the tool call below demonstrably reaches rule serving. Its absence of ONLY RULE is
        // then a reading, not a hook that served nothing at all.
        crud::rule::add(root, &ns(), "probe", "SECOND RULE", None).unwrap();
        let tool = serde_json::json!({
            "tool_name": "Read",
            "session_id": "sid-tier",
            "tool_input": { "file_path": root.join("work").join("x.md").display().to_string() },
        });
        let out = pre_tool_use::handle(&config, root, &tool).unwrap().1;
        assert!(out.contains("SECOND RULE"), "control: the tool hook reached rule serving: {out}");
        assert!(
            !out.contains("ONLY RULE"),
            "the tool hook served ONLY RULE again. It computed its tier from the prompt count (FRESH) while the \
             prompt hook stored MODERATE, and a tier that differs re-opens the record: {out}"
        );
    });
}
