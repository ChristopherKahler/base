use std::path::Path;

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;

use crate::config::{BaseConfig, NamespaceConfig, ProtocolConfig, WorkspaceEntry};
use crate::crud;
use crate::scope;

pub fn add(
    cwd: &Path,
    ns: &NamespaceConfig,
    name: &str,
    status: &str,
    path: Option<&str>,
) -> Result<String> {
    add_with_stage(cwd, ns, name, status, path, None)
}

/// Like [`add`], but also records the project's protocol lifecycle stage.
pub fn add_with_stage(
    cwd: &Path,
    ns: &NamespaceConfig,
    name: &str,
    status: &str,
    path: Option<&str>,
    stage: Option<&str>,
) -> Result<String> {
    let slug = crud::slugify(name);
    let iri = crud::build_iri(ns, "project", &slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let ws_iri = crud::build_iri(ns, "workspace", &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;
    let project_path = path
        .map(|s| s.to_string())
        .unwrap_or_else(|| cwd.to_string_lossy().to_string());

    let name = crud::escape_sparql_literal(name);
    let project_path = crud::escape_sparql_literal(&project_path);
    let stage_triple = match stage {
        Some(s) => format!("               {p}:stage \"{}\" ;\n", crud::escape_sparql_literal(s)),
        None => String::new(),
    };

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> rdf:type {p}:Project ;\n\
               {p}:name \"{name}\" ;\n\
               {p}:status \"{status}\" ;\n\
               {p}:path \"{project_path}\" ;\n\
{stage_triple}               {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
               {p}:lastActive \"{now}\"^^xsd:dateTime ;\n\
               {p}:belongsTo <{ws_iri}> .\n\
           }}\n\
         }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)?;

    // Auto-create domain trigger with path matching (filesystem-first, no keywords by default)
    auto_create_domain(cwd, &name, &project_path)?;

    // Link project to its domain in the graph
    let domain_slug = crud::slugify(&name);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);
    let link_sparql = format!(
        "INSERT DATA {{ GRAPH <{graph}> {{ <{iri}> {p}:hasDomain <{domain_iri}> }} }}"
    );
    let _ = crud::load_and_mutate(cwd, ns, &link_sparql);

    Ok(slug)
}

/// Resolve a new project's artifact folder from the protocol, create it (and its
/// optional context doc), and return (workspace-relative folder, stage name).
/// Returns Ok(None) when the protocol is disabled or defines no stages — the caller
/// then falls back to an explicit --path.
pub fn provision_folder(
    cwd: &Path,
    protocol: &ProtocolConfig,
    name: &str,
    slug: &str,
    stage: Option<&str>,
) -> Result<Option<(String, String)>> {
    if !protocol.enabled {
        return Ok(None);
    }
    let Some(stage_def) = protocol.stage_for(stage) else {
        return Ok(None);
    };
    let rel_folder = stage_def.folder.replace("{slug}", slug);

    // Workspace root = the directory containing .base/ (fallback: cwd).
    let ws_root = crate::config::find_workspace_base(cwd)
        .and_then(|b| b.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| cwd.to_path_buf());
    let abs_folder = ws_root.join(&rel_folder);
    std::fs::create_dir_all(&abs_folder)
        .with_context(|| format!("creating project folder {}", abs_folder.display()))?;

    // Context doc — created once, never overwritten.
    if let Some(doc) = &stage_def.context_doc {
        let doc_path = abs_folder.join(doc);
        if !doc_path.exists() {
            let now = crud::now_iso();
            let body = format!(
                "---\ntype: context\nstatus: active\ntags: [{slug}]\n---\n\n\
                 # {name}\n\n\
                 Project context — this folder is the artifact home; touching it keeps the project fresh.\n\n\
                 ## Goal\n\n## Status\nCreated {now} \u{00b7} stage: {stage_name}\n",
                slug = slug,
                name = name,
                now = now,
                stage_name = stage_def.name,
            );
            std::fs::write(&doc_path, body)
                .with_context(|| format!("writing context doc {}", doc_path.display()))?;
        }
    }

    Ok(Some((rel_folder, stage_def.name.clone())))
}

/// Auto-create a domain trigger entry in the nearest domains.toml.
/// Default: path-based matching. No keywords unless user adds them later.
fn auto_create_domain(cwd: &Path, project_name: &str, project_path: &str) -> Result<()> {
    // Add a path trigger via the existing add_trigger mechanism
    crate::domain::add_trigger(cwd, project_name, None, Some(project_path))?;
    Ok(())
}

// Canonicalization logic lives once in `scope` (the shared FS wrappers); these are thin
// local aliases. `canon_path` stays local — scope only wraps strings/registries.
fn canon_str(p: &str) -> String {
    scope::canonical_str(p)
}

fn canon_path(p: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn canon_registry(reg: &[WorkspaceEntry]) -> Vec<WorkspaceEntry> {
    scope::canonical_registry(reg)
}

pub fn list(cwd: &Path, config: &BaseConfig, project_scope: scope::ProjectScope) -> Result<()> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?proj ?name ?status ?priority ?lastActive ?path WHERE {{\n\
           GRAPH ?g {{\n\
             ?proj a {p}:Project ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status .\n\
             OPTIONAL {{ ?proj {p}:priority ?priority }}\n\
             OPTIONAL {{ ?proj {p}:lastActive ?lastActive }}\n\
             OPTIONAL {{ ?proj {p}:path ?path }}\n\
           }}\n\
         }}\n\
         ORDER BY ?name"
    );

    // FS wrapper around the pure scope:: core: canonicalize cwd + registry once,
    // then resolve the current workspace and filter rows by #path-derived home.
    let registry = canon_registry(&config.workspace);
    let current = scope::current_workspace(&canon_path(cwd), &registry);
    let peer_map = peer_workspaces(cwd, ns)?;

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let QueryResults::Solutions(solutions) = results else {
        return Ok(());
    };

    const COLS: [&str; 4] = ["name", "status", "priority", "lastActive"];
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut unscoped_count = 0usize;
    for sol in solutions.filter_map(|r| r.ok()) {
        let cell = |k: &str| sol.get(k).map(|t| crud::term_display(t.into()));
        let home = scope::home(
            cell("path").map(|s| canon_str(&s)).as_deref(),
            &registry,
        );
        if matches!(home, scope::Home::Unscoped) {
            unscoped_count += 1;
        }
        let peers = cell("proj")
            .and_then(|iri| peer_map.get(&iri).cloned())
            .unwrap_or_default();
        if !scope::in_scope(&home, &peers, current.as_deref(), &project_scope) {
            continue;
        }
        rows.push(
            COLS.iter()
                .map(|c| cell(c).unwrap_or_else(|| "-".into()))
                .collect(),
        );
    }

    if rows.is_empty() {
        let where_ = match &project_scope {
            scope::ProjectScope::All => "any workspace".to_string(),
            scope::ProjectScope::Unscoped => "the unscoped set".to_string(),
            scope::ProjectScope::Workspace(w) => format!("workspace '{w}'"),
            scope::ProjectScope::Current => match &current {
                Some(c) => format!("workspace '{c}'"),
                None => "any workspace".to_string(),
            },
        };
        println!("No projects in {where_}.");
    } else {
        println!("| {} |", COLS.join(" | "));
        println!("|{}|", COLS.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
        for row in &rows {
            println!("| {} |", row.join(" | "));
        }
    }

    // Backfill nudge: never silently lose un-homed projects (Req 3). Only in the
    // workspace-scoped (Current) view — `--all`/`--unscoped` already surface them.
    if matches!(project_scope, scope::ProjectScope::Current) && unscoped_count > 0 {
        println!(
            "\n{unscoped_count} project(s) have no #path (unscoped) — `base project list --unscoped`, or set a home with `base project update <slug> --path <dir>`."
        );
    }
    Ok(())
}

/// Map every project IRI → its `peerWorkspace` slugs (read-time scoping input).
fn peer_workspaces(
    cwd: &Path,
    ns: &NamespaceConfig,
) -> Result<std::collections::HashMap<String, Vec<String>>> {
    let p = &ns.prefix;
    let sparql = format!("SELECT ?proj ?peer WHERE {{ GRAPH ?g {{ ?proj {p}:peerWorkspace ?peer }} }}");
    let mut map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    if let QueryResults::Solutions(sols) = crud::load_and_query(cwd, ns, &sparql)? {
        for sol in sols.filter_map(|r| r.ok()) {
            let iri = sol.get("proj").map(|t| crud::term_display(t.into()));
            let peer = sol.get("peer").map(|t| crud::term_display(t.into()));
            if let (Some(iri), Some(peer)) = (iri, peer) {
                map.entry(iri).or_default().push(peer);
            }
        }
    }
    Ok(map)
}

/// Add (or remove with `remove = true`) a `peerWorkspace` edge so a project surfaces in another
/// workspace too. The home graph stays the single source of truth; the edge is a visibility
/// pointer, not a duplicate. Add writes to the CWD workspace graph; remove is graph-agnostic.
pub fn peer(cwd: &Path, config: &BaseConfig, slug: &str, workspace: &str, remove: bool) -> Result<()> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let iri = crud::build_iri(ns, "project", slug);
    let peer_slug = crud::slugify(workspace);
    let sparql = if remove {
        format!("DELETE WHERE {{ GRAPH ?g {{ <{iri}> {p}:peerWorkspace \"{peer_slug}\" }} }}")
    } else {
        let graph = crud::workspace_graph_iri(ns, &crud::workspace_slug(cwd));
        format!("INSERT DATA {{ GRAPH <{graph}> {{ <{iri}> {p}:peerWorkspace \"{peer_slug}\" }} }}")
    };
    crud::load_and_mutate(cwd, ns, &sparql)?;
    let verb = if remove { "removed from" } else { "added to" };
    println!("Project '{slug}': peer workspace '{peer_slug}' {verb}.");
    Ok(())
}

pub fn get(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let iri = crud::build_iri(ns, "project", slug);
    let sparql = format!(
        "SELECT ?pred ?obj WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> ?pred ?obj .\n\
           }}\n\
         }}"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<(String, String)> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let pred = row
                    .get("pred")
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default();
                let obj = row
                    .get("obj")
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default();
                (pred, obj)
            })
            .collect();

        if rows.is_empty() {
            eprintln!("Project '{slug}' not found.");
            return Ok(());
        }

        println!("Project: {slug}");
        for (pred, obj) in &rows {
            // Skip rdf:type display name — show it as "type"
            let label = if pred == "type" {
                "type".to_string()
            } else {
                pred.clone()
            };
            println!("  {label}: {obj}");
        }
    }
    Ok(())
}

pub fn update(
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    status: Option<&str>,
    blocked_by: Option<&str>,
    next_action: Option<&str>,
) -> Result<()> {
    let iri = crud::build_iri(ns, "project", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    let mut updates = Vec::new();

    if let Some(s) = status {
        updates.push(crud::field_update(&graph, &iri, &format!("{p}:status"), &format!("\"{s}\"")));
    }
    if let Some(b) = blocked_by {
        updates.push(crud::field_update(&graph, &iri, &format!("{p}:blockedBy"), &format!("\"{b}\"")));
    }
    if let Some(n) = next_action {
        updates.push(crud::field_update(&graph, &iri, &format!("{p}:nextAction"), &format!("\"{n}\"")));
    }

    // Always update timestamps
    updates.push(crud::field_update(
        &graph,
        &iri,
        &format!("{p}:updatedAt"),
        &format!("\"{now}\"^^xsd:dateTime"),
    ));
    updates.push(crud::field_update(
        &graph,
        &iri,
        &format!("{p}:lastActive"),
        &format!("\"{now}\"^^xsd:dateTime"),
    ));

    let sparql = updates.join(" ;\n");
    crud::load_and_mutate(cwd, ns, &sparql)
}

/// Lightweight (slug, display-name, stored-path) for every project — used by the
/// folder-move nudge to match a moved directory against registered project paths.
pub fn list_paths(cwd: &Path, ns: &NamespaceConfig) -> Result<Vec<(String, String, String)>> {
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?iri ?name ?path WHERE {{ GRAPH ?g {{ \
         ?iri a {p}:Project ; {p}:name ?name ; {p}:path ?path }} }}"
    );
    let QueryResults::Solutions(sols) = crud::load_and_query(cwd, ns, &sparql)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for row in sols.filter_map(|r| r.ok()) {
        let disp = |k: &str| row.get(k).map(|t| crud::term_display(t.into())).unwrap_or_default();
        let iri = disp("iri");
        let slug = iri.rsplit('/').next().unwrap_or(&iri).to_string();
        out.push((slug, disp("name"), disp("path")));
    }
    Ok(out)
}

/// Result of a repath, for caller reporting.
pub struct RepathResult {
    pub name: String,
    pub old_path: Option<String>,
    pub new_path: String,
    pub domain_changed: bool,
}

/// Re-point a project's folder in the graph (`ops:path`) AND swap its domain trigger
/// in domains.toml. This is the corrective for path drift — a folder that moved (e.g.
/// a framework promoted into the toolbox workspace) keeps being measured by the
/// active-state reconcile, and its domain keeps matching. The domain name is the
/// project's display name (the auto-created domain on `project add`).
pub fn repath(cwd: &Path, ns: &NamespaceConfig, slug: &str, new_path: &str) -> Result<RepathResult> {
    let iri = crud::build_iri(ns, "project", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let p = &ns.prefix;

    // Current name (= domain name) + old path, for the trigger swap + reporting.
    let sel = format!(
        "SELECT ?name ?path WHERE {{ GRAPH <{graph}> {{ <{iri}> {p}:name ?name . \
         OPTIONAL {{ <{iri}> {p}:path ?path }} }} }}"
    );
    let (name, old_path) = match crud::load_and_query(cwd, ns, &sel)? {
        QueryResults::Solutions(sols) => {
            let mut name = String::new();
            let mut old = None;
            for row in sols.filter_map(|r| r.ok()) {
                name = row.get("name").map(|t| crud::term_display(t.into())).unwrap_or_default();
                old = row.get("path").map(|t| crud::term_display(t.into()));
            }
            (name, old)
        }
        _ => (String::new(), None),
    };
    if name.is_empty() {
        anyhow::bail!("project '{slug}' not found in this workspace graph");
    }

    // 1) Update the graph path.
    let np = crud::escape_sparql_literal(new_path);
    let upd = crud::field_update(&graph, &iri, &format!("{p}:path"), &format!("\"{np}\""));
    crud::load_and_mutate(cwd, ns, &upd)?;

    // 2) Swap the domain trigger (domain name == project display name).
    let domain_changed =
        crate::domain::repath_trigger(cwd, &name, old_path.as_deref(), new_path).unwrap_or(false);

    Ok(RepathResult { name, old_path, new_path: new_path.to_string(), domain_changed })
}
