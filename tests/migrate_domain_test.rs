//! The migration runs once, computes its work from the store, and stamps in the
//! same write as the data.
//!
//! Every assertion here maps to an acceptance line in the fork doc:
//!
//! - a store at the old shape, opened by the new binary, migrates once and only once
//! - a `.bak` restore followed by re-open neither double-migrates nor skips
//! - the catchall population is NAMED, never "whatever was left"
//! - session traffic is never given a domain
//!
//! The `.bak` pair is the one that decides the design. A file sentinel outside the
//! store passes "migrates once" and fails "restore re-migrates", silently, forever
//! — which is why the stamp lives in the graph (G0 verdict A1).

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::migrate;

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
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = [\"probe\"]\nrules = [\"a probe rule\"]\n",
    )
    .unwrap();
    let config = BaseConfig::load(tmp.path());
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), None).unwrap();
    tmp
}

fn graph_path(cwd: &Path) -> std::path::PathBuf {
    cwd.join(".base").join("graph.nq")
}

fn graph_iri(cwd: &Path) -> String {
    crud::workspace_graph_iri(&ns(), &crud::workspace_slug(cwd))
}

fn migrate(cwd: &Path) -> migrate::Outcome {
    migrate::migrate_tier(&graph_path(cwd), &graph_iri(cwd), &ns()).unwrap()
}

fn insert(cwd: &Path, triples: &str) {
    let sparql = format!("INSERT DATA {{ GRAPH <{}> {{\n{triples}\n}} }}", graph_iri(cwd));
    crud::load_and_mutate(cwd, &ns(), &sparql).unwrap();
}

/// An orphan of the given class — typed, named, and linked to nothing.
fn plant_orphan(cwd: &Path, kind: &str, class: &str, slug: &str) -> String {
    let ns = ns();
    let p = &ns.prefix;
    let iri = crud::build_iri(&ns, kind, slug);
    insert(cwd, &format!("<{iri}> rdf:type {p}:{class} ;\n  {p}:name \"{slug}\" .\n"));
    iri
}

fn ask(cwd: &Path, pattern: &str) -> bool {
    let q = format!("ASK {{ GRAPH ?g {{ {pattern} }} }}");
    match crud::load_and_query(cwd, &ns(), &q).unwrap() {
        oxigraph::sparql::QueryResults::Boolean(b) => b,
        _ => panic!("expected ASK"),
    }
}

fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

// ─── migrates once ───────────────────────────────────────────────────────────

#[test]
fn an_orphan_gets_the_catchall_and_the_store_gets_the_stamp() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");

    let out = migrate(cwd);
    assert!(out.stamped, "the pass must stamp: {out:?}");
    assert_eq!(out.total_linked(), 1, "one orphan, one link: {out:?}");
    assert_eq!(out.catchall, 1, "a Goal is in the named catchall set");
    assert!(out.backup.is_some(), "C1: snapshot before the write");
    assert!(Path::new(out.backup.as_ref().unwrap()).exists(), "the snapshot is on disk");

    let unfiled = crud::build_iri(&ns(), "domain", migrate::CATCHALL);
    let goal = crud::build_iri(&ns(), "goal", "ship-the-thing");
    assert!(ask(cwd, &format!("<{goal}> ops:hasDomain <{unfiled}>")), "the link is on disk");
    assert!(ask(cwd, &format!("<{unfiled}> a ops:Domain")), "the catchall domain record exists");
}

#[test]
fn the_stamp_and_the_data_land_in_the_same_write() {
    // write_back is temp-plus-rename, so this is really "both or neither". What a
    // test can check is that no state exists where one is present and the other is
    // not, at the only point an outside observer can look: the file.
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");
    migrate(cwd);

    let store = base::store::load_graph(&graph_path(cwd)).unwrap();
    let stamp = migrate::stamp_of(&store, &ns(), &graph_iri(cwd));
    assert_eq!(stamp.as_deref(), Some(migrate::SCHEMA_VERSION));

    let goal = crud::build_iri(&ns(), "goal", "ship-the-thing");
    assert!(ask(cwd, &format!("<{goal}> ops:hasDomain ?d")), "data present wherever the stamp is");
}

#[test]
fn a_second_run_writes_nothing_at_all() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");
    migrate(cwd);

    let before = bytes(&graph_path(cwd));
    let out = migrate(cwd);
    assert!(out.already_migrated, "the stamp is the fast path: {out:?}");
    assert!(!out.stamped);
    assert_eq!(out.total_linked(), 0);
    assert!(out.backup.is_none(), "a no-op must not evict a compact backup from the pool of 10");
    assert_eq!(before, bytes(&graph_path(cwd)), "the file must be byte-identical");
}

#[test]
fn without_the_stamp_a_re_run_finds_nothing_to_do() {
    // The stamp is a fast path, not a truth source: strip it and the pass must
    // still reach the same end state by recomputing from the store, not redo work.
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");
    migrate(cwd);

    let iri = graph_iri(cwd);
    let del = format!(
        "DELETE WHERE {{ GRAPH <{iri}> {{ <{iri}> ops:schemaVersion ?v }} }}"
    );
    crud::load_and_mutate(cwd, &ns(), &del).unwrap();

    let out = migrate(cwd);
    assert!(!out.already_migrated, "the stamp is gone, so the pass must run");
    assert!(out.stamped, "and stamp again");
    assert_eq!(out.total_linked(), 0, "but find no work — the record is already linked: {out:?}");
}

// ─── the .bak pair — the acceptance line that decides the design ─────────────

#[test]
fn a_restored_pre_migration_backup_re_migrates() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");
    let pre = bytes(&graph_path(cwd));

    let first = migrate(cwd);
    assert_eq!(first.total_linked(), 1);

    // The operator restores a backup taken before the migration.
    std::fs::write(graph_path(cwd), &pre).unwrap();

    let second = migrate(cwd);
    assert!(
        !second.already_migrated,
        "a restored PRE-migration store has no stamp and must migrate again. A file \
         sentinel outside the store would claim it was already done: {second:?}"
    );
    assert_eq!(second.total_linked(), 1, "and redo exactly the work the restore undid");

    let goal = crud::build_iri(&ns(), "goal", "ship-the-thing");
    assert!(ask(cwd, &format!("<{goal}> ops:hasDomain ?d")), "the store ends migrated");
}

#[test]
fn a_restored_post_migration_backup_does_not_re_migrate() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");
    migrate(cwd);
    let post = bytes(&graph_path(cwd));

    // Something else churns the store, then the operator restores the migrated copy.
    plant_orphan(cwd, "goal", "Goal", "a-later-goal");
    std::fs::write(graph_path(cwd), &post).unwrap();

    let out = migrate(cwd);
    assert!(out.already_migrated, "a restored POST-migration store carries its own stamp: {out:?}");
    assert_eq!(post, bytes(&graph_path(cwd)), "and is left byte-identical");
}

// ─── what it must not touch ──────────────────────────────────────────────────

#[test]
fn a_ping_is_never_given_a_domain() {
    let tmp = workspace();
    let cwd = tmp.path();
    let ping = plant_orphan(cwd, "ping", "Ping", "lark-probe");

    let out = migrate(cwd);
    assert_eq!(out.total_linked(), 0, "session traffic is excluded, not catchalled: {out:?}");
    assert!(!ask(cwd, &format!("<{ping}> ops:hasDomain ?d")), "no domain on a Ping");
}

#[test]
fn a_record_that_already_carries_a_legacy_domain_link_is_left_alone() {
    // The orphan query reads BOTH eras. Reading only `hasDomain` would see all 465
    // note-domain links in Chris's store as orphans and write 465 duplicates.
    let tmp = workspace();
    let cwd = tmp.path();
    crud::note::learn(cwd, &ns(), "an already linked note", "insight", Some("probe"), None, None)
        .unwrap();
    let doc = crud::build_iri(&ns(), "document", "legacy-linked");
    let dom = crud::build_iri(&ns(), "domain", "probe");
    insert(
        cwd,
        &format!("<{doc}> rdf:type ops:Document ;\n  ops:name \"legacy linked\" ;\n  ops:relatedTo <{dom}> .\n"),
    );

    let out = migrate(cwd);
    assert_eq!(
        out.total_linked(),
        0,
        "a record linked under the legacy predicate is not an orphan: {out:?}"
    );
    let unfiled = crud::build_iri(&ns(), "domain", migrate::CATCHALL);
    assert!(!ask(cwd, &format!("<{doc}> ops:hasDomain <{unfiled}>")), "and is not re-filed");
}

#[test]
fn a_documents_entity_link_does_not_count_as_a_domain_link() {
    // `relatedTo` also carries 532 document-entity links in Chris's store. Treating
    // one as a domain link would leave that document orphaned while reporting it
    // covered.
    let tmp = workspace();
    let cwd = tmp.path();
    let doc = crud::build_iri(&ns(), "document", "entity-linked");
    let ent = crud::build_iri(&ns(), "entity", "some-entity");
    insert(
        cwd,
        &format!("<{doc}> rdf:type ops:Document ;\n  ops:name \"entity linked\" ;\n  ops:relatedTo <{ent}> .\n"),
    );

    let out = migrate(cwd);
    assert_eq!(out.total_linked(), 1, "the object IRI kind is the discriminator: {out:?}");
    assert!(ask(cwd, &format!("<{doc}> ops:hasDomain ?d")));
}

#[test]
fn an_unhealthy_graph_is_skipped_and_not_stamped() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");
    let mut raw = std::fs::read_to_string(graph_path(cwd)).unwrap();
    raw.push_str("this is not a quad\n");
    std::fs::write(graph_path(cwd), &raw).unwrap();

    let out = migrate(cwd);
    assert!(out.skipped_unhealthy, "defined behaviour, not silence: {out:?}");
    assert!(!out.stamped, "a skipped migration must not claim it ran");
    assert!(out.backup.is_none());
    assert!(
        migrate::format_outcomes(&[out]).contains("doctor --repair"),
        "and it must say so, with the next step"
    );
}

#[test]
fn the_migration_never_invents_a_domain() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");
    plant_orphan(cwd, "reminder", "Reminder", "call-someone");
    migrate(cwd);

    let q = "SELECT DISTINCT ?d WHERE { GRAPH ?g { ?d a ops:Domain } }";
    let oxigraph::sparql::QueryResults::Solutions(sols) = crud::load_and_query(cwd, &ns(), q).unwrap()
    else {
        panic!("expected solutions")
    };
    let mut domains: Vec<String> = sols
        .filter_map(|r| r.ok())
        .filter_map(|row| match row.get("d")? {
            oxigraph::model::Term::NamedNode(n) => {
                n.as_str().rsplit_once("domain/").map(|(_, s)| s.to_string())
            }
            _ => None,
        })
        .collect();
    domains.sort();
    domains.dedup();
    assert_eq!(
        domains,
        vec!["probe".to_string(), migrate::CATCHALL.to_string()],
        "only the declared domain and the catchall exist — no domain was invented"
    );
}

#[test]
fn the_catchall_record_is_created_once_not_per_record() {
    let tmp = workspace();
    let cwd = tmp.path();
    for n in 0..5 {
        plant_orphan(cwd, "goal", "Goal", &format!("goal-{n}"));
    }
    let out = migrate(cwd);
    assert_eq!(out.catchall, 5);

    let raw = std::fs::read_to_string(graph_path(cwd)).unwrap();
    let unfiled_type = format!(
        "<{}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
        crud::build_iri(&ns(), "domain", migrate::CATCHALL)
    );
    assert_eq!(
        raw.matches(&unfiled_type).count(),
        1,
        "one catchall domain record, however many records point at it"
    );
}

#[test]
fn a_store_with_nothing_to_do_still_ends_stamped() {
    let tmp = workspace();
    let cwd = tmp.path();
    let out = migrate(cwd);
    assert_eq!(out.total_linked(), 0);
    assert!(out.stamped, "so the next session takes the fast path instead of re-planning");
    assert!(!migrate::format_outcomes(&[out]).contains("linked"), "and says nothing about it");
}
