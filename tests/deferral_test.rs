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

/// `ON` plus the PROJECT engine. Projects run behind `[protocol] enabled`, which the shared seed
/// leaves OFF (this file's own header says so), so a project test seeded with `ON` alone
/// reconciles nothing and every assertion after it is vacuous.
const ON_PROTO: &str = "[defer]
enabled = true

[protocol]
enabled = true
";

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

/// The seed every test here stands on.
///
/// IT NO LONGER MARKS ANY MIGRATION. Until 2026-09-19 this was two functions: `workspace()`, which
/// called `mark_migrated()` so the rank 09 write gate would not engage, and `workspace_pending()`,
/// which left the marker absent to model a legacy machine. The gate is gone and `mark_fresh_install`
/// with it, so there is no state to mark and no second fixture that needs one. Both collapsed here.
///
/// WHY THE RETIRED ASSERT IS FINISHED RATHER THAN LOST. `mark_migrated` ended by asserting that the
/// marker landed, and that assert guarded ORDER rather than success: `mark_fresh_install` marked
/// only when the plan was EMPTY, so the fixture was correct only while `workspace()` ran BEFORE any
/// record was seeded. Delete the function and the hazard cannot occur — the order it protected no
/// longer exists to get wrong.
///
/// WHAT REPLACES IT, because the seed path still has one silent step. `remove_dir_all` is the only
/// call here that ignores its result; everything inside `seed::write` carries an `.expect` with a
/// named reason. On Windows a root can survive the clean while the call reports nothing, and a stale
/// root feeds another run's records into every assertion after it — the same shape as the fixture
/// that silently did not land, read from the other end. So the clean is ASSERTED.
fn workspace(tag: &str, global_toml: &str) -> seed::Seed {
    let root = std::env::temp_dir().join(format!("base-r05-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        !root.exists(),
        "the seed root {} survived its clean, so this test would run against a previous run's \
         records and every assertion below would be measuring the wrong tree",
        root.display()
    );
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

/// A project in the workspace tier, with a REAL folder on disk.
///
/// Projects are dated by `protocol::touch::folder_last_touch` — the newest FILE mtime under the
/// project folder — and NOT by the graph's `lastActive`. A fixture that only seeds `lastActive`
/// reconciles to `Hold`, which is exactly how the first draft of these two tests failed on their
/// own controls rather than on their subject.
fn add_project(seed: &seed::Seed, slug: &str, status: &str, folder_days: u64, extra: &[(&str, &str)]) {
    let dir = seed.ws.join(slug);
    std::fs::create_dir_all(&dir).expect("project folder");
    let file = dir.join("README.md");
    std::fs::write(&file, "fixture\n").expect("project file");

    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(folder_days * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .expect("open to date")
        .set_modified(when)
        .expect("set mtime");

    // THE MTIME WRITE CAN SILENTLY NOT LAND (filesystem granularity, a mount ignoring utimes), and
    // every assertion after it would then measure a FRESH folder while claiming to measure an old
    // one. Assert it rather than let the absence be silent.
    let got = std::fs::metadata(&file).expect("stat").modified().expect("mtime");
    let drift = got
        .duration_since(when)
        .or_else(|_| when.duration_since(got))
        .expect("compare mtimes");
    assert!(
        drift.as_secs() < 120,
        "the fixture mtime did not land for {slug}: asked for {folder_days}d old, drift {}s",
        drift.as_secs()
    );

    let mut q = Q::new(Tier::Workspace, &format!("project/{slug}"))
        .typ("Project")
        .lit("name", &format!("Fixture {slug}"))
        .lit("status", status)
        .lit("path", slug)
        .date("lastActive", &stamp(folder_days as i64));
    for (p, v) in extra {
        q = if p.ends_with("At") { q.date(p, v) } else { q.lit(p, v) };
    }
    append(seed, Tier::Workspace, &q.out, slug);
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
#[ignore = "KNOWN DEFECT, NOT A FLAKE: the first screen measures 2011 UTF-16 units against a \
            1990-unit bar - and past the real 2,000-unit limit too, by 11. Ignored ONLY so the \
            suite runs to completion: it halted here, leaving ~69 of ~99 targets UNMEASURED, and \
            an unknown is worse than a known red. AN IGNORE IS NOT A PASS AND MUST NEVER BE READ \
            AS ONE. The measurement is real and reproduced on every lane that carries this file \
            (grebe, auk, finch), at a61117b with none of today's work in the tree, and again on \
            the 4-lane merge. The assertion is CORRECT - auk ruled 1990 the readability bar and \
            the only thing this test was ever entitled to assert - so the OUTPUT is what is \
            wrong, not the bar. Raising the bar to make this pass is moving the goalposts. OWNER: \
            the emit/session-start surface, to shed the overflow in the with-deferred case. \
            Remove this attribute the moment it does. finch, 2026-09-21."]
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

        // THE BYTE ARM IS RETIRED. THE READING THAT DID IT IS src/emit/mod.rs:496-500 - NOT a
        // preview measurement. The note that stood here asked for exactly that: "retire one arm
        // when the preview is measured, and say which reading did it." This is the other way it
        // could land. (NOT :495 - that line is budget_bytes, the HOST field, which is the exact
        //  opposite of what this argument rests on. The citation was wrong here once already.)
        //
        // THE ARM WAS NOT THE WRONG UNIT. IT WAS AIMED AT THE WRONG TARGET. It borrowed the host's
        // byte limit and applied it to base's own first screen - and the first screen is declared,
        // at src/emit/mod.rs:496-500, to be "deliberately a different unit from budget_bytes, and
        // that is not an oversight: this one is about readability, not delivery, so it is not
        // measured against the host's limit." src/config.rs:869 says the same of the key that sets
        // it: first_screen_chars and memory_chars "ARE GENUINELY UTF-16 AND KEEP THEIR NAMES ...
        // the host's unit does not apply to them."
        //
        // So no preview measurement could ever have governed this bar, in either unit. The ratchet
        // held a line nothing needed held, and waiting on the preview probe to retire it was
        // waiting on evidence that could not bear on the question.
        //
        // DELIVERY IS STILL GUARDED, BY A DIFFERENT MECHANISM, AND THAT IS WHY REMOVING THIS IS
        // SAFE: base trims to budget_bytes BEFORE printing, so the host's byte limit is enforced
        // there and never reaches this screen. Removing the arm leaves nothing unguarded.
        //
        // The UTF-16 assertions above keep BAR at its original 1,990 - the readability bar, and
        // the only thing this test was ever entitled to assert.
        //
        // The helper bytes_to_end_of_line went with it: this arm was its only caller.
        // (auk, 2026-09-20. The byte arm's own measurement - unit 1984, BYTE 2006 on fs1-after -
        //  is preserved in ~/.base-gbl/forks/2026-09-20-auk-ruling-preview-unit-before-shedding.md,
        //  so retiring the assertion does not erase the number it found.)
    }
}

// ── D19-D24: F1, a record carrying several values is decided on all of them ─────
// F1 (lane 3 verdicts, 2026-09-15): the planner kept the FIRST solution row per subject, so a record holding two
// values for one field was decided by the store's index order, and the Defer write deletes every status value. The
// live instance is `task/rebuild-auth-guard` in the global tier: `status "active"` AND `status "completed"`, three
// `lastActive` values. One test per rule the planner now follows. Each has a single-valued twin whose outcome proves
// the pass ran, so a pass that never runs cannot pass any of them. Mutation MD14 (the first row stands again) must
// redden them.

/// The values of `subject`'s `pred`, sorted, so a comparison never depends on the order the store wrote them in.
fn sorted_values(seed: &seed::Seed, tier: Tier, subject: &str, pred: &str) -> Vec<String> {
    let mut v = values(seed, tier, subject, pred);
    v.sort();
    v
}

/// A second `lastActive` on a workspace task, typed as the store writes it (`add_task` types only `…At` extras).
fn add_last_active(seed: &seed::Seed, slug: &str, days: i64) {
    let at = stamp(days);
    let q = Q::new(Tier::Workspace, &format!("task/{slug}")).date("lastActive", &at);
    let marker = format!("<{}task/{slug}> <{}lastActive> \"{at}\"", seed::NS, seed::NS);
    append(seed, Tier::Workspace, &q.out, &marker);
}

// ── D19 ──────────────────────────────────────────────────────────────────────
/// F1 rule 1: any terminal status wins, WHATEVER ORDER the store returns a subject's values in.
/// DISCRIMINATING, and deliberately order-independent (auk, 2026-09-18). Two cold tasks that mean the same
/// thing - each carries one working status and `completed` - with their working values chosen to encode on
/// opposite sides of `completed`: `open` sorts above it, `active` sorts below it. Both must reach the SAME
/// decision, because both carry a terminal status. On commit 4 `plan_records` keeps the first row per
/// subject, so whichever way the store iterates, exactly one of the pair reads its working status and is
/// deferred while the other reads `completed` and is not - the decisions DIFFER and this arm is red under
/// either ordering rule. It cannot go green if oxigraph changes its index, because no assertion below names
/// an order. Its single-status cold twin is the control that proves the pass ran at all.
#[test]
fn a_record_carrying_a_terminal_status_is_never_deferred_whatever_else_it_carries() {
    let seed = workspace("d19", ON);
    add_task(&seed, "task-open-and-completed", "open", 30, &[("status", "completed")]);
    add_task(&seed, "task-active-and-completed", "active", 30, &[("status", "completed")]);
    add_task(&seed, "task-cold-twin", "active", 30, &[]);
    let ws = Tier::Workspace;
    let (a, b) = ("task/task-open-and-completed", "task/task-active-and-completed");
    assert_eq!(sorted_values(&seed, ws, a, "status"), ["completed", "open"], "control: pair member A carries both");
    assert_eq!(sorted_values(&seed, ws, b, "status"), ["active", "completed"], "control: pair member B carries both");

    session_start(&seed);
    assert_eq!(status(&seed, ws, "task/task-cold-twin"), "deferred", "control: the cold twin was not deferred, so the pass never ran");
    let (after_a, after_b) = (sorted_values(&seed, ws, a, "status"), sorted_values(&seed, ws, b, "status"));
    let (deferred_a, deferred_b) = (
        after_a.iter().any(|s| s == "deferred"),
        after_b.iter().any(|s| s == "deferred"),
    );
    assert_eq!(
        deferred_a, deferred_b,
        "two records carrying a terminal status got DIFFERENT decisions, so the status was picked by row order: A={after_a:?} B={after_b:?}"
    );
    for (label, after) in [("A", &after_a), ("B", &after_b)] {
        assert!(after.iter().any(|s| s == "completed"), "{label}: a completed task lost `completed` to the deferral pass: {after:?}");
        assert!(!after.iter().any(|s| s == "deferred"), "{label}: a completed task was deferred: {after:?}");
    }
}

// ── D20 ──────────────────────────────────────────────────────────────────────
/// F1 rule 6: the clock reads the NEWEST `lastActive`. A task carrying one 30 days old and one 1 day old was touched
/// yesterday and is not deferred. Its twin, carrying only the 30-day clock, is.
/// NON-DISCRIMINATING on commit 4, and structurally so (auk, 2026-09-18). It passed at commit 4 without the fix.
/// Exposing the row-order defect needs the store to return the OLDER `lastActive` first, and with ISO date
/// literals older always sorts lower, so no value choice reaches that case. There is no second lever: this
/// arm's only assertion is the clock, and a record whose values are both cold defers either way, so nothing
/// separates the two. It is kept as a REGRESSION GUARD under law 45, not as a red arm, and it was not bent
/// until it reddened. Grade any mutation against it as INERT, never PROVEN.
#[test]
fn the_clock_reads_the_newest_last_active_a_record_carries() {
    let seed = workspace("d20", ON);
    add_task(&seed, "task-two-clocks", "active", 30, &[]);
    add_last_active(&seed, "task-two-clocks", 1);
    add_task(&seed, "task-cold-twin", "active", 30, &[]);
    let ws = Tier::Workspace;
    assert_eq!(values(&seed, ws, "task/task-two-clocks", "lastActive").len(), 2, "control: the fixture carries two clocks");

    session_start(&seed);
    assert_eq!(status(&seed, ws, "task/task-cold-twin"), "deferred", "control: the cold twin was not deferred, so the pass never ran");
    assert_eq!(status(&seed, ws, "task/task-two-clocks"), "active", "a task touched yesterday was deferred on its older clock");
}

// ── D21 ──────────────────────────────────────────────────────────────────────
/// F1 rule 2: two working statuses are ambiguous, and an ambiguous record is never written. A cold task carrying
/// `active` and `in_progress` keeps both. Its single-status cold twin is deferred.
#[test]
fn a_record_carrying_two_working_statuses_is_held_and_keeps_both() {
    let seed = workspace("d21", ON);
    add_task(&seed, "task-two-statuses", "active", 30, &[("status", "in_progress")]);
    add_task(&seed, "task-cold-twin", "active", 30, &[]);
    let (ws, dual) = (Tier::Workspace, "task/task-two-statuses");
    assert_eq!(sorted_values(&seed, ws, dual, "status"), ["active", "in_progress"], "control: the fixture carries both");

    session_start(&seed);
    assert_eq!(status(&seed, ws, "task/task-cold-twin"), "deferred", "control: the cold twin was not deferred, so the pass never ran");
    assert_eq!(sorted_values(&seed, ws, dual, "status"), ["active", "in_progress"], "an ambiguous record was written");
}

// ── D22 ──────────────────────────────────────────────────────────────────────
/// F1 rule 5: a snooze running in ANY `resurfaceAt` value pins the record. A cold task carrying one `resurfaceAt`
/// 20 days past and one 5 days ahead is not deferred. Its twin, carrying only the passed one, is.
/// NON-DISCRIMINATING on commit 4, and structurally so (auk, 2026-09-18). It passed at commit 4 without the fix.
/// Exposing the defect needs the store to return the PAST `resurfaceAt` first while a future one is also
/// carried, and a future ISO date always sorts above a past one, so that case is unreachable. A record whose
/// `resurfaceAt` values are all past is not pinned either way, so it separates nothing. Kept as a REGRESSION
/// GUARD under law 45. Grade any mutation against it as INERT, never PROVEN.
#[test]
fn a_snooze_running_in_any_resurface_value_pins_the_record() {
    let seed = workspace("d22", ON);
    let (passed, ahead) = (stamp(20), stamp(-5));
    add_task(&seed, "task-snoozed", "active", 30, &[("resurfaceAt", passed.as_str()), ("resurfaceAt", ahead.as_str())]);
    add_task(&seed, "task-cold-twin", "active", 30, &[("resurfaceAt", passed.as_str())]);
    let ws = Tier::Workspace;
    assert_eq!(values(&seed, ws, "task/task-snoozed", "resurfaceAt").len(), 2, "control: the fixture carries both");

    session_start(&seed);
    assert_eq!(status(&seed, ws, "task/task-cold-twin"), "deferred", "control: the cold twin was not deferred, so the pass never ran");
    assert_eq!(status(&seed, ws, "task/task-snoozed"), "active", "a task snoozed into the future was deferred");
}

// ── D23 ──────────────────────────────────────────────────────────────────────
/// F1 rule 7: the clock revives a task only when EVERY `deferredReason` it carries starts `auto:`.
/// DISCRIMINATING, and deliberately order-independent (auk, 2026-09-18). Two tasks that mean the same thing
/// (each deferred by the pass AND by the operator, so neither may be revived) with their operator reasons
/// chosen to encode on opposite sides of `auto:`: `until Q4` sorts above it, `Q4 hold` sorts below it
/// (uppercase `Q` is 0x51, lowercase `a` is 0x61). Both must reach the SAME decision. On commit 4 the clock
/// reads the first `deferredReason` row, so whichever way the store iterates, exactly one of the pair reads
/// the operator's reason and stays deferred while the other reads the `auto:` one and is revived - the
/// decisions DIFFER and this arm is red under either ordering rule, with no assertion naming an order.
/// The all-automatic twin is the control that proves the pass ran at all.
#[test]
fn the_clock_revives_only_when_every_deferred_reason_is_automatic() {
    let seed = workspace("d23", ON);
    let at5 = stamp(5);
    for (slug, operator_reason) in [("task-auto-and-until", "until Q4"), ("task-auto-and-qhold", "Q4 hold")] {
        add_task(
            &seed,
            slug,
            "deferred",
            1,
            &[("deferredReason", "auto: cold 12d"), ("deferredReason", operator_reason), ("deferredAt", at5.as_str())],
        );
    }
    add_task(&seed, "task-auto-twin", "deferred", 1, &[("deferredReason", "auto: cold 12d"), ("deferredAt", at5.as_str())]);
    let ws = Tier::Workspace;
    let (a, b) = ("task/task-auto-and-until", "task/task-auto-and-qhold");
    assert_eq!(sorted_values(&seed, ws, a, "deferredReason"), ["auto: cold 12d", "until Q4"], "control: pair member A carries both reasons");
    assert_eq!(sorted_values(&seed, ws, b, "deferredReason"), ["Q4 hold", "auto: cold 12d"], "control: pair member B carries both reasons");

    session_start(&seed);
    assert_eq!(status(&seed, ws, "task/task-auto-twin"), "active", "control: the automatic twin was not revived, so the pass never ran");
    let (after_a, after_b) = (status(&seed, ws, a), status(&seed, ws, b));
    assert_eq!(
        after_a, after_b,
        "two records an operator also deferred got DIFFERENT decisions, so the reason was picked by row order: A={after_a} B={after_b}"
    );
    assert_eq!(after_a, "deferred", "the clock revived a task an operator also deferred");
}

// ── D24 ──────────────────────────────────────────────────────────────────────
/// F1 rule 1's note on `kind`: a record carrying `kind "fork"` is a fork, whatever other `kind` it carries, to the
/// planner AND to both listings, so they never disagree. Windows: handoff 30 days, fork 10. A record carrying `fork`
/// and `handoff`, untouched 11 days, is deferred on the fork window, listed by `base fork deferred` and not by
/// `base handoff deferred`. Its single-kind fork twin, also 11 days cold, is deferred too.
#[test]
fn a_record_carrying_the_fork_kind_is_a_fork_to_the_planner_and_to_both_listings() {
    let seed = workspace("d24", "[defer]\nenabled = true\n[defer.days]\nhandoff = 30\nfork = 10\n");
    let dual = "2026-09-04-0900-lark-two-kinds-of-record";
    let twin = "2026-09-04-0900-lark-fork-twin-record";
    for slug in [dual, twin] {
        add_handoff(
            &seed,
            Tier::Workspace,
            &H { slug, kind: "fork", project: "two-kinds", status: "open", touched: 11, resurface: 11, deferred: None },
        );
    }
    let second = Q::new(Tier::Workspace, &format!("handoff/{dual}")).lit("kind", "handoff");
    let marker = format!("<{}handoff/{dual}> <{}kind> \"handoff\"", seed::NS, seed::NS);
    append(&seed, Tier::Workspace, &second.out, &marker);
    let ws = Tier::Workspace;
    assert_eq!(sorted_values(&seed, ws, &format!("handoff/{dual}"), "kind"), ["fork", "handoff"], "control: both kinds");

    session_start(&seed);
    assert_eq!(status(&seed, ws, &format!("handoff/{twin}")), "deferred", "control: the fork twin was not deferred, so the pass never ran");
    assert_eq!(status(&seed, ws, &format!("handoff/{dual}")), "deferred", "the record carrying `fork` was not deferred on the fork window");
    let (_, forks) = base(&seed, &["fork", "deferred"]);
    assert!(forks.contains(dual), "`base fork deferred` does not list the record carrying `fork`:\n{forks}");
    let (_, handoffs) = base(&seed, &["handoff", "deferred"]);
    assert!(!handoffs.contains(dual), "`base handoff deferred` lists a record the planner treats as a fork:\n{handoffs}");
}

// ── D19b ─────────────────────────────────────────────────────────────────────
/// F1 rule 1, the INSERTION-POSITION complement of D19 (auk, 2026-09-18). D19 varies the VALUE SET and
/// holds write position fixed, so a lexical row rule makes its two members differ and reddens it, while an
/// insertion rule makes them agree and greens it. This arm pulls the other lever: the SAME two values,
/// `completed` and `open`, with only their WRITE ORDER swapped. A lexical rule greens this one; an
/// insertion rule reddens it. Whichever rule the store follows, exactly one of D19 and D19b is red on
/// commit 4, and neither assertion names a rule - so they are independent detectors in the law 39 sense,
/// and the one that greens is the control telling you which rule is in play. `add_task` writes its primary
/// status first and its extras after; that is the only thing that differs between A and B.
/// Note `handoff_like` is false for a Task (`reconcile.rs:461`), so `open` is a working status here.
#[test]
fn a_terminal_status_wins_whichever_position_it_was_written_in() {
    let seed = workspace("d19b", ON);
    add_task(&seed, "task-completed-then-open", "completed", 30, &[("status", "open")]);
    add_task(&seed, "task-open-then-completed", "open", 30, &[("status", "completed")]);
    add_task(&seed, "task-cold-twin", "active", 30, &[]);
    let ws = Tier::Workspace;
    let (a, b) = ("task/task-completed-then-open", "task/task-open-then-completed");
    assert_eq!(sorted_values(&seed, ws, a, "status"), ["completed", "open"], "control: A carries both, `completed` written first");
    assert_eq!(sorted_values(&seed, ws, b, "status"), ["completed", "open"], "control: B carries both, `open` written first");

    session_start(&seed);
    assert_eq!(status(&seed, ws, "task/task-cold-twin"), "deferred", "control: the cold twin was not deferred, so the pass never ran");
    let (after_a, after_b) = (sorted_values(&seed, ws, a, "status"), sorted_values(&seed, ws, b, "status"));
    let (deferred_a, deferred_b) = (
        after_a.iter().any(|s| s == "deferred"),
        after_b.iter().any(|s| s == "deferred"),
    );
    assert_eq!(
        deferred_a, deferred_b,
        "two records carrying the SAME two statuses got different decisions, so the WRITE ORDER decided it: A={after_a:?} B={after_b:?}"
    );
    for (label, after) in [("A", &after_a), ("B", &after_b)] {
        assert!(after.iter().any(|s| s == "completed"), "{label}: a completed task lost `completed` to the deferral pass: {after:?}");
        assert!(!after.iter().any(|s| s == "deferred"), "{label}: a completed task was deferred: {after:?}");
    }
}

// ── D23b ─────────────────────────────────────────────────────────────────────
/// F1 rule 7, the INSERTION-POSITION complement of D23. Same construction as D19b and the same reason:
/// D23 varies the VALUE SET with `auto:` held in the same write slot in both members, so an insertion rule
/// greens it. Here both members carry the SAME two reasons, `auto: cold 12d` and `until Q4`, with only
/// their write order swapped. A lexical rule greens this arm; an insertion rule reddens it. Both records
/// were deferred by the operator as well as by the pass, so neither may be revived, whichever reason the
/// clock happens to read first.
#[test]
fn the_clock_reads_every_deferred_reason_whichever_position_it_was_written_in() {
    let seed = workspace("d23b", ON);
    let at5 = stamp(5);
    add_task(
        &seed,
        "task-auto-written-first",
        "deferred",
        1,
        &[("deferredReason", "auto: cold 12d"), ("deferredReason", "until Q4"), ("deferredAt", at5.as_str())],
    );
    add_task(
        &seed,
        "task-operator-written-first",
        "deferred",
        1,
        &[("deferredReason", "until Q4"), ("deferredReason", "auto: cold 12d"), ("deferredAt", at5.as_str())],
    );
    add_task(&seed, "task-auto-twin", "deferred", 1, &[("deferredReason", "auto: cold 12d"), ("deferredAt", at5.as_str())]);
    let ws = Tier::Workspace;
    let (a, b) = ("task/task-auto-written-first", "task/task-operator-written-first");
    assert_eq!(sorted_values(&seed, ws, a, "deferredReason"), ["auto: cold 12d", "until Q4"], "control: A carries both, the automatic reason written first");
    assert_eq!(sorted_values(&seed, ws, b, "deferredReason"), ["auto: cold 12d", "until Q4"], "control: B carries both, the operator reason written first");

    session_start(&seed);
    assert_eq!(status(&seed, ws, "task/task-auto-twin"), "active", "control: the automatic twin was not revived, so the pass never ran");
    let (after_a, after_b) = (status(&seed, ws, a), status(&seed, ws, b));
    assert_eq!(
        after_a, after_b,
        "two records carrying the SAME two reasons got different decisions, so the WRITE ORDER decided it: A={after_a} B={after_b}"
    );
    assert_eq!(after_a, "deferred", "the clock revived a task an operator also deferred");
}

// ── The rank 09 pending-migration guard was tested here. REMOVED 2026-09-19 ────────────
//
// Three tests stood here. TWO ARE GONE BY NAME, because what they asserted is gone:
//
//   a_pending_migration_withholds_the_defer_and_still_revives_in_the_same_tier
//   a_pending_migration_writes_no_deferral_at_all
//
// Both seeded `workspace_pending` — a machine with the marker absent — and asserted that a cold
// record was NOT deferred. After the removal a cold record IS deferred, whatever any marker says.
// Their required answer did not weaken, it INVERTED, and a test whose required answer flipped
// cannot be carried forward. Bending either one until it went green is the bend `auk` ruled against
// on 2026-09-18, and it would have deleted the coverage while making the file look healthier.
//
// THE THIRD SURVIVES, RENAMED, BODY UNCHANGED, because its required answer did not change. It
// asserted the post-removal behaviour verbatim while the gate still stood. Its own doc named it a
// guard against the FIX rather than the defect: a filter that dropped `Defer` unconditionally — the
// guard stuck ON rather than removed — would redden this AND NOTHING ELSE in the file, which is the
// single most likely way this removal goes wrong. It is shown red against exactly that mutation.

/// A cold record defers and a revivable record revives, IN THE SAME TIER, on one pass.
///
/// RENAMED 2026-09-19 from `with_the_migration_applied_the_same_tier_defers_and_revives`. The old
/// name described the seed's migration state and there is no migration state left to be in. The
/// behaviour asserted is unchanged, so this is a rename and not a rewrite.
///
/// WHAT IT IS FOR, carried forward word for word: it is a guard against the FIX, not the defect. A
/// filter that dropped `Defer` unconditionally would redden this and nothing else in this file. That
/// is why it had to survive the removal that deleted its two siblings, and why it was renamed rather
/// than replaced by something new.
///
/// THE REVIVABLE HALF MUST BE A TASK. `plan_records` only ever produces `Revive` for a task or a
/// milestone: handoffs and forks never revive on the clock (R7). Built with a handoff the revive
/// could not fire, this test would pass on broken code, and it would look like proof.
#[test]
fn a_cold_record_defers_and_a_revivable_one_revives_in_the_same_tier() {
    let seed = workspace("r09-lift", ON);
    add_task(&seed, "cold-two", "active", 40, &[]);
    add_task(
        &seed,
        "back-two",
        "deferred",
        1,
        &[("deferredReason", "auto: cold 40d"), ("deferredAt", &stamp(30))],
    );

    let out = session_start(&seed);

    assert_eq!(
        status(&seed, Tier::Workspace, "task/cold-two"),
        "deferred",
        "a cold record did not defer. Nothing gates the defer pass any more, so the only way this \
         fails is a filter that drops `Defer`:\n{out}"
    );
    assert_eq!(
        status(&seed, Tier::Workspace, "task/back-two"),
        "active",
        "a revivable record did not revive, in a tier that also held a cold record — the failure \
         direction where records refuse to come back:\n{out}"
    );
}

/// ITEM 2, ARM A — a deferred project must record WHEN.
///
/// `apply_records` writes `deferredAt` on Defer; the PROJECT path had no such write, so every row
/// of `base project deferred` rendered "date deferred not recorded" permanently. Honest, which is
/// exactly why nobody chased it.
///
/// SPLIT FROM ARM B DELIBERATELY. In one test the first failing assertion hides the second, and the
/// revive half is the one a single-line fix leaves broken — so it must be able to go red on its own.
#[test]
fn a_deferred_project_records_when_it_was_deferred() {
    let seed = workspace("d-projat-a", ON_PROTO);
    add_project(&seed, "cold-proj", "active", 40, &[]);
    assert_eq!(
        status(&seed, Tier::Workspace, "project/cold-proj"),
        "active",
        "control: the project must start working, or deferring it proves nothing"
    );

    let (code, out) = base(&seed, &["reconcile"]);
    assert_eq!(code, 0, "{out}");

    assert_eq!(
        status(&seed, Tier::Workspace, "project/cold-proj"),
        "deferred",
        "control: the cold project did not defer, so this test never reached its subject:
{out}"
    );
    assert_eq!(
        values(&seed, Tier::Workspace, "project/cold-proj", "deferredAt").len(),
        1,
        "a deferred project must record when it was deferred, exactly once:
{out}"
    );
}

/// ITEM 2, ARM B — a revived project must CLEAR it. This is the half a one-line fix leaves broken.
///
/// It seeds `deferredAt` DIRECTLY rather than deferring first, and that is the whole design: on the
/// unfixed tree the Defer half never writes the field, so an arm that deferred first would find it
/// absent afterwards for the WRONG REASON and pass while proving nothing. Seeding the field is the
/// only way this arm discriminates the revive path independently of the defer path.
#[test]
fn a_revived_project_clears_the_date_it_was_deferred() {
    let seed = workspace("d-projat-b", ON_PROTO);
    let at4 = stamp(4);
    add_project(
        &seed,
        "back-proj",
        "deferred",
        1,
        &[("deferredReason", "auto: cold 10d"), ("deferredAt", at4.as_str())],
    );
    assert_eq!(
        status(&seed, Tier::Workspace, "project/back-proj"),
        "deferred",
        "control: must start deferred"
    );
    assert_eq!(
        values(&seed, Tier::Workspace, "project/back-proj", "deferredAt").len(),
        1,
        "control: must start WITH the field, or its absence afterwards proves nothing"
    );

    let (code, out) = base(&seed, &["reconcile"]);
    assert_eq!(code, 0, "{out}");

    assert_eq!(
        status(&seed, Tier::Workspace, "project/back-proj"),
        "active",
        "control: the warm project did not revive, so this test never reached its subject:
{out}"
    );
    assert!(
        values(&seed, Tier::Workspace, "project/back-proj", "deferredAt").is_empty(),
        "a revived project still carries deferredAt, so the field says deferred while status says active:
{out}"
    );
}
