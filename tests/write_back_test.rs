//! `store::write_back` is the one function every graph write funnels through.
//! These tests pin what it must keep true while its dump is being buffered:
//! N quads in, N quads on disk; the bytes are the serializer's bytes; the
//! per-pid temp file is cleaned up the way it always was; one write, one
//! changelog record.
//!
//! Every path lives in the test's own tempdir. The `isolation-guard` feature
//! arms `home::assert_isolated_write`, so a write that escaped would panic.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use base::changelog::{Change, LOG_FILE};
use base::store;
use oxigraph::io::{RdfFormat, RdfSerializer};
use oxigraph::model::{GraphName, Literal, NamedNode, Quad};
use oxigraph::store::Store;

/// A store holding exactly `n` distinct quads in one named graph.
fn store_with(n: usize) -> Store {
    let store = Store::new().unwrap();
    let p = NamedNode::new("http://test.local/p").unwrap();
    let g = GraphName::NamedNode(NamedNode::new("http://test.local/g").unwrap());
    for i in 0..n {
        let s = NamedNode::new(format!("http://test.local/s/{i}")).unwrap();
        store
            .insert(&Quad::new(s, p.clone(), Literal::new_simple_literal(format!("v{i}")), g.clone()))
            .unwrap();
    }
    assert_eq!(store.len().unwrap(), n, "fixture must hold exactly n quads");
    store
}

/// `<root>/ws/.base/graph.nq`, with the directory made — the shape of a real tier.
fn graph_path(root: &Path) -> PathBuf {
    let base = root.join("ws").join(".base");
    fs::create_dir_all(&base).unwrap();
    base.join("graph.nq")
}

fn tmp_files(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".nq.tmp."))
        .collect();
    out.sort();
    out
}

fn log_lines(graph: &Path) -> usize {
    let log = graph.with_file_name(LOG_FILE);
    if !log.exists() {
        return 0;
    }
    fs::read_to_string(log).unwrap().lines().filter(|l| !l.trim().is_empty()).count()
}

/// What the serializer emits with no buffering in the way: the reference bytes.
fn raw_dump(store: &Store) -> Vec<u8> {
    store
        .dump_to_writer(RdfSerializer::from_format(RdfFormat::NQuads), Vec::new())
        .unwrap()
}

// ─── N in, N out ─────────────────────────────────────────────

fn round_trips(n: usize) {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());
    let store = store_with(n);

    store::write_back(&store, &path, Change::Op("test.round-trip")).unwrap();

    let reloaded = store::load_graph(&path).unwrap();
    assert_eq!(reloaded.len().unwrap(), n, "{n} quads written must parse back as {n} quads");
    let lines = fs::read_to_string(&path).unwrap().lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(lines, n, "N-Quads is one quad per line, so {n} quads is {n} non-empty lines");
    assert!(tmp_files(path.parent().unwrap()).is_empty(), "a completed write leaves no temp file");
}

#[test]
fn write_back_round_trips_an_empty_store() {
    round_trips(0);
}

#[test]
fn write_back_round_trips_one_quad() {
    round_trips(1);
}

#[test]
fn write_back_round_trips_a_thousand_quads() {
    round_trips(1_000);
}

#[test]
fn write_back_round_trips_sixty_thousand_quads() {
    round_trips(60_000);
}

// ─── the bytes are the serializer's bytes ───────────────────

#[test]
fn the_written_file_is_byte_identical_to_the_raw_serializer_output() {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());
    let store = store_with(60_000);

    store::write_back(&store, &path, Change::Op("test.bytes")).unwrap();

    let on_disk = fs::read(&path).unwrap();
    let reference = raw_dump(&store);
    assert!(!reference.is_empty(), "precondition: the reference dump has content");
    assert_eq!(on_disk.len(), reference.len(), "same length before comparing content");
    assert!(on_disk == reference, "the file on disk must be exactly what the serializer emitted");
}

// ─── the temp-file contract, pinned ──────────────────────────

#[test]
fn a_stale_temp_file_is_reaped_and_a_fresh_one_is_left_alone() {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());
    let dir = path.parent().unwrap();

    let stale = dir.join("graph.nq.tmp.999");
    fs::write(&stale, "stale").unwrap();
    fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();
    let fresh = dir.join("graph.nq.tmp.998");
    fs::write(&fresh, "fresh").unwrap();

    store::write_back(&store_with(3), &path, Change::Op("test.sweep")).unwrap();

    assert_eq!(
        tmp_files(dir),
        vec!["graph.nq.tmp.998".to_string()],
        "the 60 s rule: older temps are reaped, a live writer's temp is never touched"
    );
}

#[test]
fn a_rewrite_replaces_the_previous_file_completely() {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());

    store::write_back(&store_with(1_000), &path, Change::Op("test.first")).unwrap();
    store::write_back(&store_with(5), &path, Change::Op("test.second")).unwrap();

    assert_eq!(store::load_graph(&path).unwrap().len().unwrap(), 5, "the second write wins in full");
    assert_eq!(fs::read(&path).unwrap(), raw_dump(&store_with(5)));
}

// ─── one write, one record ───────────────────────────────────

#[test]
fn every_write_appends_exactly_one_changelog_record() {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());

    assert_eq!(log_lines(&path), 0);
    store::write_back(&store_with(2), &path, Change::Op("test.log-1")).unwrap();
    assert_eq!(log_lines(&path), 1);
    store::write_back(&store_with(2), &path, Change::Op("test.log-2")).unwrap();
    assert_eq!(log_lines(&path), 2);
}

// ─── the failure paths: an error is an error, the original survives ─────────

/// A sink that accepts `limit` bytes and then fails every write — a full disk,
/// a yanked drive, a permission that vanished mid-dump.
struct FailAfter<W: std::io::Write> {
    inner: W,
    left: usize,
}

impl<W: std::io::Write> std::io::Write for FailAfter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            return Err(std::io::Error::other("sink failed after its byte budget"));
        }
        let n = buf.len().min(self.left);
        let n = self.inner.write(&buf[..n])?;
        self.left -= n;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// A seeded graph on disk: (path, its bytes, its mtime, its changelog length).
fn seeded(root: &Path) -> (PathBuf, Vec<u8>, SystemTime, usize) {
    let path = graph_path(root);
    store::write_back(&store_with(200), &path, Change::Op("test.seed")).unwrap();
    let bytes = fs::read(&path).unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    (path.clone(), bytes, mtime, log_lines(&path))
}

fn assert_original_survived(path: &Path, bytes: &[u8], mtime: SystemTime, log: usize, what: &str) {
    assert_eq!(fs::read(path).unwrap(), bytes, "{what}: the original file is byte-identical");
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), mtime, "{what}: the original file was not rewritten");
    assert!(tmp_files(path.parent().unwrap()).is_empty(), "{what}: no temp file is left behind");
    assert_eq!(log_lines(path), log, "{what}: nothing was logged for a write that did not land");
}

#[test]
fn a_sink_that_fails_mid_dump_is_an_error_and_the_original_survives() {
    // 0: the very first write fails. 4096: inside the first buffer flush.
    // 1 MiB + 17: the second buffer flush, past the first full chunk.
    for limit in [0usize, 4096, (1 << 20) + 17] {
        let root = tempfile::tempdir().unwrap();
        let (path, bytes, mtime, log) = seeded(root.path());
        let big = store_with(60_000); // ~3.6 MB of N-Quads, several buffer flushes

        let result = store::write_back_seamed(
            &big,
            &path,
            Change::Op("test.fail"),
            |file| FailAfter { inner: file, left: limit },
            None,
        );

        let err = result.expect_err("a failed write must surface as an error");
        let msg = format!("{err:#}");
        assert!(msg.contains("sink failed after its byte budget"), "the sink's own error is in the chain, got: {msg}");
        assert_original_survived(&path, &bytes, mtime, log, &format!("limit={limit}"));
    }
}

#[test]
fn a_flush_failure_after_a_clean_dump_is_an_error_and_the_original_survives() {
    // The dump fits the 1 MiB buffer entirely, so the sink sees its first byte —
    // and fails — only at the explicit flush. This is the "tmp cut at a line
    // boundary parses clean" case the buffer must not be allowed to hide.
    let root = tempfile::tempdir().unwrap();
    let (path, bytes, mtime, log) = seeded(root.path());

    let result = store::write_back_seamed(
        &store_with(1_000),
        &path,
        Change::Op("test.flush"),
        |file| FailAfter { inner: file, left: 0 },
        None,
    );

    let msg = format!("{:#}", result.expect_err("the flush error must propagate"));
    assert!(msg.contains("Failed to flush"), "the flush step names itself, got: {msg}");
    assert_original_survived(&path, &bytes, mtime, log, "flush");
}

#[test]
fn a_quad_count_mismatch_is_an_error_naming_both_counts_and_the_original_survives() {
    let root = tempfile::tempdir().unwrap();
    let (path, bytes, mtime, log) = seeded(root.path());

    let result = store::write_back_seamed(&store_with(1_000), &path, Change::Op("test.count"), |file| file, Some(999));

    let msg = format!("{:#}", result.expect_err("a short file must not be renamed into place"));
    assert!(msg.contains("999") && msg.contains("1000"), "both counts are named, got: {msg}");
    assert_original_survived(&path, &bytes, mtime, log, "count");
}

#[test]
fn the_seam_with_no_injection_behaves_exactly_like_write_back() {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());
    store::write_back_seamed(&store_with(1_000), &path, Change::Op("test.identity"), |file| file, None).unwrap();
    assert_eq!(fs::read(&path).unwrap(), raw_dump(&store_with(1_000)));
    assert_eq!(log_lines(&path), 1);
}

// ─── Q4: what the count costs ────────────────────────────────

#[test]
#[ignore = "measurement, run with --ignored; prints Store::len() cost on 60k quads"]
fn measure_store_len_cost_on_sixty_thousand_quads() {
    let store = store_with(60_000);
    let t = std::time::Instant::now();
    let n = store.len().unwrap();
    let elapsed = t.elapsed();
    println!("Store::len() on {n} quads: {elapsed:?}");
    assert_eq!(n, 60_000);
}
