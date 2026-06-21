use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oxigraph::io::{RdfFormat, RdfSerializer};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

/// The canonical RDF format for graph persistence.
/// NQuads is immune to the graph-block split corruption bug in oxigraph's
/// TriG serializer (one quad per line, no stateful graph-block machine).
const GRAPH_FORMAT: RdfFormat = RdfFormat::NQuads;

/// Auto-migrate a legacy graph.trig to graph.nq if present.
/// Loads the TriG file, writes it as NQuads, and removes the old file.
/// Returns Ok(true) if migration happened, Ok(false) if no legacy file found.
pub fn migrate_trig_to_nq(nq_path: &Path) -> Result<bool> {
    let trig_path = nq_path.with_extension("trig");
    // If the caller passed a .trig path directly, trig_path == nq_path and the
    // "remove old file" branch below would delete the file it was asked to load.
    if trig_path == nq_path {
        return Ok(false);
    }
    if !trig_path.exists() {
        return Ok(false);
    }

    // If graph.nq already exists alongside graph.trig, just remove the old one
    if nq_path.exists() {
        let _ = fs::remove_file(&trig_path);
        return Ok(false);
    }

    eprintln!("Migrating {} → {} ...", trig_path.display(), nq_path.display());

    // Load from TriG format
    let store = Store::new().context("Failed to create migration store")?;
    let file = fs::File::open(&trig_path)
        .with_context(|| format!("Failed to open legacy {}", trig_path.display()))?;
    let reader = BufReader::new(file);
    store
        .load_from_reader(RdfFormat::TriG, reader)
        .with_context(|| format!(
            "Failed to parse legacy {}. File may be corrupted — \
             delete it and run `base sync` to rebuild.",
            trig_path.display()
        ))?;

    // Write as NQuads using write_back (includes validation)
    write_back(&store, nq_path)?;

    // Remove old TriG file
    let _ = fs::remove_file(&trig_path);

    eprintln!("Migration complete. Legacy graph.trig removed.");
    Ok(true)
}

/// Load an NQuads file into a new in-memory store.
/// Auto-migrates legacy graph.trig if graph.nq doesn't exist yet.
pub fn load_graph(path: &Path) -> Result<Store> {
    // Auto-migrate legacy TriG if needed
    migrate_trig_to_nq(path)?;

    let store = Store::new().context("Failed to create in-memory store")?;
    let file = fs::File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    store
        .load_from_reader(GRAPH_FORMAT, reader)
        .with_context(|| format!("Failed to parse graph from {}", path.display()))?;
    Ok(store)
}

/// A line that failed to parse during a lenient load. `text` is the raw line
/// (trailing newline stripped) so `base doctor --repair` can quarantine it verbatim.
#[derive(Debug, Clone)]
pub struct BadLine {
    pub line_no: usize,
    pub text: String,
}

/// Stream `path` line-by-line into `store`, loading each N-Quads statement
/// individually and COLLECTING (not aborting on) any that fail to parse.
/// Shared by [`load_graph_lenient`] and the [`load_merged`] read fallback.
/// Returns Err only for unrecoverable IO (cannot open the file).
fn load_lenient_into(store: &Store, path: &Path) -> Result<Vec<BadLine>> {
    let file = fs::File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut bad_lines = Vec::new();
    for (idx, line_res) in reader.lines().enumerate() {
        let line_no = idx + 1;
        let line = match line_res {
            Ok(l) => l,
            Err(e) => {
                bad_lines.push(BadLine { line_no, text: format!("<read error: {e}>") });
                continue;
            }
        };

        let trimmed = line.trim();
        // Blank lines and comments carry no statement — skip without flagging.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // NQuads is line-oriented: one line = one quad. A parse error adds nothing
        // to the store, so the good set stays intact and the bad line is collected.
        let stmt = format!("{line}\n");
        if store
            .load_from_reader(GRAPH_FORMAT, stmt.as_bytes())
            .is_err()
        {
            bad_lines.push(BadLine { line_no, text: line });
        }
    }
    Ok(bad_lines)
}

/// Lenient counterpart to [`load_graph`]: skip+collect malformed quads instead of
/// aborting the whole load. One bad line costs one triple, not the graph
/// (GRAPH-DURABILITY.md §4 Layer 2). A malformed line is NEVER an Err — it lands
/// in the returned `Vec<BadLine>`; Err is reserved for unrecoverable IO.
///
/// This is the RESILIENCE primitive for READS only. Write paths stay on the strict
/// [`load_graph`] / [`load_graphs`] so a corrupt graph is never silently rewritten
/// with dropped lines — repair is explicit via `base doctor --repair` (§7.3).
pub fn load_graph_lenient(path: &Path) -> Result<(Store, Vec<BadLine>)> {
    let store = Store::new().context("Failed to create in-memory store")?;
    let bad_lines = load_lenient_into(&store, path)?;
    Ok((store, bad_lines))
}

/// Health verdict for a graph file. Produced by [`graph_health`] WITHOUT
/// requiring a successful strict parse — this is the seed of `base doctor`
/// (Phase 34), which must work precisely when the graph is broken.
#[derive(Debug, PartialEq)]
pub enum GraphHealth {
    /// File exists and parses cleanly under the strict oxigraph N-Quads reader.
    Healthy,
    /// File does not exist (a fresh workspace with no graph yet — not an error).
    Missing,
    /// File exists but the strict parse failed. `reason` is human-readable;
    /// `bad_line` is the 1-based line number of the first localized defect
    /// (None when the defect could not be pinned to a specific line).
    Unhealthy { reason: String, bad_line: Option<usize> },
}

/// Parser-independent health probe for a graph file.
///
/// Strategy:
/// 1. Missing file → [`GraphHealth::Missing`].
/// 2. Fast path: reuse the strict [`load_graph`] parse. Success → [`GraphHealth::Healthy`].
/// 3. On strict-parse failure, do a RAW line-level scan (NOT via oxigraph) to
///    localize the defect — every N-Quads statement terminates with ` .`, so the
///    first non-blank, non-comment line that does not end in `.` is flagged.
///    This catches the 2026-06-18 incident: a truncated final line (mid string
///    literal, no closing quote, no ` .`) inside an otherwise-valid 84k-line graph.
///
/// Never panics; on IO error returns [`GraphHealth::Unhealthy`] with `bad_line: None`.
/// Deliberately does NOT attempt a lenient/recovering parse — that is Phase 35.
pub fn graph_health(path: &Path) -> GraphHealth {
    if !path.exists() {
        return GraphHealth::Missing;
    }

    // Fast path: the strict reader is the source of truth for "healthy".
    if load_graph(path).is_ok() {
        return GraphHealth::Healthy;
    }

    // Strict parse failed — localize the defect with a raw, streaming line scan.
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return GraphHealth::Unhealthy {
                reason: format!("cannot open file: {e}"),
                bad_line: None,
            };
        }
    };

    let mut last_content_line = 0usize;
    let mut first_unterminated: Option<usize> = None;

    for (idx, line_res) in BufReader::new(file).lines().enumerate() {
        let line_no = idx + 1;
        let line = match line_res {
            Ok(l) => l,
            Err(e) => {
                return GraphHealth::Unhealthy {
                    reason: format!("read error at line {line_no}: {e}"),
                    bad_line: Some(line_no),
                };
            }
        };

        let trimmed = line.trim_end();
        // Blank lines and comments are not statements — skip.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        last_content_line = line_no;
        // N-Quads statements end with a space-dot terminator; after trim_end the
        // trailing space is gone, so a well-formed statement ends with '.'.
        if !trimmed.ends_with('.') && first_unterminated.is_none() {
            first_unterminated = Some(line_no);
        }
    }

    match first_unterminated {
        Some(n) => {
            let reason = if n == last_content_line {
                format!("line {n} not terminated — truncated write (missing ' .')")
            } else {
                format!("line {n} malformed — N-Quads statement not ' .'-terminated")
            };
            GraphHealth::Unhealthy { reason, bad_line: Some(n) }
        }
        // Every line is terminated yet the strict parse still failed: the defect
        // is a malformed quad we cannot pin to a line by terminator alone.
        None => GraphHealth::Unhealthy {
            reason: "graph failed to parse (malformed quad); no unterminated line found".to_string(),
            bad_line: None,
        },
    }
}

/// Load multiple graph files into a single in-memory store (cross-tier query).
/// Auto-migrates legacy TriG files if needed.
pub fn load_graphs(paths: &[&Path]) -> Result<Store> {
    let store = Store::new().context("Failed to create in-memory store")?;
    for path in paths {
        // Auto-migrate legacy TriG if needed
        migrate_trig_to_nq(path)?;

        let file = fs::File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
        let reader = BufReader::new(file);
        store
            .load_from_reader(GRAPH_FORMAT, reader)
            .with_context(|| format!("Failed to parse graph from {}", path.display()))?;
    }
    Ok(store)
}

/// Load a merged graph from global (~/.base-gbl/.base/graph.nq) and workspace tiers
/// into one store so SPARQL queries span all tiers. Returns None only if neither
/// graph exists (fail-open). Call ONCE per hook invocation and share the store.
pub fn load_merged(cwd: &Path) -> Option<Store> {
    let mut paths: Vec<std::path::PathBuf> = Vec::new();

    if let Some(home) = dirs::home_dir() {
        let global_nq = home.join(".base-gbl").join(".base").join("graph.nq");
        if global_nq.exists() {
            paths.push(global_nq);
        }
    }

    if let Some(base_dir) = crate::config::find_workspace_base(cwd) {
        let ws_nq = base_dir.join("graph.nq");
        if ws_nq.exists() {
            paths.push(ws_nq);
        }
    }

    if paths.is_empty() {
        return None;
    }

    let path_refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();

    // Fast path: strict multi-tier load.
    if let Ok(store) = load_graphs(&path_refs) {
        return Some(store);
    }

    // Strict parse failed in some tier. READS degrade rather than die: fall back to
    // a lenient per-tier load so one corrupt tier costs only its bad lines, not all
    // context. WRITES never reach here (they use strict load_graph + write_back), so
    // a corrupt graph is never silently rewritten with lines dropped (§7.3).
    let store = Store::new().ok()?;
    let mut total_bad = 0usize;
    let mut bad_tiers = 0usize;
    for path in &paths {
        match load_lenient_into(&store, path) {
            Ok(bad) if !bad.is_empty() => {
                total_bad += bad.len();
                bad_tiers += 1;
            }
            Ok(_) => {}
            Err(_) => bad_tiers += 1,
        }
    }
    if total_bad > 0 {
        eprintln!(
            "graph: skipped {total_bad} malformed line(s) across {bad_tiers} tier(s) — run `base doctor --repair`"
        );
    }
    Some(store)
}

/// Load a Turtle file (e.g. ast.ttl) into an existing store's default graph.
/// AST data lives ONLY in ast.ttl — it is never merged into graph.nq
/// (appending Turtle to an NQuads file corrupts it; see AUDIT C10).
pub fn load_turtle_into(store: &Store, path: &Path) -> Result<()> {
    let file = fs::File::open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    store
        .load_from_reader(RdfFormat::Turtle, reader)
        .with_context(|| format!("Failed to parse Turtle from {}", path.display()))?;
    Ok(())
}

/// Run a SPARQL query (SELECT or ASK) against the store.
pub fn query(store: &Store, sparql: &str) -> Result<QueryResults> {
    store
        .query(sparql)
        .with_context(|| format!("SPARQL query failed: {sparql}"))
}

/// Serialize the store to NQuads and write atomically (temp + rename).
/// Validates the output by re-parsing before committing the rename.
pub fn write_back(store: &Store, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("Path has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create directory {}", parent.display()))?;

    let tmp_path = path.with_extension("nq.tmp");
    let mut tmp_file = fs::File::create(&tmp_path)
        .with_context(|| format!("Failed to create temp file {}", tmp_path.display()))?;

    store
        .dump_to_writer(RdfSerializer::from_format(GRAPH_FORMAT), &mut tmp_file)
        .context("Failed to serialize store to NQuads")?;

    // Flush and close the file handle before validation.
    drop(tmp_file);

    // Validate: re-parse the written file to catch serializer corruption.
    {
        let check_file = fs::File::open(&tmp_path)
            .with_context(|| format!("Failed to re-open {} for validation", tmp_path.display()))?;
        let check_reader = BufReader::new(check_file);
        let check_store = Store::new().context("Failed to create validation store")?;
        if let Err(e) = check_store.load_from_reader(GRAPH_FORMAT, check_reader) {
            let _ = fs::remove_file(&tmp_path);
            anyhow::bail!(
                "write_back validation failed — serializer produced invalid output, \
                 original file preserved. Parse error: {e}"
            );
        }
    }

    // Atomic rename
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "Failed to rename {} → {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

/// Max backup snapshots to retain per graph file before rotating out the oldest.
const BACKUP_KEEP: usize = 10;

/// Snapshot `path` to a sibling `{fname}.bak-{op}-{stamp}` before a mutating op,
/// then prune the oldest `{fname}.bak*` snapshots beyond [`BACKUP_KEEP`]. This moves
/// the hand-run `cp` backup convention into the binary (GRAPH-DURABILITY.md §4 Layer 1)
/// and is the single backup path for repair / restore / compact / purge.
/// Returns the new snapshot path. Err only if the copy fails; rotation failure is
/// non-fatal (logged to stderr, snapshot still returned Ok).
pub fn snapshot(path: &Path, op: &str) -> Result<PathBuf> {
    let fname = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("graph path has no filename")?;
    let stamp = chrono::Local::now().format("%Y-%m-%d-%H%M%S");
    let backup = path.with_file_name(format!("{fname}.bak-{op}-{stamp}"));
    fs::copy(path, &backup).with_context(|| {
        format!("failed to snapshot {} → {}", path.display(), backup.display())
    })?;

    rotate_backups(path, fname);
    Ok(backup)
}

/// Keep the newest [`BACKUP_KEEP`] `{fname}.bak*` snapshots; delete the rest.
/// Non-fatal: any IO error is logged and skipped.
fn rotate_backups(path: &Path, fname: &str) {
    let Some(parent) = path.parent() else {
        return;
    };
    let prefix = format!("{fname}.bak");
    let Ok(rd) = fs::read_dir(parent) else {
        return;
    };

    let mut baks: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
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

    if baks.len() <= BACKUP_KEEP {
        return;
    }
    baks.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    for (_, p) in baks.into_iter().skip(BACKUP_KEEP) {
        if let Err(e) = fs::remove_file(&p) {
            eprintln!("graph: failed to rotate out backup {}: {e}", p.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        p
    }

    #[test]
    fn graph_health_healthy_on_wellformed_nquads() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            dir.path(),
            "graph.nq",
            "<http://example.org/s> <http://example.org/p> <http://example.org/o> .\n",
        );
        assert_eq!(graph_health(&p), GraphHealth::Healthy);
    }

    #[test]
    fn graph_health_missing_on_absent_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("does-not-exist.nq");
        assert_eq!(graph_health(&p), GraphHealth::Missing);
    }

    #[test]
    fn load_graph_lenient_skips_one_bad_line_keeps_good() {
        // Two valid statements, then a truncated third line (mid string literal).
        let dir = tempfile::tempdir().unwrap();
        let contents = concat!(
            "<http://example.org/s1> <http://example.org/p> <http://example.org/o1> .\n",
            "<http://example.org/s2> <http://example.org/p> <http://example.org/o2> .\n",
            "<http://example.org/s3> <http://example.org/p3> \"unterminated",
        );
        let p = write_file(dir.path(), "graph.nq", contents);
        let (store, bad) = load_graph_lenient(&p).unwrap();
        assert_eq!(store.len().unwrap(), 2, "two good quads survive");
        assert_eq!(bad.len(), 1, "one bad line collected");
        assert_eq!(bad[0].line_no, 3);
    }

    #[test]
    fn load_graph_lenient_clean_file_has_no_bad_lines() {
        let dir = tempfile::tempdir().unwrap();
        let contents = concat!(
            "<http://example.org/s1> <http://example.org/p> <http://example.org/o1> .\n",
            "<http://example.org/s2> <http://example.org/p> <http://example.org/o2> .\n",
        );
        let p = write_file(dir.path(), "graph.nq", contents);
        let (store, bad) = load_graph_lenient(&p).unwrap();
        assert_eq!(store.len().unwrap(), 2);
        assert!(bad.is_empty());
    }

    #[test]
    fn load_graph_lenient_missing_file_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("does-not-exist.nq");
        assert!(load_graph_lenient(&p).is_err());
    }

    #[test]
    fn snapshot_rotates_to_keep_limit() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(dir.path(), "graph.nq", "<http://x/s> <http://x/p> <http://x/o> .\n");
        // 12 pre-existing backups → snapshot adds a 13th, rotation prunes to 10.
        for i in 0..12 {
            write_file(dir.path(), &format!("graph.nq.bak-old-{i:02}"), "x\n");
        }
        let new_bak = snapshot(&p, "test").unwrap();
        assert!(new_bak.exists(), "new snapshot written");

        let count = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.starts_with("graph.nq.bak"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(count, 10, "rotation keeps exactly BACKUP_KEEP snapshots");
    }

    #[test]
    fn graph_health_unhealthy_on_truncated_last_line() {
        // Reproduces the 2026-06-18 incident shape: a valid first statement, then
        // a final line truncated mid string-literal (no closing quote, no ` .`).
        let dir = tempfile::tempdir().unwrap();
        let contents = concat!(
            "<http://example.org/s> <http://example.org/p> <http://example.org/o> .\n",
            "<http://example.org/s2> <http://example.org/p2> \"unterminated",
        );
        let p = write_file(dir.path(), "graph.nq", contents);
        match graph_health(&p) {
            GraphHealth::Unhealthy { bad_line, .. } => assert_eq!(bad_line, Some(2)),
            other => panic!("expected Unhealthy, got {other:?}"),
        }
    }
}
