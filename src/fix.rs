//! `base doctor --fix`: repair what `base doctor` reports, as one library function the upgrade path calls too (BO-12:
//! F15, F16, F24).
//!
//! ONE FUNCTION, TWO CALLERS (F15e). [`run`] plans every repair and, given `apply`, makes it. `base doctor --fix` calls
//! it, and so does the upgrade path ([`crate::migrate::upgrade`], which `base graph migrate` runs), so a user upgrading
//! from 0.15 gets exactly the repair `doctor --fix` makes. The plan and the apply are ONE computation: each tier's
//! repairs are made on a store in memory and recorded as they are made. Without `apply` that store was loaded for the
//! plan and is dropped; with it, it was loaded inside the tier's lock and is written. A plan cannot promise one thing
//! and an apply do another, because there is no second piece of code to disagree.
//!
//! Per tier, in this order:
//!
//! 1. **Records of another workspace (F15c).** Every quad in a graph doctor names as foreign leaves this tier: into
//!    that workspace's own graph when it is registered and reachable here, otherwise into `.base/foreign-<name>.nq`
//!    beside this graph. Moved verbatim, so every date a record carries goes with it (F22c). One exception, and it is a
//!    refusal rather than a repair: a "foreign" graph holding at least as many quads as the tier's own is the shape of
//!    THIS workspace under an earlier folder name (doctor's "most likely renamed" line), and moving it out would empty
//!    the workspace. It is left in place and said so.
//! 2. **Corrections that name nothing they correct (F15b).** A correction whose text names exactly one record, by its
//!    slug, by quoted text, or by a decision's title, gets the supersession edge to it. Every other one keeps its text
//!    and becomes a plain note (`noteType "insight"`, the type `base learn` writes by default), with
//!    `ops:formerNoteType "correction"` beside it, so the label it lost is still on record. Nothing is deleted.
//! 3. **Supersession disagreement (F15d).** A record with status `superseded` and no edge gets the edge when a record
//!    already says it supersedes it; otherwise the status is cleared (`active` when no other status is left). A record
//!    with the edge and no status gets the status: the edge is the truth (`crate::supersede`).
//!
//! Then each tier is compacted with the existing `base graph compact` (F24a; it refuses an unhealthy graph, so the
//! repairs come first) and keeps `[graph] keep_backups` snapshots (F24b). Last, base.toml's legacy `[signal] max_chars`
//! moves to `[budget] memory_chars`, or goes when that is already set (F16).
//!
//! Every apply snapshots a graph before writing it, as compact does. A tier that does not parse is left alone with the
//! command that repairs it: `base doctor --repair` comes first.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use oxigraph::model::{GraphName, LiteralRef, NamedNodeRef, Quad, QuadRef, Subject, Term};
use oxigraph::store::Store;
use serde::Serialize;

use crate::changelog::Change;
use crate::config::{BaseConfig, NamespaceConfig, WorkspaceEntry};
use crate::store::{self, GraphHealth};
use crate::supersede;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// The note type a relabelled correction gets: the one `base learn` writes when no `--type` is given.
pub const PLAIN_NOTE_TYPE: &str = "insight";

/// Beside a relabelled correction: the type it carried before `--fix` (F15b, nothing deleted).
pub const PRED_FORMER_NOTE_TYPE: &str = "formerNoteType";

// ─── What a run plans or did ─────────────────────────────────────────────────

/// One run of [`run`]: what it planned, or what it did when `applied`.
#[derive(Debug, Serialize)]
pub struct Report {
    pub applied: bool,
    pub tiers: Vec<TierFix>,
    pub config: Vec<ConfigFix>,
}

impl Report {
    /// True when any tier or file could not be planned or repaired.
    pub fn has_errors(&self) -> bool {
        self.tiers.iter().any(|t| t.error.is_some()) || self.config.iter().any(|c| c.error.is_some())
    }
}

/// One graph tier's repairs.
#[derive(Debug, Serialize)]
pub struct TierFix {
    /// `global` or `workspace`, as doctor names it.
    pub tier: String,
    pub path: String,
    /// Why nothing was planned for this tier: its graph does not parse, and `base doctor --repair` comes first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    pub foreign: Vec<ForeignGraph>,
    pub corrections: Corrections,
    pub supersession: Vec<Disagreement>,
    /// Compaction after the repairs (F24a). Planned: the line count now and the quads compaction would write.
    pub compact: Option<Compaction>,
    pub backups: Backups,
    /// The snapshot taken before this tier's graph was written. Applied runs only, and only when the graph changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TierFix {
    fn new(tier: &str, path: &Path) -> Self {
        TierFix {
            tier: tier.to_string(),
            path: path.display().to_string(),
            skipped: None,
            foreign: Vec::new(),
            corrections: Corrections::default(),
            supersession: Vec::new(),
            compact: None,
            backups: Backups::default(),
            snapshot: None,
            error: None,
        }
    }

    /// Whether the repairs change this tier's graph (compaction aside).
    pub fn changes_graph(&self) -> bool {
        self.foreign.iter().any(|f| !matches!(f.dest, Dest::Left { .. }))
            || !self.corrections.linked.is_empty()
            || !self.corrections.relabeled.is_empty()
            || !self.supersession.is_empty()
    }
}

/// F15b: the corrections that named nothing they correct, and what became of each.
#[derive(Debug, Default, Serialize)]
pub struct Corrections {
    /// How many there were: doctor's `correction(s) name nothing they correct`.
    pub found: usize,
    /// `(correction, the record it corrects)`: each now carries the supersession edge.
    pub linked: Vec<(String, String)>,
    /// Corrections that become plain notes.
    pub relabeled: Vec<String>,
    /// Of `relabeled`: how many named more than one record, so naming the one they correct would be a guess.
    pub several: usize,
    /// Of `relabeled`: those that named exactly one record that cannot take the edge, as `(correction, record, why)`.
    pub refused: Vec<(String, String, String)>,
}

/// F15c: one graph belonging to another workspace, and where its records go.
#[derive(Debug, Serialize)]
pub struct ForeignGraph {
    pub graph: String,
    /// The workspace the graph names.
    pub workspace: String,
    pub quads: usize,
    /// Typed records in it, by type, highest first.
    pub kinds: Vec<(String, usize)>,
    /// Every subject in it, as `<kind>/<slug>`.
    pub records: Vec<String>,
    pub dest: Dest,
    /// Quads in this tier's other graphs whose subject is one of these records. They are this workspace's own (a domain
    /// link the migration wrote, a task's link to the project) and stay where they are.
    pub left_behind: usize,
}

/// Where a foreign graph's records go.
#[derive(Debug, Serialize)]
#[serde(tag = "to", rename_all = "snake_case")]
pub enum Dest {
    /// That workspace is registered and reachable here: into its own graph.
    Workspace { path: String },
    /// No registered, reachable workspace by that name: into this file beside the graph, kept as N-Quads.
    File { path: String, why: String },
    /// Not moved, with why.
    Left { why: String },
}

/// F15d: one supersession disagreement and its repair.
#[derive(Debug, Serialize)]
#[serde(tag = "fix", rename_all = "snake_case")]
pub enum Disagreement {
    /// Status `superseded`, no edge, and `by` already said it supersedes `record`: the edge is added.
    EdgeAdded { record: String, by: String },
    /// Status `superseded`, no edge, and nothing supersedes it: the status is cleared, and `active` written when no
    /// other status was left (`now`).
    StatusCleared {
        record: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        now: Option<String>,
    },
    /// The edge with no status: the status is added.
    StatusAdded { record: String, by: String },
}

/// F24a: compaction after the repairs.
#[derive(Debug, Serialize)]
pub struct Compaction {
    /// How the tier got compact: `by the repair's write` (its dump is one line per quad), `base graph compact` (a tier
    /// the repair left alone that had duplicate lines), or `already compact` (nothing to do, so no snapshot).
    pub how: &'static str,
    pub lines_before: usize,
    /// The quads the tier holds once repaired, which compaction writes one per line. Planned runs say "about": a write
    /// landing before `--yes` moves it.
    pub lines_after: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
}

/// F24b: the tier's backup snapshots.
#[derive(Debug, Default, Serialize)]
pub struct Backups {
    /// `[graph] keep_backups` for this tier.
    pub keep: usize,
    /// Snapshots this run takes, counted among the kept: one when the graph changes or is compacted, else none.
    pub new: usize,
    /// Existing snapshots kept, newest first.
    pub kept: Vec<String>,
    /// Existing snapshots removed (planned: to be removed), with their size in bytes.
    pub removed: Vec<(String, u64)>,
    /// Other copies beside the graph that are not base's backups (another name: `graph.nq.BAK-…`, `graph.nq.torn-…`, a
    /// quarantine file). Named so the operator knows they exist; never touched.
    pub other_copies: Vec<(String, u64)>,
}

/// F16: one base.toml carrying `[signal] max_chars`.
#[derive(Debug, Serialize)]
pub struct ConfigFix {
    pub file: String,
    /// The legacy value.
    pub max_chars: Option<i64>,
    /// True: the value moved to `[budget] memory_chars`, which was unset. False: the key was removed, because
    /// `memory_chars` is set (`memory_chars` holds that value).
    pub moved: bool,
    /// The memory block's budget before the move: the value already set (removed), or the default (moved).
    pub memory_chars: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ─── The one function ────────────────────────────────────────────────────────

/// Plan every repair `base doctor` reports for the tiers it reads from `cwd`, and make them when `apply` (F15e: `base
/// doctor --fix` and the upgrade path both call this). Errors are per tier and per file, so one that fails does not
/// stop the others.
pub fn run(cwd: &Path, apply: bool) -> Report {
    // The registry from the workspace's own config, found by the walk, so a run from a subfolder reads what doctor reads.
    let root = crate::config::find_workspace_base(cwd).and_then(|b| b.parent().map(Path::to_path_buf));
    let config = BaseConfig::load(root.as_deref().unwrap_or(cwd));
    let home = crate::home::home_root();
    let tiers = crate::doctor::tier_paths(cwd)
        .into_iter()
        .map(|(tier, path)| {
            let mut fix = TierFix::new(&tier, &path);
            // Each tier read with its own namespace, from its own base.toml, as `doctor::diagnose_tier` reads it.
            let tier_root = path.parent().and_then(Path::parent).unwrap_or(&path);
            let tier_config = BaseConfig::load(tier_root);
            let ctx = Ctx { cwd, ns: &tier_config.namespace, registry: &config.workspace, home: home.clone() };
            if let Err(e) = tier_fix(&ctx, &path, apply, &mut fix) {
                fix.error = Some(format!("{e:#}"));
            }
            fix
        })
        .collect();
    Report { applied: apply, tiers, config: config_fixes(cwd, apply) }
}

struct Ctx<'a> {
    cwd: &'a Path,
    ns: &'a NamespaceConfig,
    registry: &'a [WorkspaceEntry],
    home: Option<PathBuf>,
}

fn tier_fix(ctx: &Ctx<'_>, path: &Path, apply: bool, fix: &mut TierFix) -> Result<()> {
    if let GraphHealth::Unhealthy { reason, .. } = store::graph_health(path) {
        fix.skipped = Some(format!("not healthy ({reason}): run `base doctor --repair` first, then --fix again"));
        return Ok(());
    }
    let keep = store::keep_backups_for(path);
    let lines_before = count_lines(path);
    let before_backups = store::backups(path);

    // The repairs, made on a store in memory. With `apply` it was loaded inside the tier's lock, and the lock is held
    // until the write; without, it is a copy for the plan and is dropped.
    let locked = if apply { Some(store::lock_and_load_graph(path)?) } else { None };
    let planned;
    let store = match &locked {
        Some(l) => l.store(),
        None => {
            planned = store::load_graph(path)?;
            &planned
        }
    };
    let quads_before = store.len().context("counting the store")?;
    let moves = repair_store(ctx, path, store, fix)?;
    let lines_after = store.len().context("counting the repaired store")?;
    let changed = fix.changes_graph();
    // F24a. The repair's own write is a compaction: the store dumped one line per quad, duplicates gone, which is all
    // `base graph compact` does. So compaction runs by itself only on a tier the repair leaves alone and that has
    // duplicate lines; otherwise a re-run on a repaired store would take a snapshot to change nothing, and rotate out
    // the one taken before the repair (code review, finding 1).
    let how = if changed {
        "by the repair's write"
    } else if lines_before > quads_before {
        "base graph compact"
    } else {
        "already compact"
    };
    let new = usize::from(how != "already compact");

    if let Some(locked) = locked {
        let mut backup = None;
        if changed {
            let snap = store::snapshot(path, "fix")?.display().to_string();
            fix.snapshot = Some(snap.clone());
            backup = Some(snap);
            // Destinations first: a failure here leaves this tier unwritten, so nothing is lost, and a run after it
            // finds the same records to move (a destination graph is a set, a destination file is deduplicated). Each
            // destination is written once, however many graphs go to it: one lock, one snapshot, one write.
            let mut by_dest: BTreeMap<String, (Dest, Vec<Quad>)> = BTreeMap::new();
            for (dest, quads) in moves {
                let key = match &dest {
                    Dest::Workspace { path } | Dest::File { path, .. } => path.clone(),
                    Dest::Left { .. } => continue,
                };
                by_dest.entry(key).or_insert_with(|| (dest, Vec::new())).1.extend(quads);
            }
            for (dest, quads) in by_dest.values() {
                write_destination(dest, quads)?;
            }
            locked.write(Change::Op("doctor.fix"))?;
        }
        drop(locked);
        let mut lines_now = lines_after;
        if how == "base graph compact" {
            let out = crate::graph::compact_tier(path)?;
            lines_now = out.lines_after;
            backup = Some(out.backup);
        }
        fix.compact = Some(Compaction { how, lines_before, lines_after: lines_now, backup });
        store::prune_backups(path, keep, None);
        let after: BTreeSet<PathBuf> = store::backups(path).into_iter().map(|b| b.path).collect();
        fix.backups = backup_report(path, keep, new, &before_backups, |b| after.contains(&b.path));
    } else {
        fix.compact = Some(Compaction { how, lines_before, lines_after, backup: None });
        let kept: BTreeSet<PathBuf> =
            before_backups.iter().take(keep.saturating_sub(new)).map(|b| b.path.clone()).collect();
        fix.backups = backup_report(path, keep, new, &before_backups, |b| kept.contains(&b.path));
    }
    Ok(())
}

fn backup_report(
    path: &Path,
    keep: usize,
    new: usize,
    before: &[store::Backup],
    kept: impl Fn(&store::Backup) -> bool,
) -> Backups {
    let (k, r): (Vec<&store::Backup>, Vec<&store::Backup>) = before.iter().partition(|b| kept(b));
    Backups {
        keep,
        new,
        kept: k.iter().map(|b| b.path.display().to_string()).collect(),
        removed: r.iter().map(|b| (b.path.display().to_string(), b.bytes)).collect(),
        other_copies: other_copies(path),
    }
}

/// Files beside the graph named after it that are not base's own backups, its lock or its temp files.
fn other_copies(path: &Path) -> Vec<(String, u64)> {
    let (Some(dir), Some(fname)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return Vec::new();
    };
    let (prefix, bak) = (format!("{fname}."), format!("{fname}.bak"));
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(String, u64)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let ours = name.starts_with(&bak) || name.ends_with(".lock") || name.contains(".tmp");
            (name.starts_with(&prefix) && !ours).then(|| (e.path().display().to_string(), e.metadata().map(|m| m.len()).unwrap_or(0)))
        })
        .collect();
    out.sort();
    out
}

/// The three graph repairs, on `store` in memory, recorded in `fix`. Returns the foreign quads to write to each
/// destination; the caller writes them, and only on an apply.
fn repair_store(ctx: &Ctx<'_>, path: &Path, store: &Store, fix: &mut TierFix) -> Result<Vec<(Dest, Vec<Quad>)>> {
    let moves = move_foreign(ctx, path, store, fix)?;
    link_corrections(ctx.ns, store, &mut fix.corrections)?;
    fix.supersession = settle_disagreements(ctx.ns, store)?;
    Ok(moves)
}

// ─── F15c: records of another workspace ──────────────────────────────────────

fn move_foreign(ctx: &Ctx<'_>, path: &Path, store: &Store, fix: &mut TierFix) -> Result<Vec<(Dest, Vec<Quad>)>> {
    let own_slug = crate::doctor::tier_own_slug(path);
    let mut by_graph: BTreeMap<String, Vec<Quad>> = BTreeMap::new();
    let mut own = 0usize;
    for quad in store.iter() {
        let quad = quad?;
        let GraphName::NamedNode(g) = &quad.graph_name else { continue };
        if crate::doctor::is_foreign(g.as_str(), &ctx.ns.uri, &own_slug) {
            by_graph.entry(g.as_str().to_string()).or_default().push(quad);
        } else if crate::doctor::graph_owner(g.as_str(), &ctx.ns.uri) == Some(own_slug.as_str()) {
            own += 1;
        }
    }
    // Highest count first, as doctor lists them.
    let mut graphs: Vec<(String, Vec<Quad>)> = by_graph.into_iter().collect();
    graphs.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

    // Quads per subject over the whole tier, once: what a foreign graph's records still have elsewhere in it.
    let mut per_subject: HashMap<String, usize> = HashMap::new();
    if !graphs.is_empty() {
        for q in store.iter().filter_map(Result::ok) {
            if let Some(s) = subject_iri(&q.subject) {
                *per_subject.entry(s).or_default() += 1;
            }
        }
    }
    let mut moves = Vec::new();
    for (graph, quads) in graphs {
        let workspace = crate::doctor::graph_owner(&graph, &ctx.ns.uri).unwrap_or_default().to_string();
        let subjects: BTreeSet<String> = quads.iter().filter_map(|q| subject_iri(&q.subject)).collect();
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for q in quads.iter().filter(|q| q.predicate.as_str() == RDF_TYPE) {
            if let Term::NamedNode(t) = &q.object {
                *kinds.entry(local(t.as_str(), &ctx.ns.uri)).or_default() += 1;
            }
        }
        let mut kinds: Vec<(String, usize)> = kinds.into_iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let left_behind = subjects.iter().map(|s| per_subject.get(s).copied().unwrap_or(0)).sum::<usize>()
            - quads.iter().filter(|q| subject_iri(&q.subject).is_some()).count();
        let dest = if quads.len() >= own {
            Dest::Left {
                why: format!(
                    "{} quads, at least this workspace's own {own}: the shape of this workspace under an earlier folder \
                     name, not another workspace's records",
                    quads.len()
                ),
            }
        } else {
            destination(ctx, path, &workspace)
        };
        if !matches!(dest, Dest::Left { .. }) {
            for q in &quads {
                store.remove(q)?;
            }
            moves.push((dest_clone(&dest), quads.clone()));
        }
        fix.foreign.push(ForeignGraph {
            graph,
            workspace,
            quads: quads.len(),
            kinds,
            records: subjects.iter().map(|s| local(s, &ctx.ns.uri)).collect(),
            dest,
            left_behind,
        });
    }
    Ok(moves)
}

fn dest_clone(d: &Dest) -> Dest {
    match d {
        Dest::Workspace { path } => Dest::Workspace { path: path.clone() },
        Dest::File { path, why } => Dest::File { path: path.clone(), why: why.clone() },
        Dest::Left { why } => Dest::Left { why: why.clone() },
    }
}

/// Where `workspace`'s records go from the tier at `path`. A workspace is registered when the `[[workspace]]` list names
/// a folder whose name slugifies to it, or it is the workspace `cwd` stands in, or it is the global tier (`base-gbl`);
/// it is reachable when that folder's `.base` exists here. Exactly one: its graph. None, or more than one: a file
/// beside this graph, and the reason.
fn destination(ctx: &Ctx<'_>, path: &Path, workspace: &str) -> Dest {
    let mut named: Vec<PathBuf> = Vec::new();
    if workspace == "base-gbl"
        && let Some(home) = &ctx.home
    {
        named.push(home.join(".base-gbl"));
    }
    for e in ctx.registry {
        let root = Path::new(&e.path);
        if root.file_name().and_then(|n| n.to_str()).map(crate::crud::slugify).as_deref() == Some(workspace) {
            named.push(root.to_path_buf());
        }
    }
    if let Some(base) = crate::config::find_workspace_base(ctx.cwd)
        && let Some(root) = base.parent()
        && crate::doctor::tier_own_slug(&base.join("graph.nq")) == workspace
    {
        named.push(root.to_path_buf());
    }
    let key = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let here = key(path);
    let mut reachable: Vec<PathBuf> = Vec::new();
    for root in &named {
        let graph = root.join(".base").join("graph.nq");
        if root.join(".base").is_dir() && key(&graph) != here && !reachable.iter().any(|r| key(r) == key(&graph)) {
            reachable.push(graph);
        }
    }
    // The name comes from a graph IRI, which an inbound op can make anything: only its slug reaches a file name.
    let safe = match crate::crud::slugify(workspace) {
        s if s.is_empty() => "unnamed".to_string(),
        s => s,
    };
    let file = path.with_file_name(format!("foreign-{safe}.nq")).display().to_string();
    match reachable.as_slice() {
        [one] => Dest::Workspace { path: one.display().to_string() },
        [] if named.is_empty() => Dest::File { path: file, why: format!("no registered workspace named {workspace}") },
        [] => Dest::File {
            path: file,
            why: format!("{workspace} is registered at {}, which has no .base here", named[0].display()),
        },
        many => Dest::File { path: file, why: format!("{} registered workspaces are named {workspace}", many.len()) },
    }
}

/// Write `quads` to their destination: into a reachable workspace's graph (locked, snapshotted, written as any write
/// is), or appended to the foreign file without repeating a line it already holds.
fn write_destination(dest: &Dest, quads: &[Quad]) -> Result<()> {
    match dest {
        Dest::Left { .. } => Ok(()),
        Dest::Workspace { path } => {
            let path = Path::new(path);
            let locked = store::lock_and_load_graph(path)
                .with_context(|| format!("loading {} to move records into it", path.display()))?;
            if path.exists() {
                store::snapshot(path, "fix-move-in")?;
            }
            for q in quads {
                locked.store().insert(q)?;
            }
            locked.write(Change::Op("doctor.fix.move-in"))
        }
        // Locked like a graph: two runs appending to one file at once would each write the other's lines away.
        Dest::File { path, .. } => store::with_graph_lock(Path::new(path), || {
            let path = Path::new(path);
            let existing = std::fs::read_to_string(path).unwrap_or_default();
            let have: BTreeSet<&str> = existing.lines().map(str::trim).collect();
            let mut text = existing.clone();
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            for q in quads {
                let line = format!("{q} .");
                if !have.contains(line.as_str()) {
                    text.push_str(&line);
                    text.push('\n');
                }
            }
            // Parsed back before it replaces anything: a file of records nobody can read is not a move.
            Store::new()?
                .load_from_reader(oxigraph::io::RdfFormat::NQuads, text.as_bytes())
                .with_context(|| format!("the records for {} would not parse; nothing written", path.display()))?;
            let tmp = path.with_extension("nq.fix-tmp");
            std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
            std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
        }),
    }
}

// ─── F15b: corrections that name nothing they correct ────────────────────────

/// Link each correction that names exactly one record to it; make every other one a plain note.
fn link_corrections(ns: &NamespaceConfig, store: &Store, out: &mut Corrections) -> Result<()> {
    let p = |local: &str| format!("{}{local}", ns.uri);
    let iris = [p("noteType"), p("noteText"), p(supersede::PRED_SUPERSEDES), p(supersede::PRED_SUPERSEDED_BY), p("createdAt")];
    let (note_type, note_text, supersedes, sup_by, created) =
        (node(&iris[0])?, node(&iris[1])?, node(&iris[2])?, node(&iris[3])?, node(&iris[4])?);

    // Every correction that names nothing: doctor's count, read the way `supersede::audit` reads it.
    let mut corrections: BTreeMap<String, Vec<Quad>> = BTreeMap::new();
    for q in store.quads_for_pattern(None, Some(note_type), None, None) {
        let q = q?;
        if matches!(&q.object, Term::Literal(l) if l.value() == "correction")
            && let Some(s) = subject_iri(&q.subject)
        {
            corrections.entry(s).or_default().push(q);
        }
    }
    corrections.retain(|s, _| {
        NamedNodeRef::new(s).is_ok_and(|n| store.quads_for_pattern(Some(n.into()), Some(supersedes), None, None).next().is_none())
    });
    out.found = corrections.len();
    if corrections.is_empty() {
        return Ok(());
    }

    let index = TargetIndex::build(ns, store)?;
    let literal = |s: &str, pred: NamedNodeRef<'_>| -> Option<String> {
        let n = NamedNodeRef::new(s).ok()?;
        store.quads_for_pattern(Some(n.into()), Some(pred), None, None).filter_map(Result::ok).find_map(|q| match q.object {
            Term::Literal(l) => Some(l.value().to_string()),
            _ => None,
        })
    };
    let former_iri = p(PRED_FORMER_NOTE_TYPE);
    let former = node(&former_iri)?;

    for (correction, type_quads) in &corrections {
        let text = literal(correction, note_text).unwrap_or_default();
        let mut named_records = index.named_in(&text);
        named_records.remove(correction);
        let target = match named_records.len() {
            1 => named_records.into_iter().next(),
            0 => None,
            _ => {
                out.several += 1;
                None
            }
        };
        let refusal = target.as_ref().and_then(|t| {
            let tn = NamedNodeRef::new(t).ok()?;
            if let Some(by) = store
                .quads_for_pattern(Some(tn.into()), Some(sup_by), None, None)
                .filter_map(Result::ok)
                .find_map(|q| match q.object {
                    Term::NamedNode(n) => Some(n.into_string()),
                    _ => None,
                })
            {
                return Some(format!("already superseded by {}", local(&by, &ns.uri)));
            }
            let when = |s: &str| literal(s, created).and_then(|v| chrono::DateTime::parse_from_rfc3339(&v).ok());
            if let (Some(t_at), Some(c_at)) = (when(t), when(correction))
                && t_at > c_at
            {
                return Some("written after the correction".to_string());
            }
            if supersede::would_cycle(store, ns, t, correction) {
                return Some("would close a supersession cycle".to_string());
            }
            None
        });
        match (target, refusal) {
            (Some(t), None) => {
                // Into the correction's own graph, the one its type is written in.
                let graph = match &type_quads[0].graph_name {
                    GraphName::NamedNode(g) => g.as_str().to_string(),
                    _ => bail!("{correction} carries its type in no named graph"),
                };
                let update = format!("{}\n{}", crate::crud::prefixes(ns), supersede::link_update(ns, &graph, &t, correction));
                store.update(&update).with_context(|| format!("linking {correction} to {t}"))?;
                out.linked.push((local(correction, &ns.uri), local(&t, &ns.uri)));
            }
            (target, refusal) => {
                if let (Some(t), Some(why)) = (target, refusal) {
                    out.refused.push((local(correction, &ns.uri), local(&t, &ns.uri), why));
                }
                for q in type_quads {
                    store.remove(q)?;
                    let g = q.graph_name.as_ref();
                    let s = q.subject.as_ref();
                    store.insert(QuadRef::new(s, note_type, LiteralRef::new_simple_literal(PLAIN_NOTE_TYPE), g))?;
                    store.insert(QuadRef::new(s, former, LiteralRef::new_simple_literal("correction"), g))?;
                }
                out.relabeled.push(local(correction, &ns.uri));
            }
        }
    }
    Ok(())
}

/// The records a correction can name: notes, decisions and rules in this tier (`crate::crud::supersede::resolve_slug`
/// resolves a `--supersedes` slug in one tier too), indexed three ways.
struct TargetIndex {
    /// `kind/slug` and `slug`, lowercased.
    by_slug: HashMap<String, BTreeSet<String>>,
    /// A record's whole text (a note's, a rule's, a decision's), normalised.
    by_text: HashMap<String, BTreeSet<String>>,
    /// Decision titles of three words or more, normalised: long enough that finding one inside a correction is a
    /// mention, not a coincidence.
    titles: Vec<(String, String)>,
}

impl TargetIndex {
    fn build(ns: &NamespaceConfig, store: &Store) -> Result<Self> {
        let kinds: BTreeSet<String> = ["Note", "Decision", "Rule"].iter().map(|k| format!("{}{k}", ns.uri)).collect();
        let rdf_type = NamedNodeRef::new(RDF_TYPE)?;
        let mut targets: BTreeMap<String, String> = BTreeMap::new();
        for q in store.quads_for_pattern(None, Some(rdf_type), None, None) {
            let q = q?;
            if let (Some(s), Term::NamedNode(t)) = (subject_iri(&q.subject), &q.object)
                && kinds.contains(t.as_str())
            {
                targets.insert(s, local(t.as_str(), &ns.uri));
            }
        }
        let mut index = TargetIndex { by_slug: HashMap::new(), by_text: HashMap::new(), titles: Vec::new() };
        let text_pred = |kind: &str| match kind {
            "Note" => "noteText",
            "Rule" => "ruleText",
            _ => "name",
        };
        for (iri, kind) in &targets {
            let tail = local(iri, &ns.uri).to_lowercase();
            if let Some((_, slug)) = tail.split_once('/') {
                index.by_slug.entry(slug.to_string()).or_default().insert(iri.clone());
            }
            index.by_slug.entry(tail).or_default().insert(iri.clone());
            let pred = format!("{}{}", ns.uri, text_pred(kind));
            let (Ok(s), Ok(pred)) = (NamedNodeRef::new(iri), NamedNodeRef::new(&pred)) else { continue };
            for q in store.quads_for_pattern(Some(s.into()), Some(pred), None, None).filter_map(Result::ok) {
                if let Term::Literal(l) = q.object {
                    let text = normalise(l.value());
                    if kind == "Decision" && text.split(' ').count() >= 3 {
                        index.titles.push((text.clone(), iri.clone()));
                    }
                    index.by_text.entry(text).or_default().insert(iri.clone());
                }
            }
        }
        Ok(index)
    }

    /// Every record `text` names: a slug written in it, a quoted passage that is a record's whole text, or a decision
    /// title it contains.
    fn named_in(&self, text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for token in slug_tokens(text) {
            if let Some(hits) = self.by_slug.get(&token) {
                out.extend(hits.iter().cloned());
            }
        }
        for quoted in quoted_passages(text) {
            if let Some(hits) = self.by_text.get(&normalise(&quoted)) {
                out.extend(hits.iter().cloned());
            }
        }
        let flat = normalise(text);
        for (title, iri) in &self.titles {
            if flat.contains(title.as_str()) {
                out.insert(iri.clone());
            }
        }
        out
    }
}

/// Runs of letters, digits and `._/-` in `text`, trimmed of punctuation at either end, lowercased, kept only when they
/// hold a `-`, `.` or `/`: a slug has one, and a plain word matching a one-word slug is a coincidence.
fn slug_tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-')))
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_alphanumeric()))
        .filter(|t| t.contains(['-', '.', '/']))
        .map(str::to_lowercase)
        .collect()
}

/// Passages of three characters or more between `"…"`, `` `…` `` or `“…”`.
fn quoted_passages(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (open, close) in [('"', '"'), ('`', '`'), ('\u{201c}', '\u{201d}')] {
        let mut rest = text;
        while let Some(start) = rest.find(open) {
            let after = &rest[start + open.len_utf8()..];
            let Some(end) = after.find(close) else { break };
            let passage = &after[..end];
            if passage.chars().count() >= 3 && !passage.contains('\n') {
                out.push(passage.to_string());
            }
            rest = &after[end + close.len_utf8()..];
        }
    }
    out
}

/// Lowercased, with every run of whitespace one space.
fn normalise(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

// ─── F15d: supersession disagreement ─────────────────────────────────────────

fn settle_disagreements(ns: &NamespaceConfig, store: &Store) -> Result<Vec<Disagreement>> {
    let p = |local: &str| format!("{}{local}", ns.uri);
    let (status_s, sup_s, sup_by_s) = (p("status"), p(supersede::PRED_SUPERSEDES), p(supersede::PRED_SUPERSEDED_BY));
    let (status, sup, sup_by) =
        (NamedNodeRef::new(&status_s)?, NamedNodeRef::new(&sup_s)?, NamedNodeRef::new(&sup_by_s)?);
    let superseded = LiteralRef::new_simple_literal(supersede::STATUS_SUPERSEDED);

    let mut marked: BTreeMap<String, Vec<Quad>> = BTreeMap::new();
    for q in store.quads_for_pattern(None, Some(status), Some(superseded.into()), None) {
        let q = q?;
        if let Some(s) = subject_iri(&q.subject) {
            marked.entry(s).or_default().push(q);
        }
    }
    let mut edges: BTreeMap<String, (String, Quad)> = BTreeMap::new();
    for q in store.quads_for_pattern(None, Some(sup_by), None, None) {
        let q = q?;
        if let (Some(s), Term::NamedNode(by)) = (subject_iri(&q.subject), &q.object) {
            edges.entry(s).or_insert_with(|| (by.as_str().to_string(), q.clone()));
        }
    }

    let mut out = Vec::new();
    for (record, quads) in &marked {
        if edges.contains_key(record) {
            continue;
        }
        let r = NamedNodeRef::new(record)?;
        // A record that already says it supersedes this one: the lexicographically first, so two runs agree.
        let by = store
            .quads_for_pattern(None, Some(sup), Some(r.into()), None)
            .filter_map(Result::ok)
            .filter_map(|q| subject_iri(&q.subject))
            .filter(|by| !supersede::would_cycle(store, ns, record, by))
            .min();
        let g = quads[0].graph_name.as_ref();
        match by {
            Some(by) => {
                store.insert(QuadRef::new(r, sup_by, NamedNodeRef::new(&by)?, g))?;
                out.push(Disagreement::EdgeAdded { record: local(record, &ns.uri), by: local(&by, &ns.uri) });
            }
            None => {
                for q in quads {
                    store.remove(q)?;
                }
                let left = store.quads_for_pattern(Some(r.into()), Some(status), None, None).next().is_some();
                let now = if left {
                    None
                } else {
                    store.insert(QuadRef::new(r, status, LiteralRef::new_simple_literal("active"), g))?;
                    Some("active".to_string())
                };
                out.push(Disagreement::StatusCleared { record: local(record, &ns.uri), now });
            }
        }
    }
    for (record, (by, quad)) in &edges {
        if marked.contains_key(record) {
            continue;
        }
        let r = NamedNodeRef::new(record)?;
        store.insert(QuadRef::new(r, status, superseded, quad.graph_name.as_ref()))?;
        out.push(Disagreement::StatusAdded { record: local(record, &ns.uri), by: local(by, &ns.uri) });
    }
    Ok(out)
}

// ─── F16: [signal] max_chars ─────────────────────────────────────────────────

/// Every base.toml doctor names as carrying `[signal] max_chars`, planned, and rewritten when `apply`.
fn config_fixes(cwd: &Path, apply: bool) -> Vec<ConfigFix> {
    let global = crate::home::home_root().map(|h| h.join(".base-gbl").join("base.toml"));
    let set_in = |file: &Path| -> Option<i64> {
        let table: toml::Table = std::fs::read_to_string(file).ok()?.parse().ok()?;
        table.get("budget")?.get("memory_chars")?.as_integer()
    };
    BaseConfig::legacy_keys(cwd)
        .into_iter()
        .filter(|k| k.section == "signal" && k.key == "max_chars")
        .map(|k| {
            // Set where this file reads it: in the file itself, or for a workspace's file, in the global one under it.
            let set = set_in(&k.file).or_else(|| global.as_deref().filter(|g| *g != k.file).and_then(set_in));
            let mut fix = ConfigFix {
                file: k.file.display().to_string(),
                max_chars: None,
                moved: set.is_none(),
                memory_chars: set.unwrap_or(crate::config::BudgetConfig::default().memory_chars as i64),
                error: None,
            };
            if let Err(e) = migrate_max_chars(&k.file, apply, &mut fix) {
                fix.error = Some(format!("{e:#}"));
            }
            fix
        })
        .collect()
}

fn migrate_max_chars(file: &Path, apply: bool, fix: &mut ConfigFix) -> Result<()> {
    let before = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let table: toml::Table = before.parse().with_context(|| format!("{} does not parse", file.display()))?;
    let value = table.get("signal").and_then(|s| s.get("max_chars")).cloned();
    fix.max_chars = value.as_ref().and_then(toml::Value::as_integer);
    let after = rewrite_max_chars(&before, fix.moved.then_some(fix.max_chars).flatten())?;
    if apply {
        let tmp = file.with_extension("toml.fix-tmp");
        std::fs::write(&tmp, &after).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, file).with_context(|| format!("replacing {}", file.display()))?;
    }
    Ok(())
}

/// `text` without its `[signal] max_chars` line, and with `[budget] memory_chars = <to>` when `to` is given. Every other
/// line stays as it was, comments included; the result is parsed back and refused unless exactly those keys changed.
pub fn rewrite_max_chars(text: &str, to: Option<i64>) -> Result<String> {
    let mut section = String::new();
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with('[') {
            let head = t.split('#').next().unwrap_or("");
            section = head.chars().filter(|c| !c.is_whitespace()).collect();
        }
        let key = (!t.starts_with('[') && !t.starts_with('#'))
            .then(|| t.split_once('=').map(|(k, _)| k.trim().trim_matches(['"', '\''])))
            .flatten();
        if section == "[signal]" && key == Some("max_chars") {
            continue;
        }
        out.push_str(line);
    }
    let out = match to {
        Some(v) => crate::measure::set_budget_keys(&out, &[("memory_chars", v.to_string())])?,
        None => out,
    };
    let mut want: toml::Table = text.parse().context("the base.toml does not parse")?;
    if let Some(toml::Value::Table(s)) = want.get_mut("signal") {
        s.remove("max_chars");
    }
    if let Some(v) = to {
        let budget = want.entry("budget").or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(b) = budget {
            b.insert("memory_chars".to_string(), toml::Value::Integer(v));
        }
    }
    let got: toml::Table = out.parse().context("the rewritten base.toml would not parse; nothing written")?;
    if got != want {
        bail!("the rewrite would change more than [signal] max_chars and [budget] memory_chars; nothing written");
    }
    Ok(out)
}

// ─── shared ──────────────────────────────────────────────────────────────────

fn node(iri: &str) -> Result<NamedNodeRef<'_>> {
    NamedNodeRef::new(iri).with_context(|| format!("not an IRI: {iri}"))
}

fn subject_iri(s: &Subject) -> Option<String> {
    match s {
        Subject::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    }
}

/// `{ns}note/x` → `note/x`.
fn local(iri: &str, ns_uri: &str) -> String {
    iri.strip_prefix(ns_uri).unwrap_or(iri).to_string()
}

fn count_lines(path: &Path) -> usize {
    use std::io::BufRead;
    std::fs::File::open(path).map(|f| std::io::BufReader::new(f).lines().map_while(Result::ok).count()).unwrap_or(0)
}

fn mb(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}

// ─── The human report ────────────────────────────────────────────────────────

/// `label ........ value`, the plan's row shape.
fn row(out: &mut String, label: &str, value: &str) {
    const WIDTH: usize = 46;
    let dots = WIDTH.saturating_sub(label.chars().count() + 2).max(3);
    out.push_str(&format!("    {label} {} {value}\n", ".".repeat(dots)));
}

fn names(list: &[String], max: usize) -> String {
    let shown: Vec<&str> = list.iter().take(max).map(String::as_str).collect();
    let more = list.len().saturating_sub(shown.len());
    if more > 0 { format!("{}, and {more} more", shown.join(", ")) } else { shown.join(", ") }
}

/// The plan, or what was done, as `base doctor --fix` prints it.
pub fn format_human(r: &Report) -> String {
    format_as(r, "base doctor --fix")
}

/// The plan, or what was done, under the command that ran it (`base graph migrate` prints the same rows, F15e): the
/// same rows either way, in the order the repairs run.
pub fn format_as(r: &Report, command: &str) -> String {
    let mut out = String::new();
    if r.applied {
        out.push_str(&format!("{command} --yes\ndone:\n"));
    } else {
        out.push_str(&format!("{command}\nplan (nothing changed yet; add --yes to apply):\n"));
    }
    for t in &r.tiers {
        out.push_str(&format!("  {} tier  {}\n", t.tier, t.path));
        if let Some(why) = &t.skipped {
            out.push_str(&format!("    ⚠ {why}\n"));
            continue;
        }
        if let Some(e) = &t.error {
            out.push_str(&format!("    ⚠ failed: {e}\n"));
        }
        format_tier(&mut out, t, r.applied);
    }
    if !r.config.is_empty() {
        out.push_str("  base.toml\n");
        for c in &r.config {
            let value = c.max_chars.map_or("(not a number)".to_string(), |v| v.to_string());
            let line = if c.moved {
                format!(
                    "[signal] max_chars = {value} -> [budget] memory_chars = {value} (memory_chars was unset; the memory \
                     block's budget was the default {})",
                    c.memory_chars
                )
            } else {
                format!("[signal] max_chars = {value} removed ([budget] memory_chars = {} is set)", c.memory_chars)
            };
            out.push_str(&format!("    {line}  {}\n", c.file));
            if let Some(e) = &c.error {
                out.push_str(&format!("    ⚠ failed: {e}\n"));
            }
        }
    }
    if !r.applied {
        out.push_str(&format!("nothing changed: {command} --yes applies this plan\n"));
    }
    out
}

fn format_tier(out: &mut String, t: &TierFix, applied: bool) {
    if let Some(s) = &t.snapshot {
        row(out, "snapshot", s);
    }
    if !t.foreign.is_empty() {
        let total: usize = t.foreign.iter().map(|f| f.quads).sum();
        let each: Vec<String> = t.foreign.iter().map(|f| format!("{} {}", f.workspace, f.quads)).collect();
        row(out, "move foreign records", &format!("{total} quads: {}", each.join(", ")));
        for f in &t.foreign {
            let kinds: Vec<String> = f.kinds.iter().map(|(k, n)| format!("{n} {k}")).collect();
            let to = match &f.dest {
                Dest::Workspace { path } => format!("-> {path}"),
                Dest::File { path, why } => format!("-> {path} ({why})"),
                Dest::Left { why } => format!("left in place: {why}"),
            };
            out.push_str(&format!("        {}: {} ({}) {to}\n", f.workspace, names(&f.records, 6), kinds.join(", ")));
            if f.left_behind > 0 && !matches!(f.dest, Dest::Left { .. }) {
                out.push_str(&format!(
                    "        {} quad(s) elsewhere in this tier name these records; they are this workspace's own and stay\n",
                    f.left_behind
                ));
            }
        }
    }
    let c = &t.corrections;
    if c.found > 0 {
        let mut value = format!(
            "{} found: {} linked, {} become plain notes",
            c.found,
            c.linked.len(),
            c.relabeled.len()
        );
        if c.several > 0 {
            value.push_str(&format!(" ({} name more than one record)", c.several));
        }
        row(out, "link corrections to what they correct", &value);
        for (from, to) in &c.linked {
            out.push_str(&format!("        {from} corrects {to}\n"));
        }
        for (from, to, why) in &c.refused {
            out.push_str(&format!("        {from} names {to}, not linked: {why}\n"));
        }
    }
    if !t.supersession.is_empty() {
        let each: Vec<String> = t
            .supersession
            .iter()
            .map(|d| match d {
                Disagreement::EdgeAdded { record, by } => format!("{record}: edge added ({by} supersedes it)"),
                Disagreement::StatusCleared { record, now } => format!(
                    "{record}: status cleared, no replacing record found{}",
                    now.as_deref().map(|s| format!(" (now {s})")).unwrap_or_default()
                ),
                Disagreement::StatusAdded { record, by } => format!("{record}: status added (superseded by {by})"),
            })
            .collect();
        row(out, "supersession disagreement", &format!("{}: {}", t.supersession.len(), each.join("; ")));
    }
    if let Some(cp) = &t.compact {
        let value = if cp.how == "already compact" {
            format!("{} lines, already compact (one line per quad): nothing to do", cp.lines_before)
        } else if applied {
            format!("{} -> {} lines ({})", cp.lines_before, cp.lines_after, cp.how)
        } else {
            format!("{} lines -> about {} ({})", cp.lines_before, cp.lines_after, cp.how)
        };
        row(out, "compact", &value);
    }
    let b = &t.backups;
    let bytes: u64 = b.removed.iter().map(|(_, n)| n).sum();
    let gone = if applied { "removed" } else { "remove" };
    let value = if b.removed.is_empty() {
        format!("keep {} (nothing to remove)", b.keep)
    } else {
        format!("keep {}, {gone} {} (about {} MB)", b.keep, b.removed.len(), mb(bytes))
    };
    row(out, "backups", &value);
    if !b.other_copies.is_empty() {
        let total: u64 = b.other_copies.iter().map(|(_, n)| n).sum();
        let files: Vec<String> = b
            .other_copies
            .iter()
            .map(|(p, _)| Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p).to_string())
            .collect();
        out.push_str(&format!(
            "        not base's backups, left alone: {} ({} MB)\n",
            names(&files, 4),
            mb(total)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_token_needs_a_separator_and_loses_its_punctuation() {
        let t = slug_tokens("CORRECTS whisperx-large-v3: see (base-config.hub-port), note/x-y. Plain words: port, base.");
        assert!(t.contains("whisperx-large-v3"), "{t:?}");
        assert!(t.contains("base-config.hub-port"), "{t:?}");
        assert!(t.contains("note/x-y"), "{t:?}");
        assert!(!t.contains("port") && !t.contains("base") && !t.contains("plain"), "{t:?}");
    }

    #[test]
    fn quoted_passages_are_read_from_every_quote_style() {
        let q = quoted_passages("he said \"use the other one\" and `cargo test` then \u{201c}curly one\u{201d}; \"x\"");
        assert_eq!(q, vec!["use the other one", "cargo test", "curly one"]);
    }

    #[test]
    fn rewriting_max_chars_keeps_comments_and_every_other_key() {
        let before = "# top\n[signal]\nenabled = true\nmax_chars = 2000 # legacy\nstale_days = 14\n\n# budget next\n[memory]\nmode = \"both\"\n";
        let moved = rewrite_max_chars(before, Some(2000)).unwrap();
        assert_eq!(
            moved,
            "# top\n[signal]\nenabled = true\nstale_days = 14\n\n# budget next\n[memory]\nmode = \"both\"\n\n[budget]\nmemory_chars = 2000\n"
        );
        let removed = rewrite_max_chars(before, None).unwrap();
        assert_eq!(removed, "# top\n[signal]\nenabled = true\nstale_days = 14\n\n# budget next\n[memory]\nmode = \"both\"\n");
        // CRLF files keep CRLF.
        let crlf = rewrite_max_chars("[signal]\r\nmax_chars = 2000\r\n[budget]\r\nprompt_bytes = 10000\r\n", Some(1500)).unwrap();
        assert_eq!(crlf, "[signal]\r\n[budget]\r\nprompt_bytes = 10000\r\nmemory_chars = 1500\r\n");
    }
}
