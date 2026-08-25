//! R1b — the delta gate, and what every SPARQL write records about its delta.
//!
//! Three properties this file exists to hold:
//!
//! 1. **Paired means shipped.** With `<home>/.base-gbl/sync-enabled` present, a
//!    knowledge write records fact-shaped `ops[]` and the client never parses
//!    SPARQL.
//! 2. **Unpaired means honest, not silent.** With the marker absent, the diff is
//!    skipped entirely — an install with no app pays nothing — and the record
//!    says `sync_disabled` rather than looking like a write with no changes.
//! 3. **Housekeeping never pays.** `lastRead` and `lastActive` fire on every
//!    recall and every tool call; they take no snapshot even when paired.
//!
//! Absent is not empty and is not zero: a record with no `ops[]` always names
//! the reason, so a client can render "N writes this base cannot ship" instead
//! of a confident, wrong "everything is synced".

use std::fs;
use std::path::{Path, PathBuf};

use base::changelog::{self, LOG_FILE};
use base::config::NamespaceConfig;
use base::crud;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// A workspace under an isolated home, with the pairing marker on or off.
fn workspace(home: &Path, paired: bool) -> PathBuf {
    let ws = home.join("proj");
    fs::create_dir_all(ws.join(".base")).unwrap();
    let tier = home.join(".base-gbl");
    fs::create_dir_all(&tier).unwrap();
    if paired {
        fs::write(tier.join("sync-enabled"), "").unwrap();
    }
    ws
}

fn records(ws: &Path) -> Vec<serde_json::Value> {
    let log = ws.join(".base").join(LOG_FILE);
    if !log.exists() {
        return Vec::new();
    }
    fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every log line must be valid JSON"))
        .collect()
}

fn last(ws: &Path) -> serde_json::Value {
    records(ws).pop().expect("a write must leave a record")
}

/// Every quad in a record's ops, flattened — what a client would ship.
fn shipped_quads(rec: &serde_json::Value) -> Vec<String> {
    rec["ops"]
        .as_array()
        .expect("ops[] must be an array")
        .iter()
        .flat_map(|op| {
            op["payload"]["quads"]
                .as_array()
                .expect("each op carries payload.quads")
                .iter()
                .map(|q| q.as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

// ─── (1) paired: the delta is captured ───────────────────────

#[test]
fn a_paired_knowledge_write_records_its_fact_shaped_delta() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), true);
        assert!(base::store::sync_enabled(), "precondition: the marker is present");

        crud::note::learn(&ws, &ns(), "deltas ship when paired", "insight", None, None, None)
            .unwrap();

        let rec = last(&ws);
        assert_eq!(rec["origin"], serde_json::json!("local"));
        assert!(rec.get("delta_unavailable").is_none(), "a captured delta names no gap");

        let quads = shipped_quads(&rec);
        assert!(!quads.is_empty(), "the write must ship at least one quad");
        assert!(
            quads.iter().any(|q| q.contains("deltas ship when paired")),
            "the note's own text must be in the shipped quads, got: {quads:?}"
        );
        assert!(
            quads.iter().all(|q| q.trim_end().ends_with('.')),
            "every shipped quad must be a valid N-Quads line, got: {quads:?}"
        );
    });
}

// ─── (2) unpaired: honest, never silent ──────────────────────

#[test]
fn an_unpaired_knowledge_write_says_why_it_has_no_delta() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), false);
        assert!(!base::store::sync_enabled(), "precondition: no marker");

        crud::note::learn(&ws, &ns(), "no app is paired", "insight", None, None, None).unwrap();

        let rec = last(&ws);
        assert!(rec.get("ops").is_none(), "no delta was taken");
        assert_eq!(
            rec["delta_unavailable"],
            serde_json::json!("sync_disabled"),
            "the gap must be named, or a client renders a confident wrong 'synced'"
        );
        assert_eq!(rec["origin"], serde_json::json!("local"));
    });
}

/// The marker is what pairing writes; base must follow it without a restart.
#[test]
fn pairing_switches_capture_on_for_the_next_write() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), false);
        crud::note::learn(&ws, &ns(), "before pairing", "insight", None, None, None).unwrap();
        assert_eq!(last(&ws)["delta_unavailable"], serde_json::json!("sync_disabled"));

        fs::write(tmp.path().join(".base-gbl").join("sync-enabled"), "").unwrap();
        crud::note::learn(&ws, &ns(), "after pairing", "insight", None, None, None).unwrap();

        let rec = last(&ws);
        assert!(rec.get("delta_unavailable").is_none(), "pairing takes effect immediately");
        assert!(shipped_quads(&rec).iter().any(|q| q.contains("after pairing")));
    });
}

// ─── (3) housekeeping never pays, even when paired ───────────

#[test]
fn a_housekeeping_write_takes_no_delta_even_when_paired() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), true);
        let slug =
            crud::note::learn(&ws, &ns(), "a note to read", "insight", None, None, None).unwrap();
        let iri = crud::build_iri(&ns(), "note", &slug);

        // `lastRead` stamping — fires on every `base recall`.
        crud::note::stamp_last_read(&ws, &ns(), std::slice::from_ref(&iri)).unwrap();

        let rec = last(&ws);
        assert!(rec.get("ops").is_none(), "a usage signal is not knowledge to ship");
        assert_eq!(
            rec["delta_unavailable"],
            serde_json::json!("housekeeping"),
            "delta-free BY INTENT reads differently from delta-free because unpaired"
        );
    });
}

// ─── (4) a retraction ships as a retire, not as SPARQL ───────

#[test]
fn deleting_a_note_ships_the_removed_quads_as_a_retire() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), true);
        let slug =
            crud::note::learn(&ws, &ns(), "this note gets deleted", "insight", None, None, None)
                .unwrap();

        assert!(crud::note::remove(&ws, &ns(), &slug).unwrap(), "the note existed");

        let rec = last(&ws);
        let ops = rec["ops"].as_array().expect("a retraction carries its delta");
        assert!(
            ops.iter().any(|op| op["type"] == serde_json::json!("retire")),
            "a delete must ship as a retire, not an assert: {ops:?}"
        );
        assert!(
            shipped_quads(&rec).iter().any(|q| q.contains("this note gets deleted")),
            "the retire must name the quads that were actually removed"
        );
        // A local delta mints no fact id: the portal keys idempotency on the
        // client's own counter, so base must not invent one.
        assert!(
            ops.iter().all(|op| op.get("fact_id").is_none()),
            "base must not mint fact ids for a local write"
        );
    });
}

// ─── (5) an update that spans graphs is a named gap ──────────

#[test]
fn a_multi_graph_update_on_a_hot_path_records_the_gap_rather_than_diffing_everything() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), true);
        crud::note::learn(&ws, &ns(), "seed", "insight", None, None, None).unwrap();

        let graph_path = ws.join(".base").join("graph.nq");
        let store = base::store::load_graph(&graph_path).unwrap();
        // `GRAPH ?g` names no target. On Scope::Target the honest answer is a
        // gap — diffing the whole store here would run on every tool call.
        base::store::update_and_write(
            &store,
            &graph_path,
            "DELETE WHERE { GRAPH ?g { <urn:absent> ?p ?o } }",
            base::store::Scope::Target,
            base::store::Intent::Knowledge,
        )
        .unwrap();

        assert_eq!(last(&ws)["delta_unavailable"], serde_json::json!("multi_graph"));
    });
}

/// The same update under `Scope::Wide` — for rare, destructive writes — pays the
/// whole-store diff and ships, because an unshipped retraction is unrecoverable.
#[test]
fn a_wide_scoped_retraction_ships_even_when_it_names_no_target_graph() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), true);
        let slug = crud::note::learn(&ws, &ns(), "wide retraction", "insight", None, None, None)
            .unwrap();
        let iri = crud::build_iri(&ns(), "note", &slug);

        let graph_path = ws.join(".base").join("graph.nq");
        let store = base::store::load_graph(&graph_path).unwrap();
        base::store::update_and_write(
            &store,
            &graph_path,
            &format!("DELETE WHERE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}"),
            base::store::Scope::Wide,
            base::store::Intent::Knowledge,
        )
        .unwrap();

        let rec = last(&ws);
        assert!(rec.get("delta_unavailable").is_none(), "Wide resolves what Target cannot");
        assert!(
            shipped_quads(&rec).iter().any(|q| q.contains("wide retraction")),
            "the whole-store diff must find the removed quads"
        );
    });
}

// ─── (6) the honest count a client renders ───────────────────

#[test]
fn the_log_lets_a_client_count_exactly_what_it_cannot_ship() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), true);
        let slug = crud::note::learn(&ws, &ns(), "shippable", "insight", None, None, None).unwrap();
        let iri = crud::build_iri(&ns(), "note", &slug);
        crud::note::stamp_last_read(&ws, &ns(), std::slice::from_ref(&iri)).unwrap();

        let all = records(&ws);
        // This is the computation `base changes` performs for `delta_free_count`.
        let delta_free = all.iter().filter(|r| !r["ops"].is_array()).count();
        assert_eq!(delta_free, 1, "exactly the housekeeping stamp is unshippable");
        assert!(
            all.iter()
                .filter(|r| !r["ops"].is_array())
                .all(|r| r["delta_unavailable"].is_string()),
            "every delta-free record must name its reason"
        );
    });
}

// ─── (7) round trip: local delta → another machine's store ───

#[test]
fn a_captured_delta_applies_on_a_second_machine_and_is_marked_remote() {
    let a = tempfile::tempdir().unwrap();
    let quads: Vec<String> = base::home::with_thread_home(a.path(), || {
        let ws = workspace(a.path(), true);
        crud::note::learn(&ws, &ns(), "travels between machines", "insight", None, None, None)
            .unwrap();
        shipped_quads(&last(&ws))
    });
    assert!(!quads.is_empty(), "machine A captured a delta");

    let b = tempfile::tempdir().unwrap();
    base::home::with_thread_home(b.path(), || {
        let ws = workspace(b.path(), true);
        let graph_path = ws.join(".base").join("graph.nq");

        // The app stamps identity onto base's delta before shipping it; base
        // reports only what changed.
        let ops = serde_json::json!([{
            "named_graph": "https://basemode.ai/g/6f1c",
            "type": "assert",
            "fact_id": "01ROUNDTRIP0001",
            "payload": { "quads": quads, "ws": "proj", "at": "2026-08-25T13:00:00Z" },
        }]);
        let (out, code) = base::apply_ops::run(&graph_path, &ops.to_string());
        assert_eq!(code, 0, "got {out}");
        assert_eq!(out["applied"], 1);

        assert!(
            fs::read_to_string(&graph_path).unwrap().contains("travels between machines"),
            "machine B now holds the fact"
        );
        assert_eq!(
            last(&ws)["origin"],
            serde_json::json!("remote"),
            "a pulled fact must never look local, or B ships it straight back"
        );
    });
}

/// `changes` reads what the writer wrote — the reader-side half of the contract.
#[test]
fn the_reader_sees_every_record_the_writer_appended() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), true);
        crud::note::learn(&ws, &ns(), "one", "insight", None, None, None).unwrap();
        crud::note::learn(&ws, &ns(), "two", "insight", None, None, None).unwrap();

        let log = changelog::log_path_for(&ws.join(".base").join("graph.nq"));
        let page = changelog::read_since(&log, 0).unwrap();
        assert_eq!(page.lines.len(), 2);
    });
}

// ─── benchmark ───────────────────────────────────────────────
//
// `cargo test --release --test sync_gate_test -- --ignored --nocapture bench`
//
// Measured on this machine, release, medians of 5 (real graph sizes here are
// 14,108 quads global and 39,328 workspace):
//
//     quads   | gate OFF | snapshot | diff+ops | paired total
//     --------+----------+----------+----------+-------------
//      14,108 |     1 µs |    30 ms |    64 ms |        94 ms
//      39,328 |     1 µs |   108 ms |   304 ms |       412 ms
//      60,000 |     1 µs |   147 ms |   444 ms |       591 ms
//
// The number that matters most is the first column: an install with no app
// paired pays ONE microsecond per write — a single `exists()` on the marker.
// Housekeeping is the same 1 µs even when paired, structurally: `capture` is
// false, so `snapshot_graphs` is never reached.
//
// Do NOT benchmark this through `base learn` end to end. Process start, graph
// load and N-Quads serialization dominate it, and on a loaded machine two runs
// of the SAME binary differed here by 170 ms — more than the seam being
// measured.
//
// Deliberately measures the SEAM and not `base learn` end to end: a whole
// command is dominated by process start, graph load and N-Quads serialization,
// and on a loaded machine two runs of the same binary differ by more than the
// thing being measured. Ask what the gate costs, not what the command costs.

#[test]
#[ignore = "benchmark"]
fn bench_seam_cost_by_graph_size() {
    use std::time::Instant;

    const GRAPH: &str = "http://ops-sys.local/graph/proj";
    let sizes = [14_108usize, 39_328, 60_000];

    println!("\n  seam cost per knowledge write (median of 5)\n");
    println!("  {:>8} | {:>10} | {:>10} | {:>10}", "quads", "gate OFF", "snapshot", "diff+ops");
    println!("  {:->8}-+-{:->10}-+-{:->10}-+-{:->10}", "", "", "", "");

    for n in sizes {
        let store = oxigraph::store::Store::new().unwrap();
        let mut buf = String::with_capacity(n * 60);
        for i in 0..n {
            buf.push_str(&format!("<urn:s/{i}> <urn:p/{}> \"filler {i}\" <{GRAPH}> .\n", i % 50));
        }
        store.load_from_reader(oxigraph::io::RdfFormat::NQuads, buf.as_bytes()).unwrap();
        let graphs = vec![GRAPH.to_string()];

        let med = |mut v: Vec<u128>| {
            v.sort_unstable();
            v[v.len() / 2]
        };

        // Gate OFF: the entire added cost is the marker stat.
        let off = med((0..5)
            .map(|_| {
                let t = Instant::now();
                let _ = base::store::sync_enabled();
                t.elapsed().as_micros()
            })
            .collect());

        let snap = med((0..5)
            .map(|_| {
                let t = Instant::now();
                let s = base::store::snapshot_graphs(&store, &graphs);
                std::hint::black_box(&s);
                t.elapsed().as_millis()
            })
            .collect());

        let diff = med((0..5)
            .map(|_| {
                let before = base::store::snapshot_graphs(&store, &graphs);
                let t = Instant::now();
                let ops = base::store::delta_since(&store, &graphs, before).to_ops();
                std::hint::black_box(&ops);
                t.elapsed().as_millis()
            })
            .collect());

        println!("  {n:>8} | {off:>8} µs | {snap:>7} ms | {diff:>7} ms");
    }
    println!();
}
