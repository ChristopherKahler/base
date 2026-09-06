//! One-time, idempotent schema migration: every record carries a domain link.
//!
//! Chris's ruling, 2026-09-04: *"we need to make sure everything carries a domain
//! (where relevant) with a catchall default where domain isn't necessary … I don't
//! want people's base systems to break."* 4,051 of 5,099 typed workspace records
//! are orphaned today (79.4%, hawk 2026-09-05), so a community named by domain
//! would name itself after the 20% that happen to be notes and rules.
//!
//! ## Two rules this module exists to obey
//!
//! **The stamp is in the graph, and it lands in the same write as the data.**
//! `write_back` is temp-plus-rename, so the stamp and the records it claims are
//! either both on disk or neither is. A file sentinel beside `~/.base-gbl/` would
//! be the cheaper mechanism and it lies after a `.bak` restore: the store goes back
//! to its pre-migration shape while the file still says "migrated", and the
//! migration then skips forever. After a restore the STORE is the only thing that
//! knows. (G0 verdict A1, hawk F1.)
//!
//! **Stamp WITH the write, never before it.** `auto_compact_tiers` stamps its
//! cooldown marker before compacting, and that is right there — compaction is
//! lossless and idempotent, so a crash costs nothing. Here a crash halfway would
//! leave a half-migrated store marked done, silently, forever. So the work is
//! computed from the store every time ("which covered records still have no domain
//! link"), and the stamp is a fast path, not a truth source. (G0 verdict A2, hawk F2.)
//!
//! ## Why this is safe to run against an installed base
//!
//! Expand-migrate-contract. 0.14.0 writes `hasDomain` beside the existing
//! `relatedTo` and reads both (`domain::link`); N+1 stops writing the legacy
//! triple; N+2 drops it. hawk C10 built v0.12.0 — 105 commits back — and ran it
//! against a store carrying every artefact this migration writes: it stayed
//! HEALTHY, analysed, recalled, and WROTE TWICE, and the artefacts survived. The
//! reverse too. `store::load_graph` is a syntactic N-Quads parse with no schema
//! check, and every read binds only the predicates it wants.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oxigraph::model::{GraphNameRef, LiteralRef, NamedNodeRef, QuadRef, Term};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;
use serde::Serialize;

use crate::changelog::Change;
use crate::config::NamespaceConfig;
use crate::domain::link;
use crate::store::{self, GraphHealth};

/// The schema this module migrates a store to. Written as the object of
/// `<graph> ops:schemaVersion` inside the tier's own named graph.
pub const SCHEMA_VERSION: &str = "domain-1";

/// The catchall domain, for records where a domain is genuinely not relevant.
/// Chris's ruling 1, 2026-09-06.
pub const CATCHALL: &str = "unfiled";

/// Where a record's domain comes from. One variant per family, so the set that
/// lands in the catchall is NAMED here rather than being whatever was left over —
/// which is the acceptance line, not a nicety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Source {
    /// A domain is not a meaningful attribute of this record. Straight to
    /// [`CATCHALL`].
    Catchall,
    /// Fixed domain for the whole family, assigned by the extension that ingests
    /// it (ruling 3 covers the Lore\* family).
    Fixed(&'static str),
    /// Walk one link to a parent record and take the parent's domain; catchall if
    /// the parent has none.
    Parent(&'static str),
    /// The document's own frontmatter, else the workspace's project domain, else
    /// catchall (ruling 2).
    Frontmatter,
}

/// One covered record class and where its domain comes from.
///
/// Everything the migration touches is in this table, and nothing else is touched.
/// A class absent from it keeps whatever it has — `Domain` has no domain of its
/// own, `Ping` is session traffic excluded from every read surface
/// (`ontology::transient`), and `Note`/`Rule` are already 100% linked.
pub struct Covered {
    /// RDF class local name, e.g. `Document` in `ops:Document`.
    pub class: &'static str,
    pub source: Source,
}

/// The families, in the order the migration reports them.
///
/// Counts are hawk's measurement of Chris's live workspace tier, 2026-09-05, and
/// are here so a reader can tell a table that shrank because the migration worked
/// from one that shrank because a class stopped being covered.
pub const COVERED: &[Covered] = &[
    // 999 workspace + 141 global orphans — the largest single block.
    Covered { class: "Document", source: Source::Frontmatter },
    // 1,434 orphans across five classes, all Skyrim (ruling 3).
    Covered { class: "LoreKnowledge", source: Source::Fixed("skyrim-companion") },
    Covered { class: "LoreFact", source: Source::Fixed("skyrim-companion") },
    Covered { class: "LoreRelationship", source: Source::Fixed("skyrim-companion") },
    Covered { class: "LoreItem", source: Source::Fixed("skyrim-companion") },
    Covered { class: "LoreBelief", source: Source::Fixed("skyrim-companion") },
    // 1,241 orphans; every one hangs off a plan that hangs off a project.
    Covered { class: "AcceptanceCriteria", source: Source::Parent("fromPlan") },
    Covered { class: "AcceptanceCriteriaResult", source: Source::Parent("fromPlan") },
    Covered { class: "FileChange", source: Source::Parent("fromPlan") },
    Covered { class: "PaulSummary", source: Source::Parent("fromPlan") },
    Covered { class: "PaulPlan", source: Source::Parent("fromProject") },
    // 209 + 36 orphans; the project is in the filename convention.
    Covered { class: "Handoff", source: Source::Parent("relatedTo") },
    // 142 workspace orphans: 137 hang off a document, 5 are loose.
    Covered { class: "Decision", source: Source::Parent("fromPlan") },
    // 14 orphans; a code map belongs to the app it maps.
    Covered { class: "CodeMap", source: Source::Parent("relatedTo") },
    // Small tails. A task or project with no parent domain, a goal, a reminder:
    // this IS the named catchall set, not a leftover.
    Covered { class: "Task", source: Source::Parent("belongsTo") },
    Covered { class: "Project", source: Source::Catchall },
    Covered { class: "Goal", source: Source::Catchall },
    Covered { class: "Reminder", source: Source::Catchall },
];

/// What one tier's migration did. Serialized by `base doctor --json`.
#[derive(Debug, Default, Serialize)]
pub struct Outcome {
    pub path: String,
    /// Already carried the stamp — nothing read, nothing written.
    pub already_migrated: bool,
    /// The graph was not healthy; nothing was written and nothing was stamped.
    pub skipped_unhealthy: bool,
    pub backup: Option<String>,
    /// Records linked, by RDF class. Empty when the store had no orphans.
    pub linked: BTreeMap<String, usize>,
    /// How many of those landed in [`CATCHALL`].
    pub catchall: usize,
    /// True when the store now carries the stamp because THIS pass wrote it.
    pub stamped: bool,
}

impl Outcome {
    pub fn total_linked(&self) -> usize {
        self.linked.values().sum()
    }
    /// Nothing happened and nothing needed to: the honest "no-op" test.
    pub fn is_noop(&self) -> bool {
        !self.stamped && self.total_linked() == 0
    }
}

// ─── the stamp ───────────────────────────────────────────────────────────────

/// The schema version a store claims, read from `<graph> ops:schemaVersion`.
/// `None` for a store that has never been migrated — including a `.bak` restored
/// from before the migration, which is the whole reason the stamp lives here.
pub fn stamp_of(store: &Store, ns: &NamespaceConfig, graph_iri: &str) -> Option<String> {
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let q = format!(
        "{pfx}\nSELECT ?v WHERE {{ GRAPH <{graph_iri}> {{ <{graph_iri}> {p}:schemaVersion ?v }} }}"
    );
    let QueryResults::Solutions(sols) = store::query(store, &q).ok()? else { return None };
    sols.filter_map(|r| r.ok())
        .filter_map(|row| match row.get("v")? {
            Term::Literal(l) => Some(l.value().to_string()),
            _ => None,
        })
        .next()
}

// ─── the pass ────────────────────────────────────────────────────────────────

/// Migrate one tier's graph file. Path-scoped and pure of the operator's
/// environment, the `compact_tier` shape, so a test drives it against a copy.
///
/// `graph_iri` is the tier's own named graph — the stamp's subject and the graph
/// every backfilled triple is written into.
pub fn migrate_tier(path: &Path, graph_iri: &str, ns: &NamespaceConfig) -> Result<Outcome> {
    let mut out = Outcome { path: path.display().to_string(), ..Default::default() };

    // C1: `compact_tier` refuses an unhealthy graph and a migration that reuses
    // this path inherits that. Defined behaviour, not silence: report it, write
    // nothing, stamp nothing, so the next session tries again once repair has run.
    if !matches!(store::graph_health(path), GraphHealth::Healthy) {
        out.skipped_unhealthy = true;
        return Ok(out);
    }

    let store = store::load_graph(path)?;

    // Fast path. Not a truth source — the work below is recomputed from the store
    // every time the stamp is absent, so a restored pre-migration backup migrates
    // again and a restored post-migration backup is already correct.
    if stamp_of(&store, ns, graph_iri).as_deref() == Some(SCHEMA_VERSION) {
        out.already_migrated = true;
        return Ok(out);
    }

    let plan = plan_backfill(&store, ns)?;

    // C1: snapshot first, sharing the `BACKUP_KEEP = 10` pool with compact.
    // Only once there is a write to protect — a pure re-stamp of an already-clean
    // store should not evict a compact backup.
    out.backup = Some(store::snapshot(path, "migrate")?.display().to_string());

    apply(&store, ns, graph_iri, &plan, &mut out)?;
    write_stamp(&store, ns, graph_iri)?;
    out.stamped = true;

    store::write_back(&store, path, Change::Op("migrate.domain-1"))
        .context("migration write-back failed — the store is unchanged")?;

    Ok(out)
}

/// Every tier under `cwd`, workspace then global, the `auto_compact_tiers` shape.
/// Errors are per-tier: one unmigratable tier must not stop the other.
pub fn migrate_tiers(cwd: &Path, ns: &NamespaceConfig) -> Vec<Outcome> {
    tier_roots(cwd)
        .into_iter()
        .filter_map(|root| {
            let path = root.join(".base").join("graph.nq");
            if !path.exists() {
                return None;
            }
            let iri = crate::crud::workspace_graph_iri(ns, &crate::crud::workspace_slug(&root));
            migrate_tier(&path, &iri, ns).ok()
        })
        .filter(|o| !o.is_noop() || o.skipped_unhealthy)
        .collect()
}

/// Workspace root (the dir holding `.base/`) then `~/.base-gbl`, de-duplicated.
fn tier_roots(cwd: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(ws) = crate::config::find_workspace_base(cwd)
        && let Some(root) = ws.parent()
    {
        roots.push(root.to_path_buf());
    }
    if let Some(home) = crate::home::home_root() {
        let global = home.join(".base-gbl");
        if !roots.contains(&global) {
            roots.push(global);
        }
    }
    roots
}

// ─── planning ────────────────────────────────────────────────────────────────

/// One record that needs a domain link, and the domain it will get.
#[derive(Debug, PartialEq, Eq)]
pub struct Assignment {
    pub subject: String,
    pub class: String,
    pub domain_slug: String,
}

/// What is still unmigrated, computed from the store (G0 verdict A2). Never from
/// the stamp, and never from a record count.
pub fn plan_backfill(store: &Store, ns: &NamespaceConfig) -> Result<Vec<Assignment>> {
    let known = existing_domains(store, ns)?;
    let mut plan = Vec::new();
    for cov in COVERED {
        for subject in orphans_of(store, ns, cov.class)? {
            let slug = resolve(store, ns, &subject, cov.source, &known);
            plan.push(Assignment { subject, class: cov.class.to_string(), domain_slug: slug });
        }
    }
    Ok(plan)
}

/// Subjects of `class` carrying no domain link in either era, minus session
/// traffic. The transient filter is the shared one — adding a kind to
/// `ontology::transient::TRANSIENT_KINDS` excludes it from the migration too.
fn orphans_of(store: &Store, ns: &NamespaceConfig, class: &str) -> Result<Vec<String>> {
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let no_link = link::no_domain_link(ns, "s");
    let no_transient = crate::ontology::transient::sparql_exclude(ns, "s");
    let q = format!(
        "{pfx}\nSELECT DISTINCT ?s WHERE {{ GRAPH ?g {{\n\
           ?s a {p}:{class} .\n\
           {no_link}{no_transient}\
         }} }}"
    );
    let QueryResults::Solutions(sols) = store::query(store, &q)? else { return Ok(Vec::new()) };
    Ok(sols
        .filter_map(|r| r.ok())
        .filter_map(|row| match row.get("s")? {
            Term::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect())
}

/// Every `domain/` IRI that actually exists, as slugs. A backfill links only to a
/// domain record that is already there (or to the catchall, created once) — the
/// migration never invents a domain.
fn existing_domains(store: &Store, ns: &NamespaceConfig) -> Result<std::collections::HashSet<String>> {
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let q = format!("{pfx}\nSELECT DISTINCT ?d WHERE {{ GRAPH ?g {{ ?d a {p}:Domain }} }}");
    let QueryResults::Solutions(sols) = store::query(store, &q)? else {
        return Ok(Default::default());
    };
    let prefix = link::domain_iri_prefix(ns);
    Ok(sols
        .filter_map(|r| r.ok())
        .filter_map(|row| match row.get("d")? {
            Term::NamedNode(n) => n.as_str().strip_prefix(prefix.as_str()).map(str::to_string),
            _ => None,
        })
        .collect())
}

/// The domain slug for one record. Falls back to [`CATCHALL`] whenever the source
/// yields nothing or names a domain that does not exist — never invents one.
fn resolve(
    _store: &Store,
    _ns: &NamespaceConfig,
    _subject: &str,
    source: Source,
    known: &std::collections::HashSet<String>,
) -> String {
    let candidate = match source {
        Source::Catchall => None,
        // The family resolvers land with the backfill sources (build step 4).
        // Until then every covered record resolves to the catchall, which is
        // correct-but-coarse rather than wrong: a record in `unfiled` is still
        // linked, still countable, and still moved by `base graph move`.
        Source::Fixed(_) | Source::Parent(_) | Source::Frontmatter => None,
    };
    match candidate {
        Some(slug) if known.contains(&slug) => slug,
        _ => CATCHALL.to_string(),
    }
}

// ─── applying ────────────────────────────────────────────────────────────────

/// Insert the plan's links, plus the catchall domain record if anything needs it.
/// In-memory only: nothing reaches disk until `write_back`.
fn apply(
    store: &Store,
    ns: &NamespaceConfig,
    graph_iri: &str,
    plan: &[Assignment],
    out: &mut Outcome,
) -> Result<()> {
    if plan.is_empty() {
        return Ok(());
    }
    let graph = NamedNodeRef::new(graph_iri)?;
    let has_domain = format!("{}{}", ns.uri, link::CANONICAL);
    let has_domain = NamedNodeRef::new(&has_domain)?;

    if plan.iter().any(|a| a.domain_slug == CATCHALL) {
        ensure_catchall(store, ns, graph)?;
    }

    for a in plan {
        let subject = NamedNodeRef::new(&a.subject)?;
        let domain_iri = crate::crud::build_iri(ns, "domain", &a.domain_slug);
        let object = NamedNodeRef::new(&domain_iri)?;
        store.insert(QuadRef::new(subject, has_domain, object, GraphNameRef::NamedNode(graph)))?;
        *out.linked.entry(a.class.clone()).or_default() += 1;
        if a.domain_slug == CATCHALL {
            out.catchall += 1;
        }
    }
    Ok(())
}

/// Create the catchall domain once, with the same shape `domain sync` writes.
fn ensure_catchall(store: &Store, ns: &NamespaceConfig, graph: NamedNodeRef<'_>) -> Result<()> {
    let iri = crate::crud::build_iri(ns, "domain", CATCHALL);
    let subject = NamedNodeRef::new(&iri)?;
    let rdf_type = NamedNodeRef::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type")?;
    let class = format!("{}Domain", ns.uri);
    let class = NamedNodeRef::new(&class)?;
    let name_pred = format!("{}name", ns.uri);
    let name_pred = NamedNodeRef::new(&name_pred)?;
    let g = GraphNameRef::NamedNode(graph);
    store.insert(QuadRef::new(subject, rdf_type, class, g))?;
    store.insert(QuadRef::new(subject, name_pred, LiteralRef::new_simple_literal(CATCHALL), g))?;
    Ok(())
}

/// The stamp, into the same in-memory store as the data — so `write_back`'s
/// temp-plus-rename lands both or neither.
fn write_stamp(store: &Store, ns: &NamespaceConfig, graph_iri: &str) -> Result<()> {
    let graph = NamedNodeRef::new(graph_iri)?;
    let pred = format!("{}schemaVersion", ns.uri);
    let pred = NamedNodeRef::new(&pred)?;
    // Replace any older stamp rather than accumulate one per version.
    let existing: Vec<_> = store
        .quads_for_pattern(Some(graph.into()), Some(pred), None, Some(GraphNameRef::NamedNode(graph)))
        .filter_map(Result::ok)
        .collect();
    for q in existing {
        store.remove(&q)?;
    }
    store.insert(QuadRef::new(
        graph,
        pred,
        LiteralRef::new_simple_literal(SCHEMA_VERSION),
        GraphNameRef::NamedNode(graph),
    ))?;
    Ok(())
}

// ─── reporting ───────────────────────────────────────────────────────────────

/// One line per tier, for the session-start notice. Empty when nothing happened.
pub fn format_outcomes(outcomes: &[Outcome]) -> String {
    let mut s = String::new();
    for o in outcomes {
        if o.skipped_unhealthy {
            s.push_str(&format!(
                "base: domain migration skipped — {} is not healthy; run `base doctor --repair`\n",
                o.path
            ));
            continue;
        }
        // A pass that only stamped says nothing. The operator did not ask for a
        // migration and none of their data changed; a "0 records linked" line at
        // every fresh install would be noise on the one hook that must stay quiet.
        if o.total_linked() == 0 {
            continue;
        }
        let by_kind: Vec<String> =
            o.linked.iter().map(|(k, n)| format!("{k} {n}")).collect();
        s.push_str(&format!(
            "base: domain migration — {} record(s) linked ({}), {} to `{CATCHALL}`\n",
            o.total_linked(),
            if by_kind.is_empty() { "none".to_string() } else { by_kind.join(", ") },
            o.catchall,
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_covered_class_is_named_once() {
        let mut seen = std::collections::HashSet::new();
        for c in COVERED {
            assert!(seen.insert(c.class), "{} is listed twice", c.class);
        }
    }

    #[test]
    fn the_transient_kinds_are_never_covered() {
        for t in crate::ontology::transient::TRANSIENT_KINDS.iter() {
            assert!(
                !COVERED.iter().any(|c| c.class == t.class),
                "{} is session traffic — it must not be given a domain",
                t.class
            );
        }
    }

    #[test]
    fn domain_itself_is_never_covered() {
        assert!(
            !COVERED.iter().any(|c| c.class == "Domain"),
            "a domain does not belong to a domain"
        );
    }

    #[test]
    fn the_catchall_set_is_named_not_leftover() {
        let catchall: Vec<&str> = COVERED
            .iter()
            .filter(|c| c.source == Source::Catchall)
            .map(|c| c.class)
            .collect();
        assert_eq!(
            catchall,
            vec!["Project", "Goal", "Reminder"],
            "the acceptance line requires this set to be explicit; change it here, deliberately"
        );
    }
}
