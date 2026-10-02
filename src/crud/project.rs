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
    // F25b: stored absolute, `/`-separated; a relative path is the workspace root's.
    let raw_path = path
        .map(|s| s.to_string())
        .unwrap_or_else(|| cwd.to_string_lossy().to_string());
    let project_path = PathRoots::new(cwd, ns).from_cli(&raw_path).unwrap_or(raw_path);

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
    // #127. This was `let _ =`, so a failed write here was discarded and `add`
    // returned `Ok(slug)` regardless -- `base project add` printed success over
    // a link that is not in the graph. MEASURED by inducing a real rename
    // failure on this exact write: exit 0, "Project '…' created", and zero
    // `hasDomain` quads in the store. The same injection one write earlier, at
    // `:61`, exits non-zero and says why, so the silence was this line's alone.
    //
    // The project record at `:61` has already landed by the time we get here, so
    // the message says what IS in the graph as well as what is not: a bare
    // failure would read as "nothing happened", which is the opposite of true.
    crud::load_and_mutate(cwd, ns, &link_sparql).with_context(|| {
        format!(
            "project '{slug}' IS registered, but linking it to domain \
             '{domain_slug}' failed and that link is NOT in the graph"
        )
    })?;

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
    // Add a path trigger via the existing add_trigger mechanism. false: a project
    // registered in a workspace files its domain there, which is what this always did
    // before the tier seam made it explicit. A refused trigger (F29 step 6: the path
    // covers other registered projects) leaves the project registered, creates no
    // domain, and says why; any other failure still propagates.
    match crate::domain::add_trigger(cwd, false, project_name, None, Some(project_path)) {
        Ok(_) => Ok(()),
        Err(e) if e.downcast_ref::<crate::domain::TriggerRefused>().is_some() => {
            eprintln!("base: project {project_name}: {e}; no domain was created");
            Ok(())
        }
        Err(e) => Err(e),
    }
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

// ─── F25b: one spelling for a project folder ─────────────────

/// A project folder as base stores it (F25b): absolute, `/`-separated, `.` and `..` folded, a drive letter
/// upper-cased, `\\server\share` as `//server/share`. A relative `raw` is joined to `root` (the workspace root the
/// record is filed under), `~` to `home`. `None` for an empty path, or a relative one with nothing to join it to.
pub fn absolute_path(raw: &str, root: Option<&Path>, home: Option<&Path>) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let joined = if crate::domain::matcher::is_absolute(t) {
        t.to_string()
    } else if t == "~" || t.starts_with("~/") || t.starts_with("~\\") {
        format!("{}/{}", home?.display(), &t[1..])
    } else {
        format!("{}/{}", root?.display(), t)
    };
    Some(fold_path(&joined))
}

fn is_drive(c: &str) -> bool {
    c.len() == 2 && c.as_bytes()[1] == b':' && c.as_bytes()[0].is_ascii_alphabetic()
}

/// `\` is `/`, empty and `.` parts go, `..` drops the part before it but never climbs past the drive, the share or `/`.
fn fold_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    let unc = s.starts_with("//");
    let rooted = s.starts_with('/');
    let mut parts: Vec<String> = Vec::new();
    for c in s.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                let floor = if unc { 2 } else { usize::from(parts.first().is_some_and(|f| is_drive(f))) };
                if parts.len() > floor {
                    parts.pop();
                }
            }
            c => parts.push(c.to_string()),
        }
    }
    if let Some(first) = parts.first_mut()
        && is_drive(first)
    {
        *first = first.to_ascii_uppercase();
    }
    let body = parts.join("/");
    if unc {
        format!("//{body}")
    } else if rooted {
        format!("/{body}")
    } else if parts.len() == 1 && is_drive(&parts[0]) {
        format!("{body}/")
    } else {
        body
    }
}

/// Where a relative project path is rooted (F25b), the way `domain::registered_projects` roots one for the trigger
/// rules: a record in this workspace's own graph against the workspace root, a record in any other graph against
/// home. A path typed on the command line is the workspace root's (home's, outside a workspace).
pub struct PathRoots {
    ws_graph: String,
    ws_root: Option<std::path::PathBuf>,
    home: Option<std::path::PathBuf>,
}

impl PathRoots {
    pub fn new(cwd: &Path, ns: &NamespaceConfig) -> Self {
        let home = crate::home::home_root();
        // The global tier (`~/.base-gbl/.base`) is rooted at home, as doctor and the trigger rules root it.
        let base = crate::config::find_workspace_base(cwd);
        let global = home.as_ref().map(|h| h.join(".base-gbl").join(".base"));
        let ws_root = match (&base, &global) {
            (Some(b), Some(g)) if crate::scope::canonical_str(&b.display().to_string()) == crate::scope::canonical_str(&g.display().to_string()) => {
                home.clone()
            }
            _ => base.and_then(|b| b.parent().map(Path::to_path_buf)),
        };
        Self { ws_graph: crud::workspace_graph_iri(ns, &crud::workspace_slug(cwd)), ws_root, home }
    }

    /// A path typed on the command line.
    pub fn from_cli(&self, raw: &str) -> Option<String> {
        absolute_path(raw, self.ws_root.as_deref().or(self.home.as_deref()), self.home.as_deref())
    }

    /// A path read from the record filed in `graph` (the full graph IRI). Unrootable: as stored.
    pub fn stored(&self, raw: &str, graph: Option<&str>) -> String {
        let root = if graph == Some(self.ws_graph.as_str()) { self.ws_root.as_deref() } else { self.home.as_deref() };
        absolute_path(raw, root, self.home.as_deref()).unwrap_or_else(|| raw.to_string())
    }

    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.ws_root.as_deref()
    }
}

/// A named graph's full IRI from a query row (`term_display` would cut it at the `#`).
fn graph_cell(sol: &oxigraph::sparql::QuerySolution, key: &str) -> Option<String> {
    sol.get(key).and_then(|t| match t {
        oxigraph::model::Term::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    })
}

// ─── F23: how old a next step is ─────────────────────────────

/// Whole days since `at` (an RFC 3339 time), `None` when undated or unreadable.
pub fn age_days(at: Option<&str>, now: chrono::DateTime<chrono::Local>) -> Option<i64> {
    let t = chrono::DateTime::parse_from_rfc3339(at?).ok()?;
    Some(now.signed_duration_since(t).num_days().max(0))
}

/// `(undated)`, `(0 days)`, `(1 day)`, `(12 days)`.
pub fn age_label(days: Option<i64>) -> String {
    match days {
        None => "(undated)".to_string(),
        Some(1) => "(1 day)".to_string(),
        Some(n) => format!("({n} days)"),
    }
}

/// The first `max` characters of `s`, cut back to a word, with ` ...` when anything was cut.
pub fn excerpt(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    let head = match cut.rfind(' ') {
        Some(i) if i > 0 => cut[..i].trim_end(),
        _ => cut.as_str(),
    };
    format!("{head} ...")
}

/// One project, all fields the graph holds. Stable `--json` contract for the dashboard.
#[derive(Debug, serde::Serialize)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub status: String,
    pub priority: Option<String>,
    /// Absolute, `/`-separated (F25b): a relative stored path is resolved on read.
    pub path: Option<String>,
    pub stage: Option<String>,
    pub blocked_by: Option<String>,
    pub next_action: Option<String>,
    /// When the next step was written (F23a); `None` for one written before 0.16.0.
    pub next_action_at: Option<String>,
    /// Whole days since `next_action_at`; `None` when undated.
    pub next_action_age_days: Option<i64>,
    /// The parent project's slug (D13).
    pub parent: Option<String>,
    /// Work in this project also carries its parent's rules (D13). False unless set.
    pub nested: bool,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub last_active: Option<String>,
}

const RECORD_FIELDS: &str = "?priority ?path ?stage ?blockedBy ?nextAction ?nextActionAt ?parent ?nested ?created ?updated ?lastActive";

/// The OPTIONAL patterns behind [`RECORD_FIELDS`] for subject `s` (a `?var` or an `<iri>`).
fn record_optionals(p: &str, s: &str) -> String {
    [
        ("priority", "priority"),
        ("path", "path"),
        ("stage", "stage"),
        ("blockedBy", "blockedBy"),
        ("nextAction", "nextAction"),
        ("nextActionAt", "nextActionAt"),
        ("parentProject", "parent"),
        ("nested", "nested"),
        ("createdAt", "created"),
        ("updatedAt", "updated"),
        ("lastActive", "lastActive"),
    ]
    .iter()
    .map(|(pred, var)| format!("             OPTIONAL {{ {s} {p}:{pred} ?{var} }}\n"))
    .collect()
}

/// A record from one query row. `path` is the stored path, already resolved by the caller.
fn record_from(
    id: String,
    path: Option<String>,
    cell: &dyn Fn(&str) -> Option<String>,
    now: chrono::DateTime<chrono::Local>,
) -> ProjectRecord {
    let next_action_at = cell("nextActionAt");
    let next_action = cell("nextAction");
    ProjectRecord {
        id,
        name: cell("name").unwrap_or_default(),
        status: cell("status").unwrap_or_default(),
        priority: cell("priority"),
        path,
        stage: cell("stage"),
        blocked_by: cell("blockedBy"),
        next_action_age_days: next_action.as_ref().and(age_days(next_action_at.as_deref(), now)),
        next_action,
        next_action_at,
        parent: cell("parent").map(|p| crud::slug_of(&p)),
        nested: cell("nested").is_some_and(|v| v == "true"),
        created: cell("created"),
        updated: cell("updated"),
        last_active: cell("lastActive"),
    }
}

/// Query scoped project records (typed). Returns the in-scope records plus the count
/// of unscoped (no-#path) projects — the shared core behind human `list` and `--json`
/// `list_json`. Scoping logic is identical for both surfaces.
pub fn list_data(
    cwd: &Path,
    config: &BaseConfig,
    project_scope: &scope::ProjectScope,
) -> Result<(Vec<ProjectRecord>, usize)> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let optionals = record_optionals(p, "?proj");
    let sparql = format!(
        "SELECT ?proj ?g ?name ?status {RECORD_FIELDS} WHERE {{\n\
           GRAPH ?g {{\n\
             ?proj a {p}:Project ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status .\n\
{optionals}\
           }}\n\
         }}\n\
         ORDER BY ?name"
    );

    // FS wrapper around the pure scope:: core: canonicalize cwd + registry once,
    // then resolve the current workspace and filter rows by #path-derived home.
    let registry = canon_registry(&config.workspace);
    let current = scope::current_workspace(&canon_path(cwd), &registry);
    // #19: outside a workspace this bailed with rc 1 while every other reader
    // quietly answered from the global tier. Read the global tier here too and let
    // the CLI print the sentence naming it. `scope::in_scope` already returns true
    // for `Current` when there is no current workspace, so nothing is hidden.
    let read_cwd = match crate::config::find_workspace_base(cwd) {
        Some(_) => cwd.to_path_buf(),
        None => crate::home::home_root()
            .map(|h| h.join(".base-gbl"))
            .unwrap_or_else(|| cwd.to_path_buf()),
    };
    let peer_map = peer_workspaces(&read_cwd, ns)?;

    let results = crud::load_and_query(&read_cwd, ns, &sparql)?;
    let QueryResults::Solutions(solutions) = results else {
        return Ok((Vec::new(), 0));
    };

    let roots = PathRoots::new(&read_cwd, ns);
    let now = chrono::Local::now();
    let mut records: Vec<ProjectRecord> = Vec::new();
    let mut unscoped_count = 0usize;
    for sol in solutions.filter_map(|r| r.ok()) {
        let cell = |k: &str| sol.get(k).map(|t| crud::term_display(t.into()));
        // F25b: a relative stored path is resolved before anything reads it, so it can find its workspace.
        let path = cell("path").map(|raw| roots.stored(&raw, graph_cell(&sol, "g").as_deref()));
        let home = scope::home(
            path.as_deref().map(canon_str).as_deref(),
            &registry,
        );
        if matches!(home, scope::Home::Unscoped) {
            unscoped_count += 1;
        }
        let proj_iri = cell("proj");
        let peers = proj_iri
            .as_ref()
            .and_then(|iri| peer_map.get(iri).cloned())
            .unwrap_or_default();
        if !scope::in_scope(&home, &peers, current.as_deref(), project_scope) {
            continue;
        }
        let id = proj_iri.as_deref().map(crud::slug_of).unwrap_or_default();
        records.push(record_from(id, path, &cell, now));
    }
    Ok((records, unscoped_count))
}

/// A markdown table cell: one line, no column breaks.
fn cell_text(s: &str) -> String {
    s.replace(['\r', '\n'], " ").replace('|', "\\|")
}

/// The `next` column (F23b): the step, shortened, and its age.
fn next_cell(r: &ProjectRecord) -> String {
    match &r.next_action {
        Some(n) => format!("{} {}", cell_text(&excerpt(n, 60)), age_label(r.next_action_age_days)),
        None => "-".to_string(),
    }
}

pub fn list(cwd: &Path, config: &BaseConfig, project_scope: scope::ProjectScope) -> Result<()> {
    let (records, unscoped_count) = list_data(cwd, config, &project_scope)?;
    let registry = canon_registry(&config.workspace);
    let current = scope::current_workspace(&canon_path(cwd), &registry);

    if records.is_empty() {
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
        // F25e, F23b: the folder, the parent link and the next step's age are on every row.
        const COLS: [&str; 7] = ["name", "status", "path", "parent", "nested", "next", "lastActive"];
        println!("| {} |", COLS.join(" | "));
        println!("|{}|", COLS.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
        for r in &records {
            println!(
                "| {} | {} | {} | {} | {} | {} | {} |",
                cell_text(&r.name),
                cell_text(&r.status),
                r.path.as_deref().map(cell_text).unwrap_or_else(|| "-".into()),
                r.parent.as_deref().unwrap_or("-"),
                r.nested,
                next_cell(r),
                r.last_active.as_deref().unwrap_or("-"),
            );
        }
    }

    // Backfill nudge: never silently lose un-homed projects (Req 3). Only in the
    // workspace-scoped (Current) view — `--all`/`--unscoped` already surface them.
    // "Unscoped" is `scope::home`'s: no path, or a path inside no registered workspace.
    if matches!(project_scope, scope::ProjectScope::Current) && unscoped_count > 0 {
        println!("\n{}", unscoped_advice(unscoped_count));
    }
    Ok(())
}

/// The list's backfill advice (F25e). Every command it names exists: `project update --path` since 0.16.0.
pub fn unscoped_advice(n: usize) -> String {
    format!(
        "{n} project(s) have no folder inside a registered workspace (unscoped): `base project list --unscoped` lists them. \
         Set a project's folder with `base project update <slug> --path <dir>`; `base project paths --suggest` proposes one for each."
    )
}

/// `--json` list: valid JSON array of the in-scope project records on stdout, nothing else.
pub fn list_json(cwd: &Path, config: &BaseConfig, project_scope: scope::ProjectScope) -> Result<()> {
    let (records, _unscoped) = list_data(cwd, config, &project_scope)?;
    println!("{}", serde_json::to_string_pretty(&records)?);
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

/// Fetch one project as a typed record. `None` when no node matches the slug.
pub fn get_data(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<Option<ProjectRecord>> {
    let iri = crud::build_iri(ns, "project", slug);
    let p = &ns.prefix;
    let optionals = record_optionals(p, &format!("<{iri}>"));
    let sparql = format!(
        "SELECT ?g ?name ?status {RECORD_FIELDS} WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> a {p}:Project ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status .\n\
{optionals}\
           }}\n\
         }}\n\
         LIMIT 1"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    if let QueryResults::Solutions(solutions) = results
        && let Some(row) = solutions.filter_map(|r| r.ok()).next()
    {
        let cell = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
        let roots = PathRoots::new(cwd, ns);
        let path = cell("path").map(|raw| roots.stored(&raw, graph_cell(&row, "g").as_deref()));
        return Ok(Some(record_from(slug.to_string(), path, &cell, chrono::Local::now())));
    }
    Ok(None)
}

pub fn get(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    match get_data(cwd, ns, slug)? {
        None => {
            eprintln!("Project '{slug}' not found.");
            Ok(())
        }
        Some(r) => {
            println!("Project: {}", r.id);
            println!("  name: {}", r.name);
            println!("  status: {}", r.status);
            if let Some(v) = &r.priority { println!("  priority: {v}"); }
            if let Some(v) = &r.path { println!("  path: {v}"); }
            if let Some(v) = &r.stage { println!("  stage: {v}"); }
            if let Some(v) = &r.parent { println!("  parent: {v}"); }
            println!("  nested: {}", r.nested);
            if let Some(v) = &r.blocked_by { println!("  blockedBy: {v}"); }
            if let Some(v) = &r.next_action { println!("  nextAction: {v} {}", age_label(r.next_action_age_days)); }
            if let Some(v) = &r.next_action_at { println!("  nextActionAt: {v}"); }
            if let Some(v) = &r.created { println!("  created: {v}"); }
            if let Some(v) = &r.updated { println!("  updated: {v}"); }
            if let Some(v) = &r.last_active { println!("  lastActive: {v}"); }
            Ok(())
        }
    }
}

/// `--json` get: one JSON document (record or `null`) on stdout.
pub fn get_json(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let rec = get_data(cwd, ns, slug)?;
    println!("{}", serde_json::to_string_pretty(&rec)?);
    Ok(())
}

/// Preview of a project delete: the subject set the cascade would remove, mirroring
/// exactly what `project move` traverses (domain node + tasks + milestones + decisions
/// + rules + linked notes + the project node).
pub struct ProjectDeletePlan {
    /// True when a project node with this slug actually exists in the workspace graph.
    pub exists: bool,
    /// Every subject IRI that would be removed (includes the project + domain scaffold).
    pub subjects: std::collections::HashSet<String>,
    /// Subjects beyond the project + domain scaffold (tasks/milestones/decisions/rules/notes).
    pub children: usize,
}

/// Resolve what `delete` would remove. Reuses the `graph_move` domain selector so the
/// delete traversal and the move traversal can never drift apart.
pub fn delete_plan(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<ProjectDeletePlan> {
    use crate::graph_move::{self, Selector};
    let base_dir = crate::config::find_workspace_base(cwd)
        .context("no .base/ directory found")?;
    let source_path = base_dir.join("graph.nq");
    let source_graph = crud::workspace_graph_iri(ns, &crud::workspace_slug(cwd));

    let subjects = if source_path.exists() {
        graph_move::resolve_selector(&source_path, &Selector::Domain(slug.to_string()), &source_graph, ns)?
    } else {
        std::collections::HashSet::new()
    };

    let project_iri = crud::build_iri(ns, "project", slug);
    let domain_iri = crud::build_iri(ns, "domain", slug);
    let exists = subjects.contains(&project_iri);
    let children = subjects
        .iter()
        .filter(|s| *s != &project_iri && *s != &domain_iri)
        .count();

    Ok(ProjectDeletePlan { exists, subjects, children })
}

/// Delete a project. Refuses a non-empty project (any child node) unless `force`, in
/// which case it CASCADE-deletes everything the `project move` traversal covers —
/// node + domain + tasks + milestones + decisions + rules + linked notes. Each subject
/// is removed atomically, backup-first, through the shared store primitive. Returns the
/// number of subjects removed.
pub fn delete(cwd: &Path, ns: &NamespaceConfig, slug: &str, force: bool) -> Result<usize> {
    let plan = delete_plan(cwd, ns, slug)?;
    if !plan.exists {
        anyhow::bail!("project '{slug}' not found in this workspace graph");
    }
    if plan.children > 0 && !force {
        anyhow::bail!(
            "project '{slug}' has {} child node(s) (tasks/milestones/decisions/rules). \
             Re-run with --force to cascade-delete them.",
            plan.children
        );
    }

    for subject in &plan.subjects {
        let del = format!(
            "DELETE WHERE {{ GRAPH ?g {{ <{subject}> ?p ?o }} }};\n\
             DELETE WHERE {{ GRAPH ?g {{ ?s ?p <{subject}> }} }}"
        );
        crud::load_and_mutate(cwd, ns, &del)?;
    }
    Ok(plan.subjects.len())
}

pub fn update(
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    status: Option<&str>,
    blocked_by: Option<&str>,
    next_action: Option<&str>,
) -> Result<()> {
    let change = ProjectUpdate { status, blocked_by, next_action, ..ProjectUpdate::default() };
    apply_update(cwd, ns, slug, &change).map(|_| ())
}

/// A parent link to write (D13): set to a project, or removed.
#[derive(Debug, Clone, PartialEq)]
pub enum ParentChange {
    Set(String),
    Clear,
}

/// The fields one `base project update` writes. `None` leaves a field as it is.
#[derive(Debug, Default)]
pub struct ProjectUpdate<'a> {
    pub status: Option<&'a str>,
    pub blocked_by: Option<&'a str>,
    pub next_action: Option<&'a str>,
    /// As typed: resolved against the workspace root before it is stored (F25b).
    pub path: Option<&'a str>,
    pub parent: Option<ParentChange>,
    pub nested: Option<bool>,
}

/// What an update did beyond the fields it was given.
#[derive(Debug, Default)]
pub struct UpdateOutcome {
    /// Set when the path changed: old and new, and whether the project's domain trigger moved with it.
    pub repath: Option<RepathResult>,
    /// Allowed but worth saying (F25d): `nested = true` with no parent.
    pub warnings: Vec<String>,
}

/// A project update refused before anything was written (F25c): no such parent, or a loop. The CLI prints it as
/// `Error: <text>`.
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// A SELECT over a store already loaded, with base's prefixes.
fn select(store: &oxigraph::store::Store, ns: &NamespaceConfig, sparql: &str) -> Result<Vec<oxigraph::sparql::QuerySolution>> {
    Ok(match crate::store::query(store, &format!("{}
{sparql}", crud::prefixes(ns)))? {
        QueryResults::Solutions(sols) => sols.filter_map(|r| r.ok()).collect(),
        _ => Vec::new(),
    })
}

/// The named graph holding `<project/slug> a Project` in the workspace file: this workspace's own graph when it
/// holds it, else the first other one that does. A project filed under another workspace's graph in this file (a
/// `project move` leftover, a PAUL project homed elsewhere) is written where it lives; writing to this
/// workspace's graph only, as `update` did before 0.16.0, matched nothing and reported success.
pub fn project_graph(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<Option<String>> {
    project_graph_in(&crud::load_workspace_graph(cwd)?, cwd, ns, slug)
}

fn project_graph_in(store: &oxigraph::store::Store, cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<Option<String>> {
    let iri = crud::build_iri(ns, "project", slug);
    let p = &ns.prefix;
    let rows = select(store, ns, &format!("SELECT DISTINCT ?g WHERE {{ GRAPH ?g {{ <{iri}> a {p}:Project }} }} ORDER BY ?g"))?;
    let graphs: Vec<String> = rows.iter().filter_map(|s| graph_cell(s, "g")).collect();
    let own = crud::workspace_graph_iri(ns, &crud::workspace_slug(cwd));
    Ok(graphs.iter().find(|g| **g == own).or(graphs.first()).cloned())
}

/// Every parent link in the workspace file: child slug → parent slug.
fn parent_map(store: &oxigraph::store::Store, ns: &NamespaceConfig) -> Result<std::collections::HashMap<String, String>> {
    let p = &ns.prefix;
    let mut map = std::collections::HashMap::new();
    for s in select(store, ns, &format!("SELECT ?c ?parent WHERE {{ GRAPH ?g {{ ?c {p}:parentProject ?parent }} }}"))? {
        let get = |k: &str| s.get(k).map(|t| crud::slug_of(&crud::term_display(t.into())));
        if let (Some(c), Some(parent)) = (get("c"), get("parent")) {
            map.insert(c, parent);
        }
    }
    Ok(map)
}

/// The slug `parent` names, or [`Refused`] when it names no registered project or the link would close a loop
/// (F25c, D13). Accepts a slug or a display name, as every project command does.
pub fn check_parent(cwd: &Path, ns: &NamespaceConfig, slug: &str, parent: &str) -> Result<String> {
    check_parent_in(&crud::load_workspace_graph(cwd)?, ns, slug, parent)
}

fn check_parent_in(store: &oxigraph::store::Store, ns: &NamespaceConfig, slug: &str, parent: &str) -> Result<String> {
    let parent_slug = match crud::resolve_slug_in(store, ns, "project", parent) {
        Ok(s) => s,
        Err(_) => {
            return Err(Refused(format!(
                "no project '{parent}': a parent must be a registered project (`base project list --all`)"
            ))
            .into());
        }
    };
    if parent_slug == slug {
        return Err(Refused(format!("loop: {slug} cannot be its own parent")).into());
    }
    // Walk up from the new parent. Reaching `slug` means the link closes a loop; the message names every link
    // already on the way, so the one to remove is in front of the operator.
    let parents = parent_map(store, ns)?;
    let mut links: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cur = parent_slug.clone();
    while let Some(up) = parents.get(&cur) {
        links.push(format!("{cur} already has parent {up}"));
        if *up == slug {
            return Err(Refused(format!("loop: {}", links.join(", "))).into());
        }
        if !seen.insert(cur.clone()) {
            break; // a loop above that does not run through `slug`; not this link's to refuse
        }
        cur = up.clone();
    }
    Ok(parent_slug)
}

/// What an update needs to know before it writes, from the project's own graph: its name (the domain's name), its
/// path exactly as stored (to match the domain trigger written from it), its parent, and `nested`.
struct Before {
    name: String,
    path: Option<String>,
    parent: Option<String>,
    nested: bool,
}

fn before_in(store: &oxigraph::store::Store, ns: &NamespaceConfig, iri: &str, graph: &str) -> Result<Option<Before>> {
    let p = &ns.prefix;
    let rows = select(
        store,
        ns,
        &format!(
            "SELECT ?name ?path ?parent ?nested WHERE {{ GRAPH <{graph}> {{ <{iri}> {p}:name ?name .              OPTIONAL {{ <{iri}> {p}:path ?path }} OPTIONAL {{ <{iri}> {p}:parentProject ?parent }}              OPTIONAL {{ <{iri}> {p}:nested ?nested }} }} }} LIMIT 1"
        ),
    )?;
    Ok(rows.first().map(|r| {
        let cell = |k: &str| r.get(k).map(|t| crud::term_display(t.into()));
        Before {
            name: cell("name").unwrap_or_default(),
            path: cell("path"),
            parent: cell("parent").map(|v| crud::slug_of(&v)),
            nested: cell("nested").is_some_and(|v| v == "true"),
        }
    }))
}

/// Write one project's changed fields in one graph write, then move its domain trigger when the path moved.
/// A parent is checked (and refused) before anything is written. Every check reads one load of the store.
pub fn apply_update(cwd: &Path, ns: &NamespaceConfig, slug: &str, change: &ProjectUpdate) -> Result<UpdateOutcome> {
    let iri = crud::build_iri(ns, "project", slug);
    let store = crud::load_workspace_graph(cwd)?;
    let Some(graph) = project_graph_in(&store, cwd, ns, slug)? else {
        anyhow::bail!("project '{slug}' not found in this workspace graph");
    };
    let p = &ns.prefix;
    let now = crud::now_iso();
    let lit = |s: &str| format!("\"{}\"", crud::escape_sparql_literal(s));

    let parent = match &change.parent {
        Some(ParentChange::Set(want)) => Some(ParentChange::Set(check_parent_in(&store, ns, slug, want)?)),
        other => other.clone(),
    };
    let new_path = match change.path {
        Some(raw) => Some(
            PathRoots::new(cwd, ns)
                .from_cli(raw)
                .ok_or_else(|| Refused(format!("--path '{raw}' names no folder")))?,
        ),
        None => None,
    };
    let Some(Before { name, path: old_path, parent: old_parent, nested: old_nested }) =
        before_in(&store, ns, &iri, &graph)?
    else {
        anyhow::bail!("project '{slug}' not found in this workspace graph");
    };
    drop(store);

    let mut updates = Vec::new();
    let mut set = |pred: &str, value: String| {
        updates.push(crud::field_update(&graph, &iri, &format!("{p}:{pred}"), &value));
    };
    if let Some(s) = change.status {
        set("status", lit(s));
    }
    if let Some(b) = change.blocked_by {
        set("blockedBy", lit(b));
    }
    if let Some(n) = change.next_action {
        set("nextAction", lit(n));
        // F23a: a next step carries the time it was written.
        set("nextActionAt", format!("\"{now}\"^^xsd:dateTime"));
    }
    if let Some(np) = &new_path {
        set("path", lit(np));
    }
    if let Some(ParentChange::Set(ps)) = &parent {
        set("parentProject", format!("<{}>", crud::build_iri(ns, "project", ps)));
    }
    if let Some(n) = change.nested {
        set("nested", format!("\"{n}\"^^xsd:boolean"));
    }
    set("updatedAt", format!("\"{now}\"^^xsd:dateTime"));
    set("lastActive", format!("\"{now}\"^^xsd:dateTime"));
    if parent == Some(ParentChange::Clear) {
        updates.push(format!("DELETE WHERE {{ GRAPH ?gg {{ <{iri}> {p}:parentProject ?old }} }}"));
    }
    crud::load_and_mutate(cwd, ns, &updates.join(" ;\n"))?;

    let mut outcome = UpdateOutcome::default();
    if let Some(np) = new_path {
        // The domain trigger follows the folder, as `project repath` always did.
        let domain_changed =
            crate::domain::repath_trigger(cwd, &name, old_path.as_deref(), &np).unwrap_or(false);
        outcome.repath = Some(RepathResult { name, old_path, new_path: np, domain_changed });
    }
    let has_parent = match &parent {
        Some(ParentChange::Set(_)) => true,
        Some(ParentChange::Clear) => false,
        None => old_parent.is_some(),
    };
    if change.nested.unwrap_or(old_nested) && !has_parent {
        outcome.warnings.push(format!(
            "{slug} has no parent, so nested = true does nothing until one is set: \
             base project update {slug} --parent <slug>"
        ));
    }
    Ok(outcome)
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
#[derive(Debug)]
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
    // One writer for a project's folder: `project update --path` and this command store the same absolute form
    // (F25b) and move the domain trigger the same way.
    let change = ProjectUpdate { path: Some(new_path), ..ProjectUpdate::default() };
    apply_update(cwd, ns, slug, &change)?
        .repath
        .ok_or_else(|| anyhow::anyhow!("project '{slug}': the path was not written"))
}

/// Re-home a project end-to-end to another workspace graph: the project node + its
/// domain + tasks/milestones + decisions/rules + edge-attached notes + handoffs, all
/// rewritten from `graph/ws/<current>` to `graph/ws/<to>`. Built on the
/// [`crate::graph_move`] primitive (snapshot-both → write → health-gate → rollback).
///
/// AST is NEVER moved (it rebuilds from the code): the underlying move runs with
/// `no_ast = true`, so `codemap/<slug>` + `code#` entities stay put and the
/// destination map is regenerated separately (`base sync --ast`). The current
/// workspace (derived from `cwd`) is the source; `to` is the destination workspace
/// name resolved through the `[[workspace]]` registry.
pub fn move_project(
    cwd: &Path,
    config: &BaseConfig,
    slug: &str,
    to: &str,
    dry_run: bool,
) -> Result<crate::graph_move::MoveReport> {
    use crate::graph_move::{self, Selector};
    let ns = &config.namespace;
    let from_name = crud::workspace_slug(cwd);
    let spec = graph_move::spec_from_names(&from_name, to, &config.workspace, ns, true)?;

    // Collect every subject belonging to the project. Domain selection already covers
    // the domain node, slug-convention tasks/decisions/rules/milestones, and anything
    // that links to the domain (notes via relatedTo, the project via hasDomain). Add
    // handoffs, which key off the slug but carry no domain edge.
    let mut subjects =
        graph_move::resolve_selector(&spec.source_path, &Selector::Domain(slug.to_string()), &spec.source_graph, ns)?;
    subjects.extend(graph_move::resolve_selector(
        &spec.source_path,
        &Selector::Prefix(format!("{}handoff/{}", ns.uri, slug)),
        &spec.source_graph,
        ns,
    )?);

    let project_iri = crud::build_iri(ns, "project", slug);
    if !subjects.contains(&project_iri) {
        anyhow::bail!("project '{slug}' not found in workspace '{from_name}' graph");
    }

    graph_move::graph_move_subjects(&spec, &subjects, ns, dry_run)
}
