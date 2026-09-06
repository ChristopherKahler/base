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
//!
//! ## Where a domain comes from, and why every arm is counted
//!
//! [`Source`] is the whole answer, per record class, and [`Arm`] is which hop
//! actually fired. Both are reported, because "the catchall set is NAMED
//! explicitly rather than being whatever was left over" is an acceptance line: an
//! operator has to be able to see that a record is in `unfiled` because no arm
//! matched it, not because the migration quietly gave up.

use std::collections::{BTreeMap, HashMap, HashSet};
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

/// The catchall domain, for records no arm could file. Chris's ruling 1, 2026-09-06.
pub const CATCHALL: &str = "unfiled";

/// Which hop actually produced a record's domain. Counted separately in the
/// migration log so the `unfiled` population is explicit rather than a remainder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Arm {
    /// The document's own frontmatter `domain:`.
    Frontmatter,
    /// A `paths` trigger in domains.toml — the same table the pre-tool-use hook
    /// uses to map a file to a domain.
    PathTrigger,
    /// A registered project whose path is a prefix of the record's.
    ProjectPath,
    /// A fixed domain for the whole family (ruling 3).
    Fixed,
    /// A parent record's domain, one link away.
    Parent,
    /// Nothing matched. [`CATCHALL`].
    Catchall,
}

impl Arm {
    pub fn label(self) -> &'static str {
        match self {
            Arm::Frontmatter => "frontmatter",
            Arm::PathTrigger => "path-trigger",
            Arm::ProjectPath => "project-path",
            Arm::Fixed => "fixed",
            Arm::Parent => "parent",
            Arm::Catchall => CATCHALL,
        }
    }
}

/// Where a record class's domain comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Ruling 2, with the arm vole added on the measurement below: the document's
    /// frontmatter `domain:`, else a domains.toml path trigger, else a registered
    /// project whose path is a prefix, else the catchall.
    ///
    /// Measured on Chris's live workspace tier, 1,372 Document + PaulPlan +
    /// PaulSummary records: frontmatter names an existing domain for **8**; a
    /// path trigger fires for **510**; a project path prefix for **92**. The
    /// ruling's own middle hop, "the registered workspace's project domain", has
    /// no data behind it at all — `~/.base-gbl/base.toml` registers zero
    /// workspaces and the graph holds zero `ops:Workspace` records.
    Document,
    /// A fixed domain for the whole family, created if absent (G0 verdict A3).
    Fixed(&'static str),
    /// Reverse walk: `?parent ops:<pred> <record>`, then the parent's domain.
    /// Reverse because that is the direction base actually writes these —
    /// `<plan> ops:hasAC <ac>`, `<summary> ops:hasFileChange <fc>`.
    ParentOf(&'static str),
    /// A literal naming a project slug, e.g. a Handoff's `ops:project "vintrix"`.
    ProjectLiteral(&'static str),
    /// The record's own `ops:name` names a project, e.g. a CodeMap of an app.
    NameProject,
    /// A domain is not a meaningful attribute of this record.
    Catchall,
}

/// One covered record class, where its domain comes from, and when it is resolved.
///
/// `stage` exists because four families take their domain from a parent that this
/// same pass may be filing: an AcceptanceCriteria hangs off a PaulPlan, and the
/// plan is a document. Stage 1 resolves from the store alone; stage 2 may also read
/// stage 1's assignments.
pub struct Covered {
    /// RDF class local name, e.g. `Document` in `ops:Document`.
    pub class: &'static str,
    pub source: Source,
    pub stage: u8,
}

/// The families, in the order the migration reports them.
///
/// Everything the migration touches is here, and nothing else is touched. A class
/// absent from this table keeps whatever it has: `Domain` has no domain of its own,
/// `Ping` is session traffic excluded from every read surface
/// (`ontology::transient`), `Note` and `Rule` are already 100% linked, and
/// `Milestone` reaches one through its project.
///
/// The counts are hawk's measurement of Chris's live workspace tier (2026-09-05)
/// and the predicates are from a read-only probe of that same store (lark,
/// 2026-09-06) — every parent link below was read off the data, not inferred from
/// the ontology, which does not declare half these classes.
pub const COVERED: &[Covered] = &[
    // 999 workspace + 141 global orphans. Carries `ops:path`; the Paul pair share
    // the `document/` IRI space and carry it too.
    Covered { class: "Document", source: Source::Document, stage: 1 },
    Covered { class: "PaulPlan", source: Source::Document, stage: 1 },
    Covered { class: "PaulSummary", source: Source::Document, stage: 1 },
    // 1,434 orphans across five classes, all Skyrim (ruling 3). These carry no IRI
    // edges at all — every property is a literal — so only a fixed assignment can
    // reach them.
    Covered { class: "LoreKnowledge", source: Source::Fixed("skyrim-companion"), stage: 1 },
    Covered { class: "LoreFact", source: Source::Fixed("skyrim-companion"), stage: 1 },
    Covered { class: "LoreRelationship", source: Source::Fixed("skyrim-companion"), stage: 1 },
    Covered { class: "LoreItem", source: Source::Fixed("skyrim-companion"), stage: 1 },
    Covered { class: "LoreBelief", source: Source::Fixed("skyrim-companion"), stage: 1 },
    // 209 + 36 orphans. `ops:project` is a LITERAL slug, not an IRI.
    Covered { class: "Handoff", source: Source::ProjectLiteral("project"), stage: 1 },
    // 14 orphans; `ops:name` is the app name, which is also its project slug.
    Covered { class: "CodeMap", source: Source::NameProject, stage: 1 },
    // A project's parent is a workspace, and a workspace has no domain. This IS
    // the named catchall set.
    Covered { class: "Project", source: Source::Catchall, stage: 1 },
    Covered { class: "Goal", source: Source::Catchall, stage: 1 },
    Covered { class: "Reminder", source: Source::Catchall, stage: 1 },
    // 1,241 orphans, every one reached by a reverse edge from a plan or summary
    // that stage 1 has just filed.
    Covered { class: "AcceptanceCriteria", source: Source::ParentOf("hasAC"), stage: 2 },
    Covered { class: "AcceptanceCriteriaResult", source: Source::ParentOf("hasACResult"), stage: 2 },
    Covered { class: "FileChange", source: Source::ParentOf("hasFileChange"), stage: 2 },
    // 142 workspace orphans. The other 263 are already linked the other way round
    // (`<domain> ops:hasDecision <decision>`) and `link::domain_index` sees them.
    Covered { class: "Decision", source: Source::ParentOf("hasDecision"), stage: 2 },
    Covered { class: "Task", source: Source::ParentOf("hasTask"), stage: 2 },
];

/// What one tier's migration did.
#[derive(Debug, Default, Serialize)]
pub struct Outcome {
    pub path: String,
    /// Already carried the stamp — nothing read, nothing written.
    pub already_migrated: bool,
    /// The graph was not healthy; nothing was written and nothing was stamped.
    pub skipped_unhealthy: bool,
    pub backup: Option<String>,
    /// Records linked, by RDF class.
    pub linked: BTreeMap<String, usize>,
    /// Records linked, by which arm filed them. `unfiled` is one entry among the
    /// others here, which is what makes it a decision rather than a remainder.
    pub by_arm: BTreeMap<String, usize>,
    /// How many landed in [`CATCHALL`]. Same number as `by_arm["unfiled"]`.
    pub catchall: usize,
    /// True when the store now carries the stamp because THIS pass wrote it.
    pub stamped: bool,
}

impl Outcome {
    pub fn total_linked(&self) -> usize {
        self.linked.values().sum()
    }
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

/// Covered records that still carry no domain link, per class, highest first.
///
/// `base doctor` reports this so a migration that skipped, or a store that has
/// drifted since one ran, is never invisible (C4). Drift is expected and is not a
/// fault: `base sync` writes a Document with no domain, so the count climbs again
/// after a migration until every write path carries the link.
pub fn orphan_counts(store: &Store, ns: &NamespaceConfig) -> Vec<(String, usize)> {
    let index = link::domain_index(store, ns);
    let by_class = covered_subjects(store, ns);
    let mut counts: Vec<(String, usize)> = COVERED
        .iter()
        .map(|c| {
            let n = by_class
                .get(c.class)
                .map(|subs| subs.iter().filter(|s| !index.contains_key(*s)).count())
                .unwrap_or(0);
            (c.class.to_string(), n)
        })
        .filter(|(_, n)| *n > 0)
        .collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts
}

/// The schema version a tier claims, without the caller having to know its graph
/// IRI. The stamp's subject IS its graph, so the store describes itself.
pub fn stamp_of_tier(store: &Store, ns: &NamespaceConfig) -> Option<String> {
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let q = format!("{pfx}\nSELECT ?v WHERE {{ GRAPH ?g {{ ?g {p}:schemaVersion ?v }} }}");
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
/// every backfilled triple is written into. The tier root (`<root>/.base/graph.nq`)
/// is what a document's relative `ops:path` resolves against.
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

    let root = path.parent().and_then(Path::parent).unwrap_or(path).to_path_buf();
    let facts = Facts::gather(&store, ns, root);
    let plan = plan_backfill(&facts, ns);

    // C1: snapshot first, sharing the `BACKUP_KEEP = 10` pool with compact. Only
    // once there is a write to protect — a pure re-stamp of an already-clean store
    // must not evict a compact backup.
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
        .filter(|o| o.total_linked() > 0 || o.skipped_unhealthy)
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

// ─── the facts a plan is built from ─────────────────────────────────────────

/// Everything the resolvers need, read from the store in a fixed number of
/// queries. The alternative — one query per record — is 4,000 SPARQL round trips
/// against a 13 MB store inside a session-start hook, which is not a budget.
struct Facts {
    root: PathBuf,
    /// Record IRI → the domain it already has, in either direction.
    index: HashMap<String, String>,
    /// Domain slugs that exist as `ops:Domain` records. A backfill links only to
    /// one of these (or to the catchall) — it never invents a domain.
    known: HashSet<String>,
    /// Subjects of each covered class, minus session traffic.
    by_class: HashMap<&'static str, Vec<String>>,
    path_of: HashMap<String, String>,
    name_of: HashMap<String, String>,
    project_lit: HashMap<String, String>,
    /// Child IRI → (predicate local name, parent IRI).
    parents: HashMap<String, Vec<(String, String)>>,
    /// (domain slug, root-relative trigger path), longest first, ties broken by
    /// declaration order in `domain::load_domains`.
    triggers: Vec<(String, String)>,
    /// (project IRI, root-relative project path), longest first.
    project_paths: Vec<(String, String)>,
}

impl Facts {
    fn gather(store: &Store, ns: &NamespaceConfig, root: PathBuf) -> Self {
        let index = link::domain_index(store, ns);
        let known = existing_domains(store, ns);

        let mut by_class = covered_subjects(store, ns);
        for subs in by_class.values_mut() {
            subs.retain(|s| !index.contains_key(s));
        }

        let path_of = literal_map(store, ns, "path");
        let name_of = literal_map(store, ns, "name");
        let project_lit = literal_map(store, ns, "project");

        let mut parents: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for pred in COVERED.iter().filter_map(|c| match c.source {
            Source::ParentOf(p) => Some(p),
            _ => None,
        }) {
            for (parent, child) in iri_pairs(store, ns, pred) {
                parents.entry(child).or_default().push((pred.to_string(), parent));
            }
        }

        // Longest trigger wins; a tie keeps `load_domains` order, which is global
        // then workspace-overlaid then extensions. Deterministic for a given
        // config, which is the property that matters — Chris's workspace declares
        // `tools` twice (skyrim-companion and asset-inventory) and something has
        // to decide it the same way on every run.
        let mut triggers: Vec<(String, String)> = Vec::new();
        for def in crate::domain::load_domains(&root) {
            let slug = crate::crud::slugify(&def.name);
            for p in &def.paths {
                if let Some(rel) = rel_to_root(&root, p) {
                    triggers.push((slug.clone(), rel));
                }
            }
        }
        triggers.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        let mut project_paths: Vec<(String, String)> = Vec::new();
        for iri in class_subjects(store, ns, "Project") {
            let Some(raw) = path_of.get(&iri) else { continue };
            let Some(rel) = rel_to_root(&root, raw) else { continue };
            // A project whose path is a single top-level folder claims everything
            // under it. Chris has two — `vintrix` = "Documents" and
            // `asset-inventory` = "tools" — and honouring them would file 377
            // unrelated documents under those domains. That is mis-filing, not
            // coverage, so a bare top-level path is not a project prefix.
            if rel.contains('/') {
                project_paths.push((iri, rel));
            }
        }
        project_paths.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        Facts { root, index, known, by_class, path_of, name_of, project_lit, parents, triggers, project_paths }
    }
}

/// `C:/Users/Chris/tools` under root `C:/Users/Chris` → `tools`. `None` for a path
/// outside the tier, which cannot be compared with a record's relative path.
fn rel_to_root(root: &Path, p: &str) -> Option<String> {
    let norm = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_string();
    let p = norm(&p.replace("\\\\", "\\"));
    let r = norm(&root.to_string_lossy());
    if !r.is_empty() && p.len() > r.len() && p[..r.len()].eq_ignore_ascii_case(&r) && p.as_bytes()[r.len()] == b'/' {
        return Some(p[r.len() + 1..].to_string());
    }
    // Already relative: no drive letter, no leading slash, no UNC.
    if !p.starts_with('/') && !p.contains(':') && !p.is_empty() {
        return Some(p);
    }
    None
}

// ─── planning ────────────────────────────────────────────────────────────────

/// One record that needs a domain link, the domain it will get, and the arm that
/// decided it.
#[derive(Debug, PartialEq, Eq)]
pub struct Assignment {
    pub subject: String,
    pub class: String,
    pub domain_slug: String,
    pub arm: Arm,
}

/// What is still unmigrated, computed from the store (G0 verdict A2). Never from
/// the stamp, and never from a record count.
fn plan_backfill(facts: &Facts, ns: &NamespaceConfig) -> Vec<Assignment> {
    let mut plan: Vec<Assignment> = Vec::new();
    // Stage 1's assignments are visible to stage 2: an AcceptanceCriteria takes
    // its plan's domain, and the plan may have been filed moments ago in this same
    // pass.
    let mut planned: HashMap<String, String> = HashMap::new();

    for stage in [1u8, 2u8] {
        for cov in COVERED.iter().filter(|c| c.stage == stage) {
            let Some(subjects) = facts.by_class.get(cov.class) else { continue };
            for subject in subjects {
                // One assignment per record, from the first covered class that
                // claims it. Chris's store has no subject carrying two rdf:types
                // (measured, 2026-09-06), but multi-typing is legal RDF and a
                // record filed twice would insert the same quad twice — harmless
                // in a set store — and count itself twice in the log, which is
                // not harmless: the log is the only view of what the migration
                // did.
                if planned.contains_key(subject) {
                    continue;
                }
                let (slug, arm) = resolve(facts, ns, subject, cov.source, &planned);
                planned.insert(subject.clone(), slug.clone());
                plan.push(Assignment {
                    subject: subject.clone(),
                    class: cov.class.to_string(),
                    domain_slug: slug,
                    arm,
                });
            }
        }
    }
    plan.sort_by(|a, b| a.subject.cmp(&b.subject));
    plan
}

/// The domain slug for one record, and which arm produced it.
fn resolve(
    facts: &Facts,
    ns: &NamespaceConfig,
    subject: &str,
    source: Source,
    planned: &HashMap<String, String>,
) -> (String, Arm) {
    let catchall = || (CATCHALL.to_string(), Arm::Catchall);
    match source {
        Source::Catchall => catchall(),

        // A3: a fixed family domain is created if absent, so the ruling holds on a
        // store that never declared it. This is the one place the migration may
        // add a domain record besides the catchall, and it is one per family, not
        // one per record.
        Source::Fixed(slug) => (slug.to_string(), Arm::Fixed),

        Source::Document => {
            let Some(rel) = facts.path_of.get(subject) else { return catchall() };
            let rel = rel.replace('\\', "/");
            if let Some(slug) = frontmatter_domain(&facts.root, &rel)
                && facts.known.contains(&slug)
            {
                return (slug, Arm::Frontmatter);
            }
            if let Some((slug, _)) = facts.triggers.iter().find(|(_, t)| under(&rel, t))
                && facts.known.contains(slug)
            {
                return (slug.clone(), Arm::PathTrigger);
            }
            if let Some((proj, _)) = facts.project_paths.iter().find(|(_, t)| under(&rel, t))
                && let Some(slug) = facts.index.get(proj)
            {
                return (slug.clone(), Arm::ProjectPath);
            }
            catchall()
        }

        Source::ParentOf(pred) => facts
            .parents
            .get(subject)
            .into_iter()
            .flatten()
            .filter(|(p, _)| p == pred)
            .find_map(|(_, parent)| facts.index.get(parent).or_else(|| planned.get(parent)))
            .filter(|slug| slug.as_str() != CATCHALL)
            .map(|slug| (slug.clone(), Arm::Parent))
            .unwrap_or_else(catchall),

        Source::ProjectLiteral(pred) => facts
            .project_lit
            .get(subject)
            .filter(|_| pred == "project")
            .and_then(|lit| project_domain(facts, ns, lit))
            .map(|slug| (slug, Arm::Parent))
            .unwrap_or_else(catchall),

        Source::NameProject => facts
            .name_of
            .get(subject)
            .and_then(|lit| project_domain(facts, ns, lit))
            .map(|slug| (slug, Arm::Parent))
            .unwrap_or_else(catchall),
    }
}

/// `"vintrix"` → the domain of `project/vintrix`, if that project has one.
fn project_domain(facts: &Facts, ns: &NamespaceConfig, literal: &str) -> Option<String> {
    let iri = crate::crud::build_iri(ns, "project", &crate::crud::slugify(literal));
    facts.index.get(&iri).cloned()
}

/// Is `rel` inside `dir`? Case-insensitive, because a Windows store holds
/// `Tools/stt` and a trigger may say `tools/stt`.
fn under(rel: &str, dir: &str) -> bool {
    !dir.is_empty()
        && rel.len() > dir.len()
        && rel[..dir.len()].eq_ignore_ascii_case(dir)
        && rel.as_bytes()[dir.len()] == b'/'
}

/// The `domain:` key of a markdown file's frontmatter, slugified. Reads at most
/// the head of the file — the frontmatter parser only ever needs that, and the
/// migration opens up to a few thousand of these once.
fn frontmatter_domain(root: &Path, rel: &str) -> Option<String> {
    let full = root.join(rel);
    let meta = std::fs::metadata(&full).ok()?;
    if !meta.is_file() || meta.len() > 1_000_000 {
        return None;
    }
    let content = std::fs::read_to_string(&full).ok()?;
    let fm = crate::extract::frontmatter::parse_frontmatter(&content)?;
    fm.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("domain"))
        .map(|(_, v)| crate::crud::slugify(v.trim().trim_matches('"')))
        .filter(|s| !s.is_empty())
}

// ─── store reads ─────────────────────────────────────────────────────────────

/// Subjects of every covered class, bucketed, minus session traffic.
///
/// ONE scan of the typed subjects rather than a query per class. Eighteen separate
/// `?s a ops:<Class>` queries against a 13 MB store is the shape that put 11
/// seconds into a `base doctor` run on a temp fixture; the migration and doctor
/// both ask this question, and both ask it on the session-start path.
///
/// The transient filter is the shared one — adding a kind to
/// `ontology::transient::TRANSIENT_KINDS` excludes it from the migration and from
/// doctor's orphan count at the same time.
fn covered_subjects(store: &Store, ns: &NamespaceConfig) -> HashMap<&'static str, Vec<String>> {
    let pfx = crate::crud::prefixes(ns);
    let no_transient = crate::ontology::transient::sparql_exclude(ns, "s");
    let q = format!(
        "{pfx}\nSELECT DISTINCT ?s ?t WHERE {{ GRAPH ?g {{ ?s a ?t .\n{no_transient}}} }}"
    );
    let mut out: HashMap<&'static str, Vec<String>> = HashMap::new();
    let Ok(QueryResults::Solutions(sols)) = store::query(store, &q) else { return out };
    let want: HashMap<String, &'static str> =
        COVERED.iter().map(|c| (format!("{}{}", ns.uri, c.class), c.class)).collect();
    for row in sols.filter_map(|r| r.ok()) {
        let (Some(Term::NamedNode(s)), Some(Term::NamedNode(t))) = (row.get("s"), row.get("t"))
        else {
            continue;
        };
        let Some(class) = want.get(t.as_str()) else { continue };
        out.entry(class).or_default().push(s.as_str().to_string());
    }
    for subs in out.values_mut() {
        subs.sort();
        subs.dedup();
    }
    out
}

/// Subjects of one class, for the callers that want exactly one — `Project` paths
/// and the domain inventory.
fn class_subjects(store: &Store, ns: &NamespaceConfig, class: &str) -> Vec<String> {
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let q = format!("{pfx}\nSELECT DISTINCT ?s WHERE {{ GRAPH ?g {{ ?s a {p}:{class} }} }}");
    let Ok(QueryResults::Solutions(sols)) = store::query(store, &q) else { return Vec::new() };
    let mut out: Vec<String> = sols
        .filter_map(|r| r.ok())
        .filter_map(|row| match row.get("s")? {
            Term::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

/// `?s ops:<pred> ?o` where the object is a literal.
fn literal_map(store: &Store, ns: &NamespaceConfig, pred: &str) -> HashMap<String, String> {
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let q = format!(
        "{pfx}\nSELECT ?s ?o WHERE {{ GRAPH ?g {{ ?s {p}:{pred} ?o . FILTER(isLiteral(?o)) }} }}"
    );
    let mut out = HashMap::new();
    let Ok(QueryResults::Solutions(sols)) = store::query(store, &q) else { return out };
    for row in sols.filter_map(|r| r.ok()) {
        let (Some(Term::NamedNode(s)), Some(Term::Literal(o))) = (row.get("s"), row.get("o")) else {
            continue;
        };
        out.entry(s.as_str().to_string()).or_insert_with(|| o.value().to_string());
    }
    out
}

/// `?a ops:<pred> ?b` where both are IRIs, as `(subject, object)`.
fn iri_pairs(store: &Store, ns: &NamespaceConfig, pred: &str) -> Vec<(String, String)> {
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let q = format!(
        "{pfx}\nSELECT ?a ?b WHERE {{ GRAPH ?g {{ ?a {p}:{pred} ?b . FILTER(isIRI(?b)) }} }}"
    );
    let Ok(QueryResults::Solutions(sols)) = store::query(store, &q) else { return Vec::new() };
    sols.filter_map(|r| r.ok())
        .filter_map(|row| match (row.get("a")?, row.get("b")?) {
            (Term::NamedNode(a), Term::NamedNode(b)) => {
                Some((a.as_str().to_string(), b.as_str().to_string()))
            }
            _ => None,
        })
        .collect()
}

/// Every `domain/` IRI that actually exists, as slugs.
fn existing_domains(store: &Store, ns: &NamespaceConfig) -> HashSet<String> {
    let prefix = link::domain_iri_prefix(ns);
    class_subjects(store, ns, "Domain")
        .into_iter()
        .filter_map(|iri| iri.strip_prefix(prefix.as_str()).map(str::to_string))
        .collect()
}

// ─── applying ────────────────────────────────────────────────────────────────

/// Insert the plan's links, plus any domain record the plan needs that does not
/// exist yet. In-memory only: nothing reaches disk until `write_back`.
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
    let has_domain_iri = format!("{}{}", ns.uri, link::CANONICAL);
    let has_domain = NamedNodeRef::new(&has_domain_iri)?;

    let mut created: HashSet<&str> = HashSet::new();
    for a in plan {
        if created.insert(a.domain_slug.as_str()) {
            ensure_domain(store, ns, graph, &a.domain_slug)?;
        }
        let subject = NamedNodeRef::new(&a.subject)?;
        let domain_iri = crate::crud::build_iri(ns, "domain", &a.domain_slug);
        let object = NamedNodeRef::new(&domain_iri)?;
        store.insert(QuadRef::new(subject, has_domain, object, GraphNameRef::NamedNode(graph)))?;
        *out.linked.entry(a.class.clone()).or_default() += 1;
        *out.by_arm.entry(a.arm.label().to_string()).or_default() += 1;
        if a.arm == Arm::Catchall {
            out.catchall += 1;
        }
    }
    Ok(())
}

/// Create a domain record if it is not already there, with the same shape
/// `domain sync` writes. Idempotent by construction: the store is a set, so a
/// re-insert of an existing quad changes nothing.
fn ensure_domain(store: &Store, ns: &NamespaceConfig, graph: NamedNodeRef<'_>, slug: &str) -> Result<()> {
    let iri = crate::crud::build_iri(ns, "domain", slug);
    let subject = NamedNodeRef::new(&iri)?;
    let rdf_type = NamedNodeRef::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type")?;
    let class = format!("{}Domain", ns.uri);
    let class = NamedNodeRef::new(&class)?;
    let name_pred = format!("{}name", ns.uri);
    let name_pred = NamedNodeRef::new(&name_pred)?;
    let g = GraphNameRef::NamedNode(graph);
    store.insert(QuadRef::new(subject, rdf_type, class, g))?;
    store.insert(QuadRef::new(subject, name_pred, LiteralRef::new_simple_literal(slug), g))?;
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

/// One line per tier for the session-start notice, naming what each arm filed.
///
/// The per-arm breakdown is the point, not decoration: `unfiled` appears as one
/// arm among the others, so an operator can see it was a decision no arm could
/// beat rather than a silent remainder.
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
        let arms: Vec<String> = o.by_arm.iter().map(|(k, n)| format!("{k} {n}")).collect();
        let kinds: Vec<String> = o.linked.iter().map(|(k, n)| format!("{k} {n}")).collect();
        s.push_str(&format!(
            "base: domain migration — {} record(s) now carry a domain.\n  by source: {}\n  by kind:   {}\n",
            o.total_linked(),
            arms.join(", "),
            kinds.join(", "),
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_covered_class_is_named_once() {
        let mut seen = HashSet::new();
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

    #[test]
    fn a_parent_walking_class_is_resolved_after_its_parents() {
        for c in COVERED {
            if matches!(c.source, Source::ParentOf(_)) {
                assert_eq!(c.stage, 2, "{} walks to a parent, so it resolves in stage 2", c.class);
            }
        }
    }

    #[test]
    fn rel_to_root_strips_the_tier_root_in_either_slash_style() {
        let root = Path::new("C:/Users/Chris");
        assert_eq!(rel_to_root(root, "C:/Users/Chris/tools").as_deref(), Some("tools"));
        assert_eq!(rel_to_root(root, r"C:\Users\Chris\ClaudePokes").as_deref(), Some("ClaudePokes"));
        assert_eq!(rel_to_root(root, r"C:\\Users\\Chris\\Tools\\stt").as_deref(), Some("Tools/stt"));
        assert_eq!(rel_to_root(root, "Documents/video-gen").as_deref(), Some("Documents/video-gen"));
        // Outside the tier: not comparable with a record's relative path.
        assert_eq!(rel_to_root(root, "/home/chriskahler/basemode"), None);
        assert_eq!(rel_to_root(root, "D:/elsewhere"), None);
    }

    #[test]
    fn under_is_a_path_test_not_a_string_prefix() {
        assert!(under("Documents/video-gen/a.md", "Documents/video-gen"));
        assert!(under("Tools/stt/x.md", "tools/stt"), "Windows paths are case-insensitive");
        assert!(!under("Documents/video-generator/a.md", "Documents/video-gen"));
        assert!(!under("Documents/video-gen", "Documents/video-gen"), "the dir is not under itself");
        assert!(!under("anything", ""));
    }
}
