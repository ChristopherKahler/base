//! Where each family's domain comes from — one test per arm, plus the precedence
//! between them.
//!
//! Every parent link asserted here was read off Chris's live store with a
//! read-only probe (lark, 2026-09-06), not inferred from the ontology, which does
//! not declare half these classes. The shapes:
//!
//! ```text
//! AcceptanceCriteria        <- plan     ops:hasAC          (reverse)
//! AcceptanceCriteriaResult  <- summary  ops:hasACResult    (reverse)
//! FileChange                <- summary  ops:hasFileChange  (reverse)
//! Decision                  <- summary  ops:hasDecision    (reverse)
//! Task                      <- project  ops:hasTask        (reverse)
//! Handoff                   ops:project "vintrix"          (literal, not an IRI)
//! CodeMap                   ops:name    "grazer"           (literal app name)
//! Lore*                     nothing at all — every property is a literal
//! ```

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::migrate::{self, Arm};

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// A workspace whose domains.toml is given verbatim, so a test can declare path
/// triggers.
fn workspace_with(domains_toml: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(base_dir.join("graph.nq"), "").unwrap();
    std::fs::write(
        base_dir.join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n",
    )
    .unwrap();
    std::fs::write(base_dir.join("domains.toml"), domains_toml).unwrap();
    let config = BaseConfig::load(tmp.path());
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), None).unwrap();
    tmp
}

fn workspace() -> tempfile::TempDir {
    workspace_with(
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = [\"probe\"]\nrules = [\"r\"]\n",
    )
}

fn graph_iri(cwd: &Path) -> String {
    crud::workspace_graph_iri(&ns(), &crud::workspace_slug(cwd))
}

fn insert(cwd: &Path, triples: &str) {
    let sparql = format!("INSERT DATA {{ GRAPH <{}> {{\n{triples}\n}} }}", graph_iri(cwd));
    crud::load_and_mutate(cwd, &ns(), &sparql).unwrap();
}

fn migrate(cwd: &Path) -> migrate::Outcome {
    migrate::migrate_tier(&cwd.join(".base").join("graph.nq"), &graph_iri(cwd), &ns()).unwrap()
}

/// The domain slug a record ended up with, read back off disk.
fn domain_of(cwd: &Path, iri: &str) -> Option<String> {
    let q = format!("SELECT ?d WHERE {{ GRAPH ?g {{ <{iri}> ops:hasDomain ?d }} }}");
    let oxigraph::sparql::QueryResults::Solutions(sols) =
        crud::load_and_query(cwd, &ns(), &q).unwrap()
    else {
        return None;
    };
    sols.filter_map(|r| r.ok())
        .filter_map(|row| match row.get("d")? {
            oxigraph::model::Term::NamedNode(n) => {
                n.as_str().rsplit_once("domain/").map(|(_, s)| s.to_string())
            }
            _ => None,
        })
        .next()
}

/// A project with a domain, as `base project add --domain` writes one.
fn plant_project(cwd: &Path, slug: &str, domain: &str, path: Option<&str>) -> String {
    let iri = crud::build_iri(&ns(), "project", slug);
    let dom = crud::build_iri(&ns(), "domain", domain);
    let p = path.map(|p| format!("  ops:path \"{p}\" ;\n")).unwrap_or_default();
    insert(
        cwd,
        &format!("<{iri}> rdf:type ops:Project ;\n{p}  ops:name \"{slug}\" ;\n  ops:hasDomain <{dom}> .\n"),
    );
    iri
}

/// A markdown file on disk plus its record, the shape `base sync` writes. `class`
/// is `Document`, `PaulPlan` or `PaulSummary` — the Paul pair share the
/// `document/` IRI space and carry `ops:path` the same way, and no subject in
/// Chris's store carries two rdf:types (measured, 2026-09-06).
fn plant_doclike(cwd: &Path, class: &str, rel: &str, frontmatter: &str) -> String {
    let full = cwd.join(rel);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, format!("---\n{frontmatter}---\n\n# body\n")).unwrap();
    let slug = crud::slugify(rel);
    let iri = crud::build_iri(&ns(), "document", &slug);
    insert(
        cwd,
        &format!("<{iri}> rdf:type ops:{class} ;\n  ops:name \"{slug}\" ;\n  ops:path \"{rel}\" .\n"),
    );
    iri
}

fn plant_document(cwd: &Path, rel: &str, frontmatter: &str) -> String {
    plant_doclike(cwd, "Document", rel, frontmatter)
}

// ─── ruling 3: the Lore family ───────────────────────────────────────────────

#[test]
fn the_lore_family_takes_skyrim_companion_and_the_domain_is_created_if_absent() {
    let tmp = workspace();
    let cwd = tmp.path();
    let mut iris = Vec::new();
    for (n, class) in [
        (0, "LoreKnowledge"),
        (1, "LoreFact"),
        (2, "LoreRelationship"),
        (3, "LoreItem"),
        (4, "LoreBelief"),
    ] {
        let iri = format!("http://ops-sys.local/ontology#ext/lore/{class}/{n}");
        // Exactly the shape the extension writes: every property a literal, no
        // IRI edges at all, so nothing but a fixed assignment can reach these.
        insert(cwd, &format!("<{iri}> rdf:type ops:{class} ;\n  ops:source \"ext:lore\" .\n"));
        iris.push(iri);
    }

    let out = migrate(cwd);
    assert_eq!(out.total_linked(), 5, "{out:?}");
    assert_eq!(out.by_arm.get("fixed"), Some(&5), "all five by the fixed arm");
    assert_eq!(out.catchall, 0, "ruling 3 is a real domain, not the catchall");
    for iri in &iris {
        assert_eq!(domain_of(cwd, iri).as_deref(), Some("skyrim-companion"), "{iri}");
    }
    // G0 verdict A3: created once, not invented per record.
    let raw = std::fs::read_to_string(cwd.join(".base").join("graph.nq")).unwrap();
    let marker = "<http://ops-sys.local/ontology#domain/skyrim-companion> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    assert_eq!(raw.matches(marker).count(), 1, "one skyrim-companion domain record");
}

// ─── parent walks, all reverse ───────────────────────────────────────────────

#[test]
fn paul_artefacts_inherit_the_plan_or_summary_that_owns_them() {
    let tmp = workspace_with(
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"phases\"]\nrules = [\"r\"]\n",
    );
    let cwd = tmp.path();

    // A plan and a summary, both filed by the path-trigger arm in stage 1.
    let plan = plant_doclike(cwd, "PaulPlan", "phases/01-01-PLAN.md", "");
    let summary = plant_doclike(cwd, "PaulSummary", "phases/01-01-SUMMARY.md", "");

    let ac = crud::build_iri(&ns(), "acceptance-criteria", "01-01-ac-1");
    let acr = crud::build_iri(&ns(), "ac-result", "01-01-ac-1");
    let fc = crud::build_iri(&ns(), "file-change", "01-01-src-lib-rs");
    let dec = crud::build_iri(&ns(), "decision", "01-01-a-choice");
    insert(
        cwd,
        &format!(
            "<{ac}> rdf:type ops:AcceptanceCriteria ; ops:name \"AC-1\" .\n\
             <{acr}> rdf:type ops:AcceptanceCriteriaResult ; ops:criterion \"AC-1\" .\n\
             <{fc}> rdf:type ops:FileChange ; ops:filePath \"src/lib.rs\" .\n\
             <{dec}> rdf:type ops:Decision ; ops:rationale \"because\" .\n\
             <{plan}> ops:hasAC <{ac}> .\n\
             <{summary}> ops:hasACResult <{acr}> .\n\
             <{summary}> ops:hasFileChange <{fc}> .\n\
             <{summary}> ops:hasDecision <{dec}> .\n"
        ),
    );

    let out = migrate(cwd);
    for (iri, what) in [(&ac, "AC"), (&acr, "AC result"), (&fc, "file change"), (&dec, "decision")] {
        assert_eq!(
            domain_of(cwd, iri).as_deref(),
            Some("probe"),
            "{what} must inherit the document that owns it, filed in the same pass: {out:?}"
        );
    }
    assert_eq!(out.by_arm.get("parent"), Some(&4));
    assert_eq!(out.by_arm.get("path-trigger"), Some(&2), "the plan and the summary");
    assert_eq!(out.catchall, 0);
}

#[test]
fn a_task_inherits_its_project() {
    let tmp = workspace();
    let cwd = tmp.path();
    let proj = plant_project(cwd, "meet-caddy", "probe", None);
    let task = crud::build_iri(&ns(), "task", "meet-caddy.do-the-thing");
    insert(cwd, &format!("<{task}> rdf:type ops:Task ; ops:name \"do the thing\" .\n"));
    insert(cwd, &format!("<{proj}> ops:hasTask <{task}> .\n"));

    migrate(cwd);
    assert_eq!(domain_of(cwd, &task).as_deref(), Some("probe"));
}

#[test]
fn a_decision_a_domain_already_points_at_is_not_an_orphan() {
    // `crud/decision.rs:47` writes `<domain> ops:hasDecision <decision>`. A
    // subject-side-only orphan test reads all 420 of Chris's decisions as orphans
    // and files them under `unfiled` on top of the domain they already have.
    let tmp = workspace();
    let cwd = tmp.path();
    let dec = crud::build_iri(&ns(), "decision", "probe.a-real-decision");
    let dom = crud::build_iri(&ns(), "domain", "probe");
    insert(cwd, &format!("<{dec}> rdf:type ops:Decision ; ops:rationale \"r\" .\n"));
    insert(cwd, &format!("<{dom}> ops:hasDecision <{dec}> .\n"));

    let out = migrate(cwd);
    assert_eq!(out.total_linked(), 0, "already linked, the other way round: {out:?}");
    assert_eq!(domain_of(cwd, &dec), None, "and not given a second, wrong link");
}

// ─── literal-named parents ───────────────────────────────────────────────────

#[test]
fn a_handoff_takes_the_domain_of_the_project_named_in_its_literal() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_project(cwd, "vintrix", "probe", None);
    let h = crud::build_iri(&ns(), "handoff", "2026-09-05-0642-egret-vintrix");
    insert(
        cwd,
        &format!("<{h}> rdf:type ops:Handoff ; ops:name \"vintrix\" ; ops:project \"vintrix\" .\n"),
    );

    migrate(cwd);
    assert_eq!(domain_of(cwd, &h).as_deref(), Some("probe"));
}

#[test]
fn a_codemap_takes_the_domain_of_the_app_it_maps() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_project(cwd, "grazer", "probe", None);
    let c = crud::build_iri(&ns(), "codemap", "grazer");
    insert(cwd, &format!("<{c}> rdf:type ops:CodeMap ; ops:name \"grazer\" .\n"));

    migrate(cwd);
    assert_eq!(domain_of(cwd, &c).as_deref(), Some("probe"));
}

#[test]
fn a_handoff_naming_an_unknown_project_takes_the_catchall() {
    let tmp = workspace();
    let cwd = tmp.path();
    let h = crud::build_iri(&ns(), "handoff", "2026-09-05-nobody");
    insert(cwd, &format!("<{h}> rdf:type ops:Handoff ; ops:project \"no-such-project\" .\n"));

    let out = migrate(cwd);
    assert_eq!(domain_of(cwd, &h).as_deref(), Some(migrate::CATCHALL));
    assert_eq!(out.by_arm.get(migrate::CATCHALL), Some(&1), "counted as a catchall, not hidden");
}

// ─── ruling 2: the document arms, and their order ───────────────────────────

#[test]
fn frontmatter_wins_when_it_names_a_domain_that_exists() {
    let tmp = workspace_with(
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"notes\"]\nrules = [\"r\"]\n\n\
         [[domain]]\nname = \"declared\"\nmode = \"triggered\"\nkeywords = []\nrules = [\"r\"]\n",
    );
    let cwd = tmp.path();
    let d = plant_document(cwd, "notes/a.md", "domain: declared\n");

    let out = migrate(cwd);
    assert_eq!(
        domain_of(cwd, &d).as_deref(),
        Some("declared"),
        "frontmatter is the first arm, ahead of the path trigger that also matches"
    );
    assert_eq!(out.by_arm.get("frontmatter"), Some(&1));
}

#[test]
fn frontmatter_naming_a_domain_that_does_not_exist_falls_through() {
    // "Never invent a domain record" — a typo in a doc's frontmatter must not
    // create a domain, and must not stop the next arm working.
    let tmp = workspace_with(
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"notes\"]\nrules = [\"r\"]\n",
    );
    let cwd = tmp.path();
    let d = plant_document(cwd, "notes/a.md", "domain: not-a-real-domain\n");

    migrate(cwd);
    assert_eq!(domain_of(cwd, &d).as_deref(), Some("probe"), "falls through to the path trigger");
    let raw = std::fs::read_to_string(cwd.join(".base").join("graph.nq")).unwrap();
    assert!(!raw.contains("domain/not-a-real-domain"), "and invents nothing");
}

#[test]
fn a_path_trigger_files_a_document_with_no_frontmatter_domain() {
    let tmp = workspace_with(
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"Documents/video-gen\"]\nrules = [\"r\"]\n",
    );
    let cwd = tmp.path();
    let hit = plant_document(cwd, "Documents/video-gen/a.md", "title: A\n");
    let miss = plant_document(cwd, "Documents/elsewhere/b.md", "title: B\n");

    let out = migrate(cwd);
    assert_eq!(domain_of(cwd, &hit).as_deref(), Some("probe"));
    assert_eq!(domain_of(cwd, &miss).as_deref(), Some(migrate::CATCHALL));
    assert_eq!(out.by_arm.get("path-trigger"), Some(&1));
    assert_eq!(out.by_arm.get(migrate::CATCHALL), Some(&1));
}

#[test]
fn the_longest_path_trigger_wins_and_a_tie_is_deterministic() {
    // Chris's workspace declares `tools` twice — skyrim-companion and
    // asset-inventory — so something must decide it, and decide it the same way
    // on every run. Longest trigger first; a tie keeps `load_domains` order.
    let tmp = workspace_with(
        "[[domain]]\nname = \"broad\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"tools\"]\nrules = [\"r\"]\n\n\
         [[domain]]\nname = \"narrow\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"tools/stt\"]\nrules = [\"r\"]\n\n\
         [[domain]]\nname = \"alsobroad\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"tools\"]\nrules = [\"r\"]\n",
    );
    let cwd = tmp.path();
    let deep = plant_document(cwd, "tools/stt/a.md", "title: A\n");
    let shallow = plant_document(cwd, "tools/other/b.md", "title: B\n");

    migrate(cwd);
    assert_eq!(domain_of(cwd, &deep).as_deref(), Some("narrow"), "longest trigger wins");
    let tie = domain_of(cwd, &shallow);
    assert_eq!(tie.as_deref(), Some("broad"), "a tie keeps declaration order, first wins");

    // Determinism: the same store migrated again from scratch agrees.
    let tmp2 = workspace_with(
        "[[domain]]\nname = \"broad\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"tools\"]\nrules = [\"r\"]\n\n\
         [[domain]]\nname = \"narrow\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"tools/stt\"]\nrules = [\"r\"]\n\n\
         [[domain]]\nname = \"alsobroad\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"tools\"]\nrules = [\"r\"]\n",
    );
    let cwd2 = tmp2.path();
    let shallow2 = plant_document(cwd2, "tools/other/b.md", "title: B\n");
    migrate(cwd2);
    assert_eq!(domain_of(cwd2, &shallow2), tie, "two runs, same answer");
}

#[test]
fn a_project_path_files_a_document_no_trigger_reached() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_project(cwd, "renda", "probe", Some("Documents/Meet Caddy/Renda Group"));
    let d = plant_document(cwd, "Documents/Meet Caddy/Renda Group/BRIEF.md", "title: Brief\n");

    let out = migrate(cwd);
    assert_eq!(domain_of(cwd, &d).as_deref(), Some("probe"));
    assert_eq!(out.by_arm.get("project-path"), Some(&1));
}

#[test]
fn a_project_whose_path_is_one_top_level_folder_claims_nothing() {
    // `vintrix` has path "Documents" and `asset-inventory` has "tools" in Chris's
    // store. Honouring those as prefixes files 377 unrelated documents under them.
    let tmp = workspace();
    let cwd = tmp.path();
    plant_project(cwd, "vintrix", "probe", Some("Documents"));
    let d = plant_document(cwd, "Documents/something-unrelated/x.md", "title: X\n");

    let out = migrate(cwd);
    assert_eq!(
        domain_of(cwd, &d).as_deref(),
        Some(migrate::CATCHALL),
        "a bare top-level project path is mis-filing, not coverage: {out:?}"
    );
}

// ─── the log ─────────────────────────────────────────────────────────────────

#[test]
fn the_log_names_every_arm_including_the_catchall() {
    let tmp = workspace_with(
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"notes\"]\nrules = [\"r\"]\n",
    );
    let cwd = tmp.path();
    plant_document(cwd, "notes/a.md", "title: A\n");
    plant_document(cwd, "elsewhere/b.md", "title: B\n");
    insert(cwd, "<http://ops-sys.local/ontology#ext/lore/LoreFact/1> rdf:type ops:LoreFact .\n");

    let out = migrate(cwd);
    let line = migrate::format_outcomes(std::slice::from_ref(&out));
    assert!(line.contains("by source:"), "{line}");
    assert!(line.contains("path-trigger 1"), "{line}");
    assert!(line.contains("fixed 1"), "{line}");
    assert!(
        line.contains(&format!("{} 1", migrate::CATCHALL)),
        "the catchall is one arm among the others, never a silent remainder: {line}"
    );
    assert!(line.contains("by kind:"), "{line}");
    assert_eq!(out.by_arm.values().sum::<usize>(), out.total_linked(), "arms account for every record");
}

#[test]
fn a_record_carrying_two_covered_types_is_filed_once() {
    // Legal RDF, absent from Chris's store today, and a double-file would double
    // every number in the migration log — which is the operator's only view of
    // what happened.
    let tmp = workspace_with(
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = []\npaths = [\"notes\"]\nrules = [\"r\"]\n",
    );
    let cwd = tmp.path();
    let d = plant_document(cwd, "notes/a.md", "title: A\n");
    insert(cwd, &format!("<{d}> rdf:type ops:PaulPlan .\n"));

    let out = migrate(cwd);
    assert_eq!(out.total_linked(), 1, "one record, one assignment: {out:?}");
    assert_eq!(out.by_arm.values().sum::<usize>(), 1);
    assert_eq!(domain_of(cwd, &d).as_deref(), Some("probe"));
}

#[test]
fn arm_labels_are_stable() {
    // The migration log is the operator's only view of why a record landed where
    // it did; renaming an arm silently changes what a past log meant.
    assert_eq!(Arm::Frontmatter.label(), "frontmatter");
    assert_eq!(Arm::PathTrigger.label(), "path-trigger");
    assert_eq!(Arm::ProjectPath.label(), "project-path");
    assert_eq!(Arm::Fixed.label(), "fixed");
    assert_eq!(Arm::Parent.label(), "parent");
    assert_eq!(Arm::Catchall.label(), migrate::CATCHALL);
}
