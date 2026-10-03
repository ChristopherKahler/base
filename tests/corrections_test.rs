//! BO-15 (K3, C1 to C4, D4, D10): base notices when the user corrects the AI, and turns each correction into a rule
//! proposal.
//!
//! Two kinds of check. The detector's cases are data (`tests/fixtures/corrections/cases.json`): each is a short session
//! in Claude Code's real transcript shapes (`tests/transcripts/mod.rs`) and the signals the detector must find, so a
//! correction it missed becomes a case without new code (BO-17's backstop adds them). The rest drive the real hooks and
//! commands on a seeded home: the C4 line, the signal rows, `base rule propose`, and `base rule add` inside a session.

mod seed;
mod transcripts;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use transcripts::Ev;

const CHECK_LINE: &str = "This may be a correction. If it is, run base rule propose --from-turn after answering.";
const CORRECTED_LINE: &str = "When the user corrects you, start that reply with \"CORRECTED: <what you got wrong>\".";

// ─── The detector's cases ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Cases {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    events: Vec<Ev>,
    /// Turn number → the turn's signals as `LAYER kind`.
    expect: std::collections::BTreeMap<String, Vec<String>>,
    checks: Vec<u32>,
}

fn cases() -> Vec<Case> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("corrections").join("cases.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let cases: Cases = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(cases.cases.len() >= 10, "control: the case file holds its cases ({})", cases.cases.len());
    cases.cases
}

fn case(name: &str) -> Case {
    cases().into_iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no case {name:?} in cases.json"))
}

/// Run one case through the transcript reader and the detector, and compare every turn.
fn check_case(c: &Case) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    transcripts::write(&path, "case-session", &c.events);
    let events = base::domain::transcript::read_all(&path).expect("the transcript reads");
    let turns = base::corrections::turns(events, &base::config::CorrectionsConfig::default());
    assert!(!turns.is_empty(), "[{}] control: the case has turns", c.name);
    for t in &turns {
        let mut got: Vec<String> = t.signals.iter().map(|s| format!("{} {}", s.layer, s.kind)).collect();
        got.sort();
        let mut want = c.expect.get(&t.num.to_string()).cloned().unwrap_or_default();
        want.sort();
        assert_eq!(got, want, "[{}] turn {} ({:?}): signals {:?}", c.name, t.num, t.prompt, t.signals);
    }
    let checks: Vec<u32> = turns.iter().filter(|t| t.check).map(|t| t.num).collect();
    assert_eq!(checks, c.checks, "[{}] the turns that carry the check line", c.name);
    for k in c.expect.keys() {
        let n: u32 = k.parse().expect("a turn number");
        assert!(turns.iter().any(|t| t.num == n), "[{}] expects turn {n}, which the session does not have", c.name);
    }
}

/// Every case in the file, whoever added it.
#[test]
fn every_detector_case_holds() {
    let all = cases();
    for c in &all {
        check_case(c);
    }
    eprintln!("checked {} detector cases", all.len());
}

// ─── The seeded home the hooks run in ────────────────────────────────────────

/// A fresh seeded home and workspace for `tag`.
fn home(tag: &str) -> seed::Seed {
    let root = std::env::temp_dir().join(format!("base-bo15-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    seed::write(&root, &seed::TINY, "")
}

/// A session id Claude Code would send: a UUID.
fn sid(n: u32) -> String {
    format!("b015b015-0000-4000-8000-{n:012}")
}

/// The prompt hook on `prompt`, the transcript at `transcript`, from `cwd`. Stdout.
fn prompt_hook(s: &seed::Seed, cwd: &Path, session: &str, transcript: &Path, prompt: &str) -> String {
    let payload = serde_json::json!({
        "cwd": cwd.display().to_string(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": prompt,
        "session_id": session,
        "transcript_path": transcript.display().to_string(),
    });
    let (code, out, err) = seed::run_hook_at(s, cwd, "user-prompt-submit", &payload, &[]);
    assert_eq!(code, 0, "the prompt hook failed: {err}");
    out
}

/// The Stop hook at a turn's end, with the reply's last text as Claude Code passes it.
fn stop_hook(s: &seed::Seed, cwd: &Path, session: &str, transcript: &Path, last: &str) {
    let payload = serde_json::json!({
        "cwd": cwd.display().to_string(),
        "hook_event_name": "Stop",
        "session_id": session,
        "transcript_path": transcript.display().to_string(),
        "stop_hook_active": false,
        "last_assistant_message": last,
    });
    let (code, _, err) = seed::run_hook_at(s, cwd, "stop", &payload, &[]);
    assert_eq!(code, 0, "the Stop hook failed: {err}");
}

/// Every signal row in a workspace's match log, as `(prompt, ["LAYER kind", ...])`.
fn signal_rows(ws: &Path) -> Vec<(u32, Vec<String>)> {
    let text = std::fs::read_to_string(ws.join(".base").join("match-log.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["event"] == "signal")
        .map(|v| {
            let n = v["prompt_num"].as_u64().unwrap_or(0) as u32;
            let labels = v["signals"]
                .as_array()
                .map(|a| a.iter().map(|s| format!("{} {}", s["layer"].as_str().unwrap_or(""), s["kind"].as_str().unwrap_or(""))).collect())
                .unwrap_or_default();
            (n, labels)
        })
        .collect()
}

/// `base` with extra environment, scrubbed the way the seed's runners scrub it.
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

/// How many rule proposals the workspace graph holds.
fn proposals(s: &seed::Seed) -> usize {
    std::fs::read_to_string(s.ws.join(".base").join("graph.nq"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("#RuleProposal>") && l.contains("22-rdf-syntax-ns#type"))
        .count()
}

fn transcript_in(s: &seed::Seed, name: &str) -> PathBuf {
    s.home.join(".claude").join("projects").join("work").join(format!("{name}.jsonl"))
}

// ─── C1 ──────────────────────────────────────────────────────────────────────

/// C1 flags and never decides: a phrase in a later human prompt puts the C4 line on that prompt and a signal row in
/// the match log, and nothing else; no proposal is written by any hook. A session's first prompt and a task
/// notification are never flagged.
#[test]
fn c1_flags_phrases_but_does_not_decide() {
    for name in ["c1-phrase-flags-a-later-prompt", "c1-skips-a-sessions-first-prompt", "c1-skips-a-task-notification"] {
        check_case(&case(name));
    }
    let s = home("c1");
    let session = sid(1);
    let t = transcript_in(&s, &session);
    let first = "never commit to main and don't force push; set up the release branch";
    transcripts::append(&t, &session, &[Ev::Prompt(first.into())]);
    let out = prompt_hook(&s, &s.ws, &session, &t, first);
    assert!(!out.contains(CHECK_LINE), "a session's first prompt is never flagged:\n{out}");
    transcripts::append(&t, &session, &[Ev::Text("The release branch is set up.".into())]);
    stop_hook(&s, &s.ws, &session, &t, "The release branch is set up.");

    let second = "no, that's not what I asked for";
    transcripts::append(&t, &session, &[Ev::Prompt(second.into())]);
    let out = prompt_hook(&s, &s.ws, &session, &t, second);
    assert_eq!(out.matches(CHECK_LINE).count(), 1, "the check line, once:\n{out}");
    assert_eq!(signal_rows(&s.ws), vec![(2, vec!["C1 phrase".to_string()])]);
    assert_eq!(proposals(&s), 0, "C1 decides nothing: no hook writes a proposal");
}

// ─── C2 ──────────────────────────────────────────────────────────────────────

/// C2 reads what the user did. Example 4: the user stops an Edit and types "use the other file", which has no
/// phrase; Claude Code runs no Stop hook after an interrupt, so the prompt hook reads the interrupted turn and the C4
/// line lands on that very prompt. A refused tool call works the same. A hook's refusal, and a Read of a document that
/// quotes both strings, are not the user.
#[test]
fn c2_interrupt_and_denial_from_transcript() {
    for name in [
        "example-4-interrupt-then-a-plain-correction",
        "a-plain-interrupt",
        "a-user-denial",
        "a-rule-denial-is-not-the-user",
        "a-read-of-a-document-holding-the-strings",
    ] {
        check_case(&case(name));
    }
    for (n, (stopped, label)) in [(Ev::InterruptToolUse, "C2 interrupt"), (Ev::Denial, "C2 denial")].into_iter().enumerate() {
        let s = home(&format!("c2-{n}"));
        let session = sid(10 + n as u32);
        let t = transcript_in(&s, &session);
        let first = "add the retry to the loader";
        transcripts::append(&t, &session, &[Ev::Prompt(first.into())]);
        prompt_hook(&s, &s.ws, &session, &t, first);
        // The turn ends with the user stopping it: no Stop hook runs.
        transcripts::append(&t, &session, &[Ev::Edit("C:/work/src/loader.rs".into()), stopped]);
        let second = "use the other file";
        transcripts::append(&t, &session, &[Ev::Prompt(second.into())]);
        let out = prompt_hook(&s, &s.ws, &session, &t, second);
        assert_eq!(out.matches(CHECK_LINE).count(), 1, "[{label}] the check line on the next prompt:\n{out}");
        assert_eq!(signal_rows(&s.ws), vec![(1, vec![label.to_string()])], "[{label}] the row belongs to the stopped turn");
    }
}

/// C2: a file the AI wrote that changed after its turn's Stop, and the same request sent again.
#[test]
fn c2_file_edited_and_repeat() {
    for name in ["a-file-the-ai-wrote-changed-before-the-next-reply", "the-same-request-again"] {
        check_case(&case(name));
    }
    let s = home("c2-file");
    let session = sid(20);
    let t = transcript_in(&s, &session);
    let plan = s.ws.join("plan.md");
    std::fs::write(&plan, "1. import\n2. check\n").unwrap();
    let first = "write the plan for the import";
    transcripts::append(&t, &session, &[Ev::Prompt(first.into())]);
    prompt_hook(&s, &s.ws, &session, &t, first);
    transcripts::append(&t, &session, &[Ev::Write(plan.display().to_string()), Ev::Text("The plan is written.".into())]);
    stop_hook(&s, &s.ws, &session, &t, "The plan is written.");
    // The user rewrites the file between turns.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&plan, "1. check\n2. import\n3. report\n").unwrap();
    let second = "look at the plan once more";
    transcripts::append(&t, &session, &[Ev::Prompt(second.into())]);
    let out = prompt_hook(&s, &s.ws, &session, &t, second);
    assert_eq!(out.matches(CHECK_LINE).count(), 1, "{out}");
    assert_eq!(signal_rows(&s.ws), vec![(1, vec!["C2 file-edited".to_string()])]);

    // The same request again, both prompts typed.
    let third = "close down all of the infra for this, off for the weekend";
    transcripts::append(&t, &session, &[Ev::Text("Following your order.".into()), Ev::Prompt(third.into())]);
    stop_hook(&s, &s.ws, &session, &t, "Following your order.");
    let out = prompt_hook(&s, &s.ws, &session, &t, third);
    assert!(!out.contains(CHECK_LINE), "a new request is not a repeat:\n{out}");
    let fourth = "close down all infra, off for weekend";
    transcripts::append(&t, &session, &[Ev::Text("Closing down.".into()), Ev::Prompt(fourth.into())]);
    let out = prompt_hook(&s, &s.ws, &session, &t, fourth);
    assert_eq!(out.matches(CHECK_LINE).count(), 1, "{out}");
    assert!(signal_rows(&s.ws).contains(&(4, vec!["C2 repeat".to_string()])), "{:?}", signal_rows(&s.ws));
}

// ─── C3 ──────────────────────────────────────────────────────────────────────

/// C3 reads only the AI's own text. T2's rule text arrives in CLAUDE.md, in hook output, in a reminder and in a Read,
/// each with `UPDATED:` at a line start, and the AI quotes the marker in backticks and inside a code fence: no flag,
/// from the detector or from the Stop hook given the reply as Claude Code passes it.
#[test]
fn c3_marker_only_in_assistant_text() {
    let c = case("t2-rule-text-in-injected-context-is-not-a-marker");
    check_case(&c);
    check_case(&case("thinking-and-a-subagent-are-not-the-reply"));
    let s = home("c3");
    let session = sid(30);
    let t = transcript_in(&s, &session);
    // The case's events up to its last prompt, then the hooks.
    let last = c.events.iter().rposition(|e| matches!(e, Ev::Prompt(_))).unwrap();
    transcripts::append(&t, &session, &c.events[..last]);
    let first = "is the cache still warm";
    prompt_hook(&s, &s.ws, &session, &t, first);
    let reply = "Yes. Your rules make me print `UPDATED:` or `MISREAD:` when I change position, and none applies here.";
    stop_hook(&s, &s.ws, &session, &t, reply);
    assert!(signal_rows(&s.ws).is_empty(), "no marker in the AI's text: {:?}", signal_rows(&s.ws));
}

/// The four markers mean four things: UPDATED and CORRECTED (wrong), MISREAD (a context gap), DEFERRED (the AI held:
/// logged as a disagreement, and `base rule propose` refuses it, Example 3).
#[test]
fn c3_marker_meanings() {
    let c = case("each-marker-in-its-own-turn");
    check_case(&c);
    let s = home("c3-meanings");
    let t = transcript_in(&s, &sid(31));
    transcripts::write(&t, &sid(31), &c.events);
    let tp = t.display().to_string();
    let (code, _, err) = base_env(&s, &["rule", "propose", "--from-turn", "--transcript", &tp, "--dry-run"], &[]);
    assert_eq!(code, 1, "DEFERRED is never proposed: {err}");
    assert!(err.contains("DEFERRED") && err.contains("never proposed as a rule"), "{err}");
    let (code, out, err) = base_env(
        &s,
        &["rule", "propose", "--from-turn", "--transcript", &tp, "--prompt", "4", "--text", "Staging forwards to 9000.", "--new", "--domain", "infra", "--dry-run"],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("MISREAD, the AI misunderstood the ask"), "{out}");
    let (code, out, err) = base_env(
        &s,
        &["rule", "propose", "--from-turn", "--transcript", &tp, "--prompt", "2", "--text", "The server port is 8443.", "--new", "--domain", "infra", "--dry-run"],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("C3 UPDATED") && !out.contains("meaning:"), "UPDATED means wrong, the default:\n{out}");
}

// ─── C4 ──────────────────────────────────────────────────────────────────────

/// The C4 line appears once per flagged prompt: once when C1 and C2 both fire, not on the next ordinary prompt, never
/// on a task notification (the signal waits for the user's next prompt), and again on a new flag.
#[test]
fn c4_line_after_flag() {
    let s = home("c4");
    let session = sid(40);
    let t = transcript_in(&s, &session);
    let run = |p: &str| {
        transcripts::append(&t, &session, &[Ev::Prompt(p.into())]);
        prompt_hook(&s, &s.ws, &session, &t, p)
    };
    let out = run("summarise the open tickets");
    assert!(!out.contains(CHECK_LINE));
    transcripts::append(&t, &session, &[Ev::Text("There are twelve".into()), Ev::Interrupt]);
    let out = run("no, only the ones assigned to me, again");
    assert_eq!(out.matches(CHECK_LINE).count(), 1, "C1 and C2 together: one line\n{out}");
    transcripts::append(&t, &session, &[Ev::Text("Three are yours.".into())]);
    stop_hook(&s, &s.ws, &session, &t, "Three are yours.");
    let out = run("thanks, now draft the reply to the first one");
    assert!(!out.contains(CHECK_LINE), "an ordinary prompt after it:\n{out}");
    transcripts::append(&t, &session, &[Ev::Text("Drafting".into()), Ev::Interrupt]);
    // A task notification arrives before the user types again: no line on it.
    let note = "<task-notification>\n<task-id>t1</task-id>\n<event>build finished</event>\n</task-notification>";
    transcripts::append(&t, &session, &[Ev::Notification("build finished".into())]);
    let out = prompt_hook(&s, &s.ws, &session, &t, note);
    assert!(!out.contains(CHECK_LINE), "never on a task notification:\n{out}");
    let out = run("use the shorter template");
    assert_eq!(out.matches(CHECK_LINE).count(), 1, "the interrupt waited for the user's prompt:\n{out}");
    let out = run("why did you sign it with my full name");
    assert_eq!(out.matches(CHECK_LINE).count(), 1, "a new flag, a new line:\n{out}");
}

/// Every transcript line is read once, by whichever hook comes first: a marker read at Stop is not read again by the
/// next prompt, by a second Stop in the same turn, or after the session moves its cwd into another workspace, and the
/// payload's copy of the reply does not count it twice.
#[test]
fn signal_rows_read_each_line_once() {
    let s = home("once");
    let session = sid(50);
    let t = transcript_in(&s, &session);
    let first = "which port does the staging proxy use";
    transcripts::append(&t, &session, &[Ev::Prompt(first.into())]);
    prompt_hook(&s, &s.ws, &session, &t, first);
    let reply = "UPDATED: the staging proxy uses 9000 since the change last week.";
    transcripts::append(&t, &session, &[Ev::Text(reply.into())]);
    stop_hook(&s, &s.ws, &session, &t, reply);
    stop_hook(&s, &s.ws, &session, &t, reply);
    let other = s.ws.parent().unwrap().join("other-ws");
    std::fs::create_dir_all(other.join(".base")).unwrap();
    let second = "and the production proxy";
    transcripts::append(&t, &session, &[Ev::Prompt(second.into())]);
    prompt_hook(&s, &other, &session, &t, second);
    let c3 = |ws: &Path| signal_rows(ws).into_iter().flat_map(|(_, l)| l).filter(|l| l.starts_with("C3")).count();
    assert_eq!(c3(&s.ws) + c3(&other), 1, "one C3 signal: {:?} {:?}", signal_rows(&s.ws), signal_rows(&other));
    assert_eq!(signal_rows(&s.ws), vec![(1, vec!["C3 UPDATED".to_string()])]);
}

// ─── base rule propose ───────────────────────────────────────────────────────

/// One session through the real hooks: each prompt through the prompt hook, each reply through the Stop hook except
/// the last, whose marker `base rule propose` reads from the transcript itself (the AI runs it after answering).
fn session(s: &seed::Seed, session: &str, turns: &[(&str, &str)]) -> PathBuf {
    let t = transcript_in(s, session);
    for (i, (prompt, reply)) in turns.iter().enumerate() {
        transcripts::append(&t, session, &[Ev::Prompt((*prompt).into())]);
        prompt_hook(s, &s.ws, session, &t, prompt);
        transcripts::append(&t, session, &[Ev::Text((*reply).into())]);
        if i + 1 < turns.len() {
            stop_hook(s, &s.ws, session, &t, reply);
        }
    }
    t
}

/// The three kinds (K3), in the shapes of Examples 1 and 2. A rule served to the session and corrected anyway is a
/// rewrite; a decision that fits but was never served is a keyword gap, on the decision's own keywords; nothing that
/// fits is a new rule, in the domain the prompt matched. Running it again for the same turn replaces the proposal and
/// keeps its id; `--dry-run` writes nothing; `--new` overrides the sort.
#[test]
fn propose_kinds() {
    let s = home("propose");
    std::fs::write(
        s.ws.join(".base").join("domains.toml"),
        "[[domain]]\nname = \"release\"\nmode = \"triggered\"\nprompt_keywords = [\"release notes\"]\n\n\
         [[domain]]\nname = \"versions\"\nmode = \"triggered\"\nprompt_keywords = [\"installed version\"]\n\n\
         [[domain]]\nname = \"paths\"\nmode = \"always\"\nprompt_keywords = []\n",
    )
    .unwrap();
    let ok = |args: &[&str], env: &[(&str, &str)]| {
        let (code, out, err) = base_env(&s, args, env);
        assert_eq!(code, 0, "{args:?}\nstdout:\n{out}\nstderr:\n{err}");
        out
    };
    ok(&["rule", "add", "--domain", "release", "--text", "Release notes list user-facing changes only, never internal refactors."], &[]);
    ok(
        &[
            "decision", "log", "--domain", "paths", "--decision",
            "Path triggers name each project's own folder, never a broad parent folder",
            "--rationale", "a broad folder injects every project's rules",
        ],
        &[],
    );

    // Rewrite: the rule was served on the session's first prompt, and the AI broke it on the second.
    let a = sid(60);
    session(&s, &a, &[
        ("write the release notes for 2.3", "Here are the notes, with the parser refactor first."),
        ("no, the release notes must not list internal refactors", "UPDATED: release notes list user-facing changes only; the parser refactor is out."),
    ]);
    let out = ok(&["rule", "propose", "--from-turn", "--keywords", "release notes"], &[("CLAUDE_CODE_SESSION_ID", &a)]);
    assert!(out.starts_with("proposal p-0001 · rewrite · rule release."), "{out}");
    assert!(out.contains("evidence: session b015b015 prompt 2") && out.contains("C3 UPDATED") && out.contains("C1 phrase"), "{out}");
    // BO-16 built the review, so the proposal now names it (lynx's G0 addition 3 to BO-15).
    assert!(out.contains("pending review: base rule review (p-0001)"), "the review command, named:\n{out}");

    // Keyword gap: the decision fits and nothing served it to this session.
    let b = sid(61);
    session(&s, &b, &[
        ("set up the scheduler for the nightly import", "The scheduler is set up."),
        (
            "I don't want broad folders as path triggers, give each project its own folder",
            "UPDATED: path triggers name each project's own folder, never a broad parent.",
        ),
    ]);
    let out = ok(&["rule", "propose", "--from-turn", "--keywords", "path triggers, broad folders"], &[("CLAUDE_CODE_SESSION_ID", &b)]);
    assert!(out.starts_with("proposal p-0002 · keyword gap · decision paths."), "{out}");
    assert!(out.contains("add keywords: path triggers, broad folders"), "{out}");

    // New rule (Example 1's shape): nothing fits, and the prompt matched `versions` by keyword.
    let c = sid(62);
    session(&s, &c, &[
        ("what version is installed", "The installed release is 0.15.2."),
        (
            "The installed version is not 0.15.2. It is a local dev build of 0.16.0. Now, quit trying to correct me.",
            "UPDATED: you built and installed it from the 0.16.0 dev tree; --version prints 0.15.2 until release.",
        ),
    ]);
    let text = "base --version prints the dev tree's Cargo.toml version; the build here is unreleased 0.16.0. Never read the version string as the installed release.";
    let out = ok(
        &["rule", "propose", "--from-turn", "--text", text, "--keywords", "version, installed, 0.15.2"],
        &[("CLAUDE_CODE_SESSION_ID", &c)],
    );
    assert!(out.starts_with("proposal p-0003 · new rule · domain versions"), "{out}");
    assert!(out.contains("the prompt matched it by keyword"), "{out}");
    assert!(out.contains("example, its first fires_on test: \"The installed version is not 0.15.2."), "{out}");
    assert_eq!(proposals(&s), 3);

    // Again for the same turn: replaced, same id.
    let out = ok(&["rule", "propose", "--from-turn", "--text", text, "--keywords", "version, installed"], &[("CLAUDE_CODE_SESSION_ID", &c)]);
    assert!(out.starts_with("proposal p-0003 · new rule") && out.contains("replaces this turn's earlier proposal"), "{out}");
    assert_eq!(proposals(&s), 3, "replaced, not added");
    // A dry run writes nothing; --new overrides the sort.
    let out = ok(
        &["rule", "propose", "--from-turn", "--new", "--text", "Notes skip refactors.", "--domain", "release", "--dry-run"],
        &[("CLAUDE_CODE_SESSION_ID", &a)],
    );
    assert!(out.contains("new rule · domain release") && out.contains("dry run: nothing written"), "{out}");
    assert_eq!(proposals(&s), 3, "a dry run writes nothing");
    let graph = std::fs::read_to_string(s.ws.join(".base").join("graph.nq")).unwrap();
    assert!(graph.contains("proposalId> \"p-0003\""), "the id is stored");
    assert!(graph.contains("status> \"pending\""), "pending review");
    assert!(graph.contains("firesOn> \"The installed version is not 0.15.2."), "the prompt is the first fires_on test");
}

// ─── base rule add inside a session ──────────────────────────────────────────

/// Inside a Claude Code session (`CLAUDECODE=1`) `base rule add` needs `--keywords` and `--fires-on`, says why and
/// points to `--path` for file work, and writes nothing without them. Outside a session nothing changed.
#[test]
fn rule_add_in_session_requires_keywords_and_example() {
    let s = home("rule-add");
    let in_session = [("CLAUDECODE", "1")];
    let before = std::fs::read_to_string(s.ws.join(".base").join("graph.nq")).unwrap_or_default();
    for args in [
        vec!["rule", "add", "--domain", "release", "--text", "Ship the notes on Fridays."],
        vec!["rule", "add", "--domain", "release", "--text", "Ship the notes on Fridays.", "--keywords", "release notes"],
        vec!["rule", "add", "--domain", "release", "--text", "Ship the notes on Fridays.", "--fires-on", "when do the notes ship"],
    ] {
        let (code, _, err) = base_env(&s, &args, &in_session);
        assert_eq!(code, 1, "{args:?} was accepted: {err}");
        assert!(err.contains("--keywords") && err.contains("--fires-on") && err.contains("--path"), "{err}");
    }
    let after = std::fs::read_to_string(s.ws.join(".base").join("graph.nq")).unwrap_or_default();
    assert_eq!(before, after, "a refused rule wrote nothing");

    let (code, out, err) = base_env(
        &s,
        &[
            "rule", "add", "--domain", "release", "--text", "Ship the notes on Fridays.",
            "--keywords", "release notes, friday", "--fires-on", "when do the release notes ship",
        ],
        &in_session,
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("added to domain 'release'") && out.contains("tests:"), "{out}");
    let graph = std::fs::read_to_string(s.ws.join(".base").join("graph.nq")).unwrap();
    assert!(graph.contains("matchWord> \"release notes\""), "the keywords are the rule's own words");
    assert!(graph.contains("firesOn> \"when do the release notes ship\""), "the prompt is its test");

    // `--words` is the same list as `--keywords`, so it carries the triggers too.
    let (code, out, err) = base_env(
        &s,
        &["rule", "add", "--domain", "release", "--text", "Notes go out after the tag.", "--words", "release tag", "--fires-on", "when do we tag the release"],
        &in_session,
    );
    assert_eq!(code, 0, "--words carries the triggers: {err}");
    assert!(out.contains("added to domain 'release'"), "{out}");

    let (code, out, err) = base_env(&s, &["rule", "add", "--domain", "release", "--text", "Tag every release."], &[]);
    assert_eq!(code, 0, "outside a session nothing changed: {err}");
    assert!(out.contains("added to domain 'release'"), "{out}");
}

// ─── A session that upgrades half way through ────────────────────────────────

/// A session already under way when base gains the detector reads none of its history: its old interrupt, refusal and
/// markers are not charged to the turn before the prompt that finds them, and that prompt carries no C4 line. From
/// there on it reads as any session does.
#[test]
fn a_session_that_upgrades_midway_reads_none_of_its_history() {
    let s = home("upgrade");
    let session = sid(80);
    let t = transcript_in(&s, &session);
    // An hour of work before the upgrade: an interrupt, a refusal, two marked corrections.
    transcripts::append(&t, &session, &[
        Ev::Prompt("set up the nightly import".into()),
        Ev::Text("Setting it up".into()),
        Ev::Interrupt,
        Ev::Prompt("use the staging bucket".into()),
        Ev::Denial,
        Ev::InterruptToolUse,
        Ev::Prompt("the bucket is import-staging".into()),
        Ev::Text("UPDATED: the bucket is import-staging, not imports.".into()),
        Ev::Prompt("and the schedule is 02:00".into()),
        Ev::Text("MISREAD: I read the schedule as UTC.".into()),
    ]);
    let now = "add a retry to the import job";
    transcripts::append(&t, &session, &[Ev::Prompt(now.into())]);
    let out = prompt_hook(&s, &s.ws, &session, &t, now);
    assert!(!out.contains(CHECK_LINE), "nothing in the history is this turn's:\n{out}");
    assert!(signal_rows(&s.ws).is_empty(), "no row for the history: {:?}", signal_rows(&s.ws));
    // From here on, the session reads as any other.
    transcripts::append(&t, &session, &[Ev::Text("UPDATED: the job retries three times.".into())]);
    stop_hook(&s, &s.ws, &session, &t, "UPDATED: the job retries three times.");
    let rows = signal_rows(&s.ws);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, vec!["C3 UPDATED".to_string()], "{rows:?}");
}

// ─── base rule propose: one proposal per turn ────────────────────────────────

/// The same words sent on two prompts are two turns, and each keeps its own proposal. The turn before's repeat is that
/// turn's evidence, not the next one's. A new rule given no wording is refused with the records closest to it, so the
/// AI can name one with --rule or --decision instead.
#[test]
fn propose_keys_each_turn_and_names_the_closest_records() {
    let s = home("propose-turns");
    let session = sid(90);
    let t = transcript_in(&s, &session);
    let fix = "no, leave the database running and close only the proxy";
    transcripts::write(&t, &session, &[
        Ev::Prompt("close down the staging proxy and the database for the weekend".into()),
        Ev::Text("Closing both.".into()),
        Ev::Prompt("close down staging proxy and database for weekend".into()),
        Ev::Text("Both are closing.".into()),
        Ev::Prompt(fix.into()),
        Ev::Text("UPDATED: the database stays up; only the proxy closes.".into()),
        Ev::Prompt(fix.into()),
        Ev::Text("UPDATED: the database is up again and the proxy is closed.".into()),
    ]);
    let tp = t.display().to_string();
    let propose = |n: &str| {
        let (code, out, err) = base_env(
            &s,
            &["rule", "propose", "--from-turn", "--transcript", &tp, "--prompt", n, "--text", "Weekend close-down stops only the proxy.", "--new", "--domain", "infra"],
            &[],
        );
        assert_eq!(code, 0, "prompt {n}: {err}");
        out
    };
    let third = propose("3");
    assert!(third.starts_with("proposal p-0001 · new rule"), "{third}");
    assert!(third.contains("C3 UPDATED") && !third.contains("C2 repeat"), "the second prompt's repeat is not the third's:\n{third}");
    let fourth = propose("4");
    assert!(fourth.starts_with("proposal p-0002 · new rule"), "the same words on another prompt are another turn:\n{fourth}");
    assert!(fourth.contains("C2 repeat"), "the fourth prompt repeats the third: {fourth}");
    assert_eq!(proposals(&s), 2, "both kept");

    let (code, _, err) = base_env(&s, &["rule", "propose", "--from-turn", "--transcript", &tp, "--prompt", "3", "--new"], &[]);
    assert_eq!(code, 1, "a new rule needs its wording: {err}");
    assert!(err.contains("--text") && err.contains("--decision") && err.contains("Closest:"), "{err}");
    assert_eq!(proposals(&s), 2, "the refused one wrote nothing");
}

// ─── The CORRECTED line at session start ─────────────────────────────────────

/// A user whose CLAUDE.md asks for no marker (declined, or never asked) gets the CORRECTED line at session start, once;
/// one whose CLAUDE.md asks for `UPDATED:` (Chris's T2, D10) or `CORRECTED:` never does.
#[test]
fn session_start_carries_corrected_line_when_declined() {
    let s = home("start-line");
    let (code, out, err) = seed::run_session_start(&s, Some(&sid(70)));
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.matches(CORRECTED_LINE).count(), 1, "the line, once:\n{out}");
    let claude = s.home.join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    for covered in ["When you change position, print UPDATED: <why>.", CORRECTED_LINE] {
        std::fs::write(claude.join("CLAUDE.md"), covered).unwrap();
        let (_, out, _) = seed::run_session_start(&s, Some(&sid(71)));
        assert!(!out.contains(CORRECTED_LINE) || out.matches(CORRECTED_LINE).count() == 0, "covered by {covered:?}:\n{out}");
    }
}
