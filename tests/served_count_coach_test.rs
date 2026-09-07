//! #62 and #65 at the surface a person sees: the binary, driven the way the shell and the
//! hooks drive it. First test in the tree to run `CARGO_BIN_EXE_base`. The fake home is a
//! tempdir (outside the real home, or the I8 guard refuses every write), `BASE_HOME` points
//! at it, and the cwd sits inside it so the workspace tier resolves there too. The home is
//! also the workspace root, the shape `tests/injection_scope_test.rs` uses.
//!
//! Red on 0.14.1: the sync line read `1 domains, 0 rules, 0 decisions` beside a `domain get`
//! that counted five, and a decision the `[X CONTEXT]` block listed came back a second time
//! under `<base-context>` in the same prompt.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_base");

/// One triggered domain with zero `domains.toml` rules; nothing always-on.
fn home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".base-gbl").join(".base")).unwrap();
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::create_dir_all(root.join("genai").join("vp-operators")).unwrap();
    std::fs::write(
        root.join(".base-gbl").join("domains.toml"),
        "[[domain]]\nname = \"alpha\"\nmode = \"triggered\"\nprompt_keywords = [\"alpha\"]\nrules = []\n",
    )
    .unwrap();
    std::fs::write(root.join(".base-gbl").join("base.toml"), "[update]\nauto = false\n").unwrap();
    std::fs::write(
        root.join(".base").join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n",
    )
    .unwrap();
    tmp
}

fn base(root: &Path) -> Command {
    let mut c = Command::new(BIN);
    c.current_dir(root).env("BASE_HOME", root).env("BASE_NO_AUTO_UPDATE", "1");
    c
}

/// stdout + stderr of one invocation.
fn run(root: &Path, args: &[&str]) -> String {
    let out = base(root).args(args).output().expect("base runs");
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// One prompt in session `sid`, the hook's stdout.
fn fire(root: &Path, sid: &str, text: &str) -> String {
    let event = serde_json::json!({ "session_id": sid, "cwd": root.display().to_string(), "prompt": text });
    let mut child = base(root)
        .args(["hook", "user-prompt-submit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hook runs");
    child.stdin.take().unwrap().write_all(event.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Prompts 1-2 are lean (rules only); the neighbourhood and the walk arrive on prompt 3.
fn third(root: &Path, sid: &str, text: &str) -> String {
    fire(root, sid, "hello");
    fire(root, sid, "hello again");
    fire(root, sid, text)
}

/// The row `log_hook_event` writes for a PreToolUse on `rel` in session `sid`: what a real
/// tool call leaves behind, and what a path trigger reads since 0.14.1.
fn touch(root: &Path, sid: &str, rel: &str) {
    let row = serde_json::json!({
        "ts": "2026-09-07T12:00:00-05:00", "hook": "pre-tool-use", "success": true,
        "session_id": sid, "tool_name": "Read", "file_path": root.join(rel).display().to_string(),
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(".base").join("hook-events.jsonl"))
        .unwrap();
    writeln!(f, "{row}").unwrap();
}

fn lines_with(text: &str, needle: &str) -> usize {
    text.lines().filter(|l| l.contains(needle)).count()
}

/// #62. Five CLI rules on a domain with no `domains.toml` rules: sync imported none and
/// says so, and `domain get` still counts all five (#38 stays in force).
#[test]
fn domain_sync_says_what_it_counted_and_domain_get_counts_cli_rules() {
    let tmp = home();
    let root = tmp.path();
    for i in 1..=5 {
        run(root, &["rule", "add", "--domain", "alpha", "--text", &format!("cli rule {i}")]);
    }
    let sync = run(root, &["domain", "sync"]);
    assert_eq!(sync.trim(), "Domain sync complete: 1 domains, 0 rules imported from domains.toml", "{sync}");
    let get = run(root, &["domain", "get", "alpha"]);
    assert!(get.contains("Rules (5):"), "{get}");
}

/// #62. With a carl.json, its rules count too and the line names both sources.
#[test]
fn domain_sync_with_carl_names_both_sources() {
    let tmp = home();
    let root = tmp.path();
    let carl = root.join("carl.json");
    std::fs::write(
        &carl,
        r#"{ "domains": [ { "name": "alpha", "rules": [ {"text": "carl rule one"}, {"text": "carl rule two"} ],
                          "decisions": [ {"decision": "Pick x over y", "rationale": "cheaper"} ] } ] }"#,
    )
    .unwrap();
    let sync = run(root, &["domain", "sync", "--carl", &carl.display().to_string()]);
    assert_eq!(
        sync.trim(),
        "Domain sync complete: 1 domains, 2 rules imported from domains.toml and carl.json, 1 decisions imported from carl.json",
        "{sync}"
    );
}

/// #65. The `[vp-operators CONTEXT]` block lists the decision; the walk from the name the
/// prompt used (the domain node, `resolve_strict` is kind-first) must not list it again.
/// On 0.14.1 the decision row appeared twice: `- Decision: …` and `decision  … — hasDecision`.
#[test]
fn a_decision_the_domain_block_listed_is_not_listed_again_by_the_walk() {
    let tmp = home();
    let root = tmp.path();
    let path = root.join("genai").join("vp-operators").display().to_string();
    run(root, &["project", "add", "--name", "vp-operators", "--path", &path]);
    run(root, &["decision", "log", "--domain", "vp-operators", "--decision", "Use Seedance for b-roll", "--rationale", "cheapest per clip"]);
    touch(root, "s65a", "genai/vp-operators/x.md");
    let out = third(root, "s65a", "how is `vp-operators` going");
    assert!(out.contains("[vp-operators CONTEXT]"), "the domain block must be on prompt 3:\n{out}");
    assert!(out.contains("  - Decision: Use Seedance for b-roll"), "{out}");
    assert_eq!(lines_with(&out, "Use Seedance for b-roll"), 1, "the decision was served more than once:\n{out}");
    // The served root still walks (its attachments were never served, only its row).
    assert!(out.contains("<base-context name=\"vp-operators\""), "a served root must still walk:\n{out}");
}

/// #65. Naming the served decision itself: nothing it is attached to is new (its domain is
/// the block's own header), so no walk header comes back for it. On 0.14.1 it did.
#[test]
fn a_served_decision_named_in_the_prompt_gets_no_second_serving() {
    let tmp = home();
    let root = tmp.path();
    let path = root.join("genai").join("vp-operators").display().to_string();
    run(root, &["project", "add", "--name", "vp-operators", "--path", &path]);
    run(root, &["decision", "log", "--domain", "vp-operators", "--decision", "Use Seedance for b-roll", "--rationale", "cheapest per clip"]);
    touch(root, "s65b", "genai/vp-operators/x.md");
    let out = third(root, "s65b", "why did we pick `Use Seedance for b-roll`");
    assert!(out.contains("  - Decision: Use Seedance for b-roll"), "{out}");
    assert!(!out.contains("<base-context name=\"Use Seedance for b-roll\""), "served decision came back under a walk header:\n{out}");
    assert_eq!(lines_with(&out, "Use Seedance for b-roll"), 1, "{out}");
}
