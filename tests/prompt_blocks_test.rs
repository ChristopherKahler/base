//! BO-01 (F1, F2, F7, D15) at the surface Claude reads: the prompt hook's stdout from the binary `cargo test`
//! builds, its row in `hook-output.jsonl`, and `base hooks show`.
//!
//! WHY THIS FILE EXISTS. Until BO-01 the prompt hook kept whole lines from the top until `[budget] prompt_bytes` ran
//! out. Measured on the operator's machine, 2026-09-23 to 2026-10-01: 388 of 1,027 prompts with something to say were
//! cut and 59% of the bytes lost; on 2026-10-01 the cut stopped at line 37 of a 47-line wake script the reader was told
//! to arm, and the bracket rules went out ahead of every rule matched to the prompt. The unit tests in `emit::prompt`
//! pin the fit itself; these drive the real hook, so the blocks are the ones the hook builds.
//!
//! The fixture carries every block type the main path builds: a keyword domain's rules (1) and CONTEXT (2), a topic
//! rule (1), a relay ping (3) and the wake contract (3), an always-on domain (4), an `always` rule (4), the grounding
//! block (4) and the bracket rules (5). Star commands, DEVMODE, the relay spool and the walk are separate runs in
//! `prompt_submit_pointer_commands_run`, because each one changes what the main path builds.

mod seed;

use std::collections::BTreeSet;
use std::path::PathBuf;

use seed::{run_base, run_base_in_session, run_prompt_submit, PromptBlocksFile as BlocksFile, PromptRow as Row};

fn root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("base-prompt-blocks-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(!root.exists(), "the seed root {} survived its clean", root.display());
    root
}

/// The seed plus every main-path block type. `rules` long rules go on the keyword domain, so a test can make the
/// matched block as large as it needs. `extra` is appended to the global base.toml.
fn fixture(tag: &str, budget: usize, rules: usize, extra: &str) -> seed::Seed {
    let global = format!(
        "[bracket]\nenabled = true\nfresh_until = 0\n\n[bracket.rules]\nalways = [\
         \"Bracket rule A: state claims flat and give a confidence number for anything not certain.\", \
         \"Bracket rule B: read before reasoning and name the file read for every claim made.\"]\n\n\
         [budget]\nprompt_bytes = {budget}\n\n[grounding]\nenabled = true\n\n{extra}"
    );
    let s = seed::write(&root(tag), &seed::TINY, &global);
    let mut hooks_rules: Vec<String> = vec![
        "\"Hooks rule: measure hook output on a copy of the store, never the live one.\"".into(),
        "\"Hooks rule: a cut is reported with its size, never silent.\"".into(),
    ];
    for i in 0..rules {
        hooks_rules.push(format!(
            "\"Hooks long rule {i}: this rule is long on purpose, so the block matched to the prompt is large \
             enough that what ranks below it has to compete for the budget that is left after it.\""
        ));
    }
    let domains = format!(
        "[[domain]]\nname = \"global\"\nmode = \"always\"\nrules = [\
         \"Global rule one: never give time estimates without measured grounding.\", \
         \"Global rule two: deliverables are files on disk and their paths go in the report.\"]\n\n\
         [[domain]]\nname = \"hooks\"\nprompt_keywords = [\"hook\", \"hooks\"]\nrules = [{}]\n",
        hooks_rules.join(", ")
    );
    std::fs::write(s.ws.join(".base").join("domains.toml"), domains).expect("domains.toml");
    std::fs::write(
        s.ws.join(".base").join("commands.toml"),
        "[[command]]\nname = \"audit\"\ndescription = \"Audit mode\"\nrules = [\"Audit rule one.\", \"Audit rule two.\"]\n",
    )
    .expect("commands.toml");
    for args in [
        vec!["decision", "log", "--domain", "hooks", "--decision", "Prompt blocks drop whole", "--rationale", "a cut block reads as an instruction"],
        vec!["rule", "add", "--domain", "hooks", "--text", "Topic rule: the budget counts bytes, not characters.", "--words", "budget, bytes"],
        vec!["rule", "add", "--domain", "global", "--text", "Always rule: report in your own terminal.", "--kind", "always"],
    ] {
        let (code, _, err) = run_base(&s, &args);
        assert_eq!(code, 0, "base {args:?}: {err}");
    }
    s
}

/// The header of a session's first prompt in the fixture (`fresh_until = 0`).
const FIRST_HEADER: &str = "<context-bracket>[MODERATE] (prompt 1)</context-bracket>\n";

/// The budget at which a prompt that builds `measured`'s blocks prints exactly this: every block `drop` names dropped,
/// unless it is no longer than its own pointer line (the fit never drops those), and every other block printed. The
/// pointer lines name the budget, so its digit count is part of the size: settle on a fixed point.
fn budget_dropping(measured: &BlocksFile, drop: impl Fn(&Row) -> bool) -> usize {
    let mut probe = measured.clone();
    probe.budget_bytes = 1000;
    for _ in 0..4 {
        let b = probe.budget_bytes;
        for (row, m) in probe.blocks.iter_mut().zip(&measured.blocks) {
            row.printed = !(drop(m) && m.bytes > pointer(m, b).len());
        }
        probe.budget_bytes = rebuilt(FIRST_HEADER, &probe).len();
    }
    probe.budget_bytes
}

/// Rewrite `[budget] prompt_bytes` in the seed's global base.toml.
fn set_budget(s: &seed::Seed, bytes: usize) {
    let path = s.home.join(".base-gbl").join("base.toml");
    let text = std::fs::read_to_string(&path).expect("base.toml");
    let line = text.lines().find(|l| l.starts_with("prompt_bytes = ")).expect("the fixture sets prompt_bytes").to_string();
    std::fs::write(&path, text.replace(&line, &format!("prompt_bytes = {bytes}"))).expect("base.toml");
}

/// Bind `title` to `session` and leave it a ping, so the prompt hook delivers a relay block and the watcher nudge.
fn ping(s: &seed::Seed, session: &str, title: &str) {
    ping_sized(s, session, title, 0);
}

/// [`ping`] with a message of about `bytes` bytes. Before BO-04 the 3.4 KB wake contract was the relay block that
/// pushed these fixtures over budget; it is one line now, and a long ping (a real one runs to 800 characters) is what
/// still makes relay compete for the budget.
fn ping_sized(s: &seed::Seed, session: &str, title: &str, bytes: usize) {
    let (code, _, err) = run_base_in_session(s, &["relay", "register", "--as", title], session);
    assert_eq!(code, 0, "register: {err}");
    let mut msg = String::from("please ack the hook change");
    while msg.len() < bytes {
        msg.push_str("; the build order moved and the brief has a new section to read before the next step");
    }
    let (code, _, err) = run_base(s, &["relay", "ping", "--to", title, "--msg", &msg]);
    assert_eq!(code, 0, "ping: {err}");
}

/// A spool message of about `bytes` bytes for the title the prompt hook delivers the spool as (`BASE_RELAY_AS`, set by
/// the seed's runner), so the relay inbox block is built beside the ping block.
fn spool_sized(s: &seed::Seed, bytes: usize) {
    let (code, _, err) = run_base(s, &["relay", "init", "--project", "crew"]);
    assert_eq!(code, 0, "relay init: {err}");
    let mut msg = String::from("spool note for the hook");
    while msg.len() < bytes {
        msg.push_str("; the shared schema changed and the worker queue was re-cut, see the board before claiming");
    }
    let (code, _, err) =
        run_base(s, &["relay", "send", "--project", "crew", "--from", "seed-lark", "--to", "seed-kite", "--type", "notify", "--msg", &msg]);
    assert_eq!(code, 0, "relay send: {err}");
}

fn blocks(s: &seed::Seed, session: &str) -> BlocksFile {
    seed::prompt_blocks(&s.ws, session)
}

fn last_prompt_record(s: &seed::Seed) -> serde_json::Value {
    let path = s.ws.join(".base").join("hook-output.jsonl");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["hook"] == "user-prompt-submit")
        .expect("a prompt record")
}

fn pointer(row: &Row, budget: usize) -> String {
    seed::pointer_line(row, budget)
}

fn rebuilt(stdout: &str, file: &BlocksFile) -> String {
    seed::rebuilt_prompt_output(stdout, file)
}

/// The type of a block, from its id: the fixed ids name themselves, and the per-domain, per-name and per-command ids
/// name their kind.
fn block_type(id: &str) -> &str {
    match id {
        "relay-inbox" | "relay-tasks" | "relay-wake" | "grounding" | "bracket-rules" | "devmode" | "always-rules" => id,
        _ if id.starts_with("walk-") => "walk",
        _ if id.starts_with("command-") => "command",
        _ if id.ends_with("-topic-rules") => "topic-rules",
        _ if id.ends_with("-context") => "context",
        _ if id.ends_with("-rules") => "rules",
        _ => id,
    }
}

fn run(s: &seed::Seed, prompt: &str, session: &str) -> String {
    let (code, stdout, stderr) = run_prompt_submit(s, prompt, Some(session));
    assert_eq!(code, 0, "the prompt hook failed: {stderr}");
    stdout
}

#[test]
fn prompt_submit_over_budget_never_ends_mid_block() {
    let s = fixture("mid-block", 2500, 6, "");
    ping_sized(&s, "mid-block-session", "kite-mid", 2_000);
    spool_sized(&s, 2_000);
    let stdout = run(&s, "fix the hook budget bytes", "mid-block-session");
    let file = blocks(&s, "mid-block-session");

    assert_eq!(stdout, rebuilt(&stdout, &file), "the output is whole blocks and pointer lines, nothing else");
    assert!(stdout.len() <= 2500, "{} bytes against 2,500", stdout.len());
    assert!(stdout.ends_with("]\n") || file.blocks.iter().any(|r| r.printed && stdout.ends_with(&format!("{}\n", r.text))),
        "the output ends at a block boundary:\n{stdout}");
    for row in file.blocks.iter().filter(|r| !r.printed) {
        let probe: String = row.text.chars().take(60).collect();
        assert!(!stdout.contains(&probe), "{} was dropped, yet part of it was printed", row.id);
    }
    // The output is in priority order, and within one priority in the order the hook built it.
    let order: Vec<u8> = file.blocks.iter().map(|r| r.priority).collect();
    assert!(order.windows(2).all(|w| w[0] <= w[1]), "blocks out of priority order: {order:?}");

    // Controls: the fixture is over budget, drops more than one block, and carries every main-path priority.
    let dropped = file.blocks.iter().filter(|r| !r.printed).count();
    assert!(dropped >= 2, "control: only {dropped} dropped:\n{stdout}");
    let priorities: BTreeSet<u8> = file.blocks.iter().map(|r| r.priority).collect();
    assert_eq!(priorities, (1..=5).collect(), "control: every priority is present");
    for id in ["hooks-rules", "hooks-context", "hooks-topic-rules", "relay-tasks", "relay-wake", "global-rules", "always-rules", "grounding", "bracket-rules"] {
        assert!(file.blocks.iter().any(|r| r.id == id), "control: no {id} block was built: {:?}",
            file.blocks.iter().map(|r| &r.id).collect::<Vec<_>>());
    }
}

/// F2 and lynx's re-admit condition on the real hook (the exact two-blocks case is the unit test of the same name
/// in `emit::prompt`). Every dropped block genuinely did not fit beside what was printed, and no printed block of a
/// lower priority holds room a dropped higher one could have used: printing the lower block's pointer line in its
/// place would still not make room for the higher block.
#[test]
fn prompt_submit_priority_order_holds_on_the_real_hook() {
    let s = fixture("priority", 2500, 6, "");
    ping_sized(&s, "priority-session", "kite-pri", 1_500);
    let stdout = run(&s, "fix the hook budget bytes", "priority-session");
    let file = blocks(&s, "priority-session");
    let budget = file.budget_bytes;
    let dropped: Vec<&Row> = file.blocks.iter().filter(|r| !r.printed).collect();
    assert!(!dropped.is_empty(), "control: something was dropped:\n{stdout}");
    for d in &dropped {
        let with_d = stdout.len() - pointer(d, budget).len() + d.bytes;
        assert!(with_d > budget, "{} was dropped but fits: {with_d} of {budget} bytes:\n{stdout}", d.id);
        for k in file.blocks.iter().filter(|k| k.printed && k.priority > d.priority) {
            let k_out = stdout.len() - k.bytes + pointer(k, budget).len();
            let d_in = k_out - pointer(d, budget).len() + d.bytes;
            assert!(
                d_in > budget,
                "{} (priority {}) holds room that {} (priority {}) needed: {d_in} of {budget} bytes",
                k.id, k.priority, d.id, d.priority
            );
        }
    }
    let order: Vec<u8> = file.blocks.iter().map(|r| r.priority).collect();
    assert!(order.windows(2).all(|w| w[0] <= w[1]), "blocks out of priority order: {order:?}");
    assert!(stdout.len() <= budget);
}

/// A relay block larger than the budget, against a 3,000-byte budget: dropped whole, its pointer line printed, and
/// `base hooks show relay-tasks` prints all of it. Until BO-04 the 3.7 KB wake contract was this block; it is one line
/// now, so a 3.2 KB ping stands in for it.
#[test]
fn prompt_submit_block_larger_than_budget_is_dropped_not_cut() {
    let s = fixture("oversized", 3000, 0, "");
    ping_sized(&s, "oversized-session", "kite-big", 3_200);
    let stdout = run(&s, "fix the hook budget bytes", "oversized-session");
    let file = blocks(&s, "oversized-session");
    let big = file.blocks.iter().find(|r| r.id == "relay-tasks").expect("control: the ping block was built");
    assert!(big.bytes > 3000, "control: the ping block is larger than the budget: {} bytes", big.bytes);
    assert!(!big.printed);
    assert!(!stdout.contains("please ack the hook change"), "no part of the ping block is printed:\n{stdout}");
    assert!(stdout.lines().any(|l| l == pointer(big, 3000)), "no pointer line for the ping block:\n{stdout}");
    assert!(stdout.len() <= 3000);
    // An oversized block does not take the blocks below it down with it: the re-admit puts back what fits.
    assert!(file.blocks.iter().any(|r| r.priority > 3 && r.printed), "everything below the ping block was lost:\n{stdout}");
    let (code, shown, err) = run_base_in_session(&s, &["hooks", "show", "relay-tasks"], "oversized-session");
    assert_eq!(code, 0, "{err}");
    assert_eq!(shown, format!("{}\n", big.text));
    assert!(shown.starts_with("relay: ping from "));
}

/// Example 2 (2026-10-01): the last line of one block and the first of the next arrived on one line,
/// `...companion runs on Brave<relay-ping-open>❗ UNANSWERED PING(s)...`. Every block starts on its own line.
/// BO-04 replaced the relay tags with lines that open `relay: `; those must open their line too.
#[test]
fn prompt_submit_blocks_start_on_their_own_line() {
    let s = fixture("own-line", 20_000, 0, "");
    ping(&s, "own-line-session", "kite-line");
    let stdout = run(&s, "fix the hook budget bytes", "own-line-session");
    let file = blocks(&s, "own-line-session");
    assert!(file.blocks.iter().all(|r| r.printed), "control: nothing dropped at 20,000 bytes");
    for row in &file.blocks {
        let first = row.text.lines().next().expect("a block has a line");
        assert!(
            stdout.contains(&format!("\n\n{first}\n")) || stdout.contains(&format!("\n\n{first}")),
            "{} does not start on its own line:\n{stdout}",
            row.id
        );
    }
    for line in stdout.lines() {
        if let Some(at) = line.find("relay: ") {
            assert_eq!(at, 0, "a relay block glued onto another block's line: {line:?}");
        }
    }
    assert!(stdout.contains("\n\nrelay: ping from "), "control: the relay ping is in the output");
    assert!(stdout.contains("has no inbox watcher"), "control: the watcher nudge is in the output");
}

/// Example 4 (F7): the record names every dropped block by name, items and bytes, reason `budget`, in session start's
/// row shape; `withheld_bytes` is their sum, and the record's sizes are what was printed.
#[test]
fn prompt_submit_withheld_blocks_logged_by_name_and_bytes() {
    let s = fixture("logged", 2500, 6, "");
    ping_sized(&s, "logged-session", "kite-log", 2_000);
    spool_sized(&s, 2_000);
    let stdout = run(&s, "fix the hook budget bytes", "logged-session");
    let file = blocks(&s, "logged-session");
    let record = last_prompt_record(&s);
    let want: Vec<serde_json::Value> = file
        .blocks
        .iter()
        .filter(|r| !r.printed)
        .map(|r| serde_json::json!({"block": r.id, "items": r.items, "bytes": r.bytes, "reason": "budget"}))
        .collect();
    assert!(want.len() >= 2, "control: blocks were dropped");
    assert_eq!(record["withheld"], serde_json::Value::Array(want.clone()), "every dropped block, by name");
    let sum: u64 = want.iter().map(|w| w["bytes"].as_u64().unwrap()).sum();
    assert_eq!(record["withheld_bytes"].as_u64(), Some(sum));
    assert_eq!(record["emitted_bytes"].as_u64(), Some(stdout.len() as u64), "the record is what was printed");
    assert_eq!(record["over_budget"], true);
    for w in &want {
        let id = w["block"].as_str().unwrap();
        assert!(stdout.contains(&format!("· full text: base hooks show {id}]")), "{id} has no pointer line");
    }
    // And `base doctor` names them (Example 5).
    let (_, doctor, _) = run_base(&s, &["doctor"]);
    let first = want[0]["block"].as_str().unwrap();
    assert!(
        doctor.lines().any(|l| l.contains("user-prompt-submit: last run") && l.contains(&format!("· withheld: {first} ("))),
        "doctor does not name the withheld blocks:\n{doctor}"
    );
}

/// Every block type's declared command runs and prints that block: `base hooks show <block>` in the session prints
/// exactly the block the hook built, printed or dropped. Across the main path, a star command, DEVMODE, the relay
/// spool and the walk, so every type the hook builds is covered.
#[test]
fn prompt_submit_pointer_commands_run() {
    let mut covered: BTreeSet<String> = BTreeSet::new();
    let mut check = |s: &seed::Seed, session: &str| {
        let file = blocks(s, session);
        for row in &file.blocks {
            let (code, shown, err) = run_base_in_session(s, &["hooks", "show", &row.id], session);
            assert_eq!(code, 0, "base hooks show {}: {err}", row.id);
            assert_eq!(shown, format!("{}\n", row.text), "base hooks show {} prints the block", row.id);
            covered.insert(block_type(&row.id).to_string());
        }
        file
    };

    // The main path, over budget, so dropped blocks are among those shown.
    let s = fixture("pointers", 2500, 6, "[devmode]\nenabled = true\n");
    ping(&s, "pointers-session", "kite-ptr");
    // The relay spool: a message for the title the hook delivers as (BASE_RELAY_AS, set by the seed's runner).
    let (code, _, err) = run_base(&s, &["relay", "init", "--project", "replay-spool"]);
    assert_eq!(code, 0, "relay init: {err}");
    let (code, _, err) = run_base(
        &s,
        &["relay", "send", "--project", "replay-spool", "--from", "seed-lark", "--to", "seed-kite", "--type", "notify", "--msg", "spool message for the hook"],
    );
    assert_eq!(code, 0, "relay send: {err}");
    // The walk resolves a name the prompt carries: the seed's Project 00.
    let _ = run(&s, "fix the hook budget bytes for Project 00", "pointers-session");
    let file = check(&s, "pointers-session");
    assert!(file.blocks.iter().any(|r| !r.printed), "control: blocks were dropped");

    // A star command on its own session.
    let _ = run(&s, "*audit the hook change", "pointers-star");
    check(&s, "pointers-star");

    for want in [
        "rules", "context", "topic-rules", "always-rules", "walk", "relay-inbox", "relay-tasks", "relay-wake", "grounding",
        "bracket-rules", "devmode", "command",
    ] {
        assert!(covered.contains(want), "no {want} block was covered: {covered:?}");
    }
}

/// D15: a rule counts as shown only if its whole text was printed. Prompt 1 matches the keyword domain, whose block
/// leaves no room for the always-on domain's rules or the bracket rules, so both are dropped. Prompt 2 matches no
/// keyword, so there is room, and both arrive. Prompt 3 sends neither again: once printed, they are shown.
#[test]
fn rule_marked_shown_only_when_emitted_whole() {
    let s = fixture("d15", 50_000, 4, "[relay]\nwake_nudge = false\n");
    // Measure on a throwaway session, then set the budget to what prompt 1 prints with the global rules, the
    // grounding block and the bracket rules dropped: the header, the matched blocks and the pointer lines.
    let _ = run(&s, "fix the hook output", "d15-measure");
    let measured = blocks(&s, "d15-measure");
    let gone = ["global-rules", "grounding", "bracket-rules"];
    for id in gone {
        assert!(measured.blocks.iter().any(|r| r.id == id), "control: the fixture builds {id}");
    }
    set_budget(&s, budget_dropping(&measured, |r| gone.contains(&r.id.as_str())));
    let session = "d15-session";
    let one = run(&s, "fix the hook output", session);
    let file = blocks(&s, session);
    let row = |id: &str| file.blocks.iter().find(|r| r.id == id).cloned();
    let global = row("global-rules").expect("control: prompt 1 built the global rules");
    assert!(!global.printed, "control: prompt 1 dropped the global rules:\n{one}");
    let bracket = row("bracket-rules").expect("control: prompt 1 built the bracket rules");
    assert!(!bracket.printed, "control: prompt 1 dropped the bracket rules:\n{one}");
    assert!(row("hooks-rules").is_some_and(|r| r.printed), "control: prompt 1 printed the matched rules");

    let two = run(&s, "what is next on the list", session);
    assert!(two.contains("Global rule one: never give time estimates"), "the dropped global rules were not sent:\n{two}");
    assert!(two.contains("[BRACKET RULES"), "the dropped bracket block was not sent:\n{two}");
    assert!(!two.contains("Hooks rule: measure hook output"), "control: no keyword, no hooks rules:\n{two}");

    let three = run(&s, "and after that", session);
    assert!(!three.contains("Global rule one"), "printed rules were not recorded as shown:\n{three}");
    assert!(!three.contains("[BRACKET RULES"), "the printed bracket block was not recorded:\n{three}");
}

/// Code review (2026-10-01): relay delivery used to consume as it rendered, so a relay block the budget dropped was
/// already marked delivered and its wake nudge throttled. Now a dropped relay block is not consumed: the ping is still
/// pending on disk, and the next prompt shows it. BO-04: the watcher nudge is one line, shorter than the pointer line
/// that would replace it, so the fit never drops it and it is printed on the first prompt.
#[test]
fn a_dropped_relay_block_is_not_consumed() {
    let s = fixture("relay-kept", 50_000, 6, "");
    ping_sized(&s, "relay-measure", "kite-measure", 800);
    let _ = run(&s, "fix the hook output", "relay-measure");
    let measured = blocks(&s, "relay-measure");
    for id in ["relay-tasks", "relay-wake"] {
        assert!(measured.blocks.iter().any(|r| r.id == id), "control: the fixture builds {id}");
    }
    set_budget(&s, budget_dropping(&measured, |r| r.priority >= 3));

    let session = "relay-session";
    ping_sized(&s, session, "kite-rel", 800);
    let one = run(&s, "fix the hook output", session);
    let file = blocks(&s, session);
    let row = file.blocks.iter().find(|r| r.id == "relay-tasks").expect("control: relay-tasks built");
    assert!(!row.printed, "control: prompt 1 dropped relay-tasks:\n{one}");
    let wake = file.blocks.iter().find(|r| r.id == "relay-wake").expect("control: relay-wake built");
    assert!(wake.printed, "the one-line nudge is never dropped:\n{one}");
    let inbox = s.home.join(".base-gbl").join(".base").join("relay-inbox").join("kite-rel");
    let pings: Vec<String> = std::fs::read_dir(&inbox)
        .unwrap_or_else(|e| panic!("{}: {e}", inbox.display()))
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("ping-"))
        .map(|e| std::fs::read_to_string(e.path()).expect("ping file"))
        .collect();
    assert_eq!(pings.len(), 1, "control: one ping in the inbox");
    let status = serde_json::from_str::<serde_json::Value>(&pings[0]).expect("ping JSON")["status"].clone();
    assert_eq!(status, "pending", "a dropped ping was recorded as delivered:\n{}", pings[0]);

    let two = run(&s, "what is next on the list", session);
    assert!(two.contains("relay: ping from "), "the dropped ping was not shown on the next prompt:\n{two}");
    assert!(
        !blocks(&s, session).blocks.iter().any(|r| r.id == "relay-wake"),
        "the printed nudge was not recorded, so it was said twice in one session"
    );
}

/// Code review (2026-10-01): a domain's steering lines and its CONTEXT were one hash, recorded only when both printed.
/// A context the budget keeps dropping then re-sent the role line on every prompt. Now each part is recorded when its
/// own block prints: the role line goes once, and the dropped context stays due.
#[test]
fn a_dropped_context_stays_due_and_printed_steering_does_not_repeat() {
    let s = fixture("context-due", 50_000, 2, "[relay]\nwake_nudge = false\n");
    let domains = std::fs::read_to_string(s.ws.join(".base").join("domains.toml")).unwrap().replace(
        "name = \"hooks\"\n",
        "name = \"hooks\"\nrole = \"You are reviewing hook output, block by block.\"\n",
    );
    std::fs::write(s.ws.join(".base").join("domains.toml"), domains).unwrap();
    for i in 0..6 {
        let text = format!("Decision {i}: the context block is long enough here that it competes for the budget");
        let (code, _, err) = run_base(&s, &["decision", "log", "--domain", "hooks", "--decision", &text, "--rationale", "test"]);
        assert_eq!(code, 0, "{err}");
    }
    let _ = run(&s, "fix the hook output", "context-measure");
    let measured = blocks(&s, "context-measure");
    assert!(measured.blocks.iter().any(|r| r.id == "hooks-context"), "control: the fixture builds hooks-context");
    set_budget(&s, budget_dropping(&measured, |r| r.priority >= 2));

    let session = "context-session";
    let one = run(&s, "fix the hook output", session);
    let file = blocks(&s, session);
    let rules = file.blocks.iter().find(|r| r.id == "hooks-rules").expect("control: hooks-rules built");
    assert!(rules.printed && rules.text.contains("You are reviewing hook output"), "control: the role printed:\n{one}");
    assert!(file.blocks.iter().any(|r| r.id == "hooks-context" && !r.printed), "control: the context was dropped:\n{one}");

    let two = run(&s, "fix the hook output again", session);
    let file = blocks(&s, session);
    assert!(!two.contains("You are reviewing hook output"), "the printed role line was sent again:\n{two}");
    assert!(file.blocks.iter().any(|r| r.id == "hooks-context"), "the dropped context is no longer due:\n{two}");
}

/// Code review (2026-10-01): a command mode linked by two domains went into the first domain's block only. With the
/// always-on domain first (priority 4) and the keyword domain kept (priority 1), dropping the first lost the mode. It
/// is one block now, at the better of the two priorities, and once per session.
#[test]
fn a_command_linked_by_two_domains_is_one_block_at_the_better_priority() {
    let s = fixture("linked", 50_000, 0, "[relay]\nwake_nudge = false\n");
    let domains = std::fs::read_to_string(s.ws.join(".base").join("domains.toml"))
        .unwrap()
        .replace("mode = \"always\"\n", "mode = \"always\"\ncommands = [\"audit\"]\n")
        .replace("prompt_keywords = [\"hook\", \"hooks\"]\n", "prompt_keywords = [\"hook\", \"hooks\"]\ncommands = [\"audit\"]\n");
    std::fs::write(s.ws.join(".base").join("domains.toml"), domains).unwrap();
    let session = "linked-session";
    let one = run(&s, "fix the hook output", session);
    let file = blocks(&s, session);
    let modes: Vec<&Row> = file.blocks.iter().filter(|r| r.id.starts_with("command-audit")).collect();
    assert_eq!(modes.len(), 1, "one block for the mode:\n{one}");
    assert_eq!(modes[0].priority, 1, "at the keyword domain's priority, not the always-on one's");
    assert!(
        file.blocks.iter().filter(|r| !r.id.starts_with("command-")).all(|r| !r.text.contains("[*AUDIT ACTIVATED]")),
        "the mode is also inside a domain block"
    );
    assert_eq!(one.matches("[*AUDIT ACTIVATED]").count(), 1);
    let two = run(&s, "fix the hook output again", session);
    assert!(!two.contains("[*AUDIT ACTIVATED]"), "a printed mode is sent once per session:\n{two}");
}
