//! A covered record takes its domain at CREATION when its parent already knows it.
//!
//! kite F7b, vole's ruling (2026-09-06): "every record carries a domain" must stay true
//! between session starts, not only after the next one. The session-start delta pass
//! (`migrate.rs`) still files whatever arrives without a link; these paths simply do not
//! make it wait.
//!
//! Task and Milestone take their project's domain. A Handoff (and a fork) takes the
//! domain of the project it names. An extension ingest source may declare a `domain`
//! and every record it writes carries it (ruling 3, G0 verdict A3). A parent with no
//! domain links nothing at creation, and the manual migration then files the record
//! as the catchall, so the two owners of "where does the domain come from" never
//! disagree.

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::extension::ingest::ingest_extension;
use base::extension::{ExtensionDef, ExtensionHooks, IngestSource, SessionStartHook};
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
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = [\"probe\"]\nrules = [\"r\"]\n",
    )
    .unwrap();
    let config = BaseConfig::load(tmp.path());
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), None).unwrap();
    tmp
}

fn graph_iri(cwd: &Path) -> String {
    crud::workspace_graph_iri(&ns(), &crud::workspace_slug(cwd))
}

fn insert(cwd: &Path, triples: &str) {
    let sparql = format!("INSERT DATA {{ GRAPH <{}> {{\n{triples}\n}} }}", graph_iri(cwd));
    crud::load_and_mutate(cwd, &ns(), &sparql).unwrap();
}

/// A project, with or without a domain, as `base project add` leaves one.
fn plant_project(cwd: &Path, slug: &str, domain: Option<&str>) -> String {
    let iri = crud::build_iri(&ns(), "project", slug);
    let link = domain
        .map(|d| format!("  ops:hasDomain <{}> ;\n", crud::build_iri(&ns(), "domain", d)))
        .unwrap_or_default();
    insert(cwd, &format!("<{iri}> rdf:type ops:Project ;\n{link}  ops:name \"{slug}\" .\n"));
    iri
}

/// The domain slugs a record carries under `hasDomain`, sorted.
fn domains_of(cwd: &Path, iri: &str) -> Vec<String> {
    let q = format!("SELECT ?d WHERE {{ GRAPH ?g {{ <{iri}> ops:hasDomain ?d }} }}");
    let oxigraph::sparql::QueryResults::Solutions(sols) =
        crud::load_and_query(cwd, &ns(), &q).unwrap()
    else {
        return Vec::new();
    };
    let mut v: Vec<String> = sols
        .filter_map(|r| r.ok())
        .filter_map(|row| match row.get("d")? {
            oxigraph::model::Term::NamedNode(n) => {
                n.as_str().rsplit_once("domain/").map(|(_, s)| s.to_string())
            }
            _ => None,
        })
        .collect();
    v.sort();
    v
}

#[test]
fn a_task_takes_its_projects_domain_at_creation() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_project(cwd, "kit", Some("probe"));

    let slug = crud::task::add(cwd, &ns(), "kit", "write the brief", None, None).unwrap();
    let task = crud::build_iri(&ns(), "task", &slug);
    assert_eq!(domains_of(cwd, &task), vec!["probe".to_string()], "linked in the same write");

    // Nothing left for the migration to do about it.
    let out = migrate::migrate_tier(
        &cwd.join(".base").join("graph.nq"),
        &graph_iri(cwd),
        &ns(),
        migrate::Trigger::Manual,
    )
    .unwrap();
    assert_eq!(out.linked.get("Task"), None, "already linked, not re-filed: {out:?}");
}

#[test]
fn a_milestone_takes_its_projects_domain_at_creation() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_project(cwd, "kit", Some("probe"));

    let slug = crud::milestone::add(cwd, &ns(), "kit", "phase one", None).unwrap();
    let ms = crud::build_iri(&ns(), "milestone", &slug);
    assert_eq!(domains_of(cwd, &ms), vec!["probe".to_string()]);
}

#[test]
fn a_handoff_and_a_fork_take_the_domain_of_the_project_they_name() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_project(cwd, "kit", Some("probe"));

    let h = crud::handoff::create(None, cwd, &ns(), "kit", "docs/2026-09-06-kit-handoff.md", None)
        .unwrap()
        .slug;
    let f = crud::handoff::create_fork(cwd, &ns(), "kit", "docs/kit-side-quest.md", None).unwrap();
    for slug in [h, f] {
        let iri = crud::build_iri(&ns(), "handoff", &slug);
        assert_eq!(domains_of(cwd, &iri), vec!["probe".to_string()], "{slug}");
    }
}

#[test]
fn a_parent_with_no_domain_links_nothing_and_the_migration_files_the_record() {
    // Two owners of "where does the domain come from" must not disagree: creation
    // writes a link only when the parent has one, and the catchall stays the
    // migration's decision alone.
    let tmp = workspace();
    let cwd = tmp.path();
    plant_project(cwd, "loose", None);

    let slug = crud::task::add(cwd, &ns(), "loose", "an orphan task", None, None).unwrap();
    let task = crud::build_iri(&ns(), "task", &slug);
    assert!(domains_of(cwd, &task).is_empty(), "no domain to inherit, so no link yet");

    let out = migrate::migrate_tier(
        &cwd.join(".base").join("graph.nq"),
        &graph_iri(cwd),
        &ns(),
        migrate::Trigger::Manual,
    )
    .unwrap();
    assert_eq!(out.linked.get("Task"), Some(&1), "{out:?}");
    assert_eq!(domains_of(cwd, &task), vec![migrate::CATCHALL.to_string()]);
}

#[test]
fn a_task_is_linked_exactly_once_even_though_both_paths_could_link_it() {
    // Creation links it; a later migration must see the link (either predicate,
    // `link::domain_index`) and not add a second one.
    let tmp = workspace();
    let cwd = tmp.path();
    plant_project(cwd, "kit", Some("probe"));
    let slug = crud::task::add(cwd, &ns(), "kit", "once", None, None).unwrap();
    let task = crud::build_iri(&ns(), "task", &slug);

    migrate::migrate_tier(
        &cwd.join(".base").join("graph.nq"),
        &graph_iri(cwd),
        &ns(),
        migrate::Trigger::Manual,
    )
    .unwrap();
    let raw = std::fs::read_to_string(cwd.join(".base").join("graph.nq")).unwrap();
    let marker = format!("<{task}> <http://ops-sys.local/ontology#hasDomain>");
    assert_eq!(raw.matches(&marker).count(), 1, "one link, not two");
}

#[test]
fn an_ingest_source_that_declares_a_domain_files_every_record_under_it() {
    let tmp = workspace();
    let cwd = tmp.path();
    let state_dir = cwd.join(".lore");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("facts.json"),
        r#"[{"id": "riverwood-mill", "text": "the mill is Gerdur's"}, {"id": "hod", "text": "Hod runs the mill"}]"#,
    )
    .unwrap();

    let ext = ExtensionDef {
        name: "lore".into(),
        version: "0.1.0".into(),
        description: "world model".into(),
        framework_dir: None,
        state_dir: Some(".lore".into()),
        hooks: Some(ExtensionHooks {
            session_start: Some(SessionStartHook {
                queries: vec![],
                ingest: vec![IngestSource {
                    file: "facts.json".into(),
                    entity: "LoreFact".into(),
                    strategy: "upsert".into(),
                    domain: Some("skyrim-companion".into()),
                }],
                inject: None,
            }),
            user_prompt: None,
            pre_tool: None,
            post_tool: None,
        }),
        commands: vec![],
        dist: None,
        source_path: None,
    };
    let config = BaseConfig::load(cwd);
    let stats = ingest_extension(&ext, cwd, &config).unwrap();
    assert_eq!(stats.entities, 2);

    for id in ["riverwood-mill", "hod"] {
        let iri = crud::build_iri(&ns(), "ext/lore/LoreFact", id);
        assert_eq!(domains_of(cwd, &iri), vec!["skyrim-companion".to_string()], "{id}");
    }
}

#[test]
fn an_ingest_source_with_no_domain_writes_none_and_the_migration_files_it() {
    let tmp = workspace();
    let cwd = tmp.path();
    let state_dir = cwd.join(".lore");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("facts.json"), r#"[{"id": "one", "text": "a fact"}]"#).unwrap();

    let ext = ExtensionDef {
        name: "lore".into(),
        version: "0.1.0".into(),
        description: "world model".into(),
        framework_dir: None,
        state_dir: Some(".lore".into()),
        hooks: Some(ExtensionHooks {
            session_start: Some(SessionStartHook {
                queries: vec![],
                ingest: vec![IngestSource {
                    file: "facts.json".into(),
                    entity: "LoreFact".into(),
                    strategy: "upsert".into(),
                    domain: None,
                }],
                inject: None,
            }),
            user_prompt: None,
            pre_tool: None,
            post_tool: None,
        }),
        commands: vec![],
        dist: None,
        source_path: None,
    };
    let config = BaseConfig::load(cwd);
    ingest_extension(&ext, cwd, &config).unwrap();
    let iri = crud::build_iri(&ns(), "ext/lore/LoreFact", "one");
    assert!(domains_of(cwd, &iri).is_empty(), "no declaration, no link at creation");

    let out = migrate::migrate_tier(
        &cwd.join(".base").join("graph.nq"),
        &graph_iri(cwd),
        &ns(),
        migrate::Trigger::Manual,
    )
    .unwrap();
    assert_eq!(out.by_arm.get("fixed"), Some(&1), "ruling 3's fixed arm files it: {out:?}");
    assert_eq!(domains_of(cwd, &iri), vec!["skyrim-companion".to_string()]);
}
