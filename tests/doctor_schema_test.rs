//! `base doctor` states the domain schema, always.
//!
//! C4: a migration that never ran must not look the same as one that ran and
//! found nothing. Both cases are quiet at session start by design — the hook says
//! nothing when it links nothing — so doctor is the only surface where "this store
//! has not been migrated" is visible at all.

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(base_dir.join("graph.nq"), "").unwrap();
    std::fs::write(
        base_dir.join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n",
    )
    .unwrap();
    std::fs::write(
        base_dir.join("domains.toml"),
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = []\nrules = [\"r\"]\n",
    )
    .unwrap();
    let config = BaseConfig::load(tmp.path());
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), None).unwrap();
    tmp
}

fn graph_path(cwd: &Path) -> std::path::PathBuf {
    cwd.join(".base").join("graph.nq")
}

fn insert(cwd: &Path, triples: &str) {
    let g = crud::workspace_graph_iri(&ns(), &crud::workspace_slug(cwd));
    crud::load_and_mutate(cwd, &ns(), &format!("INSERT DATA {{ GRAPH <{g}> {{\n{triples}\n}} }}"))
        .unwrap();
}

fn migrate(cwd: &Path) -> base::migrate::Outcome {
    let g = crud::workspace_graph_iri(&ns(), &crud::workspace_slug(cwd));
    base::migrate::migrate_tier(&graph_path(cwd), &g, &ns(), base::migrate::Trigger::SessionStart)
        .unwrap()
}

#[test]
fn an_unmigrated_tier_says_so_and_counts_its_orphans() {
    let tmp = workspace();
    let cwd = tmp.path();
    for n in 0..3 {
        let iri = crud::build_iri(&ns(), "goal", &format!("g{n}"));
        insert(cwd, &format!("<{iri}> rdf:type ops:Goal ; ops:name \"g{n}\" .\n"));
    }
    insert(
        cwd,
        "<http://ops-sys.local/ontology#ext/lore/LoreFact/1> rdf:type ops:LoreFact .\n",
    );

    let r = base::doctor::diagnose_tier("workspace", &graph_path(cwd));
    assert_eq!(r.status, "healthy");
    assert_eq!(r.schema_version, None, "never migrated");
    assert_eq!(
        r.domain_orphans,
        vec![("Goal".to_string(), 3), ("LoreFact".to_string(), 1)],
        "highest first, then alphabetical — a stable order to diff between runs"
    );

    let report = base::doctor::DoctorReport {
        tiers: vec![r],
        healthy: true,
        warnings: vec![],
        config_errors: vec![],
        trigger_faults: vec![],
    };
    let human = base::doctor::format_human(&report);
    assert!(human.contains("schema: not migrated"), "{human}");
    assert!(human.contains("without a domain: 4 record(s)"), "{human}");
    assert!(human.contains("Goal=3"), "{human}");
}

#[test]
fn a_migrated_tier_reports_its_schema_version_and_no_orphans() {
    let tmp = workspace();
    let cwd = tmp.path();
    let iri = crud::build_iri(&ns(), "goal", "g0");
    insert(cwd, &format!("<{iri}> rdf:type ops:Goal ; ops:name \"g0\" .\n"));
    let out = migrate(cwd);
    assert_eq!(out.total_linked(), 1);

    let r = base::doctor::diagnose_tier("workspace", &graph_path(cwd));
    assert_eq!(r.schema_version.as_deref(), Some(base::migrate::SCHEMA_VERSION));
    assert!(r.domain_orphans.is_empty(), "{:?}", r.domain_orphans);

    let report = base::doctor::DoctorReport {
        tiers: vec![r],
        healthy: true,
        warnings: vec![],
        config_errors: vec![],
        trigger_faults: vec![],
    };
    let human = base::doctor::format_human(&report);
    assert!(human.contains("schema: domain-1"), "{human}");
    assert!(!human.contains("without a domain"), "nothing to report: {human}");
}

#[test]
fn drift_after_a_migration_is_visible_and_is_not_a_fault() {
    // `base sync` writes a Document with no domain, so the orphan count climbs
    // again after a migration. That is drift, not damage, and the operator has to
    // be able to see it — the stamp alone would say "done" forever.
    let tmp = workspace();
    let cwd = tmp.path();
    migrate(cwd);
    let iri = crud::build_iri(&ns(), "document", "written-later");
    insert(cwd, &format!("<{iri}> rdf:type ops:Document ; ops:name \"later\" .\n"));

    let r = base::doctor::diagnose_tier("workspace", &graph_path(cwd));
    assert_eq!(r.schema_version.as_deref(), Some(base::migrate::SCHEMA_VERSION), "still stamped");
    assert_eq!(r.domain_orphans, vec![("Document".to_string(), 1)], "and the drift is counted");
    assert_eq!(r.status, "healthy", "drift is not a health fault");
}

#[test]
fn an_unhealthy_tier_reports_no_schema_rather_than_a_wrong_one() {
    let tmp = workspace();
    let cwd = tmp.path();
    migrate(cwd);
    let mut raw = std::fs::read_to_string(graph_path(cwd)).unwrap();
    raw.push_str("this is not a quad\n");
    std::fs::write(graph_path(cwd), &raw).unwrap();

    let r = base::doctor::diagnose_tier("workspace", &graph_path(cwd));
    assert_eq!(r.status, "unhealthy");
    assert_eq!(r.schema_version, None, "a store that will not parse claims nothing");
    assert!(r.domain_orphans.is_empty());
}

#[test]
fn a_ping_is_never_counted_as_an_orphan() {
    let tmp = workspace();
    let cwd = tmp.path();
    let iri = crud::build_iri(&ns(), "ping", "lark-probe");
    insert(cwd, &format!("<{iri}> rdf:type ops:Ping ; ops:message \"m\" .\n"));

    let r = base::doctor::diagnose_tier("workspace", &graph_path(cwd));
    assert!(
        r.domain_orphans.is_empty(),
        "session traffic is excluded from the count as well as from the backfill: {:?}",
        r.domain_orphans
    );
}
