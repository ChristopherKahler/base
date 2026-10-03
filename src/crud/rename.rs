//! `base project rename <old> <new>` (BO-24): a project and its same-named domain renamed together, in every tier,
//! with the old name kept as an alias.
//!
//! What gets a new ID (R3) is what base builds from the name: `project/<old>` and `domain/<old>`, every
//! `<kind>/<old>.<rest>` (decisions, tasks and milestones key themselves `{domain}.{x}` and `{project}.{x}`), and
//! every `rule/<old>/<rest>` (synced and `rule add` rules). Every edge to one of those IDs follows it, and a
//! handoff's `ops:project`, which names its project by slug, follows too. Left as written: document records (named
//! from file paths), notes and entities (named from their own text, which may start with the old word), prose,
//! prompt keywords (R5) and historic handoff slugs. Those are content, not IDs.
//!
//! Safety (R6): one run at a time (a lock file, refused rather than queued), each graph loaded and written inside
//! its own graph lock, a `*.BAK-<date>-pre-rename-<old>` copy of every file before it is written, and each file
//! written once. A failure part way puts back the files already written; if that fails too, the error names each
//! file and the backup to restore it from.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oxigraph::model::{GraphName, Literal, NamedNode, Quad, Subject, Term};

use crate::changelog::Change;
use crate::config::NamespaceConfig;
use crate::crud::{self, project::Refused};
use crate::store::{GraphDelta, LockedGraph};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// One tier's graph: what the rename changes in it.
pub struct GraphPlan {
    /// `workspace graph` or `global graph`.
    pub tier: &'static str,
    pub file: PathBuf,
    /// Records given a new ID, by kind (`decision`, `task`, ...): how many distinct records.
    pub rekeyed: BTreeMap<String, usize>,
    /// Edges from records that keep their ID to one that moves.
    pub links: usize,
    /// Handoff `project` (and same-valued `name`) fields rewritten.
    pub handoff_fields: usize,
    /// Whether the project's own record is in this graph.
    pub holds_project: bool,
    /// The IDs records move to.
    targets: BTreeSet<String>,
    /// The quads removed and added, in that order.
    delta: GraphDelta,
}

/// One domains.toml the rename edits.
pub struct TomlPlan {
    pub tier: &'static str,
    pub file: PathBuf,
    pub declared_rules: usize,
    before: String,
    after: String,
}

/// Everything `project rename` would do, worked out before anything is written.
pub struct RenamePlan {
    pub old: String,
    pub new: String,
    pub graphs: Vec<GraphPlan>,
    pub tomls: Vec<TomlPlan>,
    /// Each file the rename writes, and the backup it takes first.
    pub backups: Vec<(PathBuf, PathBuf)>,
    held: Vec<Held>,
    /// `Some` only for a plan made to be written: a preview takes no lock.
    rename_lock: Option<crate::store::GraphLockGuard>,
}

/// A tier's graph as the plan read it: inside its graph lock for a rename that will write, or plainly read for a
/// preview, which writes nothing and must not make other writers wait.
enum Held {
    Locked(LockedGraph),
    Read(oxigraph::store::Store),
}

impl Held {
    fn store(&self) -> &oxigraph::store::Store {
        match self {
            Held::Locked(g) => g.store(),
            Held::Read(s) => s,
        }
    }
}

impl RenamePlan {
    /// Rule records that move, over every tier, plus the rules the domain declares in its domains.toml files.
    pub fn rules(&self) -> usize {
        self.tomls.iter().map(|t| t.declared_rules).sum::<usize>()
            + self.graphs.iter().map(|g| g.rekeyed.get("rule").copied().unwrap_or(0)).sum::<usize>()
    }
}

/// The name an old project name goes by now, from either tier's domains.toml or graph; `None` when `old` is no old
/// name. For `project add`, which must not create a project an alias would shadow.
pub fn renamed_to(cwd: &Path, ns: &NamespaceConfig, old: &str) -> Option<String> {
    if let Some(d) = crate::domain::renamed_from(&crate::domain::load_domains(cwd), old) {
        return Some(d.name.clone());
    }
    crate::store::load_merged(cwd).and_then(|store| crud::alias::renamed_to(&store, ns, old))
}

/// The two tiers a rename walks: the global tier, then the workspace `cwd` sits in (when that is not the global
/// tier itself). Each is `(label, graph.nq, domains.toml)`.
fn tiers(cwd: &Path) -> Vec<(&'static str, PathBuf, PathBuf)> {
    let mut out = Vec::new();
    let global = crate::config::global_base_dir();
    if let Some(g) = &global {
        out.push(("global graph", g.join("graph.nq"), crate::domain::tier::global_domains_toml()));
    }
    if let Some(ws) = crate::config::find_workspace_base(cwd) {
        let same = global.as_ref().is_some_and(|g| crate::scope::canonical_str(&g.display().to_string()) == crate::scope::canonical_str(&ws.display().to_string()));
        if !same {
            out.push(("workspace graph", ws.join("graph.nq"), ws.join("domains.toml")));
        }
    }
    out
}

/// Where the rename's own lock lives: beside the global tier's graph when that tier exists, else beside the
/// workspace's. Never a folder the rename would have to create.
fn rename_lock_file(cwd: &Path) -> Option<PathBuf> {
    crate::config::global_base_dir()
        .filter(|g| g.is_dir())
        .or_else(|| crate::config::find_workspace_base(cwd).filter(|w| w.is_dir()))
        .map(|b| b.join("project-rename.lock"))
}

/// Maps an old ID to its new one.
struct Names<'a> {
    uri: &'a str,
    old: &'a str,
    new: &'a str,
}

impl Names<'_> {
    /// The ID `iri` moves to and its kind, or `None` when it keeps its ID.
    fn rekey(&self, iri: &str) -> Option<(String, String)> {
        let rest = iri.strip_prefix(self.uri)?;
        let (kind, slug) = rest.split_once('/')?;
        let (uri, new) = (self.uri, self.new);
        let to = match kind {
            "project" | "domain" => (slug == self.old).then(|| format!("{uri}{kind}/{new}")),
            // Named from file paths, or from what they record: never from the project's name (see the module doc).
            "document" | "handoff" | "graph" | "workspace" | "codemap" => None,
            "rule" => slug.strip_prefix(self.old)?.strip_prefix('/').map(|tail| format!("{uri}rule/{new}/{tail}")),
            _ => slug.strip_prefix(self.old)?.strip_prefix('.').map(|tail| format!("{uri}{kind}/{new}.{tail}")),
        }?;
        Some((to, kind.to_string()))
    }
}

fn named(iri: &str) -> Result<NamedNode> {
    NamedNode::new(iri).with_context(|| format!("building the IRI {iri}"))
}

/// What the rename changes in one store, without changing it.
fn plan_graph(store: &oxigraph::store::Store, ns: &NamespaceConfig, names: &Names) -> Result<GraphPlan> {
    let p = |local: &str| format!("{}{local}", ns.uri);
    let (pred_name, pred_project, pred_alias) = (p("name"), p("project"), p("alias"));
    let handoff_type = p("Handoff");
    let renamed = [crud::build_iri(ns, "project", names.old), crud::build_iri(ns, "domain", names.old)];

    let handoffs: HashSet<String> = store
        .quads_for_pattern(None, Some(named(RDF_TYPE)?.as_ref()), Some(named(&handoff_type)?.as_ref().into()), None)
        .filter_map(|q| q.ok())
        .filter_map(|q| match q.subject {
            Subject::NamedNode(n) => Some(n.into_string()),
            _ => None,
        })
        .collect();

    let mut delta = GraphDelta::default();
    let mut rekeyed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let (mut links, mut handoff_fields, mut holds_project) = (0usize, 0usize, false);
    let mut targets: BTreeSet<String> = BTreeSet::new();
    // The graphs the project's and the domain's type quads sit in: the alias goes beside each.
    let mut alias_homes: BTreeSet<(String, String)> = BTreeSet::new();

    for q in store.iter() {
        let q = q?;
        let s_iri = match &q.subject {
            Subject::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        };
        let s_to = s_iri.as_deref().and_then(|s| names.rekey(s));
        let o_to = match &q.object {
            Term::NamedNode(n) => names.rekey(n.as_str()),
            _ => None,
        };
        let pred = q.predicate.as_str();

        // An alias equal to the new name goes: the record is called that now (a rename back).
        if pred == pred_alias
            && s_iri.as_ref().is_some_and(|s| renamed.contains(s))
            && matches!(&q.object, Term::Literal(l) if l.value() == names.new)
        {
            delta.removed.push(q);
            continue;
        }
        if pred == RDF_TYPE
            && let (Some(s), Some((to, _))) = (&s_iri, &s_to)
            && renamed.contains(s)
        {
            alias_homes.insert((to.clone(), graph_iri(&q.graph_name)));
            holds_project |= *s == renamed[0];
        }

        // Literals that name the project by slug: the project's and the domain's own name, and a handoff's
        // `project` (and its `name`, which `handoff create` copies from it).
        let lit_to = match &q.object {
            Term::Literal(l) => {
                let own_name = pred == pred_name && s_iri.as_ref().is_some_and(|s| renamed.contains(s));
                let handoff_field = (pred == pred_project
                    || (pred == pred_name && s_iri.as_ref().is_some_and(|s| handoffs.contains(s))))
                    && crud::slugify(l.value()) == names.old;
                if own_name || handoff_field {
                    if handoff_field && !own_name {
                        handoff_fields += 1;
                    }
                    Some(Literal::new_simple_literal(names.new))
                } else {
                    None
                }
            }
            _ => None,
        };

        if s_to.is_none() && o_to.is_none() && lit_to.is_none() {
            continue;
        }
        if let (Some(s), Some((to, kind))) = (&s_iri, &s_to) {
            rekeyed.entry(kind.clone()).or_default().insert(s.clone());
            targets.insert(to.clone());
        } else if o_to.is_some() {
            links += 1;
        }
        let subject: Subject = match &s_to {
            Some((to, _)) => named(to)?.into(),
            None => q.subject.clone(),
        };
        let object: Term = match (&o_to, lit_to) {
            (Some((to, _)), _) => named(to)?.into(),
            (None, Some(l)) => l.into(),
            (None, None) => q.object.clone(),
        };
        delta.added.push(Quad::new(subject, q.predicate.clone(), object, q.graph_name.clone()));
        delta.removed.push(q);
    }

    for (to, graph) in alias_homes {
        let g = if graph.is_empty() { GraphName::DefaultGraph } else { named(&graph)?.into() };
        delta.added.push(Quad::new(named(&to)?, named(&pred_alias)?, Literal::new_simple_literal(names.old), g));
    }
    // A moved quad the store already holds (a note linked to both spellings, a handoff whose project was written
    // twice) is no addition: kept out, so putting the rename back never deletes a quad that was there before it.
    let removed: HashSet<&Quad> = delta.removed.iter().collect();
    let mut seen: HashSet<Quad> = HashSet::new();
    let mut added = Vec::with_capacity(delta.added.len());
    for q in std::mem::take(&mut delta.added) {
        if (removed.contains(&q) || !store.contains(&q)?) && seen.insert(q.clone()) {
            added.push(q);
        }
    }
    delta.added = added;

    Ok(GraphPlan {
        tier: "",
        file: PathBuf::new(),
        rekeyed: rekeyed.into_iter().map(|(k, v)| (k, v.len())).collect(),
        links,
        handoff_fields,
        holds_project,
        targets,
        delta,
    })
}

fn graph_iri(g: &GraphName) -> String {
    match g {
        GraphName::NamedNode(n) => n.as_str().to_string(),
        _ => String::new(),
    }
}

/// Every ID the plan moves records to that is already a record in `store`: renaming onto it would merge two records.
fn collisions(store: &oxigraph::store::Store, plan: &GraphPlan) -> Result<Vec<String>> {
    let mut hit = Vec::new();
    for t in &plan.targets {
        let node = named(t)?;
        if store.quads_for_pattern(Some(node.as_ref().into()), None, None, None).next().is_some() {
            hit.push(t.clone());
        }
    }
    Ok(hit)
}

/// A backup path beside `file` that does not exist yet: `<file>.BAK-<date>-pre-rename-<old>`, with the time added
/// when a rename of the same name already left one today.
fn backup_path(file: &Path, old: &str) -> PathBuf {
    let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let now = chrono::Local::now();
    let first = file.with_file_name(format!("{name}.BAK-{}-pre-rename-{old}", now.format("%Y%m%d")));
    if !first.exists() {
        return first;
    }
    let stamped = now.format("%Y%m%d-%H%M%S").to_string();
    let mut n = 1;
    loop {
        let suffix = if n == 1 { String::new() } else { format!("-{n}") };
        let p = file.with_file_name(format!("{name}.BAK-{stamped}{suffix}-pre-rename-{old}"));
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

/// Work out the rename, refusing it (R7) before anything is written: an unknown old name, a new name that is not a
/// slug or is already a project, a domain or an old name of another one, an ID it would merge into an existing
/// record, or (with `write`) another rename running. A plan made to be written holds the rename lock and every graph
/// lock until it is dropped or applied, so what it shows is what `apply` writes; a preview reads without locking.
pub fn plan(cwd: &Path, ns: &NamespaceConfig, old_input: &str, new: &str, write: bool) -> Result<RenamePlan> {
    let refuse = |msg: String| -> anyhow::Error { Refused(msg).into() };
    if new.is_empty() || crud::slugify(new) != new {
        let hint = crud::slugify(new);
        return Err(refuse(format!(
            "'{new}' is not a valid name: use lowercase letters, digits and dashes{}",
            if hint.is_empty() { String::new() } else { format!(" (for example '{hint}')") }
        )));
    }

    let Some(lock_file) = rename_lock_file(cwd) else {
        anyhow::bail!("no base tier here: run it from a workspace, or set up the global tier with `base scaffold`");
    };
    let rename_lock = if write {
        let Some(guard) = crate::store::try_lock(&lock_file)? else {
            let who = crate::store::lock_holder(&lock_file).map(|p| format!(" (pid {p})")).unwrap_or_default();
            return Err(refuse(format!(
                "another `base project rename` is running{who}; nothing was written. Run this again when it is done."
            )));
        };
        Some(guard)
    } else {
        None
    };

    let mut locked: Vec<(&'static str, Held, PathBuf)> = Vec::new();
    for (tier, graph, _) in tiers(cwd) {
        if graph.exists() {
            let held = if write {
                Held::Locked(crate::store::lock_and_load_graph(&graph)?)
            } else {
                Held::Read(crate::store::load_graph(&graph)?)
            };
            locked.push((tier, held, graph));
        }
    }

    // The old name: a project's slug or its display name, in either tier. Never an alias: an old name is not a
    // project to rename (say which one it is now instead).
    let mut old: Option<String> = None;
    for (_, g, _) in &locked {
        let store = g.store();
        let as_slug = crud::build_iri(ns, "project", old_input);
        let typed = |iri: &str| -> bool {
            let q = format!("{}\nASK WHERE {{ GRAPH ?g {{ <{iri}> a {}:Project }} }}", crud::prefixes(ns), ns.prefix);
            matches!(crate::store::query(store, &q), Ok(oxigraph::sparql::QueryResults::Boolean(true)))
        };
        if !old_input.contains(' ') && typed(&as_slug) {
            old = Some(old_input.to_string());
            break;
        }
        let by_name = format!(
            "{}\nSELECT ?iri WHERE {{ GRAPH ?g {{ ?iri a {p}:Project ; {p}:name ?n . FILTER(LCASE(?n) = LCASE(\"{}\")) }} }} LIMIT 1",
            crud::prefixes(ns),
            crud::escape_sparql_literal(old_input),
            p = ns.prefix
        );
        if let Ok(oxigraph::sparql::QueryResults::Solutions(mut rows)) = crate::store::query(store, &by_name)
            && let Some(Ok(row)) = rows.next()
            && let Some(Term::NamedNode(n)) = row.get("iri")
        {
            old = Some(crud::slug_of(n.as_str()));
            break;
        }
    }
    let Some(old) = old else {
        for (_, g, _) in &locked {
            if let Some(now) = crud::alias::renamed_to(g.store(), ns, &crud::slugify(old_input)) {
                return Err(refuse(format!("'{old_input}' is an old name of project '{now}'; rename '{now}' instead")));
            }
        }
        return Err(refuse(format!("no project '{old_input}' (`base project list --all` lists them)")));
    };
    if old == new {
        return Err(refuse(format!("'{old}' is already called '{new}'; nothing to rename")));
    }

    // The new name must be free: no project or domain by it in either tier or any domains.toml, and not an old name
    // of anything but this project (a rename back is allowed; its alias is dropped). Projects are named first.
    for (kind, class) in [("project", "Project"), ("domain", "Domain")] {
        let iri = crud::build_iri(ns, kind, new);
        let q = format!("{}\nASK WHERE {{ GRAPH ?g {{ <{iri}> a {}:{class} }} }}", crud::prefixes(ns), ns.prefix);
        if locked.iter().any(|(_, g, _)| matches!(crate::store::query(g.store(), &q), Ok(oxigraph::sparql::QueryResults::Boolean(true)))) {
            return Err(refuse(format!("'{new}' is already a {kind}")));
        }
    }
    // By the slug a domain's records are keyed by: a domain written `Vintryx` and never synced is `domain/vintryx`.
    let domains = crate::domain::load_domains(cwd);
    if domains.iter().any(|d| crud::slugify(&d.name) == new) {
        return Err(refuse(format!("'{new}' is already a domain")));
    }
    if let Some(d) = crate::domain::renamed_from(&domains, new)
        && d.name != old
    {
        return Err(refuse(format!("'{new}' is an old name of domain '{}'", d.name)));
    }
    for (_, g, _) in &locked {
        if let Some(now) = crud::alias::renamed_to(g.store(), ns, new)
            && now != old
        {
            return Err(refuse(format!("'{new}' is an old name of '{now}'")));
        }
    }

    let names = Names { uri: &ns.uri, old: &old, new };
    let mut graphs = Vec::new();
    for (tier, g, file) in &locked {
        let mut gp = plan_graph(g.store(), ns, &names)?;
        let hit = collisions(g.store(), &gp)?;
        if !hit.is_empty() {
            let more = hit.len().saturating_sub(5);
            return Err(refuse(format!(
                "renaming '{old}' to '{new}' would merge records that already exist in the {tier}: {}{}",
                hit.iter().take(5).map(|h| h.strip_prefix(ns.uri.as_str()).unwrap_or(h)).collect::<Vec<_>>().join(", "),
                if more > 0 { format!(" and {more} more") } else { String::new() }
            )));
        }
        gp.tier = tier;
        gp.file = file.clone();
        graphs.push(gp);
    }

    let mut tomls = Vec::new();
    for (tier, _, toml) in tiers(cwd) {
        let Ok(before) = std::fs::read_to_string(&toml) else { continue };
        let renamed = crate::domain::rename_in_text(&before, &old, new)
            .with_context(|| format!("renaming the domain in {}", toml.display()))?;
        if let Some(r) = renamed {
            tomls.push(TomlPlan {
                tier: if tier == "global graph" { "global" } else { "workspace" },
                file: toml,
                declared_rules: r.declared_rules,
                before,
                after: r.text,
            });
        }
    }

    let mut backups = Vec::new();
    for g in graphs.iter().filter(|g| !g.delta.is_empty()) {
        backups.push((g.file.clone(), backup_path(&g.file, &old)));
    }
    for t in &tomls {
        backups.push((t.file.clone(), backup_path(&t.file, &old)));
    }

    Ok(RenamePlan {
        old,
        new: new.to_string(),
        graphs,
        tomls,
        backups,
        held: locked.into_iter().map(|(_, g, _)| g).collect(),
        rename_lock,
    })
}

/// A file the apply has written, so a later failure can put it back.
enum Written {
    Graph(usize),
    Toml(usize),
}

/// Write the plan: back up every file it changes, then write each graph once (through its graph lock) and each
/// domains.toml once. On a failure the files already written are put back; the error says which, or, when putting
/// one back fails too, names the backup to restore it from.
pub fn apply(plan: &RenamePlan) -> Result<()> {
    let locked: Vec<&LockedGraph> = plan
        .held
        .iter()
        .filter_map(|h| match h {
            Held::Locked(g) => Some(g),
            Held::Read(_) => None,
        })
        .collect();
    if plan.rename_lock.is_none() || locked.len() != plan.held.len() {
        anyhow::bail!("this plan was made as a preview: it holds no locks, so it is never written");
    }
    for (file, backup) in &plan.backups {
        std::fs::copy(file, backup).with_context(|| format!("backing up {} to {}", file.display(), backup.display()))?;
    }

    let mut written: Vec<Written> = Vec::new();
    let mut failure: Option<(PathBuf, anyhow::Error)> = None;
    for (i, g) in plan.graphs.iter().enumerate() {
        if g.delta.is_empty() {
            continue;
        }
        let locked = locked[i];
        let store = locked.store();
        let step = (|| -> Result<()> {
            for q in &g.delta.removed {
                store.remove(q)?;
            }
            for q in &g.delta.added {
                store.insert(q)?;
            }
            locked.write(Change::OpWithDelta("project.rename", &g.delta.to_ops()))
        })();
        match step {
            Ok(()) => written.push(Written::Graph(i)),
            Err(e) => {
                failure = Some((g.file.clone(), e));
                break;
            }
        }
    }
    if failure.is_none() {
        for (i, t) in plan.tomls.iter().enumerate() {
            match write_text(&t.file, &t.after) {
                Ok(()) => written.push(Written::Toml(i)),
                Err(e) => {
                    failure = Some((t.file.clone(), e));
                    break;
                }
            }
        }
    }
    let Some((at, err)) = failure else { return Ok(()) };

    // Put back what was written, newest first.
    let mut stuck: Vec<String> = Vec::new();
    let mut restored: Vec<String> = Vec::new();
    for w in written.iter().rev() {
        let (file, result) = match w {
            Written::Graph(i) => {
                let g = &plan.graphs[*i];
                let locked = locked[*i];
                let store = locked.store();
                let back = GraphDelta { added: g.delta.removed.clone(), removed: g.delta.added.clone() };
                let r = (|| -> Result<()> {
                    for q in &back.removed {
                        store.remove(q)?;
                    }
                    for q in &back.added {
                        store.insert(q)?;
                    }
                    locked.write(Change::OpWithDelta("project.rename.rollback", &back.to_ops()))
                })();
                (&g.file, r)
            }
            Written::Toml(i) => {
                let t = &plan.tomls[*i];
                (&t.file, write_text(&t.file, &t.before))
            }
        };
        match result {
            Ok(()) => restored.push(file.display().to_string()),
            Err(e) => {
                let backup = plan.backups.iter().find(|(f, _)| f == file).map(|(_, b)| b.display().to_string()).unwrap_or_default();
                stuck.push(format!("{} (copy {backup} over it; putting it back failed: {e:#})", file.display()));
            }
        }
    }
    if stuck.is_empty() {
        let put_back = if restored.is_empty() { "Nothing had been written.".to_string() } else { format!("Put back as they were: {}.", restored.join(", ")) };
        Err(err.context(format!("rename stopped writing {}. {put_back} The store is as it was before the rename.", at.display())))
    } else {
        Err(err.context(format!(
            "rename stopped writing {}. These files WERE renamed and could not be put back: {}",
            at.display(),
            stuck.join("; ")
        )))
    }
}

/// Write `text` over `file` through a temp file and a rename, so a reader sees the old file or the new one.
fn write_text(file: &Path, text: &str) -> Result<()> {
    let tmp = file.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    crate::store::rename_with_retry(&tmp, file).with_context(|| format!("renaming {} over {}", tmp.display(), file.display()))
}

/// The plan as the preview and the result print it (Example 1).
pub fn describe(plan: &RenamePlan) -> String {
    let (old, new) = (&plan.old, &plan.new);
    let mut out = String::new();
    let project_in: Vec<&str> = plan.graphs.iter().filter(|g| g.holds_project).map(|g| g.tier).collect();
    out += &format!("project   {old} -> {new}  ({})\n", project_in.join(", "));
    if plan.tomls.is_empty() {
        out += &format!("domain    {old} -> {new}  (in no domains.toml: graph records only, {} rules)\n", plan.rules());
    } else {
        let files: Vec<String> = plan.tomls.iter().map(|t| t.file.display().to_string().replace('\\', "/")).collect();
        out += &format!("domain    {old} -> {new}  ({}, {} rules)\n", files.join(", "), plan.rules());
    }
    let mut first = true;
    for g in &plan.graphs {
        let mut parts: Vec<String> = g
            .rekeyed
            .iter()
            .filter(|(k, _)| *k != "project" && *k != "domain")
            .map(|(k, n)| format!("{} {n}", plural(k)))
            .collect();
        if g.links > 0 {
            parts.push(format!("links {}", g.links));
        }
        if g.handoff_fields > 0 {
            parts.push(format!("handoff fields {}", g.handoff_fields));
        }
        if parts.is_empty() {
            continue;
        }
        out += &format!("{}{}  ({})\n", if first { "records   " } else { "          " }, parts.join(", "), g.tier);
        first = false;
    }
    if first {
        out += "records   none besides the project and the domain\n";
    }
    out += &format!("alias     {old} stays as an alias of {new}\n");
    let backups: Vec<String> = plan.backups.iter().map(|(_, b)| b.display().to_string().replace('\\', "/")).collect();
    out += &format!("backups   {}\n", backups.join(", "));
    out
}

fn plural(kind: &str) -> String {
    match kind {
        "entity" => "entities".to_string(),
        k => format!("{k}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R3's ID rule, one case per shape: the project and the domain by exact name, `{name}.{rest}` records of any
    /// kind, rules under `rule/{name}/`, and nothing named from a path or from its own text.
    #[test]
    fn rekey_moves_only_ids_built_from_the_name() {
        let uri = "http://ops-sys.local/ontology#";
        let names = Names { uri, old: "vintrix", new: "vintryx" };
        let to = |tail: &str| names.rekey(&format!("{uri}{tail}")).map(|(iri, kind)| (iri.trim_start_matches(uri).to_string(), kind));
        let moved = |tail: &str, want: &str, kind: &str| assert_eq!(to(tail), Some((want.to_string(), kind.to_string())), "{tail}");
        moved("project/vintrix", "project/vintryx", "project");
        moved("domain/vintrix", "domain/vintryx", "domain");
        moved("decision/vintrix.use-cox", "decision/vintryx.use-cox", "decision");
        moved("task/vintrix.ship.it", "task/vintryx.ship.it", "task");
        moved("milestone/vintrix.v1", "milestone/vintryx.v1", "milestone");
        moved("rule/vintrix/cli-3", "rule/vintryx/cli-3", "rule");
        moved("rule/vintrix/0", "rule/vintryx/0", "rule");
        moved("note/vintrix.keyed", "note/vintryx.keyed", "note");
        for kept in [
            "project/vintrix-two",
            "domain/vintrixx",
            "decision/vintrixx.a",
            "decision/global.vintrix-is-spelled-wrong",
            "rule/vintrixx/cli-0",
            "note/vintrix-dealership-cto-offer",
            "entity/vintrix-accounting-automation",
            "document/documents-vintrix-cto-roadmap-md",
            "handoff/vintrix.accounting",
            "graph/ws/vintrix",
            "codemap/vintrix",
        ] {
            assert_eq!(to(kept), None, "{kept} keeps its ID");
        }
        assert_eq!(names.rekey("urn:other#project/vintrix"), None, "another namespace is never touched");
    }
}

#[cfg(test)]
mod rollback_tests {
    use super::*;

    /// R6, review finding 7: a quad the rename would add that the store already holds is not an addition, so putting
    /// the rename back (removing what was added, restoring what was removed) leaves the store exactly as it was.
    #[test]
    fn putting_a_rename_back_restores_every_quad_that_was_there() {
        let ns = NamespaceConfig::default();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let t = RDF_TYPE;
        let nq = format!(
            "<{u}domain/old> <{t}> <{u}Domain> <{g}> .\n\
             <{u}note/n1> <{u}hasDomain> <{u}domain/old> <{g}> .\n\
             <{u}note/n1> <{u}hasDomain> <{u}domain/new> <{g}> .\n\
             <{u}handoff/h> <{t}> <{u}Handoff> <{g}> .\n\
             <{u}handoff/h> <{u}project> \"old\" <{g}> .\n\
             <{u}handoff/h> <{u}project> \"Old\" <{g}> .\n"
        );
        let store = oxigraph::store::Store::new().unwrap();
        store.load_from_reader(oxigraph::io::RdfFormat::NQuads, nq.as_bytes()).unwrap();
        let before: HashSet<Quad> = store.iter().map(|q| q.unwrap()).collect();

        let names = Names { uri: u, old: "old", new: "new" };
        let plan = plan_graph(&store, &ns, &names).unwrap();
        let note_new = format!("<{u}note/n1> <{u}hasDomain> <{u}domain/new>");
        assert!(!plan.delta.added.iter().any(|q| q.to_string().starts_with(&note_new)), "a quad already held was added");
        let projects = plan.delta.added.iter().filter(|q| q.predicate.as_str() == format!("{u}project")).count();
        assert_eq!(projects, 1, "two spellings of the old name become one field, added once");

        for q in &plan.delta.removed {
            store.remove(q).unwrap();
        }
        for q in &plan.delta.added {
            store.insert(q).unwrap();
        }
        assert!(store.contains(&Quad::new(named(&format!("{u}note/n1")).unwrap(), named(&format!("{u}hasDomain")).unwrap(), named(&format!("{u}domain/new")).unwrap(), named(&g).unwrap())).unwrap());
        for q in &plan.delta.added {
            store.remove(q).unwrap();
        }
        for q in &plan.delta.removed {
            store.insert(q).unwrap();
        }
        let after: HashSet<Quad> = store.iter().map(|q| q.unwrap()).collect();
        assert_eq!(after, before, "the store after a rename and its rollback");
    }
}
