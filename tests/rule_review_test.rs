//! BO-16 (K5, K6, D8): every rule proposal is replayed against the user's own recent prompts and reviewed one at a time.
//!
//! Each test builds its own home from the seed, writes the match log's prompt rows the way the prompt hook writes them
//! (or runs the hook itself where what it served matters), makes proposals with `base rule propose`, and drives
//! `base rule replay` and `base rule review` as the AI and the user would. Standard input is never a terminal here, so
//! review takes its flags or lists.

mod seed;

use std::path::PathBuf;
use std::process::Command;

fn home(tag: &str) -> seed::Seed {
    let root = std::env::temp_dir().join(format!("base-bo16-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    seed::write(&root, &seed::TINY, "")
}

fn base_env(s: &seed::Seed, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_base"))
        .args(args)
        .current_dir(&s.ws)
        .env("BASE_HOME", &s.home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_NO_SPAWN", "1")
        .env_remove("BASE_RELAY_AS")
        .env_remove("BASE_HEADLESS")
        .env_remove("WT_SESSION")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CLAUDECODE")
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::null())
        .output()
        .expect("base runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn ok(s: &seed::Seed, args: &[&str]) -> String {
    ok_env(s, args, &[])
}

fn ok_env(s: &seed::Seed, args: &[&str], env: &[(&str, &str)]) -> String {
    let (code, out, err) = base_env(s, args, env);
    assert_eq!(code, 0, "{args:?}\nstdout:\n{out}\nstderr:\n{err}");
    out
}

/// The command fails with exit 1; its stderr.
fn refused(s: &seed::Seed, args: &[&str]) -> String {
    let (code, out, err) = base_env(s, args, &[]);
    assert_eq!(code, 1, "{args:?} should be refused\nstdout:\n{out}\nstderr:\n{err}");
    err
}

fn domains_toml(s: &seed::Seed) -> PathBuf {
    s.ws.join(".base").join("domains.toml")
}

/// A `base` domain served on "hooks" and "doctor", with one rule of its own file.
fn base_domain(s: &seed::Seed) {
    std::fs::write(
        domains_toml(s),
        "[[domain]]\nname = \"base\"\nmode = \"triggered\"\nprompt_keywords = [\"hooks\", \"doctor\"]\n\
         rules = [\"Read the hook output before saying a block was cut.\"]\n",
    )
    .unwrap();
    // The graph copy `rule list` reads, as every hook makes it.
    ok(s, &["domain", "sync"]);
}

/// Prompt rows in the workspace's match log, oldest first, in the shape the prompt hook writes them.
fn log_prompts(s: &seed::Seed, prompts: &[&str]) {
    let mut text = String::new();
    for (i, p) in prompts.iter().enumerate() {
        let row = serde_json::json!({
            "ts": format!("2026-10-01T{:02}:{:02}:00-05:00", 9 + i / 60, i % 60),
            "session": "b016b016-0000-4000-8000-000000000001",
            "event": "prompt",
            "prompt_num": i + 1,
            "text": p,
            "matched": [], "served": [], "cut": [], "scores": [],
        });
        text.push_str(&row.to_string());
        text.push('\n');
    }
    std::fs::write(s.ws.join(".base").join("match-log.jsonl"), text).unwrap();
}

/// Twenty prompts: two about the user prompt submit hook, one about doctor, one task notification, the rest elsewhere.
fn twenty(s: &seed::Seed) {
    let mut p: Vec<&str> = vec![
        "we're having some issues with the user prompt submit being cut off at a high rate",
        "what reminders do i need",
        "run doctor and tell me what it says",
        "<task-notification>\n<task-id>b1</task-id>\n<summary>relay wake</summary>\n</task-notification>",
        "how is the user prompt submit injection after BO-01?",
    ];
    let filler = [
        "plan the week", "draft the invoice email", "what is on the calendar", "summarize the meeting", "book the flight",
        "fix the spreadsheet totals", "send the update", "where is the contract", "order lunch", "call the bank",
        "check the mail", "review the budget", "write the agenda", "update the tracker", "reply to the vendor",
    ];
    p.extend(filler);
    log_prompts(s, &p);
}

/// The `<domain>.<id>` of the rule whose text holds `text`, from `base rule list`.
fn rule_ref(s: &seed::Seed, domain: &str, text: &str) -> String {
    let out = ok(s, &["rule", "list", "--domain", domain]);
    let line = out.lines().find(|l| l.contains(text)).unwrap_or_else(|| panic!("no rule '{text}' in:\n{out}"));
    let start = line.find(&format!("[{domain}.")).expect("a rule ref") + 1;
    let end = start + line[start..].find(']').expect("its end");
    line[start..end].to_string()
}

fn graph(s: &seed::Seed) -> String {
    std::fs::read_to_string(s.ws.join(".base").join("graph.nq")).unwrap_or_default()
}

/// The keyword-gap proposal of Example 1: "user prompt submit" for the `base` domain, given by hand.
fn propose_gap(s: &seed::Seed) -> String {
    let r = rule_ref(s, "base", "Read the hook output");
    ok(s, &[
        "rule", "propose", "--rule", &r, "--keywords", "user prompt submit",
        "--example", "we're having some issues with the user prompt submit being cut off",
    ])
}

/// Example 1: a keyword that serves two more of twenty prompts; the task notification is left out; dropping a keyword
/// stops the prompt it served.
#[test]
fn replay_reports_new_and_stopped() {
    let s = home("replay");
    base_domain(&s);
    twenty(&s);
    let out = propose_gap(&s);
    assert!(out.starts_with("proposal p-0001 · keyword gap · domain base"), "{out}");

    let out = ok(&s, &["rule", "replay", "p-0001"]);
    let want = "proposal p-0001 · keyword gap · domain base · add keyword \"user prompt submit\"\n\
                replayed 19 prompts (2026-10-01) · 1 task notification left out\n\
                \x20 newly served on 2 prompts, e.g.:\n\
                \x20   \"how is the user prompt submit injection after BO-01?\"\n\
                \x20   \"we're having some issues with the user prompt submit being cut off at a high rate\"\n\
                \x20 stops serving on 0\n\
                \x20 share: 3 / 19 = 15.8%\n";
    assert_eq!(out, want);

    let out = ok(&s, &["rule", "replay", "--domain", "base", "--drop-keyword", "doctor"]);
    assert!(out.starts_with("domain base · drop keyword \"doctor\"\n"), "{out}");
    assert!(out.contains("  newly served on 0\n  stops serving on 1 prompt, e.g.:\n    \"run doctor and tell me what it says\"\n"), "{out}");
    assert!(out.contains("  share: 0 / 19 = 0.0%\n"), "{out}");

    // A domain no domains.toml holds matches no prompt today; approving writes it with the keyword, so replay counts it.
    let out = ok(&s, &["rule", "replay", "--domain", "graph-only", "--add-keyword", "doctor"]);
    assert!(out.contains("  newly served on 1 prompt, e.g.:\n    \"run doctor and tell me what it says\"\n"), "{out}");
    // Under auto_inject = false no hook serves the domain's rules, so a new rule there is served on nothing.
    let toml = std::fs::read_to_string(domains_toml(&s)).unwrap();
    std::fs::write(domains_toml(&s), format!("{toml}\n[[domain]]\nname = \"quiet\"\nmode = \"triggered\"\nauto_inject = false\n")).unwrap();
    let out = ok(&s, &["rule", "propose", "--new", "--domain", "quiet", "--text", "Plan on Mondays.", "--keywords", "plan", "--example", "plan the week"]);
    assert!(out.starts_with("proposal p-0002 · new rule · domain quiet"), "{out}");
    let out = ok(&s, &["rule", "replay", "p-0002"]);
    assert!(out.contains("  newly served on 0\n") && out.contains("  share: 0 / 19 = 0.0%\n"), "{out}");

    // Nothing logged yet: said, not an error.
    let empty = home("replay-empty");
    base_domain(&empty);
    let out = ok(&empty, &["rule", "replay", "--domain", "base", "--add-keyword", "hooks"]);
    assert!(out.contains("no prompts in the match log yet: nothing to replay"), "{out}");
}

/// Example 2: a keyword on more than `[tune] broad_share` of the prompts is TOO BROAD; the limit is the setting's.
#[test]
fn replay_flags_too_broad() {
    let s = home("broad");
    base_domain(&s);
    let mut p: Vec<String> = (0..8).map(|i| format!("is the base build {i} done")).collect();
    p.extend((0..11).map(|i| format!("plan day {i} of the trip")));
    log_prompts(&s, &p.iter().map(String::as_str).collect::<Vec<_>>());
    let out = ok(&s, &["rule", "replay", "--domain", "base", "--add-keyword", "base"]);
    assert!(out.contains("  newly served on 8 prompts, e.g.:"), "{out}");
    assert!(out.contains("  share: 8 / 19 = 42.1%  TOO BROAD (limit 25%)\n"), "{out}");

    // An always-on domain is served everywhere before and after: a keyword adds nothing, so nothing is flagged.
    let toml = std::fs::read_to_string(domains_toml(&s)).unwrap();
    std::fs::write(domains_toml(&s), format!("{toml}\n[[domain]]\nname = \"everywhere\"\nmode = \"always\"\nrules = [\"Be brief.\"]\n")).unwrap();
    let out = ok(&s, &["rule", "replay", "--domain", "everywhere", "--add-keyword", "base"]);
    assert!(out.contains("  share: 19 / 19 = 100.0%\n") && !out.contains("TOO BROAD"), "{out}");

    let cfg = s.ws.join(".base").join("base.toml");
    let before = std::fs::read_to_string(&cfg).unwrap_or_default();
    std::fs::write(&cfg, format!("{before}\n[tune]\nbroad_share = 0.5\n")).unwrap();
    let out = ok(&s, &["rule", "replay", "--domain", "base", "--add-keyword", "base"]);
    assert!(out.contains("  share: 8 / 19 = 42.1%\n"), "under a 50% limit, not flagged:\n{out}");
}

/// K5d: approving a keyword gap writes the domain's file; a gap on a rule with words of its own adds to its words; a
/// new rule arrives with its keywords as its words and its prompt as its first fires_on test, which passes.
#[test]
fn review_approve_applies_change() {
    let s = home("approve");
    base_domain(&s);
    twenty(&s);
    propose_gap(&s);
    let out = ok(&s, &["rule", "review", "--approve", "p-0001"]);
    assert!(
        out.starts_with("approved p-0001: keyword \"user prompt submit\" added to domain base, in domains.toml (workspace tier)\n"),
        "{out}"
    );
    assert!(out.contains("  replay 15.8%"), "{out}");
    let toml = std::fs::read_to_string(domains_toml(&s)).unwrap();
    assert!(toml.contains("\"user prompt submit\""), "{toml}");
    assert!(graph(&s).contains("status> \"approved\""), "the status is stored");

    // A rule with words of its own takes the keywords as words.
    ok(&s, &["rule", "add", "--domain", "base", "--text", "Ping lynx before merging.", "--words", "relay ping"]);
    let r = rule_ref(&s, "base", "Ping lynx before merging.");
    let out = ok(&s, &["rule", "propose", "--rule", &r, "--keywords", "wake contract", "--example", "the wake contract is broken"]);
    assert!(out.starts_with("proposal p-0002 · keyword gap · rule base."), "{out}");
    let out = ok(&s, &["rule", "review", "--approve", "p-0002"]);
    assert!(out.contains("word \"wake contract\" added to rule base."), "{out}");
    let list = ok(&s, &["rule", "list", "--domain", "base"]);
    assert!(list.contains("relay ping") && list.contains("wake contract"), "{list}");

    // A new rule, with its test.
    let out = ok(&s, &[
        "rule", "propose", "--new", "--domain", "base", "--text", "Never read --version as the installed release.",
        "--keywords", "installed version", "--example", "what is the installed version",
    ]);
    assert!(out.starts_with("proposal p-0003 · new rule · domain base"), "{out}");
    let out = ok(&s, &["rule", "review", "--approve", "3"]);
    assert!(out.contains("new rule base.") && out.contains("tests: 1 fires-on · 0 quiet-on"), "{out}");
    let list = ok(&s, &["rule", "list", "--domain", "base"]);
    assert!(list.contains("Never read --version as the installed release."), "{list}");
    let test = ok(&s, &["rule", "test", "--domain", "base"]);
    assert!(test.contains("0 misses, 0 false fires"), "the new rule's own test passes:\n{test}");
}

/// The prompt hook serves `prompt` to `session`, so a proposal from that session reads the rule as served.
fn served_to(s: &seed::Seed, session: &str, prompt: &str) {
    let (code, _, err) = seed::run_prompt_submit(s, prompt, Some(session));
    assert_eq!(code, 0, "{err}");
}

/// K5d: a rewrite never overwrites. A graph rule's new wording supersedes it; a domains.toml rule moves to the graph
/// first (G0 question 2) and says so; a decision is superseded by a new one; a rewrite with no wording is refused.
#[test]
fn review_rewrite_supersedes_old_rule() {
    let s = home("rewrite");
    base_domain(&s);
    let sid = "b016b016-0000-4000-8000-000000000002";
    let env = [("CLAUDE_CODE_SESSION_ID", sid)];

    // A graph rule.
    ok(&s, &["rule", "add", "--domain", "base", "--text", "Deploy with the old script.", "--words", "deploy steps", "--fires-on", "what are the deploy steps"]);
    served_to(&s, sid, "what are the deploy steps");
    let r = rule_ref(&s, "base", "Deploy with the old script.");
    let out = ok_env(&s, &["rule", "propose", "--rule", &r, "--text", "Deploy with release.sh, never by hand.", "--example", "what are the deploy steps"], &env);
    assert!(out.starts_with("proposal p-0001 · rewrite · rule base."), "{out}");
    let out = ok(&s, &["rule", "review", "--approve", "p-0001"]);
    assert!(out.contains(&format!("rule {r} rewritten as base.")) && out.contains("superseded, not overwritten"), "{out}");
    let list = ok(&s, &["rule", "list", "--domain", "base"]);
    assert!(list.contains("Deploy with release.sh, never by hand.") && !list.contains("Deploy with the old script."), "{list}");
    let all = ok(&s, &["rule", "list", "--domain", "base", "--include-superseded"]);
    assert!(all.lines().any(|l| l.contains("Deploy with the old script.") && l.contains("[superseded]")), "{all}");
    let test = ok(&s, &["rule", "test", "--domain", "base"]);
    assert!(test.contains("0 misses"), "the new wording keeps the old rule's test, and passes it:\n{test}");

    // A domains.toml rule: served through its domain's keyword, then rewritten.
    served_to(&s, sid, "the hooks cut a block again");
    let r = rule_ref(&s, "base", "Read the hook output");
    let out = ok_env(&s, &["rule", "propose", "--rule", &r, "--text", "Read hook-output.jsonl before saying a block was cut.", "--example", "the hooks cut a block again"], &env);
    assert!(out.starts_with("proposal p-0002 · rewrite"), "{out}");
    let out = ok(&s, &["rule", "review", "--approve", "p-0002"]);
    assert!(out.contains("the rule moved from") && out.contains("to the graph"), "said plainly:\n{out}");
    let toml = std::fs::read_to_string(domains_toml(&s)).unwrap();
    assert!(!toml.contains("Read the hook output before saying a block was cut."), "the line left the file:\n{toml}");
    let all = ok(&s, &["rule", "list", "--domain", "base", "--include-superseded"]);
    assert!(all.lines().any(|l| l.contains("Read the hook output before saying") && l.contains("[superseded]")), "kept, superseded:\n{all}");
    assert!(all.lines().any(|l| l.contains("Read hook-output.jsonl before saying") && !l.contains("[superseded]")), "{all}");

    // A rewrite with no wording is refused, and stays pending.
    served_to(&s, sid, "what are the deploy steps now");
    let r = rule_ref(&s, "base", "Deploy with release.sh");
    let out = ok_env(&s, &["rule", "propose", "--rule", &r, "--keywords", "deploy steps", "--example", "what are the deploy steps now"], &env);
    assert!(out.starts_with("proposal p-0003 · rewrite"), "{out}");
    let err = refused(&s, &["rule", "review", "--approve", "p-0003"]);
    assert!(err.contains("a rewrite needs its new wording"), "{err}");
    let out = ok(&s, &["rule", "review", "--edit", "p-0003", "--text", "Deploy only with release.sh."]);
    assert!(out.starts_with("edited and applied p-0003: rule base."), "{out}");

    // A decision.
    std::fs::write(
        domains_toml(&s),
        format!("{}\n[[domain]]\nname = \"house\"\nmode = \"always\"\n", std::fs::read_to_string(domains_toml(&s)).unwrap()),
    )
    .unwrap();
    let slug = ok(&s, &["decision", "log", "--domain", "house", "--decision", "Invoices go out on Fridays", "--rationale", "the client pays on Mondays"]);
    let slug = slug.trim().trim_start_matches("Decision logged (slug: ").trim_end_matches(')').to_string();
    ok(&s, &["decision", "update", &slug, "--keywords", "invoice"]);
    served_to(&s, sid, "send the invoice today");
    let out = ok_env(&s, &["rule", "propose", "--decision", &slug, "--text", "Invoices go out on Thursdays", "--example", "send the invoice today"], &env);
    assert!(out.starts_with("proposal p-0004 · rewrite · decision "), "{out}");
    let out = ok(&s, &["rule", "review", "--approve", "p-0004"]);
    assert!(out.contains(&format!("decision {slug} rewritten as house.")), "{out}");
    let g = graph(&s);
    assert!(g.contains("supersededBy") && g.contains("Invoices go out on Thursdays"), "the old decision is superseded by the new");
}

/// K5e: a rejection is stored with its reason, and the same change is never proposed again, dry run or not.
#[test]
fn review_reject_never_reproposed() {
    let s = home("reject");
    base_domain(&s);
    propose_gap(&s);
    let out = ok(&s, &["rule", "review", "--reject", "p-0001", "--reason", "the hook is called prompt submit here"]);
    assert_eq!(out, "rejected p-0001; this proposal will not be offered again\n");
    let g = graph(&s);
    assert!(g.contains("status> \"rejected\"") && g.contains("the hook is called prompt submit here"), "status and reason stored");

    let r = rule_ref(&s, "base", "Read the hook output");
    let args = ["rule", "propose", "--rule", r.as_str(), "--keywords", "user prompt submit", "--example", "the user prompt submit hook again"];
    let err = refused(&s, &args);
    assert!(err.contains("as p-0001: this proposal is not offered again"), "{err}");
    let mut dry = args.to_vec();
    dry.push("--dry-run");
    let err = refused(&s, &dry);
    assert!(err.contains("not offered again"), "{err}");
    // A different change is still welcome.
    let out = ok(&s, &["rule", "propose", "--rule", &r, "--keywords", "prompt hook", "--example", "the prompt hook again"]);
    assert!(out.starts_with("proposal p-0002"), "{out}");
}

/// K5b, D8: session start shows one line for the proposals left from earlier sessions, after HANDOFFS, and the first
/// screen (header, instructions, DUE NOW) is exactly as long as before.
#[test]
fn session_start_shows_pending_count_after_handoffs() {
    let s = home("start");
    base_domain(&s);
    let first_screen = |s: &seed::Seed| -> u64 {
        let log = std::fs::read_to_string(s.ws.join(".base").join("hook-output.jsonl")).unwrap();
        let row = log.lines().rev().find(|l| l.contains("\"session-start\"")).expect("a session-start record");
        serde_json::from_str::<serde_json::Value>(row).unwrap()["first_screen_len_u16"].as_u64().expect("measured")
    };
    let (code, before, err) = seed::run_session_start(&s, Some("b016b016-0000-4000-8000-000000000003"));
    assert_eq!(code, 0, "{err}");
    assert!(!before.contains("rule proposals:"), "nothing pending, no line:\n{before}");
    let len_before = first_screen(&s);

    propose_gap(&s);
    ok(&s, &["rule", "propose", "--new", "--domain", "base", "--text", "Say which tier a write lands in.", "--keywords", "tier", "--example", "which tier is this"]);
    let (code, after, err) = seed::run_session_start(&s, Some("b016b016-0000-4000-8000-000000000004"));
    assert_eq!(code, 0, "{err}");
    let line = "rule proposals: 2 pending · base rule review";
    let at = after.find(line).unwrap_or_else(|| panic!("the line:\n{after}"));
    let handoffs = after.find("\nHANDOFFS (").expect("a HANDOFFS block");
    assert!(handoffs < at, "after HANDOFFS:\n{after}");
    if let Some(forks) = after.find("\nFORKS (") {
        assert!(at < forks, "before FORKS:\n{after}");
    }
    assert_eq!(first_screen(&s), len_before, "the first screen did not grow");

    // Reviewed ones are not counted.
    ok(&s, &["rule", "review", "--reject", "p-0001"]);
    let (_, again, _) = seed::run_session_start(&s, Some("b016b016-0000-4000-8000-000000000005"));
    assert!(again.contains("rule proposals: 1 pending · base rule review"), "{again}");
}

/// Example 4: the AI acts for the user with one flag per proposal; a reviewed proposal is not acted on twice; an edit
/// stores what the user changed; a TOO BROAD change needs --broad-ok; with no flag and no terminal, the queue is listed.
#[test]
fn review_noninteractive_flags() {
    let s = home("flags");
    base_domain(&s);
    twenty(&s);
    propose_gap(&s);
    ok(&s, &["rule", "propose", "--new", "--domain", "base", "--text", "Name the tier before a write.", "--keywords", "tier", "--example", "which tier is this"]);
    ok(&s, &["rule", "propose", "--new", "--domain", "base", "--text", "Keep plans short.", "--keywords", "plan", "--example", "plan the week"]);

    let listing = ok(&s, &["rule", "review"]);
    assert!(listing.starts_with("[1/3] p-0001 keyword gap · base · add \"user prompt submit\" · replay 15.8% · given by hand"), "{listing}");
    assert!(listing.contains("[2/3] p-0002 new rule · base · \"Name the tier before a write.\""), "{listing}");
    assert!(listing.contains("act on one: base rule review --approve <id>"), "{listing}");

    // An edit naming a field the kind does not use is refused, never stored as a change that was not made.
    let err = refused(&s, &["rule", "review", "--edit", "p-0001", "--text", "new wording"]);
    assert!(err.contains("p-0001 is a keyword gap: it changes keywords, not wording"), "{err}");
    assert!(ok(&s, &["rule", "review", "--approve", "p-0001"]).starts_with("approved p-0001:"));
    assert!(ok(&s, &["rule", "review", "--approve", "p-0002"]).starts_with("approved p-0002:"));
    let err = refused(&s, &["rule", "review", "--approve", "p-0001"]);
    assert!(err.contains("p-0001 is approved on ") && err.contains("not pending: nothing changed"), "{err}");

    // "plan" is in 2 of 19 prompts; as "the" it would be in most: the edit is replayed and refused as TOO BROAD.
    let err = refused(&s, &["rule", "review", "--edit", "p-0003", "--keywords", "the"]);
    assert!(err.contains("p-0003 is TOO BROAD: it would be served on ") && err.contains("--broad-ok"), "{err}");
    let out = ok(&s, &["rule", "review", "--edit", "p-0003", "--keywords", "the", "--broad-ok"]);
    assert!(out.starts_with("edited and applied p-0003: new rule base.") && out.contains("TOO BROAD"), "{out}");
    let g = graph(&s);
    assert!(g.contains("status> \"edited\"") && g.contains("editedKeyword> \"the\""), "the edit is stored beside the proposal");
    assert_eq!(ok(&s, &["rule", "review"]), "no rule proposals pending\n");
}
