//! BO-17 (K4, C5, D5, D7): `base tune` folds corrections into rule proposals, runs on evidence and on a turn count, and
//! catches up on abandoned sessions.
//!
//! The hooks are driven as Claude Code drives them, on a transcript written in its real line shapes
//! (`tests/transcripts`), so the counts come from the same reader the detector uses. `base tune` gets its judge through
//! `BASE_LLM_FAKE` (a file of canned answers, or `fail`), with `BASE_LLM_FAKE_LOG` recording every call: no test spends a
//! real call, and `no_hook_calls_llm` proves no hook path makes one.

mod seed;
mod transcripts;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use transcripts::Ev;

const DUE_3: &str = "rule pass due: 3 corrections since the last one · run base tune";

fn home(tag: &str) -> seed::Seed {
    let root = std::env::temp_dir().join(format!("base-bo17-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let s = seed::write(&root, &seed::TINY, "");
    std::fs::write(
        s.ws.join(".base").join("domains.toml"),
        "[[domain]]\nname = \"base\"\nmode = \"triggered\"\nprompt_keywords = [\"hooks\", \"doctor\"]\n\
         rules = [\"Read the hook output before saying a block was cut from the prompt.\", \
         \"Read the hook output first, before saying a block was cut from the prompt output.\"]\n\n\
         [[domain]]\nname = \"vintryx\"\nmode = \"triggered\"\nprompt_keywords = [\"dealer\", \"invoice\"]\n",
    )
    .unwrap();
    s
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
        .env_remove("BASE_LLM_FAKE")
        .env_remove("BASE_LLM_FAKE_LOG")
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

fn ok(s: &seed::Seed, args: &[&str], env: &[(&str, &str)]) -> String {
    let (code, out, err) = base_env(s, args, env);
    assert_eq!(code, 0, "{args:?}\nstdout:\n{out}\nstderr:\n{err}");
    out
}

/// A session id Claude Code would send.
fn sid(n: u32) -> String {
    format!("b017b017-0000-4000-8000-{n:012}")
}

fn corrections_dir(s: &seed::Seed) -> PathBuf {
    s.home.join(".base-gbl").join("corrections")
}

/// One session driven through the real hooks, turn by turn.
struct Live<'a> {
    s: &'a seed::Seed,
    id: String,
    transcript: PathBuf,
    env: Vec<(String, String)>,
}

impl<'a> Live<'a> {
    fn new(s: &'a seed::Seed, n: u32) -> Self {
        let id = sid(n);
        let transcript = s.home.join(".claude").join("projects").join("tune").join(format!("{id}.jsonl"));
        Live { s, id, transcript, env: Vec::new() }
    }

    fn env(&self) -> Vec<(&str, &str)> {
        self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
    }

    fn hook(&self, event: &str, extra: serde_json::Value) -> String {
        let mut payload = serde_json::json!({
            "cwd": self.s.ws.display().to_string(),
            "session_id": self.id,
            "transcript_path": self.transcript.display().to_string(),
        });
        if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                p.insert(k.clone(), v.clone());
            }
        }
        let (code, out, err) = seed::run_hook_at(self.s, &self.s.ws, event, &payload, &self.env());
        assert_eq!(code, 0, "{event}: {err}");
        out
    }

    fn start(&self) -> String {
        self.hook("session-start", serde_json::json!({ "hook_event_name": "SessionStart", "source": "startup" }))
    }

    /// A typed prompt: written to the transcript, then the prompt hook. Its output.
    fn prompt(&self, text: &str) -> String {
        transcripts::append(&self.transcript, &self.id, &[Ev::Prompt(text.to_string())]);
        self.hook("user-prompt-submit", serde_json::json!({ "hook_event_name": "UserPromptSubmit", "prompt": text }))
    }

    /// The AI's reply, then the Stop hook.
    fn reply(&self, text: &str) {
        transcripts::append(&self.transcript, &self.id, &[Ev::Text(text.to_string())]);
        self.hook("stop", serde_json::json!({ "hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": text }));
    }

    /// A whole turn; the prompt hook's output.
    fn turn(&self, prompt: &str, reply: &str) -> String {
        let out = self.prompt(prompt);
        self.reply(reply);
        out
    }

    fn end(&self) {
        self.hook("session-end", serde_json::json!({ "hook_event_name": "SessionEnd", "reason": "prompt_input_exit" }));
    }
}

/// Typed prompts no C1 phrase flags and no two of which repeat each other, about the `base` domain (its keyword
/// "hooks") when `on_topic`.
fn plain(n: usize, on_topic: bool) -> String {
    let topics = [
        "list the sessions that touched the release branch",
        "summarize what changed in the parser module",
        "write the changelog entry for the cache fix",
        "check which tests cover the session file writer",
        "draft the note about the config defaults",
        "show the largest files in the build folder",
        "explain the retry loop in the downloader",
        "count the open items on the board",
        "rename the helper that formats dates",
        "find where the timeout value is read",
        "sketch the plan for the importer",
        "compare the two output formats",
        "trace the call that writes the log",
        "outline the steps for the migration",
        "point me to the place that parses flags",
        "tidy the imports in the server file",
        "measure how long the startup takes",
        "look at the error the updater printed",
        "group the warnings by module",
        "describe the shape of the config file",
    ];
    let t = topics[n % topics.len()];
    if on_topic { format!("{t} and its hooks") } else { t.to_string() }
}

// ─── Example 1: the evidence trigger ─────────────────────────────────────────

/// Prompts 4, 7 and 9 draw a correction (C1, or C3 in the reply). After prompt 9's turn the count is 3, so prompt 10
/// carries the line, and prompt 11 does not carry it again.
#[test]
fn evidence_trigger_at_threshold() {
    let s = home("evidence");
    let l = Live::new(&s, 1);
    l.start();
    for n in 1..=11u32 {
        let (prompt, reply) = match n {
            4 => ("that is wrong, the totals come from the summary sheet".to_string(), "Reading the summary sheet.".to_string()),
            7 => (plain(7, true), "UPDATED: the cache lives under .base, not the temp folder.".to_string()),
            9 => ("quit renaming the files I did not ask about".to_string(), "Leaving the other files as they are.".to_string()),
            _ => (plain(n as usize, true), format!("Done with step {n}.")),
        };
        let out = l.turn(&prompt, &reply);
        let has = out.contains(DUE_3);
        assert_eq!(has, n == 10, "prompt {n}: the due line {}:\n{out}", if has { "printed" } else { "missing" });
    }
}

// ─── Example 2: the safety net ───────────────────────────────────────────────

/// A 20-prompt session with no flagged correction, where prompts 3 and 12 matched no domain: the line fires at prompt
/// 16 (15 since the last pass), and only then. The same session with every prompt matching a domain never gets it.
#[test]
fn turn_safety_net() {
    let s = home("safety");
    let l = Live::new(&s, 2);
    l.start();
    let mut lines: Vec<u32> = Vec::new();
    for n in 1..=20u32 {
        let out = l.turn(&plain(n as usize, n != 3 && n != 12), &format!("Done with step {n}."));
        assert!(!out.contains("This may be a correction"), "prompt {n} was flagged; the safety net test needs none:\n{out}");
        if out.contains("rule pass due: 15 prompts since the last one · run base tune") {
            lines.push(n);
        }
    }
    assert_eq!(lines, [16], "the safety net's line");

    let quiet = home("safety-quiet");
    let l = Live::new(&quiet, 3);
    l.start();
    for n in 1..=20u32 {
        let out = l.turn(&plain(n as usize, true), &format!("Done with step {n}."));
        assert!(!out.contains("rule pass due"), "prompt {n}: no evidence, no line:\n{out}");
    }
}

// ─── Example 4: the thresholds from base.toml ────────────────────────────────

#[test]
fn tune_thresholds_from_config() {
    // Example 4's values are the defaults: the template documents them.
    let defaults = base::config::TuneConfig::default();
    assert_eq!((defaults.corrections, defaults.turns), (3, 15));

    let s = home("thresholds");
    std::fs::write(s.ws.join(".base").join("base.toml"), "[tune]\ncorrections = 2\nturns = 4\n").unwrap();
    let l = Live::new(&s, 4);
    l.start();
    let mut seen: Vec<(u32, String)> = Vec::new();
    for n in 1..=4u32 {
        let prompt = if n == 2 || n == 3 { format!("that is wrong, {}", plain(n as usize, true)) } else { plain(n as usize, true) };
        let out = l.turn(&prompt, "Done.");
        if let Some(line) = out.lines().find(|x| x.starts_with("rule pass due")) {
            seen.push((n, line.to_string()));
        }
    }
    assert_eq!(seen, [(4, "rule pass due: 2 corrections since the last one · run base tune".to_string())]);

    let t = home("thresholds-turns");
    std::fs::write(t.ws.join(".base").join("base.toml"), "[tune]\ncorrections = 2\nturns = 4\n").unwrap();
    let l = Live::new(&t, 5);
    l.start();
    let lines: Vec<u32> = (1..=6u32)
        .filter(|n| l.turn(&plain(*n as usize, *n != 2), "Done.").contains("rule pass due: 4 prompts since the last one"))
        .collect();
    assert_eq!(lines, [5], "the safety net after 4 typed prompts");

    let off = home("thresholds-off");
    std::fs::write(off.ws.join(".base").join("base.toml"), "[tune]\ncorrections = 0\nturns = 0\n").unwrap();
    let l = Live::new(&off, 6);
    l.start();
    for n in 1..=8u32 {
        let out = l.turn(&format!("that is wrong, {}", plain(n as usize, false)), "Done.");
        assert!(!out.contains("rule pass due"), "both triggers off, prompt {n}:\n{out}");
    }
}

// ─── D7d: SessionEnd ─────────────────────────────────────────────────────────

/// SessionEnd only marks the session: the marker is written, and no graph, no match log and no LLM is touched.
#[test]
fn session_end_marks_only() {
    let s = home("session-end");
    let l = Live::new(&s, 7);
    l.start();
    l.turn(&plain(1, true), "Done.");
    let graph = |p: &Path| std::fs::read(p).unwrap_or_default();
    let (ws_graph, gbl_graph) = (s.ws.join(".base").join("graph.nq"), s.home.join(".base-gbl").join(".base").join("graph.nq"));
    let log = s.ws.join(".base").join("match-log.jsonl");
    let before = (graph(&ws_graph), graph(&gbl_graph), graph(&log));
    let calls = s.home.join("llm-calls.jsonl");
    let mut l = l;
    l.env = vec![("BASE_LLM_FAKE".into(), "fail".into()), ("BASE_LLM_FAKE_LOG".into(), calls.display().to_string())];
    let t = Instant::now();
    l.end();
    let took = t.elapsed();
    let marker = corrections_dir(&s).join(format!("{}.ended", l.id));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&marker).expect("the marker")).unwrap();
    assert_eq!(v["reason"], "prompt_input_exit");
    assert_eq!(v["transcript"], l.transcript.display().to_string());
    assert!(v["ts"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!((graph(&ws_graph), graph(&gbl_graph), graph(&log)), before, "no graph and no match-log write");
    assert!(!calls.exists(), "no LLM call");
    assert!(took < Duration::from_secs(5), "fast: {took:?}");
}

// ─── Example 3: catch-up at session start ────────────────────────────────────

/// Set a file's modification time `days` days back.
fn age(path: &Path, days: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).expect("the file");
    f.set_modified(std::time::SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60)).expect("its time");
}

/// A closed terminal fires no SessionEnd: a session with 2 flagged corrections, untouched for a day, is named at the
/// next session start; one that ended counts at once; a pass that read them clears the line.
#[test]
fn catch_up_line_at_session_start() {
    let s = home("catch-up");
    let a = Live::new(&s, 8);
    a.start();
    a.turn(&plain(1, true), "Done.");
    a.turn("that is wrong, the totals come from the summary sheet", "Reading the summary sheet.");
    a.turn("quit renaming the files I did not ask about", "Leaving them.");
    age(&corrections_dir(&s).join(format!("{}.json", a.id)), 2);

    let b = Live::new(&s, 9);
    let out = b.start();
    assert!(out.contains("rule pass due: 1 earlier session has unreviewed corrections · run base tune"), "{out}");

    // An ended session counts the same day; one with no correction does not count at all.
    let c = Live::new(&s, 10);
    c.start();
    c.turn(&plain(2, true), "Done.");
    c.turn("that is wrong, use the second table", "Using the second table.");
    c.end();
    let d = Live::new(&s, 11);
    d.start();
    d.turn(&plain(3, true), "Done.");
    d.end();
    let out = Live::new(&s, 12).start();
    assert!(out.contains("rule pass due: 2 earlier sessions have unreviewed corrections · run base tune"), "{out}");

    // A pass reads them (its judge finds nothing): the line is gone.
    let answers = s.home.join("answers.json");
    std::fs::write(&answers, r#"{"answers":[{"answer":"{\"corrections\":[],\"not_corrections\":[],\"unmatched\":[]}"}]}"#).unwrap();
    let out = ok(&s, &["tune"], &[("BASE_LLM_FAKE", &answers.display().to_string())]);
    assert!(out.starts_with("read: 3 sessions"), "{out}");
    let out = Live::new(&s, 13).start();
    assert!(!out.contains("rule pass due"), "{out}");
}

// ─── K4c: no hook calls the LLM ──────────────────────────────────────────────

/// Every hook event, on a home where a pass is due and catch-up has a session to name, with the judge set to fail on
/// use: the call log is never written, and the due and catch-up lines print (the paths ran). Control: `base tune` on
/// the same setting writes the log, so the seam records what reaches it.
#[test]
fn no_hook_calls_llm() {
    let s = home("no-llm");
    let calls = s.home.join("llm-calls.jsonl");
    let fail = vec![("BASE_LLM_FAKE".to_string(), "fail".to_string()), ("BASE_LLM_FAKE_LOG".to_string(), calls.display().to_string())];
    let mut a = Live::new(&s, 14);
    a.env = fail.clone();
    a.start();
    // A session's first typed prompt is never C1's (nothing to correct yet), so prompts 2 to 4 are the three.
    a.turn(&plain(1, true), "Done.");
    a.turn("that is wrong, the totals come from the summary sheet", "Reading.");
    a.turn("quit renaming the files", "Leaving them.");
    a.turn("that is wrong, the dates are off by one", "Fixing.");
    let out = a.turn(&plain(5, true), "Done.");
    assert!(out.contains(DUE_3), "control: the due line printed:\n{out}");
    a.hook("pre-tool-use", serde_json::json!({ "hook_event_name": "PreToolUse", "tool_name": "Read", "tool_input": { "file_path": s.ws.join("x.md").display().to_string() } }));
    a.hook("post-tool-use", serde_json::json!({ "hook_event_name": "PostToolUse", "tool_name": "Read", "tool_input": { "file_path": s.ws.join("x.md").display().to_string() }, "tool_response": {} }));
    a.end();
    let mut b = Live::new(&s, 15);
    b.env = fail.clone();
    let out = b.start();
    assert!(out.contains("rule pass due: 1 earlier session has unreviewed corrections"), "control: the catch-up line:\n{out}");
    b.turn(&plain(1, true), "Done.");
    b.end();
    assert!(!calls.exists(), "a hook reached the LLM:\n{}", std::fs::read_to_string(&calls).unwrap_or_default());

    let env: Vec<(&str, &str)> = fail.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let (_, out, _) = base_env(&s, &["tune"], &env);
    assert!(calls.exists(), "control: base tune's call reached the seam:\n{out}");
    assert!(out.contains("the judge's call failed"), "{out}");
}

// ─── The pass: proposals with their evidence ─────────────────────────────────

const R1: &str = "Read the hook output before saying a block was cut from the prompt.";
const R2: &str = "Read the hook output first, before saying a block was cut from the prompt output.";

/// Prompt rows that served R1 and R2 together, in the match log, as the prompt hook writes them.
fn served_together(s: &seed::Seed, n: usize) {
    let ids = [base::domain::rules::rule_id("base", R1), base::domain::rules::rule_id("base", R2)];
    let mut text = String::new();
    for i in 0..n {
        let row = serde_json::json!({
            "ts": format!("2026-10-02T09:{:02}:00-05:00", i),
            "session": "b017b017-0000-4000-8000-999999999999",
            "event": "prompt", "prompt_num": i + 1, "text": format!("how do the hooks behave in case {i}"),
            "matched": [], "cut": [], "scores": [],
            "served": ids.iter().map(|id| serde_json::json!({ "id": id, "kind": "rule", "domain": "base", "block": "domain-base" })).collect::<Vec<_>>(),
        });
        text.push_str(&row.to_string());
        text.push('\n');
    }
    std::fs::write(s.ws.join(".base").join("match-log.jsonl"), text).unwrap();
}

/// A fake judge's answers: `when` a substring of the prompt, the JSON it answers with.
fn answers(s: &seed::Seed, list: &[(&str, serde_json::Value)]) -> String {
    let path = s.home.join("answers.json");
    let answers: Vec<serde_json::Value> = list.iter().map(|(w, a)| serde_json::json!({ "when": w, "answer": a.to_string() })).collect();
    std::fs::write(&path, serde_json::json!({ "answers": answers }).to_string()).unwrap();
    path.display().to_string()
}

/// Example 1's session, read with `--transcript`: two prompts about the prompt hook that matched no domain, a
/// correction marked UPDATED, and R1 and R2 served together six times.
fn example_one(tag: &str) -> (seed::Seed, String, String) {
    let s = home(tag);
    served_together(&s, 6);
    let id = sid(20);
    let tp = s.home.join(".claude").join("projects").join("tune").join(format!("{id}.jsonl"));
    transcripts::write(&tp, &id, &[
        Ev::Prompt("set up the hooks for the release build".into()),
        Ev::Text("Set up.".into()),
        Ev::Prompt("the user prompt submit output is cut off at a high rate".into()),
        Ev::Text("Looking.".into()),
        Ev::Prompt("quit reading the version string as the installed release, it is the dev tree build".into()),
        Ev::Text("UPDATED: the build is the dev tree's; the version string is Cargo.toml's.".into()),
        Ev::Prompt("is the user prompt submit cut fixed now".into()),
        Ev::Text("Yes.".into()),
    ]);
    let fake = answers(&s, &[
        ("TURNS:", serde_json::json!({
            "corrections": [{ "turn": 3, "why": "it read the version string as the installed release",
                              "rule": "base --version prints the dev tree's Cargo.toml version; never read it as the installed release.",
                              "keywords": ["version string", "dev tree", "installed release"], "domain": "base" }],
            "not_corrections": [],
            "unmatched": [{ "turn": 2, "domain": "base", "keywords": ["user prompt submit"] },
                          { "turn": 4, "domain": "base", "keywords": ["user prompt submit"] }],
        })),
        ("MERGE:", serde_json::json!({ "merge": [{ "id": "m1", "same": true, "text": "Read the hook output before saying a block was cut." }] })),
    ]);
    (s, tp.display().to_string(), fake)
}

/// Example 1: a keyword gap from two prompts, a new rule from a correction, a merge from two rules served together,
/// each with its evidence; a rerun costs no call and writes nothing again.
#[test]
fn tune_writes_proposals_with_evidence() {
    let (s, tp, fake) = example_one("proposals");
    let env = [("BASE_LLM_FAKE", fake.as_str())];
    let dry = ok(&s, &["tune", "--transcript", &tp, "--store", "--dry-run"], &env);
    assert!(dry.contains("would read: 1 session (b017b017), 4 prompts, 1 flagged correction, 3 with no domain match\n"), "{dry}");
    assert!(dry.contains("haiku: would make 2 calls (0 cached)\n"), "{dry}");
    assert!(dry.ends_with("dry run: no call made, nothing written\n"), "{dry}");
    assert!(ok(&s, &["rule", "review"], &[]).starts_with("no rule proposals pending"), "a dry run writes nothing");

    let out = ok(&s, &["tune", "--transcript", &tp, "--store"], &env);
    assert!(out.starts_with("read: 1 session (b017b017), 4 prompts, 1 flagged correction, 0 unflagged found by the backstop\n"), "{out}");
    assert!(out.contains("haiku: 2 calls (cached: 0)\n"), "{out}");
    assert!(out.contains("proposals written: 3 (see base rule review)\n"), "{out}");
    let line = |id: &str| out.lines().find(|l| l.trim_start().starts_with(id)).unwrap_or_else(|| panic!("no {id} in:\n{out}")).to_string();
    let (p1, p2, p3) = (line("p-0001"), line("p-0002"), line("p-0003"));
    assert!(p1.contains("new rule") && p1.contains("· base ·") && p1.contains("evidence: 1 correction (UPDATED)"), "{p1}");
    assert!(p2.contains("keyword gap") && p2.contains("add \"user prompt submit\"") && p2.contains("evidence: 2 prompts"), "{p2}");
    assert!(p3.contains("merge") && p3.contains("say the same thing") && p3.contains("evidence: served together 6 times"), "{p3}");
    assert!(out.contains("detector: 1 of 1 judged corrections flagged (by layer: C1 1, C3 1), 0 missed"), "{out}");

    let review = ok(&s, &["rule", "review"], &[]);
    assert!(review.contains("[1/3] p-0001 new rule · base"), "{review}");
    assert!(review.contains("evidence: judged a correction: it read the version string as the installed release"), "{review}");
    assert!(review.contains("p-0002 keyword gap · base · add \"user prompt submit\"") && review.contains("from the rule pass"), "{review}");
    assert!(review.contains("evidence: 2 prompts matched no domain; the judge put them in base"), "{review}");
    assert!(review.contains("p-0003 merge · rule base.") && review.contains("evidence: served together on 6 of the last"), "{review}");
    let graph = std::fs::read_to_string(s.ws.join(".base").join("graph.nq")).unwrap();
    assert_eq!(graph.matches("proposalOrigin> \"tune\"").count(), 3, "each stored as the rule pass's");
    assert!(graph.contains("mergeTarget"), "the merge's second rule is stored");

    // The same pass again: its marks say the session was read, its store answer is cached, and nothing is written twice.
    let again = ok(&s, &["tune", "--transcript", &tp, "--store"], &env);
    assert!(again.starts_with("read: 0 sessions, 0 prompts"), "{again}");
    assert!(again.contains("haiku: 0 calls (cached: 1)\n") && again.contains("proposals written: 0\n"), "{again}");
    assert!(again.contains("is already p-0003 (pending); not written again"), "{again}");
}

/// What base serves, byte for byte: both tiers' config files and every rule the graphs hold.
fn served_state(s: &seed::Seed) -> Vec<String> {
    let read = |p: PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    let rules = |p: PathBuf| {
        let mut l: Vec<String> = read(p)
            .lines()
            .filter(|l| ["ruleText", "hasRule", "match", "firesOn", "quietOn", "supersede", "retiredAt", "prompt_keywords"].iter().any(|k| l.contains(k)))
            .filter(|l| !l.contains("proposal/p-"))
            .map(String::from)
            .collect();
        l.sort();
        l.join("\n")
    };
    let gbl = s.home.join(".base-gbl");
    vec![
        read(s.ws.join(".base").join("domains.toml")),
        read(gbl.join("domains.toml")),
        read(s.ws.join(".base").join("base.toml")),
        read(gbl.join("base.toml")),
        rules(s.ws.join(".base").join("graph.nq")),
        rules(gbl.join(".base").join("graph.nq")),
    ]
}

/// K4a: nothing is applied. The config files and every rule in both graphs are the same after a pass that wrote three
/// proposals.
#[test]
fn tune_never_applies_changes() {
    let (s, tp, fake) = example_one("never-applies");
    ok(&s, &["domain", "sync"], &[]);
    let before = served_state(&s);
    let out = ok(&s, &["tune", "--transcript", &tp, "--store"], &[("BASE_LLM_FAKE", fake.as_str())]);
    assert!(out.contains("proposals written: 3"), "control: the pass wrote proposals:\n{out}");
    assert_eq!(served_state(&s), before, "what base serves is unchanged");
}

/// A session whose only typed prompt is its first (a spawned session's boot prompt; the rest are task notifications)
/// and matched a domain holds nothing a judge could find: no reply came before it, so it is no correction, and it is no
/// keyword gap. The pass reads it and makes no call (gate 4: 9 of the 19 sessions of 2026-10-02). The control: a second
/// typed prompt in the same session is judged, one call.
#[test]
fn a_session_with_nothing_to_judge_costs_no_call() {
    let s = home("nothing-to-judge");
    let l = Live::new(&s, 7);
    l.start();
    l.turn(&plain(0, true), "Done.");
    let tp = l.transcript.display().to_string();
    let log = s.ws.join("calls.jsonl");
    let env = [("BASE_LLM_FAKE", "fail"), ("BASE_LLM_FAKE_LOG", log.to_str().unwrap())];
    let out = ok(&s, &["tune", "--transcript", &tp], &env);
    assert!(out.starts_with("read: 1 session (b017b017), 1 prompt, 0 flagged corrections"), "{out}");
    assert!(out.contains("haiku: 0 calls (cached: 0)\n") && !out.contains("note:"), "{out}");
    assert!(!log.exists(), "no call was made");

    l.turn(&plain(1, true), "Done again.");
    let out = ok(&s, &["tune", "--transcript", &tp, "--dry-run"], &[]);
    assert!(out.starts_with("would read: 1 session (b017b017), 1 prompt, "), "only the new turn:\n{out}");
    assert!(out.contains("haiku: would make 1 call (0 cached)\n"), "the second prompt is judged:\n{out}");
}

// ─── C5: the backstop ────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct Case {
    name: String,
    events: Vec<Ev>,
    expect: std::collections::BTreeMap<String, Vec<String>>,
    checks: Vec<u32>,
    missed: Missed,
}

#[derive(Debug, serde::Deserialize)]
struct Missed {
    turn: u32,
    why: String,
}

/// A correction with no phrase, no marker and no C2 signal: the judge finds it, the pass saves it as a detector case in
/// BO-15's schema, and that case holds when BO-15's own check runs it.
#[test]
fn backstop_saves_missed_correction_as_test_case() {
    let s = home("backstop");
    let id = sid(21);
    let tp = s.home.join(".claude").join("projects").join("tune").join(format!("{id}.jsonl"));
    transcripts::write(&tp, &id, &[
        Ev::Prompt("make the chart with the blue palette from the brief".into()),
        Ev::Text("Here is the chart, in red.".into()),
        Ev::Prompt("the charts should be blue as the brief says".into()),
        Ev::Text("Switching every chart to blue.".into()),
    ]);
    let fake = answers(&s, &[("TURNS:", serde_json::json!({
        "corrections": [{ "turn": 2, "why": "it used red where the brief says blue", "rule": "Use the brief's palette for every chart.",
                          "keywords": ["charts", "brief"], "domain": "base" }],
        "not_corrections": [], "unmatched": [],
    }))]);
    let out = ok(&s, &["tune", "--transcript", &tp.display().to_string()], &[("BASE_LLM_FAKE", fake.as_str())]);
    assert!(out.contains("1 unflagged found by the backstop"), "{out}");
    assert!(out.contains("detector: 1 missed correction saved as a test case"), "{out}");
    assert!(ok(&s, &["rule", "review"], &[]).contains("evidence: base's detector did not flag it: the backstop found it"));

    let path = s.home.join(".base-gbl").join("corrections").join("tune").join("missed-cases.json");
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("the saved cases")).unwrap();
    let cases: Vec<Case> = serde_json::from_value(doc["cases"].clone()).expect("cases in BO-15's schema");
    assert_eq!(cases.len(), 1);
    let c = &cases[0];
    assert_eq!(c.missed.turn, 2);
    assert_eq!(c.missed.why, "it used red where the brief says blue");
    // BO-15's check (`corrections_test::check_case`): written as a transcript, read back, run through the detector.
    let dir = tempfile::tempdir().unwrap();
    let t = dir.path().join("case.jsonl");
    transcripts::write(&t, "case-session", &c.events);
    let turns = base::corrections::turns(base::domain::transcript::read_all(&t).unwrap(), &base::config::CorrectionsConfig::default());
    assert_eq!(turns.len(), 2, "[{}] the turn before and the missed one", c.name);
    for turn in &turns {
        let mut got: Vec<String> = turn.signals.iter().map(|s| format!("{} {}", s.layer, s.kind)).collect();
        got.sort();
        assert_eq!(got, c.expect.get(&turn.num.to_string()).cloned().unwrap_or_default(), "[{}] turn {}", c.name, turn.num);
    }
    let checks: Vec<u32> = turns.iter().filter(|t| t.check).map(|t| t.num).collect();
    assert_eq!(checks, c.checks, "[{}] the check lines", c.name);
    assert!(!c.checks.contains(&c.missed.turn), "a miss is a turn the detector does not flag");
}

// ─── Review: the rule pass's kinds ───────────────────────────────────────────

const R3: &str = "When a release is cut, tag it from main after the CI run on that exact commit is green on both platforms. \
                  Then write the release notes from the merged pull requests, one line each, grouped by the area of the code \
                  they change, and send them to the channel.";
const R4: &str = "Use the dashboard's colour scale for every chart of hook timings.";

/// The `<domain>.<id>` of a rule's wording.
fn short(text: &str) -> String {
    let id = base::domain::rules::rule_id("base", text);
    format!("base.{}", &id[..8])
}

/// Approving each of the rule pass's kinds does what it says: a merge supersedes both rules, a split rewrites one and
/// adds the other part, a drop takes the keyword away and adds the narrower one, and a retirement stops serving the
/// rule without deleting it, until `rule unretire`.
#[test]
fn review_applies_merge_split_retire_drop() {
    let s = home("kinds");
    std::fs::write(
        s.ws.join(".base").join("domains.toml"),
        format!(
            "[[domain]]\nname = \"base\"\nmode = \"triggered\"\nprompt_keywords = [\"hooks\", \"doctor\"]\n\
             rules = [{:?}, {:?}, {:?}, {:?}]\n",
            R1, R2, R3, R4
        ),
    )
    .unwrap();
    // A match log 40 days long: an archive file's first row, then six prompts that served R1, R2 and R3 and matched
    // `base` on "hooks", four of them about other hooks.
    let archive = s.ws.join(".base").join("match-log");
    std::fs::create_dir_all(&archive).unwrap();
    let old = (chrono::Local::now() - chrono::Duration::days(40)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    std::fs::write(
        archive.join("2026-08-24T00-00-00.jsonl"),
        format!("{}\n", serde_json::json!({ "ts": old, "event": "prompt", "text": "hello", "matched": [], "served": [], "cut": [], "scores": [] })),
    )
    .unwrap();
    let texts = [
        "the coat hooks in the hall are loose",
        "buy fishing hooks for saturday",
        "the base hooks are slow on the first prompt",
        "hang the picture on two hooks",
        "the curtain hooks broke again",
        "why do the base hooks print twice",
    ];
    let ids: Vec<String> = [R1, R2, R3].iter().map(|t| base::domain::rules::rule_id("base", t)).collect();
    let mut log = String::new();
    for (i, t) in texts.iter().enumerate() {
        log.push_str(
            &serde_json::json!({
                "ts": format!("2026-10-02T10:{:02}:00-05:00", i), "session": "b017b017-0000-4000-8000-888888888888",
                "event": "prompt", "prompt_num": i + 1, "text": t,
                "matched": [{ "domain": "base", "by": "keyword", "value": "hooks" }], "cut": [], "scores": [],
                "served": ids.iter().map(|id| serde_json::json!({ "id": id, "kind": "rule", "domain": "base" })).collect::<Vec<_>>(),
            })
            .to_string(),
        );
        log.push('\n');
    }
    std::fs::write(s.ws.join(".base").join("match-log.jsonl"), log).unwrap();
    let fake = answers(&s, &[("MERGE:", serde_json::json!({
        "merge": [{ "id": "m1", "same": true, "text": "Read the hook output before saying a block was cut." }],
        "split": [{ "id": "s1", "split": true, "parts": [
            { "text": "Tag a release from main once CI is green on both platforms for that commit.", "keywords": ["release tag"] },
            { "text": "Write release notes from the merged pull requests, grouped by area.", "keywords": ["release notes"] } ] }],
        // The judge reads the prompts newest first: "why do the base hooks print twice" and "the base hooks are slow on
        // the first prompt" are the two about base.
        "keywords": [{ "id": "k1", "about": [true, false, false, true, false, false], "instead": ["base hooks"] }],
    }))]);
    let out = ok(&s, &["tune", "--store"], &[("BASE_LLM_FAKE", fake.as_str())]);
    assert!(out.contains("store check: ran · 1 merge, 1 split, 1 keyword, 1 retire candidates"), "{out}");
    assert!(out.contains("proposals written: 4"), "{out}");
    let id_of = |kind: &str| {
        out.lines()
            .filter(|l| l.trim_start().starts_with("p-"))
            .find(|l| l.contains(&format!(" {kind} ")))
            .and_then(|l| l.trim_start().split(' ').next())
            .unwrap_or_else(|| panic!("no {kind} in:\n{out}"))
            .to_string()
    };
    let (merge, split, drop, retire) = (id_of("merge"), id_of("split"), id_of("drop keyword"), id_of("retire"));

    for id in [&merge, &split, &drop, &retire] {
        let done = ok(&s, &["rule", "review", "--approve", id, "--broad-ok"], &[]);
        assert!(done.starts_with(&format!("approved {id}: ")), "{done}");
    }
    let list = ok(&s, &["rule", "list", "--domain", "base"], &[]);
    assert!(list.contains("Read the hook output before saying a block was cut."), "the merged rule:\n{list}");
    assert!(!list.contains(R1) && !list.contains(R2), "both merged rules superseded:\n{list}");
    assert!(list.contains("Tag a release from main") && list.contains("Write release notes from the merged"), "both parts:\n{list}");
    assert!(!list.contains("When a release is cut"), "the split rule superseded:\n{list}");
    assert!(!list.contains(R4), "the retired rule is not listed as live:\n{list}");
    let all = ok(&s, &["rule", "list", "--domain", "base", "--include-superseded"], &[]);
    assert!(all.lines().any(|l| l.contains(R1) && l.contains("[superseded]")), "{all}");
    assert!(all.lines().any(|l| l.contains(R4) && l.contains("[retired")), "kept, and marked:\n{all}");
    let toml = std::fs::read_to_string(s.ws.join(".base").join("domains.toml")).unwrap();
    assert!(toml.contains("\"base hooks\"") && !toml.contains("\"hooks\""), "dropped and narrowed:\n{toml}");

    // No hook serves a retired rule; unretire brings it back.
    let l = Live::new(&s, 22);
    l.start();
    let out = l.prompt("run doctor on the base hooks");
    assert!(out.contains("Read the hook output before saying a block was cut."), "control: the domain was served:\n{out}");
    assert!(!out.contains(R4), "a retired rule is not served:\n{out}");
    let back = ok(&s, &["rule", "unretire", &short(R4)], &[]);
    assert!(back.contains("is served again"), "{back}");
    let out = Live::new(&s, 23).prompt("run doctor on the base hooks");
    assert!(out.contains(R4), "served again:\n{out}");
}
