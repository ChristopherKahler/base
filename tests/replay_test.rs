//! Replay tests (BO-00 G4): a committed corpus run through the session-start and prompt-submit
//! hooks, the way Claude Code drives them, with a temporary home and a fixture config.
//!
//! WHY THIS FILE EXISTS. In the 0.16.0 round every test was green while 59% of the prompt hook's
//! context was lost on the operator's machine, and FS1 passed on its fixture while 91 of 228 real
//! session starts overflowed the first screen. Tests on made-up shapes passed while real behaviour
//! was broken. The corpus here is modeled on real shapes, and the failing one is in it, scrubbed:
//! `real-5-due` is the session start that measured 2,871 UTF-16 units on 2026-10-01.
//!
//! THE CORPUS (tests/fixtures/replay/): `prompts.txt` (one prompt per line), `session-start.txt`
//! (store shapes, one `[case]` each), `domains.toml` (the rules the prompts can match) and
//! `base.toml` (the config, appended to the seed's global one). Synthetic or scrubbed, because this
//! repository is public; `replay_corpus_has_no_real_names` holds that.
//!
//! ADDING A CHECK (build rule 13). Every later build order adds the "after" it proves here as a
//! `replay_*` test over `session_starts()` or `prompt_runs()`, so a later order cannot undo it. A
//! check that is not true yet is not added, and never added as ignored. Changing a check or the
//! corpus needs its reason in the PR body (build rule 14).

mod seed;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use seed::{run_base, run_base_in_session, run_pre_tool_use, run_pre_tool_use_at, run_prompt_submit, run_session_start, units};

/// FS1's bar (tests/deferral_test.rs), which `base.toml` sets as the first-screen limit.
const BAR: usize = 1990;
/// The budgets `base.toml` sets: the shipped defaults, measured on Claude Code 2.1.287 (BO-02).
const SESSION_START_BYTES: usize = 10_000;
const PROMPT_BYTES: usize = 10_000;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("replay")
}

fn fixture(name: &str) -> String {
    let path = fixtures().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn content_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// One store shape from `session-start.txt`.
#[derive(Debug, Default)]
struct Case {
    name: String,
    /// (days past due, name), oldest due first.
    reminders: Vec<(i64, String)>,
    /// (slug, project), newest first.
    handoffs: Vec<(String, String)>,
    deferred: usize,
    relay_store: Option<String>,
}

fn cases() -> Vec<Case> {
    let text = fixture("session-start.txt");
    let mut out: Vec<Case> = Vec::new();
    for line in content_lines(&text) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.push(Case { name: name.to_string(), ..Case::default() });
            continue;
        }
        let case = out.last_mut().unwrap_or_else(|| panic!("a key before the first [case]: {line}"));
        let (key, value) = line.split_once(" = ").unwrap_or_else(|| panic!("not 'key = value': {line}"));
        let pair = || {
            let (a, b) = value.split_once(" | ").unwrap_or_else(|| panic!("not 'a | b': {line}"));
            (a.trim().to_string(), b.trim().to_string())
        };
        match key {
            "reminder" => {
                let (days, name) = pair();
                assert!(!name.contains(['"', '\\']), "a name the seed cannot write unescaped: {name}");
                case.reminders.push((days.parse().expect("days past due"), name));
            }
            "handoff" => case.handoffs.push(pair()),
            "deferred" => case.deferred = value.parse().expect("deferred count"),
            "relay-store" => case.relay_store = Some(value.to_string()),
            other => panic!("unknown key {other:?} in [{}]", case.name),
        }
    }
    assert!(out.len() >= 4, "control: the corpus has its session-start cases: {}", out.len());
    out
}

fn prompts() -> Vec<String> {
    let text = fixture("prompts.txt");
    let out: Vec<String> = content_lines(&text).map(str::to_string).collect();
    assert!(out.len() >= 30, "control: the corpus has about 30 prompts: {}", out.len());
    out
}

/// `days` (and `seconds`) before now, as the store writes a dateTime.
fn ago(days: i64, seconds: i64) -> String {
    (chrono::Local::now() - chrono::Duration::days(days) - chrono::Duration::seconds(seconds))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// N-Quads in the workspace tier's seed graph.
struct Quads(String);

impl Quads {
    fn graph() -> String {
        format!("{}graph/ws/seed", seed::NS)
    }
    fn typ(&mut self, s: &str, ty: &str) {
        self.0.push_str(&format!("<{0}{s}> <{RDF_TYPE}> <{0}{ty}> <{1}> .\n", seed::NS, Self::graph()));
    }
    fn lit(&mut self, s: &str, p: &str, v: &str) {
        self.0.push_str(&format!("<{0}{s}> <{0}{p}> \"{v}\" <{1}> .\n", seed::NS, Self::graph()));
    }
    fn date(&mut self, s: &str, p: &str, v: &str) {
        self.0.push_str(&format!("<{0}{s}> <{0}{p}> \"{v}\"^^<{XSD_DATETIME}> <{1}> .\n", seed::NS, Self::graph()));
    }
    fn iri(&mut self, s: &str, p: &str, o: &str) {
        self.0.push_str(&format!("<{0}{s}> <{0}{p}> <{0}{o}> <{1}> .\n", seed::NS, Self::graph()));
    }
}

/// A fresh seed root for `tag`, cleaned first and the clean asserted: a stale root would feed
/// another run's records into every check.
fn root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("base-replay-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(!root.exists(), "the seed root {} survived its clean", root.display());
    root
}

/// The real-size seed with no reminders of its own, so every due reminder is the case's.
const REAL_NO_REMINDERS: seed::Sizes = seed::Sizes { due_reminders: 0, ..seed::REAL };

fn write_case(case: &Case) -> seed::Seed {
    write_case_as(case, &case.name)
}

/// [`write_case`] under its own root `tag`, for a check that runs a case again without touching the shared run's seed.
fn write_case_as(case: &Case, tag: &str) -> seed::Seed {
    let s = seed::write(&root(tag), &REAL_NO_REMINDERS, &fixture("base.toml"));
    let mut q = Quads(String::new());
    for (i, (days, name)) in case.reminders.iter().enumerate() {
        let r = format!("reminder/replay-reminder-{i}");
        q.typ(&r, "Reminder");
        q.lit(&r, "name", name);
        // A minute and then a second apart past the whole days, so the order is the corpus order.
        q.date(&r, "resurfaceAt", &ago(*days, 60 + (case.reminders.len() - i) as i64));
    }
    let docs = s.ws.parent().expect("seed root").join("handoffs");
    std::fs::create_dir_all(&docs).expect("handoff docs");
    for (i, (slug, project)) in case.handoffs.iter().enumerate() {
        let doc = docs.join(format!("{slug}.md"));
        std::fs::write(&doc, format!("# {slug}\n\nreplay fixture\n")).expect("handoff doc");
        let h = format!("handoff/{slug}");
        // Created in the last hours, first listed newest, so the case's handoffs take the letters.
        let created = (chrono::Local::now() - chrono::Duration::minutes(10 + i as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        q.typ(&h, "Handoff");
        q.lit(&h, "name", project);
        q.lit(&h, "project", project);
        q.lit(&h, "handoffDoc", &doc.display().to_string().replace('\\', "/"));
        q.lit(&h, "kind", "handoff");
        q.lit(&h, "status", "open");
        for p in ["createdAt", "resurfaceAt", "lastActive"] {
            q.date(&h, p, &created);
        }
    }
    for i in 0..case.deferred {
        let h = format!("handoff/2026-08-30-1000-replay-parked-{i:02}");
        q.typ(&h, "Handoff");
        q.lit(&h, "name", &format!("parked-{i:02}"));
        q.lit(&h, "project", &format!("parked-{i:02}"));
        q.lit(&h, "kind", "handoff");
        q.lit(&h, "status", "deferred");
        q.lit(&h, "deferredReason", "auto: cold 12d");
        q.date(&h, "deferredAt", &ago(4, 0));
        for p in ["createdAt", "resurfaceAt", "lastActive"] {
            q.date(&h, p, &ago(16, 0));
        }
    }
    let graph = s.ws.join(".base").join("graph.nq");
    let mut text = std::fs::read_to_string(&graph).expect("the seed's workspace graph");
    text.push_str(&q.0);
    std::fs::write(&graph, text).expect("the case's quads");
    if let Some(store) = &case.relay_store {
        std::fs::create_dir_all(s.ws.join(".base").join("relay").join(store)).expect("relay store");
    }
    s
}

/// The last record `hook` kept in the workspace's hook-output.jsonl.
fn last_record(s: &seed::Seed, hook: &str) -> serde_json::Value {
    let path = s.ws.join(".base").join("hook-output.jsonl");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["hook"] == hook)
        .unwrap_or_else(|| panic!("no {hook} record in {}", path.display()))
}

struct SessionRun {
    case: Case,
    seed: seed::Seed,
    stdout: String,
    stderr: String,
    record: serde_json::Value,
}

/// Every session-start case, run once and shared by every check.
fn session_starts() -> &'static [SessionRun] {
    static RUNS: OnceLock<Vec<SessionRun>> = OnceLock::new();
    RUNS.get_or_init(|| {
        cases()
            .into_iter()
            .map(|case| {
                let seed = write_case(&case);
                let (code, stdout, stderr) = run_session_start(&seed, None);
                assert_eq!(code, 0, "[{}] session start failed: {stderr}", case.name);
                assert!(!stdout.is_empty(), "[{}] session start printed nothing: {stderr}", case.name);
                let record = last_record(&seed, "session-start");
                SessionRun { case, seed, stdout, stderr, record }
            })
            .collect()
    })
}

struct PromptRun {
    prompt: String,
    stdout: String,
    record: serde_json::Value,
    /// The session the prompt ran in and the workspace, so a check can read the session's prompt blocks (BO-01).
    session: String,
    ws: PathBuf,
}

/// Every prompt, each as the first prompt of its own session on one seed carrying the corpus's
/// domains and the bracket, run once and shared by every check.
fn prompt_runs() -> &'static [PromptRun] {
    static RUNS: OnceLock<Vec<PromptRun>> = OnceLock::new();
    RUNS.get_or_init(|| {
        let s = seed::write(&root("prompts"), &seed::TINY, &fixture("base.toml"));
        std::fs::write(s.ws.join(".base").join("domains.toml"), fixture("domains.toml")).expect("domains.toml");
        prompts()
            .into_iter()
            .enumerate()
            .map(|(i, prompt)| {
                let session = format!("replay-{i:02}");
                let (code, stdout, stderr) = run_prompt_submit(&s, &prompt, Some(&session));
                assert_eq!(code, 0, "{prompt:?}: the prompt hook failed: {stderr}");
                let record = last_record(&s, "user-prompt-submit");
                PromptRun { prompt, stdout, record, session, ws: s.ws.clone() }
            })
            .collect()
    })
}

fn number(v: &serde_json::Value, key: &str) -> usize {
    v[key].as_u64().unwrap_or_else(|| panic!("no number {key:?} in {v}")) as usize
}

/// Where the first screen ends in what was printed: after DUE NOW's last line when DUE NOW is
/// there, else after the instruction block's Letters line.
fn first_screen_end(stdout: &str) -> usize {
    let mut end = None;
    let mut offset = 0;
    let mut in_due = false;
    for line in stdout.split_inclusive('\n') {
        offset += line.len();
        let text = line.trim_end_matches(['\r', '\n']);
        if text.starts_with("DUE NOW (") {
            in_due = true;
            end = Some(offset);
        } else if in_due && text.starts_with("  ") {
            end = Some(offset);
        } else if in_due {
            break;
        } else if text.starts_with("Letters: ") {
            end = Some(offset);
        }
    }
    end.expect("neither DUE NOW nor the Letters line is in the output")
}

#[test]
fn replay_first_screen_fits_for_every_corpus_case() {
    for run in session_starts() {
        let name = &run.case.name;
        let measured = number(&run.record, "first_screen_len_u16");
        assert_eq!(number(&run.record, "first_screen_u16"), BAR, "[{name}] control: the fixture config set the limit");
        assert!(
            measured <= BAR && run.record["first_screen_ok"] == true,
            "[{name}] first screen {measured} UTF-16 units, past the {BAR} bar:\n{}",
            &run.stdout[..first_screen_end(&run.stdout)]
        );
        assert!(
            !run.stderr.contains("instructions and DUE NOW take"),
            "[{name}] an overflow was reported: {}",
            run.stderr
        );
        // The recorded length is what was printed: the record cannot drift from the output.
        let printed = units(&run.stdout[..first_screen_end(&run.stdout)]);
        assert_eq!(measured, printed, "[{name}] the record's first_screen_len_u16 is not the printed first screen");
        println!("replay [{name}]: first screen {measured} of {BAR} UTF-16 units");
    }
}

/// lynx, BO-00 B4: DUE NOW is trimmed only to fit, never below the most overdue reminder, and its
/// first line says how many it shows of how many; the numbers it prints resolve to the reminders
/// they stand for. Checked on the 5- and 10-due cases and every other case with reminders.
#[test]
fn replay_due_now_keeps_the_most_overdue_and_says_what_it_cut() {
    let mut checked = 0;
    for run in session_starts().iter().filter(|r| !r.case.reminders.is_empty()) {
        let name = &run.case.name;
        let total = run.case.reminders.len();
        let lines: Vec<&str> = run.stdout.lines().collect();
        let head = lines
            .iter()
            .position(|l| l.starts_with(&format!("DUE NOW ({total}) · ")))
            .unwrap_or_else(|| panic!("[{name}] no DUE NOW ({total}) line:\n{}", run.stdout));
        let items: Vec<&str> = lines[head + 1..].iter().take_while(|l| l.starts_with("  ")).copied().collect();
        let shown = items.len();
        assert!(shown >= 1, "[{name}] DUE NOW shows no reminder at all");
        let want_head = if shown == total {
            format!("DUE NOW ({total}) · all: base reminder list")
        } else {
            format!("DUE NOW ({total}) · {shown} shown · +{} more · all: base reminder list", total - shown)
        };
        assert_eq!(lines[head], want_head, "[{name}] DUE NOW's first line");
        for (i, item) in items.iter().enumerate() {
            let want = format!("  {} {}", i + 1, run.case.reminders[i].1);
            assert!(item.starts_with(&want), "[{name}] DUE NOW line {} is {item:?}, wanted {want:?}", i + 1);
        }
        // The numbers resolve: the letters file maps each printed number to its reminder.
        let letters = std::fs::read_to_string(run.seed.ws.join(".base").join("last-session-start-letters.json"))
            .expect("the letters file");
        let file: serde_json::Value = serde_json::from_str(&letters).expect("letters JSON");
        for i in 0..total {
            assert_eq!(
                file["reminders"][(i + 1).to_string()],
                format!("replay-reminder-{i}"),
                "[{name}] number {} does not resolve to its reminder",
                i + 1
            );
        }
        println!("replay [{name}]: DUE NOW shows {shown} of {total}");
        checked += 1;
    }
    let by_count = |n: usize| session_starts().iter().any(|r| r.case.reminders.len() == n);
    assert!(checked >= 3 && by_count(5) && by_count(10), "control: the 5- and 10-due cases are in the corpus");
    assert!(
        session_starts().iter().any(|r| r.case.reminders.len() == 10 && !r.stdout.contains("DUE NOW (10) · all:")),
        "control: the 10-due case does not fit whole, so the trim is exercised"
    );
}

/// The relay join notice is not due now: it never takes first-screen room, and it is still written.
#[test]
fn replay_relay_join_notice_stays_off_the_first_screen() {
    let mut checked = 0;
    for run in session_starts().iter().filter(|r| r.case.relay_store.is_some()) {
        let name = &run.case.name;
        let screen = &run.stdout[..first_screen_end(&run.stdout)];
        assert!(!screen.contains("<relay-notice>"), "[{name}] the join notice is on the first screen:\n{screen}");
        let full = std::fs::read_to_string(run.seed.ws.join(".base").join("last-session-start.md")).expect("full output");
        assert!(full.contains("<relay-notice>"), "[{name}] control: no join notice was produced at all");
        checked += 1;
    }
    assert!(checked >= 1, "control: a case carries a relay store");
}

#[test]
fn replay_output_within_budget() {
    for run in session_starts() {
        let name = &run.case.name;
        assert!(
            run.stdout.len() <= SESSION_START_BYTES && run.record["over_budget"] == false,
            "[{name}] session start printed {} bytes against {SESSION_START_BYTES}",
            run.stdout.len()
        );
        assert_eq!(number(&run.record, "budget_bytes"), SESSION_START_BYTES, "[{name}] control: the budget");
    }
    let mut cut = 0;
    for run in prompt_runs() {
        let emitted = number(&run.record, "emitted_bytes");
        assert!(
            run.stdout.len() <= PROMPT_BYTES && emitted <= PROMPT_BYTES,
            "{:?}: the prompt hook printed {} bytes (record {emitted}) against {PROMPT_BYTES}",
            run.prompt,
            run.stdout.len()
        );
        assert_eq!(emitted, run.stdout.len(), "{:?}: the record is not what was printed", run.prompt);
        cut += usize::from(number(&run.record, "withheld_bytes") > 0);
    }
    // A budget check over a corpus that never reaches the budget proves nothing.
    assert!(cut > 0, "control: no prompt in the corpus reached the {PROMPT_BYTES}-byte budget");
    assert!(
        prompt_runs().iter().any(|r| r.stdout.contains("A handled reminder is archived")),
        "control: no prompt matched a keyword domain, so the corpus injects nothing it could lose"
    );
    println!("replay: {} prompts, {cut} cut at the budget", prompt_runs().len());
}

/// BO-01 (F1, F2, F7). Every prompt's output is whole blocks and pointer lines and nothing else, rebuilt byte for byte
/// from the session's blocks file; the blocks are in priority order; and the record names every dropped block by
/// name, items and bytes, with `withheld_bytes` their sum. Before BO-01 the hook cut lines from the end, so a cut
/// prompt ended inside a block and its record said `"withheld": []`.
#[test]
fn replay_prompt_output_is_whole_blocks_in_priority_order() {
    let mut cut = 0;
    for run in prompt_runs() {
        let p = &run.prompt;
        let file = seed::prompt_blocks(&run.ws, &run.session);
        assert_eq!(run.stdout, seed::rebuilt_prompt_output(&run.stdout, &file), "{p:?}: not whole blocks and pointer lines");
        let order: Vec<u8> = file.blocks.iter().map(|b| b.priority).collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "{p:?}: blocks out of priority order: {order:?}");
        let dropped: Vec<serde_json::Value> = file
            .blocks
            .iter()
            .filter(|b| !b.printed)
            .map(|b| serde_json::json!({"block": b.id, "items": b.items, "bytes": b.bytes, "reason": "budget"}))
            .collect();
        assert_eq!(run.record["withheld"], serde_json::Value::Array(dropped.clone()), "{p:?}: the record's rows");
        let sum: usize = file.blocks.iter().filter(|b| !b.printed).map(|b| b.bytes).sum();
        assert_eq!(number(&run.record, "withheld_bytes"), sum, "{p:?}: withheld_bytes is the sum of the rows");
        cut += usize::from(!dropped.is_empty());
    }
    assert!(cut > 0, "control: no prompt in the corpus dropped a block, so nothing here was exercised");
    println!("replay: {} prompts, {cut} with blocks dropped whole", prompt_runs().len());
}

/// BO-02 (F6c). The corpus runs at the shipped budgets, and those are now the size `base doctor --measure` found the
/// host delivering whole on Claude Code 2.1.287: no prompt prints more than that, and a block larger than the old
/// 4,000-byte cap prints whole. On the operator's store the always-on rules block is 9,372 bytes, and under the old
/// cap it was dropped on every prompt (BO-01 FINAL STATE); the corpus's `ledger` domain is the same shape, synthetic.
/// A later order that lowers the default, or lets the fixture drift from it, turns this red.
#[test]
fn replay_measured_budget_carries_a_block_the_old_cap_dropped() {
    const OLD_CAP: usize = 4000;
    let shipped = base::config::BudgetConfig::default();
    assert_eq!(
        (shipped.prompt_bytes, shipped.session_start_bytes),
        (PROMPT_BYTES, SESSION_START_BYTES),
        "control: the corpus runs at the shipped defaults"
    );
    assert_eq!(PROMPT_BYTES, base::config::MEASURED_HOOK_BYTES, "control: the shipped default is the measured size");
    let mut carried = Vec::new();
    for run in prompt_runs() {
        let p = &run.prompt;
        assert!(run.stdout.len() <= base::config::MEASURED_HOOK_BYTES, "{p:?}: printed {} bytes", run.stdout.len());
        let file = seed::prompt_blocks(&run.ws, &run.session);
        for b in file.blocks.iter().filter(|b| b.printed && b.bytes > OLD_CAP) {
            assert!(run.stdout.contains(&b.text), "{p:?}: {} is marked printed but is not in the output whole", b.id);
            carried.push(format!("{} ({} bytes)", b.id, b.bytes));
        }
    }
    assert!(
        !carried.is_empty(),
        "control: no prompt carried a block over the old {OLD_CAP}-byte cap, so the measured budget was never exercised"
    );
    println!("replay: blocks over the old {OLD_CAP}-byte cap printed whole: {}", carried.join(", "));
}

// ── BO-03 (F3, F5, F14b) ─────────────────────────────────────────────────────────────────────────────

/// A global decision in BO-03's seed: its text, its keywords, and whether a later one supersedes it.
struct GlobalFixture {
    slug: &'static str,
    text: &'static str,
    keywords: &'static [&'static str],
    superseded_by: Option<&'static str>,
}

/// Synthetic decisions of the corpus's always-on `global` domain, matched to words the corpus prompts use.
const GLOBAL_DECISIONS: [GlobalFixture; 6] = [
    GlobalFixture { slug: "release", text: "REPLAY_DECISION release builds are tagged only after the user test plan passes", keywords: &["release"], superseded_by: None },
    GlobalFixture { slug: "dealer-normal", text: "REPLAY_DECISION dealer data is normalized before it is published", keywords: &["dealer"], superseded_by: None },
    GlobalFixture { slug: "ledger", text: "REPLAY_DECISION the ledger closes only after review signs off", keywords: &["ledger", "payroll"], superseded_by: None },
    GlobalFixture { slug: "codename", text: "REPLAY_DECISION every session keeps one codename across a refresh", keywords: &[], superseded_by: None },
    GlobalFixture { slug: "dealer-weekly", text: "REPLAY_DECISION dealer lists are exported weekly", keywords: &["dealer"], superseded_by: Some("dealer-daily") },
    GlobalFixture { slug: "dealer-daily", text: "REPLAY_DECISION dealer lists are exported daily", keywords: &["dealer"], superseded_by: None },
];

/// BO-03's session: the corpus's prompts, in order, as ONE session, so the bracket walks FRESH, MODERATE,
/// DEPLETED and CRITICAL (turn thresholds 3, 10, 20); the corpus's domains; the bracket rules of
/// `bracket.toml`; the user CLAUDE.md of `claude.md`; and the global decisions above. Then one session start.
struct Bo03Session {
    prompts: Vec<(String, String)>,
    session_start: String,
}

fn bo03_session() -> &'static Bo03Session {
    static RUN: OnceLock<Bo03Session> = OnceLock::new();
    RUN.get_or_init(|| {
        let s = seed::write(&root("bo03"), &seed::TINY, &format!("{}\n{}", fixture("base.toml"), fixture("bracket.toml")));
        std::fs::write(s.ws.join(".base").join("domains.toml"), fixture("domains.toml")).expect("domains.toml");
        std::fs::create_dir_all(s.home.join(".claude")).expect("the seed home's .claude");
        std::fs::write(s.home.join(".claude").join("CLAUDE.md"), fixture("claude.md")).expect("CLAUDE.md");
        let mut q = Quads(String::new());
        for d in &GLOBAL_DECISIONS {
            let iri = format!("decision/global.{}", d.slug);
            q.typ(&iri, "Decision");
            q.lit(&iri, "name", d.text);
            q.lit(&iri, "rationale", "replay fixture");
            q.iri("domain/global", "hasDecision", &iri);
            for k in d.keywords {
                q.lit(&iri, "decisionKeyword", k);
            }
            if let Some(next) = d.superseded_by {
                q.iri(&iri, "supersededBy", &format!("decision/global.{next}"));
                q.iri(&format!("decision/global.{next}"), "supersedes", &iri);
            }
        }
        let graph = s.ws.join(".base").join("graph.nq");
        let mut text = std::fs::read_to_string(&graph).expect("the seed's workspace graph");
        text.push_str(&q.0);
        std::fs::write(&graph, text).expect("the decisions' quads");
        let prompts = prompts()
            .into_iter()
            .map(|prompt| {
                let (code, stdout, stderr) = run_prompt_submit(&s, &prompt, Some("replay-bo03"));
                assert_eq!(code, 0, "{prompt:?}: the prompt hook failed: {stderr}");
                (prompt, stdout)
            })
            .collect();
        let (code, session_start, stderr) = run_session_start(&s, Some("replay-bo03-start"));
        assert_eq!(code, 0, "session start failed: {stderr}");
        Bo03Session { prompts, session_start }
    })
}

/// BO-03 F3. Over one session: a bracket rule covered by the user's CLAUDE.md is never sent, and every other rule
/// is sent exactly once, its tier's rules included (DEPLETED and CRITICAL once each, at their tier). Before BO-03
/// every rule in force went again at each tier change, covered or not.
#[test]
fn replay_bracket_rules_once_per_session_and_never_when_claude_md_covers_them() {
    let run = bo03_session();
    let count = |label: &str| run.prompts.iter().filter(|(_, out)| out.contains(&format!(". {label} "))).count();
    for covered in ["REPLAY_R1", "REPLAY_R2", "REPLAY_R1R2"] {
        assert_eq!(count(covered), 0, "{covered} is covered by claude.md and was sent");
    }
    for (label, tier) in [("REPLAY_ALWAYS", "FRESH"), ("REPLAY_DEPLETED", "DEPLETED"), ("REPLAY_CRITICAL", "CRITICAL")] {
        assert_eq!(count(label), 1, "{label} is sent once in the session");
        let (p, out) = run.prompts.iter().find(|(_, out)| out.contains(&format!(". {label} "))).unwrap();
        assert!(out.contains(&format!("[{tier}]")), "{label} went at {tier}: {p:?}\n{out}");
    }
    assert!(
        run.prompts.iter().all(|(_, out)| out.starts_with("<context-bracket>[")),
        "control: every prompt names its tier in the header line"
    );
    println!("replay: {} prompts in one session, each uncovered bracket rule sent once, covered ones never", run.prompts.len());
}

/// BO-03 F5 and F14b. A global decision reaches a prompt only when the prompt carries one of its keywords; one with
/// no keywords never does and is at session start; a superseded one never does.
#[test]
fn replay_global_decisions_only_on_their_keywords() {
    let run = bo03_session();
    let mut served = 0;
    for (prompt, out) in &run.prompts {
        let lower = prompt.to_lowercase();
        for d in &GLOBAL_DECISIONS {
            if !out.contains(d.text) {
                continue;
            }
            assert!(d.superseded_by.is_none(), "{prompt:?}: a superseded decision was served:\n{out}");
            assert!(
                d.keywords.iter().any(|k| base::domain::matcher::contains_word(&lower, k)),
                "{prompt:?}: {} was served without one of its keywords {:?}:\n{out}",
                d.slug,
                d.keywords
            );
            served += 1;
        }
    }
    assert!(served > 0, "control: no prompt carried a keyword, so the rule was never exercised");
    let keywordless = GLOBAL_DECISIONS.iter().find(|d| d.keywords.is_empty()).unwrap();
    assert!(
        run.session_start.contains(&format!("  - Decision: {}", keywordless.text)),
        "the decision with no keywords is at session start:\n{}",
        run.session_start
    );
    println!("replay: global decisions served {served} times, each on one of its keywords");
}

// ── BO-06 (F10, F11, D16b) ───────────────────────────────────────────────────────────────────────────────

/// The number on `line` after `prefix`, when the line starts with it.
fn number_after(line: &str, prefix: &str) -> Option<usize> {
    line.strip_prefix(prefix)?.split(|c: char| !c.is_ascii_digit()).find(|w| !w.is_empty())?.parse().ok()
}

/// Line 1's number before `label` (` · 5 due`) or after it (` · forks 157`).
fn header_number(line1: &str, label: &str) -> usize {
    line1
        .split(" · ")
        .find_map(|part| {
            let words: Vec<&str> = part.split_whitespace().collect();
            match words.as_slice() {
                [n, l] if *l == label => n.parse().ok(),
                [l, n, ..] if *l == label => n.parse().ok(),
                _ => None,
            }
        })
        .unwrap_or_else(|| panic!("no {label} on line 1: {line1}"))
}

/// BO-06 (F10). On every corpus case, line 1, the pulse and each block's first line print one number per label. Measured
/// on 2026-10-01: line 1 said `projects 0 · tasks 0` beside a pulse of 28 and 145, and the pulse said `Reminders: 4
/// overdue` beside a DUE NOW of 5.
#[test]
fn replay_counts_agree_on_every_corpus_case() {
    let mut checked = 0;
    for run in session_starts() {
        let name = &run.case.name;
        let out = &run.stdout;
        let line1 = out.lines().next().unwrap_or_default();
        let due = run.case.reminders.len();
        assert_eq!(header_number(line1, "due"), due, "[{name}] line 1's due: {line1}");
        let mut pairs = vec![("DUE NOW (", "due"), ("HANDOFFS (", "handoffs"), ("FORKS (", "forks")];
        pairs.extend([("PROJECTS (", "projects"), ("TASKS (", "tasks"), ("MILESTONES (", "milestones")]);
        pairs.extend([("Projects: ", "projects"), ("Tasks: ", "tasks"), ("Reminders: ", "due")]);
        for (prefix, label) in pairs {
            if let Some(n) = out.lines().find_map(|l| number_after(l, prefix)) {
                assert_eq!(n, header_number(line1, label), "[{name}] {prefix:?} against line 1's {label}:\n{out}");
                checked += 1;
            }
        }
        if let Some(l) = out.lines().find(|l| l.starts_with("Reminders: ")) {
            assert_eq!(l, format!("Reminders: {due} due"), "[{name}] the pulse uses DUE NOW's word and rule");
        }
    }
    assert!(checked >= 30, "control: {checked} numbers compared across the corpus");
    println!("replay: {checked} block and pulse numbers agree with line 1");
}

/// BO-06 (D16b, F11b). The 5-due case run as a session, the way Claude Code runs it: line 1 names the session's own
/// file (about 45 units longer than the workspace one), and all five due reminders still print inside the 1,990 bar.
/// Before BO-06 the case showed 3 of 5 with the shorter workspace path. The seed's root is a temp folder, longer than
/// the operator's home, so this is harsher than his store: there it measured 1,930 (FINAL STATE of BO-06).
#[test]
fn replay_five_due_fit_with_the_session_file_named() {
    let case = cases().into_iter().find(|c| c.name == "real-5-due").expect("the real-5-due case");
    let s = write_case_as(&case, "r5s");
    let session = "b0600000-0000-4000-8000-000000000005";
    let (code, out, err) = run_session_start(&s, Some(session));
    assert_eq!(code, 0, "{err}");
    let own = s.ws.join(".base").join("hook-output").join(session).join("session-start.md");
    let line1 = out.lines().next().unwrap_or_default();
    assert!(line1.ends_with(&format!("· full: {}]", own.display())), "line 1 names the session's file: {line1}");
    assert!(out.contains("DUE NOW (5) · all: base reminder list"), "all five print:\n{}", &out[..first_screen_end(&out)]);
    let record = last_record(&s, "session-start");
    let measured = number(&record, "first_screen_len_u16");
    assert!(measured <= BAR && record["first_screen_ok"] == true, "first screen {measured} of {BAR}");
    println!("replay [real-5-due as a session]: first screen {measured} of {BAR} UTF-16 units, all 5 due shown");
}

#[test]
fn replay_corpus_has_no_real_names() {
    const DENY: [&str; 11] = [
        "anthony", "vintryx", "vintrix", "renda", "caddy", "chriskahler", "chris",
        "c:/users", "c:\\users", "/home/", "/users/",
    ];
    let mut read = 0;
    for entry in std::fs::read_dir(fixtures()).expect("the corpus directory") {
        let path = entry.expect("entry").path();
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).to_lowercase();
        for word in DENY {
            assert!(!text.contains(word), "{} carries {word:?}; the corpus is public", path.display());
        }
        read += 1;
    }
    assert!(read >= 4, "control: the corpus files were read: {read}");
}

/// One session holding a relay title, no inbox watcher, and two pending pings from two senders, driven through every
/// corpus prompt in order with a Bash tool call after each, the way a working session runs. The prompts and the tool
/// calls' outputs, in order.
struct RelayRun {
    prompts: Vec<String>,
    tools: Vec<String>,
}

fn relay_run() -> &'static RelayRun {
    static RUN: OnceLock<RelayRun> = OnceLock::new();
    RUN.get_or_init(|| {
        let s = seed::write(&root("relay"), &seed::TINY, &fixture("base.toml"));
        std::fs::write(s.ws.join(".base").join("domains.toml"), fixture("domains.toml")).expect("domains.toml");
        let session = "replay-relay";
        let (code, _, err) = run_base_in_session(&s, &["relay", "register", "--as", "seed-kite"], session);
        assert_eq!(code, 0, "register: {err}");
        for (from, msg) in [("seed-bison", "RELAY-PING-ONE: stand down on the progress doc"), ("seed-heron", "RELAY-PING-TWO: which schema?")] {
            let (code, _, err) = run_base(&s, &["relay", "ping", "--from", from, "--to", "seed-kite", "--msg", msg]);
            assert_eq!(code, 0, "ping: {err}");
        }
        let mut run = RelayRun { prompts: Vec::new(), tools: Vec::new() };
        for prompt in prompts() {
            let (code, stdout, stderr) = run_prompt_submit(&s, &prompt, Some(session));
            assert_eq!(code, 0, "{prompt:?}: the prompt hook failed: {stderr}");
            run.prompts.push(stdout);
            let (code, stdout, stderr) = run_pre_tool_use(&s, "Bash", serde_json::json!({ "command": "ls" }), session, &[]);
            assert_eq!(code, 0, "the pre-tool hook failed: {stderr}");
            run.tools.push(stdout);
        }
        run
    })
}

/// BO-04 (F4, F13). Measured on 2026-10-01 in session 5b860473: the 3,400-byte wake contract on six prompts and on
/// ordinary tool calls, "Reply RIGHT NOW" on every prompt and many tool calls while a ping stayed unanswered. Across a
/// whole corpus session now: the watcher nudge is one line, said once; each ping is shown once, on the first prompt;
/// no tool call carries relay text; nothing claims priority over the user; the script is never in a hook.
#[test]
fn replay_relay_takes_one_line_and_never_repeats() {
    let run = relay_run();
    assert!(run.prompts.len() >= 20, "control: the corpus session ran {} prompts", run.prompts.len());
    let all_prompts = run.prompts.join("\n");
    let nudge = "relay: seed-kite has no inbox watcher · run base relay arm and start the Monitor it prints";
    assert_eq!(all_prompts.matches(nudge).count(), 1, "the nudge is said once per session");
    assert!(run.prompts[0].contains(nudge), "and on the first prompt:\n{}", run.prompts[0]);
    for marker in ["RELAY-PING-ONE", "RELAY-PING-TWO"] {
        assert_eq!(all_prompts.matches(marker).count(), 1, "{marker} is shown once in the session");
        assert!(run.prompts[0].contains(marker), "{marker} is shown on the first prompt:\n{}", run.prompts[0]);
    }
    for (i, out) in run.prompts.iter().chain(&run.tools).enumerate() {
        for banned in ["RELAY WAKE CONTRACT", "$INBOX/.watching", "persistent", "REPLY REQUIRED", "RIGHT NOW", "DIRECTIVE"] {
            assert!(!out.contains(banned), "output {i} carries {banned:?}:\n{out}");
        }
    }
    for (i, out) in run.tools.iter().enumerate() {
        assert!(!out.contains("relay:") && !out.contains("RELAY-PING"), "tool call {i} carries relay text:\n{out}");
    }
    let relay_bytes: usize = run
        .prompts
        .iter()
        .flat_map(|p| p.split("\n\n"))
        .filter(|b| b.starts_with("relay:") || b.starts_with("relay ("))
        .map(|b| b.len())
        .sum();
    println!("replay relay: {} prompts, {} tool calls, {relay_bytes} relay bytes in all", run.prompts.len(), run.tools.len());
}

/// BO-05 (F12). Measured on 2026-10-01: session 5b860473 was auto-given the title `lynx`, inherited the inbox of the
/// session that had held it, and was shown bison's pings about a doc it had never seen. Through a whole corpus session
/// now: `seed-kite` was held by a session that ended with two of seed-bison's pings unshown, and the corpus session is
/// auto-titled `seed-kite` at its start. No output shows either ping or lists it as unanswered, both pings are in the
/// old holder's archive folder, and seed-bison is told once, addressed to the session that sent them.
#[test]
fn replay_reassigned_title_never_inherits_pings() {
    let s = seed::write(&root("relay-reassign"), &seed::TINY, &fixture("base.toml"));
    std::fs::write(s.ws.join(".base").join("domains.toml"), fixture("domains.toml")).expect("domains.toml");
    for (title, session) in [("seed-bison", "replay-bison"), ("seed-kite", "replay-ended")] {
        let (code, _, err) = run_base_in_session(&s, &["relay", "register", "--as", title], session);
        assert_eq!(code, 0, "register: {err}");
    }
    for msg in ["RELAY-PING-ONE: go: edit the progress doc", "RELAY-PING-TWO: stand down on the progress doc"] {
        let (code, _, err) = run_base_in_session(&s, &["relay", "ping", "--to", "seed-kite", "--msg", msg], "replay-bison");
        assert_eq!(code, 0, "ping: {err}");
    }
    let session = "replay-successor";
    let mut outputs = Vec::new();
    let (code, stdout, stderr) = run_session_start(&s, Some(session));
    assert_eq!(code, 0, "session start: {stderr}");
    outputs.push(stdout);
    for prompt in prompts() {
        let (code, stdout, stderr) = run_prompt_submit(&s, &prompt, Some(session));
        assert_eq!(code, 0, "{prompt:?}: the prompt hook failed: {stderr}");
        outputs.push(stdout);
    }
    assert!(outputs.len() >= 31, "control: the corpus session ran {} hooks", outputs.len());
    let (_, sessions, _) = run_base(&s, &["relay", "sessions"]);
    assert!(sessions.contains("session:replay-successor"), "control: the session was auto-titled:\n{sessions}");
    for (i, out) in outputs.iter().enumerate() {
        assert!(!out.contains("RELAY-PING") && !out.contains("unanswered"), "output {i} carries the old holder's pings:\n{out}");
    }
    let inbox = s.home.join(".base-gbl").join(".base").join("relay-inbox");
    let count = |dir: &Path| -> Vec<serde_json::Value> {
        std::fs::read_dir(dir)
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(count(&inbox.join(".archive").join("seed-kite-replay-ended")).len(), 2, "both pings are archived");
    let told = count(&inbox.join("seed-bison"));
    assert_eq!(told.len(), 1, "seed-bison is told once: {told:?}");
    assert_eq!(told[0]["kind"], "undelivered");
    assert_eq!(told[0]["to_session"], "replay-bison");
    println!("replay relay reassignment: {} hooks, 0 inherited pings, 2 archived, 1 notice", outputs.len());
}

/// One call from `tool-calls.txt`.
struct ToolCall {
    expect: String,
    tool: String,
    cwd: String,
    input: String,
}

fn tool_calls() -> Vec<ToolCall> {
    let text = fixture("tool-calls.txt");
    let out: Vec<ToolCall> = content_lines(&text)
        .map(|line| {
            let mut parts = line.splitn(4, " | ");
            let mut next = || parts.next().unwrap_or_else(|| panic!("not 'expect | tool | cwd | input': {line}")).trim().to_string();
            ToolCall { expect: next(), tool: next(), cwd: next(), input: next() }
        })
        .collect();
    assert!(out.len() >= 40, "control: the corpus has its tool calls: {}", out.len());
    out
}

/// BO-07 (F20, F26). Measured on 2026-10-01 in session 5b860473: the AST hint on 45 of the 119 tool calls the pre-tool
/// hook saw, nearly all of them `base … | grep`, TOML and markdown searches, or folders no map covers; and A4 and A8 on
/// a markdown fork doc. Through a corpus of calls shaped on that session's: a hint only on a code search, naming the
/// map that covers the folder searched and a plain name to look up; standards only on code (the shipped seed's own
/// scopes); and every expectation in the corpus met.
#[test]
fn replay_pre_tool_hints_fit_the_file_or_command() {
    let s = seed::write(&root("pre-tool-hints"), &seed::TINY, &fixture("base.toml"));
    std::fs::write(s.ws.join(".base").join("domains.toml"), fixture("domains.toml")).expect("domains.toml");
    let shipped = toml::to_string_pretty(&base::standards::sync::seed_file()).expect("the shipped standards");
    std::fs::write(s.home.join(".base-gbl").join("standards.toml"), shipped).expect("standards.toml");
    let apps = seed::write_apps(&s);
    let slash = |p: &Path| p.display().to_string().replace('\\', "/");
    let places = [
        ("{home}", slash(&s.home)),
        ("{ws}", slash(&s.ws)),
        ("{mapped}", slash(&apps.mapped)),
        ("{plain}", slash(&apps.plain)),
        ("{cached}", slash(&apps.cached)),
        ("{loose}", slash(&apps.loose)),
    ];
    let fill = |text: &str| places.iter().fold(text.replace("\\n", "\n"), |t, (k, v)| t.replace(k, v));

    let session = "replay-pre-tool";
    let (mut hints, mut blocks, mut bytes) = (0usize, 0usize, 0usize);
    let calls = tool_calls();
    for call in &calls {
        let cwd: &Path = match call.cwd.as_str() {
            "ws" => &s.ws,
            "home" => &s.home,
            "mapped" => &apps.mapped,
            "plain" => &apps.plain,
            "cached" => &apps.cached,
            other => panic!("unknown cwd {other:?}"),
        };
        let input = fill(&call.input);
        let (tool, json) = match call.tool.as_str() {
            "Bash" | "PowerShell" => (call.tool.clone(), serde_json::json!({ "command": input })),
            "ctx_batch" => (
                "mcp__plugin_context-mode_context-mode__ctx_batch_execute".to_string(),
                serde_json::json!({ "commands": [{ "label": "corpus", "command": input }] }),
            ),
            "Write" | "Edit" => {
                let (path, content) = input.split_once(" :: ").unwrap_or_else(|| panic!("not '<path> :: <content>': {input}"));
                std::fs::create_dir_all(Path::new(path).parent().expect("a folder")).expect("the file's folder");
                let json = if call.tool == "Write" {
                    serde_json::json!({ "file_path": path, "content": content })
                } else {
                    serde_json::json!({ "file_path": path, "old_string": "", "new_string": content })
                };
                (call.tool.clone(), json)
            }
            other => panic!("unknown tool {other:?}"),
        };
        let (code, stdout, stderr) = run_pre_tool_use_at(&s, cwd, &tool, json, session, &[]);
        assert_eq!(code, 0, "{}: the pre-tool hook failed: {stderr}", call.input);
        let out = match stdout.trim() {
            "" => String::new(),
            json => serde_json::from_str::<serde_json::Value>(json).expect("the JSON envelope")["hookSpecificOutput"]
                ["additionalContext"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        };
        bytes += out.len();
        let hint = out.split("<ast-hint>").nth(1).map(|h| h.split("</ast-hint>").next().unwrap_or(h));
        let ids: Vec<&str> = out
            .split("<standards")
            .nth(1)
            .map(|b| {
                b.lines()
                    .filter_map(|l| l.trim().split_once(". [").and_then(|(_, rest)| rest.split(" · ").next()))
                    .collect()
            })
            .unwrap_or_default();
        hints += usize::from(hint.is_some());
        blocks += usize::from(!ids.is_empty());
        let (kind, arg) = call.expect.split_once(':').unwrap_or((call.expect.as_str(), ""));
        let what = format!("{} ({}):\n{out}", call.input, call.expect);
        match kind {
            "none" => assert!(hint.is_none() && ids.is_empty(), "expected nothing: {what}"),
            "query" | "target" | "file" => {
                let h = hint.unwrap_or_else(|| panic!("expected a hint: {what}"));
                let mode = if kind == "file" { "--file" } else { "--contains" };
                assert!(h.contains(&format!("{mode} \"{arg}\"")), "expected {mode} {arg:?}: {what}");
                assert_eq!(h.contains("--target \""), kind == "target", "--target only when the map is not the cwd's: {what}");
            }
            "generic" => {
                let h = hint.unwrap_or_else(|| panic!("expected a hint: {what}"));
                assert!(h.contains("Try `base ast query` for code navigation.") && !h.contains("--contains \""), "{what}");
            }
            "nomap" => {
                let h = hint.unwrap_or_else(|| panic!("expected a hint: {what}"));
                let line = if arg == "never" { "never maps it automatically" } else { "building one in the background" };
                assert!(h.contains("No code map covers") && h.contains(line), "{what}");
            }
            "standards" => {
                assert!(hint.is_none(), "a write carries no AST hint: {what}");
                if arg == "none" {
                    assert!(ids.is_empty(), "expected no standards: {what}");
                } else {
                    for id in arg.split(',') {
                        assert!(ids.contains(&id), "expected {id}: {what}");
                    }
                }
            }
            other => panic!("unknown expectation {other:?}"),
        }
    }
    println!("replay pre-tool: {} calls, {hints} with an AST hint, {blocks} with a standards block, {bytes} bytes added", calls.len());
}

/// BO-08 (F27): inside base's own headless calls (`BASE_HEADLESS`, which every `claude -p` base starts carries), base's
/// hooks print nothing and write nothing. Every corpus store shape gets a session start, and every corpus prompt a
/// prompt hook, a tool call and a stop, all with the calling session's relay title and terminal tab inherited, the way
/// a `base graph extract` started from a session's Bash tool runs them. Measured on Chris's store before the fix: one
/// small extract wrote hook log rows and registered relay sessions (FINAL STATE in the BO-08 doc).
#[test]
fn replay_headless_calls_leave_no_trace() {
    let inherited = [
        ("BASE_HEADLESS", "1"),
        ("BASE_RELAY_AS", "seed-kite"),
        ("WT_SESSION", "0f1e2d3c-tab"),
        ("CLAUDE_CODE_SESSION_ID", "headless-child"),
    ];
    let quiet = |s: &seed::Seed, event: &str, payload: serde_json::Value, what: &str| {
        let (code, stdout, stderr) = seed::run_hook(s, event, &payload, &inherited);
        assert_eq!((code, stdout.as_str(), stderr.as_str()), (0, "", ""), "{what}: {event} under the marker");
    };
    let mut runs = 0;
    for case in cases() {
        let s = write_case_as(&case, &format!("headless-{}", case.name));
        let root = s.ws.parent().expect("seed root").to_path_buf();
        let before = seed::files_under(&root);
        let cwd = s.ws.display().to_string();
        quiet(
            &s,
            "session-start",
            serde_json::json!({ "cwd": cwd, "hook_event_name": "SessionStart", "source": "startup", "session_id": "headless-child" }),
            &case.name,
        );
        runs += 1;
        assert_eq!(seed::files_under(&root), before, "[{}] a session start under the marker wrote to the store", case.name);
    }
    let s = seed::write(&root("headless-prompts"), &seed::TINY, &fixture("base.toml"));
    std::fs::write(s.ws.join(".base").join("domains.toml"), fixture("domains.toml")).expect("domains.toml");
    let top = s.ws.parent().expect("seed root").to_path_buf();
    let before = seed::files_under(&top);
    let cwd = s.ws.display().to_string();
    for prompt in prompts() {
        let session = serde_json::json!("headless-child");
        quiet(&s, "user-prompt-submit", serde_json::json!({ "cwd": cwd, "hook_event_name": "UserPromptSubmit", "prompt": prompt, "session_id": session }), &prompt);
        quiet(&s, "pre-tool-use", serde_json::json!({ "cwd": cwd, "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": { "command": "grep -rn relay src" }, "session_id": session }), &prompt);
        quiet(&s, "post-tool-use", serde_json::json!({ "cwd": cwd, "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": { "command": "grep -rn relay src" }, "tool_response": {}, "session_id": session }), &prompt);
        quiet(&s, "stop", serde_json::json!({ "cwd": cwd, "hook_event_name": "Stop", "stop_hook_active": false, "session_id": session }), &prompt);
        runs += 4;
    }
    assert_eq!(seed::files_under(&top), before, "a prompt, tool or stop hook under the marker wrote to the store");
    assert!(runs >= 4 + 4 * 30, "control: the corpus was driven through: {runs} hook runs");
    println!("replay headless: {runs} hook runs under BASE_HEADLESS, 0 bytes printed, 0 files written");
}

/// BO-09 (F25e, F23b, F23c; build rule 13), on a corpus store: `project list` advises only flags its commands take,
/// every next step shows its age, doctor flags each undated step once, and an update reaches the corpus projects,
/// which are filed under the seed's graph (`graph/ws/seed`) rather than the workspace folder's (`graph/ws/ws`). That
/// last write was a silent no-op before 0.16.0. The corpus's projects are the same in every case (`seed::REAL`), so
/// the first case carries the check.
#[test]
fn replay_project_records_hold_on_the_corpus_store() {
    let case = cases().into_iter().next().expect("a corpus case");
    let s = write_case_as(&case, "bo09-projects");
    let run = |args: &[&str]| {
        let (code, out, err) = run_base(&s, args);
        assert!(code == 0 || args[0] == "doctor", "base {args:?}: {err}");
        out
    };

    let list = run(&["project", "list"]);
    let mut advised = 0;
    for cmd in list.split('`').skip(1).step_by(2).filter(|c| c.starts_with("base project ")) {
        let words: Vec<&str> = cmd.split_whitespace().collect();
        let help = run(&["project", words[2], "--help"]);
        for flag in words.iter().filter(|w| w.starts_with("--")) {
            assert!(help.contains(flag), "`{cmd}` advises {flag}, which `base project {}` does not take:\n{help}", words[2]);
            advised += 1;
        }
    }
    assert!(advised >= 3, "control: the list's advice was read ({advised} flags):\n{list}");

    let rows: Vec<&str> = list.lines().filter(|l| l.starts_with("| Project ")).collect();
    assert_eq!(rows.len(), seed::REAL.projects, "{list}");
    assert!(rows.iter().all(|r| r.contains(" (undated) |")), "every corpus step predates 0.16.0:\n{list}");

    let doctor = run(&["doctor"]);
    for i in 0..seed::REAL.projects {
        assert_eq!(doctor.matches(&format!("project project-{i:02}: next step undated")).count(), 1, "project-{i:02}:\n{doctor}");
    }

    run(&["project", "update", "project-00", "--next-action", "ship slice 0 again"]);
    let list = run(&["project", "list"]);
    assert!(list.lines().any(|l| l.starts_with("| Project 00 |") && l.contains("ship slice 0 again (0 days)")), "{list}");
    let doctor = run(&["doctor"]);
    assert!(!doctor.contains("project project-00:"), "a rewritten step is not flagged:\n{doctor}");
    println!("replay project records: {advised} advised flags exist, {} steps aged, 1 update landed", rows.len());
}

/// BO-10 (topic P, D1, D13; build rule 13), on a corpus store. Three corpus projects get folders, project-01 nested in
/// project-00, and a topic domain sits on all of `Documents` as F29's operators had it. A tool call on a project's
/// file brings that project's rules and no sibling's, and the broad trigger never reaches into a project; a nested
/// child brings its parent after its own block. Doctor names the broad trigger until `base domain paths` and a
/// reviewed list narrow it, then names none, and the injection is unchanged.
#[test]
fn replay_file_scoped_injection_holds_on_the_corpus_store() {
    let case = cases().into_iter().next().expect("a corpus case");
    let s = write_case_as(&case, "bo10-file-scoped");
    let run = |args: &[&str]| {
        let (code, out, err) = run_base(&s, args);
        assert!(code == 0 || args[0] == "doctor", "base {args:?}: {err}");
        out
    };
    let slash = |p: &Path| p.display().to_string().replace('\\', "/");
    let docs = s.ws.join("Documents");
    let folders = [("project-00", "p00"), ("project-01", "p00/p01"), ("project-02", "p02")];
    for (slug, rel) in folders {
        let dir = docs.join(rel);
        std::fs::create_dir_all(&dir).expect("a project folder");
        std::fs::write(dir.join("notes.md"), "x\n").expect("a project file");
        run(&["project", "update", slug, "--path", &slash(&dir)]);
    }
    std::fs::write(docs.join("loose.md"), "x\n").expect("a file in no project");
    run(&["project", "update", "project-01", "--parent", "project-00", "--nested", "true"]);
    for (slug, rel) in folders {
        run(&["domain", "add-trigger", "--domain", slug, "--path", &slash(&docs.join(rel))]);
        run(&["rule", "add", "--domain", slug, "--text", &format!("{slug} corpus rule")]);
    }
    let toml_path = s.ws.join(".base").join("domains.toml");
    let mut domains = std::fs::read_to_string(&toml_path).expect("domains.toml");
    domains.push_str("\n[[domain]]\nname = \"corpus-docs\"\npaths = [\"Documents\"]\nrules = [\"corpus-docs rule\"]\n");
    std::fs::write(&toml_path, domains).expect("domains.toml");

    let read = |file: &Path, session: &str| -> String {
        let (code, stdout, stderr) = run_pre_tool_use(&s, "Read", serde_json::json!({ "file_path": slash(file) }), session, &[]);
        assert_eq!(code, 0, "{stderr}");
        match stdout.trim() {
            "" => String::new(),
            json => serde_json::from_str::<serde_json::Value>(json).expect("the JSON envelope")["hookSpecificOutput"]
                ["additionalContext"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        }
    };
    let check = |leg: &str| {
        let out = read(&docs.join("p02/notes.md"), &format!("bo10-{leg}-p02"));
        assert!(out.contains("[FILE MATCH: project-02]\n  0. project-02 corpus rule"), "{leg}: {out}");
        assert!(!out.contains("project-00 corpus rule") && !out.contains("corpus-docs rule"), "{leg}: reached into p02: {out}");
        let out = read(&docs.join("p00/p01/notes.md"), &format!("bo10-{leg}-p01"));
        let child = out.find("[FILE MATCH: project-01]").unwrap_or_else(|| panic!("{leg}: {out}"));
        let parent = out.find("[FILE MATCH: project-00 (parent of project-01)]").unwrap_or_else(|| panic!("{leg}: {out}"));
        assert!(child < parent && !out.contains("project-02 corpus rule"), "{leg}: {out}");
        // A file no project holds: no project's rules; the topic domain's (its broad trigger before, the file after).
        let out = read(&docs.join("loose.md"), &format!("bo10-{leg}-loose"));
        for slug in ["project-00", "project-01", "project-02"] {
            assert!(!out.contains(&format!("{slug} corpus rule")), "{leg}: {slug} on a loose file: {out}");
        }
        assert!(out.contains("[FILE MATCH: corpus-docs]"), "{leg}: {out}");
    };

    check("before");
    let doctor = run(&["doctor"]);
    assert!(doctor.contains("path trigger `Documents` on `corpus-docs` holds 3 registered projects"), "{doctor}");
    let list = s.ws.join("bo10-paths.toml");
    let suggested = run(&["domain", "paths", "--suggest", "--out", &slash(&list)]);
    assert!(suggested.contains("corpus-docs: `Documents` holds 3 registered projects"), "listed for review, never guessed:\n{suggested}");
    // The reviewer's decision: the topic domain is about the one loose file.
    let mut reviewed = std::fs::read_to_string(&list).expect("the list");
    reviewed.push_str(&format!(
        "\n[[domain]]\ntier = \"workspace\"\nname = \"corpus-docs\"\npaths = [{}]\n",
        toml::Value::String(slash(&docs.join("loose.md")))
    ));
    std::fs::write(&list, reviewed).expect("the reviewed list");
    run(&["domain", "paths", "--apply", &slash(&list)]);
    let doctor = run(&["doctor"]);
    assert!(!doctor.contains("path trigger `"), "no broad trigger left:\n{doctor}");
    check("after");
    println!("replay file-scoped injection: 3 projects, 1 nested, broad trigger named then narrowed, 6 tool calls checked");
}

/// BO-24 (build rule 13), on a corpus store: `project rename` moves a corpus project's records in both tiers (its
/// handoffs' `project` fields too: the corpus files one handoff in five in the global tier), keeps the old name as an
/// alias, edits domains.toml by two lines and leaves doctor's counts as they were. Its preview writes nothing.
#[test]
fn replay_project_rename_holds_on_the_corpus_store() {
    let case = cases().into_iter().next().expect("a corpus case");
    let s = write_case_as(&case, "bo24-rename");
    let run = |args: &[&str]| {
        let (code, out, err) = run_base(&s, args);
        assert!(code == 0 || args[0] == "doctor", "base {args:?}: {err}");
        (out, err)
    };
    // The records a real project carries: a domain with its name as a keyword, a decision, a rule.
    run(&["domain", "add-trigger", "--domain", "project-00", "--keyword", "project-00"]);
    run(&["decision", "log", "--domain", "project-00", "--decision", "corpus decision", "--rationale", "r"]);
    run(&["rule", "add", "--domain", "project-00", "--text", "project-00 corpus rule"]);
    let (ws, gbl, toml) = (
        s.ws.join(".base").join("graph.nq"),
        s.home.join(".base-gbl").join(".base").join("graph.nq"),
        s.ws.join(".base").join("domains.toml"),
    );
    let bytes = |p: &Path| std::fs::read(p).unwrap_or_default();
    let text = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
    let fields = |name: &str| -> (usize, usize) {
        let f = format!("<{}project> \"{name}\"", seed::NS);
        (text(&ws).matches(&f).count(), text(&gbl).matches(&f).count())
    };
    let orphans = |json: &str| -> usize {
        let v: serde_json::Value = serde_json::from_str(json).expect("doctor --json");
        v["tiers"].as_array().expect("tiers").iter()
            .flat_map(|t| t["domain_orphans"].as_array().cloned().unwrap_or_default())
            .map(|p| p[1].as_u64().unwrap_or(0) as usize)
            .sum()
    };
    let before = [bytes(&ws), bytes(&gbl), bytes(&toml)];
    let toml_before = text(&toml);
    let handoffs = fields("project-00");
    assert!(handoffs.0 > 0 && handoffs.1 > 0, "control: corpus handoffs name project-00 in both tiers: {handoffs:?}");
    let orphans_before = orphans(&run(&["doctor", "--json"]).0);

    let (preview, _) = run(&["project", "rename", "project-00", "corpus-renamed"]);
    assert!(preview.starts_with("PREVIEW") && preview.contains("(global graph)"), "{preview}");
    assert_eq!([bytes(&ws), bytes(&gbl), bytes(&toml)], before, "the preview wrote");

    run(&["project", "rename", "project-00", "corpus-renamed", "--yes"]);
    let all = text(&ws) + &text(&gbl);
    for old in ["project/project-00>", "domain/project-00>", "decision/project-00.", "rule/project-00/"] {
        assert!(!all.contains(&format!("{}{old}", seed::NS)), "{old} survived the rename");
    }
    assert_eq!(fields("corpus-renamed"), handoffs, "every handoff naming the project follows it, in its own tier");
    assert_eq!(fields("project-00"), (0, 0));
    assert_eq!(
        text(&toml),
        toml_before.replacen("name = \"project-00\"\n", "name = \"corpus-renamed\"\naliases = [\"project-00\"]\n", 1),
        "domains.toml changes by the name line and one alias line"
    );
    assert!(text(&toml).contains("\"project-00\""), "the old name stays a keyword (R5)");
    let (out, err) = run(&["project", "get", "project-00"]);
    assert!(out.starts_with("Project: corpus-renamed") && err.contains("project-00 is now corpus-renamed"), "{out}{err}");
    let (out, _) = run(&["decision", "log", "--domain", "project-00", "--decision", "late corpus decision", "--rationale", "r"]);
    assert!(out.contains("slug: corpus-renamed.late-corpus-decision"), "{out}");
    assert_eq!(orphans(&run(&["doctor", "--json"]).0), orphans_before, "doctor's orphan count moved");
    println!("replay project rename: handoff fields {handoffs:?} (workspace, global) followed, 0 old IDs, orphans {orphans_before} before and after");
}

/// BO-11 (build rule 13), on a corpus store: a create in a named lane archives only that lane's earlier handoff and
/// leaves the corpus's own handoffs on the project open, in both tiers (corpus `project-15` has one in each, written
/// before lanes existed); `unarchive` undoes an archive; and with the home a workspace, as on the measured machine, a
/// create standing in a doc folder inside the global root lands in the home's tier and leaves the global graph's
/// bytes as they were.
#[test]
fn replay_handoff_lanes_and_tiers_hold_on_the_corpus_store() {
    let case = cases().into_iter().next().expect("a corpus case");
    let s = write_case_as(&case, "bo11-lanes");
    let run_at = |cwd: &Path, relay_as: &str, args: &[&str]| -> String {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_base"))
            .args(args)
            .current_dir(cwd)
            .env("BASE_HOME", &s.home)
            .env("BASE_NO_AUTO_UPDATE", "1")
            .env("BASE_AST_NO_SPAWN", "1")
            .env("BASE_RELAY_AS", relay_as)
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("WT_SESSION")
            .env_remove("BASE_HEADLESS")
            .output()
            .expect("the base binary runs");
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "base {args:?}: {stdout}{stderr}");
        stdout.into_owned()
    };
    let (ws, gbl) = (s.ws.join(".base").join("graph.nq"), s.home.join(".base-gbl").join(".base").join("graph.nq"));
    let text = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
    let status_of = |p: &Path, slug: &str| -> Vec<String> {
        let subject = format!("handoff/{slug}> <{}status> \"", seed::NS);
        text(p).lines().filter_map(|l| l.split(&subject).nth(1)?.split('"').next().map(String::from)).collect()
    };
    let docs = s.ws.parent().expect("seed root").join("handoffs");
    let doc = |slug: &str| -> String {
        let path = docs.join(format!("{slug}.md"));
        std::fs::write(&path, format!("# {slug}\n")).expect("doc");
        path.display().to_string()
    };
    let (corpus_g, corpus_w) = (seed::handoff_slug(15, "handoff"), seed::handoff_slug(16, "handoff"));
    assert_eq!(status_of(&gbl, &corpus_g), ["open"], "control: corpus project-15 has an open handoff in the global tier");
    assert_eq!(status_of(&ws, &corpus_w), ["open"], "control: and one in the workspace tier");

    let first = "2026-10-02-2300-agent-a-project-15";
    let out = run_at(&s.ws, "agent-a", &["handoff", "create", "--project", "project-15", "--doc", &doc(first)]);
    assert!(out.contains("archived: nothing (no earlier open handoff by agent-a on project-15)"), "{out}");
    assert!(out.contains(&format!("{corpus_g} (no lane)")) && out.contains(&format!("{corpus_w} (no lane)")), "{out}");
    let second = "2026-10-02-2310-agent-a-project-15";
    let out = run_at(&s.ws, "agent-a", &["handoff", "create", "--project", "project-15", "--doc", &doc(second)]);
    assert!(out.contains(&format!("archived: {first} (workspace tier)")), "{out}");
    assert_eq!((status_of(&gbl, &corpus_g), status_of(&ws, &corpus_w)), (vec!["open".to_string()], vec!["open".to_string()]));

    let out = run_at(&s.ws, "agent-a", &["handoff", "unarchive", first]);
    assert_eq!(out.trim(), format!("unarchived {first} (workspace tier): status archived -> open"));
    assert_eq!(status_of(&ws, first), ["open"]);

    // F22a: the home becomes a workspace, and a session stands in the handoff doc folder inside the global root.
    std::fs::create_dir_all(s.home.join(".base")).expect("home workspace");
    let folder = s.home.join(".base-gbl").join("handoffs");
    std::fs::create_dir_all(&folder).expect("doc folder");
    let gbl_before = std::fs::read(&gbl).expect("global graph");
    let leak = "2026-10-02-2320-agent-b-corpus-new";
    let out = run_at(&folder, "agent-b", &["handoff", "create", "--project", "corpus-new", "--doc", &doc(leak)]);
    assert!(out.contains("registered (slug: 2026-10-02-2320-agent-b-corpus-new)"), "{out}");
    assert_eq!(status_of(&s.home.join(".base").join("graph.nq"), leak), ["open"], "in the home's workspace tier");
    assert!(std::fs::read(&gbl).expect("global graph") == gbl_before, "the global graph was written");
    println!("replay handoff lanes: 2 corpus handoffs on project-15 left open across 2 creates; unarchive undone; a create in the global root's doc folder landed in the home tier");
}

/// BO-12 (build rule 13), on a corpus store: `base doctor --fix` plans and writes nothing; `--fix --yes` leaves no
/// correction naming nothing (the corpus writes one note in four as a correction, none naming a record), moves the
/// seed's legacy `[signal] max_chars`, and cuts the backups to `[graph] keep_backups`. The corpus workspace keeps every
/// quad in `graph/ws/seed` under a folder named `ws`, the shape of a renamed workspace, so `--fix` leaves its records in
/// place rather than moving the whole store out as another workspace's.
#[test]
fn replay_doctor_fix_holds_on_the_corpus_store() {
    let case = cases().into_iter().next().expect("a corpus case");
    let s = write_case_as(&case, "bo12-fix");
    let run = |args: &[&str]| -> String {
        let (code, out, err) = run_base(&s, args);
        assert!(code == 0 || args == ["doctor"], "base {args:?}: {out}{err}");
        out
    };
    let (ws, gbl, toml) = (
        s.ws.join(".base").join("graph.nq"),
        s.home.join(".base-gbl").join(".base").join("graph.nq"),
        s.home.join(".base-gbl").join("base.toml"),
    );
    for i in 0..6u64 {
        let b = s.ws.join(".base").join(format!("graph.nq.bak-compact-2026-09-2{i}-080000"));
        std::fs::write(&b, "<http://x/s> <http://x/p> <http://x/o> .\n").expect("backup");
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs((10 - i) * 86_400);
        std::fs::File::options().write(true).open(&b).and_then(|f| f.set_modified(when)).expect("backup age");
    }
    let bytes = |p: &Path| std::fs::read(p).unwrap_or_default();
    let subjects = || -> std::collections::BTreeSet<String> {
        let g = format!("<{}graph/ws/seed> .", seed::NS);
        std::fs::read_to_string(&ws)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.ends_with(&g))
            .filter_map(|l| l.split('>').next().map(String::from))
            .collect()
    };
    let before = run(&["doctor"]);
    for want in ["correction(s) name nothing", "legacy: [signal] max_chars", "most likely renamed", "keeps 6 backups"] {
        assert!(before.contains(want), "control: doctor reports {want:?} on the corpus:\n{before}");
    }
    let records = subjects();
    let files = [bytes(&ws), bytes(&gbl), bytes(&toml)];

    let plan = run(&["doctor", "--fix"]);
    assert!(plan.contains("plan (nothing changed yet") && plan.contains("left in place"), "{plan}");
    assert_eq!([bytes(&ws), bytes(&gbl), bytes(&toml)], files, "the plan wrote");

    run(&["doctor", "--fix", "--yes"]);
    let after = run(&["doctor"]);
    assert!(!after.contains("name nothing they correct"), "{after}");
    assert!(!after.contains("legacy: [signal] max_chars"), "{after}");
    assert!(after.contains("keeps 3 backup(s)") && !after.contains("more than [graph] keep_backups"), "{after}");
    assert!(after.contains("most likely renamed"), "the corpus workspace's records moved:\n{after}");
    assert_eq!(subjects(), records, "a corpus record left the workspace graph");
    println!("replay doctor --fix: {} corpus records kept in place, corrections and the legacy key repaired, 6 backups cut to 3", records.len());
}
