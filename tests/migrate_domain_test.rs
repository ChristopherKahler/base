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

/// The session-start path: takes the stamp as a fast path.
fn migrate(cwd: &Path) -> migrate::Outcome {
    migrate::migrate_tier(&graph_path(cwd), &graph_iri(cwd), &ns(), migrate::Trigger::SessionStart)
        .unwrap()
}

/// `base graph migrate`: always re-plans past the stamp.
fn migrate_manual(cwd: &Path) -> migrate::Outcome {
    migrate::migrate_tier(&graph_path(cwd), &graph_iri(cwd), &ns(), migrate::Trigger::Manual)
        .unwrap()
}

/// The `.bak-migrate-*` snapshots on disk.
fn snapshots(cwd: &Path) -> Vec<String> {
    let dir = cwd.join(".base");
    let mut v: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".bak-migrate-"))
        .collect();
    v.sort();
    v
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
fn a_pass_with_nothing_to_link_takes_no_snapshot() {
    // The comment, `docs/graph-durability.md` and the outcome's own `backup: None`
    // all say a no-op takes no snapshot. The code took one anyway, and the test
    // that looked like it covered this passed only because the STAMP fast path
    // returned first — so the unstamped-empty-plan case was never exercised
    // (kite, PR #50). Snapshots share one pool of ten with compact; a pass that
    // writes nothing but the stamp must not evict a real backup.
    let tmp = workspace();
    let cwd = tmp.path();

    let out = migrate(cwd);
    assert_eq!(out.total_linked(), 0, "nothing to link in a fresh workspace: {out:?}");
    assert!(out.stamped, "it still stamps, so session start takes the fast path next time");
    assert_eq!(out.backup, None, "and takes NO snapshot");
    assert!(snapshots(cwd).is_empty(), "nothing on disk either: {:?}", snapshots(cwd));

    // The control: a FIRST migration with work to do does snapshot. On a tier that is
    // already stamped, later work is a delta pass, which takes none by design: it only
    // adds quads onto a store already at this schema (e2a5d4d, vole's amendment to
    // kite F7), so the control has to be a fresh, unstamped tier.
    let tmp2 = workspace();
    let cwd2 = tmp2.path();
    plant_orphan(cwd2, "goal", "Goal", "ship-the-thing");
    let out = migrate(cwd2);
    assert_eq!(out.total_linked(), 1);
    assert!(!out.delta, "a first migration is not a delta: {out:?}");
    assert!(out.backup.is_some(), "real work is protected: {out:?}");
    assert_eq!(snapshots(cwd2).len(), 1);
}

#[test]
fn the_manual_command_re_plans_past_the_stamp() {
    // `base graph migrate` printed "Nothing to migrate" on a stamped tier while
    // `base doctor` reported `without a domain: N` on the same tier in the same
    // second. Both cannot be true (kite, PR #50). The stamp is session start's
    // fast path and must never answer a question the operator asked directly.
    let tmp = workspace();
    let cwd = tmp.path();
    migrate(cwd);

    // Drift: something writes an unlinked record after the migration ran.
    plant_orphan(cwd, "goal", "Goal", "written-later");
    // Pin the delta marker newer than the store, so the hook's mtime gate reads "nothing
    // wrote since I last looked" and takes the fast path. That is the state this test is
    // about: a stamped tier the hook has no reason to re-plan, and drift that only the
    // operator's own command goes looking for. (With the store newer than the marker the
    // hook would file it itself — `a_record_written_after_the_migration_is_linked_at_the_next_session`.)
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(cwd.join(".base").join(".last-domain-delta"), "pinned by the test").unwrap();

    let hook = migrate(cwd);
    assert!(hook.already_migrated, "the hook still takes the fast path: {hook:?}");
    assert_eq!(hook.total_linked(), 0);

    let manual = migrate_manual(cwd);
    assert!(!manual.already_migrated, "the operator asked, so it re-planned: {manual:?}");
    assert_eq!(manual.total_linked(), 1, "and found the drift");
    let goal = crud::build_iri(&ns(), "goal", "written-later");
    assert!(ask(cwd, &format!("<{goal}> ops:hasDomain ?d")));
}

#[test]
fn the_manual_run_agrees_with_doctor_on_the_same_tier() {
    // Three surfaces answer one question — doctor, `--dry-run`, and the run —
    // and they are not allowed to differ.
    let tmp = workspace();
    let cwd = tmp.path();
    migrate(cwd);
    for n in 0..3 {
        plant_orphan(cwd, "goal", "Goal", &format!("later-{n}"));
    }

    let reported: usize = base::doctor::diagnose_tier("workspace", &graph_path(cwd))
        .domain_orphans
        .iter()
        .map(|(_, n)| n)
        .sum();
    assert_eq!(reported, 3, "doctor sees the drift");

    let dry = base::migrate::format_dry_run(cwd, &ns());
    assert!(dry.contains("would link 3 record(s)"), "the dry run agrees: {dry}");

    let manual = migrate_manual(cwd);
    assert_eq!(manual.total_linked(), reported, "and so does the run: {manual:?}");

    let after: usize = base::doctor::diagnose_tier("workspace", &graph_path(cwd))
        .domain_orphans
        .iter()
        .map(|(_, n)| n)
        .sum();
    assert_eq!(after, 0, "doctor is clean afterwards");
}

#[test]
fn a_manual_run_with_nothing_to_do_still_writes_nothing() {
    // Re-planning past the stamp must not cost a 13.4 MB rewrite for no change.
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");
    migrate(cwd);

    let before = bytes(&graph_path(cwd));
    let out = migrate_manual(cwd);
    assert!(out.already_migrated, "re-planned, found nothing: {out:?}");
    assert!(!out.stamped);
    assert_eq!(out.backup, None);
    assert_eq!(before, bytes(&graph_path(cwd)), "byte-identical");
}

#[test]
fn a_record_written_after_the_migration_is_linked_at_the_next_session() {
    // "Every record carries a domain" was false one write later (kite F7):
    // `base sync` and the PAUL ingest manufacture unlinked records every session,
    // and a stamped tier used to skip them forever.
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");
    let first = migrate(cwd);
    assert_eq!(first.total_linked(), 1);
    assert!(!first.delta, "the first pass is a migration, not a delta");

    // Something writes an unlinked record afterwards.
    plant_orphan(cwd, "document", "Document", "written-later");

    let second = migrate(cwd);
    assert!(second.delta, "an already-stamped tier does a DELTA pass: {second:?}");
    assert_eq!(second.total_linked(), 1, "and finds the new record");
    assert_eq!(second.backup, None, "a delta adds quads only — no snapshot");
    assert!(snapshots(cwd).len() == 1, "still just the first migration's: {:?}", snapshots(cwd));
    let doc = crud::build_iri(&ns(), "document", "written-later");
    assert!(ask(cwd, &format!("<{doc}> ops:hasDomain ?d")));

    // The claim, stated as a test: nothing is left unlinked.
    let left: usize = base::doctor::diagnose_tier("workspace", &graph_path(cwd))
        .domain_orphans
        .iter()
        .map(|(_, n)| n)
        .sum();
    assert_eq!(left, 0);
}

#[test]
fn a_session_that_wrote_nothing_costs_no_parse() {
    // The delta pass must not re-plan a 13.4 MB store every session start. It is
    // gated on the store's own mtime against a marker — one `stat` — so a session
    // that changed nothing is skipped outright.
    let tmp = workspace();
    let cwd = tmp.path();
    plant_orphan(cwd, "goal", "Goal", "ship-the-thing");
    migrate(cwd);

    let before = bytes(&graph_path(cwd));
    let out = migrate(cwd);
    assert!(out.already_migrated, "nothing wrote, so nothing was looked at: {out:?}");
    assert!(!out.delta);
    assert_eq!(out.total_linked(), 0);
    assert_eq!(before, bytes(&graph_path(cwd)), "and the store is byte-identical");
    assert!(
        cwd.join(".base").join(".last-domain-delta").exists(),
        "the marker is what makes the skip possible"
    );
}

#[test]
fn the_delta_pass_reruns_once_the_store_moves_again() {
    let tmp = workspace();
    let cwd = tmp.path();
    migrate(cwd);
    assert!(migrate(cwd).already_migrated, "quiet session, skipped");

    // A write moves the store's mtime past the marker.
    plant_orphan(cwd, "reminder", "Reminder", "call-someone");
    let out = migrate(cwd);
    assert!(out.delta, "the store moved, so the pass ran: {out:?}");
    assert_eq!(out.total_linked(), 1);
}

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
