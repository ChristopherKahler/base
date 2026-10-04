//! BO-20 (K9): shadow mode, through the real hooks and commands on seeded homes.
//!
//! Every name and text here is invented (this repository is public). Each test seeds a home with its own
//! `domains.toml`, drives `base hook ...` as Claude Code does, and reads the match log, the shadow state and base.toml
//! back. Where a test needs evidence (wins, losses, corrections), it writes match-log rows of the shape the hooks write
//! (`emit::match_log::Row`), timed after the candidate started.

mod seed;

use std::path::{Path, PathBuf};

use seed::{run_base, run_pre_tool_use, run_prompt_submit, run_session_start};
use serde_json::{json, Value};

/// Four invented domains. `ledger` and `tide` each hold a rule with words no other rule shares, so a correction made
/// of those words sorts to that rule (BO-15's sort: a stand-out fit). `books` is matched by the folder a file is in.
const DOMAINS: &str = r#"
[[domain]]
name = "ledger"
prompt_keywords = ["ledger"]
rules = [
  { text = "Close the books only after the payroll export is reconciled against the bank feed.", fires_on = ["the payroll export never reconciled with the bank feed this month"] },
  "Rotate the quartz signing keys before the falcon upload runs.",
]

[[domain]]
name = "tide"
prompt_keywords = ["tide"]
rules = ["Mirror the cobalt harbor charts into the archive every night."]

[[domain]]
name = "garden"
prompt_keywords = ["garden"]
rules = ["Water the tomatoes before noon in the summer heat."]
"#;

const PAYROLL: &str = "Close the books only after the payroll export is reconciled against the bank feed.";
const QUARTZ: &str = "Rotate the quartz signing keys before the falcon upload runs.";
const COBALT: &str = "Mirror the cobalt harbor charts into the archive every night.";

/// A correction made of [`QUARTZ`]'s words, and one of [`COBALT`]'s: each fits its rule alone.
const FITS_QUARTZ: &str = "no, rotate the quartz signing keys before the falcon upload, rotate quartz signing keys falcon upload";
const FITS_COBALT: &str =
    "wrong, mirror the cobalt harbor charts into the archive, cobalt harbor charts archive mirror, cobalt harbor charts every night";

/// A prompt with no keyword of `ledger` that shares words with its payroll rule's test prompt.
const NEAR: &str = "why does the payroll export not reconcile with the bank feed";

fn id(domain: &str, text: &str) -> String {
    base::domain::rules::rule_id(domain, text)
}

fn root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("base-shadow-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(!root.exists(), "the seed root {} survived its clean", root.display());
    root
}

/// A tiny seed holding [`DOMAINS`], `toml` appended to its global base.toml.
fn seeded(tag: &str, toml: &str) -> seed::Seed {
    let s = seed::write(&root(tag), &seed::TINY, toml);
    std::fs::write(s.ws.join(".base").join("domains.toml"), DOMAINS).expect("domains.toml");
    s
}

fn base_ok(s: &seed::Seed, args: &[&str]) -> String {
    let (code, out, err) = run_base(s, args);
    assert_eq!(code, 0, "base {}: {out}{err}", args.join(" "));
    out
}

fn shadow_dir(s: &seed::Seed) -> PathBuf {
    s.home.join(".base-gbl").join(".base").join("shadow")
}

fn state(s: &seed::Seed) -> Value {
    let p = shadow_dir(s).join("state.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).expect("state")
}

/// The running candidate's name.
fn candidate(s: &seed::Seed) -> String {
    state(s)["candidate"]["name"].as_str().expect("a candidate runs").to_string()
}

fn log_path(s: &seed::Seed) -> PathBuf {
    s.ws.join(".base").join("match-log.jsonl")
}

fn rows(s: &seed::Seed) -> Vec<Value> {
    std::fs::read_to_string(log_path(s))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn global_toml(s: &seed::Seed) -> String {
    std::fs::read_to_string(s.home.join(".base-gbl").join("base.toml")).expect("base.toml")
}

/// `seconds` from now, as a row's `ts`.
fn at(seconds: i64) -> String {
    (chrono::Local::now() + chrono::Duration::seconds(seconds)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// Append rows to the workspace's match log.
fn append(s: &seed::Seed, rows: &[Value]) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log_path(s)).expect("match log");
    for r in rows {
        writeln!(f, "{r}").expect("row");
    }
}

/// A typed prompt's row, with the candidate's entry when given.
fn prompt_row(ts: &str, session: &str, n: u32, text: &str, shadow: Option<Value>) -> Value {
    let mut r = json!({"ts": ts, "session": session, "event": "prompt", "prompt_num": n, "text": text,
                       "matched": [], "served": [], "cut": [], "scores": []});
    if let Some(sh) = shadow {
        r["shadow"] = sh;
    }
    r
}

/// A correction signal logged on prompt `n`: a C1 phrase, about reply `n - 1` (lynx's Q4 mapping).
fn phrase_row(ts: &str, session: &str, n: u32) -> Value {
    json!({"ts": ts, "session": session, "event": "signal", "prompt_num": n,
           "signals": [{"layer": "C1", "kind": "phrase", "value": "no,"}]})
}

/// An interrupt logged on prompt `n`: about reply `n` itself.
fn interrupt_row(ts: &str, session: &str, n: u32) -> Value {
    json!({"ts": ts, "session": session, "event": "signal", "prompt_num": n, "signals": [{"layer": "C2", "kind": "interrupt"}]})
}

fn entry(name: &str, adds: &[&str], drops: &[&str]) -> Value {
    json!({"candidate": name, "adds": adds, "drops": drops, "ms": 3})
}

/// Evidence for `name`: `events` typed prompts the candidate ran on, of which `wins` added the quartz rule and a
/// correction about that reply fits it, and `losses` dropped the cobalt rule and a correction fits that. Each in a
/// session of its own; the corrections are prompt 2 of their session.
fn evidence(s: &seed::Seed, name: &str, events: usize, wins: usize, losses: usize) {
    let (quartz, cobalt) = (id("ledger", QUARTZ), id("tide", COBALT));
    let mut out: Vec<Value> = Vec::new();
    for i in 0..events {
        let session = format!("ev-{i:04}");
        let t = 10 + i as i64 * 3;
        let sh = if i < wins {
            entry(name, &[quartz.as_str()], &[])
        } else if i < wins + losses {
            entry(name, &[], &[cobalt.as_str()])
        } else {
            entry(name, &[], &[])
        };
        out.push(prompt_row(&at(t), &session, 1, &format!("an ordinary request number {i}"), Some(sh)));
        let fix = if i < wins {
            Some(FITS_QUARTZ)
        } else if i < wins + losses {
            Some(FITS_COBALT)
        } else {
            None
        };
        if let Some(text) = fix {
            out.push(prompt_row(&at(t + 1), &session, 2, text, None));
            out.push(phrase_row(&at(t + 1), &session, 2));
        }
    }
    append(s, &out);
}

/// Integers that look like a time in seconds since the epoch, set to 0: the session file stamps when it was written.
fn untimed(v: &mut Value) {
    match v {
        Value::Number(n) if n.as_u64().is_some_and(|x| (1_600_000_000..4_000_000_000).contains(&x)) => *v = json!(0),
        Value::Array(a) => a.iter_mut().for_each(untimed),
        Value::Object(o) => o.values_mut().for_each(untimed),
        _ => {}
    }
}

/// `text` with `s`'s root written as `<root>`, in every spelling a path takes in a file.
fn rooted(s: &seed::Seed, text: &str) -> String {
    let root = s.ws.parent().expect("seed root").display().to_string();
    let mut out = text.to_string();
    for r in [root.clone(), root.replace('\\', "/"), root.replace('\\', "\\\\")] {
        // The drive letter in either case: one writer spells it `c:`.
        let mut c = r.chars();
        let Some(d) = c.next() else { continue };
        for spelling in [format!("{}{}", d.to_ascii_lowercase(), c.as_str()), format!("{}{}", d.to_ascii_uppercase(), c.as_str())] {
            out = out.replace(&spelling, "<root>");
        }
    }
    out
}

/// What one prompt and one tool call left: the two outputs, the session file, the prompt blocks file, the prompt's
/// output record, and both match-log rows, each without its time and its shadow entry.
#[derive(Debug, PartialEq)]
struct Left {
    prompt_out: String,
    tool_out: String,
    session: Value,
    blocks: Value,
    record: Value,
    prompt_row: Value,
    file_row: Value,
}

fn run_both(s: &seed::Seed, sid: &str) -> (Left, Value, Value) {
    let (code, prompt_out, err) = run_prompt_submit(s, NEAR, Some(sid));
    assert_eq!(code, 0, "{err}");
    let file = s.ws.join("books").join("q3.md");
    let (code, tool_out, err) = run_pre_tool_use(s, "Read", json!({"file_path": file.display().to_string()}), sid, &[]);
    assert_eq!(code, 0, "{err}");
    let base = s.ws.join(".base");
    let mut session: Value =
        serde_json::from_str(&rooted(s, &std::fs::read_to_string(base.join(".session")).expect(".session"))).expect("json");
    untimed(&mut session);
    let blocks_file = base.join("hook-output").join(sid).join("prompt-blocks.json");
    let mut blocks: Value =
        serde_json::from_str(&rooted(s, &std::fs::read_to_string(blocks_file).expect("blocks"))).expect("blocks json");
    // When it was written: the two runs are a second apart.
    blocks.as_object_mut().expect("object").remove("written_at");
    let record_text = std::fs::read_to_string(base.join("hook-output.jsonl")).expect("records");
    let mut record: Value = serde_json::from_str(&rooted(s, record_text.lines().last().expect("a record"))).expect("record");
    record.as_object_mut().expect("object").remove("ts");
    let all = rows(s);
    let mut prompt_row = all.iter().find(|r| r["event"] == "prompt").cloned().expect("a prompt row");
    let mut file_row = all.iter().find(|r| r["event"] == "file").cloned().expect("a file row");
    let (ps, fs) = (prompt_row["shadow"].take(), file_row["shadow"].take());
    for r in [&mut prompt_row, &mut file_row] {
        let o = r.as_object_mut().expect("object");
        o.remove("ts");
        o.remove("shadow");
        *r = serde_json::from_str(&rooted(s, &r.to_string())).expect("row");
    }
    (Left { prompt_out: rooted(s, &prompt_out), tool_out: rooted(s, &tool_out), session, blocks, record, prompt_row, file_row }, ps, fs)
}

/// `books` matched by the folder: a domain whose path trigger is the seed's `books` folder.
fn with_books(s: &seed::Seed) {
    let books = s.ws.join("books");
    std::fs::create_dir_all(&books).expect("books");
    std::fs::write(books.join("q3.md"), "# q3\n").expect("q3.md");
    let extra = format!(
        "\n[[domain]]\nname = \"books\"\npaths = [\"{}\"]\nrules = [\"Keep every quarter's books in its own file.\"]\n",
        books.display().to_string().replace('\\', "/")
    );
    let path = s.ws.join(".base").join("domains.toml");
    let text = std::fs::read_to_string(&path).expect("domains.toml") + &extra;
    std::fs::write(&path, text).expect("domains.toml");
}

/// K9c: the hook output is byte for byte the same with the shadow on and off, and so is the live session state, on a
/// prompt and on a file touch. Control: the candidate's pick differs (it admits the near miss by score).
#[test]
fn shadow_does_not_change_output_or_state() {
    let toml = "[match]\nbm25 = false\n";
    let off = seeded("k9c-off", toml);
    with_books(&off);
    let (left_off, ps, fs) = run_both(&off, "k9c-1");
    assert!(ps.is_null() && fs.is_null(), "no shadow, no entry");

    let on = seeded("k9c-on", toml);
    with_books(&on);
    base_ok(&on, &["shadow", "start", "--matcher", "bm25", "--min-score", "2.0"]);
    let (left_on, ps, fs) = run_both(&on, "k9c-1");
    assert_eq!(left_off, left_on, "the shadow changed what live printed or recorded");

    let name = candidate(&on);
    assert_eq!(ps["candidate"], name.as_str(), "{ps}");
    assert!(ps.get("skipped").is_none(), "{ps}");
    let payroll = id("ledger", PAYROLL);
    assert!(
        ps["adds"].as_array().is_some_and(|a| a.iter().any(|x| x == payroll.as_str())),
        "control: the candidate admits the near miss live does not serve: {ps}"
    );
    assert_eq!(fs["candidate"], name.as_str(), "the file touch carries an entry too: {fs}");
    assert!(fs.get("adds").is_none() && fs.get("drops").is_none(), "a [match] setting never reaches the tool hook: {fs}");
    assert!(left_on.tool_out.contains("Keep every quarter"), "control: the file touch served books: {}", left_on.tool_out);
}

/// K9d: past `[shadow] max_ms` the candidate is stopped and the row says so; the output is live's all the same. Control
/// at 10000 ms: it runs to the end.
#[test]
fn shadow_skips_when_slow() {
    let s = seeded("slow", "[match]\nbm25 = false\n\n[shadow]\nmax_ms = 0\n");
    base_ok(&s, &["shadow", "start", "--matcher", "bm25", "--min-score", "2.0"]);
    let (code, out, err) = run_prompt_submit(&s, NEAR, Some("slow-1"));
    assert_eq!(code, 0, "{err}");
    let row = rows(&s).into_iter().find(|r| r["session"] == "slow-1").expect("row");
    assert_eq!(row["shadow"]["skipped"], "slow", "{row}");
    assert!(row["shadow"].get("adds").is_none(), "nothing is known of a stopped run's pick: {row}");
    assert!(!out.contains(PAYROLL), "live keyword-only does not serve the near miss: {out}");

    let s = seeded("slow-control", "[match]\nbm25 = false\n\n[shadow]\nmax_ms = 10000\n");
    base_ok(&s, &["shadow", "start", "--matcher", "bm25", "--min-score", "2.0"]);
    let (code, out2, err) = run_prompt_submit(&s, NEAR, Some("slow-1"));
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, out2, "the same output either way");
    let row = rows(&s).into_iter().find(|r| r["session"] == "slow-1").expect("row");
    assert!(row["shadow"].get("skipped").is_none(), "control: within the limit it runs: {row}");
}

/// K9e, Example 2's shape on a fixture log: the events, where they differ, what the candidate adds and drops, and the
/// wins and losses the corrections that followed give it.
#[test]
fn shadow_report_wins_and_losses() {
    let s = seeded("report", "[match]\nbm25 = false\n");
    base_ok(&s, &["shadow", "start", "--matcher", "bm25"]);
    let name = candidate(&s);
    evidence(&s, &name, 6, 2, 1);
    // A file touch the candidate would have served something else on, in a session of its own.
    append(&s, &[json!({"ts": at(200), "session": "ev-file", "event": "file", "tool": "Read", "path": "x.md",
                        "matched": [], "served": [], "cut": [], "scores": [],
                        "shadow": {"candidate": name, "adds": [id("garden", "Water the tomatoes before noon in the summer heat.")], "ms": 1}})]);
    let j: Value = serde_json::from_str(&base_ok(&s, &["shadow", "report", "--json"])).expect("report json");
    assert_eq!(j["events"], 6, "{j}");
    assert_eq!(j["files"], 1, "{j}");
    assert_eq!((j["differ_prompts"].as_u64(), j["differ_files"].as_u64()), (Some(3), Some(1)), "{j}");
    assert_eq!(j["adds"][0], json!([id("ledger", QUARTZ), 2]), "{j}");
    assert_eq!(j["drops"][0], json!([id("tide", COBALT), 1]), "{j}");
    assert_eq!(j["wins"].as_array().map(Vec::len), Some(2), "{j}");
    assert_eq!(j["losses"].as_array().map(Vec::len), Some(1), "{j}");
    assert_eq!(j["protected_losses"].as_array().map(Vec::len), Some(0), "{j}");
    let text = base_ok(&s, &["shadow", "report"]);
    assert!(text.contains(&format!("candidate {name} vs live")), "{text}");
    assert!(text.contains("wins 2 · losses 1 · protected losses 0"), "{text}");
    assert!(text.contains("status: collecting: 6 of [shadow] min_prompts = 200"), "{text}");
}

/// K9f with lynx's Q5 ruling: promoted at session start when every condition holds, and not when one fails: 199
/// typed prompts; wins under 3 x losses; no win at all.
#[test]
fn auto_promote_when_conditions_met() {
    for (tag, events, wins, losses, promotes) in
        [("short", 199, 3, 1, false), ("unclear", 200, 2, 1, false), ("no-win", 200, 0, 0, false), ("met", 200, 3, 1, true)]
    {
        let s = seeded(&format!("promote-{tag}"), "[match]\n# the operator's own note\nbm25 = false\n");
        base_ok(&s, &["shadow", "start", "--matcher", "bm25"]);
        let name = candidate(&s);
        evidence(&s, &name, events, wins, losses);
        let (code, out, err) = run_session_start(&s, Some(&format!("start-{tag}")));
        assert_eq!(code, 0, "[{tag}] {err}");
        let line = format!("matcher: candidate {name} promoted (wins {wins}, losses {losses}, {events} prompts) · undo: base shadow rollback");
        if promotes {
            assert!(out.contains(&line), "[{tag}] the session start that promoted says so:\n{out}");
            let toml = global_toml(&s);
            assert!(toml.contains("bm25 = true") && toml.contains("# the operator's own note"), "[{tag}] {toml}");
            assert_eq!(state(&s)["live"], name.as_str(), "[{tag}]");
            assert!(state(&s).get("candidate").is_none(), "[{tag}] the candidate is live now");
            let backups = std::fs::read_dir(s.home.join(".base-gbl"))
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().contains(&format!("-pre-{name}")))
                .count();
            assert_eq!(backups, 1, "[{tag}] base.toml backed up before the write");
        } else {
            assert!(!out.contains("matcher:"), "[{tag}] nothing to say:\n{out}");
            assert!(global_toml(&s).contains("bm25 = false"), "[{tag}] base.toml untouched");
            assert_eq!(candidate(&s), name, "[{tag}] still running");
        }
    }
}

/// K9f: a loss on a rule marked protected blocks promotion whatever the wins, and the report says why.
#[test]
fn protected_rule_loss_blocks_promotion() {
    let s = seeded("protected", "[match]\nbm25 = false\n");
    let cobalt = base::domain::rule_test::short_ref("tide", &id("tide", COBALT));
    let out = base_ok(&s, &["rule", "update", &cobalt, "--protected"]);
    assert!(out.contains("is protected"), "{out}");
    base_ok(&s, &["shadow", "start", "--matcher", "bm25"]);
    let name = candidate(&s);
    evidence(&s, &name, 200, 5, 1);
    let (code, out, err) = run_session_start(&s, Some("start-protected"));
    assert_eq!(code, 0, "{err}");
    assert!(!out.contains("promoted"), "no promotion past a protected loss:\n{out}");
    let report = base_ok(&s, &["shadow", "report"]);
    assert!(report.contains("protected losses 1"), "{report}");
    assert!(report.contains("status: not promoted: a loss on protected rule"), "{report}");
}

/// Promote by hand, so the watch starts now: `before` typed prompts before it with `before_fixed` corrected (written
/// earlier in time), then `watch` typed prompts after it with `after_fixed` corrected. The candidate's name.
fn promoted_and_watched(s: &seed::Seed, before_fixed: usize, after_fixed: usize) -> String {
    base_ok(s, &["shadow", "start", "--matcher", "bm25"]);
    let name = candidate(s);
    let mut earlier: Vec<Value> = Vec::new();
    for i in 0..100 {
        let (session, ts) = (format!("before-{i:03}"), at(-3600 + i as i64));
        earlier.push(prompt_row(&ts, &session, 1, &format!("an earlier request number {i}"), None));
        if i < before_fixed {
            earlier.push(interrupt_row(&ts, &session, 1));
        }
    }
    append(s, &earlier);
    base_ok(s, &["shadow", "promote"]);
    let mut later: Vec<Value> = Vec::new();
    for i in 0..100 {
        let (session, ts) = (format!("after-{i:03}"), at(5 + i as i64));
        later.push(prompt_row(&ts, &session, 1, &format!("a later request number {i}"), None));
        if i < after_fixed {
            later.push(interrupt_row(&ts, &session, 1));
        }
    }
    append(s, &later);
    name
}

/// K9g with lynx's Q6 ruling (Example 3's shape): corrections rose from 4 to 7 per 100 prompts, under the noise margin
/// at a rate of 4 (2.5 x sqrt(2 x 0.04 x 0.96 / 100) x 100 = 6.9), so the promotion stands; from 4 to 15, over it, so
/// live goes back and session start says so with both rates and the margin.
#[test]
fn auto_rollback_when_corrections_rise() {
    let s = seeded("rollback-noise", "[match]\nbm25 = false\n");
    let name = promoted_and_watched(&s, 4, 7);
    let (code, out, err) = run_session_start(&s, Some("watch-1"));
    assert_eq!(code, 0, "{err}");
    assert!(!out.contains("rolled back"), "a rise inside the noise keeps the promotion:\n{out}");
    assert!(global_toml(&s).contains("bm25 = true"));
    assert_eq!(state(&s)["promotion"]["watching"], false, "the watch is over");
    assert_eq!(state(&s)["live"], name.as_str());

    let s = seeded("rollback-real", "[match]\nbm25 = false\n");
    let name = promoted_and_watched(&s, 4, 15);
    let previous = state(&s)["promotion"]["previous"].as_str().expect("previous").to_string();
    let (code, out, err) = run_session_start(&s, Some("watch-1"));
    assert_eq!(code, 0, "{err}");
    let line = format!(
        "matcher: {name} rolled back (corrections rose from 4 to 15 per 100 prompts, over the 7 noise margin) · redo: base shadow promote {name}"
    );
    assert!(out.contains(&line), "{line}\n----\n{out}");
    assert!(global_toml(&s).contains("bm25 = false"), "{}", global_toml(&s));
    assert_eq!(state(&s)["live"], previous.as_str());
}

/// K9h: an announcement is printed by one session start, and not by the next.
#[test]
fn announcement_once_at_session_start() {
    let s = seeded("announce", "[match]\nbm25 = false\n");
    base_ok(&s, &["shadow", "start", "--matcher", "bm25"]);
    let name = candidate(&s);
    base_ok(&s, &["shadow", "promote"]);
    let (_, first, _) = run_session_start(&s, Some("announce-1"));
    assert!(first.contains(&format!("matcher: {name} promoted by hand")), "{first}");
    let (_, second, _) = run_session_start(&s, Some("announce-2"));
    assert!(!second.contains("matcher:"), "said once:\n{second}");
}

/// K9i: promote and roll back by hand, and promote a named version again; base.toml's `[match]` follows each time and
/// every other line of the file stays. `stop` ends a candidate and deletes nothing but the state's candidate (Q11).
#[test]
fn manual_promote_and_rollback() {
    let s = seeded("manual", "[match]\n# keep this note\nbm25 = false # and this one\n\n[shadow]\nmax_ms = 50\n");
    let before = global_toml(&s);
    base_ok(&s, &["shadow", "start", "--matcher", "bm25"]);
    let name = candidate(&s);
    let out = base_ok(&s, &["shadow", "promote"]);
    assert!(out.contains(&format!("{name} promoted by hand")), "{out}");
    let promoted = global_toml(&s);
    assert!(promoted.contains("bm25 = true # and this one"), "the value changed, its comment kept:\n{promoted}");
    assert_eq!(promoted.replace("bm25 = true", "bm25 = false"), before, "nothing else in the file changed");

    let out = base_ok(&s, &["shadow", "rollback"]);
    assert!(out.contains(&format!("{name} rolled back by hand")), "{out}");
    assert_eq!(global_toml(&s), before, "rolled back to the file as it was");

    let out = base_ok(&s, &["shadow", "promote", &name]);
    assert!(out.contains("promoted"), "{out}");
    assert!(global_toml(&s).contains("bm25 = true # and this one"));

    // Stop: the candidate goes from the state; its version file stays.
    base_ok(&s, &["shadow", "start", "--matcher", "keyword-only"]);
    let second = candidate(&s);
    let out = base_ok(&s, &["shadow", "stop"]);
    assert!(out.contains(&format!("stopped candidate {second}")), "{out}");
    assert!(state(&s).get("candidate").is_none());
    assert!(shadow_dir(&s).join("versions").join(format!("{second}.json")).is_file(), "the version stays");
    let (code, _, err) = run_base(&s, &["shadow", "start", "--matcher", "bm25"]);
    assert_eq!(code, 1, "live is bm25 now, so bm25 is no candidate: {err}");
    assert!(err.contains("nothing would differ"), "{err}");
}

/// K9a with proposals: a keyword gap and a new rule run as the candidate; their prompts carry the rules they would
/// serve; promotion approves both; rollback undoes both, and they are pending again.
#[test]
fn shadow_start_from_proposals_runs_their_changes() {
    let s = seeded("proposals", "");
    base_ok(&s, &["domain", "sync"]);
    let gap_prompt = "the cobalt manifest came late again";
    let new_prompt = "where does the walrus roster go on friday";
    base_ok(&s, &[
        "rule", "propose", "--example", gap_prompt, "--keywords", "cobalt manifest", "--rule",
        &base::domain::rule_test::short_ref("tide", &id("tide", COBALT)),
    ]);
    base_ok(&s, &[
        "rule", "propose", "--new", "--domain", "garden", "--text", "File the walrus roster every Friday afternoon.",
        "--keywords", "walrus roster", "--example", new_prompt,
    ]);
    let listing = base_ok(&s, &["rule", "review"]);
    assert!(listing.contains("p-0001") && listing.contains("p-0002"), "{listing}");
    let before_domains = std::fs::read_to_string(s.ws.join(".base").join("domains.toml")).unwrap();

    base_ok(&s, &["shadow", "start", "--from-proposals", "p-0001,p-0002"]);
    let name = candidate(&s);
    assert!(name.starts_with("proposals-"), "{name}");
    for (sid, p) in [("gap-1", gap_prompt), ("new-1", new_prompt)] {
        let (code, out, err) = run_prompt_submit(&s, p, Some(sid));
        assert_eq!(code, 0, "{err}");
        assert!(!out.contains("cobalt harbor") && !out.contains("walrus roster every"), "live serves neither yet: {out}");
    }
    let all = rows(&s);
    let adds = |sid: &str| all.iter().find(|r| r["session"] == sid).map(|r| r["shadow"]["adds"].clone()).unwrap_or_default();
    assert!(adds("gap-1").as_array().is_some_and(|a| a.iter().any(|x| x == id("tide", COBALT).as_str())), "{:?}", adds("gap-1"));
    let walrus = id("garden", "File the walrus roster every Friday afternoon.");
    assert!(adds("new-1").as_array().is_some_and(|a| a.iter().any(|x| x == walrus.as_str())), "{:?}", adds("new-1"));

    // Two prompts in the log: every change serves on half of them, which BO-16's replay calls TOO BROAD.
    base_ok(&s, &["shadow", "promote", "--broad-ok"]);
    let review = base_ok(&s, &["rule", "review"]);
    assert!(!review.contains("p-0001") && !review.contains("p-0002"), "both approved: {review}");
    let promoted_domains = std::fs::read_to_string(s.ws.join(".base").join("domains.toml")).unwrap();
    assert!(promoted_domains.contains("cobalt manifest"), "{promoted_domains}");
    let (_, out, _) = run_prompt_submit(&s, gap_prompt, Some("gap-2"));
    assert!(out.contains("cobalt harbor charts"), "live serves the gap's rule now: {out}");

    base_ok(&s, &["shadow", "rollback"]);
    let review = base_ok(&s, &["rule", "review"]);
    assert!(review.contains("p-0001") && review.contains("p-0002"), "pending again: {review}");
    let rolled = std::fs::read_to_string(s.ws.join(".base").join("domains.toml")).unwrap();
    assert!(!rolled.contains("cobalt manifest"), "the keyword is gone again: {rolled}");
    let (code, list, err) = run_base(&s, &["rule", "list", "--domain", "garden"]);
    assert_eq!(code, 0, "{err}");
    assert!(!list.contains("walrus"), "the new rule is gone again: {list}");
    let _ = before_domains;
}

/// BO-20's three admission fixes each turn away a rule `[match] min_score` alone admits; at their defaults they turn
/// nothing away and the output is what it was.
#[test]
fn admission_fixes_narrow_min_score() {
    let admitted = |tag: &str, toml: &str, prompt: &str| -> String {
        let s = seeded(tag, toml);
        base_ok(&s, &["domain", "sync"]);
        let (code, out, err) = run_prompt_submit(&s, prompt, Some(&format!("{tag}-1")));
        assert_eq!(code, 0, "{err}");
        out
    };
    let plain = admitted("adm-plain", "[match]\nmin_score = 2.0\n", NEAR);
    assert!(plain.contains(PAYROLL), "control: min_score alone admits the near miss:\n{plain}");
    let defaults = admitted("adm-defaults", "[match]\nmin_score = 2.0\nmin_terms = 1\nprompt_idf = false\n", NEAR);
    assert_eq!(plain, defaults, "the three keys at their defaults change nothing");

    // min_terms: a prompt sharing two words with the rule's test prompt and no two-word term.
    let two = "payroll and the feed";
    let ok = admitted("adm-terms-1", "[match]\nmin_score = 0.5\n", two);
    assert!(ok.contains(PAYROLL), "control: two shared words reach a low min_score:\n{ok}");
    let narrowed = admitted("adm-terms-3", "[match]\nmin_score = 0.5\nmin_terms = 3\n", two);
    assert!(!narrowed.contains(PAYROLL), "min_terms 3 turns two words away:\n{narrowed}");

    // relative: the near miss's best rule is the payroll rule; nothing else may come in under 0.99 of it.
    let rel = admitted("adm-relative", "[match]\nmin_score = 2.0\nrelative = 1.01\n", NEAR);
    assert!(!rel.contains(PAYROLL), "no rule reaches 101% of the best score:\n{rel}");

    // prompt_idf: every word of the near miss is in most of the user's own prompts, so it weighs nearly nothing.
    let s = seeded("adm-idf", "[match]\nmin_score = 2.0\nprompt_idf = true\n");
    let mut log: Vec<Value> = Vec::new();
    for i in 0..40 {
        log.push(prompt_row(&at(-7200 + i), &format!("idf-{i:02}"), 1, &format!("{NEAR} {i}"), None));
    }
    append(&s, &log);
    base_ok(&s, &["domain", "sync"]);
    assert!(s.ws.join(".base").join("bm25-prompts.json").is_file(), "the counts are kept beside the index");
    let (code, out, err) = run_prompt_submit(&s, NEAR, Some("idf-x"));
    assert_eq!(code, 0, "{err}");
    assert!(!out.contains(PAYROLL), "prompt_idf turns away a near miss made of the user's common words:\n{out}");
}

/// `base rule update --protected` keeps the mark where the rule's tests are: a domains.toml entry, or a graph rule.
#[test]
fn rule_protected_stored_with_the_rule() {
    let s = seeded("protect-store", "");
    let quartz = base::domain::rule_test::short_ref("ledger", &id("ledger", QUARTZ));
    base_ok(&s, &["rule", "update", &quartz, "--protected"]);
    let toml = std::fs::read_to_string(s.ws.join(".base").join("domains.toml")).unwrap();
    assert!(toml.contains("protected = true"), "{toml}");
    base_ok(&s, &["rule", "update", &quartz, "--unprotected"]);
    let toml = std::fs::read_to_string(s.ws.join(".base").join("domains.toml")).unwrap();
    assert!(!toml.contains("protected"), "cleared, and the entry a plain string again:\n{toml}");
    assert!(toml.contains(&format!("\"{QUARTZ}\"")), "{toml}");

    let text = "Seal every crate with the amber tape before it ships.";
    base_ok(&s, &["rule", "add", "--domain", "garden", "--text", text]);
    let amber = base::domain::rule_test::short_ref("garden", &id("garden", text));
    base_ok(&s, &["rule", "update", &amber, "--protected"]);
    let graph = std::fs::read_to_string(s.ws.join(".base").join("graph.nq")).unwrap();
    assert!(graph.contains("#protected> \"true\""), "the graph rule carries the mark");
    let ns = base::config::NamespaceConfig::default();
    let ids = base::crud::rule::protected_ids(&s.ws, &ns);
    assert!(ids.contains(&id("garden", text)), "{ids:?}");
    base_ok(&s, &["rule", "update", &amber, "--unprotected"]);
    let graph = std::fs::read_to_string(s.ws.join(".base").join("graph.nq")).unwrap();
    assert!(!graph.contains("#protected>"), "cleared");
}

/// The seamless upgrade (Chris, 2026-10-03): with no shadow ever started, no hook writes a `shadow` field, session
/// start says nothing new, and nothing is written for the shadow.
#[test]
fn no_shadow_started_changes_nothing() {
    let s = seeded("seamless", "");
    with_books(&s);
    let (code, out, err) = run_session_start(&s, Some("plain-1"));
    assert_eq!(code, 0, "{err}");
    assert!(!out.contains("matcher"), "{out}");
    let (code, _, err) = run_prompt_submit(&s, "open the ledger", Some("plain-1"));
    assert_eq!(code, 0, "{err}");
    let file = s.ws.join("books").join("q3.md");
    let (code, _, err) = run_pre_tool_use(&s, "Read", json!({"file_path": file.display().to_string()}), "plain-1", &[]);
    assert_eq!(code, 0, "{err}");
    let all = rows(&s);
    assert!(all.iter().any(|r| r["event"] == "prompt") && all.iter().any(|r| r["event"] == "file"), "control: rows were written");
    assert!(all.iter().all(|r| r.get("shadow").is_none()), "no shadow field on any row");
    assert!(!shadow_dir(&s).exists(), "nothing written for a shadow nobody started");
    let report = base_ok(&s, &["shadow", "report"]);
    assert!(report.starts_with("no shadow is running"), "{report}");
    assert!(!shadow_dir(&s).exists(), "a report writes nothing");
}

/// A version name never leaves its folder.
#[test]
fn version_names_stay_in_their_folder() {
    assert!(base::shadow::safe_name("bm25-0003"));
    assert!(!base::shadow::safe_name("../state"));
    assert!(!base::shadow::safe_name("a/b"));
    assert!(!base::shadow::safe_name(""));
    let _ = Path::new("");
}
