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

use seed::{run_prompt_submit, run_session_start, units};

/// FS1's bar (tests/deferral_test.rs), which `base.toml` sets as the first-screen limit.
const BAR: usize = 1990;
const SESSION_START_BYTES: usize = 9000;
const PROMPT_BYTES: usize = 4000;

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
    let s = seed::write(&root(&case.name), &REAL_NO_REMINDERS, &fixture("base.toml"));
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
                let (code, stdout, stderr) = run_prompt_submit(&s, &prompt, Some(&format!("replay-{i:02}")));
                assert_eq!(code, 0, "{prompt:?}: the prompt hook failed: {stderr}");
                let record = last_record(&s, "user-prompt-submit");
                PromptRun { prompt, stdout, record }
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
