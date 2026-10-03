use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::NamespaceConfig;
use crate::crud;
use crate::crud::{all_tier_files, tier_label_of_file};

/// Resolve the target graph file + graph IRI for a write.
///
/// Workspace tier, always. The old global catchall (issue #8) meant a handoff
/// created outside a workspace landed in `~/.base-gbl` and then resurfaced at
/// the start of every unrelated project, forever. Global is now something you
/// opt into with `-g` — which routes cwd to `~/.base-gbl`, so this resolves it
/// as that tier's workspace rather than as a silent fallback.
fn write_tier(cwd: &Path, ns: &NamespaceConfig) -> Result<(PathBuf, String)> {
    let base = crate::config::find_workspace_base(cwd).context(
        "no .base/ directory found — refusing to write outside a workspace. \
         Use --global (-g) to file this against the global tier deliberately, \
         or run `base scaffold` here first.",
    )?;
    let ws_slug = crud::workspace_slug(cwd);
    Ok((base.join("graph.nq"), crud::workspace_graph_iri(ns, &ws_slug)))
}

/// Does `file` hold this handoff/fork at all?
///
/// Asked BEFORE mutating, because a SPARQL UPDATE whose WHERE binds nothing is a
/// silent no-op: 0.14.1 printed `Fork '<slug>' archived` with rc 0 for a slug
/// that existed in neither tier (#72). Runs on the store the caller already
/// loaded, so it costs no extra parse of a 16 MB graph.
fn store_holds(store: &Store, ns: &NamespaceConfig, slug: &str) -> bool {
    let iri = crud::build_iri(ns, "handoff", slug);
    let p = &ns.prefix;
    let ask = format!(
        "{}\nASK {{ GRAPH ?g {{ <{iri}> a {p}:Handoff }} }}",
        crud::prefixes(ns)
    );
    matches!(
        store.query(&ask),
        Ok(QueryResults::Boolean(true))
    )
}

/// Mutate `path` only if it holds `slug`. Returns whether it did.
///
/// Load, ask and write all happen inside one graph lock: asking outside it would
/// answer about a graph that another writer may have replaced before the write.
fn mutate_file_if_holds(
    path: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    sparql: &str,
) -> Result<bool> {
    crate::store::with_graph_lock(path, || {
        let store = crate::store::load_or_empty(path)?;
        if !store_holds(&store, ns, slug) {
            return Ok(false);
        }
        let full = format!("{}\n{}", crud::prefixes(ns), sparql);
        crate::store::update_and_write(
            &store,
            path,
            &full,
            crate::store::Scope::Target,
            crate::store::Intent::Knowledge,
        )
        .with_context(|| format!("handoff update failed: {full}"))?;
        Ok(true)
    })
}

/// Load one graph file, run a SPARQL UPDATE, write back atomically.
fn mutate_file(path: &Path, ns: &NamespaceConfig, sparql: &str) -> Result<()> {
    let full = format!("{}\n{}", crud::prefixes(ns), sparql);
    // Locked, load inside: four builders registering inside twelve seconds is
    // how #71-#74 were filed, and every one of those writes reported success.
    crate::store::locked_update(
        path,
        &full,
        crate::store::Scope::Target,
        crate::store::Intent::Knowledge,
    )
    .with_context(|| format!("handoff update failed: {full}"))
}

/// Derive a flow-doc slug from its doc path basename (no extension).
/// `/abs/path/FORK-COMMAND-SPEC.md` → `FORK-COMMAND-SPEC`. Used VERBATIM (no
/// slugify/lowercase) so the doc filename and the graph slug are the SAME string
/// — that single name is the title a handoff/fork is summoned by (doc==slug protocol).
fn doc_basename_slug(doc_path: &str) -> Result<String> {
    Path::new(doc_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("could not derive a slug from doc path '{doc_path}'"))
}

/// Resolve the slug for a flow-doc: an explicit `--slug` override (verbatim) when
/// given and non-blank, else the doc basename. THE STANDARD for both handoff and
/// fork: doc filename and graph slug always align, so the operator summons the
/// next session by one consistent name.
fn resolve_doc_slug(slug: Option<&str>, doc_path: &str) -> Result<String> {
    match slug {
        Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        _ => doc_basename_slug(doc_path),
    }
}

/// What a `handoff create` actually did, so the CLI can say it.
///
/// 0.14.1 archived the prior handoff silently and only in the tier it wrote to,
/// while the help promised a project-wide archive. Four builders registering
/// inside twelve seconds each archived the one before it with no output, and an
/// open handoff in the other tier sat there untouched and unmentioned (#71).
/// 0.15.2 named an open handoff in the other tier and left it open, and under
/// `-g` could not see the workspace at all (lane 2, rank 08).
#[derive(Debug)]
pub struct CreateOutcome {
    pub slug: String,
    /// The tier this create wrote to: "workspace tier" or "global tier".
    pub tier: String,
    /// The lane the new handoff was filed in, and where that name came from.
    /// `None` when nothing named one: then it shares the lane of every other
    /// handoff on the project that has none (BO-11, F18a).
    pub lane: Option<Lane>,
    /// Every prior handoff this create archived, as (slug, tier), across every
    /// tier (`auk`'s Q2 ruling). Empty when the project had no prior open or
    /// deferred continuity handoff in this lane anywhere.
    pub archived: Vec<(String, String)>,
    /// The project's open or deferred handoffs in other lanes, which this create
    /// left alone, as (slug, lane). Before BO-11 the first of these to be found
    /// was archived, in every tier, and nobody was told.
    pub left_open: Vec<(String, Option<String>)>,
}

/// A handoff's lane: the line of work it belongs to. A new handoff archives only
/// the earlier open handoff on its project in the same lane (BO-11, F18a). Sessions
/// `auk`, `plover`, `grebe` and `finch` all wrote handoffs for `base-0160`, and each
/// new one archived another session's, in every tier, with no undo; so they stopped
/// registering them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane {
    pub name: String,
    pub from: LaneFrom,
}

/// Where a lane's name came from, in the order `create` looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneFrom {
    /// `--lane <name>`: a lane several sessions hand back and forth.
    Flag,
    /// The doc's front matter `by:`.
    DocBy,
    /// The codename in a `<date>-<codename>-<project>` slug, the name the
    /// `*handoff` flow gives every doc, matched against the relay's titles.
    Slug,
    /// The relay title of the session running the create.
    RelayTitle,
}

impl LaneFrom {
    pub fn describe(self) -> &'static str {
        match self {
            LaneFrom::Flag => "--lane",
            LaneFrom::DocBy => "the doc's by: field",
            LaneFrom::Slug => "the codename in the slug",
            LaneFrom::RelayTitle => "this session's relay title",
        }
    }
}

/// What `create` needs from outside the graph to name a new handoff's lane. The
/// CLI gathers it; [`create`] passes none of it, so a library caller gets the
/// lane the doc's `by:` names, or none.
#[derive(Debug, Default, Clone)]
pub struct LaneInputs {
    /// `--lane`.
    pub flag: Option<String>,
    /// The relay title of the session running the create.
    pub relay_title: Option<String>,
    /// Every relay title on this machine: what a codename in a slug is matched against.
    pub titles: Vec<String>,
}

/// The `by:` field of the doc at `doc_path`, when it has front matter that names one.
fn doc_by(doc_path: &str) -> Option<String> {
    let text = std::fs::read_to_string(doc_path).ok()?;
    crate::extract::frontmatter::parse_frontmatter(&text)?
        .into_iter()
        .find(|(key, _)| key == "by")
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// `slug` without its leading `YYYY-MM-DD-` and the `HHMM-` after it, if any. `None`
/// when the slug does not start with a date.
fn after_date(slug: &str) -> Option<&str> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let mut parts = slug.splitn(4, '-');
    let (y, m, d, rest) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if !(y.len() == 4 && m.len() == 2 && d.len() == 2 && digits(y) && digits(m) && digits(d)) {
        return None;
    }
    match rest.split_once('-') {
        Some((hhmm, after)) if hhmm.len() == 4 && digits(hhmm) => Some(after),
        _ => Some(rest),
    }
}

/// The codename a `<date>-<codename>-<project>` slug was written by: the longest
/// relay title that the part after the date is, or starts with followed by `-`.
/// Matched against real titles rather than cut at the first hyphen, because a
/// codename can hold one (`otter-bo11` is not `otter`). `None` when the slug has no
/// date or no title fits; a wrong guess here would archive another session's
/// handoff, which is the bug this exists to stop.
fn slug_codename(slug: &str, titles: &[String]) -> Option<String> {
    let rest = after_date(slug)?.to_ascii_lowercase();
    titles
        .iter()
        .map(|title| title.trim().to_ascii_lowercase())
        .filter(|title| !title.is_empty())
        .filter(|title| rest == *title || rest.starts_with(&format!("{title}-")))
        .max_by_key(|title| title.len())
}

/// The new handoff's lane: `--lane`, else the doc's `by:`, else the codename in its
/// slug, else the relay title of the session running the create.
fn lane_of_new(inputs: &LaneInputs, doc_path: &str, slug: &str) -> Option<Lane> {
    let named = |name: Option<String>, from: LaneFrom| {
        name.map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .map(|name| Lane { name, from })
    };
    named(inputs.flag.clone(), LaneFrom::Flag)
        .or_else(|| named(doc_by(doc_path), LaneFrom::DocBy))
        .or_else(|| named(slug_codename(slug, &inputs.titles), LaneFrom::Slug))
        .or_else(|| named(inputs.relay_title.clone(), LaneFrom::RelayTitle))
}

/// Two lanes are one lane when both are unnamed, or both carry the same name, case aside.
fn same_lane(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

/// A prior continuity handoff on the project, with the lane it is in.
struct Prior {
    slug: String,
    lane: Option<String>,
}

/// The prior continuity handoffs for `project` in one loaded graph: status `open`
/// or `deferred` (E4), never a fork, each with its lane.
///
/// Forks share the Handoff type and the project but are additive side-work, so
/// they are excluded here exactly as they are in `create`'s archive step. A lane
/// is the one recorded on the handoff; a handoff written before lanes existed
/// takes it from its doc's `by:`, then from the codename in its slug, and has
/// none when neither names one.
fn priors_in(store: &Store, ns: &NamespaceConfig, project: &str, titles: &[String]) -> Result<Vec<Prior>> {
    let p = &ns.prefix;
    let esc = crud::escape_sparql_literal(project);
    let q = format!(
        "{}\nSELECT ?h ?lane ?doc WHERE {{ GRAPH ?g {{ ?h a {p}:Handoff ; {p}:project \"{esc}\" ; {p}:status ?s .\n\
           FILTER(?s IN (\"open\", \"deferred\"))\n\
           OPTIONAL {{ ?h {p}:kind ?kind }}\n\
           FILTER(!BOUND(?kind) || ?kind != \"fork\")\n\
           OPTIONAL {{ ?h {p}:lane ?lane }}\n\
           OPTIONAL {{ ?h {p}:handoffDoc ?doc }} }} }}",
        crud::prefixes(ns)
    );
    let QueryResults::Solutions(solutions) = crate::store::query(store, &q)? else {
        return Ok(Vec::new());
    };
    // slug -> (recorded lane, doc), first value of each kept.
    let mut rows: std::collections::BTreeMap<String, (Option<String>, Option<String>)> = Default::default();
    for sol in solutions.filter_map(|sol| sol.ok()) {
        let Some(h) = sol.get("h").map(|term| crud::term_display(term.as_ref())) else {
            continue;
        };
        let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
        let get = |k: &str| sol.get(k).map(|t| crud::term_display(t.as_ref())).filter(|v| !v.trim().is_empty());
        let row = rows.entry(slug).or_default();
        if row.0.is_none() {
            row.0 = get("lane");
        }
        if row.1.is_none() {
            row.1 = get("doc");
        }
    }
    Ok(rows
        .into_iter()
        .map(|(slug, (recorded, doc))| {
            let lane = recorded
                .or_else(|| doc.as_deref().and_then(doc_by))
                .or_else(|| slug_codename(&slug, titles));
            Prior { slug, lane }
        })
        .collect())
}

/// The UPDATE that archives exactly `slugs`, wherever in the file they are still
/// open or deferred continuity handoffs of `project`. Empty when there is nothing
/// to archive. The project and fork checks are the ones `priors_in` chose by, so a
/// record at the same IRI in another named graph of the file (another workspace's,
/// for another project, or a fork) is never caught by the IRI alone.
fn archive_slugs_update(ns: &NamespaceConfig, slugs: &[String], project: &str) -> String {
    if slugs.is_empty() {
        return String::new();
    }
    let p = &ns.prefix;
    let esc = crud::escape_sparql_literal(project);
    let iris: Vec<String> = slugs
        .iter()
        .map(|slug| format!("<{}>", crud::build_iri(ns, "handoff", slug)))
        .collect();
    format!(
        "DELETE {{ GRAPH ?g {{ ?h {p}:status ?s }} }}\n\
         INSERT {{ GRAPH ?g {{ ?h {p}:status \"archived\" }} }}\n\
         WHERE  {{ GRAPH ?g {{ ?h a {p}:Handoff ; {p}:project \"{esc}\" ; {p}:status ?s .\n\
           FILTER(?s IN (\"open\", \"deferred\"))\n\
           OPTIONAL {{ ?h {p}:kind ?kind }}\n\
           FILTER(!BOUND(?kind) || ?kind != \"fork\")\n\
           FILTER(?h IN ({})) }} }}",
        iris.join(", ")
    )
}

/// Register a handoff pointing at a resume document, with no lane input beyond
/// the doc's own `by:` (see [`create_in_lane`]).
pub fn create(
    gbl_root: Option<&Path>,
    cwd: &Path,
    standing_cwd: &Path,
    ns: &NamespaceConfig,
    project: &str,
    doc_path: &str,
    slug: Option<&str>,
) -> Result<CreateOutcome> {
    create_in_lane(gbl_root, cwd, standing_cwd, ns, project, doc_path, slug, &LaneInputs::default())
}

/// Register a handoff pointing at a resume document. Archives the earlier open
/// or deferred continuity handoff for the same project in the same lane, in every
/// tier (one open handoff per project and lane: E4, narrowed by BO-11's F18a), then
/// inserts the new one with `resurfaceAt = now` so it surfaces next session start.
/// Other lanes' handoffs are left open and listed. Slug defaults to the doc
/// basename (doc==slug protocol); pass `slug` to override. Re-registering the same
/// slug re-points it idempotently (no duplicate triples).
///
/// `cwd` picks the tier to write. `standing_cwd` is where the operator stands,
/// and every tier is found from there: under `-g` the CLI routes `cwd` to the
/// global tier, and discovery from it could never see the workspace (rank 08, R3).
#[allow(clippy::too_many_arguments)]
pub fn create_in_lane(
    gbl_root: Option<&Path>,
    cwd: &Path,
    standing_cwd: &Path,
    ns: &NamespaceConfig,
    project: &str,
    doc_path: &str,
    slug: Option<&str>,
    inputs: &LaneInputs,
) -> Result<CreateOutcome> {
    let now = crud::now_iso();
    let slug = resolve_doc_slug(slug, doc_path)?;
    let iri = crud::build_iri(ns, "handoff", &slug);
    let (path, graph) = write_tier(cwd, ns)?;
    let p = &ns.prefix;
    let lane = lane_of_new(inputs, doc_path, &slug);
    let lane_name = lane.as_ref().map(|l| l.name.as_str());
    // The project this handoff names, as the IRI its domain hangs off (kite F7b).
    let project_iri = crud::build_iri(ns, "project", &crud::slugify(project));
    // `priors_in` escapes the name itself. Handed the escaped copy, it searched
    // for a different literal whenever the name held a quote or a backslash.
    let project_name = project;
    let project = crud::escape_sparql_literal(project);
    let doc = crud::escape_sparql_literal(doc_path);
    let lane_triple = lane_name
        .map(|name| format!("             {p}:lane \"{}\" ;\n", crud::escape_sparql_literal(name)))
        .unwrap_or_default();

    // Clean any existing node at this exact slug so re-registration re-points
    // it instead of layering duplicate status/timestamp triples.
    let clean_target = format!(
        "DELETE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }} WHERE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }}"
    );

    // The new handoff.
    let insert = format!(
        "INSERT DATA {{ GRAPH <{graph}> {{\n\
           <{iri}> rdf:type {p}:Handoff ;\n\
             {p}:name \"{project}\" ;\n\
             {p}:project \"{project}\" ;\n\
             {p}:handoffDoc \"{doc}\" ;\n\
             {p}:kind \"handoff\" ;\n\
         {lane_triple}\
             {p}:status \"open\" ;\n\
             {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:resurfaceAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
         }} }}"
    );

    // The handoff takes the domain of the project it names, in the same write.
    let inherit = crate::domain::link::inherit_update(ns, &graph, &iri, &project_iri);

    // Archive the project's earlier open or deferred *continuity* handoffs in this
    // lane (E4, F18a), in every tier that holds one. Which ones is decided from the
    // graph loaded inside the same lock as the write, so the names printed are the
    // names archived. Forks (kind = "fork") share the Handoff type and project but
    // are additive side-work: never archived, in any tier. In the tier being
    // written the new slug is left out, because the insert re-points it there; in
    // another tier it is an older copy of this very handoff and is archived
    // whatever lane it reads as (`auk`'s ruling D1).
    let mut left_open: Vec<(String, Option<String>)> = Vec::new();
    let mut split = |priors: Vec<Prior>, here: bool| -> Vec<String> {
        let mut archive = Vec::new();
        for prior in priors {
            if prior.slug == slug {
                if !here {
                    archive.push(prior.slug);
                }
            } else if same_lane(prior.lane.as_deref(), lane_name) {
                archive.push(prior.slug);
            } else if !left_open.iter().any(|(s, _)| *s == prior.slug) {
                left_open.push((prior.slug, prior.lane));
            }
        }
        archive
    };

    // The tier being written goes first, in one write with the new node, so a
    // failure in another tier leaves one extra open handoff and says so, never an
    // archived handoff with nothing registered in its place. A tier being written
    // that cannot be read is a plain error, and nothing is written.
    let tier = tier_label_of_file(&path, gbl_root);
    let archived_here = crate::store::with_graph_lock(&path, || {
        let store = crate::store::load_or_empty(&path)?;
        let archive = split(priors_in(&store, ns, project_name, &inputs.titles)?, true);
        let archive_update = archive_slugs_update(ns, &archive, project_name);
        let statements: Vec<&str> = [archive_update.as_str(), &clean_target, &insert, &inherit]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let full = format!("{}\n{}", crud::prefixes(ns), statements.join(";\n"));
        crate::store::update_and_write(
            &store,
            &path,
            &full,
            crate::store::Scope::Target,
            crate::store::Intent::Knowledge,
        )
        .with_context(|| format!("handoff update failed: {full}"))?;
        Ok(archive)
    })?;
    let mut archived: Vec<(String, String)> =
        archived_here.into_iter().map(|prior| (prior, tier.to_string())).collect();
    // What an error after the write says first: the handoff is registered, and
    // what it archived in its own tier, so no archive goes unmentioned (#71).
    let registered = if archived.is_empty() {
        format!("registered '{slug}' in the {tier}")
    } else {
        let names: Vec<String> = archived.iter().map(|(prior, t)| format!("{prior} ({t})")).collect();
        format!("registered '{slug}' in the {tier} and archived {}", names.join(", "))
    };

    // Every other tier, found from where the operator stands. Only a tier that
    // holds a prior handoff in this lane is written: the global graph is shared by
    // every live session. A tier that cannot be read must not stop the handoff
    // being registered (`auk`'s ruling D5): it is skipped, and named at the end.
    let key = |f: &Path| f.canonicalize().unwrap_or_else(|_| f.to_path_buf());
    let written = key(&path);
    let mut unreadable: Vec<String> = Vec::new();
    for file in all_tier_files(gbl_root, standing_cwd) {
        if key(&file) == written {
            continue;
        }
        let label = tier_label_of_file(&file, gbl_root);
        // Look without the lock first: most creates find nothing to archive in another tier, and the global graph's
        // lock is shared by every live session.
        let priors = match crate::store::load_or_empty(&file)
            .and_then(|store| priors_in(&store, ns, project_name, &inputs.titles))
        {
            Ok(priors) => priors,
            Err(e) => {
                unreadable.push(format!(
                    "could not read the {label} at {} to look for a prior handoff there: {e:#}",
                    file.display()
                ));
                continue;
            }
        };
        if split(priors, false).is_empty() {
            continue;
        }
        // Something to archive: decide again from the graph loaded inside the lock, so the names printed are the
        // names archived.
        let archive = crate::store::with_graph_lock(&file, || {
            let store = crate::store::load_or_empty(&file)?;
            let archive = split(priors_in(&store, ns, project_name, &inputs.titles)?, false);
            if !archive.is_empty() {
                let full = format!("{}\n{}", crud::prefixes(ns), archive_slugs_update(ns, &archive, project_name));
                crate::store::update_and_write(
                    &store,
                    &file,
                    &full,
                    crate::store::Scope::Target,
                    crate::store::Intent::Knowledge,
                )?;
            }
            Ok(archive)
        })
        .with_context(|| format!("{registered}, but archiving the prior handoff in the {label} failed"))?;
        archived.extend(archive.into_iter().map(|prior| (prior, label.to_string())));
    }
    if !unreadable.is_empty() {
        anyhow::bail!("{registered}, but {}", unreadable.join("; and "));
    }
    left_open.retain(|(slug, _)| !archived.iter().any(|(a, _)| a == slug));
    Ok(CreateOutcome {
        slug,
        tier: tier.to_string(),
        lane,
        archived,
        left_open,
    })
}

/// Register a fork pointing at a build-spec document. Forks are ADDITIVE —
/// creating one does NOT archive sibling forks (contrast `create`, which archives
/// the prior open handoff for the project). Slug defaults to the doc basename
/// (doc==slug protocol), so `handoff/<doc-basename>` is the node IRI; pass `slug`
/// to override. `resurfaceAt = now` so it surfaces next session.
pub fn create_fork(
    cwd: &Path,
    ns: &NamespaceConfig,
    project: &str,
    doc_path: &str,
    slug: Option<&str>,
) -> Result<String> {
    let now = crud::now_iso();
    let slug = resolve_doc_slug(slug, doc_path)?;
    let iri = crud::build_iri(ns, "handoff", &slug);
    let (path, graph) = write_tier(cwd, ns)?;
    let p = &ns.prefix;
    // The project this handoff names, as the IRI its domain hangs off (kite F7b).
    let project_iri = crud::build_iri(ns, "project", &crud::slugify(project));
    let project = crud::escape_sparql_literal(project);
    let name = crud::escape_sparql_literal(&slug);
    let doc = crud::escape_sparql_literal(doc_path);

    // Additive: no archive-prior. A re-create of the same slug re-points it
    // (idempotent) by deleting any existing node at this IRI first.
    let insert = format!(
        "DELETE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }} WHERE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }};\n\
         INSERT DATA {{ GRAPH <{graph}> {{\n\
           <{iri}> rdf:type {p}:Handoff ;\n\
             {p}:name \"{name}\" ;\n\
             {p}:project \"{project}\" ;\n\
             {p}:handoffDoc \"{doc}\" ;\n\
             {p}:kind \"fork\" ;\n\
             {p}:status \"open\" ;\n\
             {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:resurfaceAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
         }} }}"
    );

    // The fork takes the domain of the project it names, in the same write (kite F7b).
    let inherit = crate::domain::link::inherit_update(ns, &graph, &iri, &project_iri);
    mutate_file(&path, ns, &format!("{insert};\n{inherit}"))?;
    Ok(slug)
}

/// List handoffs (continuity docs only — forks excluded) across both tiers.
pub fn list(cwd: &Path, ns: &NamespaceConfig) -> Result<()> {
    let Some(store) = crate::store::load_merged(cwd) else {
        println!("No handoffs.");
        return Ok(());
    };
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?h ?project ?status ?resurfaceAt ?lastActive WHERE {{\n\
           GRAPH ?g {{\n\
             ?h a {p}:Handoff ;\n\
               {p}:project ?project ;\n\
               {p}:status ?status .\n\
             OPTIONAL {{ ?h {p}:resurfaceAt ?resurfaceAt }}\n\
             OPTIONAL {{ ?h {p}:lastActive ?lastActive }}\n\
             OPTIONAL {{ ?h {p}:kind ?kind }}\n\
             FILTER(!BOUND(?kind) || ?kind != \"fork\")\n\
           }}\n\
         }}\n\
         ORDER BY ?status ?project",
        pfx = crud::prefixes(ns)
    );

    if let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? {
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let get = |k: &str| {
                    row.get(k).map(|t| crud::term_display(t.into())).unwrap_or_default()
                };
                let h = get("h");
                let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
                vec![slug, get("project"), get("status"), get("resurfaceAt"), get("lastActive")]
            })
            .collect();

        if rows.is_empty() {
            println!("No handoffs.");
            return Ok(());
        }

        println!("| slug | project | status | resurfaceAt | lastActive |");
        println!("|------|---------|--------|-------------|------------|");
        for row in &rows {
            println!("| {} | {} | {} | {} | {} |", row[0], row[1], row[2], row[3], row[4]);
        }
    }
    Ok(())
}

/// List forks (parallel side-work build-specs) across both tiers. Multiple may
/// be open at once. Title == slug == doc basename.
pub fn list_forks(cwd: &Path, ns: &NamespaceConfig) -> Result<()> {
    let Some(store) = crate::store::load_merged(cwd) else {
        println!("No forks.");
        return Ok(());
    };
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?h ?project ?status ?doc WHERE {{\n\
           GRAPH ?g {{\n\
             ?h a {p}:Handoff ;\n\
               {p}:kind \"fork\" ;\n\
               {p}:project ?project ;\n\
               {p}:status ?status ;\n\
               {p}:handoffDoc ?doc .\n\
           }}\n\
         }}\n\
         ORDER BY ?status ?h",
        pfx = crud::prefixes(ns)
    );

    if let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? {
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let get = |k: &str| {
                    row.get(k).map(|t| crud::term_display(t.into())).unwrap_or_default()
                };
                let h = get("h");
                let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
                vec![slug, get("project"), get("status"), get("doc")]
            })
            .collect();

        if rows.is_empty() {
            println!("No forks.");
            return Ok(());
        }

        println!("| title | project | status | doc |");
        println!("|-------|---------|--------|-----|");
        for row in &rows {
            println!("| {} | {} | {} | {} |", row[0], row[1], row[2], row[3]);
        }
    }
    Ok(())
}

/// Snooze a handoff: push `resurfaceAt` to now + `days`, hiding it until then.
/// Applied to every tier file so it works wherever the handoff lives.
pub fn snooze(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    days: i64,
) -> Result<Vec<String>> {
    let iri = crud::build_iri(ns, "handoff", slug);
    let wake = (chrono::Local::now() + chrono::Duration::days(days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let p = &ns.prefix;
    let sparql = format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt ?old }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt \"{wake}\"^^xsd:dateTime }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Handoff }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt ?old }} }} }}"
    );
    apply_to_tiers(gbl_root, cwd, ns, slug, &sparql)
}

/// Archive a handoff: set status to "archived" so it stops resurfacing.
pub fn archive(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
) -> Result<Vec<String>> {
    let iri = crud::build_iri(ns, "handoff", slug);
    let p = &ns.prefix;
    let sparql = format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:status \"archived\" }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Handoff }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }} }}"
    );
    apply_to_tiers(gbl_root, cwd, ns, slug, &sparql)
}

/// What `unarchive` found for the slug in one tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unarchived {
    /// "workspace tier" or "global tier".
    pub tier: String,
    /// The status the record had there before.
    pub before: String,
    pub outcome: UnarchiveOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnarchiveOutcome {
    /// Archived, and now open again.
    Reopened,
    /// Archived, and left so: another tier holds the same slug open, or holds a newer copy. Two tiers can hold one
    /// slug when it was registered in one and then the other, and `create` archives the older copy (ruling D1).
    LeftArchived,
    /// Not archived here (open, deferred), so there was nothing to undo.
    NotArchived,
}

/// Undo an archive (BO-11, F18c): set an archived handoff or fork back to `open`,
/// in the tier that holds it, and mark it active now so the defer pass does not park
/// it again on the spot (the deferral fields an archived deferred handoff kept go
/// too, as `deferred::revive_handoff` clears them). An empty vec means no tier holds
/// the slug at all.
///
/// When two tiers hold the slug, only the newest copy (by `createdAt`, else the
/// tier `cwd` writes to) is reopened, and nothing is while any tier holds it open:
/// the other is the older copy `create` archived as a duplicate, and reopening it
/// would make the handoff open twice, once in the global tier where every project
/// sees it.
///
/// Until this existed an archive could not be undone, so a handoff archived by
/// another session's create was lost for good (F18).
pub fn unarchive(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
) -> Result<Vec<Unarchived>> {
    let iri = crud::build_iri(ns, "handoff", slug);
    let p = &ns.prefix;
    let now = crud::now_iso();
    let held_q = format!(
        "{}\nSELECT ?s ?created WHERE {{ GRAPH ?g {{ <{iri}> a {p}:Handoff ; {p}:status ?s .\n\
           OPTIONAL {{ <{iri}> {p}:createdAt ?created }} }} }}",
        crud::prefixes(ns)
    );
    let reopen = format!(
        "{}\nDELETE {{ GRAPH ?g {{ <{iri}> {p}:status \"archived\" . <{iri}> {p}:lastActive ?la .\n\
                                <{iri}> {p}:deferredReason ?why . <{iri}> {p}:deferredAt ?at }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:status \"open\" . <{iri}> {p}:lastActive \"{now}\"^^xsd:dateTime }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Handoff ; {p}:status \"archived\" }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:lastActive ?la }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:deferredReason ?why }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:deferredAt ?at }} }} }}",
        crud::prefixes(ns)
    );
    // (statuses, newest createdAt) of the slug in one loaded graph; `None` when it does not hold it.
    let held = |store: &Store| -> Result<Option<(Vec<String>, String)>> {
        let QueryResults::Solutions(solutions) = crate::store::query(store, &held_q)? else {
            return Ok(None);
        };
        let (mut statuses, mut created) = (Vec::new(), String::new());
        for sol in solutions.filter_map(|sol| sol.ok()) {
            if let Some(s) = sol.get("s").map(|t| crud::term_display(t.as_ref())) {
                statuses.push(s);
            }
            if let Some(c) = sol.get("created").map(|t| crud::term_display(t.as_ref())) {
                created = created.max(c);
            }
        }
        statuses.sort();
        statuses.dedup();
        Ok((!statuses.is_empty()).then_some((statuses, created)))
    };

    // Read every tier first, without its lock; only the tier written is locked.
    let target_tier = crate::config::find_workspace_base(cwd).map(|b| b.join("graph.nq"));
    let mut seen: Vec<(PathBuf, String, Vec<String>, String)> = Vec::new();
    for file in all_tier_files(gbl_root, cwd) {
        let tier = tier_label_of_file(&file, gbl_root).to_string();
        let store = crate::store::load_or_empty(&file)?;
        if let Some((statuses, created)) = held(&store)? {
            seen.push((file, tier, statuses, created));
        }
    }
    // The newest copy: latest createdAt, a tie going to the tier `cwd` writes to.
    let newest = seen
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            a.3.cmp(&b.3).then_with(|| (target_tier.as_ref() == Some(&a.0)).cmp(&(target_tier.as_ref() == Some(&b.0))))
        })
        .map(|(i, _)| i);
    let open_somewhere = seen.iter().any(|(_, _, statuses, _)| statuses.iter().any(|s| s != "archived"));
    let mut found = Vec::new();
    for (i, (file, tier, statuses, _)) in seen.iter().enumerate() {
        let archived = statuses.iter().any(|s| s == "archived");
        let outcome = if !archived {
            UnarchiveOutcome::NotArchived
        } else if open_somewhere || Some(i) != newest {
            UnarchiveOutcome::LeftArchived
        } else {
            // Decide again inside the lock, from the graph the write is made to.
            let reopened = crate::store::with_graph_lock(file, || {
                let store = crate::store::load_or_empty(file)?;
                let still = held(&store)?.is_some_and(|(s, _)| s.iter().any(|s| s == "archived"));
                if still {
                    crate::store::update_and_write(
                        &store,
                        file,
                        &reopen,
                        crate::store::Scope::Target,
                        crate::store::Intent::Knowledge,
                    )
                    .with_context(|| format!("unarchive failed in the {tier}: {reopen}"))?;
                }
                Ok(still)
            })?;
            if reopened { UnarchiveOutcome::Reopened } else { UnarchiveOutcome::NotArchived }
        };
        found.push(Unarchived { tier: tier.clone(), before: statuses.join(", "), outcome });
    }
    Ok(found)
}

/// Run `sparql` against every tier file that actually holds `slug`, and return
/// the tier labels that changed.
///
/// An empty vec means no tier held it — the caller must NOT print success. That
/// is the whole of #72's observable: 0.14.1 ran the UPDATE over each tier file
/// and printed `archived` unconditionally, so a no-op and a real archive were
/// indistinguishable from the outside.
pub(crate) fn apply_to_tiers(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    sparql: &str,
) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for f in all_tier_files(gbl_root, cwd) {
        if mutate_file_if_holds(&f, ns, slug, sparql)? {
            changed.push(tier_label_of_file(&f, gbl_root).to_string());
        }
    }
    Ok(changed)
}

/// The tier files a lookup searched, for the not-found sentence.
pub fn searched_tiers(gbl_root: Option<&Path>, cwd: &Path) -> Vec<String> {
    all_tier_files(gbl_root, cwd)
        .iter()
        .map(|f| format!("{} ({})", f.display(), tier_label_of_file(f, gbl_root)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn after_date_drops_the_date_and_the_time_when_there_is_one() {
        assert_eq!(after_date("2026-09-20-2100-auk-base-0160"), Some("auk-base-0160"));
        assert_eq!(after_date("2026-09-21-plover-queries-toml"), Some("plover-queries-toml"));
        assert_eq!(after_date("auk-base-0160"), None);
        assert_eq!(after_date("2026-9-20-auk"), None);
        assert_eq!(after_date("2026-09-20"), None);
    }

    #[test]
    fn the_codename_in_a_slug_is_the_longest_title_that_fits() {
        let known = titles(&["otter", "otter-bo11", "auk", "chris"]);
        assert_eq!(slug_codename("2026-10-02-2300-otter-bo11-base-0160", &known).as_deref(), Some("otter-bo11"));
        assert_eq!(slug_codename("2026-10-01-1200-otter-base-0160", &known).as_deref(), Some("otter"));
        assert_eq!(slug_codename("2026-09-20-2100-AUK-base-0160", &known).as_deref(), Some("auk"));
        assert_eq!(slug_codename("2026-09-20-2100-auk", &known).as_deref(), Some("auk"), "the whole rest");
        // Not a title, not at the start, or no date: no lane, never a guess.
        assert_eq!(slug_codename("2026-09-20-2100-grebe-base-0160", &known), None);
        assert_eq!(slug_codename("2026-09-20-2100-base-auk-0160", &known), None);
        assert_eq!(slug_codename("2026-09-20-2100-auklet-base", &known), None, "a title is a whole word");
        assert_eq!(slug_codename("auk-base-0160", &known), None);
    }

    #[test]
    fn lanes_match_by_name_case_aside_and_unnamed_only_with_unnamed() {
        assert!(same_lane(Some("dealer-crawl"), Some("Dealer-Crawl")));
        assert!(same_lane(None, None));
        assert!(!same_lane(Some("auk"), None));
        assert!(!same_lane(Some("auk"), Some("plover")));
    }
}
