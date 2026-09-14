//! Commit 4d of rank 01, the wiring: both hooks serve a rule by its own matchers.
//!
//! Spec F2 to F8 and J7, with `auk`'s rulings of 2026-09-14 (lane brief, section COMMIT 4, part 4): A2, the
//! PowerShell tool's command is read as well as Bash's; A3, the action branch runs for a tool call that carries no
//! `file_path`; and the HARD RULE, a rule with no kind is served by today's domain path and counted by `base doctor`,
//! never dropped.
//!
//! Written BEFORE the wiring and run RED at the 4c head, as J1 requires. At 4c the model, its storage and `select`
//! exist, and neither hook calls `select`: a rule with matchers arrives only through its domain's trigger, and a tool
//! call with no `file_path` never reaches rule serving at all. One test must be GREEN in both arms: the J9.2 control,
//! which proves 4d leaves a store with no matchers exactly as it was (K4).
//!
//! How a test reads what an event served. A tool call: the text the pre-tool hook returns. A prompt: the per-rule
//! record the prompt hook saves, because that hook prints its text instead of returning it. A rule id that is in the
//! record after an event, and was not before it, is a rule that event served. A reader like that says "nothing" when
//! it cannot see, so every test that leans on it also has a leg where the record must hold something (law 48). J7.6
//! needs the printed pointer lines, so it drives the real binary and reads its stdout.
//!
//! Sessions. The record scopes its keys by the PROCESS session id, which only the hook dispatcher sets
//! (`set_process_session`). A hook called in process, as these tests call it, therefore records every key under the
//! shared scope whatever `session_id` the event carries. So each test owns a whole workspace, the reader ignores the
//! session part of a key, and a new session is stood in for by wiping the record (`fresh_session`).

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use base::config::{BaseConfig, BracketConfig, NamespaceConfig};
use base::crud;
use base::domain::rules::{Matcher, rule_id};
use base::domain::session::SessionState;
use base::hook::{pre_tool_use, user_prompt_submit};
use serde_json::json;

/// `SessionState`'s key separator. A record key is `<session> SEP r SEP <rule id>`, plus `SEP <scope>` for a place or
/// an action rule (`SessionState::rule_key`).
const SEP: char = '\u{1}';

/// A domain no event in these tests triggers: a rule filed in it arrives by its own matchers or not at all.
const QUIET: &str = "[[domain]]\nname = \"quiet\"\nmode = \"triggered\"\nprompt_keywords = [\"zzqq never said\"]\n";

/// A domain with a prompt keyword and a path trigger, for the legs that go through today's domain path.
const PROBE: &str = "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nprompt_keywords = [\"probe\"]\npaths = [\"work\"]\n";

const SID: &str = "sid-4d";

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

fn config() -> BaseConfig {
    BaseConfig { namespace: ns(), ..Default::default() }
}

/// Percent mode in the operator's shape: FRESH to 20 percent, MODERATE to 40, DEPLETED to 60, of a 1,000-token window.
fn percent_config() -> BaseConfig {
    BaseConfig {
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
    }
}

/// A workspace with an empty graph, this `domains.toml`, and no session record.
fn workspace(root: &Path, domains_toml: &str) {
    let base = root.join(".base");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("graph.nq"), "").unwrap();
    std::fs::write(base.join("domains.toml"), domains_toml).unwrap();
    fresh_session(root);
}

/// Stands in for a new Claude Code session: the record of what was shown is gone.
fn fresh_session(root: &Path) {
    SessionState::clear(&root.join(".base"));
}

/// A CLI rule with these matchers, the way `base rule add --kind ...` writes one (F11).
fn add(root: &Path, domain: &str, text: &str, matchers: Vec<Matcher>) {
    crud::rule::add_with_matchers(root, &ns(), domain, text, None, None, &matchers).unwrap();
}

fn tool_with(cfg: &BaseConfig, root: &Path, name: &str, input: serde_json::Value) -> String {
    let event = json!({ "tool_name": name, "session_id": SID, "tool_input": input });
    pre_tool_use::handle(cfg, root, &event).unwrap().1
}

fn tool(root: &Path, name: &str, input: serde_json::Value) -> String {
    tool_with(&config(), root, name, input)
}

fn shell(root: &Path, shell: &str, command: &str) -> String {
    tool(root, shell, json!({ "command": command }))
}

fn edit(root: &Path, path: &Path) -> String {
    tool(root, "Edit", json!({ "file_path": path.display().to_string(), "old_string": "a", "new_string": "b" }))
}

/// Runs the prompt hook and returns the number of rules it reports serving. `used` writes a transcript whose newest
/// usage block holds that many tokens, which is how the hook reads its percentage.
fn prompt(root: &Path, cfg: &BaseConfig, text: &str, used: Option<u64>) -> usize {
    let mut event = json!({ "prompt": text, "session_id": SID });
    if let Some(used) = used {
        let path = root.join("transcript.jsonl");
        let line = json!({ "message": { "usage": { "input_tokens": used } } });
        std::fs::write(&path, format!("{line}\n")).unwrap();
        event["transcript_path"] = json!(path.display().to_string());
    }
    user_prompt_submit::handle(cfg, root, &event).unwrap().rules_injected
}

/// The record's entries for one rule in this workspace, as (scope, tier). An empty scope is a topic or always rule.
fn record_for(root: &Path, domain: &str, text: &str) -> Vec<(String, String)> {
    let id = rule_id(domain, text);
    SessionState::load(&root.join(".base"))
        .rules_shown
        .iter()
        .filter_map(|(key, entry)| {
            let mut parts = key.splitn(4, SEP);
            let (_session, marker, rule) = (parts.next()?, parts.next()?, parts.next()?);
            (marker == "r" && rule == id).then(|| (parts.next().unwrap_or_default().to_string(), entry.tier.clone()))
        })
        .collect()
}

fn shown(root: &Path, domain: &str, text: &str) -> bool {
    !record_for(root, domain, text).is_empty()
}

fn tiers(root: &Path, domain: &str, text: &str) -> Vec<String> {
    record_for(root, domain, text).into_iter().map(|(_, tier)| tier).collect()
}

#[test]
fn j7_1_a_place_rule_arrives_on_the_first_touch_of_its_file_and_not_on_the_next() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root, QUIET);
        add(root, "quiet", "PLACE RULE ONE", vec![Matcher::for_place("commands.toml")]);
        let file = root.join("cfg").join("commands.toml");

        // The Write-then-Edit case from 2026-09-12: one file, two tools, one session.
        let write = tool(root, "Write", json!({ "file_path": file.display().to_string(), "content": "x" }));
        assert!(write.contains("PLACE RULE ONE"), "the first touch of the named file serves its place rule (F4): {write}");
        assert!(!record_for(root, "quiet", "PLACE RULE ONE").is_empty(), "control: the record holds what was served");

        // A rule added after the first touch, on the same file. Its arrival proves the Edit reached rule serving, so
        // the first rule's absence below is a reading and not a hook that served nothing.
        add(root, "quiet", "PLACE RULE TWO", vec![Matcher::for_place("commands.toml")]);
        let second = edit(root, &file);
        assert!(second.contains("PLACE RULE TWO"), "control: the Edit reached rule serving: {second}");
        assert!(!second.contains("PLACE RULE ONE"), "Write then Edit on one file served the place rule twice: {second}");

        // Whole segments, never a substring (F4). A new session, so the record above cannot hide a wrong match, and a
        // positive control in that same session afterwards.
        fresh_session(root);
        let bak = edit(root, &root.join("cfg").join("commands.toml.bak"));
        assert!(!bak.contains("PLACE RULE"), "commands.toml.bak is not commands.toml: {bak}");
        let real = edit(root, &file);
        assert!(real.contains("PLACE RULE ONE"), "control: the same session gets the rule on the real file: {real}");
    });
}

#[test]
fn j7_2_a_tier_change_shows_place_topic_and_always_rules_again_once() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root, QUIET);
        let cfg = percent_config();
        add(root, "quiet", "ALWAYS RULE", vec![Matcher::always()]);
        add(root, "quiet", "TOPIC RULE", vec![Matcher::for_topic(vec!["release checklist".into()])]);
        add(root, "quiet", "PLACE RULE", vec![Matcher::for_place("commands.toml")]);
        let file = root.join("cfg").join("commands.toml").display().to_string();
        let place = || tool_with(&cfg, root, "Edit", json!({ "file_path": file })).contains("PLACE RULE");

        // FRESH: 100 of 1,000 tokens.
        assert_eq!(prompt(root, &cfg, "the release checklist", Some(100)), 2, "FRESH, first prompt: always and topic");
        assert_eq!(tiers(root, "quiet", "ALWAYS RULE"), ["FRESH"], "control: the record holds the always rule");
        assert_eq!(tiers(root, "quiet", "TOPIC RULE"), ["FRESH"]);
        assert!(place(), "FRESH: the place rule on the first touch");
        assert_eq!(prompt(root, &cfg, "the release checklist again", Some(120)), 0, "FRESH again: nothing twice");
        assert!(!place(), "FRESH again: the place rule not twice");

        // MODERATE: 250 of 1,000 tokens.
        assert_eq!(prompt(root, &cfg, "the release checklist", Some(250)), 2, "MODERATE: always and topic once more (F8)");
        assert_eq!(tiers(root, "quiet", "ALWAYS RULE"), ["MODERATE"]);
        assert_eq!(tiers(root, "quiet", "TOPIC RULE"), ["MODERATE"]);
        assert!(place(), "MODERATE: the place rule once more (F8)");
        assert_eq!(prompt(root, &cfg, "the release checklist", Some(260)), 0, "MODERATE again: nothing twice");
        assert!(!place(), "MODERATE again: the place rule not twice");
    });
}

#[test]
fn j7_3_an_action_rule_fires_through_powershell_and_bash_once_per_throttle_window() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root, QUIET);
        add(root, "quiet", "PING RULE ONE", vec![Matcher::for_command("base relay ping --to chris")]);

        // PowerShell (A2): the operator's primary shell, which carried every relay ping on 2026-09-14. The call has no
        // file_path (A3).
        let ps = shell(root, "PowerShell", "base relay ping --from shrike --to chris --msg 'x'");
        assert!(ps.contains("PING RULE ONE"), "a PowerShell command serves its action rule: {ps}");

        add(root, "quiet", "PING RULE TWO", vec![Matcher::for_command("base relay ping")]);
        let again = shell(root, "Bash", "base relay ping --to chris --msg 'y'");
        assert!(again.contains("PING RULE TWO"), "control: the second ping reached rule serving: {again}");
        assert!(!again.contains("PING RULE ONE"), "a second ping inside the 10 minute throttle served it again: {again}");

        // Eleven minutes pass. The throttle compares a record's `at` with now, so every record moves back.
        let base = root.join(".base");
        let mut state = SessionState::load(&base);
        let mut aged = 0;
        for entry in state.rules_shown.values_mut() {
            entry.at = entry.at.saturating_sub(11 * 60);
            aged += 1;
        }
        assert!(aged >= 2, "control: both action records exist to age, found {aged}");
        state.save(&base).unwrap();
        let later = shell(root, "Bash", "base relay ping --to chris --msg 'z'");
        assert!(later.contains("PING RULE ONE"), "a ping after the throttle window serves the rule again (F5): {later}");

        // Bash as the first shell of a new session, so neither shell passes on the other's record.
        fresh_session(root);
        let bash = shell(root, "Bash", "base relay ping --to chris --msg 'b'");
        assert!(bash.contains("PING RULE ONE"), "a Bash command serves its action rule: {bash}");
    });
}

#[test]
fn j7_4_the_tool_hook_matches_through_wrappers_and_each_part_of_a_compound_command() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root, QUIET);
        add(root, "quiet", "PING RULE", vec![Matcher::for_command("base relay ping --to chris")]);

        // A new session per case, so no case rides on another's record. The two `false` cases are only meaningful
        // beside the `true` ones, which prove the hook serves at all.
        let cases = [
            (
                "MSYS_NO_PATHCONV=1 wsl -- bash -c 'cd sub && base relay ping --to chris --msg hi'",
                true,
                "VAR=value, wsl --, bash -c and a && b",
            ),
            ("git status && base relay ping --to chris --msg hi", true, "the second part of a && b"),
            ("cd sub; base relay ping --from shrike --to chris --msg hi | tee out.txt", true, "a ; b and a pipe"),
            ("powershell -NoProfile -Command \"base relay ping --to chris --msg 'a; b'\"", true, "powershell -Command"),
            ("echo base relay ping --to chris", false, "words handed to echo are not the command"),
            ("base relay ping --to auk --msg 'tell --to chris'", false, "a flag inside a quoted message is not a flag"),
        ];
        for (command, want, why) in cases {
            fresh_session(root);
            let out = shell(root, "Bash", command);
            assert_eq!(out.contains("PING RULE"), want, "{why}: `{command}` served: {out}");
        }
    });
}

#[test]
fn j7_5_a_rule_whose_place_and_action_both_hit_one_tool_call_shows_once() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root, QUIET);
        // The one event where two kinds meet by construction: a tool call that is a place and an action at once.
        add(root, "quiet", "TWO MATCHER RULE", vec![Matcher::for_place("notes.md"), Matcher::for_tool("Edit")]);
        let out = edit(root, &root.join("docs").join("notes.md"));
        assert_eq!(out.matches("TWO MATCHER RULE").count(), 1, "place and action both hit this Edit; it shows once (F3): {out}");
    });
}

#[test]
fn j7_6_topic_rules_are_capped_with_true_pointer_counts_on_a_123_rule_seed() {
    // The machine's shape (J2): 31 domains and 123 rules, with synthetic text, because the base repository is not the
    // place for the operator's private rules. Twelve rules, four in each of three domains, share one phrase, so one
    // prompt matches twelve and the cap of five must withhold seven.
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let mut toml = String::new();
    for d in 0..31 {
        toml.push_str(&format!(
            "[[domain]]\nname = \"seed-{d:02}\"\nmode = \"triggered\"\nprompt_keywords = [\"seedword{d:02}\"]\n\n"
        ));
    }
    workspace(root, &toml);
    let seeded = base::home::with_thread_home(home.path(), || {
        let mut n = 0;
        for d in 0..31 {
            for _ in 0..4 {
                if n == 123 {
                    break;
                }
                let words = if d < 3 { vec!["harbour schedule".to_string()] } else { vec![format!("subject{n:03}")] };
                add(root, &format!("seed-{d:02}"), &format!("SEED RULE {n:03}"), vec![Matcher::for_topic(words)]);
                n += 1;
            }
        }
        n
    });
    assert_eq!(seeded, 123, "control: the seed holds 123 rules");

    let payload = json!({ "prompt": "what is the harbour schedule", "session_id": "sid-cap" }).to_string();
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_base"))
        .args(["hook", "user-prompt-submit"])
        .current_dir(root)
        .env("BASE_HOME", home.path())
        .env("BASE_NO_AUTO_UPDATE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(payload.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    let elapsed = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!("J7.6 latency, one prompt hook process over 123 rules, first domain sync included: {elapsed:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the hook exited {:?}: {stderr}", output.status.code());

    // Which rules arrived, by number. Rule n was filed in domain n / 4.
    let served: Vec<usize> = stdout
        .match_indices("SEED RULE ")
        .filter_map(|(i, m)| stdout[i + m.len()..].get(..3).and_then(|s| s.parse().ok()))
        .collect();
    assert_eq!(served.len(), 5, "twelve topic rules matched and topic_max is 5 (F6): {stdout}");
    assert!(served.iter().all(|n| *n < 12), "only the twelve that share the phrase can match: {served:?}");

    // The pointer lines: "<N> more <domain> rules", one per domain that had more matches than were shown.
    let mut pointers: Vec<(String, usize)> = Vec::new();
    for line in stdout.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        for w in words.windows(4) {
            if w[1] == "more" && w[3].starts_with("rules") {
                // An unreadable count becomes 0, which cannot equal a real withheld count, so it fails loudly below.
                let count = w[0].trim_start_matches('(').parse().unwrap_or(0);
                pointers.push((w[2].to_string(), count));
            }
        }
    }
    let mut want: Vec<(String, usize)> = (0..3)
        .map(|d| (format!("seed-{d:02}"), 4 - served.iter().filter(|n| **n / 4 == d).count()))
        .filter(|(_, withheld)| *withheld > 0)
        .collect();
    pointers.sort();
    want.sort();
    assert_eq!(pointers, want, "each pointer line counts exactly what the cap withheld in its domain: {stdout}");
    assert_eq!(pointers.iter().map(|(_, c)| c).sum::<usize>(), 7, "seven withheld in total: {stdout}");

    // The same prompt in process, in a new session, after the sync has run: the steady-state figure.
    fresh_session(root);
    let (again, took) = base::home::with_thread_home(home.path(), || {
        let started = Instant::now();
        let n = prompt(root, &config(), "what is the harbour schedule", None);
        (n, started.elapsed())
    });
    eprintln!("J7.6 latency, prompt hook in process over 123 rules, sync already done: {took:?}");
    assert_eq!(again, 5, "a new session is served the capped five again");
}

#[test]
fn a_rule_with_matchers_leaves_its_domain_trigger_and_a_rule_without_a_kind_stays_on_it() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root, PROBE);
        crud::rule::add(root, &ns(), "probe", "LEGACY RULE", None).unwrap();
        add(root, "probe", "CONVERTED RULE", vec![Matcher::for_topic(vec!["zebra crossing".into()])]);
        let cfg = config();

        let n = prompt(root, &cfg, "a probe question", None);
        assert!(shown(root, "probe", "LEGACY RULE"), "HARD RULE: a rule with no kind is served by its domain's trigger");
        assert!(
            !shown(root, "probe", "CONVERTED RULE"),
            "a rule with matchers of its own is no longer served through its domain's trigger (4d)"
        );
        assert_eq!(n, 1, "the prompt served exactly the rule with no kind");

        prompt(root, &cfg, "is the zebra crossing safe", None);
        assert!(shown(root, "probe", "CONVERTED RULE"), "the converted rule arrives on its own topic");

        // The tool hook, in a new session so the prompt's record does not hide either rule.
        fresh_session(root);
        let out = tool(root, "Read", json!({ "file_path": root.join("work").join("x.md").display().to_string() }));
        assert!(out.contains("LEGACY RULE"), "HARD RULE on the tool hook: the path trigger still serves it: {out}");
        assert!(!out.contains("CONVERTED RULE"), "the tool hook does not serve a converted rule by its domain's path: {out}");
    });
}

#[test]
fn j9_2_with_no_matcher_anywhere_every_rule_still_serves_through_its_domain() {
    // The control leg: GREEN at 4c and GREEN at 4d, or 4d broke K4.
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root, PROBE);
        let texts = ["FIRST PLAIN RULE", "SECOND PLAIN RULE", "THIRD PLAIN RULE"];
        for text in texts {
            crud::rule::add(root, &ns(), "probe", text, None).unwrap();
        }
        assert_eq!(prompt(root, &config(), "a probe question", None), 3, "a prompt that triggers the domain serves all three");
        for text in texts {
            assert!(shown(root, "probe", text), "control: the record holds {text}");
        }
        fresh_session(root);
        let out = tool(root, "Read", json!({ "file_path": root.join("work").join("x.md").display().to_string() }));
        for text in texts {
            assert!(out.contains(text), "the path trigger serves {text}: {out}");
        }
    });
}

#[test]
fn doctor_counts_the_rules_with_no_matcher_of_their_own() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root, PROBE);
        crud::rule::add(root, &ns(), "probe", "NO KIND ONE", None).unwrap();
        crud::rule::add(root, &ns(), "probe", "NO KIND TWO", None).unwrap();
        add(root, "probe", "HAS A KIND", vec![Matcher::always()]);
        let report = base::doctor::format_human(&base::doctor::diagnose(root));
        assert!(
            report.contains("2 rules with no matcher of their own"),
            "HARD RULE: doctor counts the rules still served by their domain, and never drops them: {report}"
        );
    });
}

/// The 17 base-config rules of spec F17, shortened, with the kinds and matchers of the lane brief's COMMIT 4 part 1
/// and `auk`'s rulings in part 4. ws 1 fits no kind, carries no matcher, and is served by the domain path.
fn f17_rules() -> Vec<(&'static str, Vec<Matcher>)> {
    let cmd = Matcher::for_command;
    let words = |w: &[&str]| Matcher::for_topic(w.iter().map(|s| s.to_string()).collect());
    let slack = |name: &str| Matcher::for_tool(&format!("mcp__claude_ai_Slack__{name}"));
    vec![
        ("WS0 ESCALATIONS GO TO THE CHRIS PIPE", vec![cmd("base relay ping --to chris")]),
        ("WS1 A CHILD IDLE MEANS ORCHESTRATE IT", vec![]),
        ("WS2 GATED CONDUCT RESOLVES AGAINST THE PROCESS DOC", vec![words(&["gated build", "hold", "lift"])]),
        ("WS3 THE PACKAGED HUB IS CANONICAL", vec![Matcher::for_place("ping-chat-hub")]),
        ("WS4 FORK ONLY ON COMMAND", vec![Matcher::always()]),
        (
            "WS5 MESSAGES TO ALBERT SHARE",
            vec![slack("slack_send_message"), slack("slack_send_message_draft"), slack("slack_schedule_message")],
        ),
        ("WS6 NO STAR COMMAND STRINGS IN RELAY MESSAGES", vec![cmd("base relay ping")]),
        (
            "WS7 AT MOST THREE CHILD SESSIONS",
            vec![cmd("spawn-child.ps1"), cmd("respawn.ps1"), cmd("curl -X POST http://127.0.0.1:7799/api/spawn")],
        ),
        ("WS8 PINGS TO CHRIS USE LINE BREAKS", vec![cmd("base relay ping --to chris")]),
        ("WS9 HOLD A BUILDER BEFORE MERGING", vec![cmd("git merge"), cmd("gh pr merge")]),
        ("WS10 BUILD ON WINDOWS NATIVELY", vec![words(&["scaffold", "wsl"])]),
        ("WS11 HELP BEFORE ANY CLAIM", vec![words(&["base cli", "can base"])]),
        ("WS12 WSL IS MIGRATING", vec![words(&["wsl"])]),
        (
            "WS13 STATE THE TIER BEFORE A WRITE",
            ["base rule add", "base learn", "base decision log", "base handoff create", "base fork create"].map(cmd).to_vec(),
        ),
        ("GBL0 COMMANDS TOML IS WSL OWNED", vec![Matcher::for_place("commands.toml")]),
        ("GBL1 NEVER EDIT A WINDOWS COPY", vec![Matcher::for_place("commands.toml")]),
        ("GBL2 THE SYMLINK NEEDS WSL RUNNING", vec![Matcher::for_place("commands.toml")]),
    ]
}

enum Ev {
    Prompt(&'static str),
    Tool(&'static str, serde_json::Value),
}

#[test]
fn f17_acceptance_seventeen_served_sixteen_by_their_kind_and_ws1_by_the_domain_path() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        // Only ws 1's own moment names the domain, so the domain path can serve nothing earlier.
        workspace(
            root,
            "[[domain]]\nname = \"base-config\"\nmode = \"triggered\"\nprompt_keywords = [\"idle awaiting orders\"]\n",
        );
        let rules = f17_rules();
        for (text, matchers) in &rules {
            if matchers.is_empty() {
                crud::rule::add(root, &ns(), "base-config", text, None).unwrap();
            } else {
                add(root, "base-config", text, matchers.clone());
            }
        }
        let cfg = config();
        let path = |rel: &str| root.join(rel).display().to_string();
        let in_record = || -> BTreeSet<&'static str> {
            rules.iter().map(|(t, _)| *t).filter(|t| shown(root, "base-config", t)).collect()
        };

        // One real event per rule, in the order a session meets them, and what each must serve.
        let ping = "base relay ping --from shrike --to chris --msg 'x'";
        let respawn = "& 'C:/Users/Chris/.base-gbl/scripts/respawn.ps1' -Codename shrike";
        let learn = "base learn --text 'x' --domain base-config --type insight";
        let events: Vec<(&str, Ev, Vec<&str>)> = vec![
            ("prompt: good morning", Ev::Prompt("good morning"), vec!["WS4 FORK ONLY ON COMMAND"]),
            (
                "prompt: gated build",
                Ev::Prompt("we are in a gated build, hold for refresh"),
                vec!["WS2 GATED CONDUCT RESOLVES AGAINST THE PROCESS DOC"],
            ),
            ("prompt: scaffold", Ev::Prompt("scaffold a new workspace"), vec!["WS10 BUILD ON WINDOWS NATIVELY"]),
            ("prompt: can base", Ev::Prompt("can base do this"), vec!["WS11 HELP BEFORE ANY CLAIM"]),
            ("prompt: wsl", Ev::Prompt("is wsl still supported"), vec!["WS12 WSL IS MIGRATING"]),
            (
                "Edit ping-chat-hub/src/hub.py",
                Ev::Tool("Edit", json!({ "file_path": path("tools/ping-chat-hub/src/hub.py") })),
                vec!["WS3 THE PACKAGED HUB IS CANONICAL"],
            ),
            (
                "Edit cfg/commands.toml",
                Ev::Tool("Edit", json!({ "file_path": path("cfg/commands.toml") })),
                vec!["GBL0 COMMANDS TOML IS WSL OWNED", "GBL1 NEVER EDIT A WINDOWS COPY", "GBL2 THE SYMLINK NEEDS WSL RUNNING"],
            ),
            (
                "Slack send",
                Ev::Tool("mcp__claude_ai_Slack__slack_send_message", json!({ "channel_id": "C1", "message": "hi" })),
                vec!["WS5 MESSAGES TO ALBERT SHARE"],
            ),
            (
                "PowerShell: relay ping to chris",
                Ev::Tool("PowerShell", json!({ "command": ping })),
                vec![
                    "WS0 ESCALATIONS GO TO THE CHRIS PIPE",
                    "WS6 NO STAR COMMAND STRINGS IN RELAY MESSAGES",
                    "WS8 PINGS TO CHRIS USE LINE BREAKS",
                ],
            ),
            (
                "PowerShell: respawn.ps1",
                Ev::Tool("PowerShell", json!({ "command": respawn })),
                vec!["WS7 AT MOST THREE CHILD SESSIONS"],
            ),
            (
                "Bash: git merge",
                Ev::Tool("Bash", json!({ "command": "git merge feature" })),
                vec!["WS9 HOLD A BUILDER BEFORE MERGING"],
            ),
            ("Bash: base learn", Ev::Tool("Bash", json!({ "command": learn })), vec!["WS13 STATE THE TIER BEFORE A WRITE"]),
            (
                "prompt: idle awaiting orders",
                Ev::Prompt("the child is idle awaiting orders"),
                vec!["WS1 A CHILD IDLE MEANS ORCHESTRATE IT"],
            ),
        ];

        let mut before = in_record();
        let mut rows: Vec<(&str, BTreeSet<&str>, BTreeSet<&str>)> = Vec::new();
        for (name, event, want) in events {
            let rendered = match event {
                Ev::Prompt(text) => {
                    prompt(root, &cfg, text, None);
                    None
                }
                Ev::Tool(tool_name, input) => Some(tool(root, tool_name, input)),
            };
            let after = in_record();
            let got: BTreeSet<&str> = after.difference(&before).copied().collect();
            if let Some(out) = &rendered {
                for label in &got {
                    assert!(out.contains(label), "{name}: {label} was recorded as shown but never rendered: {out}");
                }
            }
            rows.push((name, want.into_iter().collect(), got));
            before = after;
        }

        let table: Vec<String> = rows.iter().map(|(name, want, got)| format!("  {name}: want {want:?} got {got:?}")).collect();
        let table = table.join("\n");
        let mismatched = rows.iter().filter(|(_, want, got)| want != got).count();
        assert_eq!(mismatched, 0, "F17: each rule is served by its own event\n{table}");
        let by_kind: usize = rows[..12].iter().map(|(_, _, got)| got.len()).sum();
        let by_domain = rows[12].2.len();
        assert_eq!((by_kind, by_domain), (16, 1), "F17 prediction: 17 served, 16 by kind and ws 1 by the domain path\n{table}");
    });
}
