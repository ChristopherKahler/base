//! Rank 05: deferred state (spec Part C) for handoffs, forks, tasks and milestones.
//!
//! Every test here compiles against rank 05's law-11 commit 1, where the `deferred` subcommands and
//! `base fork show` parse and do nothing and `BaseConfig::defer_days` answers 10 for everything. The
//! red ones fail on an assertion there, never on the build. The guards (D2, D14, D18, FS1) are green
//! at commit 1 by construction and are proven only by the mutation named at each.
//!
//! **Every fixture is written relative to the run.** "Untouched 11 days" is measured from the moment
//! the hook runs, so no fixed date stays at a chosen offset. That is also why these tests stand on
//! [`BARE`]: the shared seed's handoffs, forks, tasks and milestones carry fixed 2026-08 clocks, and a
//! deferral pass would move all of them. Lane 1's `tests/seed/mod.rs` is read, never edited.
//!
//! **`[defer] enabled` is FALSE in code** (lane 3 verdicts, AMENDMENTS B), so every test that expects
//! deferral writes `[defer] enabled = true` into the seed's global `base.toml` through [`ON`]. D18 is
//! the test that pins the default itself.
//!
//! **Tier rules these fixtures obey, both ruled before this file was written.** Tasks, milestones and
//! projects are written to the WORKSPACE tier only: `active_awareness` reads one tier, so a global-tier
//! task never renders at session start (lane 1's item 2). And a notice is asserted on the run where
//! its count changes, never on a second run over an unchanged state, because suppression hashes the
//! block's text and an unchanged block is silent on purpose.
//!
//! **Where a test reads the graph, it reads the FILE.** The tier that holds a record is the claim
//! under test, and only the file on disk can say which tier changed.

mod seed;

use std::path::PathBuf;

use base::config::{BaseConfig, DeferKind};
use seed::{run_base, run_session_start};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// Deferral on. Appended to the seed's global `base.toml`.
const ON: &str = "[defer]\nenabled = true\n";

/// The seed with nothing that carries a clock: no handoffs, forks, tasks, milestones or reminders.
/// Its one project was touched an hour before the seed is written, and projects keep their own
/// engine behind `[protocol] enabled`, which the seed leaves off. Every record asserted on below is a
/// fixture the test wrote itself.
const BARE: seed::Sizes = seed::Sizes {
    open_handoffs: 0,
    archived_handoffs: 0,
    open_forks: 0,
    archived_forks: 0,
    tasks: 0,
    milestones: 0,
    due_reminders: 0,
    ..seed::TINY
};

fn workspace(tag: &str, global_toml: &str) -> seed::Seed {
    let root = std::env::temp_dir().join(format!("base-r05-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    seed::write(&root, &BARE, global_toml)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    Global,
    Workspace,
}

fn graph_file(seed: &seed::Seed, tier: Tier) -> PathBuf {
    match tier {
        Tier::Global => seed.home.join(".base-gbl").join(".base").join("graph.nq"),
        Tier::Workspace => seed.ws.join(".base").join("graph.nq"),
    }
}

/// The named graph the seed writes each tier's quads into.
fn graph_iri(tier: Tier) -> String {
    match tier {
        Tier::Global => format!("{}graph/global/seed", seed::NS),
        Tier::Workspace => format!("{}graph/ws/seed", seed::NS),
    }
}

/// `days` before now, as the store writes a dateTime. A negative `days` is in the future.
fn stamp(days: i64) -> String {
    (chrono::Local::now() - chrono::Duration::days(days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// Append N-Quads to a tier file and prove they landed. A fixture that silently did not land would
/// read as the feature under test being absent.
fn append(seed: &seed::Seed, tier: Tier, quads: &str, marker: &str) {
    let path = graph_file(seed, tier);
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    text.push_str(quads);
    std::fs::write(&path, text).expect("fixture write");
    assert!(
        std::fs::read_to_string(&path)
            .expect("fixture reread")
            .contains(marker),
        "fixture {marker} did not land in {}",
        path.display()
    );
}

struct Q {
    tier: Tier,
    subject: String,
    out: String,
}

impl Q {
    fn new(tier: Tier, subject: &str) -> Self {
        Q {
            tier,
            subject: format!("{}{subject}", seed::NS),
            out: String::new(),
        }
    }
    fn typ(mut self, ty: &str) -> Self {
        self.out.push_str(&format!(
            "<{}> <{RDF_TYPE}> <{}{ty}> <{}> .\n",
            self.subject,
            seed::NS,
            graph_iri(self.tier)
        ));
        self
    }
    fn lit(mut self, p: &str, v: &str) -> Self {
        self.out.push_str(&format!(
            "<{}> <{}{p}> \"{v}\" <{}> .\n",
            self.subject,
            seed::NS,
            graph_iri(self.tier)
        ));
        self
    }
    fn date(mut self, p: &str, v: &str) -> Self {
        self.out.push_str(&format!(
            "<{}> <{}{p}> \"{v}\"^^<{XSD_DATETIME}> <{}> .\n",
            self.subject,
            seed::NS,
            graph_iri(self.tier)
        ));
        self
    }
    fn iri(mut self, p: &str, object: &str) -> Self {
        self.out.push_str(&format!(
            "<{}> <{}{p}> <{}{object}> <{}> .\n",
            self.subject,
            seed::NS,
            seed::NS,
            graph_iri(self.tier)
        ));
        self
    }
}

/// A handoff or fork, in the shape `base handoff create` and `base fork create` write, with its
/// clocks set relative to now. `resurface` is days before now; a negative value is a snooze into
/// the future. `deferred` is `(deferredReason, days before now it was deferred)`.
struct H<'a> {
    slug: &'a str,
    kind: &'a str,
    project: &'a str,
    status: &'a str,
    touched: i64,
    resurface: i64,
    deferred: Option<(&'a str, i64)>,
}

/// Write the handoff and create its doc on disk. Returns the doc's path as stored.
fn add_handoff(seed: &seed::Seed, tier: Tier, h: &H) -> String {
    let docs = seed.ws.parent().expect("seed root").join("handoffs");
    std::fs::create_dir_all(&docs).expect("handoffs dir");
    let doc = docs.join(format!("{}.md", h.slug));
    std::fs::write(&doc, format!("# {}\n\nfixture\n", h.slug)).expect("doc");
    let doc = doc.display().to_string().replace('\\', "/");
    let name = if h.kind == "fork" { h.slug } else { h.project };
    let mut q = Q::new(tier, &format!("handoff/{}", h.slug))
        .typ("Handoff")
        .lit("name", name)
        .lit("project", h.project)
        .lit("handoffDoc", &doc)
        .lit("kind", h.kind)
        .lit("status", h.status)
        .date("createdAt", &stamp(h.touched.max(h.resurface)))
        .date("resurfaceAt", &stamp(h.resurface))
        .date("lastActive", &stamp(h.touched));
    if let Some((why, at)) = h.deferred {
        q = q.lit("deferredReason", why).date("deferredAt", &stamp(at));
    }
    append(seed, tier, &q.out, h.slug);
    doc
}

/// A task in the workspace tier, linked to the seed's one project.
fn add_task(seed: &seed::Seed, slug: &str, status: &str, touched: i64, extra: &[(&str, &str)]) {
    let mut q = Q::new(Tier::Workspace, &format!("task/{slug}"))
        .typ("Task")
        .lit("name", &format!("Fixture {slug}"))
        .lit("status", status)
        .date("lastActive", &stamp(touched));
    for (p, v) in extra {
        q = if p.ends_with("At") {
            q.date(p, v)
        } else {
            q.lit(p, v)
        };
    }
    let link = Q::new(Tier::Workspace, "project/project-00").iri("hasTask", &format!("task/{slug}"));
    append(seed, Tier::Workspace, &format!("{}{}", q.out, link.out), slug);
}

/// A milestone in the workspace tier, linked to the seed's one project.
fn add_milestone(seed: &seed::Seed, slug: &str, status: &str, touched: i64, extra: &[(&str, &str)]) {
    let mut q = Q::new(Tier::Workspace, &format!("milestone/{slug}"))
        .typ("Milestone")
        .lit("name", &format!("Fixture {slug}"))
        .lit("status", status)
        .date("lastActive", &stamp(touched));
    for (p, v) in extra {
        q = if p.ends_with("At") {
            q.date(p, v)
        } else {
            q.lit(p, v)
        };
    }
    let link =
        Q::new(Tier::Workspace, "project/project-00").iri("hasMilestone", &format!("milestone/{slug}"));
    append(seed, Tier::Workspace, &format!("{}{}", q.out, link.out), slug);
}

/// Every literal value of `<subject> <pred>` in one tier file, as written.
fn values(seed: &seed::Seed, tier: Tier, subject: &str, pred: &str) -> Vec<String> {
    let head = format!("<{}{subject}> <{}{pred}> \"", seed::NS, seed::NS);
    std::fs::read_to_string(graph_file(seed, tier))
        .expect("graph file")
        .lines()
        .filter_map(|l| l.strip_prefix(&head))
        .map(|rest| rest.split('"').next().unwrap_or_default().to_string())
        .collect()
}

/// The one status `subject` carries in `tier`. Absent or doubled is a failure, never a reading.
fn status(seed: &seed::Seed, tier: Tier, subject: &str) -> String {
    let found = values(seed, tier, subject, "status");
    assert_eq!(
        found.len(),
        1,
        "{subject} in {tier:?}: expected exactly one status, found {found:?}"
    );
    found[0].clone()
}

/// Both tier files, byte for byte.
fn both_files(seed: &seed::Seed) -> [Vec<u8>; 2] {
    [Tier::Global, Tier::Workspace].map(|t| std::fs::read(graph_file(seed, t)).expect("graph bytes"))
}

fn session_start(seed: &seed::Seed) -> String {
    let (code, stdout, stderr) = run_session_start(seed, None);
    assert_eq!(code, 0, "hooks fail open. stderr: {stderr}");
    assert!(!stdout.is_empty(), "session start printed nothing. stderr: {stderr}");
    stdout
}

/// Run `base`, assert it printed something, and hand back code and stdout. An empty stdout with a
/// zero exit code would satisfy every `!contains` below for the wrong reason.
fn base(seed: &seed::Seed, args: &[&str]) -> (i32, String) {
    let (code, stdout, stderr) = run_base(seed, args);
    assert!(
        !stdout.is_empty(),
        "`base {}` printed nothing (exit {code}). stderr: {stderr}",
        args.join(" ")
    );
    (code, stdout)
}

fn line_with<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
    haystack.lines().find(|l| l.contains(needle))
}

// ── D1 ───────────────────────────────────────────────────────────────────────
fn config(text: &str) -> BaseConfig {
    toml::from_str(text).unwrap_or_else(|e| panic!("{text:?} does not parse: {e}"))
}

/// J4.1 and spec Part H's order: `mode = "global"` overrides every type; `"asset"` (or unset, or
/// unknown) reads the type, then `global_days`, then 10. A fork reads `fork` and never `handoff`:
/// one record type, two keys. A project reads its key, then `[protocol] stale_days` (spec G6).
/// Mutation MD2 (a fork reads `days.handoff`) must redden this and nothing else.
#[test]
fn defer_days_resolve_global_then_type_then_global_days_then_ten() {
    use DeferKind::{Fork, Handoff, Milestone, Project, Task};

    let c = config("");
    for k in [Handoff, Fork, Task, Milestone] {
        assert_eq!(c.defer_days(k), 10, "{k:?} with nothing set");
    }

    let c = config(
        "[defer]\nmode = \"global\"\nglobal_days = 4\n\
         [defer.days]\nhandoff = 7\nfork = 8\ntask = 9\nmilestone = 11\nproject = 12\n",
    );
    for k in [Handoff, Fork, Task, Milestone, Project] {
        assert_eq!(c.defer_days(k), 4, "{k:?}: mode global overrides the type");
    }

    let c = config("[defer]\nmode = \"global\"\n[defer.days]\nfork = 3\n");
    assert_eq!(c.defer_days(Fork), 10, "mode global with no global_days is 10, never the type");

    let c = config("[defer]\nmode = \"asset\"\nglobal_days = 6\n[defer.days]\nfork = 3\n");
    assert_eq!(c.defer_days(Fork), 3, "asset: the type first");
    for k in [Handoff, Task, Milestone] {
        assert_eq!(c.defer_days(k), 6, "{k:?}: asset with no type value reads global_days");
    }

    let c = config("[defer.days]\nhandoff = 2\n");
    assert_eq!(c.defer_days(Handoff), 2, "mode unset reads as asset");
    assert_eq!(c.defer_days(Fork), 10, "a fork never reads the handoff key");
    let c = config("[defer.days]\nfork = 5\n");
    assert_eq!(c.defer_days(Fork), 5);
    assert_eq!(c.defer_days(Handoff), 10, "a handoff never reads the fork key");

    let c = config("[defer]\nmode = \"globl\"\nglobal_days = 4\n[defer.days]\ntask = 2\n");
    assert_eq!(c.defer_days(Task), 2, "an unknown mode reads as asset");

    let c = config("[protocol]\nstale_days = 7\n[defer]\nglobal_days = 6\n");
    assert_eq!(c.defer_days(Project), 7, "asset: an absent project key reads [protocol] stale_days (G6)");
    let c = config("[protocol]\nstale_days = 7\n[defer.days]\nproject = 5\n");
    assert_eq!(c.defer_days(Project), 5, "the project key wins over stale_days");
}

// ── D2 ───────────────────────────────────────────────────────────────────────
/// A hook driven with a JSON payload on stdin, isolated the way `seed::run` isolates: `BASE_HOME` at
/// the seed home, no auto-update, no detached map build, no relay variables from the caller's shell.
fn hook(seed: &seed::Seed, event: &str, payload: &str) -> (i32, String, String) {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_base"))
        .args(["hook", event])
        .current_dir(&seed.ws)
        .env("BASE_HOME", &seed.home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_NO_SPAWN", "1")
        .env_remove("BASE_NO_WAKE_NUDGE")
        .env_remove("BASE_NO_AUTONAME")
        .env_remove("BASE_RELAY_AS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the base binary runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("payload written");
    let out = child.wait_with_output().expect("hook finishes");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// R7's last row: a Read of a deferred handoff's doc moves its `lastActive` and does NOT revive it,
/// and the next session start, with deferral on, leaves it deferred even though its clock is fresh.
/// Handoffs and forks never revive on the clock: only `show` or a new registration brings one back.
///
/// GREEN at commit 1 by construction (nothing revives there). A guard, proven only by mutation MD1:
/// `plan_records` revives a handoff whose `lastActive` is inside the window.
#[test]
fn a_read_of_a_deferred_handoff_moves_its_clock_and_never_revives_it() {
    let seed = workspace("d2", ON);
    let slug = "2026-08-20-0900-heron-read-only-probe";
    let subject = format!("handoff/{slug}");
    let doc = add_handoff(
        &seed,
        Tier::Workspace,
        &H {
            slug,
            kind: "handoff",
            project: "read-only-probe",
            status: "deferred",
            touched: 26,
            resurface: 26,
            deferred: Some(("auto: cold 12d", 14)),
        },
    );
    let before = values(&seed, Tier::Workspace, &subject, "lastActive");
    assert_eq!(before.len(), 1, "control: one lastActive before the Read: {before:?}");

    let payload = serde_json::json!({
        "cwd": seed.ws.display().to_string(),
        "hook_event_name": "PostToolUse",
        "session_id": "r05-d2",
        "tool_name": "Read",
        "tool_input": { "file_path": doc },
        "tool_response": { "type": "text" }
    })
    .to_string();
    let (code, _, stderr) = hook(&seed, "post-tool-use", &payload);
    assert_eq!(code, 0, "hooks fail open. stderr: {stderr}");

    let after = values(&seed, Tier::Workspace, &subject, "lastActive");
    assert_eq!(after.len(), 1, "one lastActive after the Read: {after:?}");
    assert_ne!(
        before, after,
        "control: the Read did not move the clock, so this test proves nothing about revival"
    );
    assert_eq!(status(&seed, Tier::Workspace, &subject), "deferred", "a Read revived it");

    session_start(&seed);
    assert_eq!(
        status(&seed, Tier::Workspace, &subject),
        "deferred",
        "session start revived a handoff on its fresh clock"
    );
}

// ── D3 ───────────────────────────────────────────────────────────────────────
/// C5 across tiers: a handoff untouched 11 days in the workspace tier and a fork untouched 11 days in
/// the GLOBAL tier are both deferred at session start, each in the file that holds it, with the
/// reason and the date written. Their twins, touched a day ago, stay open. Read from a workspace cwd,
/// so the global fork is the case a one-tier pass would miss.
#[test]
fn cold_handoffs_and_forks_are_deferred_in_the_tier_that_holds_them() {
    let seed = workspace("d3", ON);
    let cold_handoff = "2026-09-01-1000-wren-cold-handoff";
    let warm_handoff = "2026-09-14-1000-wren-warm-handoff";
    let cold_fork = "COLD-FORK-SPEC";
    let warm_fork = "WARM-FORK-SPEC";
    for (tier, slug, kind, touched) in [
        (Tier::Workspace, cold_handoff, "handoff", 11),
        (Tier::Workspace, warm_handoff, "handoff", 1),
        (Tier::Global, cold_fork, "fork", 11),
        (Tier::Global, warm_fork, "fork", 1),
    ] {
        add_handoff(
            &seed,
            tier,
            &H {
                slug,
                kind,
                project: slug,
                status: "open",
                touched,
                resurface: touched,
                deferred: None,
            },
        );
    }

    let out = session_start(&seed);

    for (tier, slug) in [(Tier::Workspace, cold_handoff), (Tier::Global, cold_fork)] {
        let subject = format!("handoff/{slug}");
        assert_eq!(status(&seed, tier, &subject), "deferred", "{slug} in {tier:?}\n{out}");
        assert_eq!(
            values(&seed, tier, &subject, "deferredReason"),
            ["auto: cold 11d"],
            "{slug}: the reason names the days"
        );
        assert_eq!(
            values(&seed, tier, &subject, "deferredAt").len(),
            1,
            "{slug}: the date deferred is written"
        );
    }
    for (tier, slug) in [(Tier::Workspace, warm_handoff), (Tier::Global, warm_fork)] {
        assert_eq!(
            status(&seed, tier, &format!("handoff/{slug}")),
            "open",
            "{slug}: touched a day ago and deferred anyway"
        );
    }
    // Written in the tier that holds it, and nowhere else.
    assert!(
        values(&seed, Tier::Global, &format!("handoff/{cold_handoff}"), "status").is_empty(),
        "the workspace handoff's status leaked into the global file"
    );
    assert!(
        !out.contains(cold_fork),
        "a deferred fork is still listed at session start:\n{out}"
    );
}

// ── D4 ───────────────────────────────────────────────────────────────────────
/// The single point the design rests on (R7): "get the handoff for the graph portal thing" brings a
/// deferred handoff back from loose words, prints its doc, says it revived it, and writes exactly
/// the revival: `status "open"`, a fresh `lastActive`, and no `deferredReason` or `deferredAt`.
///
/// And the carve-out ruled 2026-09-15 (verdicts, AMENDMENTS C, conditions 1 and 3): a ONE-match
/// `show` of an OPEN record writes nothing, byte for byte. Mutation MD10, writing `lastActive` on an
/// open match, must redden that arm. MD5 (the resolver back to `open` only) reddens the revival arm.
#[test]
fn show_brings_back_a_deferred_handoff_from_loose_words_and_says_so() {
    let seed = workspace("d4", "");
    let slug = "2026-08-19-1714-kestrel-graph-portal";
    let subject = format!("handoff/{slug}");
    let doc = add_handoff(
        &seed,
        Tier::Workspace,
        &H {
            slug,
            kind: "handoff",
            project: "graph-portal",
            status: "deferred",
            touched: 31,
            resurface: 31,
            deferred: Some(("auto: cold 26d", 5)),
        },
    );
    let open_slug = "2026-09-13-0800-lemur-ledger-audit";
    add_handoff(
        &seed,
        Tier::Global,
        &H {
            slug: open_slug,
            kind: "handoff",
            project: "ledger-audit",
            status: "open",
            touched: 2,
            resurface: 2,
            deferred: None,
        },
    );

    // Condition 1: one match, an OPEN record, nothing written.
    let before = both_files(&seed);
    let (code, out) = base(&seed, &["handoff", "show", open_slug]);
    assert_eq!(code, 0, "control: the open handoff is one match:\n{out}");
    assert!(out.starts_with("doc: "), "control: an answer, not a question:\n{out}");
    assert!(
        before == both_files(&seed),
        "a one-match show of an OPEN handoff wrote to a graph file"
    );
    assert!(!out.contains("revived"), "an open handoff was reported revived:\n{out}");

    // Condition 2: loose words, a DEFERRED record, revived and said so.
    let (code, out) = base(
        &seed,
        &["handoff", "show", "get", "the", "handoff", "for", "the", "graph", "portal", "thing"],
    );
    assert_eq!(out.lines().next(), Some(format!("doc: {doc}").as_str()), "{out}");
    assert!(
        out.contains("matched: 2 of 7 words"),
        "the rule that matched is named:\n{out}"
    );
    let revived = line_with(&out, "revived: ").unwrap_or_else(|| panic!("no revived line:\n{out}"));
    assert_eq!(
        revived, "revived: deferred 5 days ago (auto: cold 26d)",
        "the line says how long it was deferred and why"
    );
    assert_eq!(code, 0, "{out}");

    assert_eq!(status(&seed, Tier::Workspace, &subject), "open", "not revived in its file");
    assert!(
        values(&seed, Tier::Workspace, &subject, "deferredReason").is_empty(),
        "deferredReason survived the revival"
    );
    assert!(
        values(&seed, Tier::Workspace, &subject, "deferredAt").is_empty(),
        "deferredAt survived the revival"
    );
    let touched = values(&seed, Tier::Workspace, &subject, "lastActive");
    assert_eq!(touched.len(), 1, "one lastActive: {touched:?}");
    assert!(
        touched[0].starts_with(&chrono::Local::now().format("%Y-%m-%d").to_string()),
        "the clock was not reset to now, so the next session start defers it again: {touched:?}"
    );
}

// ── D5 ───────────────────────────────────────────────────────────────────────
/// R7 row 2 and C9: `base handoff deferred` numbers its lines D1, D2, and `show D2` brings back the
/// second. A key is a lookup, never a source of truth (flag 5b): a key the listing did not print, or
/// one whose handoff is no longer deferred, is refused with the word "re-list" and writes nothing.
#[test]
fn show_revives_by_deferred_key_and_refuses_a_stale_one() {
    let seed = workspace("d5", "");
    let first = "2026-09-02-1000-otter-first-deferred";
    let second = "2026-08-25-1000-otter-second-deferred";
    for (slug, touched) in [(first, 12), (second, 20)] {
        add_handoff(
            &seed,
            Tier::Workspace,
            &H {
                slug,
                kind: "handoff",
                project: slug,
                status: "deferred",
                touched,
                resurface: touched,
                deferred: Some(("auto: cold 10d", 2)),
            },
        );
    }

    let (code, listing) = base(&seed, &["handoff", "deferred"]);
    assert_eq!(code, 0, "{listing}");
    let keyed = |key: &str| {
        line_with(&listing, &format!("{key} "))
            .unwrap_or_else(|| panic!("no {key} line:\n{listing}"))
            .to_string()
    };
    let d1 = keyed("D1");
    let d2 = keyed("D2");
    let (want, other) = if d2.contains(second) { (second, first) } else { (first, second) };
    assert!(d2.contains(want) && !d2.contains(other), "D2 names one handoff: {d2}");
    assert!(d1.contains(other), "D1 names the other: {d1}");

    let (code, out) = base(&seed, &["handoff", "show", "D2"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("revived: deferred "), "{out}");
    assert_eq!(status(&seed, Tier::Workspace, &format!("handoff/{want}")), "open");
    assert_eq!(
        status(&seed, Tier::Workspace, &format!("handoff/{other}")),
        "deferred",
        "the key revived the wrong handoff, or both"
    );

    // D2 now names a handoff that is open: stale, refused.
    let before = both_files(&seed);
    let (code, out) = base(&seed, &["handoff", "show", "D2"]);
    assert_ne!(code, 0, "a stale key succeeded:\n{out}");
    assert!(out.contains("re-list"), "a stale key does not say re-list:\n{out}");
    // A key the listing never printed: refused.
    let (code, out) = base(&seed, &["handoff", "show", "D9"]);
    assert_ne!(code, 0, "an unknown key succeeded:\n{out}");
    assert!(out.contains("re-list"), "an unknown key does not say re-list:\n{out}");
    assert!(before == both_files(&seed), "a refused key wrote to a graph file");
}

// ── D6 ───────────────────────────────────────────────────────────────────────
/// R7: a question never revives anything. Two deferred handoffs sharing the words asked for are both
/// listed, none is picked, exit 2; no match exits 1. Both graph files stay byte-identical either way.
/// Mutation MD4, `show` reviving the first of several, must redden this.
#[test]
fn several_matches_or_none_revive_nothing() {
    let seed = workspace("d6", "");
    let a = "2026-08-19-1714-kestrel-graph-portal";
    let b = "2026-08-21-1100-heron-graph-portal-v2";
    for slug in [a, b] {
        add_handoff(
            &seed,
            Tier::Workspace,
            &H {
                slug,
                kind: "handoff",
                project: slug,
                status: "deferred",
                touched: 25,
                resurface: 25,
                deferred: Some(("auto: cold 11d", 14)),
            },
        );
    }
    let before = both_files(&seed);

    let (code, out) = base(&seed, &["handoff", "show", "graph", "portal"]);
    assert!(
        out.starts_with("2 handoffs match \"graph portal\" (2 deferred) by 2 of 2 words. None was picked;"),
        "several deferred matches are a question:\n{out}"
    );
    for slug in [a, b] {
        assert!(
            out.lines().any(|l| l.starts_with(&format!("  {slug} · ")) && l.ends_with(" · deferred")),
            "{slug} is not listed as deferred:\n{out}"
        );
    }
    assert!(!out.lines().any(|l| l.starts_with("doc: ")), "a doc was picked:\n{out}");
    assert_eq!(code, 2, "{out}");

    let (code, out) = base(&seed, &["handoff", "show", "zebra", "unicorn"]);
    assert!(out.starts_with("no open handoff matches \"zebra unicorn\"\n"), "{out}");
    assert!(
        out.contains("no deferred handoff matches either"),
        "a miss does not say the deferred ones were searched:\n{out}"
    );
    assert_eq!(code, 1, "{out}");

    assert!(before == both_files(&seed), "a question wrote to a graph file");
    for slug in [a, b] {
        assert_eq!(status(&seed, Tier::Workspace, &format!("handoff/{slug}")), "deferred");
    }
}

// ── D7 ───────────────────────────────────────────────────────────────────────
/// J4.3's third arm and C10: a new handoff registered for a project archives the older DEFERRED one,
/// not only an open one. The behaviour is lane 2's rank 08 `create` (`prior_handoffs_in` matches
/// `open` or `deferred`), merged to `main` in PR #165 and absent from this branch's base `65718ae`,
/// whose `create` archives `open` only. Lane 3 owes the test, never the behaviour.
///
/// Parked: RED on this branch, predicted GREEN at the integration base. It runs there, first.
#[test]
#[ignore = "needs lane 2 rank 08 create (main f433a9a): runs at the integration base only"]
fn a_new_handoff_archives_the_older_deferred_one() {
    let seed = workspace("d7", "");
    let old = "2026-08-30-1000-auk-base-0160";
    add_handoff(
        &seed,
        Tier::Workspace,
        &H {
            slug: old,
            kind: "handoff",
            project: "base-0160",
            status: "deferred",
            touched: 16,
            resurface: 16,
            deferred: Some(("auto: cold 12d", 4)),
        },
    );
    let new_doc = seed.ws.parent().expect("root").join("handoffs").join("2026-09-15-1500-auk-base-0160.md");
    std::fs::write(&new_doc, "# new\n").expect("new doc");
    let (code, out) = base(
        &seed,
        &["handoff", "create", "--project", "base-0160", "--doc", &new_doc.display().to_string()],
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        status(&seed, Tier::Workspace, &format!("handoff/{old}")),
        "archived",
        "the older deferred handoff was left deferred beside the new one:\n{out}"
    );
}

// ── D8 ───────────────────────────────────────────────────────────────────────
/// J4.4 and C7: a handoff snoozed into the future is not deferred while the snooze runs, however long
/// it has gone untouched. Its twin, identical but for the snooze, is the control. Mutation MD6 (the
/// snooze exemption removed) must redden this.
#[test]
fn a_snoozed_handoff_is_not_deferred_while_its_twin_is() {
    let seed = workspace("d8", ON);
    let snoozed = "2026-09-04-1000-lynx-snoozed-twin";
    let plain = "2026-09-04-1000-lynx-plain-twin";
    for (slug, resurface) in [(snoozed, -3), (plain, 11)] {
        add_handoff(
            &seed,
            Tier::Workspace,
            &H {
                slug,
                kind: "handoff",
                project: slug,
                status: "open",
                touched: 11,
                resurface,
                deferred: None,
            },
        );
    }
    session_start(&seed);
    assert_eq!(
        status(&seed, Tier::Workspace, &format!("handoff/{plain}")),
        "deferred",
        "control: the unsnoozed twin was not deferred, so the snooze proves nothing"
    );
    assert_eq!(
        status(&seed, Tier::Workspace, &format!("handoff/{snoozed}")),
        "open",
        "a handoff snoozed into the future was deferred"
    );
}

// ── D9 ───────────────────────────────────────────────────────────────────────
/// K11 as built to its default (flag 10, Chris's): a snooze that ended 2 days ago counts as a touch at
/// its end, so a handoff untouched for 20 days whose snooze just ended is not deferred in the session
/// start it resurfaces in. The twin whose `resurfaceAt` is as old as its clock is deferred. Mutation
/// MD7 (`resurfaceAt` dropped from the clock) must redden this.
#[test]
fn an_ended_snooze_counts_from_its_end() {
    let seed = workspace("d9", ON);
    let ended = "2026-08-26-1000-egret-snooze-ended";
    let never = "2026-08-26-1000-egret-never-snoozed";
    for (slug, resurface) in [(ended, 2), (never, 20)] {
        add_handoff(
            &seed,
            Tier::Workspace,
            &H {
                slug,
                kind: "handoff",
                project: slug,
                status: "open",
                touched: 20,
                resurface,
                deferred: None,
            },
        );
    }
    session_start(&seed);
    assert_eq!(
        status(&seed, Tier::Workspace, &format!("handoff/{never}")),
        "deferred",
        "control: the never-snoozed twin was not deferred"
    );
    assert_eq!(
        status(&seed, Tier::Workspace, &format!("handoff/{ended}")),
        "open",
        "a snooze that ended 2 days ago did not count as a touch"
    );
}

// ── D11 ──────────────────────────────────────────────────────────────────────
/// `YYYY-MM-DD`, `days` before today, as the listings print a date.
fn date_of(days: i64) -> String {
    (chrono::Local::now() - chrono::Duration::days(days)).format("%Y-%m-%d").to_string()
}

/// J4.6, C9 and C10. Each `base <type> deferred` lists only that type's deferred records, one line
/// each, carrying the key (handoffs and forks), the slug, the project, the codename where the slug
/// has one, the days untouched, the date deferred, the reason, and the command that brings it back.
/// `base handoff list` lists every status and gains the `lastActive` column.
#[test]
fn each_deferred_command_lists_only_deferred_and_handoff_list_shows_every_status_with_last_active() {
    let seed = workspace("d11", "");
    let open = "2026-09-14-0900-wren-open-one";
    let parked = "2026-09-01-0900-wren-parked-one";
    let closed = "2026-08-15-0900-wren-closed-one";
    let fork_parked = "PARKED-FORK-SPEC";
    let fork_open = "OPEN-FORK-SPEC";
    for (slug, kind, status, touched, deferred) in [
        (open, "handoff", "open", 1, None),
        (parked, "handoff", "deferred", 15, Some(("auto: cold 11d", 4))),
        (closed, "handoff", "archived", 30, None),
        (fork_parked, "fork", "deferred", 15, Some(("auto: cold 11d", 4))),
        (fork_open, "fork", "open", 1, None),
    ] {
        add_handoff(
            &seed,
            Tier::Workspace,
            &H { slug, kind, project: "listing-project", status, touched, resurface: touched, deferred },
        );
    }
    let at4 = stamp(4);
    let why_at = [("deferredReason", "auto: cold 11d"), ("deferredAt", at4.as_str())];
    add_task(&seed, "task-parked", "deferred", 15, &why_at);
    add_task(&seed, "task-live", "active", 1, &[]);
    add_milestone(&seed, "milestone-parked", "deferred", 15, &why_at);
    add_milestone(&seed, "milestone-live", "active", 1, &[]);
    let project = Q::new(Tier::Workspace, "project/paused-project")
        .typ("Project")
        .lit("name", "Paused Project")
        .lit("status", "deferred")
        .date("lastActive", &stamp(9));
    append(&seed, Tier::Workspace, &project.out, "paused-project");

    let (code, out) = base(&seed, &["handoff", "deferred"]);
    assert_eq!(code, 0, "{out}");
    let line = line_with(&out, parked).unwrap_or_else(|| panic!("the deferred handoff is not listed:\n{out}"));
    for want in [
        "D1 ".to_string(),
        "project listing-project".to_string(),
        "wren".to_string(),
        "untouched 15d".to_string(),
        format!("deferred {}", date_of(4)),
        "auto: cold 11d".to_string(),
        format!("base handoff show {parked}"),
    ] {
        assert!(line.contains(&want), "the line lacks {want:?}: {line}");
    }
    for absent in [open, closed, fork_parked] {
        assert!(!out.contains(absent), "`handoff deferred` listed {absent}:\n{out}");
    }

    let (code, out) = base(&seed, &["fork", "deferred"]);
    assert_eq!(code, 0, "{out}");
    let line = line_with(&out, fork_parked).unwrap_or_else(|| panic!("the deferred fork is not listed:\n{out}"));
    assert!(line.contains(&format!("base fork show {fork_parked}")), "{line}");
    assert!(!out.contains(fork_open) && !out.contains(parked), "{out}");

    for (kind, parked_slug, live_slug) in [
        ("task", "task-parked", "task-live"),
        ("milestone", "milestone-parked", "milestone-live"),
    ] {
        let (code, out) = base(&seed, &[kind, "deferred"]);
        assert_eq!(code, 0, "{out}");
        let line = line_with(&out, parked_slug)
            .unwrap_or_else(|| panic!("`{kind} deferred` does not list {parked_slug}:\n{out}"));
        assert!(
            line.contains(&format!("base {kind} update {parked_slug} --status active")),
            "no revive command: {line}"
        );
        assert!(!out.contains(live_slug), "`{kind} deferred` listed a live one:\n{out}");
    }
    let (code, out) = base(&seed, &["project", "deferred"]);
    assert_eq!(code, 0, "{out}");
    let line = line_with(&out, "paused-project").unwrap_or_else(|| panic!("{out}"));
    assert!(line.contains("no reason recorded"), "an operator deferral names no reason: {line}");

    let (code, out) = base(&seed, &["handoff", "list"]);
    assert_eq!(code, 0, "{out}");
    let header = out.lines().next().unwrap_or_default();
    assert!(header.contains("lastActive"), "no lastActive column: {header}");
    for (slug, status) in [(open, "open"), (parked, "deferred"), (closed, "archived")] {
        let line = line_with(&out, slug).unwrap_or_else(|| panic!("`handoff list` lost {slug}:\n{out}"));
        assert!(line.contains(&format!("| {status} |")), "{slug} is not shown as {status}: {line}");
    }
}

// ── D12 ──────────────────────────────────────────────────────────────────────
/// The first deferral pass would otherwise print every auto-deferred record into `[Resurface]`,
/// because each carries a `resurfaceAt` in the past (G0.1 row 14). An auto-deferred record stays out;
/// an operator's own "defer until" still shows, which is also this test's control against a scan that
/// simply never runs. Both fixtures carry `name`, which the scan requires, or neither would reach it.
/// Mutation MD3 (the filter placed outside `GRAPH ?g`) must redden this.
#[test]
fn auto_deferred_records_stay_out_of_resurface_and_an_operator_deferral_still_shows() {
    let seed = workspace("d12", "[flow]\nenabled = true\nresurface = true\n");
    add_handoff(
        &seed,
        Tier::Workspace,
        &H {
            slug: "2026-08-20-1000-auk-auto-parked-thing",
            kind: "handoff",
            project: "auto-parked-thing",
            status: "deferred",
            touched: 26,
            resurface: 20,
            deferred: Some(("auto: cold 11d", 15)),
        },
    );
    let project = Q::new(Tier::Workspace, "project/client-portal-rebuild")
        .typ("Project")
        .lit("name", "Client Portal Rebuild")
        .lit("status", "deferred")
        .lit("deferredReason", "until the client signs")
        .date("resurfaceAt", &stamp(2))
        .date("lastActive", &stamp(30));
    append(&seed, Tier::Workspace, &project.out, "client-portal-rebuild");

    let out = session_start(&seed);
    assert!(
        out.contains("- Client Portal Rebuild (deferred until "),
        "control: the operator's own deferral is not in [Resurface], so the scan did not run:\n{out}"
    );
    assert!(
        !out.contains("- auto-parked-thing (deferred until "),
        "an auto-deferred record was resurfaced:\n{out}"
    );
}

// ── D13 ──────────────────────────────────────────────────────────────────────
/// C8, B2 and B3 on the run where every count changes. Each block ends with its notice; HANDOFFS,
/// with 0 open and 2 deferred, still renders its title line so the notice has somewhere to be; line 1
/// carries `· deferred 7`; the instruction block carries the one deferred line. Mutation MD13 (a
/// block with 0 open dropped again) must redden this.
#[test]
fn each_block_ends_with_its_notice_and_a_block_with_zero_open_still_renders() {
    let seed = workspace("d13", ON);
    let parked = Some(("auto: cold 12d", 3));
    for (tier, slug, kind, status, touched, deferred) in [
        (Tier::Workspace, "2026-09-01-1000-kite-parked-a", "handoff", "deferred", 15, parked),
        (Tier::Global, "2026-09-01-1000-kite-parked-b", "handoff", "deferred", 15, parked),
        (Tier::Workspace, "LIVE-FORK-SPEC", "fork", "open", 1, None),
        (Tier::Global, "PARKED-FORK-SPEC", "fork", "deferred", 15, parked),
    ] {
        add_handoff(
            &seed,
            tier,
            &H { slug, kind, project: slug, status, touched, resurface: touched, deferred },
        );
    }
    let at3 = stamp(3);
    let why_at = [("deferredReason", "auto: cold 12d"), ("deferredAt", at3.as_str())];
    add_task(&seed, "task-live", "active", 1, &[]);
    add_task(&seed, "task-parked-1", "deferred", 15, &why_at);
    add_task(&seed, "task-parked-2", "deferred", 15, &why_at);
    add_milestone(&seed, "milestone-parked", "deferred", 15, &why_at);
    let project = Q::new(Tier::Workspace, "project/paused-project")
        .typ("Project")
        .lit("name", "Paused Project")
        .lit("status", "deferred")
        .date("lastActive", &stamp(9));
    append(&seed, Tier::Workspace, &project.out, "paused-project");

    let out = session_start(&seed);
    let notices = [
        ("HANDOFFS (0 open", "2 handoffs are marked deferred: they are open, but paused. Run base handoff deferred to list all deferred."),
        ("FORKS (1 open", "1 fork is marked deferred: it is open, but paused. Run base fork deferred to list all deferred."),
        ("PROJECTS (", "1 project is marked deferred: it is open, but paused. Run base project deferred to list all deferred."),
        ("TASKS (", "2 tasks are marked deferred: they are open, but paused. Run base task deferred to list all deferred."),
        ("MILESTONES (", "1 milestone is marked deferred: it is open, but paused. Run base milestone deferred to list all deferred."),
    ];
    for (title, notice) in notices {
        let lines: Vec<&str> = out.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.starts_with(title))
            .unwrap_or_else(|| panic!("no block titled {title:?}:\n{out}"));
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.is_empty() || !l.starts_with("  "))
            .map_or(lines.len(), |i| start + 1 + i);
        assert_eq!(
            lines[end - 1].trim_start(),
            notice,
            "block {title:?} does not end with its notice:\n{}",
            lines[start..end].join("\n")
        );
    }
    let line1 = out.lines().next().unwrap_or_default();
    assert!(line1.contains(" · deferred 7 · "), "line 1 lacks the deferred total: {line1}");
    assert!(
        out.contains("Deferred = open but paused, not listed; each block counts them. Bring one back: `base handoff show <words>` (forks: `base fork show`)."),
        "the instruction block lacks the deferred line:\n{out}"
    );
}

// ── D14 ──────────────────────────────────────────────────────────────────────
/// C10: `base handoff archive` works on a deferred handoff, in the tier that holds it. GREEN at
/// commit 1, because `archive` already rewrites any status. A guard, proven by mutation MD9:
/// `archive` requires `status "open"`.
#[test]
fn a_deferred_handoff_archives() {
    let seed = workspace("d14", "");
    let slug = "2026-08-28-1000-plover-parked-for-good";
    add_handoff(
        &seed,
        Tier::Global,
        &H {
            slug,
            kind: "handoff",
            project: slug,
            status: "deferred",
            touched: 18,
            resurface: 18,
            deferred: Some(("auto: cold 10d", 8)),
        },
    );
    let (code, out) = base(&seed, &["handoff", "archive", slug]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("archived (global tier)"), "{out}");
    assert_eq!(status(&seed, Tier::Global, &format!("handoff/{slug}")), "archived");
}

// ── D15 ──────────────────────────────────────────────────────────────────────
/// Flag 4: `base fork show` is `handoff show`'s resolver with the kind set to fork. It finds a fork by
/// its title and brings a deferred one back; on an open fork it writes nothing (AMENDMENTS C, the
/// same carve-out). The kinds never cross: `handoff show` on the shared project name answers with the
/// continuity handoff, never the fork.
#[test]
fn fork_show_finds_a_fork_by_title_and_revives_a_deferred_one() {
    let seed = workspace("d15", "");
    let parked = "GRAPH-PORTAL-FORK";
    let live = "LEDGER-AUDIT-FORK";
    let handoff = "2026-09-12-1000-kestrel-graph-portal";
    let parked_doc = add_handoff(
        &seed,
        Tier::Global,
        &H {
            slug: parked,
            kind: "fork",
            project: "graph-portal",
            status: "deferred",
            touched: 20,
            resurface: 20,
            deferred: Some(("auto: cold 10d", 10)),
        },
    );
    add_handoff(
        &seed,
        Tier::Workspace,
        &H { slug: live, kind: "fork", project: "ledger-audit", status: "open", touched: 1, resurface: 1, deferred: None },
    );
    let handoff_doc = add_handoff(
        &seed,
        Tier::Workspace,
        &H { slug: handoff, kind: "handoff", project: "graph-portal", status: "open", touched: 3, resurface: 3, deferred: None },
    );

    let before = both_files(&seed);
    let (code, out) = base(&seed, &["fork", "show", live]);
    assert_eq!(code, 0, "control: the open fork is one match:\n{out}");
    assert!(out.starts_with("doc: "), "{out}");
    assert!(before == both_files(&seed), "a one-match show of an OPEN fork wrote to a graph file");

    let (code, out) = base(&seed, &["handoff", "show", "graph-portal"]);
    assert_eq!(
        out.lines().next(),
        Some(format!("doc: {handoff_doc}").as_str()),
        "`handoff show` crossed into forks:\n{out}"
    );
    assert_eq!(code, 0, "{out}");

    let (code, out) = base(&seed, &["fork", "show", parked]);
    assert_eq!(out.lines().next(), Some(format!("doc: {parked_doc}").as_str()), "{out}");
    assert!(out.contains("fork: GRAPH-PORTAL-FORK · project graph-portal · global tier · "), "{out}");
    assert!(out.contains("revived: deferred 10 days ago (auto: cold 10d)"), "{out}");
    assert_eq!(code, 0, "{out}");
    assert_eq!(status(&seed, Tier::Global, &format!("handoff/{parked}")), "open");
}

// ── D16 ──────────────────────────────────────────────────────────────────────
/// Tasks and milestones defer AND revive on the clock, because their `lastActive` moves only on a
/// deliberate base command. A cold task and milestone are deferred; an auto-deferred one touched a day
/// ago comes back to `active` with its reason and date deleted. An OPERATOR's deferral, one whose
/// reason is not `auto:`, is never revived by the clock: `task update --status deferred` itself stamps
/// `lastActive`, so reviving on the clock would undo the operator's own act at the next session
/// start. Terminal, blocked and dated tasks are never deferred. Mutation MD12 (revival ignores the
/// `auto:` reason) must redden the operator arm.
#[test]
fn cold_tasks_and_milestones_defer_a_deliberate_touch_revives_them_and_an_operator_deferral_stays() {
    let seed = workspace("d16", ON);
    let at5 = stamp(5);
    let auto = [("deferredReason", "auto: cold 12d"), ("deferredAt", at5.as_str())];
    add_task(&seed, "task-cold", "active", 11, &[]);
    add_task(&seed, "task-touched", "deferred", 1, &auto);
    add_task(&seed, "task-operator", "deferred", 1, &[("deferredReason", "until Q4 planning")]);
    add_task(&seed, "task-done", "completed", 30, &[]);
    add_task(&seed, "task-blocked", "blocked", 30, &[]);
    add_task(&seed, "task-dated", "active", 30, &[("due", "2026-10-01")]);
    add_milestone(&seed, "milestone-cold", "active", 11, &[]);
    add_milestone(&seed, "milestone-touched", "deferred", 1, &auto);

    session_start(&seed);
    let ws = Tier::Workspace;
    for subject in ["task/task-cold", "milestone/milestone-cold"] {
        assert_eq!(status(&seed, ws, subject), "deferred", "{subject} is cold and was not deferred");
        assert_eq!(values(&seed, ws, subject, "deferredReason"), ["auto: cold 11d"], "{subject}");
    }
    for subject in ["task/task-touched", "milestone/milestone-touched"] {
        assert_eq!(status(&seed, ws, subject), "active", "{subject} was touched and not revived");
        assert!(values(&seed, ws, subject, "deferredReason").is_empty(), "{subject}: reason kept");
        assert!(values(&seed, ws, subject, "deferredAt").is_empty(), "{subject}: date kept");
    }
    assert_eq!(status(&seed, ws, "task/task-operator"), "deferred", "the clock undid an operator's deferral");
    for (subject, kept) in [
        ("task/task-done", "completed"),
        ("task/task-blocked", "blocked"),
        ("task/task-dated", "active"),
    ] {
        assert_eq!(status(&seed, ws, subject), kept, "{subject} must never be deferred");
    }
}

// ── D17 ──────────────────────────────────────────────────────────────────────
/// `base reconcile --dry-run` reports every type's plan, by tier, and writes nothing. It plans with
/// deferral OFF, as the project dry run always has, so an operator can preview before turning it on.
#[test]
fn reconcile_dry_run_plans_every_type_by_tier_and_writes_nothing() {
    let seed = workspace("d17", "");
    let cold = "2026-09-02-1000-dunlin-cold-for-preview";
    add_handoff(
        &seed,
        Tier::Workspace,
        &H { slug: cold, kind: "handoff", project: cold, status: "open", touched: 11, resurface: 11, deferred: None },
    );
    add_handoff(
        &seed,
        Tier::Global,
        &H { slug: "WARM-PREVIEW-FORK", kind: "fork", project: "p", status: "open", touched: 1, resurface: 1, deferred: None },
    );
    let at4 = stamp(4);
    add_task(&seed, "task-back-soon", "deferred", 1, &[("deferredReason", "auto: cold 10d"), ("deferredAt", at4.as_str())]);

    let before = both_files(&seed);
    let (code, out) = base(&seed, &["reconcile", "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.lines().any(|l| l.contains("workspace tier") && l.contains("handoff") && l.contains("WOULD DEFER") && l.contains(cold)),
        "no WOULD DEFER line for the cold handoff:\n{out}"
    );
    assert!(
        out.lines().any(|l| l.contains("workspace tier") && l.contains("task") && l.contains("WOULD REVIVE") && l.contains("task-back-soon")),
        "no WOULD REVIVE line for the touched task:\n{out}"
    );
    assert!(
        !out.lines().any(|l| l.contains("WOULD") && l.contains("WARM-PREVIEW-FORK")),
        "a warm fork was planned:\n{out}"
    );
    assert!(before == both_files(&seed), "a dry run wrote to a graph file");
}

// ── D18 ──────────────────────────────────────────────────────────────────────
/// AMENDMENTS B: `[defer] enabled` is false in code, so an install that never opts in keeps today's
/// behaviour, which is what keeps every seed-driven session start test in the suite as it was. The
/// same fixtures with deferral ON are the control: without that arm, a pass that never runs at all
/// would pass this test. Mutation MD11 (the gate ignored) must redden the OFF arm.
#[test]
fn nothing_defers_unless_defer_is_enabled() {
    for (tag, toml, want) in [("d18-off", "", "open"), ("d18-on", ON, "deferred")] {
        let seed = workspace(tag, toml);
        let slug = "2026-08-16-1000-gadwall-cold-thirty";
        add_handoff(
            &seed,
            Tier::Workspace,
            &H { slug, kind: "handoff", project: slug, status: "open", touched: 30, resurface: 30, deferred: None },
        );
        add_task(&seed, "task-cold-thirty", "active", 30, &[]);
        session_start(&seed);
        let task_want = if want == "open" { "active" } else { "deferred" };
        assert_eq!(status(&seed, Tier::Workspace, &format!("handoff/{slug}")), want, "{tag}");
        assert_eq!(status(&seed, Tier::Workspace, "task/task-cold-thirty"), task_want, "{tag}");
    }
}

// ── FS1 ──────────────────────────────────────────────────────────────────────
/// A5 with lane 3's lines, on the case nobody has measured: ten letters of 50-character slugs, the
/// longest DUE NOW lines (one past the day-8 warning), and deferred records so the B3 line and the
/// header's `· deferred N` both print. The header, the whole instruction block and the whole DUE NOW
/// block must end inside the first 2,000 UTF-16 units, before lane 3's lines and after them. Both
/// readings are printed whatever the outcome: the numbers are the deliverable.
#[test]
fn first_screen_holds_with_real_length_slugs_and_lane_3_lines() {
    fn units_to_end_of_line(s: &str, needle: &str) -> usize {
        let at = s.find(needle).unwrap_or_else(|| panic!("{needle:?} is not in the output:\n{s}"));
        let end = s[at..].find('\n').map_or(s.len(), |i| at + i + 1);
        s[..end].encode_utf16().count()
    }
    for (tag, with_deferred) in [("fs1-before", false), ("fs1-after", true)] {
        let seed = workspace(tag, "");
        for i in 0..10 {
            let slug = format!("2026-09-14-1735-merlin-served-count-and-coach-x{i:03}");
            assert_eq!(slug.chars().count(), 50, "control: a real-length slug is 50 characters");
            add_handoff(
                &seed,
                Tier::Workspace,
                &H { slug: &slug, kind: "handoff", project: &format!("project-{i:02}"), status: "open", touched: 1, resurface: 1, deferred: None },
            );
        }
        for (slug, name, days) in [
            ("renew-the-wildcard-certificate", "Renew the wildcard certificate", 3),
            ("send-the-quarterly-board-pack", "Send the quarterly board pack", 9),
        ] {
            let q = Q::new(Tier::Workspace, &format!("reminder/{slug}"))
                .typ("Reminder")
                .lit("name", name)
                .date("resurfaceAt", &stamp(days));
            append(&seed, Tier::Workspace, &q.out, slug);
        }
        if with_deferred {
            add_handoff(
                &seed,
                Tier::Workspace,
                &H {
                    slug: "2026-08-30-1000-merlin-parked-for-the-count",
                    kind: "handoff",
                    project: "parked",
                    status: "deferred",
                    touched: 16,
                    resurface: 16,
                    deferred: Some(("auto: cold 12d", 4)),
                },
            );
        }

        let out = session_start(&seed);
        let letters = units_to_end_of_line(&out, "Letters: A=");
        // DUE NOW sorts oldest due first, so the 9-day reminder is the FIRST item and the 3-day one the LAST.
        // The first draft measured the first item under the last item's name (green run, 2026-09-15).
        let (first, last) = (out.find("Send the quarterly board pack"), out.find("Renew the wildcard certificate"));
        assert!(
            matches!((first, last), (Some(f), Some(l)) if f < l),
            "{tag}: DUE NOW's two items are not both in full, oldest due first:\n{out}"
        );
        let due_last = units_to_end_of_line(&out, "Renew the wildcard certificate");
        println!(
            "FS1 {tag}: letters line ends at {letters} UTF-16 units, DUE NOW's last item at {due_last}; output {} units",
            out.encode_utf16().count()
        );
        assert!(out.contains("archives "), "control: the day-8 warning line is not in the output:\n{out}");
        if with_deferred {
            assert!(out.lines().next().unwrap_or_default().contains(" · deferred 1 · "), "control: {out}");
        }
        // The bar is 1,990, not A5's 2,000: a 10-unit margin for the header's count digits, which this fixture
        // does not max out (forks, tasks and deferred run to three digits on a real store). The ten slugs are
        // already at the 50-character maximum, so no margin is owed to them (auk, verdicts, 2026-09-15).
        // Lane 3 added the B3 line and the B2 count, so lane 3 keeps the screen inside the bar.
        const BAR: usize = 1990;
        assert!(letters <= BAR, "{tag}: the letters line ends at unit {letters}, past the {BAR}-unit bar");
        assert!(due_last <= BAR, "{tag}: DUE NOW's last item ends at unit {due_last}, past the {BAR}-unit bar");
    }
}
