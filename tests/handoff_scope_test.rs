//! BO-11 (F18, F22): a new handoff archives only the earlier one in its own project and lane, an archive can be
//! undone, and writes made inside a workspace stay in that workspace's tier.
//!
//! F18: `base handoff create` archived the project's previous open handoff in every tier, whoever wrote it, with no
//! undo. Sessions `auk`, `plover`, `grebe` and `finch` all wrote handoffs for `base-0160`, each new one archived
//! another session's, and they stopped registering handoffs at all. F22: a shell parked in `~/.base-gbl/handoffs`
//! found the global tier's `.base` first and wrote the global tier from inside the workspace at `~`: 59 open
//! handoffs and forks leaked into Chris's global tier that way.
//!
//! Driven through the binary, so the assertions are on what an operator sees: `create`'s own lines, the status
//! column of `base handoff list` / `base fork list`, and, where the tier matters, the graph file each tier writes.
//! Isolation: `BASE_HOME` is a run root under the temp dir, and the relay identity variables a session's shell
//! carries are scrubbed, so the lane a create picks is the one each test names.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Rig {
    home: PathBuf,
    ws: PathBuf,
    docs: PathBuf,
    _root: tempfile::TempDir,
}

/// A home with a global tier, a separate workspace, and a folder for docs.
fn rig() -> Rig {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let ws = root.path().join("ws");
    let docs = root.path().join("docs");
    for dir in [home.join(".base-gbl").join(".base"), ws.join(".base"), docs.clone()] {
        std::fs::create_dir_all(&dir).unwrap();
    }
    Rig { home, ws, docs, _root: root }
}

/// A home that is itself a workspace, as `C:/Users/Chris` is: `<home>/.base` beside `<home>/.base-gbl/.base`, and
/// the handoff and fork doc folders inside the global root, where sessions write their docs.
fn home_workspace_rig() -> Rig {
    let r = rig();
    std::fs::create_dir_all(r.home.join(".base")).unwrap();
    for sub in ["handoffs", "forks"] {
        std::fs::create_dir_all(r.home.join(".base-gbl").join(sub)).unwrap();
    }
    Rig { ws: r.home.clone(), ..r }
}

/// `base <args>` standing in `cwd`, as session `relay_as` (no relay identity at all when `None`).
fn run_in(rig: &Rig, cwd: &Path, relay_as: Option<&str>, args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_base"));
    cmd.args(args)
        .current_dir(cwd)
        .env("BASE_HOME", &rig.home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_NO_SPAWN", "1")
        .env_remove("BASE_RELAY_AS")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("WT_SESSION")
        .env_remove("BASE_HEADLESS");
    if let Some(title) = relay_as {
        cmd.env("BASE_RELAY_AS", title);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn base(rig: &Rig, relay_as: Option<&str>, args: &[&str]) -> (i32, String, String) {
    run_in(rig, &rig.ws, relay_as, args)
}

/// A handoff doc on disk, with front matter naming the project and, when given, its author. Its basename is the slug.
fn doc(rig: &Rig, slug: &str, by: Option<&str>) -> String {
    let path = rig.docs.join(format!("{slug}.md"));
    let by = by.map(|b| format!("by: {b}\n")).unwrap_or_default();
    std::fs::write(&path, format!("---\nproject: base-0160\n{by}---\n# {slug}\n")).unwrap();
    path.display().to_string()
}

/// `base handoff create` for `project`, as `relay_as`, with `extra` flags after the subcommand's own. Asserts the
/// registration line, the positive control for everything after it, and returns stdout.
fn create(rig: &Rig, relay_as: Option<&str>, project: &str, slug: &str, extra: &[&str]) -> String {
    let doc = doc(rig, slug, None);
    let mut args = vec!["handoff", "create", "--project", project, "--doc", doc.as_str()];
    args.extend_from_slice(extra);
    let (rc, out, err) = base(rig, relay_as, &args);
    assert_eq!(rc, 0, "create {slug} failed: {out}{err}");
    assert!(out.contains(&format!("registered (slug: {slug})")), "control: {slug} was registered: {out}");
    out
}

/// The status `base <kind> list` shows for `slug`.
fn status(rig: &Rig, kind: &str, slug: &str) -> Option<String> {
    let (rc, out, err) = base(rig, None, &[kind, "list"]);
    assert_eq!(rc, 0, "{kind} list failed: {out}{err}");
    let row = out.lines().find(|line| line.split('|').any(|cell| cell.trim() == slug))?;
    row.split('|')
        .map(str::trim)
        .find(|cell| matches!(*cell, "open" | "archived" | "deferred" | "snoozed"))
        .map(String::from)
}

fn global_graph(rig: &Rig) -> PathBuf {
    rig.home.join(".base-gbl").join(".base").join("graph.nq")
}

fn workspace_graph(rig: &Rig) -> PathBuf {
    rig.ws.join(".base").join("graph.nq")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Every status literal one tier's graph file holds for `slug`.
fn graph_statuses(file: &Path, slug: &str) -> Vec<String> {
    let subject = format!("/{slug}> ");
    read(file)
        .lines()
        .filter(|line| line.contains(&subject) && line.contains("#status> \""))
        .filter_map(|line| line.split("#status> \"").nth(1)?.split('"').next().map(String::from))
        .collect()
}

#[test]
fn handoff_create_archives_only_same_lane() {
    // Example 1. plover's handoff is in the workspace tier and grebe's in the global tier; auk writes two.
    let r = rig();
    create(&r, Some("plover"), "base-0160", "2026-09-19-1000-plover-base-0160", &[]);
    let grebe_doc = doc(&r, "2026-09-19-1100-grebe-base-0160", None);
    let (rc, out, err) = base(
        &r,
        Some("grebe"),
        &["handoff", "-g", "create", "--project", "base-0160", "--doc", grebe_doc.as_str()],
    );
    assert_eq!(rc, 0, "grebe's global create: {out}{err}");

    let first = create(&r, Some("auk"), "base-0160", "2026-09-20-2100-auk-base-0160", &[]);
    assert!(first.contains("lane: auk (from this session's relay title)"), "{first}");
    assert!(first.contains("archived: nothing (no earlier open handoff by auk on base-0160)"), "{first}");
    for other in ["2026-09-19-1000-plover-base-0160", "2026-09-19-1100-grebe-base-0160"] {
        assert_eq!(status(&r, "handoff", other).as_deref(), Some("open"), "{other} must stay open: {first}");
    }

    let second = create(&r, Some("auk"), "base-0160", "2026-09-21-0900-auk-base-0160", &[]);
    assert!(second.contains("archived: 2026-09-20-2100-auk-base-0160 (workspace tier)"), "{second}");
    assert_eq!(status(&r, "handoff", "2026-09-20-2100-auk-base-0160").as_deref(), Some("archived"));
    assert_eq!(status(&r, "handoff", "2026-09-21-0900-auk-base-0160").as_deref(), Some("open"));
    assert_eq!(status(&r, "handoff", "2026-09-19-1000-plover-base-0160").as_deref(), Some("open"), "{second}");
    assert_eq!(graph_statuses(&global_graph(&r), "2026-09-19-1100-grebe-base-0160"), ["open"], "{second}");

    // The same lane is still archived in every tier: auk's next handoff, written to the global tier, archives the one
    // in the workspace tier and nothing else.
    let third_doc = doc(&r, "2026-09-22-0900-auk-base-0160", None);
    let (rc, third, err) = base(
        &r,
        Some("auk"),
        &["handoff", "-g", "create", "--project", "base-0160", "--doc", third_doc.as_str()],
    );
    assert_eq!(rc, 0, "{third}{err}");
    assert!(third.contains("archived: 2026-09-21-0900-auk-base-0160 (workspace tier)"), "{third}");
    assert_eq!(graph_statuses(&workspace_graph(&r), "2026-09-19-1000-plover-base-0160"), ["open"], "{third}");
    assert_eq!(graph_statuses(&global_graph(&r), "2026-09-19-1100-grebe-base-0160"), ["open"], "{third}");
}

#[test]
fn handoff_lane_flag_shares_a_lane() {
    // Example 2: two sessions hand the `dealer-crawl` lane back and forth; a third session's own handoff on the same
    // project is in its own lane and touches neither.
    let r = rig();
    let jaguar = create(&r, Some("jaguar"), "vintryx", "2026-09-29-1455-jaguar-vintryx-dealer-crawl", &["--lane", "dealer-crawl"]);
    assert!(jaguar.contains("lane: dealer-crawl (from --lane)"), "{jaguar}");
    assert!(jaguar.contains("archived: nothing (no earlier open handoff in lane dealer-crawl on vintryx)"), "{jaguar}");
    create(&r, Some("lemur"), "vintryx", "2026-09-30-0700-lemur-vintryx-inventory-crawl", &[]);

    let caracal = create(&r, Some("caracal"), "vintryx", "2026-09-30-0955-caracal-vintryx-dealer-crawl", &["--lane", "dealer-crawl"]);
    assert!(caracal.contains("archived: 2026-09-29-1455-jaguar-vintryx-dealer-crawl (workspace tier)"), "{caracal}");
    assert_eq!(status(&r, "handoff", "2026-09-29-1455-jaguar-vintryx-dealer-crawl").as_deref(), Some("archived"));
    assert_eq!(status(&r, "handoff", "2026-09-30-0955-caracal-vintryx-dealer-crawl").as_deref(), Some("open"));
    assert_eq!(status(&r, "handoff", "2026-09-30-0700-lemur-vintryx-inventory-crawl").as_deref(), Some("open"), "{caracal}");

    // The lane name is matched case aside, and a lane recorded by --lane is not the author's lane.
    let lemur = create(&r, Some("lemur"), "vintryx", "2026-10-01-0800-lemur-vintryx-inventory-crawl", &[]);
    assert!(lemur.contains("archived: 2026-09-30-0700-lemur-vintryx-inventory-crawl (workspace tier)"), "{lemur}");
    assert_eq!(status(&r, "handoff", "2026-09-30-0955-caracal-vintryx-dealer-crawl").as_deref(), Some("open"), "{lemur}");
    let upper = create(&r, Some("lemur"), "vintryx", "2026-10-01-0900-lemur-vintryx-dealer-crawl", &["--lane", "Dealer-Crawl"]);
    assert!(upper.contains("archived: 2026-09-30-0955-caracal-vintryx-dealer-crawl (workspace tier)"), "{upper}");
}

#[test]
fn handoff_create_reports_what_it_archived() {
    // F18b: every create says what it archived, by slug and tier, or that it archived nothing; and it names what it
    // left open in other lanes. The first line stays as scripts and the *end flow read it.
    let r = rig();
    let first = create(&r, Some("plover"), "base-0160", "2026-09-19-1000-plover-base-0160", &[]);
    let lines: Vec<&str> = first.lines().collect();
    assert_eq!(lines[0], "Handoff for 'base-0160' registered (slug: 2026-09-19-1000-plover-base-0160)", "{first}");
    assert!(lines.contains(&"archived: nothing (no earlier open handoff by plover on base-0160)"), "{first}");
    assert!(!first.contains("left open"), "nothing else is open: {first}");

    let auk = create(&r, Some("auk"), "base-0160", "2026-09-20-2100-auk-base-0160", &[]);
    assert!(auk.contains("left open, other lanes: 2026-09-19-1000-plover-base-0160 (plover)"), "{auk}");

    let plover = create(&r, Some("plover"), "base-0160", "2026-09-20-1000-plover-base-0160", &[]);
    let lines: Vec<&str> = plover.lines().collect();
    assert!(lines.contains(&"archived: 2026-09-19-1000-plover-base-0160 (workspace tier)"), "{plover}");
    assert!(lines.contains(&"left open, other lanes: 2026-09-20-2100-auk-base-0160 (auk)"), "{plover}");
    assert!(!plover.contains("archived: nothing"), "{plover}");

    // No lane from anywhere: the create says so, and shares the lane of the project's other handoffs that have none.
    let a = create(&r, None, "kit", "kit-resume-a", &[]);
    assert!(a.contains("lane: none (no --lane, no by: in the doc, no codename in the slug, no relay title)"), "{a}");
    assert!(a.contains("archived: nothing (no earlier open handoff with no lane on kit)"), "{a}");
    let b = create(&r, None, "kit", "kit-resume-b", &[]);
    assert!(b.contains("archived: kit-resume-a (workspace tier)"), "{b}");
}

#[test]
fn a_handoff_from_before_lanes_reads_its_lane_from_by_or_the_slug() {
    // Chris's store holds hundreds of handoffs written before lanes existed: no lane recorded. Each reads its lane
    // from its doc's `by:`, else from the codename in its slug, matched against the relay's titles; one with
    // neither has no lane, and a create in a named lane never archives it.
    let r = rig();
    let entry = |title: &str| {
        serde_json::json!({ "title": title, "session_id": format!("s-{title}"), "registered_at": "2026-09-01T00:00:00Z", "last_heartbeat": "2026-09-01T00:00:00Z" })
    };
    let registry = serde_json::json!({ "sessions": { "otter-bo11": entry("otter-bo11"), "otter": entry("otter") } });
    std::fs::write(r.home.join(".base-gbl").join(".base").join("sessions.json"), registry.to_string()).unwrap();
    // auk is named only by the workspace's relay store, as on Chris's machine, where the global registry had dropped
    // most old titles (42 there, 291 in the store).
    let store = r.ws.join(".base").join("relay").join("team");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("registry.json"), serde_json::json!({ "sessions": { "auk": entry("auk") } }).to_string()).unwrap();

    // Four legacy handoffs: auk's by slug, finch's by `by:` alone, otter's by slug, and one with neither.
    let legacy = [
        ("2026-09-19-1045-auk-base-0160", None),
        ("finch-notes-base-0160", Some("finch")),
        ("2026-09-20-0900-otter-base-0160", None),
        ("base-0160-loose-notes", None),
    ];
    let p = "http://ops-sys.local/ontology#";
    let mut quads = String::new();
    for (slug, by) in legacy {
        let path = doc(&r, slug, by);
        let s = format!("<{p}handoff/{slug}>");
        let g = format!("<{p}graph/ws/ws>");
        for (pred, obj) in [
            ("http://www.w3.org/1999/02/22-rdf-syntax-ns#type".to_string(), format!("<{p}Handoff>")),
            (format!("{p}project"), "\"base-0160\"".to_string()),
            (format!("{p}kind"), "\"handoff\"".to_string()),
            (format!("{p}status"), "\"open\"".to_string()),
            (format!("{p}handoffDoc"), format!("\"{}\"", path.replace('\\', "/"))),
        ] {
            quads.push_str(&format!("{s} <{pred}> {obj} {g} .\n"));
        }
    }
    std::fs::write(workspace_graph(&r), quads).unwrap();
    for (slug, _) in legacy {
        assert_eq!(status(&r, "handoff", slug).as_deref(), Some("open"), "control: fixture {slug} reads open");
    }

    // otter-bo11's create: its slug names otter-bo11, the longest title that fits, so otter's is another lane.
    let ob = create(&r, None, "base-0160", "2026-10-02-2300-otter-bo11-base-0160", &[]);
    assert!(ob.contains("lane: otter-bo11 (from the codename in the slug)"), "{ob}");
    assert!(ob.contains("archived: nothing (no earlier open handoff by otter-bo11 on base-0160)"), "{ob}");
    // auk's create archives auk's legacy handoff, found by its slug, and nothing else.
    let auk = create(&r, Some("auk"), "base-0160", "2026-10-02-2310-auk-base-0160", &[]);
    assert!(auk.contains("archived: 2026-09-19-1045-auk-base-0160 (workspace tier)"), "{auk}");
    // finch's handoff names finch only in its doc.
    let finch = create(&r, Some("finch"), "base-0160", "2026-10-02-2320-finch-base-0160", &[]);
    assert!(finch.contains("archived: finch-notes-base-0160 (workspace tier)"), "{finch}");
    for kept in ["2026-09-20-0900-otter-base-0160", "base-0160-loose-notes"] {
        assert_eq!(status(&r, "handoff", kept).as_deref(), Some("open"), "{kept}: {ob}{auk}{finch}");
    }
    assert!(finch.contains("base-0160-loose-notes (no lane)"), "listed as left open, with no lane: {finch}");
}

#[test]
fn handoff_and_fork_unarchive() {
    // Example 3: an archive is undone in the tier that holds it, and the command says what changed.
    let r = rig();
    create(&r, Some("petrel"), "base-0160", "2026-09-14-1306-petrel-base-0160", &[]);
    let (rc, out, err) = base(&r, None, &["handoff", "archive", "2026-09-14-1306-petrel-base-0160"]);
    assert_eq!(rc, 0, "control: archive: {out}{err}");
    assert_eq!(status(&r, "handoff", "2026-09-14-1306-petrel-base-0160").as_deref(), Some("archived"));

    let (rc, out, err) = base(&r, None, &["handoff", "unarchive", "2026-09-14-1306-petrel-base-0160"]);
    assert_eq!(rc, 0, "{out}{err}");
    assert_eq!(
        out.trim(),
        "unarchived 2026-09-14-1306-petrel-base-0160 (workspace tier): status archived -> open",
        "{out}{err}"
    );
    assert_eq!(status(&r, "handoff", "2026-09-14-1306-petrel-base-0160").as_deref(), Some("open"));
    assert_eq!(graph_statuses(&workspace_graph(&r), "2026-09-14-1306-petrel-base-0160"), ["open"], "one status triple");

    // A second unarchive changes nothing and fails, naming the status it found; an unknown slug fails too.
    let (rc, out, err) = base(&r, None, &["handoff", "unarchive", "2026-09-14-1306-petrel-base-0160"]);
    assert_ne!(rc, 0, "a no-op is not a success: {out}{err}");
    assert!(out.contains("status open, not archived") && err.contains("not archived in any tier"), "{out}{err}");
    let (rc, out, err) = base(&r, None, &["handoff", "unarchive", "2026-01-01-nobody"]);
    assert_ne!(rc, 0, "{out}{err}");
    assert!(err.contains("no handoff '2026-01-01-nobody' in either tier"), "{err}");

    // A fork in the global tier, the same way.
    let fork_doc = doc(&r, "2026-09-14-side-quest", None);
    let (rc, out, err) = base(&r, None, &["fork", "-g", "create", "--project", "base-0160", "--doc", fork_doc.as_str()]);
    assert_eq!(rc, 0, "{out}{err}");
    let (rc, out, err) = base(&r, None, &["fork", "archive", "2026-09-14-side-quest"]);
    assert_eq!(rc, 0, "{out}{err}");
    assert_eq!(status(&r, "fork", "2026-09-14-side-quest").as_deref(), Some("archived"));
    let (rc, out, err) = base(&r, None, &["fork", "unarchive", "2026-09-14-side-quest"]);
    assert_eq!(rc, 0, "{out}{err}");
    assert_eq!(out.trim(), "unarchived 2026-09-14-side-quest (global tier): status archived -> open", "{out}");
    assert_eq!(status(&r, "fork", "2026-09-14-side-quest").as_deref(), Some("open"));
}

/// The four writes F22a names, standing in `cwd`, with `-g` when `global`. Returns the text each wrote.
fn write_four(rig: &Rig, cwd: &Path, global: bool, tag: &str) -> [String; 4] {
    let g: &[&str] = if global { &["-g"] } else { &[] };
    let handoff = format!("2026-10-02-2300-{tag}-handoff");
    let fork = format!("2026-10-02-{tag}-fork");
    let decision = format!("{tag} decision about tiers");
    let note = format!("{tag} note about tiers");
    let h_doc = doc(rig, &handoff, None);
    let f_doc = doc(rig, &fork, None);
    let runs: [Vec<&str>; 4] = [
        [&["handoff"][..], g, &["create", "--project", "kit", "--doc", h_doc.as_str()]].concat(),
        [&["fork"][..], g, &["create", "--project", "kit", "--doc", f_doc.as_str()]].concat(),
        [&["decision"][..], g, &["log", "--domain", "kit", "--decision", decision.as_str(), "--rationale", "r"]].concat(),
        [&["learn"][..], g, &["--text", note.as_str(), "--domain", "kit", "--type", "insight"]].concat(),
    ];
    for args in &runs {
        let (rc, out, err) = run_in(rig, cwd, None, args);
        assert_eq!(rc, 0, "base {args:?} in {}: {out}{err}", cwd.display());
    }
    [handoff, fork, decision, note]
}

#[test]
fn writes_default_to_workspace_tier() {
    // Example 4: in a workspace, handoff, fork, decision and note writes go to its tier, including from the folders
    // inside the global root where the docs live, and from that root itself without -g.
    let r = home_workspace_rig();
    let gbl = r.home.join(".base-gbl");
    for (cwd, tag) in [
        (r.home.clone(), "home"),
        (gbl.join("handoffs"), "handoffs-dir"),
        (gbl.join("forks"), "forks-dir"),
        (gbl.clone(), "gbl-root"),
    ] {
        let written = write_four(&r, &cwd, false, tag);
        let (ws, global) = (read(&workspace_graph(&r)), read(&global_graph(&r)));
        for what in &written {
            assert!(ws.contains(what.as_str()), "{what} (standing in {}) is in the workspace tier", cwd.display());
            assert!(!global.contains(what.as_str()), "{what} (standing in {}) leaked to the global tier", cwd.display());
        }
    }

    // Outside every workspace, a folder inside the global root still writes the global tier, as before.
    let r = rig();
    let docs_dir = r.home.join(".base-gbl").join("handoffs");
    std::fs::create_dir_all(&docs_dir).unwrap();
    let written = write_four(&r, &docs_dir, false, "no-workspace");
    let global = read(&global_graph(&r));
    for what in &written {
        assert!(global.contains(what.as_str()), "{what}: outside every workspace the global tier is used");
    }
}

#[test]
fn global_flag_still_writes_global() {
    // Example 4's other half: -g writes the global tier, from the workspace and from inside the global root.
    let r = home_workspace_rig();
    let gbl = r.home.join(".base-gbl");
    for (cwd, tag) in [(r.home.clone(), "home-g"), (gbl.join("handoffs"), "handoffs-dir-g"), (gbl.clone(), "gbl-root-g")] {
        let written = write_four(&r, &cwd, true, tag);
        let (ws, global) = (read(&workspace_graph(&r)), read(&global_graph(&r)));
        for what in &written {
            assert!(global.contains(what.as_str()), "{what} (-g, standing in {}) is in the global tier", cwd.display());
            assert!(!ws.contains(what.as_str()), "{what} (-g, standing in {}) went to the workspace", cwd.display());
        }
    }
}
