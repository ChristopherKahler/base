//! BO-03, F5 and F14b — what prompt-submit serves of the decisions, driven through the binary as Claude Code
//! drives it (JSON on stdin, a temporary home).
//!
//! F5 (locked under D12): a global decision, one filed under an always-on domain such as GLOBAL, is served on a
//! prompt only when the prompt contains one of its keywords; one with no keywords is served at session start
//! only. Before BO-03 every global decision rode every prompt's `[GLOBAL CONTEXT]`: on 2026-10-01 a prompt about
//! the prompt hook being cut off received five decisions about profile mirroring, a second account, security
//! practice, the auto-memory archive and grazer (Example 2, the decisions below).
//!
//! F14b: prompt-submit never serves a superseded record (`src/cli.rs` says the serving surfaces return only the
//! live version; Example 4 pins it on the prompt).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_base");

/// Example 2's five global decisions, as they read on the operator's store, and the keywords proposed for them.
const MIRROR: &str = "Mirror profile A into profile B with Windows junctions for the shareable dirs";
const TWO_ACCOUNTS: &str = "Run two Claude Code accounts on one machine by pointing CLAUDE_CONFIG_DIR at a second config dir";
const SECURITY: &str = "Security, compliance and DevOps practice is product-agnostic";
const MEMORY: &str = "The Claude Code auto-memory store is ARCHIVED and MEMORY.md no longer loads";
const GRAZER: &str = "grazer replaces Claude in Chrome as the default browser tool";

const EXAMPLE_TWO: [(&str, &str); 5] = [
    (MIRROR, "CLAUDE_CONFIG_DIR, second profile, junction, profile B"),
    (TWO_ACCOUNTS, "CLAUDE_CONFIG_DIR, two accounts, second account"),
    (SECURITY, "security, compliance, devops, guardrails"),
    (MEMORY, ""),
    (GRAZER, "grazer, browser, Claude in Chrome, Brave"),
];

/// The prompt that received all five on 2026-10-01 (audit prompt 3, as the fork quotes it).
const PROMPT_THREE: &str =
    "the user prompt submit being cut off at a high rate, look at the hook injections and what they carry";

struct Ws {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    ws: PathBuf,
}

/// A home whose global config puts every prompt past FRESH, so the domain context is not skipped as it is on a
/// session's first two FRESH prompts (lean mode), and a workspace with the GLOBAL domain and `extra` domains.
fn workspace(extra_domains: &str) -> Ws {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let ws = home.join("work");
    std::fs::create_dir_all(home.join(".base-gbl")).unwrap();
    std::fs::write(
        home.join(".base-gbl").join("base.toml"),
        "[bracket]\nenabled = true\nfresh_until = 0\nmoderate_until = 50\n",
    )
    .unwrap();
    std::fs::create_dir_all(ws.join(".base")).unwrap();
    std::fs::write(ws.join(".base").join("graph.nq"), "").unwrap();
    std::fs::write(
        ws.join(".base").join("domains.toml"),
        format!("[[domain]]\nname = \"GLOBAL\"\nmode = \"always\"\nrules = [\"the global rule\"]\n{extra_domains}"),
    )
    .unwrap();
    Ws { _tmp: tmp, home, ws }
}

fn run(w: &Ws, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut child = Command::new(BIN)
        .args(args)
        .current_dir(&w.ws)
        .env("BASE_HOME", &w.home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_NO_SPAWN", "1")
        .env("BASE_NO_WAKE_NUDGE", "1")
        .env("BASE_NO_AUTONAME", "1")
        .env_remove("BASE_RELAY_AS")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CLAUDE_CONFIG_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the base binary runs");
    let mut pipe = child.stdin.take().expect("stdin");
    if let Some(text) = stdin {
        pipe.write_all(text.as_bytes()).expect("stdin written");
    }
    drop(pipe);
    let out = child.wait_with_output().expect("base finishes");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn base(w: &Ws, args: &[&str]) -> String {
    let (code, out, err) = run(w, args, None);
    assert_eq!(code, 0, "base {args:?} failed: {err}\n{out}");
    out
}

/// `base decision log`; the slug it printed.
fn log(w: &Ws, domain: &str, text: &str) -> String {
    let out = base(w, &["decision", "log", "--domain", domain, "--decision", text, "--rationale", "fixture"]);
    out.split("slug: ")
        .nth(1)
        .and_then(|s| s.split(')').next())
        .unwrap_or_else(|| panic!("no slug in {out:?}"))
        .to_string()
}

fn keywords(w: &Ws, slug: &str, list: &str) {
    base(w, &["decision", "update", slug, "--keywords", list]);
}

/// One prompt, as the first prompt of session `session`; what the hook printed.
fn prompt(w: &Ws, text: &str, session: &str) -> String {
    let payload = serde_json::json!({
        "cwd": w.ws.display().to_string(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": text,
        "session_id": session,
    })
    .to_string();
    let (code, out, err) = run(w, &["hook", "user-prompt-submit"], Some(&payload));
    assert_eq!(code, 0, "the prompt hook failed: {err}");
    out
}

fn session_start(w: &Ws, session: &str) -> String {
    let payload = serde_json::json!({
        "cwd": w.ws.display().to_string(),
        "hook_event_name": "SessionStart",
        "session_id": session,
    })
    .to_string();
    let (code, out, err) = run(w, &["hook", "session-start"], Some(&payload));
    assert_eq!(code, 0, "session start failed: {err}");
    out
}

fn example_two(w: &Ws) {
    for (text, kws) in EXAMPLE_TWO {
        let slug = log(w, "GLOBAL", text);
        if !kws.is_empty() {
            keywords(w, &slug, kws);
        }
    }
}

fn carries(out: &str) -> Vec<&'static str> {
    EXAMPLE_TWO.iter().map(|(t, _)| *t).filter(|t| out.contains(t)).collect()
}

#[test]
fn global_decision_served_only_on_keyword() {
    let w = workspace("");
    example_two(&w);

    // Example 2, one direction: prompt 3 names none of the keywords, so it gets no global decision at all.
    let three = prompt(&w, PROMPT_THREE, "s-three");
    assert!(three.contains("the global rule"), "control: the always-on domain matched and its rules printed:\n{three}");
    assert!(carries(&three).is_empty(), "no keyword, no global decision:\n{three}");
    assert!(!three.contains("[GLOBAL CONTEXT]"), "and no context block at all:\n{three}");

    // The other direction: a prompt carrying keywords gets exactly the decisions they name.
    let accounts = prompt(&w, "set up a second account with CLAUDE_CONFIG_DIR on the laptop", "s-accounts");
    assert_eq!(carries(&accounts), [MIRROR, TWO_ACCOUNTS], "{accounts}");
    let browser = prompt(&w, "open it in Brave, not Claude in Chrome", "s-browser");
    assert_eq!(carries(&browser), [GRAZER], "{browser}");

    // The walk reaches the GLOBAL domain's decisions from the domain's own name; it meets the same rule.
    let named = prompt(&w, "what does GLOBAL say about the hook injections", "s-named");
    assert!(carries(&named).is_empty(), "naming the domain does not bring its decisions:\n{named}");
}

#[test]
fn global_decision_without_keywords_only_at_session_start() {
    let w = workspace("");
    example_two(&w);

    let start = session_start(&w, "s-start");
    // The block, from its header to the blank line after it. Other session-start blocks (recent activity) list
    // decisions on their own terms; F5 governs this one.
    let block: String = start
        .lines()
        .skip_while(|l| !l.starts_with("[GLOBAL CONTEXT · decisions with no keywords"))
        .take_while(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(block.contains(MEMORY), "the decision with no keywords is at session start, under its header:\n{start}");
    for kept in [MIRROR, TWO_ACCOUNTS, SECURITY, GRAZER] {
        assert!(!block.contains(kept), "a decision WITH keywords waits for one: {kept}\n{block}");
    }

    // Never on a prompt: not on its own words, not by naming its domain, not on any keyword of another decision.
    for (i, text) in [
        "the auto-memory store and MEMORY.md, is it ARCHIVED?",
        "what does GLOBAL decide about memory",
        "set up a second account with CLAUDE_CONFIG_DIR, with grazer in Brave, and the security guardrails",
    ]
    .iter()
    .enumerate()
    {
        let out = prompt(&w, text, &format!("s-never-{i}"));
        assert!(!out.contains(MEMORY), "{text:?} received the keywordless decision:\n{out}");
    }
    let all = prompt(&w, "second profile, second account, grazer, security", "s-all");
    assert_eq!(carries(&all), [MIRROR, TWO_ACCOUNTS, SECURITY, GRAZER], "control: the others do arrive:\n{all}");
}

#[test]
fn decision_update_sets_keywords() {
    let w = workspace("");
    let slug = log(&w, "GLOBAL", GRAZER);
    assert_eq!(slug, "global.grazer-replaces-claude-in-chrome-as-the-default-browser-tool");

    // Example 3.
    let out = base(&w, &["decision", "update", &slug, "--keywords", "grazer, browser, Claude in Chrome, Brave"]);
    assert!(out.contains("keywords: grazer, browser, Claude in Chrome, Brave"), "{out}");
    let read = |w: &Ws| -> Vec<String> {
        let json = base(w, &["decision", "search", "--keyword", "grazer", "--json"]);
        let rows: serde_json::Value = serde_json::from_str(&json).unwrap();
        let mut k: Vec<String> =
            rows[0]["keywords"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        k.sort();
        k
    };
    assert_eq!(read(&w), ["Brave", "Claude in Chrome", "browser", "grazer"]);

    // A second update REPLACES the list (the alias `u` addresses the same command).
    base(&w, &["decision", "u", &slug, "--keywords", "grazer"]);
    assert_eq!(read(&w), ["grazer"]);

    // An empty list clears it, and the decision goes back to session start only.
    let cleared = base(&w, &["decision", "update", &slug, "--keywords", ""]);
    assert!(cleared.contains("keywords cleared"), "{cleared}");
    assert!(read(&w).is_empty());
    assert!(!prompt(&w, "grazer please", "s-cleared").contains(GRAZER), "no keywords, no prompt");
}

#[test]
fn superseded_decision_never_served_on_prompt() {
    // Example 4, through both surfaces that serve a decision on a prompt: a keyword domain's context, and a
    // global decision's keyword.
    let w = workspace("[[domain]]\nname = \"net\"\nmode = \"triggered\"\nprompt_keywords = [\"port\"]\n");
    let a = log(&w, "net", "use port 8080");
    let b = log(&w, "net", "use port 9090");
    let ga = log(&w, "GLOBAL", "global port is 7070");
    let gb = log(&w, "GLOBAL", "global port is 6060");
    keywords(&w, &ga, "port");
    keywords(&w, &gb, "port");

    let before = prompt(&w, "which port do we use", "s-before");
    for live in ["use port 8080", "use port 9090", "global port is 7070", "global port is 6060"] {
        assert!(before.contains(live), "control: before the supersede, {live:?} is served:\n{before}");
    }

    base(&w, &["graph", "supersede", &a, &b]);
    base(&w, &["graph", "supersede", &ga, &gb]);
    let after = prompt(&w, "which port do we use", "s-after");
    assert!(after.contains("use port 9090") && after.contains("global port is 6060"), "the live ones:\n{after}");
    assert!(!after.contains("use port 8080"), "the superseded domain decision:\n{after}");
    assert!(!after.contains("global port is 7070"), "the superseded global decision:\n{after}");

    // Named directly, a superseded decision answers with its successor, never with itself.
    let named = prompt(&w, "what about `net` and its port", "s-named");
    assert!(!named.contains("use port 8080"), "{named}");
}

#[test]
fn a_global_decision_in_the_global_tier_takes_keywords_with_global() {
    // One of the operator's six global decisions lives in the global tier: `base decision --global update`.
    let w = workspace("");
    std::fs::create_dir_all(w.home.join(".base-gbl").join(".base")).unwrap();
    let out = base(&w, &["decision", "--global", "log", "--domain", "GLOBAL", "--decision", SECURITY, "--rationale", "r"]);
    let slug = out.split("slug: ").nth(1).and_then(|s| s.split(')').next()).unwrap().to_string();
    base(&w, &["decision", "--global", "update", &slug, "--keywords", "security, guardrails"]);
    let hit = prompt(&w, "check the security guardrails", "s-gt");
    assert!(hit.contains(SECURITY), "{hit}");
    assert!(!prompt(&w, PROMPT_THREE, "s-gt2").contains(SECURITY), "and not without a keyword");
}

/// A quad in the workspace's own graph, `graph/ws/work` (the workspace folder is `work`).
fn append_quad(w: &Ws, s: &str, p: &str, o: &str) {
    const NS: &str = "http://ops-sys.local/ontology#";
    let graph = w.ws.join(".base").join("graph.nq");
    let mut text = std::fs::read_to_string(&graph).unwrap();
    text.push_str(&format!("<{NS}{s}> <{NS}{p}> <{NS}{o}> <{NS}graph/ws/work> .\n"));
    std::fs::write(&graph, text).unwrap();
}

#[test]
fn a_global_decision_is_served_once_per_session() {
    // Code review, 2026-10-02: the decisions a prompt names change with each prompt, so they are their own block,
    // each decision recorded as shown when it prints, like a rule; the domain's CONTEXT never changes because of
    // them, so it is never re-sent because of them either.
    let w = workspace("");
    example_two(&w);
    assert!(prompt(&w, "grazer please", "s-once").contains(GRAZER), "the first time");
    assert!(!prompt(&w, "grazer again", "s-once").contains(GRAZER), "not twice in a session");
    let other = prompt(&w, "open Brave and set up a second account", "s-once");
    assert!(!other.contains(GRAZER), "not on another of its keywords either:\n{other}");
    assert!(other.contains(TWO_ACCOUNTS), "while a decision not yet shown still goes:\n{other}");
    assert!(prompt(&w, "grazer please", "s-other").contains(GRAZER), "a new session has been shown nothing");
}

#[test]
fn a_decision_filed_under_a_matched_domain_too_is_that_domains_as_well() {
    // Code review, 2026-10-02: F5 governs an always-on domain's decisions. One also filed under a keyword domain is
    // that domain's too: its CONTEXT keeps it when the domain matches, and it is not a session-start-only record.
    let w = workspace("[[domain]]\nname = \"widgets\"\nmode = \"triggered\"\nprompt_keywords = [\"widgets\"]\n");
    let slug = log(&w, "GLOBAL", MEMORY);
    append_quad(&w, "domain/widgets", "hasDecision", &format!("decision/{slug}"));
    let hit = prompt(&w, "how are the widgets doing", "s-widgets");
    assert!(hit.contains("[widgets CONTEXT]") && hit.contains(MEMORY), "the matched domain keeps it:\n{hit}");
    assert!(!prompt(&w, PROMPT_THREE, "s-none").contains(MEMORY), "and GLOBAL alone does not serve it");
    let start = session_start(&w, "s-start");
    assert!(!start.contains("decisions with no keywords"), "it reaches prompts, so it is not listed there:\n{start}");
}

#[test]
fn a_decision_under_two_always_on_domains_is_listed_under_the_first_by_name() {
    let w = workspace("[[domain]]\nname = \"ALSO\"\nmode = \"always\"\n");
    let slug = log(&w, "GLOBAL", MEMORY);
    append_quad(&w, "domain/also", "hasDecision", &format!("decision/{slug}"));
    for i in 0..3 {
        let start = session_start(&w, &format!("s-two-{i}"));
        assert!(
            start.contains(&format!("[ALSO CONTEXT · decisions with no keywords, shown at session start only]\n  - Decision: {MEMORY}")),
            "run {i}: one heading, the same every time:\n{start}"
        );
        assert!(!start.contains("[GLOBAL CONTEXT · decisions with no keywords"), "listed once:\n{start}");
    }
}

#[test]
fn decision_update_refuses_keywords_outside_its_tier() {
    // Code review, 2026-10-02: the keywords are written into this tier's graph. A decision whose record sits in
    // another graph is refused by name, and its keywords are left as they were, not deleted and reported as set.
    const NS: &str = "http://ops-sys.local/ontology#";
    let w = workspace("");
    let other = format!("<{NS}graph/ws/elsewhere>");
    let d = format!("<{NS}decision/global.moved-decision>");
    let quads = format!(
        "{d} <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{NS}Decision> {other} .\n\
         {d} <{NS}name> \"moved decision\" {other} .\n\
         {d} <{NS}rationale> \"r\" {other} .\n\
         {d} <{NS}decisionKeyword> \"kept\" {other} .\n\
         <{NS}domain/global> <{NS}hasDecision> {d} {other} .\n"
    );
    std::fs::write(w.ws.join(".base").join("graph.nq"), quads).unwrap();
    let (code, out, err) = run(&w, &["decision", "update", "global.moved-decision", "--keywords", "new"], None);
    assert_ne!(code, 0, "refused: {out}");
    assert!(err.contains("is not in this tier's graph"), "{err}");
    let json = base(&w, &["decision", "search", "--keyword", "moved", "--json"]);
    assert!(json.contains("\"kept\"") && !json.contains("\"new\""), "the keywords are as they were: {json}");
}
