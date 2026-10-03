//! BO-13 (K1, D2, D14): every prompt and every file touch is logged, so rules and keywords can be judged against
//! history. Each test drives the binary the way Claude Code drives it (JSON on stdin, a temporary home that is also
//! the workspace root) and reads `.base/match-log.jsonl` back.
//!
//! The brief's examples, with a fake home in place of the operator's: Example 1 is a prompt about base's hooks that
//! matched only the always-on GLOBAL domain (the miss the log exists to record); Example 2 is a file in the child
//! project `vintryx-dealer-registry`, nested in `vintryx`; Example 3 is the secret scrub; Example 4 the reader.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_base");

/// Example 1's prompt (Chris's prompt 3 of session 5b860473, shortened as the brief shortens it).
const EXAMPLE_ONE: &str = "I want to make sure that we are working with a version of the base that's actually good. \
And I believe that we're having some issues with the user prompt submit being cut off";

struct Home {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Home {
    /// A home that is also the workspace root, `global_toml` as its global config and `domains` as the workspace's
    /// domains.toml.
    fn new(global_toml: &str, domains: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("home");
        std::fs::create_dir_all(root.join(".base-gbl").join(".base")).unwrap();
        std::fs::create_dir_all(root.join(".base")).unwrap();
        std::fs::write(root.join(".base-gbl").join("base.toml"), global_toml).unwrap();
        std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
        std::fs::write(root.join(".base").join("domains.toml"), domains).unwrap();
        Home { _tmp: tmp, root }
    }

    fn log_path(&self) -> PathBuf {
        self.root.join(".base").join("match-log.jsonl")
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
        let mut child = Command::new(BIN)
            .args(args)
            .current_dir(&self.root)
            .env("BASE_HOME", &self.root)
            .env("BASE_NO_AUTO_UPDATE", "1")
            .env("BASE_AST_NO_SPAWN", "1")
            .env("BASE_NO_WAKE_NUDGE", "1")
            .env("BASE_NO_AUTONAME", "1")
            .env_remove("BASE_RELAY_AS")
            .env_remove("BASE_HEADLESS")
            .env_remove("WT_SESSION")
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

    fn base(&self, args: &[&str]) -> String {
        let (code, out, err) = self.run(args, None);
        assert_eq!(code, 0, "base {args:?}: {err}\n{out}");
        out
    }

    fn prompt(&self, text: &str, session: &str) -> (i32, String, String) {
        let payload = serde_json::json!({
            "cwd": self.root.display().to_string(),
            "hook_event_name": "UserPromptSubmit",
            "prompt": text,
            "session_id": session,
        });
        self.run(&["hook", "user-prompt-submit"], Some(&payload.to_string()))
    }

    fn tool(&self, tool: &str, input: serde_json::Value, session: &str) -> (i32, String, String) {
        let payload = serde_json::json!({
            "cwd": self.root.display().to_string(),
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "tool_input": input,
            "session_id": session,
        });
        self.run(&["hook", "pre-tool-use"], Some(&payload.to_string()))
    }

    fn session_start(&self, session: &str) -> (i32, String, String) {
        let payload = serde_json::json!({
            "cwd": self.root.display().to_string(),
            "hook_event_name": "SessionStart",
            "source": "startup",
            "session_id": session,
        });
        self.run(&["hook", "session-start"], Some(&payload.to_string()))
    }

    fn rows(&self) -> Vec<serde_json::Value> {
        let text = std::fs::read_to_string(self.log_path()).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not a row: {e}: {l}"))).collect()
    }

    fn only_row(&self) -> serde_json::Value {
        let rows = self.rows();
        assert_eq!(rows.len(), 1, "one row: {rows:#?}");
        rows.into_iter().next().unwrap()
    }
}

/// Every prompt past FRESH's lean mode, so a domain's CONTEXT is built on a session's first prompt.
const PAST_LEAN: &str = "[bracket]\nenabled = true\nfresh_until = 0\nmoderate_until = 50\n";

const GLOBAL_DOMAIN: &str =
    "[[domain]]\nname = \"GLOBAL\"\nmode = \"always\"\nrules = [\"the global rule\", \"a second global rule\"]\n";

fn ids(list: &serde_json::Value) -> Vec<String> {
    list.as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect()
}

fn rule_id(domain: &str, text: &str) -> String {
    base::domain::rules::rule_id(domain, text)
}

/// Example 1: a prompt about base's hooks matched only GLOBAL. The row says so, with what was served by id, the
/// block that carried each, and no cut. A global decision named by keyword is served as a decision, by its slug.
#[test]
fn match_log_row_for_prompt() {
    let h = Home::new(PAST_LEAN, GLOBAL_DOMAIN);
    let out = h.base(&[
        "decision",
        "log",
        "--domain",
        "GLOBAL",
        "--decision",
        "Prompt submit output is fitted block by block",
        "--rationale",
        "fixture",
    ]);
    let slug = out.split("slug: ").nth(1).and_then(|s| s.split(')').next()).expect(&out).to_string();
    h.base(&["decision", "update", &slug, "--keywords", "prompt submit"]);

    let (code, stdout, stderr) = h.prompt(EXAMPLE_ONE, "5b860473-5522-5a45-85ed-de1df8fca274");
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("the global rule"), "control: GLOBAL's rules were printed:\n{stdout}");
    let row = h.only_row();

    let ts = row["ts"].as_str().expect("ts");
    assert!(chrono::DateTime::parse_from_rfc3339(ts).is_ok(), "ts is RFC 3339: {ts}");
    assert_eq!(row["session"], "5b860473-5522-5a45-85ed-de1df8fca274");
    assert_eq!(row["event"], "prompt");
    assert_eq!(row["prompt_num"], 1);
    assert_eq!(row["text"], EXAMPLE_ONE, "full text by default");
    assert_eq!(row["matched"], serde_json::json!([{ "domain": "GLOBAL", "by": "always" }]), "the miss, on record");

    let served = row["served"].as_array().expect("served");
    let rule = |text: &str| {
        served
            .iter()
            .find(|i| i["id"] == rule_id("GLOBAL", text))
            .unwrap_or_else(|| panic!("{text:?} not served: {served:#?}"))
    };
    for text in ["the global rule", "a second global rule"] {
        let item = rule(text);
        assert_eq!(item["kind"], "rule");
        assert_eq!(item["domain"], "GLOBAL");
        assert_eq!(item["block"], "global-rules");
    }
    let decision = served.iter().find(|i| i["kind"] == "decision").unwrap_or_else(|| panic!("{served:#?}"));
    assert_eq!(decision["id"], slug.as_str(), "a decision by its slug");
    assert_eq!(decision["block"], "global-decisions");
    assert_eq!(row["cut"], serde_json::json!([]));
    assert_eq!(row["scores"], serde_json::json!([]));
    assert!(row.get("tool").is_none() && row.get("path").is_none(), "{row}");
}

/// Example 2: a file in a child project that says `nested = true` brings the child by its folder and the parent as
/// its parent. The row names the file, each domain and what held it, and the rules printed.
#[test]
fn match_log_row_for_file_touch() {
    let h = Home::new("", "");
    let parent = h.root.join("Documents").join("Vintryx");
    let child = parent.join("dealer-registry");
    std::fs::create_dir_all(&child).unwrap();
    let readme = child.join("README.md");
    std::fs::write(&readme, "x\n").unwrap();
    let slash = |p: &Path| p.display().to_string().replace('\\', "/");
    h.base(&["project", "add", "--name", "vintryx", "--path", &slash(&parent)]);
    h.base(&["project", "add", "--name", "vintryx-dealer-registry", "--path", &slash(&child)]);
    h.base(&["project", "update", "vintryx-dealer-registry", "--parent", "vintryx", "--nested", "true"]);
    h.base(&["rule", "add", "--domain", "vintryx", "--text", "Vintryx rule: the EOD goes out twice a day"]);
    h.base(&["rule", "add", "--domain", "vintryx-dealer-registry", "--text", "Dealer registry rule: one row per rooftop"]);

    let (code, stdout, stderr) = h.tool("Read", serde_json::json!({ "file_path": slash(&readme) }), "bo13-file");
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("Dealer registry rule"), "control: the child's rule was printed:\n{stdout}");
    let row = h.only_row();
    assert_eq!(row["event"], "file");
    assert_eq!(row["session"], "bo13-file");
    assert_eq!(row["tool"], "Read");
    assert_eq!(row["path"], slash(&readme).as_str(), "the file the call touched, as the call named it");
    assert!(row.get("text").is_none(), "a file row keeps no prompt text: {row}");
    let held = base::domain::matcher::resolve_trigger(&slash(&child), None, None).unwrap();
    assert_eq!(
        row["matched"],
        serde_json::json!([
            { "domain": "vintryx-dealer-registry", "by": "path", "value": held },
            { "domain": "vintryx", "by": "parent", "value": "vintryx-dealer-registry nested" },
        ]),
    );
    assert_eq!(
        ids(&row["served"]),
        [
            rule_id("vintryx-dealer-registry", "Dealer registry rule: one row per rooftop"),
            rule_id("vintryx", "Vintryx rule: the EOD goes out twice a day"),
        ],
        "child first, then the parent, as printed"
    );
    assert!(row["served"].as_array().unwrap().iter().all(|i| i["kind"] == "rule"));
    assert_eq!(row["cut"], serde_json::json!([]), "the tool hook prints whole: nothing is cut");

    // A tool call that names no path and serves nothing is not a file touch: no row.
    h.tool("ToolSearch", serde_json::json!({ "query": "select:Monitor" }), "bo13-file");
    assert_eq!(h.rows().len(), 1, "no row for a call that touched nothing");
}

/// A budget cut is in `cut` with its reason, the setting that decided it, and the block that carried it: here the
/// always-on GLOBAL rules (priority 4) do not fit beside the rules matched to the prompt (priority 1).
#[test]
fn match_log_records_cut_with_reason() {
    let long = "a long global rule that will not fit ".repeat(30);
    let domains = format!(
        "[[domain]]\nname = \"GLOBAL\"\nmode = \"always\"\nrules = [{}]\n\n[[domain]]\nname = \"hooks\"\nprompt_keywords = [\"prompt submit\"]\nrules = [\"hooks rule: fit block by block\"]\n",
        toml::Value::String(long.clone())
    );
    let h = Home::new("[budget]\nprompt_bytes = 600\n", &domains);
    let (code, stdout, stderr) = h.prompt(EXAMPLE_ONE, "bo13-cut");
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("hooks rule: fit block by block"), "control: the matched rule was printed:\n{stdout}");
    assert!(stdout.contains("[base: withheld global-rules"), "control: GLOBAL's rules were dropped:\n{stdout}");
    let row = h.only_row();
    assert_eq!(
        row["matched"],
        serde_json::json!([
            { "domain": "GLOBAL", "by": "always" },
            { "domain": "hooks", "by": "keyword", "value": "prompt submit" },
        ])
    );
    assert_eq!(ids(&row["served"]), [rule_id("hooks", "hooks rule: fit block by block")]);
    assert_eq!(
        row["cut"],
        serde_json::json!([{
            "id": rule_id("GLOBAL", &long),
            "kind": "rule",
            "domain": "GLOBAL",
            "block": "global-rules",
            "reason": "budget",
            "limit": "prompt_bytes",
        }]),
    );
}

/// The two cuts `select` makes before the fit, and the scores it used: a topic rule past `[rules] topic_max` is cut
/// for the topic limit; one that scored above zero and under `topic_min_score` is cut as not matched. Both scores,
/// and the served rule's, are in `scores`.
#[test]
fn match_log_records_topic_cuts_and_scores() {
    let h = Home::new("[rules]\ntopic_max = 1\n", "");
    h.base(&["rule", "add", "--domain", "topics", "--text", "First topic rule", "--words", "relay ping, ping chris"]);
    h.base(&["rule", "add", "--domain", "topics", "--text", "Second topic rule", "--words", "relay ping"]);
    h.base(&["rule", "add", "--domain", "topics", "--text", "Watch the inbox folder", "--words", "inbox watcher"]);
    let (code, stdout, stderr) = h.prompt("relay ping chris about the inbox", "bo13-topics");
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("First topic rule"), "control: the best-scoring rule was printed:\n{stdout}");
    let row = h.only_row();
    let (first, second, third) =
        (rule_id("topics", "First topic rule"), rule_id("topics", "Second topic rule"), rule_id("topics", "Watch the inbox folder"));
    assert_eq!(ids(&row["served"]), [first.clone()]);
    assert_eq!(row["served"][0]["by"], "topic");
    assert_eq!(row["served"][0]["score"], 2.0);
    let cut = row["cut"].as_array().unwrap();
    let find = |id: &str| cut.iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("{id} not cut: {cut:#?}"));
    assert_eq!(find(&second)["reason"], "topic limit");
    assert_eq!(find(&second)["limit"], "topic_max");
    assert_eq!(find(&second)["score"], 1.0);
    assert_eq!(find(&third)["reason"], "not matched");
    assert_eq!(find(&third)["limit"], "topic_min_score");
    assert_eq!(find(&third)["score"], 0.25, "one rule-text word: under the 0.75 minimum");
    let mut scores: Vec<(String, f64)> = row["scores"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_str().unwrap().to_string(), s["score"].as_f64().unwrap()))
        .collect();
    let mut want = vec![(first, 2.0), (second, 1.0), (third, 0.25)];
    scores.sort_by(|a, b| a.0.cmp(&b.0));
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(scores, want, "every score select computed, served or not");
}

/// K1d: every shape becomes `[SECRET:<kind>]`; ordinary text is left alone. Then Example 3 end to end: the key never
/// reaches the file.
#[test]
fn secret_scrub_kinds() {
    let cases = [
        ("key sk-ant-api03-AbC123dEf456GhI789 here", "key [SECRET:anthropic-key] here"),
        ("key sk-proj-a1B2c3D4e5F6g7H8i9J0kL here", "key [SECRET:openai-key] here"),
        ("token ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789 here", "token [SECRET:github-token] here"),
        ("token github_pat_11ABCDEFG0123456789_abcdefghijk here", "token [SECRET:github-token] here"),
        ("id AKIAIOSFODNN7EXAMPLE here", "id [SECRET:aws-key] here"),
        ("bot xoxb-1234567890-abcdefghij here", "bot [SECRET:slack-token] here"),
        ("user xoxp-1234567890-abcdefghij here", "user [SECRET:slack-token] here"),
        ("Authorization: Bearer abc.DEF-123_456~xyz789 ok", "Authorization: Bearer [SECRET:bearer-token] ok"),
        ("login with password=hunter2 now", "login with password=[SECRET:password] now"),
        ("passwd: s3cr3t! then", "passwd: [SECRET:password] then"),
        ("{\"password\": \"a b c\"}", "{\"password\": \"[SECRET:password]\"}"),
        ("OPENAI_API_KEY=abc123 set", "OPENAI_API_KEY=[SECRET:credential] set"),
        (
            "x -----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\nabc\n-----END RSA PRIVATE KEY----- y",
            "x [SECRET:private-key] y",
        ),
        ("cut -----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk", "cut [SECRET:private-key]"),
        (
            "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U ok",
            "jwt [SECRET:jwt] ok",
        ),
    ];
    for (input, want) in cases {
        assert_eq!(base::scrub::scrub(input), want, "{input:?}");
    }
    for plain in [
        "draft the morning update for Anthony",
        "the task-force and risk-assessment-for-the-new-project-2026 notes",
        "a bearer token and the password field are both described here",
        "AKIAN is a word, eyJ.short.x is not a JWT",
        "-----BEGIN PUBLIC KEY----- is not secret",
    ] {
        assert_eq!(base::scrub::scrub(plain), plain, "ordinary text is untouched");
    }

    // Example 3, through the hook.
    let h = Home::new("", GLOBAL_DOMAIN);
    let key = "sk-ant-api03-AbC123dEf456GhI789jKl012";
    let (code, _, stderr) = h.prompt(&format!("here is the key {key} use it for the test"), "bo13-scrub");
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(h.only_row()["text"], "here is the key [SECRET:anthropic-key] use it for the test");
    let on_disk = std::fs::read_to_string(h.log_path()).unwrap();
    assert!(!on_disk.contains("sk-ant"), "the key never reached the file: {on_disk}");
}

/// K1c: `full` keeps the prompt, `matched` only the words that matched, `off` no text; a value base does not know
/// keeps nothing.
#[test]
fn prompt_text_setting() {
    let domains = "[[domain]]\nname = \"vintryx\"\nprompt_keywords = [\"anthony\", \"eod\"]\nrules = [\"Vintryx rule\"]\n";
    let prompt = "draft the morning update for Anthony, password=hunter2";
    let run = |setting: Option<&str>| {
        let toml = setting.map(|s| format!("[log]\nprompt_text = \"{s}\"\n")).unwrap_or_default();
        let h = Home::new(&toml, domains);
        let (code, _, stderr) = h.prompt(prompt, "bo13-text");
        assert_eq!(code, 0, "{stderr}");
        let row = h.only_row();
        assert_eq!(row["matched"], serde_json::json!([{ "domain": "vintryx", "by": "keyword", "value": "anthony" }]));
        row.get("text").cloned()
    };
    let scrubbed = "draft the morning update for Anthony, password=[SECRET:password]";
    assert_eq!(run(None), Some(scrubbed.into()), "the default is full, scrubbed");
    assert_eq!(run(Some("full")), Some(scrubbed.into()));
    assert_eq!(run(Some("matched")), Some("anthony".into()), "only the words that matched");
    assert_eq!(run(Some("off")), None, "no text at all");
    assert_eq!(run(Some("everything")), None, "an unknown value keeps the least");
}

/// K1e: session start removes rows dated more than `[log] prompt_days` calendar days ago, keeps the rest in order,
/// and drops a line that is not a row. With the first row young it leaves the file alone.
#[test]
fn match_log_retention() {
    let row = |days: i64, tag: &str| {
        let ts = (chrono::Local::now() - chrono::Duration::days(days)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        serde_json::json!({ "ts": ts, "session": tag, "event": "prompt", "matched": [], "served": [], "cut": [], "scores": [] })
            .to_string()
    };
    let write = |h: &Home, lines: &[String]| {
        std::fs::write(h.log_path(), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    };
    let sessions = |h: &Home| -> Vec<String> {
        h.rows().iter().map(|r| r["session"].as_str().unwrap().to_string()).collect()
    };

    let h = Home::new("", "");
    write(&h, &[row(120, "d120"), row(91, "d91"), "not a row".into(), row(89, "d89"), row(0, "today")]);
    let (code, stdout, stderr) = h.session_start("bo13-retention");
    assert_eq!(code, 0, "{stderr}");
    assert!(!stdout.is_empty(), "control: session start printed");
    assert_eq!(sessions(&h), ["d89", "today"], "the default keeps 90 days");

    // The setting is read: 30 days removes the 89-day row too.
    let h = Home::new("[log]\nprompt_days = 30\n", "");
    write(&h, &[row(89, "d89"), row(31, "d31"), row(29, "d29"), row(0, "today")]);
    assert_eq!(h.session_start("bo13-retention-30").0, 0);
    assert_eq!(sessions(&h), ["d29", "today"]);

    // Nothing old first: nothing is rewritten, byte for byte.
    let before = std::fs::read(h.log_path()).unwrap();
    assert_eq!(h.session_start("bo13-retention-again").0, 0);
    assert_eq!(std::fs::read(h.log_path()).unwrap(), before);
}

/// K1g: a row that cannot be written changes nothing the hook prints and never fails it; the reason goes to stderr.
/// Two failures: the log is a read-only file, and a folder stands where the file goes.
#[test]
fn match_log_write_failure_does_not_break_hook() {
    let domains = "[[domain]]\nname = \"hooks\"\nprompt_keywords = [\"prompt submit\"]\nrules = [\"hooks rule\"]\npaths = [\"notes\"]\n";
    let setup = || {
        let h = Home::new("", domains);
        std::fs::create_dir_all(h.root.join("notes")).unwrap();
        std::fs::write(h.root.join("notes").join("a.txt"), "x\n").unwrap();
        h
    };
    // Two sessions, so the tool call is not told the rule the prompt was already told.
    let run = |h: &Home| {
        let prompt = h.prompt(EXAMPLE_ONE, "bo13-fail-prompt");
        let file = h.root.join("notes").join("a.txt").display().to_string();
        let tool = h.tool("Read", serde_json::json!({ "file_path": file }), "bo13-fail-tool");
        (prompt, tool)
    };

    let ok = setup();
    let ((code, prompt_out, _), (tcode, tool_out, _)) = run(&ok);
    assert_eq!((code, tcode), (0, 0));
    assert!(prompt_out.contains("hooks rule") && tool_out.contains("hooks rule"), "control: both hooks served");
    assert_eq!(ok.rows().len(), 2, "control: the writable log took both rows");

    let folder = setup();
    std::fs::create_dir_all(folder.log_path()).unwrap();
    let readonly = setup();
    std::fs::write(readonly.log_path(), "").unwrap();
    let mut perms = std::fs::metadata(readonly.log_path()).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(readonly.log_path(), perms).unwrap();
    // An account that writes through read-only bits (root) cannot run the read-only leg; the folder leg still runs.
    let readonly_blocks = std::fs::OpenOptions::new().append(true).open(readonly.log_path()).is_err();

    for (name, h) in [("folder", &folder), ("read-only", &readonly)] {
        if name == "read-only" && !readonly_blocks {
            println!("skipped the read-only leg: this account writes through read-only files");
            continue;
        }
        let ((code, out, err), (tcode, tout, terr)) = run(h);
        assert_eq!((code, tcode), (0, 0), "{name}: the hooks exit 0: {err}\n{terr}");
        assert_eq!(out, prompt_out, "{name}: the prompt hook's output is unchanged");
        assert_eq!(tout, tool_out, "{name}: the tool hook's output is unchanged");
        assert!(err.contains("could not write its match log row"), "{name}: the prompt hook says why: {err}");
        assert!(terr.contains("could not write its match log row"), "{name}: the tool hook says why: {terr}");
    }

    // Session start's retention pass fails open the same way.
    let (code, out, err) = folder.session_start("bo13-fail-start");
    assert_eq!(code, 0, "{err}");
    assert!(!out.is_empty() && err.contains("could not prune the match log"), "{err}");
}

/// K1f, Example 4: `base log matches` prints one line per row, oldest first, with the time, the event, each domain
/// and what matched it, the counts, and the prompt's start or the file. `--json`, `--session`, `--rule` and `--last`
/// read the same rows.
#[test]
fn log_matches_reader() {
    let h = Home::new("", "");
    let today = chrono::Local::now().date_naive();
    let at = |hms: (u32, u32, u32)| {
        today
            .and_hms_opt(hms.0, hms.1, hms.2)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .earliest()
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
    };
    let items = |n: usize, tag: &str| -> Vec<serde_json::Value> {
        (0..n).map(|i| serde_json::json!({ "id": format!("{tag}{i:02}"), "kind": "rule" })).collect()
    };
    let rows = [
        serde_json::json!({
            "ts": at((14, 11, 29)), "session": "5b860473", "event": "prompt",
            "text": "I want to make sure that we are working with a version of the base that's actually good.",
            "matched": [{ "domain": "GLOBAL", "by": "always" }],
            "served": items(5, "g"), "cut": [], "scores": [],
        }),
        serde_json::json!({
            "ts": at((14, 13, 2)), "session": "5b860473", "event": "file", "tool": "Read",
            "path": "C:/Users/someone/Documents/Vintryx/dealer-registry/README.md",
            "matched": [
                { "domain": "vintryx-dealer-registry", "by": "path", "value": "C:/Users/someone/Documents/Vintryx/dealer-registry" },
                { "domain": "vintryx", "by": "parent", "value": "vintryx-dealer-registry nested" },
            ],
            "served": items(9, "v"),
            "cut": [{ "id": "c00", "kind": "rule", "reason": "budget", "limit": "prompt_bytes" }],
            "scores": [],
        }),
        serde_json::json!({
            "ts": at((14, 20, 45)), "session": "a1c0ffee", "event": "prompt",
            "text": "draft the morning update for Anthony",
            "matched": [{ "domain": "vintryx", "by": "keyword", "value": "anthony" }],
            "served": items(4, "a"), "cut": [], "scores": [],
        }),
    ];
    let body: String = rows.iter().map(|r| format!("{r}\n")).collect();
    std::fs::write(h.log_path(), body).unwrap();

    let out = h.base(&["log", "matches", "--last", "3"]);
    let want = [
        "14:11:29  prompt  GLOBAL(always)                        served 5  cut 0  \"I want to make sure that we are working...\"",
        "14:13:02  file    vintryx-dealer-registry(path) vintryx(parent)  served 9  cut 1  .../dealer-registry/README.md",
        "14:20:45  prompt  vintryx(keyword: anthony)             served 4  cut 0  \"draft the morning update for Anthony\"",
    ];
    assert_eq!(out.lines().collect::<Vec<_>>(), want, "Example 4:\n{out}");

    let json = h.base(&["log", "matches", "--json"]);
    let back: Vec<serde_json::Value> = json.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(back.len(), 3);
    assert_eq!(back[1]["path"], rows[1]["path"]);
    assert_eq!(back[1]["cut"], rows[1]["cut"], "a row reads back as written");

    assert_eq!(h.base(&["log", "matches", "--last", "1"]).lines().count(), 1);
    assert!(h.base(&["log", "matches", "--last", "1"]).starts_with("14:20:45"), "the newest");
    let one = h.base(&["log", "matches", "--session", "a1c0"]);
    assert_eq!(one.lines().count(), 1, "{one}");
    assert!(one.contains("vintryx(keyword: anthony)"));
    let rule = h.base(&["log", "matches", "--rule", "c00"]);
    assert_eq!(rule.lines().collect::<Vec<_>>(), [want[1]], "a rule cut counts");
    let none = h.run(&["log", "matches", "--rule", "nothing-has-this"], None);
    assert_eq!((none.0, none.1.as_str()), (0, ""), "no rows is not an error");
    assert!(none.2.contains("no rows"), "{}", none.2);
}
