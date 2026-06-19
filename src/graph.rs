//! `base graph` — first-class graph maintenance (Phase 36, GRAPH-DURABILITY.md §4
//! Layer 1). Replaces hand-editing `graph.nq` during cleanup: every op snapshots
//! first via [`store::snapshot`] (rotating) and rewrites atomically via
//! [`store::write_back`]. `compact` is the safe version of the hand-run "stale purge"
//! that caused the 2026-06-18 corruption.

use std::path::Path;

use anyhow::{Context, Result};
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use serde::Serialize;

use crate::config::NamespaceConfig;
use crate::store::{self, GraphHealth};

/// Outcome of a `base graph compact` pass over one tier.
#[derive(Debug, Serialize)]
pub struct CompactOutcome {
    pub path: String,
    pub backup: String,
    pub lines_before: usize,
    pub lines_after: usize,
    pub removed: usize,
}

/// Compact one graph file: snapshot → strict load → atomic `write_back`. Loading into
/// a `Store` collapses duplicate quads (set semantics) and re-serialization
/// canonicalizes order — the safe equivalent of a manual cleanup. PURE/path-scoped
/// (the test-isolation seam, like `doctor::diagnose_tier`). Refuses an unhealthy
/// graph: repair must come first, so compact never rewrites a broken file.
pub fn compact_tier(path: &Path) -> Result<CompactOutcome> {
    if !matches!(store::graph_health(path), GraphHealth::Healthy) {
        anyhow::bail!(
            "graph is not healthy — run `base doctor --repair` before compacting: {}",
            path.display()
        );
    }

    let lines_before = count_lines(path);
    let backup = store::snapshot(path, "compact").context("failed to snapshot before compact")?;

    let graph = store::load_graph(path)?;
    store::write_back(&graph, path)?;

    let lines_after = count_lines(path);
    Ok(CompactOutcome {
        path: path.display().to_string(),
        backup: backup.display().to_string(),
        lines_before,
        lines_after,
        removed: lines_before.saturating_sub(lines_after),
    })
}

/// Compact the nearest workspace graph (`<ws>/.base/graph.nq`).
pub fn compact(cwd: &Path) -> Result<CompactOutcome> {
    let base_dir = crate::config::find_workspace_base(cwd)
        .with_context(|| format!("no workspace .base/ found from {}", cwd.display()))?;
    compact_tier(&base_dir.join("graph.nq"))
}

/// Human report for a compact pass.
pub fn format_compact_human(o: &CompactOutcome) -> String {
    format!(
        "═══════════════════════════════════════\n\
         base graph compact\n\
         ═══════════════════════════════════════\n\
         {}\n   backup: {}\n   {} → {} lines ({} removed)\n   ✓ compacted\n",
        o.path, o.backup, o.lines_before, o.lines_after, o.removed
    )
}

// ─── Purge (Phase 36-02 — usage-based note GC) ─────────────────────────────

/// Outcome of a `base graph purge --stale` pass.
#[derive(Debug, Serialize)]
pub struct PurgeOutcome {
    pub path: String,
    /// Pre-purge snapshot (only when `applied` and something was deleted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    /// false = dry-run preview (nothing written).
    pub applied: bool,
    pub candidates: usize,
    pub deleted: usize,
    /// Up to 5 note-text previews of the stale set.
    pub sample: Vec<String>,
}

/// Select (and, when `apply`, delete) stale notes by RECENCY alone: unread for
/// > `days` (COALESCE(lastRead, createdAt, epoch) < cutoff). Attachment is
/// irrelevant — `lastRead` renews the clock on every recall, so only notes never
/// pulled back up age out (operator decision 2026-06-19). DRY-RUN by default —
/// writes nothing unless `apply`. Refuses an unhealthy graph. PURE/path-scoped.
pub fn purge_stale(path: &Path, ns: &NamespaceConfig, days: i64, apply: bool) -> Result<PurgeOutcome> {
    if !matches!(store::graph_health(path), GraphHealth::Healthy) {
        anyhow::bail!(
            "graph is not healthy — run `base doctor --repair` before purging: {}",
            path.display()
        );
    }
    let p = &ns.prefix;
    let cutoff = chrono::Local::now()
        .checked_sub_signed(chrono::Duration::days(days))
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
        .unwrap_or_default();

    let store = store::load_graph(path)?;

    let select = format!(
        "{prefixes}\n\
         SELECT ?n ?text WHERE {{\n\
           GRAPH ?g {{\n\
             ?n a {p}:Note ; {p}:noteText ?text .\n\
             OPTIONAL {{ ?n {p}:lastRead ?lr }}\n\
             OPTIONAL {{ ?n {p}:createdAt ?cr }}\n\
             FILTER( COALESCE(?lr, ?cr, \"1970-01-01T00:00:00Z\"^^xsd:dateTime) < \"{cutoff}\"^^xsd:dateTime )\n\
           }}\n\
         }}",
        prefixes = crate::crud::prefixes(ns),
    );

    let mut candidates: Vec<String> = Vec::new();
    let mut sample: Vec<String> = Vec::new();
    if let QueryResults::Solutions(sols) = store::query(&store, &select)? {
        for row in sols.filter_map(|r| r.ok()) {
            if let Some(Term::NamedNode(n)) = row.get("n") {
                candidates.push(n.as_str().to_string());
                if sample.len() < 5 {
                    if let Some(Term::Literal(l)) = row.get("text") {
                        sample.push(l.value().chars().take(70).collect());
                    }
                }
            }
        }
    }

    // Dry-run, or nothing to do → write nothing.
    if !apply || candidates.is_empty() {
        return Ok(PurgeOutcome {
            path: path.display().to_string(),
            backup: None,
            applied: false,
            candidates: candidates.len(),
            deleted: 0,
            sample,
        });
    }

    // Apply: snapshot first, delete each note's triples, atomic write_back, re-verify.
    let backup = store::snapshot(path, "pre-purge").context("failed to snapshot before purge")?;
    let mut stmts: Vec<String> = Vec::new();
    for iri in &candidates {
        stmts.push(format!("DELETE WHERE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}"));
        stmts.push(format!("DELETE WHERE {{ GRAPH ?g {{ ?s ?p2 <{iri}> }} }}"));
    }
    let update = format!("{}\n{}", crate::crud::prefixes(ns), stmts.join(";\n"));
    store.update(&update)?;
    store::write_back(&store, path)?;

    Ok(PurgeOutcome {
        path: path.display().to_string(),
        backup: Some(backup.display().to_string()),
        applied: true,
        candidates: candidates.len(),
        deleted: candidates.len(),
        sample,
    })
}

/// Purge stale notes from the nearest workspace graph.
pub fn purge(cwd: &Path, ns: &NamespaceConfig, days: i64, apply: bool) -> Result<PurgeOutcome> {
    let base_dir = crate::config::find_workspace_base(cwd)
        .with_context(|| format!("no workspace .base/ found from {}", cwd.display()))?;
    purge_stale(&base_dir.join("graph.nq"), ns, days, apply)
}

/// Human report for a purge pass.
pub fn format_purge_human(o: &PurgeOutcome) -> String {
    let mut out = String::new();
    out.push_str("═══════════════════════════════════════\n");
    out.push_str("base graph purge --stale\n");
    out.push_str("═══════════════════════════════════════\n");
    out.push_str(&format!("{}\n", o.path));
    out.push_str(&format!("   {} stale note(s) (unread past threshold)\n", o.candidates));
    for s in &o.sample {
        out.push_str(&format!("     · {s}\n"));
    }
    if o.applied {
        out.push_str(&format!("   deleted {} · backup: {}\n", o.deleted, o.backup.as_deref().unwrap_or("-")));
        out.push_str("   ✓ purged\n");
    } else if o.candidates == 0 {
        out.push_str("   nothing to purge\n");
    } else {
        out.push_str("   DRY RUN — nothing deleted. Re-run with --apply to delete.\n");
    }
    out
}

/// Count lines via a streaming reader (never loads the whole file at once).
fn count_lines(path: &Path) -> usize {
    use std::io::BufRead;
    match std::fs::File::open(path) {
        Ok(f) => std::io::BufReader::new(f)
            .lines()
            .map_while(Result::ok)
            .count(),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        p
    }

    #[test]
    fn compact_dedups_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        // Same triple twice — set semantics collapse it to one quad.
        let q = "<http://x/s> <http://x/p> <http://x/o> .\n";
        let p = write_file(dir.path(), "graph.nq", &format!("{q}{q}"));

        let first = compact_tier(&p).unwrap();
        assert!(first.removed >= 1, "duplicate quad collapsed");
        let after1 = fs::read(&p).unwrap();

        let second = compact_tier(&p).unwrap();
        assert_eq!(second.removed, 0, "second compact removes nothing");
        let after2 = fs::read(&p).unwrap();
        assert_eq!(after1, after2, "compact is idempotent (byte-identical)");
    }

    #[test]
    fn compact_refuses_unhealthy_graph() {
        let dir = tempfile::tempdir().unwrap();
        let contents =
            "<http://x/s> <http://x/p> <http://x/o> .\n<http://x/s2> <http://x/p2> \"broken";
        let p = write_file(dir.path(), "graph.nq", contents);
        let before = fs::read(&p).unwrap();

        assert!(compact_tier(&p).is_err(), "refuses corrupt graph");
        assert_eq!(fs::read(&p).unwrap(), before, "corrupt graph left untouched");
    }

    // ── purge ──────────────────────────────────────────────────────────────

    /// Build the N-Quads for one note in a named graph, using the namespace uri.
    fn note_quads(
        uri: &str,
        graph: &str,
        slug: &str,
        created: &str,
        last_read: Option<&str>,
        attached: bool,
    ) -> String {
        let n = format!("{uri}note/{slug}");
        let dt = "http://www.w3.org/2001/XMLSchema#dateTime";
        let rdftype = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
        let mut s = String::new();
        s += &format!("<{n}> <{rdftype}> <{uri}Note> <{graph}> .\n");
        s += &format!("<{n}> <{uri}noteText> \"{slug} text\" <{graph}> .\n");
        s += &format!("<{n}> <{uri}status> \"active\" <{graph}> .\n");
        s += &format!("<{n}> <{uri}createdAt> \"{created}\"^^<{dt}> <{graph}> .\n");
        if let Some(lr) = last_read {
            s += &format!("<{n}> <{uri}lastRead> \"{lr}\"^^<{dt}> <{graph}> .\n");
        }
        if attached {
            s += &format!("<{n}> <{uri}relatedTo> <{uri}domain/d> <{graph}> .\n");
        }
        s
    }

    fn purge_fixture(dir: &Path) -> (std::path::PathBuf, NamespaceConfig) {
        let ns = NamespaceConfig::default();
        let g = format!("{}graph/ws/test", ns.uri);
        let mut body = String::new();
        // (a) stale: old, unread, unattached → purgeable
        body += &note_quads(&ns.uri, &g, "stale", "2020-01-01T00:00:00Z", None, false);
        // (b) attached: old + unread but linked → spared
        body += &note_quads(&ns.uri, &g, "attached", "2020-01-01T00:00:00Z", None, true);
        // (c) recently read: unattached but read recently → spared
        body += &note_quads(&ns.uri, &g, "fresh", "2020-01-01T00:00:00Z", Some("2026-06-18T00:00:00Z"), false);
        let p = write_file(dir, "graph.nq", &body);
        (p, ns)
    }

    #[test]
    fn purge_dry_run_selects_by_recency_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (p, ns) = purge_fixture(dir.path());
        let before = fs::read(&p).unwrap();

        // Recency-only: both old notes (stale + attached) qualify; only the
        // recently-read "fresh" note is spared. Attachment is irrelevant.
        let out = purge_stale(&p, &ns, 90, false).unwrap();
        assert_eq!(out.candidates, 2, "both unread-past-threshold notes selected");
        assert!(!out.applied);
        assert_eq!(out.deleted, 0);
        assert_eq!(fs::read(&p).unwrap(), before, "dry-run writes nothing");
    }

    #[test]
    fn purge_apply_deletes_unread_spares_recent_and_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let (p, ns) = purge_fixture(dir.path());

        let out = purge_stale(&p, &ns, 90, true).unwrap();
        assert!(out.applied);
        assert_eq!(out.deleted, 2);
        assert!(out.backup.is_some(), "pre-purge snapshot taken");

        // Graph still healthy; only the recently-read note remains.
        assert_eq!(store::graph_health(&p), GraphHealth::Healthy);
        let remaining = fs::read_to_string(&p).unwrap();
        assert!(!remaining.contains("note/stale"), "stale note removed");
        assert!(!remaining.contains("note/attached"), "attached-but-unread note removed (recency-only)");
        assert!(remaining.contains("note/fresh"), "recently-read note spared (lastRead renews clock)");

        // Idempotent-ish: a second dry-run now finds zero.
        let again = purge_stale(&p, &ns, 90, false).unwrap();
        assert_eq!(again.candidates, 0);
    }

    #[test]
    fn purge_refuses_unhealthy_graph() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "graph.nq", "<http://x/s> <http://x/p> <http://x/o> .\n<http://x/s2> \"broken");
        let before = fs::read(&p).unwrap();
        let ns = NamespaceConfig::default();
        assert!(purge_stale(&p, &ns, 90, true).is_err(), "refuses corrupt graph");
        assert_eq!(fs::read(&p).unwrap(), before, "corrupt graph untouched");
    }
}
