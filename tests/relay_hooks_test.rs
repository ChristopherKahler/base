//! BO-04: relay takes one line in the hooks and never outranks the user (F4, F13).
//!
//! Driven through the real binary, as Claude Code drives it: session start, prompt submit and pre-tool use with JSON
//! on stdin, `base relay ...` with `CLAUDE_CODE_SESSION_ID` set. Measured before this order, on 2026-10-01: the
//! 3,400-byte wake contract in prompt-submit on six prompts and on ordinary Bash and ToolSearch calls, every prompt
//! and many tool calls carrying "Reply RIGHT NOW", and a withdrawn message shown again after the session had
//! answered the one that withdrew it.

mod seed;

use std::path::PathBuf;

use seed::{run_base, run_base_in_session, run_pre_tool_use, run_prompt_submit, run_session_start};

/// What only the watcher script and the old wake contract carry. `relay_arm_prints_host_monitor_fields` is the
/// control that each one is really in the text `base relay arm` prints, so their absence elsewhere means something.
const SCRIPT_MARKERS: [&str; 4] = ["RELAY WAKE CONTRACT", "$INBOX/.watching", "while true; do", "persistent"];

/// Every phrase F13a removes, and the urgency marks the old relay text opened with.
const PRIORITY_CLAIMS: [&str; 12] = [
    "REPLY REQUIRED",
    "BEFORE YOUR NEXT ACTION",
    "DIRECTIVE",
    "PAUSE",
    "RIGHT NOW",
    "IMMEDIATELY",
    "\u{1F6A8}",
    "\u{2757}",
    "\u{26A0}",
    "\u{1F514}",
    "\u{1F4E8}",
    "\u{23F3}",
];

/// The title the seed's hook runner delivers the spool as (`BASE_RELAY_AS`), registered here too, so the inbox and
/// the spool reach the same session.
const TITLE: &str = "seed-kite";

fn root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("base-relay-hooks-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(!root.exists(), "the seed root {} survived its clean", root.display());
    root
}

/// A seed whose budgets fit everything, so a block is never missing for a budget reason.
fn fixture(tag: &str) -> seed::Seed {
    seed::write(&root(tag), &seed::TINY, "[budget]\nprompt_bytes = 50000\nsession_start_bytes = 50000\n")
}

fn ok(out: (i32, String, String), what: &str) -> String {
    let (code, stdout, stderr) = out;
    assert_eq!(code, 0, "{what} failed: {stderr}\n{stdout}");
    stdout
}

fn register(s: &seed::Seed, session: &str) -> String {
    ok(run_base_in_session(s, &["relay", "register", "--as", TITLE], session), "relay register")
}

fn ping(s: &seed::Seed, from: &str, msg: &str) {
    ok(run_base(s, &["relay", "ping", "--from", from, "--to", TITLE, "--msg", msg]), "relay ping");
}

fn prompt(s: &seed::Seed, session: &str) -> String {
    ok(run_prompt_submit(s, "carry on with the build", Some(session)), "prompt hook")
}

fn start(s: &seed::Seed, session: &str) -> String {
    ok(run_session_start(s, Some(session)), "session start")
}

fn tool(s: &seed::Seed, session: &str) -> String {
    ok(run_pre_tool_use(s, "Bash", serde_json::json!({ "command": "ls" }), session, &[]), "pre-tool hook")
}

/// The inbox files for the title, parsed.
fn inbox(s: &seed::Seed) -> Vec<serde_json::Value> {
    let dir = s.home.join(".base-gbl").join(".base").join("relay-inbox").join(TITLE);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap())
        .collect()
}

/// F4a and F4b: the hooks never carry the script or the old contract; the one line does.
#[test]
fn wake_contract_never_in_prompt_submit_or_pre_tool() {
    let s = fixture("no-contract");
    let session = "sess-contract";
    let registered = register(&s, session);
    assert!(registered.contains("$INBOX/.watching"), "control: register prints the watcher setup:\n{registered}");

    let mut outputs = vec![("session start", start(&s, session))];
    for _ in 0..3 {
        outputs.push(("prompt", prompt(&s, session)));
        outputs.push(("pre-tool", tool(&s, session)));
    }
    for (hook, out) in &outputs {
        for marker in SCRIPT_MARKERS {
            assert!(!out.contains(marker), "{hook} carried {marker:?}:\n{out}");
        }
    }
    let line = format!("relay: {TITLE} has no inbox watcher · run base relay arm and start the Monitor it prints");
    assert!(outputs[0].1.contains(&line), "session start carries the one line:\n{}", outputs[0].1);
    let total: usize = outputs.iter().map(|(_, o)| o.matches(&line).count()).sum();
    assert_eq!(total, 1, "the line is said once in this session, at session start:\n{outputs:#?}");

    // A new session holding the title is told once, on its first prompt.
    let later = "sess-contract-2";
    register(&s, later);
    let first = prompt(&s, later);
    assert!(first.contains(&line), "{first}");
    assert!(!prompt(&s, later).contains("inbox watcher"), "and not again");
}

/// F4a and F4c: `base relay arm` prints the host Monitor tool's fields, the re-arm step and the status line.
#[test]
fn relay_arm_prints_host_monitor_fields() {
    let s = fixture("arm");
    let session = "sess-arm";
    register(&s, session);
    let text = ok(run_base_in_session(&s, &["relay", "arm"], session), "relay arm");
    for marker in &SCRIPT_MARKERS[1..3] {
        assert!(text.contains(marker), "control: the script is in the arm text ({marker:?}):\n{text}");
    }
    assert!(text.contains(&format!("description: relay wake: {TITLE}\n")), "{text}");
    assert!(text.contains("timeout_ms:  1800000   (the longest this Claude Code allows"), "{text}");
    assert!(text.contains("command:\n    INBOX=\""), "{text}");
    assert!(text.contains("When the Monitor reports that it expired, run `base relay arm` again."), "{text}");
    assert!(text.contains("Status line: echo \"<what you are working on>\" > "), "{text}");
    assert!(text.contains(&format!("Not registered yet? base relay register --as {TITLE}")), "{text}");
    assert!(!text.contains("persistent"), "the host Monitor tool has no persistent field:\n{text}");
    // BO-05: the watcher prints only the pings addressed to the session holding the title.
    assert!(text.contains(&format!("SESSION=\"{session}\"")), "{text}");

    // Named explicitly, any title.
    let other = ok(run_base_in_session(&s, &["relay", "arm", "--as", "lark"], session), "relay arm --as");
    assert!(other.contains("description: relay wake: lark\n"), "{other}");

    // A session holding no title is told how to get one, and the command fails.
    let (code, out, err) = run_base_in_session(&s, &["relay", "arm"], "sess-untitled");
    assert_ne!(code, 0, "no title must fail:\n{out}");
    assert!(err.contains("holds no relay title"), "{err}");
}

/// F13a: nothing relay prints, in any hook or on registering, claims to come before the user.
#[test]
fn relay_text_has_no_priority_claims() {
    let s = fixture("no-claims");
    ok(run_base(&s, &["relay", "init", "--project", "crew"]), "relay init");
    let session = "sess-claims";
    let mut texts = vec![register(&s, session)];
    ping(&s, "bison", "a question for you");
    ok(run_base(&s, &["relay", "ping", "--to", TITLE, "--msg", "from nobody"]), "untitled ping");
    ok(
        run_base(&s, &["relay", "task", "--to", TITLE, "--slug", "wire-the-form", "--summary", "wire the form", "--from", "heron"]),
        "relay task",
    );
    ok(
        run_base(&s, &["relay", "send", "--project", "crew", "--from", "lark", "--to", TITLE, "--type", "question", "--msg", "which schema?"]),
        "relay send",
    );
    texts.push(start(&s, session));
    // A reply: the session asks heron, and heron answers.
    ok(run_base_in_session(&s, &["relay", "register", "--as", "heron"], "sess-heron"), "register heron");
    ok(run_base_in_session(&s, &["relay", "ping", "--to", "heron", "--msg", "need the status"], session), "ask heron");
    ok(run_base_in_session(&s, &["relay", "ping", "--to", TITLE, "--msg", "status is green"], "sess-heron"), "heron answers");
    texts.push(prompt(&s, session));
    texts.push(start(&s, session));
    texts.push(tool(&s, session));
    let all = texts.join("\n");
    for needle in ["relay: ping from bison", "relay: task wire-the-form", "relay (crew): question from lark", "relay: reply from heron", "unanswered ping"] {
        assert!(all.contains(needle), "control: {needle:?} was rendered somewhere:\n{all}");
    }
    for claim in PRIORITY_CLAIMS {
        assert!(!all.contains(claim), "{claim:?} in relay text:\n{all}");
    }
}

/// F13b: relay content is never on an interactive session's tool call, and a tool call consumes nothing. An interactive
/// session may carry `BASE_RELAY_AS` (the operator's launchers set it on every one), and that changes nothing.
#[test]
fn relay_never_in_pre_tool() {
    let s = fixture("no-pre-tool");
    ok(run_base(&s, &["relay", "init", "--project", "crew"]), "relay init");
    let session = "sess-tool";
    register(&s, session);
    ping(&s, "bison", "status of the build?");
    ok(
        run_base(&s, &["relay", "send", "--project", "crew", "--from", "lark", "--to", TITLE, "--type", "notify", "--msg", "spool news"]),
        "relay send",
    );
    let interactive: [&[(&str, &str)]; 2] = [&[], &[("BASE_RELAY_AS", TITLE)]];
    for env in interactive {
        for (tool_name, input) in [
            ("Bash", serde_json::json!({ "command": "ls" })),
            ("ToolSearch", serde_json::json!({ "query": "select:Monitor" })),
            ("Read", serde_json::json!({ "file_path": s.ws.join("notes.md").display().to_string() })),
        ] {
            let out = ok(run_pre_tool_use(&s, tool_name, input, session, env), "pre-tool hook");
            for needle in ["relay:", "relay (", "status of the build?", "spool news", "inbox watcher"] {
                assert!(!out.contains(needle), "{tool_name} {env:?}: {needle:?} on a tool call:\n{out}");
            }
            for marker in SCRIPT_MARKERS {
                assert!(!out.contains(marker), "{tool_name} {env:?}: {marker:?} on a tool call:\n{out}");
            }
        }
    }
    let ping_file = inbox(&s).into_iter().find(|t| t["kind"] == "ping").expect("the ping is in the inbox");
    assert_eq!(ping_file["status"], "pending", "a tool call recorded the ping as delivered: {ping_file}");
    let next = prompt(&s, session);
    assert!(next.contains("status of the build?") && next.contains("spool news"), "the next prompt shows both:\n{next}");
}

/// F13b, example 3: a new ping is shown once, on the next prompt; while unanswered, session start lists it in one
/// line; a reply ends it.
#[test]
fn new_ping_shown_once_then_listed_at_session_start() {
    let s = fixture("ping-once");
    let session = "sess-once";
    register(&s, session);
    ping(&s, "bison", "stand down on the progress doc");

    let first = prompt(&s, session);
    let header = first.lines().find(|l| l.starts_with("relay: ping from bison (")).unwrap_or_else(|| panic!("{first}"));
    assert!(header.ends_with(" · reply: base relay ping --to bison --msg \"...\""), "{header}");
    assert!(first.lines().any(|l| l == "stand down on the progress doc"), "{first}");
    for _ in 0..2 {
        let again = prompt(&s, session);
        assert!(!again.contains("stand down") && !again.contains("relay: ping"), "repeated on a prompt:\n{again}");
    }

    let listed = start(&s, session);
    let line = listed.lines().find(|l| l.starts_with("relay: 1 unanswered ping (bison ")).unwrap_or_else(|| panic!("{listed}"));
    assert!(line.contains("· read: base relay tasks --from <sender>"), "{line}");
    assert!(!listed.contains("stand down"), "session start lists, it does not show again:\n{listed}");
    assert!(!prompt(&s, session).contains("relay:"), "and the prompt after it says nothing");

    // The listing command shows the text.
    let tasks = ok(run_base(&s, &["relay", "tasks", "--from", "bison"]), "relay tasks");
    assert!(tasks.contains("stand down on the progress doc"), "{tasks}");

    // A reply clears it.
    ok(run_base_in_session(&s, &["relay", "register", "--as", "bison"], "sess-bison"), "register bison");
    ok(run_base_in_session(&s, &["relay", "ping", "--to", "bison", "--msg", "done"], session), "reply");
    assert!(!start(&s, session).contains("unanswered"), "a reply ends the listing");

    // BO-05 (F12c) replaced "the successor holding the title gets the same one line at its start": a successor
    // starts with an empty inbox. What was sent to this session is archived, and its sender told.
    ping(&s, "bison", "one more for the first session");
    let successor = "sess-once-2";
    register(&s, successor);
    let first = start(&s, successor);
    assert!(!first.contains("unanswered") && !first.contains("one more for the first session"), "{first}");
    assert!(inbox(&s).is_empty(), "the successor's inbox starts empty");
}

/// F13c, example 4: a superseded message is never shown, a delivered one never again, and one message sent through
/// both channels (the spool and its wake notify) is shown once.
#[test]
fn delivered_or_superseded_message_never_shown_again() {
    let s = fixture("never-again");
    ok(run_base(&s, &["relay", "init", "--project", "crew"]), "relay init");
    let session = "sess-again";
    register(&s, session);

    // Example 4: bison's "go", then "stand down" before the next prompt.
    ping(&s, "bison", "go: edit the doc");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ping(&s, "bison", "stand down, the go is withdrawn");
    let first = prompt(&s, session);
    assert!(first.contains("stand down, the go is withdrawn"), "{first}");
    assert!(!first.contains("go: edit the doc"), "a superseded message was shown:\n{first}");
    assert!(first.contains("(1 earlier message from bison hidden, this one is newer: base relay tasks --from bison)"), "{first}");
    // Hidden, never deleted: the named command still shows it.
    let listed = ok(run_base(&s, &["relay", "tasks", "--from", "bison"]), "relay tasks");
    assert!(listed.contains("go: edit the doc") && listed.contains("[superseded,"), "{listed}");

    // The session answers bison: nothing from bison is shown again, on a prompt or at a start.
    ok(run_base_in_session(&s, &["relay", "register", "--as", "bison"], "sess-bison"), "register bison");
    ok(run_base_in_session(&s, &["relay", "ping", "--to", "bison", "--msg", "standing down"], session), "reply");
    let after = [prompt(&s, session), start(&s, session), prompt(&s, session)].join("\n");
    assert!(!after.contains("go: edit the doc") && !after.contains("stand down, the go"), "{after}");

    // One spool message, which `relay send` also drops as a wake notify: shown once across both channels.
    ok(
        run_base(&s, &["relay", "send", "--project", "crew", "--from", "lark", "--to", TITLE, "--type", "notify", "--msg", "the schema is frozen"]),
        "relay send",
    );
    assert!(inbox(&s).iter().any(|t| t["kind"] == "notify"), "control: the send dropped a wake notify");
    let shown = prompt(&s, session);
    assert_eq!(shown.matches("the schema is frozen").count(), 1, "shown once, not once per channel:\n{shown}");
    let later = [prompt(&s, session), start(&s, session)].join("\n");
    assert!(!later.contains("the schema is frozen"), "a delivered message was shown again:\n{later}");
    let polled = ok(run_base(&s, &["relay", "poll", "--project", "crew", "--for", TITLE, "--peek"]), "relay poll");
    assert!(polled.contains("No pending messages"), "the spool copy was marked seen with the notify:\n{polled}");
}

/// Lynx's amendment to F13b, its three conditions. A run that has said it cannot keep an inbox watcher
/// (`BASE_NO_WAKE_NUDGE`) gets each NEW ping, reply, notify and task once, on its next tool call, in the plain F13a
/// form, with no watcher line and never the script; and what a tool call showed, no later tool call or prompt shows
/// again. (Condition 1, no relay on a tool call when the variable is not set, is `relay_never_in_pre_tool`.)
#[test]
fn monitorless_run_gets_new_items_once_on_a_tool_call() {
    let s = fixture("monitorless");
    ok(run_base(&s, &["relay", "init", "--project", "crew"]), "relay init");
    let session = "sess-sdk";
    register(&s, session);
    ping(&s, "bison", "which schema is live?");
    ok(
        run_base(&s, &["relay", "task", "--to", TITLE, "--slug", "wire-the-form", "--summary", "wire the form", "--from", "heron"]),
        "relay task",
    );
    ok(
        run_base(&s, &["relay", "send", "--project", "crew", "--from", "lark", "--to", TITLE, "--type", "notify", "--msg", "the schema is frozen"]),
        "relay send",
    );
    ok(run_base_in_session(&s, &["relay", "register", "--as", "heron"], "sess-heron"), "register heron");
    ok(run_base_in_session(&s, &["relay", "ping", "--to", "heron", "--msg", "status?"], session), "ask heron");
    ok(run_base_in_session(&s, &["relay", "ping", "--to", TITLE, "--msg", "status is green"], "sess-heron"), "heron answers");

    let sdk: &[(&str, &str)] = &[("BASE_NO_WAKE_NUDGE", "1")];
    let ls = || serde_json::json!({ "command": "ls" });
    let first = ok(run_pre_tool_use(&s, "Bash", ls(), session, sdk), "pre-tool hook");
    let texts = ["which schema is live?", "wire the form", "the schema is frozen", "status is green"];
    for needle in ["relay: ping from bison (", "relay: task wire-the-form", "relay (crew): notify from lark (", "relay: reply from heron ("]
        .iter()
        .chain(texts.iter())
    {
        assert!(first.contains(needle), "{needle:?} was not on the first tool call:\n{first}");
    }
    for marker in SCRIPT_MARKERS {
        assert!(!first.contains(marker), "{marker:?} on a tool call:\n{first}");
    }
    assert!(!first.contains("inbox watcher") && !first.contains("base relay arm"), "a watcher line on a tool call:\n{first}");
    for claim in PRIORITY_CLAIMS {
        assert!(!first.contains(claim), "{claim:?} in relay text:\n{first}");
    }

    // Once: not on the next tool call, and not on the next prompt.
    let again = ok(run_pre_tool_use(&s, "Bash", ls(), session, sdk), "pre-tool hook");
    assert!(!again.contains("relay:") && !again.contains("relay ("), "shown twice on tool calls:\n{again}");
    let next = prompt(&s, session);
    for text in texts {
        assert!(!next.contains(text), "{text:?} shown again on the prompt after the tool call:\n{next}");
    }
}
