//! Applying inbound fact ops into the local graph — the pull half of desktop sync.
//!
//! The client ships local writes to the portal as fact ops and pulls the team's
//! ops back. base is the only writer of `graph.nq`, so the pull path has to be a
//! base verb: `base graph apply-ops` reads ops as JSON on stdin and applies them
//! through the normal write path, so `changes.jsonl` records them like any other
//! write.
//!
//! ## Why there is a ledger
//!
//! An op is `{named_graph, type, author_counter, fact_id, supersedes_fact_id,
//! payload, …}` — pinned by the portal's `StoreGraphOpsRequest`, where `payload`
//! is validated as nothing more specific than "an array". A **retire names a fact
//! id and carries no quads**. So base cannot know what to remove unless it
//! remembers what each fact asserted, and it cannot answer "already applied?" for
//! idempotency unless it remembers which ids it has seen.
//!
//! Both needs are one need, so applied facts are recorded in a reserved named
//! graph, [`LEDGER_GRAPH`], inside the same `graph.nq`. In the same file because
//! [`crate::store::write_back`] is one atomic temp+rename of one file: a ledger
//! inside it commits in the same instant as the facts it describes. A sidecar file
//! would have no atomicity with the graph, and the first crash between the two
//! writes leaves base holding facts it can no longer retire.
//!
//! The cost, stated plainly: a synced quad is stored twice, once as itself and
//! once as a ledger literal. That is the price of a correct retire, and it beats
//! trusting a delete instruction to say what it deletes.
//!
//! ## Why the ledger is invisible
//!
//! It is bookkeeping, not knowledge. `base recall` must never surface it. Two
//! things keep it out: it uses its own `urn:base:sync#` vocabulary, so every
//! predicate-bound query in this crate structurally cannot match it, and the
//! read-only loaders drop it outright — see [`crate::store::load_graphs`].

use std::path::Path;

use oxigraph::io::RdfFormat;
use oxigraph::model::{Literal, NamedNode, Quad};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;
use serde_json::{Value, json};

use crate::changelog::{AppliedOp, Change};

/// One quad as a re-parseable N-Quads line.
///
/// `Quad`'s `Display` omits the terminating `.`, which makes its output invalid
/// N-Quads — and the ledger's whole job is to hand that text back to the parser
/// on retire. Round-tripping through it is the point, so the dot goes on here,
/// once, rather than at each call site.
fn nquad_line(q: &Quad) -> String {
    format!("{q} .")
}

/// The reserved named graph holding the applied-fact ledger.
pub const LEDGER_GRAPH: &str = "urn:base:sync:facts";

/// Vocabulary for ledger predicates. Deliberately NOT the workspace namespace:
/// every read query in this crate binds on `<ns>:` predicates or types, so a
/// ledger in its own vocabulary cannot be matched by any of them even if a
/// loader-level exclusion is ever missed.
const LEDGER_NS: &str = "urn:base:sync#";

/// Node for one applied fact. `fact_id` is charset-validated before it ever
/// reaches here, so this cannot forge an IRI.
fn fact_node(fact_id: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("urn:base:sync:fact/{fact_id}"))
}

fn pred(name: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{LEDGER_NS}{name}"))
}

fn ledger_graph() -> NamedNode {
    NamedNode::new_unchecked(LEDGER_GRAPH)
}

// ─── Errors ──────────────────────────────────────────────────

/// A refusal, shaped for a machine. Every failure path exits non-zero with one of
/// these and applies nothing.
#[derive(Debug)]
pub struct OpError {
    pub code: &'static str,
    pub op_index: Option<usize>,
    pub message: String,
}

impl OpError {
    fn at(code: &'static str, op_index: usize, message: impl Into<String>) -> Self {
        Self { code, op_index: Some(op_index), message: message.into() }
    }
    fn whole(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, op_index: None, message: message.into() }
    }
    fn to_json(&self) -> Value {
        let mut e = serde_json::Map::new();
        e.insert("code".into(), self.code.into());
        if let Some(i) = self.op_index {
            e.insert("op_index".into(), i.into());
        }
        e.insert("message".into(), self.message.clone().into());
        json!({ "error": Value::Object(e) })
    }
}

// ─── Input ───────────────────────────────────────────────────

/// One op, validated and resolved to exactly what it will do to the store.
enum Prepared {
    Assert { fact_id: String, partition: String, quads: Vec<Quad> },
    Retire { fact_id: String },
}

/// Accepts either a bare array of ops or `{"ops": [...]}`.
///
/// Both, because the client forwards server response bodies straight through and
/// making it re-wrap them would be a sharp edge for no gain.
fn parse_input(input: &str) -> Result<Vec<Value>, OpError> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| OpError::whole("bad_json", format!("stdin is not valid JSON: {e}")))?;

    let arr = match v {
        Value::Array(a) => a,
        Value::Object(mut o) => match o.remove("ops") {
            Some(Value::Array(a)) => a,
            _ => {
                return Err(OpError::whole(
                    "bad_input",
                    "expected a JSON array of ops, or an object with an `ops` array",
                ));
            }
        },
        _ => {
            return Err(OpError::whole(
                "bad_input",
                "expected a JSON array of ops, or an object with an `ops` array",
            ));
        }
    };
    Ok(arr)
}

/// A fact id becomes an IRI, so it is charset-checked at the boundary rather than
/// trusted. ULIDs pass; anything that could break out of an IRI does not.
fn valid_fact_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 255
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'))
}

/// Validate every op before any of them is applied. This is what makes "applies
/// nothing on any invalid op" structural rather than a promise: the store is not
/// even loaded until this returns Ok.
fn prepare(ops: &[Value]) -> Result<Vec<Prepared>, OpError> {
    let mut out = Vec::with_capacity(ops.len());

    for (i, op) in ops.iter().enumerate() {
        let named_graph = op.get("named_graph").and_then(Value::as_str).unwrap_or("");
        if named_graph.is_empty() {
            return Err(OpError::at("missing_named_graph", i, "op has no `named_graph`"));
        }
        let partition = NamedNode::new(named_graph)
            .map_err(|e| OpError::at("bad_named_graph", i, format!("`named_graph` is not an IRI: {e}")))?;

        match op.get("type").and_then(Value::as_str) {
            Some("assert") => {
                let fact_id = op.get("fact_id").and_then(Value::as_str).unwrap_or("");
                if !valid_fact_id(fact_id) {
                    return Err(OpError::at(
                        "bad_fact_id",
                        i,
                        "an assert op needs a `fact_id` of [A-Za-z0-9._:-], 1-255 chars",
                    ));
                }
                let quads = quads_of(op, &partition)
                    .map_err(|m| OpError::at("bad_quads", i, m))?;
                if quads.is_empty() {
                    return Err(OpError::at("bad_quads", i, "an assert op asserts no quads"));
                }
                out.push(Prepared::Assert {
                    fact_id: fact_id.to_string(),
                    partition: partition.into_string(),
                    quads,
                });
            }
            Some("retire") => {
                let fact_id = op.get("supersedes_fact_id").and_then(Value::as_str).unwrap_or("");
                if !valid_fact_id(fact_id) {
                    return Err(OpError::at(
                        "bad_supersedes_fact_id",
                        i,
                        "a retire op must name the fact it retires",
                    ));
                }
                out.push(Prepared::Retire { fact_id: fact_id.to_string() });
            }
            other => {
                return Err(OpError::at(
                    "bad_type",
                    i,
                    format!("`type` must be \"assert\" or \"retire\", got {other:?}"),
                ));
            }
        }
    }
    Ok(out)
}

/// Parse `payload.quads[]` into quads placed in their final graph.
///
/// A quad that names its own graph keeps it — base's named graphs are workspace
/// and domain shaped and stable across machines, so the sender's own graph name is
/// already the right local answer, and the local graph does not fill up with
/// portal UUIDs. A quad with no graph component falls back to the op's partition.
fn quads_of(op: &Value, partition: &NamedNode) -> Result<Vec<Quad>, String> {
    let Some(list) = op.pointer("/payload/quads").and_then(Value::as_array) else {
        return Err("op has no `payload.quads` array".into());
    };

    let mut text = String::new();
    for (n, q) in list.iter().enumerate() {
        let line = q.as_str().ok_or_else(|| format!("`payload.quads[{n}]` is not a string"))?;
        text.push_str(line.trim_end());
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }

    // Parse through a scratch store rather than a bare parser: this is the exact
    // load path the rest of the crate uses, so a line base would refuse on disk is
    // refused here too.
    let scratch = Store::new().map_err(|e| format!("scratch store: {e}"))?;
    scratch
        .load_from_reader(RdfFormat::NQuads, text.as_bytes())
        .map_err(|e| format!("not valid N-Quads: {e}"))?;

    let mut out = Vec::new();
    for q in scratch.iter() {
        let q = q.map_err(|e| format!("reading parsed quads: {e}"))?;
        let placed = if q.graph_name.is_default_graph() {
            Quad::new(q.subject, q.predicate, q.object, partition.clone())
        } else {
            q
        };
        out.push(placed);
    }
    Ok(out)
}

// ─── Apply ───────────────────────────────────────────────────

/// What one invocation did. Every op lands in exactly one of these counters —
/// absent is never folded into zero (law 7).
#[derive(Debug, Default)]
pub struct Outcome {
    pub applied: usize,
    pub skipped_duplicate: usize,
    /// A retire naming a fact this machine never applied. Normal traffic between
    /// machines, not an error: erroring would deadlock the pull on that page
    /// forever, and counting it as applied would be a lie.
    pub skipped_unknown: usize,
    pub records: Vec<AppliedOp>,
}

impl Outcome {
    fn to_json(&self) -> Value {
        json!({
            "applied": self.applied,
            "skipped_duplicate": self.skipped_duplicate,
            "skipped_unknown": self.skipped_unknown,
        })
    }
}

/// The ledger's record of one fact: its state, and the quads it asserted.
fn ledger_lookup(store: &Store, fact_id: &str) -> (Option<String>, Vec<String>) {
    let node = fact_node(fact_id);
    let q = format!(
        "SELECT ?state ?quad WHERE {{ GRAPH <{LEDGER_GRAPH}> {{\n\
           <{node}> <{LEDGER_NS}state> ?state .\n\
           OPTIONAL {{ <{node}> <{LEDGER_NS}quad> ?quad }}\n\
         }} }}",
        node = node.as_str(),
    );

    let mut state = None;
    let mut quads = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.filter_map(|r| r.ok()) {
            if state.is_none()
                && let Some(oxigraph::model::Term::Literal(l)) = row.get("state")
            {
                state = Some(l.value().to_string());
            }
            if let Some(oxigraph::model::Term::Literal(l)) = row.get("quad") {
                quads.push(l.value().to_string());
            }
        }
    }
    (state, quads)
}

fn ledger_write(store: &Store, fact_id: &str, partition: &str, state: &str, quads: &[Quad]) {
    let node = fact_node(fact_id);
    let g = ledger_graph();
    let _ = store.insert(&Quad::new(node.clone(), pred("factId"), Literal::new_simple_literal(fact_id), g.clone()));
    let _ = store.insert(&Quad::new(
        node.clone(),
        pred("partition"),
        NamedNode::new_unchecked(partition.to_string()),
        g.clone(),
    ));
    let _ = store.insert(&Quad::new(
        node.clone(),
        pred("appliedAt"),
        Literal::new_simple_literal(crate::crud::now_iso()),
        g.clone(),
    ));
    let _ = store.insert(&Quad::new(
        node.clone(),
        pred("state"),
        Literal::new_simple_literal(state),
        g.clone(),
    ));
    for q in quads {
        let _ = store.insert(&Quad::new(
            node.clone(),
            pred("quad"),
            Literal::new_simple_literal(nquad_line(q)),
            g.clone(),
        ));
    }
}

fn ledger_set_state(store: &Store, fact_id: &str, from: &str, to: &str) {
    let node = fact_node(fact_id);
    let g = ledger_graph();
    let _ = store.remove(&Quad::new(
        node.clone(),
        pred("state"),
        Literal::new_simple_literal(from),
        g.clone(),
    ));
    let _ = store.insert(&Quad::new(node, pred("state"), Literal::new_simple_literal(to), g));
}

/// Drop a retired fact's stored quads from the ledger.
///
/// They exist only so a retire knows what to remove; once it has, keeping them
/// leaves the retired content sitting in `graph.nq` forever, which is both waste
/// and a small surprise for anyone who retires something on purpose. The `state`
/// marker stays, so re-applying the same retire is still `skipped_duplicate`
/// rather than `skipped_unknown`.
fn ledger_forget_quads(store: &Store, fact_id: &str, quad_texts: &[String]) {
    let node = fact_node(fact_id);
    let g = ledger_graph();
    for text in quad_texts {
        let _ = store.remove(&Quad::new(
            node.clone(),
            pred("quad"),
            Literal::new_simple_literal(text.as_str()),
            g.clone(),
        ));
    }
}

/// Apply prepared ops to an in-memory store. Pure with respect to the filesystem —
/// the single `write_back` happens in [`run`], so one invocation is one transaction.
fn apply(store: &Store, prepared: &[Prepared]) -> Result<Outcome, OpError> {
    let mut out = Outcome::default();

    for p in prepared {
        match p {
            Prepared::Assert { fact_id, partition, quads } => {
                if ledger_lookup(store, fact_id).0.is_some() {
                    out.skipped_duplicate += 1;
                    continue;
                }
                for q in quads {
                    let _ = store.insert(q);
                }
                ledger_write(store, fact_id, partition, "asserted", quads);
                out.applied += 1;
                out.records.push(AppliedOp::assert(
                    fact_id.clone(),
                    quads.iter().map(nquad_line).collect(),
                ));
            }
            Prepared::Retire { fact_id } => {
                let (state, quad_texts) = ledger_lookup(store, fact_id);
                match state.as_deref() {
                    None => {
                        out.skipped_unknown += 1;
                    }
                    Some("retired") => {
                        out.skipped_duplicate += 1;
                    }
                    Some(_) => {
                        // If the ledger's own text will not re-parse, the retire
                        // would remove nothing while reporting success — the exact
                        // silent no-op this whole path exists to avoid. Fail loud.
                        let scratch = Store::new().expect("in-memory store");
                        let joined = quad_texts.join("\n");
                        scratch.load_from_reader(RdfFormat::NQuads, joined.as_bytes()).map_err(|e| {
                            OpError::whole(
                                "ledger_corrupt",
                                format!("fact {fact_id}'s recorded quads are not valid N-Quads: {e}"),
                            )
                        })?;
                        let mut removed = Vec::new();
                        for q in scratch.iter().filter_map(Result::ok) {
                            let _ = store.remove(&q);
                            removed.push(nquad_line(&q));
                        }
                        ledger_set_state(store, fact_id, "asserted", "retired");
                        ledger_forget_quads(store, fact_id, &quad_texts);
                        out.applied += 1;
                        out.records.push(AppliedOp::retire(fact_id.clone(), removed));
                    }
                }
            }
        }
    }
    Ok(out)
}

// ─── Entry point ─────────────────────────────────────────────

/// Read ops, apply them, write once. Returns the JSON to print and the exit code.
///
/// Validation happens before the graph is loaded, so an invalid batch cannot have
/// half-applied: there is nothing to half-apply yet.
pub fn run(graph_path: &Path, input: &str) -> (Value, i32) {
    let ops = match parse_input(input) {
        Ok(o) => o,
        Err(e) => return (e.to_json(), 1),
    };
    let prepared = match prepare(&ops) {
        Ok(p) => p,
        Err(e) => return (e.to_json(), 1),
    };
    if prepared.is_empty() {
        return (Outcome::default().to_json(), 0);
    }

    // A machine that has never written anything has no graph.nq yet, and a first
    // pull is exactly that case — so start from an empty store rather than
    // refusing, matching `crud::load_workspace_store`, the write path `base learn`
    // already takes on a fresh home.
    let store = if graph_path.exists() {
        match crate::store::load_graph(graph_path) {
            Ok(s) => s,
            Err(e) => {
                return (OpError::whole("graph_load_failed", e.to_string()).to_json(), 1);
            }
        }
    } else {
        match Store::new() {
            Ok(s) => s,
            Err(e) => {
                return (OpError::whole("store_init_failed", e.to_string()).to_json(), 1);
            }
        }
    };

    let outcome = match apply(&store, &prepared) {
        Ok(o) => o,
        Err(e) => return (e.to_json(), 1),
    };

    // Nothing changed → no write, and therefore no changes.jsonl record. A
    // duplicate batch must not leave a second record behind.
    if outcome.records.is_empty() {
        return (outcome.to_json(), 0);
    }

    if let Err(e) = crate::store::write_back(&store, graph_path, Change::RemoteOps(&outcome.records))
    {
        return (OpError::whole("write_failed", e.to_string()).to_json(), 1);
    }
    (outcome.to_json(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_at(dir: &Path) -> std::path::PathBuf {
        let base = dir.join(".base");
        std::fs::create_dir_all(&base).unwrap();
        let g = base.join("graph.nq");
        std::fs::write(&g, "").unwrap();
        g
    }

    fn assert_op(fact_id: &str, quad: &str) -> String {
        json!({
            "named_graph": "https://basemode.ai/g/6f1c",
            "type": "assert",
            "author_counter": 1,
            "fact_id": fact_id,
            "payload": { "quads": [quad] }
        })
        .to_string()
    }

    const Q: &str = "<urn:s/1> <urn:p/name> \"Ada\" <urn:g/ws> .";

    #[test]
    fn applies_a_batch_and_records_one_change() {
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        let (out, code) = run(&g, &format!("[{}]", assert_op("01ABC", Q)));
        assert_eq!(code, 0, "got {out}");
        assert_eq!(out["applied"], 1);

        let body = std::fs::read_to_string(&g).unwrap();
        assert!(body.contains("<urn:s/1>"), "fact landed: {body}");

        let log = std::fs::read_to_string(crate::changelog::log_path_for(&g)).unwrap();
        assert_eq!(log.lines().count(), 1, "exactly one change record");
        assert!(log.contains("\"origin\":\"remote\""), "tagged remote: {log}");
    }

    #[test]
    fn first_pull_on_a_machine_with_no_graph_yet() {
        // A new machine's first pull arrives before it has ever written anything,
        // so there is no graph.nq to load. Refusing here would make the very first
        // sync the one that cannot work.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join(".base");
        std::fs::create_dir_all(&base).unwrap();
        let g = base.join("graph.nq");
        assert!(!g.exists(), "precondition: no graph file");

        let (out, code) = run(&g, &format!("[{}]", assert_op("01FIRST", Q)));
        assert_eq!(code, 0, "got {out}");
        assert_eq!(out["applied"], 1);
        assert!(std::fs::read_to_string(&g).unwrap().contains("<urn:s/1>"), "graph created");
    }

    #[test]
    fn accepts_both_bare_array_and_ops_object() {
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        let (_, code) = run(&g, &format!("{{\"ops\":[{}]}}", assert_op("01ABC", Q)));
        assert_eq!(code, 0);
        let (out, code) = run(&g, &format!("[{}]", assert_op("01DEF", Q)));
        assert_eq!(code, 0);
        assert_eq!(out["applied"], 1);
    }

    #[test]
    fn duplicate_fact_is_skipped_and_writes_no_second_record() {
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        let batch = format!("[{}]", assert_op("01ABC", Q));
        run(&g, &batch);
        let (out, code) = run(&g, &batch);
        assert_eq!(code, 0);
        assert_eq!(out["applied"], 0);
        assert_eq!(out["skipped_duplicate"], 1);

        let log = std::fs::read_to_string(crate::changelog::log_path_for(&g)).unwrap();
        assert_eq!(log.lines().count(), 1, "a duplicate batch adds no record");
    }

    #[test]
    fn retire_removes_exactly_what_the_fact_asserted() {
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        let other = "<urn:s/2> <urn:p/name> \"Grace\" <urn:g/ws> .";
        run(&g, &format!("[{},{}]", assert_op("01ABC", Q), assert_op("01DEF", other)));

        let retire = json!({
            "named_graph": "https://basemode.ai/g/6f1c",
            "type": "retire",
            "author_counter": 2,
            "supersedes_fact_id": "01ABC",
            "payload": {}
        });
        let (out, code) = run(&g, &format!("[{retire}]"));
        assert_eq!(code, 0, "got {out}");
        assert_eq!(out["applied"], 1);

        let body = std::fs::read_to_string(&g).unwrap();
        assert!(!body.contains("<urn:s/1>"), "retired fact is gone; body:\n{body}");
        assert!(
            !body.contains("<urn:base:sync#quad> \"<urn:s/1>"),
            "and the ledger stops carrying its text: {body}"
        );
        assert!(body.contains("<urn:s/2>"), "the other fact is untouched: {body}");
    }

    #[test]
    fn retiring_an_unknown_fact_is_counted_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        let retire = json!({
            "named_graph": "https://basemode.ai/g/6f1c",
            "type": "retire",
            "supersedes_fact_id": "01NEVERSEEN",
            "payload": {}
        });
        let (out, code) = run(&g, &format!("[{retire}]"));
        assert_eq!(code, 0, "a retire for an unseen fact is normal traffic");
        assert_eq!(out["skipped_unknown"], 1);
        assert_eq!(out["applied"], 0);
    }

    #[test]
    fn an_invalid_op_applies_nothing_and_names_its_index() {
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        let bad = json!({ "named_graph": "https://basemode.ai/g/6f1c", "type": "explode" });
        let (out, code) = run(&g, &format!("[{},{}]", assert_op("01ABC", Q), bad));
        assert_eq!(code, 1);
        assert_eq!(out["error"]["code"], "bad_type");
        assert_eq!(out["error"]["op_index"], 1);

        assert_eq!(std::fs::read_to_string(&g).unwrap(), "", "nothing applied");
        assert!(!crate::changelog::log_path_for(&g).exists(), "and nothing logged");
    }

    #[test]
    fn a_fact_id_cannot_forge_an_iri() {
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        let (out, code) = run(&g, &format!("[{}]", assert_op("01A> <urn:evil", Q)));
        assert_eq!(code, 1);
        assert_eq!(out["error"]["code"], "bad_fact_id");
    }

    #[test]
    fn a_quad_without_a_graph_falls_back_to_the_op_partition() {
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        run(&g, &format!("[{}]", assert_op("01ABC", "<urn:s/9> <urn:p/name> \"Ada\" .")));
        let body = std::fs::read_to_string(&g).unwrap();
        assert!(body.contains("<https://basemode.ai/g/6f1c>"), "placed in the partition: {body}");
    }

    // ─── The ledger must survive maintenance, and stay invisible ───

    #[test]
    fn ledger_survives_compact_and_purge() {
        // D3. Compact is a whole-store round trip and purge only ever selects
        // `<ns>:Note` nodes, so neither can touch the ledger today — this test is
        // here for the day someone changes one of them.
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        let batch = format!("[{}]", assert_op("01ABC", Q));
        run(&g, &batch);

        crate::graph::compact_tier(&g).expect("compact");
        let ns = crate::config::NamespaceConfig::default();
        crate::graph::purge_stale(&g, &ns, 0, true).expect("purge");

        let body = std::fs::read_to_string(&g).unwrap();
        assert!(body.contains(LEDGER_GRAPH), "ledger survived maintenance: {body}");

        // The real proof: idempotency still works, which is what the ledger is for.
        let (out, code) = run(&g, &batch);
        assert_eq!(code, 0);
        assert_eq!(out["skipped_duplicate"], 1, "still recognised as already applied");
    }

    #[test]
    fn ledger_is_invisible_to_read_loaders() {
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        run(&g, &format!("[{}]", assert_op("01ABC", Q)));

        let store = crate::store::load_graphs(&[&g]).expect("read load");

        let ledger_rows = format!(
            "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{LEDGER_GRAPH}> {{ ?s ?p ?o }} }}"
        );
        assert_eq!(count(&store, &ledger_rows), 0, "a read never sees the ledger");

        // A union read — the shape `base recall` and the dashboard use — is clean too.
        let any_sync = "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o . FILTER(CONTAINS(STR(?p), \"urn:base:sync\")) }";
        assert_eq!(
            crate::store::query_union(&store, any_sync)
                .ok()
                .and_then(|r| first_count(r)),
            Some(0),
            "no ledger predicate reaches a union read"
        );

        // And the fact itself is still perfectly visible.
        let fact = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH <urn:g/ws> { ?s ?p ?o } }";
        assert_eq!(count(&store, fact), 1, "the synced fact IS readable");
    }

    fn count(store: &Store, q: &str) -> u64 {
        crate::store::query(store, q).ok().and_then(first_count).unwrap_or(u64::MAX)
    }

    fn first_count(r: QueryResults) -> Option<u64> {
        let QueryResults::Solutions(sols) = r else { return None };
        for row in sols.filter_map(|x| x.ok()) {
            if let Some(oxigraph::model::Term::Literal(l)) = row.get("n") {
                return l.value().parse().ok();
            }
        }
        None
    }

    #[test]
    fn a_quad_keeps_its_own_graph_when_it_names_one() {
        let dir = tempfile::tempdir().unwrap();
        let g = graph_at(dir.path());
        run(&g, &format!("[{}]", assert_op("01ABC", Q)));
        let body = std::fs::read_to_string(&g).unwrap();
        assert!(body.contains("<urn:g/ws>"), "sender's graph honoured: {body}");
        assert!(
            !body.contains("<urn:s/1> <urn:p/name> \"Ada\" <https://basemode.ai/g/6f1c>"),
            "and not re-homed into the partition: {body}"
        );
    }
}
