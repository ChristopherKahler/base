//! BO-19 (K8, D6, F14c): `base doctor` lists the rules that never fire, fire everywhere, or get ignored, the decisions
//! to review, and the correction detector's record, all from the match log; `base rule stats` prints each rule's
//! numbers. Advice only: none of it changes doctor's verdict.
//!
//! Every name and text here is invented. The match log is written row by row, in the shape `emit::match_log` writes,
//! with times counted back from today, so the windows (30 days, 60 days) hold on any day the tests run.

use std::path::PathBuf;
use std::process::Command;

use base::usage::{self, Current, Key, Reading};
use chrono::{Duration, Local, SecondsFormat};
use serde_json::{json, Value};

const NS: &str = "http://ops-sys.local/ontology#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

const R_LINT: &str = "Name the lint target first.";
const R_HOOKLOG: &str = "Read the hook log first.";
const R_NEW: &str = "Pin the formatter version.";
const R_TRUCKS: &str = "Count the trucks first.";
const R_PLAIN: &str = "Answer in plain words.";
const R_MEMO: &str = "Date every memo.";

fn rid(domain: &str, text: &str) -> String {
    base::domain::rules::rule_id(domain, text)
}

fn short(domain: &str, text: &str) -> String {
    format!("{domain}.{}", &rid(domain, text)[..8])
}

/// A time `days` days and `minutes` minutes before now, as a row's `ts`.
fn ago(days: i64, minutes: i64) -> String {
    (Local::now() - Duration::days(days) - Duration::minutes(minutes)).to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn day_ago(days: i64) -> String {
    (Local::now() - Duration::days(days)).format("%Y-%m-%d").to_string()
}

/// The days a log covers when its oldest row is [`ago`]`(days, 0)`, counted as the reader does: on a day with a clock
/// change near midnight, `days` times 24 hours back can land a calendar day off.
fn covered_since(days: i64) -> u64 {
    let oldest = (Local::now() - Duration::days(days)).date_naive();
    (Local::now().date_naive() - oldest).num_days() as u64 + 1
}

/// A home and a workspace: domains.toml, the workspace graph, the global base.toml, and a match log built by
/// [`Log`].
struct Fixture {
    home: PathBuf,
    ws: PathBuf,
}

impl Fixture {
    fn new(tag: &str, base_toml: &str) -> Self {
        let root = std::env::temp_dir().join(format!("base-bo19-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let ws = root.join("usagews");
        std::fs::create_dir_all(home.join(".base-gbl").join(".base")).unwrap();
        std::fs::create_dir_all(ws.join(".base")).unwrap();
        std::fs::write(home.join(".base-gbl").join("base.toml"), base_toml).unwrap();
        std::fs::write(
            ws.join(".base").join("domains.toml"),
            format!(
                "[[domain]]\nname = \"tools\"\nmode = \"triggered\"\nprompt_keywords = [\"hook\", \"lint\"]\n\
                 rules = [\"{R_LINT}\", \"{R_HOOKLOG}\", \"{R_NEW}\"]\n\n\
                 [[domain]]\nname = \"fleet\"\nmode = \"triggered\"\nprompt_keywords = [\"depot\"]\nrules = [\"{R_TRUCKS}\"]\n\n\
                 [[domain]]\nname = \"notes\"\nmode = \"triggered\"\nprompt_keywords = [\"memo\"]\nrules = [\"{R_MEMO}\"]\n\n\
                 [[domain]]\nname = \"GLOBAL\"\nmode = \"always\"\nrules = [\"{R_PLAIN}\"]\n"
            ),
        )
        .unwrap();
        std::fs::write(ws.join(".base").join("graph.nq"), "").unwrap();
        Fixture { home, ws }
    }

    fn graph(&self, quads: &str) {
        std::fs::write(self.ws.join(".base").join("graph.nq"), quads).unwrap();
    }

    fn log(&self, log: &Log) {
        std::fs::write(self.ws.join(".base").join("match-log.jsonl"), log.text()).unwrap();
    }

    fn tune_log(&self, rows: &[Value]) {
        let dir = self.home.join(".base-gbl").join("corrections").join("tune");
        std::fs::create_dir_all(&dir).unwrap();
        let text: String = rows.iter().map(|r| format!("{r}\n")).collect();
        std::fs::write(dir.join("log.jsonl"), text).unwrap();
    }

    fn base(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_base"))
            .args(args)
            .current_dir(&self.ws)
            .env("BASE_HOME", &self.home)
            .env("BASE_NO_AUTO_UPDATE", "1")
            .env("BASE_AST_NO_SPAWN", "1")
            .env_remove("BASE_RELAY_AS")
            .env_remove("BASE_HEADLESS")
            .env_remove("WT_SESSION")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CLAUDECODE")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("base runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// `base doctor`'s rules-and-decisions section, from its heading to the next section.
    fn section(&self) -> (i32, String, String) {
        let (code, out, err) = self.base(&["doctor"]);
        let start = out.find("─── rules and decisions").unwrap_or_else(|| panic!("no section:\n{out}\n{err}"));
        let rest = &out[start..];
        let end = rest[3..].find("\n───").map(|i| i + 3).or_else(|| rest.find("\nVerdict")).unwrap_or(rest.len());
        (code, rest[..end].to_string(), out)
    }
}

/// A match log, row by row.
#[derive(Default)]
struct Log {
    rows: Vec<Value>,
}

fn items(list: &[(&str, &str)]) -> Vec<Value> {
    list.iter().map(|(kind, id)| json!({"id": id, "kind": kind})).collect()
}

impl Log {
    fn text(&self) -> String {
        self.rows.iter().map(|r| format!("{r}\n")).collect()
    }

    /// A prompt row: `matched` as (domain, by, value).
    #[allow(clippy::too_many_arguments)]
    fn prompt(
        &mut self,
        ts: String,
        session: &str,
        n: u32,
        text: &str,
        matched: &[(&str, &str, Option<&str>)],
        served: &[(&str, &str)],
        cut: &[(&str, &str, &str, &str)],
    ) {
        let matched: Vec<Value> =
            matched.iter().map(|(d, by, v)| json!({"domain": d, "by": by, "value": v})).collect();
        let cut: Vec<Value> =
            cut.iter().map(|(kind, id, reason, block)| json!({"id": id, "kind": kind, "reason": reason, "block": block, "limit": "prompt_bytes"})).collect();
        self.rows.push(json!({
            "ts": ts, "session": session, "event": "prompt", "prompt_num": n, "text": text,
            "matched": matched, "served": items(served), "cut": cut, "scores": [],
        }));
    }

    fn file(&mut self, ts: String, session: &str, served: &[(&str, &str)]) {
        self.rows.push(json!({
            "ts": ts, "session": session, "event": "file", "tool": "Edit", "path": "C:/work/tools/lint.toml",
            "matched": [], "served": items(served), "cut": [], "scores": [],
        }));
    }

    fn signal(&mut self, ts: String, session: &str, n: u32, signals: &[(&str, &str)]) {
        let signals: Vec<Value> = signals.iter().map(|(layer, kind)| json!({"layer": layer, "kind": kind})).collect();
        self.rows.push(json!({
            "ts": ts, "session": session, "event": "signal", "prompt_num": n,
            "matched": [], "served": [], "cut": [], "scores": [], "signals": signals,
        }));
    }
}

fn quads_decision(slug: &str, name: &str, created: Option<&str>, updated: Option<&str>, superseded: bool) -> String {
    let g = format!("{NS}graph/ws/usagews");
    let s = format!("{NS}decision/{slug}");
    let mut out = format!("<{s}> <{RDF_TYPE}> <{NS}Decision> <{g}> .\n<{s}> <{NS}name> \"{name}\" <{g}> .\n");
    out.push_str(&format!("<{s}> <{NS}rationale> \"invented for a test\" <{g}> .\n"));
    if let Some(c) = created {
        out.push_str(&format!("<{s}> <{NS}createdAt> \"{c}\"^^<{XSD_DATETIME}> <{g}> .\n"));
    }
    if let Some(u) = updated {
        out.push_str(&format!("<{s}> <{NS}updatedAt> \"{u}\"^^<{XSD_DATETIME}> <{g}> .\n"));
    }
    if superseded {
        out.push_str(&format!("<{s}> <{NS}supersededBy> <{NS}decision/{slug}-v2> <{g}> .\n"));
        out.push_str(&format!("<{s}> <{NS}status> \"superseded\" <{g}> .\n"));
    }
    out
}

fn lines_of(section: &str) -> Vec<&str> {
    section.lines().collect()
}

// ─── K8a ─────────────────────────────────────────────────────────────────────

/// K8a's log: rules a to h and one decision, served, cut, scored and corrected as `usage_counts_from_match_log` says.
fn k8a_log() -> Log {
    let (a, b, c, d, e, f) = ("rule-a", "rule-b", "rule-c", "rule-d", "rule-e", "rule-f");
    let mut log = Log::default();
    // rule-a: served 40 days ago (outside the window), then twice inside it. Cut and scored on other prompts: neither
    // counts.
    log.prompt(ago(40, 0), "s1", 1, "set up the lint step", &[], &[("rule", a)], &[]);
    log.prompt(ago(2, 0), "s2", 1, "lint the parser", &[], &[("rule", a)], &[("rule", b, "budget", "tools-rules"), ("rule", b, "topic limit", "tools-topic-rules")]);
    log.prompt(ago(1, 0), "s3", 1, "lint the parser again", &[], &[("rule", a)], &[]);
    let mut scored = log.rows.last().unwrap().clone();
    scored["prompt_num"] = json!(2);
    scored["served"] = json!([]);
    scored["scores"] = json!([{"id": b, "domain": "tools", "score": 9.5, "by": "bm25"}, {"not": "a score at all"}]);
    log.rows.push(scored);
    // s4. Each signal is about one reply: C1, the repeat and a C3 marker logged on prompt N answer reply N - 1; an
    // interrupt or a refusal logged on N is about reply N. A rule served on N counts a signal about reply N or N + 1.
    // rule-b on 3, C1 on 4 (about reply 3): corrected. rule-c on 5, C2 interrupt on 5: corrected. rule-d on 7, C3
    // UPDATED on 8 (about 7): corrected. rule-e on 9: MISREAD on 10 and DEFERRED on 9: not. rule-f on 11, C1 on 14
    // (about 13, two replies on): not. rule-g on 20 with C1 and C3 on 20 itself (about reply 19, before rule-g was
    // seen): not. rule-h on 22, a refusal on 23 (the next reply): corrected.
    log.prompt(ago(1, 50), "s4", 3, "tidy the hook", &[], &[("rule", b)], &[]);
    log.prompt(ago(1, 49), "s4", 4, "no, not that file", &[], &[], &[]);
    log.signal(ago(1, 49), "s4", 4, &[("C1", "phrase")]);
    log.prompt(ago(1, 48), "s4", 5, "rename the hook", &[], &[("rule", c)], &[]);
    log.signal(ago(1, 47), "s4", 5, &[("C2", "interrupt")]);
    log.prompt(ago(1, 46), "s4", 7, "move the lint config", &[], &[("rule", d)], &[]);
    log.signal(ago(1, 45), "s4", 8, &[("C3", "UPDATED")]);
    log.prompt(ago(1, 44), "s4", 9, "explain the lint rule", &[], &[("rule", e)], &[]);
    log.signal(ago(1, 43), "s4", 9, &[("C3", "DEFERRED")]);
    log.signal(ago(1, 43), "s4", 10, &[("C3", "MISREAD")]);
    log.prompt(ago(1, 42), "s4", 11, "list the hooks", &[], &[("rule", f)], &[]);
    log.signal(ago(1, 41), "s4", 14, &[("C1", "phrase")]);
    // A file row after prompt 14 serves rule-f; C1 on 15 (about reply 14): it joins prompt 14, so it is corrected.
    log.prompt(ago(1, 40), "s4", 14, "edit the lint config", &[], &[], &[]);
    log.file(ago(1, 39), "s4", &[("rule", f)]);
    log.signal(ago(1, 38), "s4", 15, &[("C1", "phrase")]);
    log.prompt(ago(1, 30), "s4", 20, "no, the lint rule again", &[], &[("rule", "rule-g")], &[]);
    log.signal(ago(1, 30), "s4", 20, &[("C1", "phrase"), ("C3", "UPDATED")]);
    log.prompt(ago(1, 28), "s4", 22, "run the hook", &[], &[("rule", "rule-h")], &[]);
    log.signal(ago(1, 27), "s4", 23, &[("C2", "denial")]);
    // A decision served twice, the first time before its update.
    log.prompt(ago(3, 0), "s5", 1, "where do memos go", &[], &[("decision", "global.memo-folder")], &[]);
    log.signal(ago(3, 0), "s5", 2, &[("C1", "phrase")]);
    log.prompt(ago(1, 0), "s6", 1, "where do memos go now", &[], &[("decision", "global.memo-folder")], &[]);
    log
}

/// K8a on a fixture log: served in the window and in all, printed only; last served; corrected after in the same or
/// the next turn, by C1, C2, C3 UPDATED or CORRECTED, and not by MISREAD, DEFERRED or a signal two turns on; a file
/// row's serving joins the session's latest prompt.
#[test]
fn usage_counts_from_match_log() {
    let dir = std::env::temp_dir().join(format!("base-bo19-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b, c, d, e, f) = ("rule-a", "rule-b", "rule-c", "rule-d", "rule-e", "rule-f");
    let log = k8a_log();
    std::fs::write(dir.join("match-log.jsonl"), log.text()).unwrap();

    let today = Local::now().date_naive();
    let scan = usage::scan(std::slice::from_ref(&dir), today, 30, &Current::default());
    let k = |id: &str| Key::Rule(id.to_string());

    let ca = scan.counts(&k(a));
    assert_eq!((ca.served_window, ca.served_all), (2, 3), "served 30d and all, printed only");
    assert_eq!(ca.last_served.map(|d| d.format("%Y-%m-%d").to_string()), Some(day_ago(1)));
    assert_eq!(scan.counts(&k(b)).served_all, 1, "the cut and the score of rule-b do not count as served");
    assert_eq!(scan.counts(&k(b)).withheld_window, 1, "its cut for the budget is withheld; its topic-limit cut is not the budget");
    assert_eq!(scan.counts(&k(b)).withheld_from.as_deref(), Some("tools-rules"), "the block it was withheld from");
    assert_eq!(scan.counts(&k(b)).corrected_after, 1, "C1 on the next turn");
    assert_eq!(scan.counts(&k(c)).corrected_after, 1, "C2 on the same turn");
    assert_eq!(scan.counts(&k(d)).corrected_after, 1, "C3 UPDATED on the next turn");
    assert_eq!(scan.counts(&k(e)).corrected_after, 0, "MISREAD and DEFERRED are not corrections of a rule");
    let cf = scan.counts(&k(f));
    assert_eq!(cf.served_all, 2, "a prompt row and a file row");
    assert_eq!(cf.corrected_after, 1, "the file row joins prompt 14 and C1 on 15 is about reply 14; C1 on 14 is two replies after 11");
    assert_eq!(scan.counts(&k("rule-g")).corrected_after, 0, "a phrase on the prompt that brought the rule in is about the reply before it");
    assert_eq!(scan.counts(&k("rule-h")).corrected_after, 1, "a refusal in the next turn");
    assert_eq!(scan.typed, 14, "typed prompts in the window: the one 40 days ago is outside it, and the row whose scores hold a malformed entry still counts (scores are not built)");
    assert_eq!(scan.days_covered(), covered_since(40));

    let dk = Key::Decision("global.memo-folder".into());
    assert_eq!(scan.counts(&dk).corrected_after, 1);
    let since = (Local::now() - Duration::days(2)).timestamp();
    let after = scan.counts_since(&dk, Some(since));
    assert_eq!((after.served_all, after.corrected_after), (1, 0), "a decision's update starts it again");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The reader cuts each file into blocks of whole lines and each block into one piece per thread, then applies the rows
/// in file order: the counts are the same at any thread count and block size, over an archive file, the live file, a
/// line longer than a block, lines that are blank or not UTF-8, and a second tier.
#[test]
fn usage_scan_is_the_same_at_any_thread_count() {
    let root = std::env::temp_dir().join(format!("base-bo19-threads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (own, global) = (root.join("ws"), root.join("gbl"));
    std::fs::create_dir_all(own.join("match-log")).unwrap();
    std::fs::create_dir_all(&global).unwrap();

    let mut log = k8a_log();
    log.prompt(ago(2, 5), "s7", 1, "the lint memo", &[("tools", "keyword", Some("lint")), ("tools", "path", Some("C:/work/tools"))], &[("rule", "rule-a")], &[]);
    log.prompt(ago(2, 4), "s7", 2, "<task-notification>done</task-notification>", &[], &[], &[]);
    let mut textless = log.rows.last().unwrap().clone();
    textless.as_object_mut().unwrap().remove("text");
    textless["prompt_num"] = json!(3);
    log.rows.push(textless);
    log.prompt(ago(2, 3), "s7", 4, &"a long prompt about the depot ".repeat(200), &[], &[("rule", "rule-b")], &[]);
    let text = log.text();
    let lines: Vec<&str> = text.lines().collect();
    let (older, newer) = lines.split_at(lines.len() / 2);
    let quiet = json!({"ts": ago(45, 0), "session": "s9", "event": "file", "tool": "Read", "path": "C:/work/a.txt",
        "matched": [], "served": [], "cut": [], "scores": []});
    std::fs::write(own.join("match-log").join("2026-01-01T23-59-59.jsonl"), format!("{quiet}\n{}\n", older.join("\n"))).unwrap();
    let mut live = newer.join("\n").into_bytes();
    live.extend_from_slice(b"\n\n\xff\xfe not text\n\n");
    std::fs::write(own.join("match-log.jsonl"), live).unwrap();
    let mut other = Log::default();
    other.prompt(ago(1, 1), "s1", 9, "lint it from home", &[], &[("rule", "rule-a")], &[]);
    std::fs::write(global.join("match-log.jsonl"), other.text()).unwrap();

    let current = Current {
        keywords: [("tools".to_string(), vec!["hook".to_string(), "lint".to_string()])].into(),
        aliases: Default::default(),
        decisions: vec![("global.memo-folder".to_string(), vec!["memo".to_string()])],
    };
    let dirs = [own, global];
    let today = Local::now().date_naive();
    let one = usage::scan_with(&dirs, today, 30, &current, Reading { threads: 1, block: Reading::BLOCK, piece: Reading::PIECE });
    assert_eq!((one.typed, one.machine, one.textless), (17, 1, 1), "K8a's 14, two more typed here and one in the other tier");
    assert_eq!(one.days_covered(), covered_since(45), "the quiet row 45 days ago in the archive file");
    assert_eq!(one.counts(&Key::Rule("rule-a".into())).served_all, 5);
    assert_eq!(one.counts(&Key::Rule("rule-b".into())).served_all, 2, "K8a's one and the long line's");
    assert_eq!(one.keyword_prompts.get("tools"), Some(&1));
    assert!(one.decision_reach.get("global.memo-folder").is_some_and(|n| *n >= 1));
    // A piece of 1 byte splits even these small files among the threads; the default 1 MB reads them on one.
    for (threads, block, piece) in [(8, Reading::BLOCK, 1), (3, 4096, 1), (2, 97, 1), (5, 1, 1), (8, Reading::BLOCK, Reading::PIECE)] {
        let many = usage::scan_with(&dirs, today, 30, &current, Reading { threads, block, piece });
        assert_eq!(many, one, "{threads} threads, blocks of {block} bytes, pieces of {piece} or more");
    }
    let _ = std::fs::remove_dir_all(&root);
}

// ─── K8b ─────────────────────────────────────────────────────────────────────

/// The fixture both list tests and the verdict test use: a log of 36 days whose section lists one of each.
fn listing_fixture(tag: &str, base_toml: &str) -> Fixture {
    let fx = Fixture::new(tag, base_toml);
    let (lint, hooklog, plain, memo) = (rid("tools", R_LINT), rid("tools", R_HOOKLOG), rid("GLOBAL", R_PLAIN), rid("notes", R_MEMO));
    let mut log = Log::default();
    // 35 days ago: the hook-log rule served, and never again.
    log.prompt(ago(35, 0), "old", 1, "why did the hook fail", &[("tools", "keyword", Some("hook"))], &[("rule", &hooklog)], &[]);
    // 117 typed prompts in the window, and m1 to m3 below: 120. Keyword matches: tools on 30 (25%, at the limit, not
    // over), notes on 28 + 3 by `memo` (25.8%, over) and 10 more by `minutes`, a keyword notes no longer has. Path
    // matches of tools on 47 do not count.
    for i in 0..117u32 {
        let ts = ago(1 + i64::from(i % 9), i64::from(i));
        let session = format!("s{:03}", i / 4);
        let n = i % 4 + 1;
        let mut matched: Vec<(&str, &str, Option<&str>)> = vec![("GLOBAL", "always", None)];
        if i < 30 {
            matched.push(("tools", "keyword", Some("lint")));
        }
        if (30..58).contains(&i) {
            matched.push(("notes", "keyword", Some("memo")));
        }
        if (58..68).contains(&i) {
            matched.push(("notes", "keyword", Some("minutes")));
        }
        if (70..117).contains(&i) {
            matched.push(("tools", "path", Some("C:/work/tools")));
        }
        let served: Vec<(&str, &str)> = if i < 30 && n == 1 { vec![("rule", lint.as_str())] } else { vec![] };
        let cut: Vec<(&str, &str, &str, &str)> = if i % 30 == 0 { vec![("rule", plain.as_str(), "budget", "global-rules")] } else { vec![] };
        let text = format!("typed prompt number {i} about the work");
        log.prompt(ts, &session, n, &text, &matched, &served, &cut);
    }
    // 50 task notifications that match tools by keyword: not prompts a person typed, so outside the share.
    for i in 0..50 {
        log.prompt(ago(1, i), "bg", 50 + i as u32, "<task-notification>lint finished</task-notification>", &[("tools", "keyword", Some("lint"))], &[], &[]);
    }
    // The lint rule corrected after 3 of its 8 servings (s000, s001, s002 at prompt 1; C1 on prompt 2); the memo rule
    // after 3 of its 3. A decision served 8 times on file touches, never corrected. The log's average: 6 of 20 servings
    // (lint 8, memo 3, the hook-log rule 1, the decision 8), 30%, so an ignored rule needs 60%: memo is, lint is not.
    for s in ["s000", "s001", "s002"] {
        log.signal(ago(1, 1), s, 2, &[("C1", "phrase")]);
    }
    for s in ["m1", "m2", "m3"] {
        log.prompt(ago(2, 5), s, 1, "date the memo", &[("notes", "keyword", Some("memo"))], &[("rule", &memo)], &[]);
        log.signal(ago(2, 4), s, 2, &[("C1", "phrase")]);
    }
    for i in 0..8 {
        log.file(ago(3, i), &format!("f{i}"), &[("decision", "global.plain-decision")]);
    }
    let mut rows = log.rows;
    rows.sort_by(|x, y| {
        let t = |v: &Value| chrono::DateTime::parse_from_rfc3339(v["ts"].as_str().unwrap()).unwrap();
        t(x).cmp(&t(y))
    });
    fx.log(&Log { rows });
    fx
}

/// Each kind at its threshold: dead (a domain never matched, a rule last served before the window, a rule only ever
/// withheld, a new rule counted not listed), noisy (over the share, not at it; current keywords only; no path matches,
/// no task notifications), ignored (3 corrections at twice the log average listed; 3 corrections under it not).
#[test]
fn doctor_lists_dead_noisy_ignored() {
    let fx = listing_fixture("lists", "");
    let (_, section, out) = fx.section();
    let l = lines_of(&section);
    let has = |want: &str| assert!(section.contains(want), "missing {want:?} in:\n{section}\n\nfull:\n{out}");
    let hasnt = |want: &str| assert!(!section.contains(want), "unexpected {want:?} in:\n{section}");

    has("   match log: 36 days (since ");
    has(" · 120 typed prompts, 50 task notifications · 0 with no text");
    // Dead: fleet's 1 rule, the hook-log rule, GLOBAL's rule; the new tools rule counted, not listed.
    has("   dead (not served in 30 days): 3 · 1 more first seen under 30 days ago");
    has("     domain fleet · not matched in 30 days · its 1 rule not served · check its triggers: base rule test --domain fleet");
    has(&format!(
        "     {} \"Read the hook log first.\"   0 served · last {} · check its triggers: base rule test --domain tools",
        short("tools", R_HOOKLOG),
        day_ago(35)
    ));
    has(&format!(
        "     {} \"Answer in plain words.\"   0 served · never served in the log · withheld 4 times by the budget: base hooks show global-rules",
        short("GLOBAL", R_PLAIN)
    ));
    let dead_block = &section[section.find("   dead").unwrap()..section.find("   noisy").unwrap()];
    for served_or_new in [short("tools", R_NEW), short("tools", R_LINT), short("notes", R_MEMO)] {
        assert!(!dead_block.contains(&served_or_new), "{served_or_new} is not dead:\n{dead_block}");
    }
    // Noisy: notes over the share on `memo` only; tools at 25% exactly is not over it.
    has("   noisy (matched by keyword on more than 25% of typed prompts, last 30 days): 1");
    has("     domain notes · 26% (31 of 120) · by keyword memo 26% · its 1 rule · narrow its keywords: base rule replay --domain notes --drop-keyword memo");
    hasnt("domain tools ·");
    hasnt("minutes");
    // Ignored: the memo rule, corrected after 3 of 3 servings; the lint rule's 3 of 8 (38%) is under twice the log's 30%.
    has("   ignored (corrected after 3+ times, and after at least twice the log average of 30% of servings): 1");
    has(&format!(
        "     {} \"Date every memo.\"   served 3 · corrected after 3 (100%, log average 30%) · reword it: base rule propose --rule {} --text \"...\"",
        short("notes", R_MEMO),
        short("notes", R_MEMO)
    ));
    let ignored_block = &section[section.find("   ignored").unwrap()..section.find("   review").unwrap()];
    assert!(!ignored_block.contains(&short("tools", R_LINT)), "3 of 8 is under twice the average:\n{ignored_block}");
    assert!(l.iter().any(|x| x.starts_with("   correction detector")), "{section}");
}

// ─── F14c ────────────────────────────────────────────────────────────────────

/// F14c: a decision served 20 times and unchanged 60 days is listed; 59 days, 19 servings, an update 10 days ago, or a
/// supersession keep one off the list; one with no date says so.
#[test]
fn doctor_lists_decisions_to_review() {
    let fx = Fixture::new("review", "");
    let mut quads = String::new();
    quads.push_str(&quads_decision("global.keep-notes-short", "Keep notes short", Some(&ago(61, 0)), None, false));
    quads.push_str(&quads_decision("global.young-rule", "A young decision", Some(&ago(59, 0)), None, false));
    quads.push_str(&quads_decision("global.updated-rule", "An updated decision", Some(&ago(100, 0)), Some(&ago(10, 0)), false));
    quads.push_str(&quads_decision("global.undated-rule", "A decision with no date", None, None, false));
    quads.push_str(&quads_decision("global.rare-rule", "A decision served 19 times", Some(&ago(100, 0)), None, false));
    quads.push_str(&quads_decision("global.replaced-rule", "A replaced decision", Some(&ago(100, 0)), None, true));
    fx.graph(&quads);
    let mut log = Log::default();
    for i in 0..25i64 {
        let mut served: Vec<(&str, &str)> =
            vec![("decision", "global.keep-notes-short"), ("decision", "global.young-rule"), ("decision", "global.updated-rule")];
        if i < 20 {
            served.push(("decision", "global.undated-rule"));
            served.push(("decision", "global.replaced-rule"));
        }
        if i < 19 {
            served.push(("decision", "global.rare-rule"));
        }
        log.prompt(ago(20 - i % 20, i), &format!("r{i:02}"), 1, "what do we keep in notes", &[], &served, &[]);
    }
    fx.log(&log);
    let (_, section, _) = fx.section();
    let has = |want: &str| assert!(section.contains(want), "missing {want:?} in:\n{section}");
    has("   review (served 20+ times, unchanged 60+ days): 2");
    has("     global.keep-notes-short \"Keep notes short\"   served 25 · unchanged 61 days · still true? base decision update global.keep-notes-short --name \"...\" · or replace it: base decision log --domain global --decision \"...\" --rationale \"...\" --supersedes global.keep-notes-short");
    has("     global.undated-rule \"A decision with no date\"   served 20 · no date on record · still true?");
    for absent in ["global.young-rule", "global.updated-rule", "global.rare-rule", "global.replaced-rule"] {
        assert!(!section.contains(absent), "{absent} listed:\n{section}");
    }
}

// ─── K8c ─────────────────────────────────────────────────────────────────────

/// K8c: the detector's record over the window; a pass answered wholly from the judge's cache is not counted twice
/// (lynx's G0 ruling on question 6); the line's three shapes.
#[test]
fn detector_health_line() {
    let dir = std::env::temp_dir().join(format!("base-bo19-detector-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let record = json!({"judged": 120, "flagged": 45, "corrections": 26, "misses": 11,
                        "hits": {"C1": 12, "C2": 2, "C3": 5}, "false_flags": {"C1": 20, "C2": 4, "C3": 6}});
    let old = json!({"ts": ago(40, 0), "sessions": 3, "calls": 4, "cached": 0, "detector": record});
    let pass = json!({"ts": ago(2, 0), "sessions": 5, "calls": 6, "cached": 0, "detector": record});
    let rerun = json!({"ts": ago(2, -1), "sessions": 5, "calls": 0, "cached": 6, "detector": record});
    // A first run stopped before it logged leaves only its rerun's record, answered from the cache: that one counts.
    let lone = json!({"ts": ago(1, 0), "sessions": 2, "calls": 0, "cached": 3,
                      "detector": {"judged": 10, "flagged": 4, "corrections": 4, "misses": 1, "hits": {"C1": 3}, "false_flags": {}}});
    let path = dir.join("log.jsonl");
    std::fs::write(&path, format!("{old}\n{pass}\n{rerun}\n{lone}\n")).unwrap();

    let since = (Local::now() - Duration::days(29)).date_naive();
    let d = base::corrections::tune_pass::detector_totals_in(&path, Some(since));
    assert_eq!((d.judged, d.corrections, d.misses), (130, 30, 12), "the old pass is outside the window, the rerun is not counted twice, the lone cached pass is");
    let all = base::corrections::tune_pass::detector_totals_in(&path, None);
    assert_eq!((all.corrections, all.misses), (56, 23), "with no window both full passes count, the rerun still does not");

    let line = |judged, corrections, misses| {
        let s = usage::Section {
            log: Some(usage::LogSpan { days: 1, since: today(), typed: 0, machine: 0, textless: 0 }),
            detector: usage::DetectorLine { judged, corrections, misses },
            ..usage::Section::default()
        };
        usage::render(&s).lines().find(|l| l.starts_with("   correction detector")).unwrap().to_string()
    };
    assert_eq!(line(120, 26, 11), "   correction detector, last 30 days: 15 caught · 11 missed (found by the backstop) · 58% caught");
    assert_eq!(line(40, 0, 0), "   correction detector, last 30 days: 40 turns judged, no correction found");
    assert_eq!(line(0, 0, 0), "   correction detector: no rule pass in the last 30 days (base tune runs one)");

    // Through doctor, from the tune log in the home.
    let fx = Fixture::new("detector", "");
    let mut log = Log::default();
    log.prompt(ago(0, 1), "s", 1, "a prompt", &[], &[], &[]);
    fx.log(&log);
    fx.tune_log(&[pass, rerun]);
    let (_, section, _) = fx.section();
    assert!(section.contains("   correction detector, last 30 days: 15 caught · 11 missed (found by the backstop) · 58% caught"), "{section}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ─── K8d ─────────────────────────────────────────────────────────────────────

/// Example 2: `base rule stats --domain tools`, its table and its `--json`.
#[test]
fn rule_stats_output() {
    let fx = listing_fixture("stats", "");
    let (code, out, err) = fx.base(&["rule", "stats", "--domain", "tools"]);
    assert_eq!(code, 0, "{out}{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], format!("from the match log: 36 days (since {})", day_ago(35)));
    let (lint, hooklog, new) = (short("tools", R_LINT), short("tools", R_HOOKLOG), short("tools", R_NEW));
    let w = lint.len();
    assert_eq!(lines[1], format!("| {:<w$} | served 30d | served all | corrected after | last served |", "rule"));
    let row = |rule: &str, a: usize, b: usize, c: usize, last: &str| format!("| {rule} | {a:<10} | {b:<10} | {c:<15} | {last:<11} |");
    assert_eq!(lines[2], row(&lint, 8, 8, 3, &day_ago(1)));
    assert_eq!(lines[3], row(&hooklog, 0, 1, 0, &day_ago(35)));
    assert_eq!(lines[4], row(&new, 0, 0, 0, "-"));
    assert_eq!(lines.len(), 5, "only the tools domain's three rules:\n{out}");

    let (code, out, _) = fx.base(&["rule", "stats", "--json"]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["window_days"], 30);
    assert_eq!(v["log"]["days"], 36);
    let rules = v["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 6, "every rule of every domain");
    let row = rules.iter().find(|r| r["rule"] == lint.as_str()).unwrap();
    assert_eq!((row["served_window"].as_u64(), row["served_all"].as_u64(), row["corrected_after"].as_u64()), (Some(8), Some(8), Some(3)));
    assert_eq!(row["id"], rid("tools", R_LINT));
}

// ─── Config ──────────────────────────────────────────────────────────────────

/// Non-default thresholds are honoured, and a base.toml without the keys reads as the defaults.
#[test]
fn thresholds_from_config() {
    let toml = "[doctor]\ndead_days = 40\nignored_after = 4\nreview_served = 3\nreview_days = 5\n\n[tune]\nbroad_share = 0.2\n";
    let fx = listing_fixture("thresholds", toml);
    fx.graph(&quads_decision("global.keep-notes-short", "Keep notes short", Some(&ago(6, 0)), None, false));
    let mut log: Vec<Value> = std::fs::read_to_string(fx.ws.join(".base").join("match-log.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut extra = Log::default();
    for i in 0..3 {
        extra.prompt(ago(0, i), &format!("d{i}"), 1, "keep notes short?", &[], &[("decision", "global.keep-notes-short")], &[]);
    }
    log.extend(extra.rows);
    fx.log(&Log { rows: log });
    let (_, section, _) = fx.section();
    let has = |want: &str| assert!(section.contains(want), "missing {want:?} in:\n{section}");
    // dead_days 40: the 36-day log is too young for it.
    has("   dead (not served in 40 days): not judged yet · the match log covers 36 days, fewer than [doctor] dead_days = 40");
    // broad_share 0.2: tools at 25% is over it now.
    has("   noisy (matched by keyword on more than 20% of typed prompts, last 40 days): 2");
    // A 40-day window also holds the prompt 35 days ago (tools by `hook`) and the 3 decision prompts below.
    has("     domain tools · 25% (31 of 124) · by keyword lint 24%, hook 1% ·");
    // ignored_after 4: the memo rule's 3 corrections are under it now.
    has("   ignored (corrected after 4+ times, and after at least twice the log average of 26% of servings): 0");
    // review_served 3, review_days 5.
    has("   review (served 3+ times, unchanged 5+ days): 1");
    has("     global.keep-notes-short \"Keep notes short\"   served 3 · unchanged 6 days ·");

    // [log] prompt_text = "matched": a row keeps only matched words, so noisy is not judged and the first line says why.
    let matched = Fixture::new("matched", "[log]\nprompt_text = \"matched\"\n");
    let mut log = Log::default();
    for i in 0..120 {
        log.prompt(ago(1, i), &format!("m{i}"), 1, "lint", &[("tools", "keyword", Some("lint"))], &[], &[]);
    }
    matched.log(&log);
    let (_, section, _) = matched.section();
    let lines = lines_of(&section);
    assert_eq!(lines[1], format!("   match log: 2 days (since {}) · 120 prompts · [log] prompt_text = \"matched\"", day_ago(1)), "{section}");
    assert_eq!(lines[3], "   noisy: not judged · [log] prompt_text = \"matched\" keeps too little of a prompt to tell yours from a task notification", "{section}");

    // No [doctor] keys: the defaults.
    let defaults = Fixture::new("defaults", "[doctor]\nstale_next_days = 14\n");
    let cfg = base::config::BaseConfig::load(&defaults.ws);
    assert_eq!(
        (cfg.doctor.dead_days, cfg.doctor.ignored_after, cfg.doctor.review_served, cfg.doctor.review_days, cfg.doctor.stale_next_days),
        (30, 3, 20, 60, 14)
    );
}

// ─── The young log, and the verdict ──────────────────────────────────────────

/// The exact lines of G0's drawing: no match log at all, and a log of one day (61 typed prompts, 646 task
/// notifications, as the operator's own log was on its first day).
#[test]
fn doctor_usage_young_log() {
    let fx = Fixture::new("young", "");
    // The global tier's log is 40 days old; the workspace's own log, which says how old the log is, has no row yet.
    let mut global = Log::default();
    global.file(ago(40, 0), "elsewhere", &[("rule", "rule-elsewhere")]);
    std::fs::write(fx.home.join(".base-gbl").join(".base").join("match-log.jsonl"), global.text()).unwrap();
    let (_, section, _) = fx.section();
    assert_eq!(
        section.trim_end(),
        "─── rules and decisions, from the match log ───\n   no match log yet: base writes it from the next prompt on, and this section fills in as it grows"
    );

    let mut log = Log::default();
    for i in 0..61 {
        log.prompt(ago(0, 0), &format!("y{i}"), 1, "a typed prompt", &[("GLOBAL", "always", None)], &[], &[]);
    }
    for i in 0..646 {
        log.prompt(ago(0, 0), "bg", i + 2, "<task-notification>done</task-notification>", &[], &[], &[]);
    }
    fx.log(&log);
    let (_, section, _) = fx.section();
    let lines = lines_of(&section);
    assert_eq!(lines[0], "─── rules and decisions, from the match log ───");
    assert_eq!(lines[1], format!("   match log: 1 day (since {}) · 61 typed prompts, 646 task notifications · 0 with no text", today()));
    assert_eq!(lines[2], "   dead (not served in 30 days): not judged yet · the match log covers 1 day, fewer than [doctor] dead_days = 30");
    assert_eq!(lines[3], "   noisy: not judged yet · 61 typed prompts in the last 30 days, fewer than 100");
    assert_eq!(lines[4], "   ignored (corrected after 3+ times, and after at least twice the log average of 0% of servings): 0");
}

/// Advice only: a store whose section lists dead, noisy, ignored and review ends HEALTHY, exit 0, as the same store
/// with no match log does.
#[test]
fn doctor_usage_never_changes_the_verdict() {
    let fx = listing_fixture("verdict", "[doctor]\nreview_served = 1\nreview_days = 5\n");
    fx.graph(&quads_decision("global.keep-notes-short", "Keep notes short", Some(&ago(6, 0)), None, false));
    let mut rows: Vec<Value> = std::fs::read_to_string(fx.ws.join(".base").join("match-log.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut extra = Log::default();
    extra.prompt(ago(0, 0), "v", 1, "keep notes short?", &[], &[("decision", "global.keep-notes-short")], &[]);
    rows.extend(extra.rows);
    fx.log(&Log { rows });

    let (code, section, out) = fx.section();
    for listed in ["   dead (not served in 30 days): 3", "   noisy (matched by keyword", "   ignored (corrected after 3+ times, and after at least twice the log average of 29% of servings): 1", "   review (served 1+ times, unchanged 5+ days): 1"] {
        assert!(section.contains(listed), "missing {listed:?}:\n{section}");
    }
    assert!(out.trim_end().ends_with("Verdict: HEALTHY ✓"), "{out}");
    assert_eq!(code, 0, "{out}");
    let (code, json, _) = fx.base(&["doctor", "--json"]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["healthy"], true);
    assert_eq!(v["usage"]["ignored"].as_array().map(Vec::len), Some(1));

    // The control: the same store with no log.
    std::fs::remove_file(fx.ws.join(".base").join("match-log.jsonl")).unwrap();
    let (code, out, _) = fx.base(&["doctor"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.trim_end().ends_with("Verdict: HEALTHY ✓"), "{out}");
}

