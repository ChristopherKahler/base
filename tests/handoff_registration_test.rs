//! Rank 08: registering a handoff archives the project's prior handoff in every tier (spec E4 and J6 item 1, with
//! `auk`'s Q2 ruling and the rank 08 rulings of 2026-09-14; lane brief, sections 6, "J6 ITEM 1 — REPRODUCTION" and
//! "RANK 08").
//!
//! `base handoff create` archives the project's prior open or deferred continuity handoff in EVERY tier, never a fork
//! in any tier, and prints what it archived with its tier. The new slug is left out only in the tier being written,
//! where the create re-points it; in the other tier the same slug is an older copy and is archived (ruling D1). An
//! unreadable other tier does not stop the registration: the handoff is written, then create fails naming the tier
//! file it could not read (ruling D5). At
//! `4af8eaf` the archive is scoped to the tier the write lands in, it matches `open` only, and under `-g` the CLI swaps
//! the caller's cwd for the global tier before `create` runs, so a global create cannot see the workspace at all
//! (brief, R1 to R3).
//!
//! Driven through the binary, so the file compiles in both arms and asserts on what an operator sees: `create`'s own
//! lines, the status column of `base handoff list` and `base fork list`, and, where the tier matters and the list
//! merges tiers away, the graph file each tier writes. Isolation per law 21's amendment: `BASE_HOME` is a run root under
//! the temp dir and `HOME` is left alone, so base's own write tripwire stays armed.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Rig {
    home: PathBuf,
    ws: PathBuf,
    docs: PathBuf,
    _root: tempfile::TempDir,
}

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

/// `base <args>` standing in the workspace, as (exit code, stdout, stderr).
fn base(rig: &Rig, args: &[&str]) -> (i32, String, String) {
    run_in(rig, &rig.ws, args)
}

fn run_in(rig: &Rig, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_base"))
        .args(args)
        .current_dir(cwd)
        .env("BASE_HOME", &rig.home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (out.status.code().unwrap_or(-1), stdout, stderr)
}

/// A handoff doc on disk. Its basename is the slug.
fn doc(rig: &Rig, slug: &str) -> String {
    let path = rig.docs.join(format!("{slug}.md"));
    std::fs::write(&path, format!("---\nproject: base\n---\n# {slug}\n")).unwrap();
    path.display().to_string()
}

/// `base handoff [-g] create` for project `base`, standing in the workspace. Asserts the registration line, which is the
/// positive control for everything after it, and returns stdout.
fn create(rig: &Rig, global: bool, slug: &str) -> String {
    let doc = doc(rig, slug);
    let mut args = vec!["handoff"];
    if global {
        args.push("-g");
    }
    args.extend(["create", "--project", "base", "--doc", doc.as_str()]);
    let (rc, out, err) = base(rig, &args);
    assert_eq!(rc, 0, "create {slug} failed: {out}{err}");
    assert!(out.contains(&format!("registered (slug: {slug})")), "control: {slug} was registered: {out}");
    out
}

/// The status `base <kind> list` shows for `slug`: the status word in the table row that names it.
fn status(rig: &Rig, kind: &str, slug: &str) -> Option<String> {
    let (rc, out, err) = base(rig, &[kind, "list"]);
    assert_eq!(rc, 0, "{kind} list failed: {out}{err}");
    let row = out.lines().find(|line| line.split('|').any(|cell| cell.trim() == slug))?;
    row.split('|')
        .map(str::trim)
        .find(|cell| matches!(*cell, "open" | "archived" | "deferred" | "snoozed"))
        .map(String::from)
}

/// Every status literal one tier's graph file holds for `slug`, read from the N-Quads on disk. The tier a record lives
/// in is the file it is in, and `base handoff list` merges the tiers into one table.
fn graph_statuses(file: &Path, slug: &str) -> Vec<String> {
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("control: {} is readable: {e}", file.display()));
    let subject = format!("/{slug}> ");
    text.lines()
        .filter(|line| line.contains(&subject) && line.contains("#status> \""))
        .filter_map(|line| line.split("#status> \"").nth(1)?.split('"').next().map(String::from))
        .collect()
}

/// Marks a workspace handoff `deferred` in the fixture graph: lane 3's state, which this tree has no command for yet.
fn defer(rig: &Rig, slug: &str) {
    let path = rig.ws.join(".base").join("graph.nq");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut changed = 0;
    let mut out = String::new();
    for line in text.lines() {
        if line.contains(&format!("/{slug}>")) && line.contains("#status> \"open\"") {
            out.push_str(&line.replace("#status> \"open\"", "#status> \"deferred\""));
            changed += 1;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    assert_eq!(changed, 1, "fixture: exactly one status line to defer for {slug}");
    std::fs::write(&path, out).unwrap();
}

#[test]
fn control_same_tier_the_second_create_archives_the_first() {
    // Predicted GREEN in both arms: #71 fixed the same-tier case, and this is what separates "the feature is broken"
    // from "the harness did not run".
    let r = rig();
    create(&r, false, "2026-09-14-first-base");
    let out = create(&r, false, "2026-09-14-second-base");
    assert_eq!(status(&r, "handoff", "2026-09-14-second-base").as_deref(), Some("open"), "control: the new one is open");
    assert_eq!(status(&r, "handoff", "2026-09-14-first-base").as_deref(), Some("archived"), "same tier: {out}");
    assert!(out.contains("2026-09-14-first-base"), "create names what it archived: {out}");
}

#[test]
fn workspace_then_global_the_global_create_archives_the_workspace_handoff() {
    // S and W: the workspace handoff first, the global one second. The blind direction (R3).
    let r = rig();
    create(&r, false, "2026-09-14-ws-base");
    let out = create(&r, true, "2026-09-14-gbl-base");
    assert_eq!(status(&r, "handoff", "2026-09-14-gbl-base").as_deref(), Some("open"), "control: the new one is open");
    assert_eq!(status(&r, "handoff", "2026-09-14-ws-base").as_deref(), Some("archived"), "every tier: {out}");
    assert!(out.contains("archived prior handoff: 2026-09-14-ws-base (workspace tier)"), "named with its tier: {out}");
}

#[test]
fn global_then_workspace_the_workspace_create_archives_the_global_handoff() {
    let r = rig();
    create(&r, true, "2026-09-14-gfirst-base");
    let out = create(&r, false, "2026-09-14-wsecond-base");
    assert_eq!(status(&r, "handoff", "2026-09-14-wsecond-base").as_deref(), Some("open"), "control: the new one is open");
    assert_eq!(status(&r, "handoff", "2026-09-14-gfirst-base").as_deref(), Some("archived"), "every tier: {out}");
    assert!(out.contains("archived prior handoff: 2026-09-14-gfirst-base (global tier)"), "named with its tier: {out}");
}

#[test]
fn the_same_doc_registered_into_the_other_tier_leaves_one_open_record() {
    // Ruling D1: the new slug is left out only in the tier being written, where the create re-points it. In the other
    // tier the same slug is an older copy of the registration, so it is archived like any prior handoff.
    let r = rig();
    create(&r, false, "2026-09-14-moved-base");
    let out = create(&r, true, "2026-09-14-moved-base");
    let global = graph_statuses(&r.home.join(".base-gbl").join(".base").join("graph.nq"), "2026-09-14-moved-base");
    let workspace = graph_statuses(&r.ws.join(".base").join("graph.nq"), "2026-09-14-moved-base");
    assert_eq!(global, ["open"], "control: the tier written holds the new record, open: {out}");
    assert_eq!(workspace, ["archived"], "the other tier's copy of the same slug is archived: {out}");
    assert!(out.contains("archived prior handoff: 2026-09-14-moved-base (workspace tier)"), "named with its tier: {out}");
}

#[test]
fn a_deferred_prior_handoff_is_archived_too() {
    let r = rig();
    create(&r, false, "2026-09-14-parked-base");
    defer(&r, "2026-09-14-parked-base");
    assert_eq!(status(&r, "handoff", "2026-09-14-parked-base").as_deref(), Some("deferred"), "control: fixture reads deferred");
    let out = create(&r, false, "2026-09-14-next-base");
    assert_eq!(status(&r, "handoff", "2026-09-14-parked-base").as_deref(), Some("archived"), "E4: open or deferred: {out}");
}

#[test]
fn a_fork_in_another_tier_is_never_archived() {
    // Predicted GREEN in both arms: the guard on the multi-seat ruling. A fork is additive side-work in every tier.
    let r = rig();
    let fork_doc = doc(&r, "2026-09-14-side-fork");
    let (rc, out, err) = base(&r, &["fork", "-g", "create", "--project", "base", "--doc", fork_doc.as_str()]);
    assert_eq!(rc, 0, "fork create failed: {out}{err}");
    assert_eq!(status(&r, "fork", "2026-09-14-side-fork").as_deref(), Some("open"), "control: the fork is listed open");
    create(&r, false, "2026-09-14-main-base");
    assert_eq!(status(&r, "fork", "2026-09-14-side-fork").as_deref(), Some("open"), "a handoff create never archives a fork");
}

#[test]
fn an_unreadable_other_tier_does_not_block_registration() {
    // Ruling D5: a tier being written that cannot be read is a plain error, but an unreadable OTHER tier must not stop
    // the handoff being registered. The new handoff is written first, then create exits non-zero naming the slug it
    // registered and the tier file it could not read. A directory where the global graph file belongs is a tier that
    // exists and cannot be read: the loader opens it and the first read fails.
    let r = rig();
    let global_graph = r.home.join(".base-gbl").join(".base").join("graph.nq");
    std::fs::create_dir_all(&global_graph).unwrap();
    let slug = "2026-09-14-blocked-base";
    let doc_path = doc(&r, slug);
    let (rc, out, err) = base(&r, &["handoff", "create", "--project", "base", "--doc", doc_path.as_str()]);
    let said = format!("{out}{err}");
    let workspace = graph_statuses(&r.ws.join(".base").join("graph.nq"), slug);
    assert_eq!(workspace, ["open"], "control: the handoff is registered in the tier written: rc {rc}: {said}");
    assert!(said.contains(&global_graph.display().to_string()), "the unreadable tier is named: rc {rc}: {said}");
    assert!(said.contains(&format!("registered '{slug}'")), "the error says the handoff was registered: {said}");
    assert_ne!(rc, 0, "an unreadable tier is not a clean success: {said}");
}

#[test]
fn create_help_promises_every_tier() {
    let r = rig();
    let (rc, out, err) = base(&r, &["handoff", "create", "--help"]);
    assert_eq!(rc, 0, "help failed: {err}");
    assert!(out.contains("every tier"), "the help says what create does (Q2 ruling): {out}");
    assert!(!out.contains("in this tier"), "the per-tier promise is gone: {out}");
}
