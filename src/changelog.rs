//! Append-only change log for graph writes.
//!
//! Every successful graph write appends one JSON line to `changes.jsonl`, written
//! beside the `graph.nq` that was actually written. External readers (the Electron
//! app) tail it with a **byte offset** cursor — see [`read_since`].
//!
//! Three properties this module exists to guarantee:
//!
//! 1. **Nothing is logged that did not land.** [`append`] is called from
//!    [`crate::store::write_back`] *after* the atomic rename returns, never before.
//! 2. **Logging never fails the user's command.** [`append`] returns `()`; a failed
//!    log write warns once to stderr and is dropped. A missing log line is a
//!    recoverable gap for the reader; a failed `base learn` is not.
//! 3. **Tier is derived, never passed.** `write_back` already holds the resolved
//!    graph path, and the tier *is* that path (`<ws>/.base/graph.nq` vs
//!    `~/.base-gbl/.base/graph.nq`). Keying off it covers workspace and global
//!    tiers with no tier parameter and no branch that can get them backwards.
//!
//! ## Why O_APPEND and not temp+rename
//!
//! The rest of this crate writes atomically with temp+rename, which is correct for
//! whole-file rewrites. It is wrong here. On Windows the *rename* is the unreliable
//! step — a scanner or backup agent holding a transient handle makes it throw
//! `EPERM`/`EBUSY`, and unretried the write is silently lost (memory:
//! `windows-atomic-write-retry`). An append has no rename to lose, and a single
//! `O_APPEND` write is atomic against concurrent appenders on both targets: Linux
//! holds the inode lock across `generic_file_write_iter`, and Windows resolves
//! `FILE_APPEND_DATA` at the filesystem. Hooks fire on every tool call, so several
//! `base` processes really do write at once — this is the property that keeps their
//! lines from interleaving into a torn one.
//!
//! That atomicity holds for **one** write syscall. `write_all` loops on a short
//! write, so an unbounded line could in principle be split; [`SPARQL_MAX_BYTES`]
//! bounds the line instead of hoping.

use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Log file name, written beside the graph file it describes.
pub const LOG_FILE: &str = "changes.jsonl";

/// Longest SPARQL body recorded verbatim. Beyond this the body is cut on a UTF-8
/// boundary and the record carries `sparql_truncated` + the original byte length,
/// so a reader can tell a cut body from a short one.
///
/// This exists to bound the line, not the data: AST-map and extraction updates can
/// run to megabytes, and a megabyte line is the one case where `write_all` could
/// issue more than one `write` and let a concurrent appender interleave. 64 KiB
/// keeps every line to a single syscall while leaving ordinary CRUD deltas — which
/// are hundreds of bytes — completely intact.
pub const SPARQL_MAX_BYTES: usize = 64 * 1024;

/// What produced a graph write.
///
/// [`crate::store::write_back`] takes this by value, so a new write path cannot
/// compile without saying which it is. That is the point: hand-wiring the ~100
/// existing writers would leave a gap the day someone adds the next one, and that
/// gap surfaces as the reader quietly missing writes.
/// One fact-shaped delta carried in a change record's `ops[]`.
///
/// The same shape the desktop client ships to the portal, so a reader never has
/// to parse SPARQL to know what a write did.
#[derive(Debug, Clone)]
pub struct AppliedOp {
    kind: &'static str,
    fact_id: String,
    quads: Vec<String>,
}

impl AppliedOp {
    pub fn assert(fact_id: String, quads: Vec<String>) -> Self {
        Self { kind: "assert", fact_id, quads }
    }

    pub fn retire(fact_id: String, quads: Vec<String>) -> Self {
        Self { kind: "retire", fact_id, quads }
    }

    fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("type".into(), self.kind.into());
        // A retire names the fact it supersedes; an assert names the fact it is.
        let id_field = if self.kind == "retire" { "supersedes_fact_id" } else { "fact_id" };
        m.insert(id_field.into(), self.fact_id.clone().into());
        m.insert(
            "payload".into(),
            serde_json::json!({ "quads": self.quads }),
        );
        serde_json::Value::Object(m)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Change<'a> {
    /// A SPARQL UPDATE was applied to the store before this write. The string is
    /// the exact update text, and it *is* the delta.
    Sparql(&'a str),
    /// A whole-store rewrite with no SPARQL delta — compaction, repair, restore,
    /// bulk extraction. The string names the caller (`"graph.compact"`), so the
    /// reader still sees that a write happened. A silent write is worse than an
    /// unlabelled one.
    Op(&'a str),
    /// A write produced by applying inbound ops from another machine
    /// (`base graph apply-ops`). Carries the fact-shaped delta, and is the one
    /// variant whose record is tagged `origin: "remote"` — the primary guard
    /// against a pulled fact being shipped straight back out as a local write.
    RemoteOps(&'a [AppliedOp]),
}

impl Change<'_> {
    /// Where this write came from.
    ///
    /// `"remote"` for ops applied from another machine, `"local"` for everything
    /// this machine did itself. Stamped on **every** record and never omitted:
    /// a reader translating the log into ops for the team graph has to skip what
    /// it already pulled, and an absent field makes "is this an echo?"
    /// unanswerable — which is the one bug that would corrupt a shared graph,
    /// because B re-ships A's fact, A re-ships it back, unbounded.
    fn origin(&self) -> &'static str {
        match self {
            Change::RemoteOps(_) => "remote",
            Change::Sparql(_) | Change::Op(_) => "local",
        }
    }
}

/// Append one record describing a completed write to `graph_path`'s sibling log.
///
/// Best-effort by contract: never returns, never panics, never fails the caller.
pub fn append(graph_path: &Path, change: Change<'_>) {
    crate::home::assert_isolated_write(graph_path);
    let log_path = log_path_for(graph_path);
    let line = record_line(graph_path, change);
    if let Err(e) = append_line(&log_path, &line) {
        eprintln!(
            "warning: change log append failed ({}): {e}",
            log_path.display()
        );
    }
}

/// The log path beside a given graph file.
pub fn log_path_for(graph_path: &Path) -> PathBuf {
    graph_path.with_file_name(LOG_FILE)
}

/// Current end offset of the log — the cursor a fresh reader starts from to see
/// only future writes. Zero when the log does not exist yet.
pub fn cursor(log_path: &Path) -> u64 {
    std::fs::metadata(log_path).map(|m| m.len()).unwrap_or(0)
}

/// Everything after byte offset `since`, plus the offset to pass next time.
///
/// The returned offset advances only past the last **complete** line, so a reader
/// that catches a concurrent appender mid-write re-reads that line next call
/// instead of losing it.
///
/// `reset` is true when `since` is past the end of the file — the log was
/// truncated or replaced under the reader. Lines come back empty and the offset
/// comes back `0` rather than silently replaying the whole file; the reader
/// decides whether to re-read from the start.
pub struct Page {
    pub lines: Vec<String>,
    pub offset: u64,
    pub reset: bool,
}

pub fn read_since(log_path: &Path, since: u64) -> io::Result<Page> {
    let len = match std::fs::metadata(log_path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Page { lines: Vec::new(), offset: 0, reset: false });
        }
        Err(e) => return Err(e),
    };

    if since > len {
        return Ok(Page { lines: Vec::new(), offset: 0, reset: true });
    }
    if since == len {
        return Ok(Page { lines: Vec::new(), offset: len, reset: false });
    }

    let mut file = std::fs::File::open(log_path)?;
    file.seek(SeekFrom::Start(since))?;
    let mut buf = Vec::with_capacity((len - since) as usize);
    file.take(len - since).read_to_end(&mut buf)?;

    // Stop at the last newline: anything after it is a line still being written.
    let complete = match buf.iter().rposition(|b| *b == b'\n') {
        Some(i) => i + 1,
        None => {
            return Ok(Page { lines: Vec::new(), offset: since, reset: false });
        }
    };

    let lines = BufReader::new(&buf[..complete])
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .collect();

    Ok(Page { lines, offset: since + complete as u64, reset: false })
}

// ─── Record construction ─────────────────────────────────────

/// Build the one-line JSON record, newline included.
fn record_line(graph_path: &Path, change: Change<'_>) -> String {
    let mut rec = serde_json::Map::new();
    rec.insert("at".into(), crate::crud::now_iso().into());
    rec.insert("ws".into(), ws_slug(graph_path).into());
    rec.insert("origin".into(), change.origin().into());

    match change {
        Change::Sparql(sparql) => {
            if let Some(g) = derive_graph_iri(sparql) {
                rec.insert("graph".into(), g.into());
            }
            let (body, truncated) = clamp(sparql);
            rec.insert("sparql".into(), body.into());
            if truncated {
                rec.insert("sparql_truncated".into(), true.into());
                rec.insert("sparql_bytes".into(), sparql.len().into());
            }
        }
        Change::Op(op) => {
            // No delta to record — name the caller so the write is still visible.
            rec.insert("kind".into(), op.into());
        }
        Change::RemoteOps(ops) => {
            rec.insert("kind".into(), "graph.apply-ops".into());
            let arr: Vec<_> = ops.iter().map(AppliedOp::to_json).collect();
            let body = serde_json::Value::Array(arr);
            // Same single-syscall bound the SPARQL body gets: an unbounded line is
            // the one case `write_all` can split and let a concurrent appender
            // interleave. Over the bound the delta is dropped and *said* to be
            // dropped, never silently shortened into a plausible-looking one.
            let bytes = body.to_string().len();
            if bytes <= SPARQL_MAX_BYTES {
                rec.insert("ops".into(), body);
            } else {
                rec.insert("ops_omitted".into(), true.into());
                rec.insert("ops_bytes".into(), bytes.into());
            }
        }
    }

    let mut line = serde_json::Value::Object(rec).to_string();
    line.push('\n');
    line
}

/// Workspace slug for the tier that owns this graph: `<ws>/.base/graph.nq` → `<ws>`,
/// `~/.base-gbl/.base/graph.nq` → `base-gbl`. The global tier self-labels, so a
/// reader can tell the tiers apart from the record alone.
fn ws_slug(graph_path: &Path) -> String {
    graph_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .map(crate::crud::slugify)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into())
}

/// Cut a SPARQL body to [`SPARQL_MAX_BYTES`] on a UTF-8 boundary.
/// Returns the body and whether it was cut.
fn clamp(sparql: &str) -> (&str, bool) {
    if sparql.len() <= SPARQL_MAX_BYTES {
        return (sparql, false);
    }
    let mut cut = SPARQL_MAX_BYTES;
    while cut > 0 && !sparql.is_char_boundary(cut) {
        cut -= 1;
    }
    (&sparql[..cut], true)
}

/// The named graph an update targets, when it targets exactly one.
///
/// Reads `GRAPH <iri>` occurrences. A `GRAPH ?var` form contributes nothing, and
/// two *different* IRIs mean the write spans graphs — both cases omit the field
/// rather than guess, which is what "if derivable" buys the reader.
fn derive_graph_iri(sparql: &str) -> Option<String> {
    // `to_ascii_lowercase` is byte-length preserving, so offsets index both.
    let lower = sparql.to_ascii_lowercase();
    let mut found: Option<String> = None;
    let mut at = 0usize;

    while let Some(rel) = lower[at..].find("graph") {
        let start = at + rel;
        at = start + "graph".len();

        // Must be a standalone token, not the tail of `?graph` / `ex:graph`.
        if start > 0 {
            let prev = lower.as_bytes()[start - 1];
            if prev.is_ascii_alphanumeric() || matches!(prev, b'_' | b':' | b'?' | b'$' | b'<' | b'-') {
                continue;
            }
        }

        let rest = &sparql[at..];
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('<') {
            continue;
        }
        let Some(close) = trimmed.find('>') else { continue };
        let iri = &trimmed[1..close];
        if iri.is_empty() {
            continue;
        }
        at += (rest.len() - trimmed.len()) + close + 1;

        match &found {
            None => found = Some(iri.to_string()),
            Some(seen) if seen == iri => {}
            Some(_) => return None, // spans graphs — not derivable
        }
    }
    found
}

// ─── Append with the Windows lock-family retry ───────────────

const RETRY_ATTEMPTS: usize = 5;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(15);

/// Append one line in a single `write_all`, retrying only the transient
/// lock family.
fn append_line(log_path: &Path, line: &str) -> io::Result<()> {
    let mut delay = RETRY_BASE_DELAY;
    let mut last: Option<io::Error> = None;

    for attempt in 0..RETRY_ATTEMPTS {
        let attempt_result = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .and_then(|mut f| f.write_all(line.as_bytes()));

        match attempt_result {
            Ok(()) => return Ok(()),
            // Anything outside the lock family is a real failure. Retrying ENOSPC
            // turns a clear error into a slow one.
            Err(e) if !is_transient_lock(&e) => return Err(e),
            Err(e) => last = Some(e),
        }

        if attempt + 1 < RETRY_ATTEMPTS {
            std::thread::sleep(delay);
            delay *= 2;
        }
    }

    Err(last.unwrap_or_else(|| io::Error::other("change log append failed")))
}

/// The transient lock family a scanner or backup agent produces:
/// `EPERM`, `EBUSY`, `EACCES`, `EMFILE`, `ENFILE` — plus their Windows
/// equivalents. Everything else is fatal.
fn is_transient_lock(e: &io::Error) -> bool {
    use io::ErrorKind::*;
    if matches!(e.kind(), PermissionDenied | Interrupted | WouldBlock) {
        return true;
    }
    match e.raw_os_error() {
        #[cfg(unix)]
        // EPERM, EACCES, EBUSY, EMFILE, ENFILE
        Some(1) | Some(13) | Some(16) | Some(24) | Some(23) => true,
        #[cfg(windows)]
        // ACCESS_DENIED, SHARING_VIOLATION, LOCK_VIOLATION, TOO_MANY_OPEN_FILES
        Some(5) | Some(32) | Some(33) | Some(4) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_slug_names_the_tier() {
        assert_eq!(ws_slug(Path::new("/home/x/proj/.base/graph.nq")), "proj");
        assert_eq!(ws_slug(Path::new("/home/x/.base-gbl/.base/graph.nq")), "base-gbl");
    }

    #[test]
    fn graph_iri_derives_only_when_unambiguous() {
        assert_eq!(
            derive_graph_iri("INSERT DATA { GRAPH <http://ex/g/ws/proj> { <a> <b> <c> } }"),
            Some("http://ex/g/ws/proj".to_string())
        );
        // Same graph twice is still one graph.
        assert_eq!(
            derive_graph_iri(
                "DELETE { GRAPH <http://ex/g> { ?s ?p ?o } } WHERE { GRAPH <http://ex/g> { ?s ?p ?o } }"
            ),
            Some("http://ex/g".to_string())
        );
        // Two different graphs, a variable graph, and no graph at all: omit.
        assert_eq!(
            derive_graph_iri("INSERT { GRAPH <http://ex/a> { ?s ?p ?o } } WHERE { GRAPH <http://ex/b> { ?s ?p ?o } }"),
            None
        );
        assert_eq!(derive_graph_iri("DELETE WHERE { GRAPH ?g { <a> <b> ?o } }"), None);
        assert_eq!(derive_graph_iri("INSERT DATA { <a> <b> <c> }"), None);
    }

    #[test]
    fn oversize_sparql_is_clamped_and_flagged() {
        let big = "x".repeat(SPARQL_MAX_BYTES + 500);
        let line = record_line(Path::new("/w/.base/graph.nq"), Change::Sparql(&big));
        assert!(line.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["sparql_truncated"], serde_json::json!(true));
        assert_eq!(v["sparql_bytes"], serde_json::json!(big.len()));
        assert_eq!(v["sparql"].as_str().unwrap().len(), SPARQL_MAX_BYTES);
        // The whole line still fits one write syscall.
        assert!(line.len() < SPARQL_MAX_BYTES + 4096);
    }

    #[test]
    fn every_record_states_its_origin() {
        // Never absent — the echo guard depends on being able to ask.
        let g = Path::new("/w/.base/graph.nq");
        for (change, want) in [
            (Change::Sparql("INSERT DATA { }"), "local"),
            (Change::Op("graph.compact"), "local"),
            (Change::RemoteOps(&[]), "remote"),
        ] {
            let v: serde_json::Value =
                serde_json::from_str(record_line(g, change).trim()).unwrap();
            assert_eq!(v["origin"], want, "origin for {change:?}");
        }
    }

    #[test]
    fn op_records_name_the_caller_and_carry_no_sparql() {
        let line = record_line(Path::new("/w/.base/graph.nq"), Change::Op("graph.compact"));
        let v: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["kind"], serde_json::json!("graph.compact"));
        assert!(v.get("sparql").is_none());
        assert_eq!(v["ws"], serde_json::json!("w"));
        assert!(v["at"].as_str().unwrap().contains('T'));
    }

    #[test]
    fn reader_advances_only_past_complete_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(LOG_FILE);

        // Nothing written yet.
        assert_eq!(cursor(&log), 0);
        let page = read_since(&log, 0).unwrap();
        assert!(page.lines.is_empty() && page.offset == 0 && !page.reset);

        append_line(&log, "{\"a\":1}\n").unwrap();
        append_line(&log, "{\"a\":2}\n").unwrap();
        let page = read_since(&log, 0).unwrap();
        assert_eq!(page.lines.len(), 2);
        assert_eq!(page.offset, cursor(&log));

        // Resuming from the returned offset yields nothing new.
        let tail = read_since(&log, page.offset).unwrap();
        assert!(tail.lines.is_empty());
        assert_eq!(tail.offset, page.offset);

        // A partial line is not consumed; the offset holds until it completes.
        std::fs::OpenOptions::new().append(true).open(&log).unwrap()
            .write_all(b"{\"a\":3}").unwrap();
        let partial = read_since(&log, page.offset).unwrap();
        assert!(partial.lines.is_empty());
        assert_eq!(partial.offset, page.offset);

        // Offset past the end means the log was replaced under the reader.
        let reset = read_since(&log, 999_999).unwrap();
        assert!(reset.reset && reset.lines.is_empty() && reset.offset == 0);
    }

    #[test]
    fn append_is_best_effort_and_never_panics() {
        // A directory where the log file should be: the open fails, `append`
        // swallows it, the caller's write still counts as done.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(LOG_FILE)).unwrap();
        append(&dir.path().join("graph.nq"), Change::Op("test"));
    }
}
