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
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;

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
    pub latest_backup: Option<BackupCompare>,
}

/// Full doctor report across all resolved tiers.
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub tiers: Vec<TierReport>,
    /// True when no tier is "unhealthy" AND no config file is corrupt
    /// (missing/empty tiers and files do not count against health).
    pub healthy: bool,
    /// Advisory operational warnings (bloat, write-probe failures). Do not affect `healthy`.
    pub warnings: Vec<String>,
    /// Config files that exist but cannot be parsed. The loaders that read these
    /// fail open by design, so this is the only surface that reports them — a
    /// corrupt file otherwise looks exactly like an absent one. Counts against
    /// `healthy`: silently-dead star commands are a fault, not an advisory.
    pub config_errors: Vec<String>,
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
            latest_backup: None,
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
    let config_errors = crate::command::check_command_files(cwd);
    let healthy = tiers.iter().all(|t| t.status != "unhealthy") && config_errors.is_empty();
    DoctorReport {
        tiers,
        healthy,
        warnings,
        config_errors,
    }
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
    let Some(home) = dirs::home_dir() else {
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
            store::write_back(&good, path)?;

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

    if let Some(home) = dirs::home_dir() {
        let global = home.join(".base-gbl").join(".base").join("graph.nq");
        if global.exists() {
            let key = fs::canonicalize(&global).unwrap_or_else(|_| global.clone());
            if seen.insert(key) {
                tiers.push(("global".to_string(), global));
            }
        }
    }

    let mut dir = cwd.to_path_buf();
    loop {
        let ws = dir.join(".base").join("graph.nq");
        if ws.exists() {
            let key = fs::canonicalize(&ws).unwrap_or_else(|_| ws.clone());
            if seen.insert(key) {
                tiers.push(("workspace".to_string(), ws));
            }
            break;
        }
        if !dir.pop() {
            break;
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
