//! `base doctor` — parser-independent graph health diagnosis.
//!
//! Phase 34 of v0.5 Graph Durability. Turns the Phase 33 [`store::graph_health`]
//! primitive into an operator-facing (human) and automation-facing (`--json`)
//! diagnostic. It MUST run *because* the graph is broken, not despite it — so
//! nothing here depends on a successful strict parse except [`entity_composition`],
//! which is a best-effort signal that fails open to an empty list.
//!
//! Test-isolation seam: the per-tier logic lives in the pure [`diagnose_tier`],
//! which only touches the path it is given + that path's siblings. [`diagnose`]
//! adds real tier resolution (incl. the host's global graph) and is therefore
//! NOT unit-testable in isolation — tests target `diagnose_tier` only.

use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;

use anyhow::{Context, Result};
use oxigraph::model::{GraphName, Term};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::changelog::Change;
use crate::store::{self, GraphHealth};

/// Size/line comparison of the current graph against its newest backup.
/// A negative `line_delta` (current smaller than backup) is a data-loss smell.
#[derive(Debug, Serialize)]
pub struct BackupCompare {
    pub path: String,
    pub backup_line_count: usize,
    pub line_delta: i64,
}

/// Health report for a single graph tier.
#[derive(Debug, Serialize)]
pub struct TierReport {
    pub tier: String,
    pub path: String,
    /// "healthy" | "missing" | "unhealthy"
    pub status: String,
    pub reason: Option<String>,
    pub bad_line: Option<usize>,
    pub line_count: usize,
    pub size_bytes: u64,
    pub ends_with_newline: bool,
    pub stale_tmp: bool,
    /// rdf:type → count, populated only when the tier parses (status == "healthy").
    pub entity_composition: Vec<(String, usize)>,
    /// The schema version this tier's graph claims, e.g. `"domain-1"`. `None` on a
    /// tier the domain migration has not reached — which is what makes a skipped
    /// migration visible instead of silent (C4).
    pub schema_version: Option<String>,
    /// Covered classes that still carry no domain link, highest first. Non-empty
    /// after a migration is not a fault: `base sync` writes a Document with no
    /// domain, so the count climbs again until every write path carries the link.
    pub domain_orphans: Vec<(String, usize)>,
    /// Supersession counts and defects for this tier. Default (all zero, no
    /// lists) on a tier that has never used the feature, and doctor then prints
    /// nothing about it — a store from before 0.14.0 reads exactly as it did.
    pub supersede_audit: crate::supersede::Audit,
    /// Named graphs in this tier that belong to ANOTHER workspace, quad count,
    /// highest first (#142). Empty on a clean tier, so a store with nothing
    /// foreign in it serialises and prints exactly as it did before.
    ///
    /// **Counts against `healthy`.** This is the whole point of #142: the only
    /// signal an operator got was `rule list` showing them, while doctor — the
    /// one surface whose job is to say what is wrong — said nothing at all.
    pub foreign_graphs: Vec<(String, usize)>,
    /// Named graphs `base` writes ON PURPOSE that belong to no workspace, so
    /// they are not-own without being a fault. Today exactly one member:
    /// [`crate::apply_ops::LEDGER_GRAPH`], which `apply_ops` documents as living
    /// inside the same `graph.nq` as everything else.
    ///
    /// Reported so nothing is silently dropped, and reported SEPARATELY from
    /// [`Self::unrecognised_graphs`] because an operator who sees
    /// `urn:base:sync:facts` named as a fault goes looking for a corruption that
    /// does not exist. **Advisory: never counted against `healthy`.**
    pub unscoped_graphs: Vec<(String, usize)>,
    /// Every other named graph — not this tier's, not another workspace's, and
    /// not one of the crate's own workspace-independent graphs.
    ///
    /// An ops-sync pull writes portal-supplied partitions here (the `named_graph`
    /// of an incoming op is an arbitrary IRI), which is why this cannot count
    /// against `healthy`: doing so would report every synced machine as unhealthy
    /// over data it fetched correctly. **Advisory.**
    pub unrecognised_graphs: Vec<(String, usize)>,
    pub latest_backup: Option<BackupCompare>,
}

/// Full doctor report across all resolved tiers.
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub tiers: Vec<TierReport>,
    /// True when ALL FIVE of these hold. Missing/empty tiers and files do not
    /// count against health.
    ///
    /// 1. no tier is `"unhealthy"` (i.e. no tier failed to parse),
    /// 2. [`Self::config_errors`] is empty,
    /// 3. [`Self::trigger_faults`] is empty,
    /// 4. no hook is failing **now**, and
    /// 5. no tier carries [`TierReport::foreign_graphs`] (#142).
    ///
    /// This comment enumerates every conjunct on purpose. It previously listed
    /// **two** while the computation had **four**, and anyone rewriting `healthy`
    /// reads the comment rather than counting the `&&`s — see the ruling in the
    /// #142 lane doc. Keep it in step with the expression below or delete it;
    /// a comment that undercounts is worse than none.
    pub healthy: bool,
    /// Advisory operational warnings (bloat, write-probe failures). Do not affect `healthy`.
    pub warnings: Vec<String>,
    /// Config files that exist but cannot be parsed. The loaders that read these
    /// fail open by design, so this is the only surface that reports them — a
    /// corrupt file otherwise looks exactly like an absent one. Counts against
    /// `healthy`: silently-dead star commands are a fault, not an advisory.
    pub config_errors: Vec<String>,
    /// Path triggers that cannot fire (F29 step 6): per tier, a domains.toml trigger that
    /// is unrooted or covers two or more registered projects, with the projects named.
    /// Counts against `healthy`: an inert trigger is a domain that silently stopped
    /// loading, and the fix is one line in domains.toml.
    pub trigger_faults: Vec<String>,
    /// The write seam this binary was built with, [`store::LOCK_SEAM_MARKER`].
    ///
    /// Not diagnostic information for an operator — it is here so a verification
    /// harness can prove WHICH binary it is driving from the binary's own output,
    /// rather than from a filename, an mtime or a directory it was copied to.
    /// Referencing it from a live path is also what keeps the constant reachable
    /// in the linked binary, so a substring probe of the executable means
    /// something.
    pub seam: &'static str,
}

// ─── Provenance: which workspace does a named graph belong to? (#142) ────────

/// Where a named graph found inside a tier came from.
///
/// The shapes below are enumerated **from the codebase**, not from what a graph
/// name looks like it ought to be. A detector whose rule is *anything that is not
/// my own workspace graph is foreign* reports the crate's OWN
/// workspace-independent graphs as corruption — which is #142's defect with the
/// sign flipped, and it would fire on every machine that has run an ops pull.
///
/// Every writer that can put a named graph inside a tier's `graph.nq`:
///
/// | shape | built at | verdict |
/// |---|---|---|
/// | `{ns}graph/ws/{slug}` | [`crate::crud::workspace_graph_iri`] | [`Own`](GraphOrigin::Own) or [`Foreign`](GraphOrigin::Foreign), by the slug |
/// | `{ns}graph/semantic/{ws}/{doc}` | `crud::semantic::doc_graph_iri` | `Own` or `Foreign`, by the `{ws}` segment |
/// | `urn:base:sync:facts` | [`crate::apply_ops::LEDGER_GRAPH`], which `apply_ops` states lives inside the same `graph.nq` | [`Unscoped`](GraphOrigin::Unscoped) |
/// | an arbitrary IRI supplied by the portal | an incoming op's `named_graph`, applied by `apply_ops` | [`Unrecognised`](GraphOrigin::Unrecognised) |
///
/// `{ns_base}/relay/{project}` (`crate::relay::Relay::export_nq`) is deliberately
/// absent rather than forgotten: it writes `inbox.nq` in the relay root and is
/// documented there as an ephemeral snapshot, "never the live medium", so it can
/// never reach a tier graph and therefore never reaches this function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraphOrigin {
    /// This tier's own workspace.
    Own,
    /// Another workspace's. The only variant that counts against `healthy`.
    Foreign,
    /// A graph `base` writes on purpose that belongs to no workspace.
    Unscoped,
    /// Not attributable to any shape this build knows.
    Unrecognised,
}

/// The workspace slug a tier's own quads are stamped with, derived from the tier
/// FILE rather than from the process's cwd.
///
/// Must stay equal to [`crate::crud::workspace_slug`] for both tier shapes —
/// `<root>/.base/graph.nq` and `<home>/.base-gbl/.base/graph.nq`, the latter
/// yielding `base-gbl`. It deliberately does NOT call it: `workspace_slug`
/// resolves through [`crate::config::find_workspace_base`] (`config.rs:49`), which
/// walks up from **cwd** and returns the first `.base` it meets. Calling it here
/// would make this answer about where the process happens to be standing instead
/// of about the file it was handed, and would destroy the purity of the
/// [`diagnose_tier`] seam.
///
/// If the two ever drift, doctor reports 100% of every tier as foreign — the worst
/// failure this feature can have — which is why
/// `tier_own_slug_agrees_with_crud_workspace_slug` pins them together rather than
/// leaving the equivalence as an assumption inside a larger test.
fn tier_own_slug(path: &Path) -> String {
    path.parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .map(crate::crud::slugify)
        .unwrap_or_else(|| "default".into())
}

/// Classify one named graph against the tier holding it.
fn classify_graph(graph: &str, ns_uri: &str, own_slug: &str) -> GraphOrigin {
    if graph == crate::apply_ops::LEDGER_GRAPH {
        return GraphOrigin::Unscoped;
    }
    let Some(rest) = graph.strip_prefix(ns_uri).and_then(|r| r.strip_prefix("graph/")) else {
        return GraphOrigin::Unrecognised;
    };
    // `ws/{slug}` and `semantic/{ws}/{doc}` both carry the owning workspace, in
    // different positions. Any other shape under `graph/` is one this build does
    // not know: say so rather than inventing an owner for it.
    let owner = if let Some(slug) = rest.strip_prefix("ws/") {
        slug
    } else if let Some(tail) = rest.strip_prefix("semantic/") {
        tail.split('/').next().unwrap_or("")
    } else {
        return GraphOrigin::Unrecognised;
    };
    if owner == own_slug { GraphOrigin::Own } else { GraphOrigin::Foreign }
}

/// Every quad in `store` bucketed by the origin of its graph, each bucket
/// **enumerated**.
///
/// Nothing here is derived by subtracting one total from another. Two quantities
/// that count different things, subtracted, print a clean number over a
/// contradiction, and a clamp hides it outright — so every bucket is counted by
/// visiting its members and `own + foreign + unscoped + unrecognised +
/// default_graph` equals the store's quad count exactly. That identity is what
/// `every_quad_lands_in_exactly_one_bucket` asserts.
#[derive(Debug, Default)]
struct GraphProvenance {
    own: usize,
    /// Quads in a graph belonging to another workspace, IRI → count.
    foreign: Vec<(String, usize)>,
    /// Quads in one of the crate's own workspace-independent graphs.
    unscoped: Vec<(String, usize)>,
    /// Quads in a graph no known shape attributes.
    unrecognised: Vec<(String, usize)>,
    /// Quads carrying no graph term at all. `base` writes none; a hand-edited
    /// file can. Counted so the identity above closes, reported nowhere.
    default_graph: usize,
}

/// One pass over the loaded store, grouping by graph name.
///
/// The store is already in memory by the time this runs (`diagnose_tier` loads it
/// for the schema stamp and the supersession audit), so this adds no I/O.
fn graph_provenance(store: &Store, ns_uri: &str, own_slug: &str) -> GraphProvenance {
    use std::collections::BTreeMap;

    let mut out = GraphProvenance::default();
    let mut named: BTreeMap<String, usize> = BTreeMap::new();
    for quad in store.iter().filter_map(Result::ok) {
        match quad.graph_name {
            GraphName::DefaultGraph => out.default_graph += 1,
            GraphName::NamedNode(n) => *named.entry(n.into_string()).or_default() += 1,
            GraphName::BlankNode(b) => *named.entry(format!("_:{}", b.as_str())).or_default() += 1,
        }
    }

    for (graph, count) in named {
        match classify_graph(&graph, ns_uri, own_slug) {
            GraphOrigin::Own => out.own += count,
            GraphOrigin::Foreign => out.foreign.push((graph, count)),
            GraphOrigin::Unscoped => out.unscoped.push((graph, count)),
            GraphOrigin::Unrecognised => out.unrecognised.push((graph, count)),
        }
    }

    // Highest count first, then IRI, so the report is deterministic and the
    // biggest offender is the line an operator reads first.
    let by_size = |v: &mut Vec<(String, usize)>| {
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    };
    by_size(&mut out.foreign);
    by_size(&mut out.unscoped);
    by_size(&mut out.unrecognised);
    out
}

/// Diagnose a single graph file. PURE: only touches `path` and its siblings
/// (`.nq.tmp`, `.bak*`). This is the test-isolation seam — never reaches the
/// real global tier, so it is deterministic under unit test.
pub fn diagnose_tier(tier: &str, path: &Path) -> TierReport {
    let (status, reason, bad_line) = match store::graph_health(path) {
        GraphHealth::Healthy => ("healthy", None, None),
        GraphHealth::Missing => ("missing", None, None),
        GraphHealth::Unhealthy { reason, bad_line } => ("unhealthy", Some(reason), bad_line),
    };

    // A lingering write_back temp means a write was interrupted. Matches both the
    // legacy shared `graph.nq.tmp` and per-writer `graph.nq.tmp.<pid>` temps.
    let stale_tmp = stale_temp_count(path) > 0;

    if status == "missing" {
        return TierReport {
            tier: tier.to_string(),
            path: path.display().to_string(),
            status: status.to_string(),
            reason,
            bad_line,
            line_count: 0,
            size_bytes: 0,
            ends_with_newline: true,
            stale_tmp,
            entity_composition: Vec::new(),
            schema_version: None,
            domain_orphans: Vec::new(),
            // A tier with no file holds no quads, so it holds no foreign ones.
            // Empty here is a measurement, not a default standing in for one.
            foreign_graphs: Vec::new(),
            unscoped_graphs: Vec::new(),
            unrecognised_graphs: Vec::new(),
            latest_backup: None,
            supersede_audit: crate::supersede::Audit::default(),
        };
    }

    let size_bytes = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let line_count = count_lines(path);
    let ends_with_newline = size_bytes == 0 || file_ends_with_newline(path);

    let entity_composition = if status == "healthy" {
        entity_composition(path)
    } else {
        Vec::new()
    };

    // Namespace from THIS tier's own base.toml (`<root>/.base/graph.nq` → `<root>`),
    // so a workspace with a custom prefix is read with its own vocabulary rather
    // than the default. Keeps `diagnose_tier` path-scoped — the test-isolation seam.
    let (schema_version, domain_orphans, supersede_audit, provenance) = if status == "healthy" {
        let root = path.parent().and_then(Path::parent).unwrap_or(path);
        let ns = crate::config::BaseConfig::load(root).namespace;
        match store::load_graph(path) {
            Ok(s) => (
                crate::migrate::stamp_of_tier(&s, &ns),
                crate::migrate::orphan_counts(&s, &ns),
                crate::supersede::audit(&s, &ns),
                // #142. Same already-loaded store, so this costs one more pass
                // over memory and no I/O at all.
                graph_provenance(&s, &ns.uri, &tier_own_slug(path)),
            ),
            Err(_) => (None, Vec::new(), crate::supersede::Audit::default(), GraphProvenance::default()),
        }
    } else {
        // An unparseable tier is already `unhealthy` for a stated reason with a
        // bad line number. Claiming a provenance verdict from a store that never
        // loaded would put a confident zero where the honest answer is "could not
        // look" — the failure this whole lane exists to remove.
        (None, Vec::new(), crate::supersede::Audit::default(), GraphProvenance::default())
    };

    let latest_backup = newest_backup(path).map(|bpath| {
        let backup_line_count = count_lines(&bpath);
        BackupCompare {
            path: bpath.display().to_string(),
            backup_line_count,
            line_delta: line_count as i64 - backup_line_count as i64,
        }
    });

    TierReport {
        tier: tier.to_string(),
        path: path.display().to_string(),
        status: status.to_string(),
        reason,
        bad_line,
        line_count,
        size_bytes,
        ends_with_newline,
        stale_tmp,
        entity_composition,
        supersede_audit,
        schema_version,
        domain_orphans,
        foreign_graphs: provenance.foreign,
        unscoped_graphs: provenance.unscoped,
        unrecognised_graphs: provenance.unrecognised,
        latest_backup,
    }
}

/// Diagnose both graph tiers (global `~/.base-gbl/.base/graph.nq`, then the
/// nearest workspace `.base/graph.nq`). Mirrors the tier walk in
/// `hook::session_start::warn_unhealthy_graphs`. NOT unit-tested (reads the real
/// global tier) — see module docs.
pub fn diagnose(cwd: &Path) -> DoctorReport {
    let mut warnings = Vec::new();
    let tiers: Vec<TierReport> = tier_paths(cwd)
        .into_iter()
        .map(|(tier, path)| {
            let report = diagnose_tier(&tier, &path);
            warnings.extend(bloat_warnings(&report));
            if report.status != "missing"
                && let Some(err) = write_probe(&path) {
                    warnings.push(format!("{tier} tier write probe FAILED — {err}"));
                }
            report
        })
        .collect();
    warnings.extend(leaked_global_handoffs());
    warnings.extend(coach_drift());
    // #20: a failed hook is invisible everywhere else (fail-open by design); doctor names it.
    // The cwd PARAM, not the process cwd: `diagnose` is called with a path and
    // shadowing it with `std::env::current_dir()` made this section untestable and
    // wrong for any caller that does not chdir first.
    let mut hooks_broken = false;
    {
        for (tier, base_dir) in crate::hook::hook_log_dirs(cwd) {
            if let Some(t) = crate::hook::hook_failure_summary(&base_dir) {
                warnings.push(format!("{tier} tier {}", t.summary));
                // Only a hook that is failing NOW is a fault; an older failure with
                // successes after it is reported and forgiven.
                hooks_broken |= t.broken_now;
            }
        }
    }
    let config_errors = crate::command::check_command_files(cwd);
    let trigger_faults = trigger_faults(cwd);
    // FIVE conjuncts. Keep the doc comment on `DoctorReport::healthy` in step
    // with this expression — it undercounted for four releases (#142).
    let healthy = tiers.iter().all(|t| t.status != "unhealthy")
        && config_errors.is_empty()
        && trigger_faults.is_empty()
        && !hooks_broken
        // #142: a tier full of quads belonging to another workspace parses
        // perfectly, so nothing above can see it. Only `unscoped` and
        // `unrecognised` stay advisory — those are graphs base writes on purpose.
        && tiers.iter().all(|t| t.foreign_graphs.is_empty());
    DoctorReport {
        tiers,
        healthy,
        warnings,
        config_errors,
        trigger_faults,
        seam: store::LOCK_SEAM_MARKER,
    }
}

/// Every inert path trigger, per tier, as the sentence `add-trigger` refuses with
/// (F29 step 6). Each tier is read from its own domains.toml and resolved against its
/// own root, against the registered projects of the merged store.
fn trigger_faults(cwd: &Path) -> Vec<String> {
    let ctx = crate::domain::trigger_context(cwd);
    let mut tiers: Vec<(&str, PathBuf, Option<PathBuf>)> = Vec::new();
    if let Some(home) = crate::home::home_root() {
        tiers.push(("global", home.join(".base-gbl").join("domains.toml"), Some(home)));
    }
    if let Some(base_dir) = crate::config::find_workspace_base(cwd) {
        let root = base_dir.parent().map(Path::to_path_buf);
        tiers.push(("workspace", base_dir.join("domains.toml"), root));
    }
    let mut out = Vec::new();
    for (tier, path, root) in tiers {
        let domains = crate::domain::load_domains_file(&path, root.as_deref());
        for (domain, trigger, fault) in crate::domain::matcher::inert_triggers(&domains, &ctx) {
            out.push(format!("{tier} tier: {}", crate::domain::matcher::fault_sentence(domain, trigger, &fault)));
        }
    }
    out
}

/// Render a clearly-delimited human report.
pub fn format_human(report: &DoctorReport) -> String {
    let mut out = String::new();
    out.push_str("═══════════════════════════════════════\n");
    out.push_str("base doctor — graph health\n");
    out.push_str("═══════════════════════════════════════\n");

    if report.tiers.is_empty() {
        out.push_str("No graph tiers found (no .base/graph.nq in workspace or global tier).\n");
        // A corrupt config is still worth reporting with no graph present — it is
        // the reason star commands went quiet, and it has nothing to do with tiers.
        for e in &report.config_errors {
            out.push_str(&format!("   ⚠ {e}\n"));
        }
        for t in &report.trigger_faults {
            out.push_str(&format!("   ⚠ {t}\n"));
        }
        // Same reasoning for advisories: a coach lagging the binary is true
        // whether or not a graph exists here, and this early return used to
        // swallow it entirely.
        for w in &report.warnings {
            out.push_str(&format!("   ⚠ {w}\n"));
        }
        return out;
    }

    for t in &report.tiers {
        let mark = match t.status.as_str() {
            "healthy" => "✓",
            "missing" => "·",
            _ => "⚠",
        };
        out.push_str(&format!("\n{mark} {} tier — {}\n", t.tier, t.status.to_uppercase()));
        out.push_str(&format!("   {}\n", t.path));

        if let Some(reason) = &t.reason {
            let line = t.bad_line.map(|n| format!(" (line {n})")).unwrap_or_default();
            out.push_str(&format!("   reason: {reason}{line}\n"));
        }

        if t.status != "missing" {
            let nl = if t.ends_with_newline {
                "ends with newline"
            } else {
                "⚠ NO trailing newline (truncated write?)"
            };
            out.push_str(&format!("   {} lines · {} bytes · {nl}\n", t.line_count, t.size_bytes));
        }

        if t.stale_tmp {
            out.push_str("   ⚠ stale graph.nq.tmp present — a write_back was interrupted\n");
        }

        if !t.entity_composition.is_empty() {
            let comp: Vec<String> = t
                .entity_composition
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            out.push_str(&format!("   composition: {}\n", comp.join(", ")));
        }

        // #142. Quads belonging to another workspace, named with their counts.
        // A message saying "foreign quads present" without the graph and the
        // number is not a diagnosis, it is a rumour.
        if !t.foreign_graphs.is_empty() {
            let total: usize = t.foreign_graphs.iter().map(|(_, n)| n).sum();
            out.push_str(&format!(
                "   ⚠ {total} quad(s) in {} graph(s) belonging to ANOTHER workspace:\n",
                t.foreign_graphs.len()
            ));
            for (g, n) in &t.foreign_graphs {
                out.push_str(&format!("       {n} · {g}\n"));
            }
            // The one bounded extra line auk ruled in scope. Fires ONLY for the
            // shape where every quad in the tier sits in a single non-own graph,
            // because reporting "N foreign quads" when the cause is a renamed
            // folder sends the operator hunting corruption that is not there.
            // Diagnosis only: it names the likely cause and stops.
            if t.foreign_graphs.len() == 1 && total == t.line_count && total > 0 {
                out.push_str(
                    "       every quad in this tier is in that one graph — \
                     this workspace directory was most likely renamed\n",
                );
            }
        }

        // Advisory, and deliberately worded so neither reads as corruption.
        // `urn:base:sync:facts` is base's own ledger; an ops pull writes
        // portal-supplied partitions. Naming either as a fault would send an
        // operator looking for damage that does not exist.
        if !t.unscoped_graphs.is_empty() {
            let total: usize = t.unscoped_graphs.iter().map(|(_, n)| n).sum();
            out.push_str(&format!(
                "   {total} quad(s) in {} of base's own workspace-independent graph(s) — normal\n",
                t.unscoped_graphs.len()
            ));
        }
        if !t.unrecognised_graphs.is_empty() {
            let total: usize = t.unrecognised_graphs.iter().map(|(_, n)| n).sum();
            out.push_str(&format!(
                "   {total} quad(s) in {} graph(s) this build does not attribute \
                 (an ops-sync pull writes portal-supplied graphs here) — advisory, not a fault\n",
                t.unrecognised_graphs.len()
            ));
            for (g, n) in &t.unrecognised_graphs {
                out.push_str(&format!("       {n} · {g}\n"));
            }
        }

        // The domain schema, always stated when the tier parses. A migration that
        // never ran must not look the same as one that ran and found nothing.
        if t.status == "healthy" {
            let orphans: usize = t.domain_orphans.iter().map(|(_, n)| n).sum();
            match t.schema_version.as_deref() {
                Some(v) => out.push_str(&format!("   schema: {v}\n")),
                None => out.push_str(
                    "   schema: not migrated — the domain backfill runs at the next session start\n",
                ),
            }
            // Supersession, reported ONLY when there is something to report, so a
            // store that has never used the feature prints exactly what it printed
            // before. Counts and defects, never a judgement: doctor is where an
            // operator goes looking for problems.
            let a = &t.supersede_audit;
            if !a.is_silent() {
                out.push_str(&format!("   superseded: {} record(s)\n", a.superseded));
                if a.corrections_naming_nothing > 0 {
                    // The answer to G0 Q1: reported here once instead of nagging on
                    // every `learn --type correction`.
                    out.push_str(&format!(
                        "   {} correction(s) name nothing they correct\n",
                        a.corrections_naming_nothing
                    ));
                }
                if a.status_without_edge > 0 || a.edge_without_status > 0 {
                    // Two numbers, not one: status-without-edge is a pre-0.14.0
                    // artefact, edge-without-status is a writer that half-ran.
                    out.push_str(&format!(
                        "   supersession disagreement: {} with the status and no edge, \
                         {} with the edge and no status\n",
                        a.status_without_edge, a.edge_without_status
                    ));
                }
                if !a.long_chains.is_empty() {
                    out.push_str(&format!(
                        "   {} chain(s) longer than 3 links — first: {}\n",
                        a.long_chains.len(),
                        a.long_chains.first().map(String::as_str).unwrap_or("")
                    ));
                }
                if !a.cycles.is_empty() {
                    // A defect, not a warning: the writer refuses to create one, so a
                    // cycle here arrived by another route and `resolve_head` is
                    // returning an arbitrary member of it as the live version.
                    out.push_str(&format!(
                        "   DEFECT: {} supersession cycle(s) — first: {}\n",
                        a.cycles.len(),
                        a.cycles.first().map(String::as_str).unwrap_or("")
                    ));
                }
            }

            if orphans > 0 {
                let top: Vec<String> = t
                    .domain_orphans
                    .iter()
                    .take(6)
                    .map(|(k, n)| format!("{k}={n}"))
                    .collect();
                let more = t.domain_orphans.len().saturating_sub(6);
                out.push_str(&format!(
                    "   without a domain: {orphans} record(s) — {}{}\n",
                    top.join(", "),
                    if more > 0 { format!(", +{more} more kind(s)") } else { String::new() },
                ));
            }
        }

        if let Some(b) = &t.latest_backup {
            if b.line_delta < 0 {
                out.push_str(&format!(
                    "   ⚠ {} lines smaller than newest backup ({} lines) — possible data loss [{}]\n",
                    -b.line_delta, b.backup_line_count, b.path,
                ));
            } else {
                out.push_str(&format!(
                    "   backup: {} lines (current +{} vs backup) [{}]\n",
                    b.backup_line_count, b.line_delta, b.path,
                ));
            }
        }
    }

    if !report.config_errors.is_empty() {
        out.push_str("\n─── config faults ────────────────────\n");
        for e in &report.config_errors {
            out.push_str(&format!("   ⚠ {e}\n"));
        }
    }

    if !report.trigger_faults.is_empty() {
        out.push_str("\n─── path triggers ────────────────────\n");
        for t in &report.trigger_faults {
            out.push_str(&format!("   ⚠ {t}\n"));
        }
    }

    if !report.warnings.is_empty() {
        out.push_str("\n─── advisories ───────────────────────\n");
        for w in &report.warnings {
            out.push_str(&format!("   ⚠ {w}\n"));
        }
    }

    let verdict = if report.healthy {
        "Verdict: HEALTHY ✓"
    } else {
        "Verdict: UNHEALTHY ⚠ — repair before relying on recall / learn / sync"
    };
    out.push_str(&format!("\n{verdict}\n"));
    out
}

/// Count lines via a streaming reader (never loads the whole file at once).
fn count_lines(path: &Path) -> usize {
    match fs::File::open(path) {
        Ok(f) => BufReader::new(f).lines().map_while(Result::ok).count(),
        Err(_) => 0,
    }
}

/// True if the file's final byte is a newline. Fail-open to `true` so a read
/// glitch never masquerades as a truncated write.
fn file_ends_with_newline(path: &Path) -> bool {
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return true,
    };
    if f.seek(SeekFrom::End(-1)).is_err() {
        return true;
    }
    let mut byte = [0u8; 1];
    match f.read_exact(&mut byte) {
        Ok(()) => byte[0] == b'\n',
        Err(_) => true,
    }
}

/// rdf:type composition of a healthy graph. Best-effort: any parse/query error
/// yields an empty list (never panics, never blocks the report).
fn entity_composition(path: &Path) -> Vec<(String, usize)> {
    let store = match store::load_graph(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let sparql =
        "SELECT ?type (COUNT(?s) AS ?n) WHERE { ?s a ?type } GROUP BY ?type ORDER BY DESC(?n)";
    let results = match store::query(&store, sparql) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    if let QueryResults::Solutions(solutions) = results {
        for sol in solutions.flatten() {
            let name = sol.get("type").map(local_name).unwrap_or_default();
            let count = sol.get("n").and_then(term_count).unwrap_or(0);
            if !name.is_empty() {
                out.push((name, count));
            }
        }
    }
    out
}

/// Local name of a NamedNode (after the last `#` or `/`).
fn local_name(term: &Term) -> String {
    match term {
        Term::NamedNode(n) => {
            let iri = n.as_str();
            iri.rfind('#')
                .or_else(|| iri.rfind('/'))
                .map(|pos| iri[pos + 1..].to_string())
                .unwrap_or_else(|| iri.to_string())
        }
        _ => String::new(),
    }
}

/// Parse a COUNT literal into a usize.
fn term_count(term: &Term) -> Option<usize> {
    match term {
        Term::Literal(l) => l.value().parse::<usize>().ok(),
        _ => None,
    }
}

/// Newest sibling backup file matching `{graph-name}.bak*`, if any.
fn newest_backup(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let fname = path.file_name()?.to_str()?;
    let prefix = format!("{fname}.bak");

    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(parent).ok()?.flatten() {
        let p = entry.path();
        let is_bak = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with(&prefix))
            .unwrap_or(false);
        if !is_bak {
            continue;
        }
        if let Some(mt) = entry.metadata().ok().and_then(|m| m.modified().ok()) {
            match &newest {
                Some((best, _)) if *best >= mt => {}
                _ => newest = Some((mt, p)),
            }
        }
    }
    newest.map(|(_, p)| p)
}

/// Filesystem-safe local timestamp for backup/quarantine filenames.
fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d-%H%M%S").to_string()
}

// ─── base-help coach drift ────────────────────────────────────────────────

/// Advisory when the installed base-help coach has fallen behind the binary.
/// PURE — no filesystem, so the wording is unit-testable.
///
/// The coach is base's own answer surface. Before `base update` learned to
/// refresh skills, updating the binary left it frozen at whatever release last
/// ran a full `base install`, and nothing said so: the operator got confident
/// answers describing the previous version of the CLI. This is the surface that
/// makes that visible, and it is the ONLY reader the background update path has,
/// which is why it is a report rather than a prompt.
///
/// Advisory only — it never counts against `healthy`. A stale coach is wrong
/// documentation, not a broken graph.
pub fn skill_drift_warning(
    bank_version: Option<&str>,
    binary_version: &str,
    skill_installed: bool,
) -> Option<String> {
    match bank_version {
        // Registered and in step: nothing to say.
        Some(v) if v == binary_version => None,
        Some("unstamped") => Some(
            "base-help coach carries no version stamp in references/qa.md — cannot tell whether it matches this binary; run `base install` to reinstall it"
                .to_string(),
        ),
        Some(v) => Some(format!(
            "base-help coach is stamped for base v{v} but this binary is {binary_version} — its answers may describe the previous release; run `base update` (or `base install`) to refresh it"
        )),
        // On disk but never registered: an old install, or a hand-copied skill.
        None if skill_installed => Some(
            "base-help coach is installed but not registered in manifest.toml — version drift cannot be detected; run `base install` to register it"
                .to_string(),
        ),
        None => None,
    }
}

/// Read the two facts `skill_drift_warning` compares, then apply it.
fn coach_drift() -> Option<String> {
    let home = crate::home::home_root()?;
    let installed = home
        .join(".claude")
        .join("skills")
        .join("base-help")
        .join("references")
        .join("qa.md")
        .is_file();
    let recorded = crate::manifest::Manifest::load()
        .and_then(|m| m.components.get("base-help").map(|c| c.version.clone()));
    skill_drift_warning(recorded.as_deref(), env!("CARGO_PKG_VERSION"), installed)
}

// ─── Operational health: stale-temp, bloat, write-probe ───────────────────

/// Count lingering write_back temp files for this graph — both the legacy shared
/// `graph.nq.tmp` and per-writer `graph.nq.tmp.<pid>`. Any present = an interrupted write.
fn stale_temp_count(path: &Path) -> usize {
    let (Some(parent), Some(fname)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
    else {
        return 0;
    };
    let prefix = format!("{fname}.tmp");
    fs::read_dir(parent)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .map(|n| n.starts_with(&prefix))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

/// Count + total bytes of `{name}.bak*` sibling backups.
fn backup_footprint(path: &Path) -> (usize, u64) {
    let (Some(parent), Some(fname)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
    else {
        return (0, 0);
    };
    let prefix = format!("{fname}.bak");
    let mut count = 0usize;
    let mut bytes = 0u64;
    if let Ok(rd) = fs::read_dir(parent) {
        for e in rd.flatten() {
            if e
                .file_name()
                .to_str()
                .map(|n| n.starts_with(&prefix))
                .unwrap_or(false)
            {
                count += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (count, bytes)
}

/// Advisory size/backup bloat warnings for a tier. Never affect `healthy` — a big
/// graph is a smell (slow writes widen the write-race window), not a failure.
/// Open handoffs/forks sitting in the GLOBAL tier are almost always leaks from
/// the pre-#8 fallback, which wrote them there whenever a session ran outside a
/// workspace — and a global handoff then resurfaces at the start of every
/// unrelated project. The user has no way to suspect this without being told,
/// so surface it with the exact command that stops it.
fn leaked_global_handoffs() -> Vec<String> {
    let Some(home) = crate::home::home_root() else {
        return Vec::new();
    };
    leaked_handoffs_in(&home.join(".base-gbl").join(".base").join("graph.nq"))
}

/// PURE seam for [`leaked_global_handoffs`]: only touches `gbl`, so it never
/// reaches the real global tier under test.
fn leaked_handoffs_in(gbl: &Path) -> Vec<String> {
    if !gbl.exists() {
        return Vec::new();
    }
    let Ok(store) = store::load_graph(gbl) else {
        return Vec::new();
    };
    // Namespace-agnostic: match on the type's local name so a customized
    // prefix/URI still reports.
    let sparql = "SELECT ?h WHERE { GRAPH ?g { ?h a ?t ; ?statusP \"open\" . \
                  FILTER(STRENDS(STR(?t), \"Handoff\")) \
                  FILTER(STRENDS(STR(?statusP), \"status\")) } }";
    let Ok(QueryResults::Solutions(sols)) = store::query(&store, sparql) else {
        return Vec::new();
    };
    let slugs: Vec<String> = sols
        .filter_map(|r| r.ok())
        .filter_map(|row| row.get("h").map(local_name))
        .collect();
    if slugs.is_empty() {
        return Vec::new();
    }
    vec![format!(
        "{} open handoff/fork(s) in the GLOBAL tier — these resurface in every project. \
         Likely leaks from writes made outside a workspace. Review with `base handoff list` \
         and stop each with `base handoff archive <slug>`: {}",
        slugs.len(),
        slugs.join(", ")
    )]
}

fn bloat_warnings(tier: &TierReport) -> Vec<String> {
    const BLOAT_GRAPH_BYTES: u64 = 20 * 1024 * 1024;
    const BLOAT_BACKUP_COUNT: usize = 5;
    const BLOAT_BACKUP_BYTES: u64 = 50 * 1024 * 1024;
    let mut w = Vec::new();
    if tier.status == "missing" {
        return w;
    }
    if tier.size_bytes > BLOAT_GRAPH_BYTES {
        w.push(format!(
            "{} graph is {} MB / {} lines — consider `base graph compact`",
            tier.tier,
            tier.size_bytes / (1024 * 1024),
            tier.line_count
        ));
    }
    let (count, bytes) = backup_footprint(Path::new(&tier.path));
    if count > BLOAT_BACKUP_COUNT || bytes > BLOAT_BACKUP_BYTES {
        w.push(format!(
            "{} tier keeps {} backups ({} MB) — prune old .bak files",
            tier.tier,
            count,
            bytes / (1024 * 1024)
        ));
    }
    w
}

/// Exercise the write path the same way `write_back` does — a temp file + atomic
/// rename in the tier directory — without touching the real graph. Catches a
/// read-only FS, bad permissions, or a full disk that the read-only `graph_health`
/// can't see. Returns `Some(error)` on failure.
fn write_probe(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    let tmp = parent.join(format!(".doctor-write-probe.{}", std::process::id()));
    let dst = parent.join(".doctor-write-probe");
    let result = (|| -> std::io::Result<()> {
        fs::write(&tmp, b"probe\n")?;
        fs::rename(&tmp, &dst)?;
        fs::remove_file(&dst)?;
        Ok(())
    })();
    let _ = fs::remove_file(&tmp);
    let _ = fs::remove_file(&dst);
    result.err().map(|e| e.to_string())
}

// ─── Repair (Phase 35, GRAPH-DURABILITY §4 Layer 3) ────────────────────────

/// Result of a `doctor --repair` pass over one tier.
#[derive(Debug, Serialize)]
pub struct RepairOutcome {
    pub tier: String,
    pub path: String,
    /// Pre-repair snapshot written before any mutation (None when nothing was repaired).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    /// Quarantine file holding the skipped malformed lines (None when none were skipped).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quarantine: Option<String>,
    /// Good quads written back.
    pub kept: usize,
    /// Malformed lines moved to quarantine.
    pub quarantined: usize,
    /// graph_health == Healthy after the rewrite.
    pub healthy_after: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Repair a single tier: back up, lenient-load the good set, quarantine the bad
/// lines, atomically rewrite via [`store::write_back`], re-verify. PURE/path-scoped
/// (like [`diagnose_tier`]) — the test-isolation seam. A Healthy tier is a no-op
/// (the file is left byte-for-byte untouched). Missing is a no-op too.
pub fn repair_tier(tier: &str, path: &Path) -> Result<RepairOutcome> {
    let base = |backup, quarantine, kept, quarantined, healthy_after| RepairOutcome {
        tier: tier.to_string(),
        path: path.display().to_string(),
        backup,
        quarantine,
        kept,
        quarantined,
        healthy_after,
        error: None,
    };

    match store::graph_health(path) {
        GraphHealth::Missing => Ok(base(None, None, 0, 0, true)),
        GraphHealth::Healthy => Ok(base(None, None, count_lines(path), 0, true)),
        GraphHealth::Unhealthy { .. } => {
            // #87: the lock is taken before the snapshot, so the backup, the
            // lenient re-parse and the rewrite all describe one file.
            //
            // NOT `lock_and_load_graph`: this arm runs on a graph that does not
            // parse, so a load here would fail on exactly the file the repair
            // exists to fix. `lock_for_rebuild` takes the lock and records the
            // identity without loading, and the rebuilt store goes through
            // `write_store`.
            let locked = store::lock_for_rebuild(path)?;

            // 1. Snapshot first (rotating, in-binary backup — Phase 36 store::snapshot).
            let backup_path = store::snapshot(path, "pre-repair")
                .context("failed to back up before repair")?;

            // 2. Recover the good quads; collect the bad lines.
            let (good, bad) = store::load_graph_lenient(path)?;

            // 3. Quarantine the malformed lines verbatim (only if any).
            let quarantine_path = if bad.is_empty() {
                None
            } else {
                let qpath = path.with_file_name(format!(
                    "{}.quarantine-{}",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("graph.nq"),
                    stamp()
                ));
                let body: String =
                    bad.iter().map(|b| format!("{}\n", b.text)).collect();
                fs::write(&qpath, body)
                    .with_context(|| format!("failed to write quarantine {}", qpath.display()))?;
                Some(qpath.display().to_string())
            };

            // 4. Atomic rewrite of the good set (temp → validate → rename).
            locked.write_store(&good, Change::Op("doctor.repair"))?;

            // 5. Re-verify.
            let healthy_after = matches!(store::graph_health(path), GraphHealth::Healthy);

            Ok(RepairOutcome {
                tier: tier.to_string(),
                path: path.display().to_string(),
                backup: Some(backup_path.display().to_string()),
                quarantine: quarantine_path,
                kept: good.len().unwrap_or(0),
                quarantined: bad.len(),
                healthy_after,
                error: None,
            })
        }
    }
}

/// Repair every resolved tier (same walk as [`diagnose`]). Errors are captured per
/// tier (never panic), so one failing tier does not abort the others.
pub fn repair(cwd: &Path) -> Vec<RepairOutcome> {
    tier_paths(cwd)
        .into_iter()
        .map(|(tier, path)| {
            repair_tier(&tier, &path).unwrap_or_else(|e| RepairOutcome {
                tier,
                path: path.display().to_string(),
                backup: None,
                quarantine: None,
                kept: 0,
                quarantined: 0,
                healthy_after: false,
                error: Some(e.to_string()),
            })
        })
        .collect()
}

/// Human report for a repair pass.
pub fn format_repair_human(outcomes: &[RepairOutcome]) -> String {
    let mut out = String::new();
    out.push_str("═══════════════════════════════════════\n");
    out.push_str("base doctor --repair\n");
    out.push_str("═══════════════════════════════════════\n");
    for o in outcomes {
        out.push_str(&format!("\n{} tier — {}\n", o.tier, o.path));
        if let Some(e) = &o.error {
            out.push_str(&format!("   ⚠ repair failed: {e}\n"));
            continue;
        }
        if let Some(b) = &o.backup {
            out.push_str(&format!("   backup: {b}\n"));
        }
        if let Some(q) = &o.quarantine {
            out.push_str(&format!("   quarantined {} line(s) → {q}\n", o.quarantined));
        }
        out.push_str(&format!("   kept {} quad(s)\n", o.kept));
        let v = if o.healthy_after {
            "✓ healthy after repair"
        } else if o.backup.is_none() {
            "· nothing to repair"
        } else {
            "⚠ STILL UNHEALTHY after repair — consider doctor --restore"
        };
        out.push_str(&format!("   {v}\n"));
    }
    out
}

// ─── Restore (Phase 35) ────────────────────────────────────────────────────

/// Backup snapshots for a tier: sibling `{fname}.bak*` files with line counts,
/// newest first.
pub fn list_backups(path: &Path) -> Vec<(PathBuf, usize)> {
    let (parent, prefix) = match (path.parent(), path.file_name().and_then(|n| n.to_str())) {
        (Some(p), Some(f)) => (p, format!("{f}.bak")),
        _ => return Vec::new(),
    };
    let mut baks: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let Ok(rd) = fs::read_dir(parent) else {
        return Vec::new();
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let is_bak = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with(&prefix))
            .unwrap_or(false);
        if !is_bak {
            continue;
        }
        if let Some(mt) = entry.metadata().ok().and_then(|m| m.modified().ok()) {
            baks.push((mt, p));
        }
    }
    baks.sort_by_key(|b| std::cmp::Reverse(b.0)); // newest first
    baks.into_iter().map(|(_, p)| {
        let n = count_lines(&p);
        (p, n)
    }).collect()
}

/// Restore `path` from `backup`. Snapshots the CURRENT file first (so a wrong
/// restore is itself recoverable), then swaps the backup into place via the same
/// temp→rename discipline as `write_back` (never hand-writes the live file).
pub fn restore_tier(path: &Path, backup: &Path) -> Result<()> {
    if !backup.exists() {
        anyhow::bail!("backup not found: {}", backup.display());
    }
    if path.exists() {
        store::snapshot(path, "pre-restore")
            .context("failed to back up current graph before restore")?;
    }
    let tmp = path.with_extension("nq.tmp");
    fs::copy(backup, &tmp)
        .with_context(|| format!("failed to stage restore from {}", backup.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to swap {} into place", path.display()))?;
    Ok(())
}

/// Resolve the (tier, graph.nq path) pairs the same way [`diagnose`] walks them:
/// global first, then the nearest workspace, deduped by canonical path.
fn tier_paths(cwd: &Path) -> Vec<(String, PathBuf)> {
    let mut tiers = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if let Some(home) = crate::home::home_root() {
        let global = home.join(".base-gbl").join(".base").join("graph.nq");
        if global.exists() {
            let key = fs::canonicalize(&global).unwrap_or_else(|_| global.clone());
            if seen.insert(key) {
                tiers.push(("global".to_string(), global));
            }
        }
    }

    if let Some(ws) = crate::config::walk_up(cwd, |dir| {
        let ws = dir.join(".base").join("graph.nq");
        ws.exists().then_some(ws)
    }) {
        let key = fs::canonicalize(&ws).unwrap_or_else(|_| ws.clone());
        if seen.insert(key) {
            tiers.push(("workspace".to_string(), ws));
        }
    }
    tiers
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // A well-formed N-Quads statement with an rdf:type so entity_composition is non-empty.
    const TYPED: &str = "<http://example.org/s> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Thing> .\n";

    fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        p
    }

    // ─── #142 provenance fixtures ────────────────────────────────────────────

    /// The default namespace URI, read from the config rather than typed here so
    /// this cannot drift from what the product builds IRIs with.
    fn nsuri() -> String {
        crate::config::NamespaceConfig::default().uri
    }

    /// A realistic tier layout, `<root>/.base/graph.nq`, returned with the slug
    /// that layout gives the tier.
    ///
    /// The existing fixtures above write `graph.nq` loose in a tempdir, which is
    /// fine for parse-shaped assertions and wrong for provenance ones:
    /// `tier_own_slug` reads the file's GRANDPARENT, so a loose file is attributed
    /// to the system temp folder and every quad in it would read foreign.
    fn tier_at(name: &str) -> (tempfile::TempDir, PathBuf, String) {
        let td = tempfile::tempdir().unwrap();
        let base = td.path().join(name).join(".base");
        fs::create_dir_all(&base).unwrap();
        (td, base.join("graph.nq"), crate::crud::slugify(name))
    }

    /// One well-formed quad in graph `g`.
    fn quad_in(subject: &str, g: &str) -> String {
        format!(
            "<http://example.org/{subject}> \
             <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <http://example.org/Thing> <{g}> .\n"
        )
    }

    fn ws_graph(slug: &str) -> String {
        format!("{}graph/ws/{slug}", nsuri())
    }

    /// A1 — POSITIVE CONTROL. The detector can fire, and it names both the graph
    /// and the count. A message without the number is a rumour, not a diagnosis.
    #[test]
    fn foreign_quads_are_named_with_their_count() {
        let (_td, p, own) = tier_at("mine");
        let other = ws_graph("theirs");
        let mut body = quad_in("a", &ws_graph(&own));
        body.push_str(&quad_in("b", &other));
        body.push_str(&quad_in("c", &other));
        write_file(p.parent().unwrap(), "graph.nq", &body);

        let r = diagnose_tier("workspace", &p);
        assert_eq!(r.foreign_graphs, vec![(other, 2)]);
        assert!(r.unscoped_graphs.is_empty());
        assert!(r.unrecognised_graphs.is_empty());
        // The tier still PARSES, which is exactly why nothing before #142 saw it.
        // Overloading `status` would switch off composition, schema and the
        // supersession audit on the one tier an operator is trying to diagnose.
        assert_eq!(r.status, "healthy", "provenance is not a parse fault");
    }

    /// A2 — NEGATIVE CONTROL, and the one most likely to be skipped. Without it a
    /// detector that reports foreign quads unconditionally passes A1 identically.
    #[test]
    fn a_clean_tier_reports_no_foreign_graphs() {
        let (_td, p, own) = tier_at("mine");
        let body = quad_in("a", &ws_graph(&own)) + &quad_in("b", &ws_graph(&own));
        write_file(p.parent().unwrap(), "graph.nq", &body);

        let r = diagnose_tier("workspace", &p);
        assert!(r.foreign_graphs.is_empty(), "a clean tier must stay silent");
        assert!(r.unscoped_graphs.is_empty());
        assert!(r.unrecognised_graphs.is_empty());
    }

    /// A3 — base's own ledger is NOT foreign. `apply_ops` documents it as living
    /// inside the same `graph.nq`; naming it a fault sends an operator hunting a
    /// corruption that does not exist.
    #[test]
    fn the_sync_ledger_graph_is_not_foreign() {
        let (_td, p, own) = tier_at("mine");
        let body = quad_in("a", &ws_graph(&own)) + &quad_in("f", crate::apply_ops::LEDGER_GRAPH);
        write_file(p.parent().unwrap(), "graph.nq", &body);

        let r = diagnose_tier("workspace", &p);
        assert!(r.foreign_graphs.is_empty(), "the ledger is base's own");
        assert_eq!(r.unscoped_graphs, vec![(crate::apply_ops::LEDGER_GRAPH.to_string(), 1)]);
        assert!(r.unrecognised_graphs.is_empty());
    }

    /// A4 — a portal-supplied partition is UNRECOGNISED, never foreign. An
    /// incoming op's `named_graph` is an arbitrary IRI, so counting these against
    /// health would report every ops-synced machine as unhealthy over data it
    /// fetched correctly.
    #[test]
    fn a_portal_partition_is_unrecognised_not_foreign() {
        let (_td, p, own) = tier_at("mine");
        let portal = "https://basemode.ai/g/6f1c";
        let body = quad_in("a", &ws_graph(&own)) + &quad_in("p", portal);
        write_file(p.parent().unwrap(), "graph.nq", &body);

        let r = diagnose_tier("workspace", &p);
        assert!(r.foreign_graphs.is_empty());
        assert!(r.unscoped_graphs.is_empty());
        assert_eq!(r.unrecognised_graphs, vec![(portal.to_string(), 1)]);
    }

    /// A5 — the second shape that carries a workspace. A detector that knows only
    /// `graph/ws/` is blind to a whole family it claims to cover.
    #[test]
    fn a_foreign_semantic_graph_is_detected() {
        let (_td, p, own) = tier_at("mine");
        let g = format!("{}graph/semantic/theirs/some-doc", nsuri());
        let body = quad_in("a", &ws_graph(&own)) + &quad_in("s", &g);
        write_file(p.parent().unwrap(), "graph.nq", &body);

        let r = diagnose_tier("workspace", &p);
        assert_eq!(r.foreign_graphs, vec![(g, 1)]);
    }

    /// A6 — the same shape with THIS tier's slug is own, not foreign. Pairs with
    /// A5: one mutation, opposite outcome.
    #[test]
    fn the_tiers_own_semantic_graph_is_not_foreign() {
        let (_td, p, own) = tier_at("mine");
        let g = format!("{}graph/semantic/{own}/some-doc", nsuri());
        let body = quad_in("a", &ws_graph(&own)) + &quad_in("s", &g);
        write_file(p.parent().unwrap(), "graph.nq", &body);

        let r = diagnose_tier("workspace", &p);
        assert!(r.foreign_graphs.is_empty(), "our own semantic graph is ours");
        assert!(r.unrecognised_graphs.is_empty());
    }

    /// A7 — every quad lands in exactly one bucket, and the buckets are counted by
    /// visiting their members. No bucket is derived by subtracting one total from
    /// another: a subtraction between quantities that count different things
    /// prints a clean number over a contradiction.
    #[test]
    fn every_quad_lands_in_exactly_one_bucket() {
        let own_slug = "mine";
        let mut body = String::new();
        body.push_str(&quad_in("a", &ws_graph(own_slug)));
        body.push_str(&quad_in("b", &ws_graph(own_slug)));
        body.push_str(&quad_in("c", &ws_graph("theirs")));
        body.push_str(&quad_in("d", &format!("{}graph/semantic/theirs/doc", nsuri())));
        body.push_str(&quad_in("e", crate::apply_ops::LEDGER_GRAPH));
        body.push_str(&quad_in("f", "https://basemode.ai/g/6f1c"));
        body.push_str(&quad_in("g", &format!("{}graph/somethingnew/x", nsuri())));
        // A bare triple: base writes none, a hand-edited file can.
        body.push_str(TYPED);

        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "graph.nq", &body);
        let store = store::load_graph(&p).unwrap();
        let total = store.len().unwrap();
        let prov = graph_provenance(&store, &nsuri(), own_slug);

        let foreign: usize = prov.foreign.iter().map(|(_, n)| n).sum();
        let unscoped: usize = prov.unscoped.iter().map(|(_, n)| n).sum();
        let unrecognised: usize = prov.unrecognised.iter().map(|(_, n)| n).sum();

        assert_eq!(prov.own, 2, "own");
        assert_eq!(foreign, 2, "foreign: ws/theirs + semantic/theirs");
        assert_eq!(unscoped, 1, "unscoped: the ledger");
        assert_eq!(unrecognised, 2, "unrecognised: the portal graph + an unknown shape");
        assert_eq!(prov.default_graph, 1, "the bare triple");
        assert_eq!(
            prov.own + foreign + unscoped + unrecognised + prov.default_graph,
            total,
            "every quad in the store must land in exactly one bucket"
        );
        assert_eq!(total, 8, "and the store holds what the fixture wrote");
    }

    /// A8 — the global tier's own slug. Measured against the operator's real
    /// global graph on 2026-09-09: all 104,962 of its quads are stamped
    /// `…#graph/ws/base-gbl`, which is this derivation's output.
    #[test]
    fn the_global_tier_owns_the_base_gbl_slug() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join(".base-gbl").join(".base").join("graph.nq");
        assert_eq!(tier_own_slug(&p), "base-gbl");
    }

    /// A9 — the drift that would report 100% of every tier as foreign.
    ///
    /// `tier_own_slug` and [`crate::crud::workspace_slug`] derive the same value by
    /// two different routes — the tier file's grandparent, and a walk up from cwd.
    /// They agree today; nothing but this arm keeps them agreeing.
    #[test]
    fn tier_own_slug_agrees_with_crud_workspace_slug() {
        for name in ["mine", "Chris", "some project", "UPPER-Case_1"] {
            let (_td, p, _) = tier_at(name);
            fs::write(&p, "").unwrap();
            let root = p.parent().and_then(Path::parent).unwrap();
            assert_eq!(
                tier_own_slug(&p),
                crate::crud::workspace_slug(root),
                "the two slug derivations must not drift for {name:?}"
            );
        }
    }

    /// A12 — a workspace with a custom namespace is read with its own vocabulary.
    /// `diagnose_tier` loads THIS tier's `base.toml`; a detector hard-coded to the
    /// default URI would call every quad in such a store unrecognised.
    #[test]
    fn a_custom_namespace_uri_is_respected() {
        let (_td, p, own) = tier_at("mine");
        let base_dir = p.parent().unwrap();
        write_file(
            base_dir,
            "base.toml",
            "[namespace]\nprefix = \"acme\"\nuri = \"https://acme.example/onto#\"\n",
        );
        let mine = format!("https://acme.example/onto#graph/ws/{own}");
        let theirs = "https://acme.example/onto#graph/ws/theirs";
        let body = quad_in("a", &mine) + &quad_in("b", theirs);
        write_file(base_dir, "graph.nq", &body);

        let r = diagnose_tier("workspace", &p);
        assert_eq!(r.foreign_graphs, vec![(theirs.to_string(), 1)]);
        assert!(
            r.unrecognised_graphs.is_empty(),
            "our own custom-namespace graph must not read as unattributable"
        );
    }

    /// A13 — the one bounded extra line. A report saying "3 foreign quads" when
    /// the cause is a renamed folder routes the operator to the wrong fix, and
    /// sending someone to the wrong remedy is a diagnostic defect of its own.
    #[test]
    fn a_wholly_foreign_tier_names_a_possible_rename() {
        let (_td, p, _) = tier_at("renamed-after-the-fact");
        let old = ws_graph("what-it-used-to-be-called");
        let body = quad_in("a", &old) + &quad_in("b", &old) + &quad_in("c", &old);
        write_file(p.parent().unwrap(), "graph.nq", &body);

        let r = diagnose_tier("workspace", &p);
        assert_eq!(r.foreign_graphs, vec![(old, 3)]);
        let report = DoctorReport {
            tiers: vec![r],
            healthy: false,
            warnings: Vec::new(),
            config_errors: Vec::new(),
            trigger_faults: Vec::new(),
            seam: store::LOCK_SEAM_MARKER,
        };
        let human = format_human(&report);
        assert!(human.contains("most likely renamed"), "got:\n{human}");

        // And it must NOT fire on a tier that merely has some foreign quads —
        // otherwise it is noise on the common case rather than a diagnosis of
        // the specific one. Same fixture, one own quad added.
        let (_td2, p2, own2) = tier_at("mine");
        let body2 = quad_in("a", &ws_graph(&own2)) + &quad_in("b", &ws_graph("theirs"));
        write_file(p2.parent().unwrap(), "graph.nq", &body2);
        let report2 = DoctorReport {
            tiers: vec![diagnose_tier("workspace", &p2)],
            healthy: false,
            warnings: Vec::new(),
            config_errors: Vec::new(),
            trigger_faults: Vec::new(),
            seam: store::LOCK_SEAM_MARKER,
        };
        let human2 = format_human(&report2);
        assert!(human2.contains("ANOTHER workspace"), "got:\n{human2}");
        assert!(!human2.contains("most likely renamed"), "got:\n{human2}");
    }

    #[test]
    fn diagnose_tier_healthy_with_composition() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "graph.nq", TYPED);
        let r = diagnose_tier("workspace", &p);
        assert_eq!(r.status, "healthy");
        assert_eq!(r.line_count, 1);
        assert!(r.ends_with_newline);
        assert!(!r.stale_tmp);
        assert_eq!(r.entity_composition, vec![("Thing".to_string(), 1)]);
        assert!(r.latest_backup.is_none());
    }

    #[test]
    fn diagnose_tier_unhealthy_truncated_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let contents = format!(
            "{TYPED}<http://example.org/s2> <http://example.org/p2> \"unterminated"
        );
        let p = write_file(dir.path(), "graph.nq", &contents);
        let r = diagnose_tier("workspace", &p);
        assert_eq!(r.status, "unhealthy");
        assert_eq!(r.bad_line, Some(2));
        assert!(!r.ends_with_newline);
        assert!(r.entity_composition.is_empty()); // no parse → no composition
    }

    #[test]
    fn diagnose_tier_flags_stale_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "graph.nq", TYPED);
        write_file(dir.path(), "graph.nq.tmp", "interrupted write\n");
        let r = diagnose_tier("workspace", &p);
        assert!(r.stale_tmp);
    }

    #[test]
    fn diagnose_tier_backup_delta_flags_shrinkage() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "graph.nq", TYPED); // 1 line
        write_file(dir.path(), "graph.nq.bak", "x\nx\nx\n"); // 3 lines
        let r = diagnose_tier("workspace", &p);
        let backup = r.latest_backup.expect("backup detected");
        assert_eq!(backup.backup_line_count, 3);
        assert_eq!(backup.line_delta, -2); // current smaller than backup → data-loss smell
    }

    #[test]
    fn repair_tier_heals_corrupt_graph() {
        let dir = tempfile::tempdir().unwrap();
        // One good typed quad + a truncated final line.
        let contents = format!("{TYPED}<http://example.org/s2> <http://example.org/p2> \"unterminated");
        let p = write_file(dir.path(), "graph.nq", &contents);

        let outcome = repair_tier("workspace", &p).unwrap();
        assert!(outcome.healthy_after, "graph parses after repair");
        assert_eq!(outcome.quarantined, 1);
        assert_eq!(outcome.kept, 1);

        // Backup + quarantine written.
        assert!(outcome.backup.is_some(), "pre-repair backup taken");
        let qpath = outcome.quarantine.expect("quarantine path");
        let qbody = fs::read_to_string(&qpath).unwrap();
        assert!(qbody.contains("unterminated"), "bad line quarantined verbatim");

        // On-disk graph is healthy now.
        assert_eq!(store::graph_health(&p), GraphHealth::Healthy);
    }

    #[test]
    fn repair_tier_noop_on_healthy() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "graph.nq", TYPED);
        let outcome = repair_tier("workspace", &p).unwrap();
        assert!(outcome.healthy_after);
        assert_eq!(outcome.quarantined, 0);
        assert!(outcome.backup.is_none(), "healthy graph left untouched");
    }

    #[test]
    fn restore_tier_swaps_in_backup_and_snapshots_current() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "graph.nq", TYPED); // current
        let bak = write_file(dir.path(), "graph.nq.bak-old", "a\nb\nc\n"); // snapshot

        restore_tier(&p, &bak).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "a\nb\nc\n", "backup swapped in");

        // The original was snapshotted before the swap.
        let baks = list_backups(&p);
        assert!(
            baks.iter().any(|(bp, _)| bp
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains("bak-pre-restore"))
                .unwrap_or(false)),
            "pre-restore snapshot created"
        );
    }

    #[test]
    fn list_backups_reports_snapshots_with_line_counts() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("graph.nq");
        write_file(dir.path(), "graph.nq.bak-one", "x\nx\n"); // 2 lines
        let baks = list_backups(&p);
        assert_eq!(baks.len(), 1);
        assert_eq!(baks[0].1, 2);
    }

    // Issue #8 cleanup: a handoff that leaked into the global tier resurfaces
    // in every unrelated project. Doctor must name it and the archive command.
    #[test]
    fn leaked_global_handoff_is_flagged_with_its_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let gbl = tmp.path().join("graph.nq");

        // Absent tier is silent, not a warning.
        assert!(leaked_handoffs_in(&gbl).is_empty());

        let o = "http://ops-sys.local/ontology#";
        fs::write(
            &gbl,
            format!(
                "<{o}handoff/stray-one> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{o}Handoff> <{o}graph/global> .\n\
                 <{o}handoff/stray-one> <{o}status> \"open\" <{o}graph/global> .\n\
                 <{o}handoff/archived-one> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{o}Handoff> <{o}graph/global> .\n\
                 <{o}handoff/archived-one> <{o}status> \"archived\" <{o}graph/global> .\n"
            ),
        )
        .unwrap();

        let w = leaked_handoffs_in(&gbl);
        assert_eq!(w.len(), 1, "one advisory line, got {w:?}");
        assert!(w[0].contains("stray-one"), "must name the open handoff: {}", w[0]);
        assert!(!w[0].contains("archived-one"), "archived ones are already silenced: {}", w[0]);
        assert!(w[0].contains("base handoff archive"), "must name the fix: {}", w[0]);
    }
}

#[cfg(test)]
mod coach_drift_tests {
    use super::*;

    #[test]
    fn doctor_is_quiet_when_the_coach_is_in_step() {
        assert_eq!(skill_drift_warning(Some("0.13.2"), "0.13.2", true), None);
    }

    /// The drift this whole fork exists to make visible: binary moved, coach
    /// did not.
    #[test]
    fn doctor_reports_a_lagging_coach() {
        let w = skill_drift_warning(Some("0.12.3"), "0.13.2", true).expect("must warn");
        assert!(w.contains("0.12.3") && w.contains("0.13.2"), "got: {w}");
        assert!(w.contains("base update"), "must name the fix: {w}");
    }

    /// A coach AHEAD of the binary is drift too — it happens when someone rolls
    /// back a binary — so warn rather than only comparing one direction.
    #[test]
    fn doctor_reports_a_coach_ahead_of_the_binary() {
        assert!(skill_drift_warning(Some("0.14.0"), "0.13.2", true).is_some());
    }

    #[test]
    fn doctor_reports_an_unstamped_bank() {
        let w = skill_drift_warning(Some("unstamped"), "0.13.2", true).expect("must warn");
        assert!(w.contains("no version stamp"), "got: {w}");
    }

    /// Installed but never registered: an install predating the manifest entry,
    /// or a hand-copied skill. Drift is undetectable until it is registered.
    #[test]
    fn doctor_reports_an_unregistered_coach() {
        let w = skill_drift_warning(None, "0.13.2", true).expect("must warn");
        assert!(w.contains("not registered"), "got: {w}");
    }

    /// No skill installed at all is not a fault — base works fine without it.
    #[test]
    fn doctor_is_quiet_when_no_coach_is_installed() {
        assert_eq!(skill_drift_warning(None, "0.13.2", false), None);
    }

    /// Advisory, never a health verdict: a stale coach is wrong documentation,
    /// not a broken graph.
    #[test]
    fn coach_drift_never_counts_against_health() {
        let report = DoctorReport {
            tiers: vec![],
            healthy: true,
            warnings: vec![skill_drift_warning(Some("0.12.3"), "0.13.2", true).unwrap()],
            config_errors: vec![],
            trigger_faults: vec![],
            seam: store::LOCK_SEAM_MARKER,
        };
        assert!(report.healthy, "an advisory must not flip the verdict");
        // Reaches the reader even with no graph tiers present — a lagging coach
        // is true regardless of whether a graph exists in this directory.
        assert!(format_human(&report).contains("0.12.3"), "advisory must be rendered");
    }
}
