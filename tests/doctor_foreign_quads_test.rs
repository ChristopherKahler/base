//! #142 — `base doctor` reports quads belonging to another workspace instead of
//! passing the tier as HEALTHY.
//!
//! The per-tier classification is unit-tested in `src/doctor.rs` against the pure
//! `diagnose_tier` seam. What CANNOT be reached from there is `DoctorReport::healthy`
//! itself, which `doctor::diagnose` computes after resolving both tiers — and the
//! whole complaint in #142 is about that one boolean. `home::with_thread_home` gives
//! this file a fake home per test, so `diagnose` runs against tiers built here and
//! never against the operator's own graph.
//!
//! Every arm states which tier it is measuring. A doctor test that cannot say which
//! tier it measured has not measured anything.

use std::fs;
use std::path::{Path, PathBuf};

/// A fake home plus a workspace under it, both inside the test sandbox.
///
/// Returns `(tempdir, home, workspace root)`. The global tier is created empty and
/// unwritten so `tier_paths` skips it: these arms are about the WORKSPACE tier, and
/// a global tier that also carried quads would make it ambiguous which one moved
/// the verdict.
fn sandbox() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let td = tempfile::tempdir().unwrap();
    let home = td.path().join("home");
    let ws = home.join("projects").join("mine");
    fs::create_dir_all(ws.join(".base")).unwrap();
    fs::create_dir_all(home.join(".base-gbl")).unwrap();
    (td, home, ws)
}

fn nsuri() -> String {
    base::config::NamespaceConfig::default().uri
}

fn quad_in(subject: &str, g: &str) -> String {
    format!(
        "<http://example.org/{subject}> \
         <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
         <http://example.org/Thing> <{g}> .\n"
    )
}

fn write_graph(ws: &Path, body: &str) {
    fs::write(ws.join(".base").join("graph.nq"), body).unwrap();
}

/// A10 — the leg the whole lane turns on. A tier carrying another workspace's
/// quads must read NOT healthy.
///
/// The before-state is asserted in the same run, from the same fixture with one
/// variable changed, so "it reads false" is a difference rather than a claim: a
/// detector that returned false unconditionally would fail the first half.
#[test]
fn foreign_quads_make_the_report_not_healthy() {
    let (_td, home, ws) = sandbox();
    let own = format!("{}graph/ws/mine", nsuri());
    let theirs = format!("{}graph/ws/theirs", nsuri());

    base::home::with_thread_home(&home, || {
        // BEFORE — the same tier with only its own quads.
        write_graph(&ws, &(quad_in("a", &own) + &quad_in("b", &own)));
        let clean = base::doctor::diagnose(&ws);
        assert_eq!(clean.tiers.len(), 1, "workspace tier only; global is unwritten");
        assert_eq!(clean.tiers[0].tier, "workspace");
        assert!(clean.healthy, "the control arm must be healthy or the test proves nothing");
        assert!(clean.tiers[0].foreign_graphs.is_empty());

        // AFTER — one foreign quad added, nothing else touched.
        write_graph(&ws, &(quad_in("a", &own) + &quad_in("b", &own) + &quad_in("c", &theirs)));
        let dirty = base::doctor::diagnose(&ws);
        assert_eq!(dirty.tiers[0].foreign_graphs, vec![(theirs, 1)]);
        assert!(!dirty.healthy, "a tier carrying another workspace's quads is NOT healthy");

        // And the tier still parses. #142 is a provenance fault, not a parse one:
        // overloading `status` would switch off composition, schema and the
        // supersession audit on exactly the tier being diagnosed.
        assert_eq!(dirty.tiers[0].status, "healthy");
    });
}

/// A11 — MUST-FAIL CANARY. `healthy` was not widened into always-true.
///
/// A corrupt `commands.toml` made the report unhealthy before #142 and must still
/// do it, **for the same stated reason**. Asserting only "still false" would pass
/// identically if the new conjunct were what turned it false, so this arm pins the
/// reason string and asserts `foreign_graphs` is EMPTY while it does.
#[test]
fn a_corrupt_config_still_reads_not_healthy_for_its_own_reason() {
    let (_td, home, ws) = sandbox();
    let own = format!("{}graph/ws/mine", nsuri());

    base::home::with_thread_home(&home, || {
        write_graph(&ws, &quad_in("a", &own));
        fs::write(ws.join(".base").join("commands.toml"), "this is not [valid toml").unwrap();

        let r = base::doctor::diagnose(&ws);
        assert!(!r.healthy, "a corrupt commands.toml must still fail the report");
        assert_eq!(r.config_errors.len(), 1, "exactly one config error, named");
        assert!(
            r.config_errors[0].contains("commands.toml is not valid TOML"),
            "the pre-existing reason must survive verbatim, got: {:?}",
            r.config_errors[0]
        );
        assert!(
            r.tiers[0].foreign_graphs.is_empty(),
            "and it must be failing for THAT reason, not for #142's"
        );
    });
}

/// A14 — the global tier resolves its own slug in a live `diagnose`, not just in
/// the unit test's synthetic path.
///
/// `<home>/.base-gbl/.base/graph.nq` must attribute its quads to `base-gbl`. If it
/// did not, every global tier on every machine would read 100% foreign the moment
/// this shipped — the worst failure this change can have.
#[test]
fn the_global_tier_attributes_its_own_quads() {
    let (_td, home, ws) = sandbox();
    let gbl_base = home.join(".base-gbl").join(".base");
    fs::create_dir_all(&gbl_base).unwrap();
    let gbl_own = format!("{}graph/ws/base-gbl", nsuri());
    fs::write(gbl_base.join("graph.nq"), quad_in("g", &gbl_own)).unwrap();

    base::home::with_thread_home(&home, || {
        write_graph(&ws, &quad_in("a", &format!("{}graph/ws/mine", nsuri())));
        let r = base::doctor::diagnose(&ws);

        let global = r.tiers.iter().find(|t| t.tier == "global").expect("global tier resolved");
        assert!(
            global.foreign_graphs.is_empty(),
            "the global tier's own quads must not read as another workspace's, got {:?}",
            global.foreign_graphs
        );
        let workspace = r.tiers.iter().find(|t| t.tier == "workspace").expect("workspace tier");
        assert!(workspace.foreign_graphs.is_empty());
        assert!(r.healthy, "two clean tiers");
    });
}
