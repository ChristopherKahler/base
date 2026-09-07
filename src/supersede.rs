//! Supersession: the one place that knows what "this record replaced that one" is
//! spelled as, mirroring [`crate::domain::link`].
//!
//! `ops:supersedes` and `ops:supersededBy` have been declared in `ops.ttl` since
//! before 0.13.19 (`:136` and `:139-140`, inverse of each other) and **nothing has
//! ever written either one** — measured on Chris's store, 2026-09-07: 0 quads of
//! each across both tiers. The only supersession base has today is
//! `supersedes_fact_id`, a JSON field in the basemode sync ledger
//! (`apply_ops.rs:11,189-192`, `changelog.rs:82-102`), which no graph reader can
//! see. So a correction is stored beside the thing it corrects, both come back
//! from `recall` together, and nothing says which one is live. That is the drift
//! this module exists to end.
//!
//! ## The edge is the truth; the status is a label
//!
//! Every decision path keys on [`PRED_SUPERSEDED_BY`] and nothing else. The writer
//! also sets `ops:status "superseded"` on the old record so the dashboard and
//! `learn --list` do not show it as active, but **no reader may require both**:
//! `ops:status` is unvalidated free text. Measured on the frozen copy, both tiers:
//! 44 distinct values against the six `ops.ttl:117` declares, including case
//! variants used as separate values (`Pass` 242, `PASS` 43, `PASS (no change)` 1)
//! and whole sentences (`"no silent loss"`, `"CWD outside any ws → global"`, one
//! truncated markdown paragraph). One record already carries the status with no
//! edge. `base doctor` reports the disagreement in both directions rather than any
//! reader trying to reconcile it.
//!
//! ## Chains
//!
//! A → B → C is three records and two edges; nothing is ever re-pointed.
//! [`resolve_head`] walks `supersededBy` forward to the live end, so `resolve_head(A)`
//! is C. A cycle is refused at write time by [`would_cycle`]; if one reaches the
//! store by another route the walk still terminates on its visited set rather than
//! hanging a session-start hook.

use std::collections::BTreeSet;

use oxigraph::model::{NamedNodeRef, Term};
use oxigraph::store::Store;

use crate::config::NamespaceConfig;

/// `new ops:supersedes old` — written on the SUCCESSOR, naming what it replaced.
pub const PRED_SUPERSEDES: &str = "supersedes";

/// `old ops:supersededBy new` — the inverse, written in the same statement. This is
/// the predicate every reader keys on.
pub const PRED_SUPERSEDED_BY: &str = "supersededBy";

/// The `ops:status` value the writer stamps on a superseded record. Written for the
/// dashboard, never read by a decision path — see the module docs.
pub const STATUS_SUPERSEDED: &str = "superseded";

/// `FILTER NOT EXISTS {{ ?var ops:supersededBy ?var_supersededBy }}` — "serve only the
/// live version".
///
/// **This must be interpolated INSIDE the `GRAPH ?g {{ … }}` group it filters**, next
/// to the arm's other filters. A triple pattern outside every GRAPH group is matched
/// against the default graph, where base keeps nothing, so `NOT EXISTS` is always
/// true and the filter silently excludes nothing while reading like a working filter.
/// That is not hypothetical: it shipped once as F16 (`crud/note.rs`, kite, 2026-09-06),
/// where a transient filter placed after the last UNION arm let `recall --keyword`
/// keep printing pings.
///
/// The bound variable is derived from `var` so interpolating this into an arm that
/// already binds `?x` cannot capture it.
pub fn sparql_exclude_superseded(ns: &NamespaceConfig, var: &str) -> String {
    let p = &ns.prefix;
    // Trailing newline, matching `ontology::transient::sparql_exclude`, so the two
    // interpolate identically inside an arm: `{no_transient}{no_superseded}`.
    format!("FILTER NOT EXISTS {{ ?{var} {p}:{PRED_SUPERSEDED_BY} ?{var}_supersededBy }}\n")
}

/// The live end of `iri`'s supersession chain, or `iri` itself when nothing
/// supersedes it.
///
/// Walks `supersededBy` forward across ANY graph: a workspace correction to a global
/// note puts both edges in the workspace graph (the tier of the NEW record, per the
/// G0 verdict), and a merged read must still find them.
///
/// Terminates on a visited set. A cycle cannot be written — [`would_cycle`] refuses
/// it — but the walk runs inside the prompt-submit hook, and a hook that hangs on
/// malformed data is worse than one that stops early and lets `doctor` report it.
/// A record with two successors is likewise a defect the writer prevents; if one
/// exists the lexicographically first is taken so two runs agree.
pub fn resolve_head(store: &Store, ns: &NamespaceConfig, iri: &str) -> String {
    let pred_iri = format!("{}{PRED_SUPERSEDED_BY}", ns.uri);
    let Ok(pred) = NamedNodeRef::new(&pred_iri) else {
        return iri.to_string();
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    seen.insert(iri.to_string());
    let mut cur = iri.to_string();

    loop {
        let Ok(subject) = NamedNodeRef::new(&cur) else { return cur };
        // Lexicographically first successor, so a two-successor defect still
        // resolves the same way on every run.
        let next = store
            .quads_for_pattern(Some(subject.into()), Some(pred), None, None)
            .filter_map(|q| q.ok())
            .filter_map(|q| match q.object {
                Term::NamedNode(n) => Some(n.into_string()),
                _ => None,
            })
            .min();
        match next {
            Some(n) if seen.insert(n.clone()) => cur = n,
            // No successor, or one we have already walked through: this is the end
            // of the chain we can trust.
            _ => return cur,
        }
    }
}

/// True when making `new_iri` supersede `old_iri` would close a cycle — i.e. `old_iri`
/// is already reachable by walking forward from `new_iri`.
///
/// Checked BEFORE anything is inserted, so a refusal leaves the store untouched.
pub fn would_cycle(store: &Store, ns: &NamespaceConfig, old_iri: &str, new_iri: &str) -> bool {
    if old_iri == new_iri {
        return true;
    }
    let pred_iri = format!("{}{PRED_SUPERSEDED_BY}", ns.uri);
    let Ok(pred) = NamedNodeRef::new(&pred_iri) else {
        return false;
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    seen.insert(new_iri.to_string());
    let mut cur = new_iri.to_string();

    loop {
        let Ok(subject) = NamedNodeRef::new(&cur) else { return false };
        let next = store
            .quads_for_pattern(Some(subject.into()), Some(pred), None, None)
            .filter_map(|q| q.ok())
            .filter_map(|q| match q.object {
                Term::NamedNode(n) => Some(n.into_string()),
                _ => None,
            })
            .min();
        match next {
            Some(n) if n == old_iri => return true,
            Some(n) if seen.insert(n.clone()) => cur = n,
            _ => return false,
        }
    }
}

/// The whole supersession fact as ONE `INSERT DATA`: the forward edge, its inverse,
/// and the status label, into `graph_iri`.
///
/// One statement on purpose. Three separate updates can be interrupted between the
/// second and the third, leaving a record that reads as superseded to one surface and
/// live to another — the exact ambiguity this fork exists to remove.
///
/// `graph_iri` is the tier of the NEW record (G0 verdict): a workspace correction to a
/// global note puts both edges in the workspace graph, where [`resolve_head`] finds
/// them on a merged read.
pub fn link_update(ns: &NamespaceConfig, graph_iri: &str, old_iri: &str, new_iri: &str) -> String {
    let p = &ns.prefix;
    format!(
        "INSERT DATA {{ GRAPH <{graph_iri}> {{\n\
        \x20 <{new_iri}> {p}:{PRED_SUPERSEDES} <{old_iri}> .\n\
        \x20 <{old_iri}> {p}:{PRED_SUPERSEDED_BY} <{new_iri}> .\n\
        \x20 <{old_iri}> {p}:status \"{STATUS_SUPERSEDED}\" .\n\
        }} }}"
    )
}

/// What `base doctor` reports about supersession on one tier.
///
/// Every field is a COUNT or a list of IRIs, never a judgement: doctor is where an
/// operator goes looking for problems, so it states what is there and lets them
/// decide. The two disagreement counts are deliberately separate — a record with the
/// status and no edge is a pre-0.14.0 artefact (one exists in Chris's store today),
/// while a record with the edge and no status is a writer that half-ran, and folding
/// them into one number would hide which of those happened.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Audit {
    /// Records carrying `supersededBy`.
    pub superseded: usize,
    /// Chains longer than three links, by their starting record.
    pub long_chains: Vec<String>,
    /// Records whose forward walk revisits a node. The writer refuses these; one here
    /// arrived by another route and is a defect, not a warning.
    pub cycles: Vec<String>,
    /// `status "superseded"` with no `supersededBy` edge.
    pub status_without_edge: usize,
    /// `supersededBy` edge with no `status "superseded"`.
    pub edge_without_status: usize,
    /// `noteType "correction"` notes that name nothing they correct.
    ///
    /// Reported here, once, rather than warned about at write time: Chris writes
    /// corrections daily, and a per-write nag fires several times a day, is ignored
    /// within a week, and trains him to ignore the next warning that matters (G0 Q1,
    /// ruled 2026-09-06).
    pub corrections_naming_nothing: usize,
}

impl Audit {
    /// Nothing to report — doctor prints no supersession line at all in this case,
    /// so a store that has never used the feature reads exactly as it did before.
    pub fn is_silent(&self) -> bool {
        *self == Audit::default()
    }
}

/// Audit one loaded store. One pass per predicate, then the walks — this runs inside
/// `base doctor`, which already loads the store, so it adds no parse.
pub fn audit(store: &Store, ns: &NamespaceConfig) -> Audit {
    let mut out = Audit::default();
    // Owned first: `NamedNodeRef` borrows, so the String has to outlive the ref.
    let mk = |local: &str| format!("{}{local}", ns.uri);
    let (sup_by_s, status_s, note_type_s) =
        (mk(PRED_SUPERSEDED_BY), mk("status"), mk("noteType"));
    let (Ok(sup_by), Ok(status_p), Ok(note_type)) = (
        NamedNodeRef::new(&sup_by_s),
        NamedNodeRef::new(&status_s),
        NamedNodeRef::new(&note_type_s),
    ) else {
        return out;
    };

    let subjects_of = |pred: NamedNodeRef<'_>| -> BTreeSet<String> {
        store
            .quads_for_pattern(None, Some(pred), None, None)
            .filter_map(|q| q.ok())
            .filter_map(|q| match q.subject {
                oxigraph::model::Subject::NamedNode(n) => Some(n.into_string()),
                _ => None,
            })
            .collect()
    };

    let has_edge = subjects_of(sup_by);
    out.superseded = has_edge.len();

    let marked: BTreeSet<String> = store
        .quads_for_pattern(None, Some(status_p), None, None)
        .filter_map(|q| q.ok())
        .filter(|q| matches!(&q.object, Term::Literal(l) if l.value() == STATUS_SUPERSEDED))
        .filter_map(|q| match &q.subject {
            oxigraph::model::Subject::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect();

    out.status_without_edge = marked.difference(&has_edge).count();
    out.edge_without_status = has_edge.difference(&marked).count();

    // Chain length and cycles, walked from every record that has a successor. A
    // cycle is detected by the walk revisiting, which is the same visited-set rule
    // `resolve_head` uses, so the two cannot disagree about what a cycle is.
    for start in &has_edge {
        let mut seen = BTreeSet::new();
        seen.insert(start.clone());
        let mut cur = start.clone();
        let mut hops = 0usize;
        while let Ok(subject) = NamedNodeRef::new(&cur) {
            let next = store
                .quads_for_pattern(Some(subject.into()), Some(sup_by), None, None)
                .filter_map(|q| q.ok())
                .filter_map(|q| match q.object {
                    Term::NamedNode(n) => Some(n.into_string()),
                    _ => None,
                })
                .min();
            match next {
                Some(n) if seen.insert(n.clone()) => {
                    cur = n;
                    hops += 1;
                }
                Some(_) => {
                    out.cycles.push(start.clone());
                    break;
                }
                None => break,
            }
        }
        if hops > 3 {
            out.long_chains.push(start.clone());
        }
    }

    // A correction that names nothing it corrects.
    out.corrections_naming_nothing = store
        .quads_for_pattern(None, Some(note_type), None, None)
        .filter_map(|q| q.ok())
        .filter(|q| matches!(&q.object, Term::Literal(l) if l.value() == "correction"))
        .filter_map(|q| match &q.subject {
            oxigraph::model::Subject::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect::<BTreeSet<String>>()
        .into_iter()
        .filter(|s| {
            let sup_s = mk(PRED_SUPERSEDES);
            match (NamedNodeRef::new(s), NamedNodeRef::new(&sup_s)) {
                (Ok(n), Ok(p)) => {
                    store.quads_for_pattern(Some(n.into()), Some(p), None, None).next().is_none()
                }
                _ => false,
            }
        })
        .count();

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns() -> NamespaceConfig {
        NamespaceConfig::default()
    }

    fn store_with(nq: &str) -> Store {
        let store = Store::new().unwrap();
        store
            .load_from_reader(oxigraph::io::RdfFormat::NQuads, nq.as_bytes())
            .unwrap();
        store
    }

    /// `a supersededBy b`, in graph `g`, as one N-Quads line.
    fn edge(u: &str, g: &str, a: &str, b: &str) -> String {
        format!("<{u}{a}> <{u}{PRED_SUPERSEDED_BY}> <{u}{b}> <{g}> .\n")
    }

    #[test]
    fn the_exclusion_filter_binds_a_variable_derived_from_its_subject() {
        let f = sparql_exclude_superseded(&ns(), "n");
        assert_eq!(f, "FILTER NOT EXISTS { ?n ops:supersededBy ?n_supersededBy }\n");
        assert!(
            !f.contains("GRAPH"),
            "the filter carries no GRAPH group of its own — the caller must place it \
             INSIDE the arm it filters, or it is inert (F16)"
        );
    }

    #[test]
    fn custom_namespace_is_honoured() {
        let ns = NamespaceConfig { prefix: "mybase".into(), uri: "http://example.com/base#".into() };
        assert!(sparql_exclude_superseded(&ns, "x").contains("mybase:supersededBy"));
        assert!(link_update(&ns, "g", "old", "new").contains("mybase:supersedes"));
    }

    #[test]
    fn link_update_writes_both_edges_and_the_status_in_one_statement() {
        let u = link_update(&ns(), "g", "old", "new");
        assert_eq!(
            u.matches("INSERT DATA").count(),
            1,
            "three statements can be interrupted between the second and the third: {u}"
        );
        assert!(u.contains("<new> ops:supersedes <old> ."), "{u}");
        assert!(u.contains("<old> ops:supersededBy <new> ."), "{u}");
        assert!(u.contains("<old> ops:status \"superseded\" ."), "{u}");
    }

    #[test]
    fn resolve_head_walks_a_chain_to_its_live_end() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let store = store_with(&format!("{}{}", edge(u, &g, "note/a", "note/b"), edge(u, &g, "note/b", "note/c")));

        assert_eq!(resolve_head(&store, &ns, &format!("{u}note/a")), format!("{u}note/c"));
        assert_eq!(resolve_head(&store, &ns, &format!("{u}note/b")), format!("{u}note/c"));
        assert_eq!(
            resolve_head(&store, &ns, &format!("{u}note/c")),
            format!("{u}note/c"),
            "the head resolves to itself"
        );
    }

    #[test]
    fn resolve_head_crosses_graphs() {
        // The edge pair lives in the NEW record's tier; a global record superseded by
        // a workspace one is found only if the walk ignores graph names.
        let ns = ns();
        let u = &ns.uri;
        let store = store_with(&edge(u, &format!("{u}graph/ws/chris"), "note/global", "note/local"));
        assert_eq!(resolve_head(&store, &ns, &format!("{u}note/global")), format!("{u}note/local"));
    }

    #[test]
    fn resolve_head_terminates_on_a_cycle_instead_of_hanging() {
        // `would_cycle` refuses to write this. It runs inside the prompt-submit hook,
        // so if one ever arrives by another route the walk must still stop.
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let store = store_with(&format!("{}{}", edge(u, &g, "note/a", "note/b"), edge(u, &g, "note/b", "note/a")));
        let head = resolve_head(&store, &ns, &format!("{u}note/a"));
        assert!(head == format!("{u}note/a") || head == format!("{u}note/b"), "{head}");
    }

    #[test]
    fn resolve_head_is_deterministic_when_a_record_has_two_successors() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let store = store_with(&format!("{}{}", edge(u, &g, "note/a", "note/z"), edge(u, &g, "note/a", "note/b")));
        // Lexicographically first, so two runs of the same store agree.
        assert_eq!(resolve_head(&store, &ns, &format!("{u}note/a")), format!("{u}note/b"));
    }

    #[test]
    fn would_cycle_catches_a_self_reference_and_a_closed_loop() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let a = format!("{u}note/a");
        let _b = format!("{u}note/b");
        let c = format!("{u}note/c");

        let store = store_with(&format!("{}{}", edge(u, &g, "note/a", "note/b"), edge(u, &g, "note/b", "note/c")));
        assert!(would_cycle(&store, &ns, &a, &a), "a record cannot supersede itself");
        // a -> b -> c. Superseding C **by A** is the cycle: the new edge is
        // `c supersededBy a`, and walking forward from A reaches c again.
        assert!(
            would_cycle(&store, &ns, &c, &a),
            "a is upstream of c, so making a the successor of c closes the loop"
        );
        // The other direction is a diamond, not a cycle: a would have two
        // successors, which is a defect the writer resolves deterministically
        // rather than one it must refuse.
        assert!(!would_cycle(&store, &ns, &a, &c));
        assert!(
            !would_cycle(&store, &ns, &c, &format!("{u}note/d")),
            "a fresh successor for the head is the normal case"
        );
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    fn ns() -> NamespaceConfig {
        NamespaceConfig::default()
    }

    fn store_with(nq: &str) -> Store {
        let store = Store::new().unwrap();
        store
            .load_from_reader(oxigraph::io::RdfFormat::NQuads, nq.as_bytes())
            .unwrap();
        store
    }

    fn edge(u: &str, g: &str, a: &str, b: &str) -> String {
        format!("<{u}{a}> <{u}{PRED_SUPERSEDED_BY}> <{u}{b}> <{g}> .\n")
    }

    fn status(u: &str, g: &str, a: &str) -> String {
        format!("<{u}{a}> <{u}status> \"{STATUS_SUPERSEDED}\" <{g}> .\n")
    }

    #[test]
    fn a_store_that_never_used_the_feature_is_silent() {
        // The whole point: `base doctor` on a pre-0.14.0 store must print exactly
        // what it printed before, so its output is byte-comparable across the upgrade.
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let nq = format!("<{u}note/a> <{u}noteText> \"plain\" <{g}> .\n");
        let a = audit(&store_with(&nq), &ns);
        assert!(a.is_silent(), "{a:?}");
        assert_eq!(a, Audit::default());
    }

    #[test]
    fn the_two_disagreements_are_counted_apart() {
        // Summing them would hide which happened: status-without-edge is a
        // pre-0.14.0 artefact, edge-without-status is a writer that half-ran.
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let nq = format!(
            "{}{}{}",
            status(u, &g, "note/orphan-status"),
            edge(u, &g, "note/bare-edge", "note/live"),
            status(u, &g, "note/both") + &edge(u, &g, "note/both", "note/live2"),
        );
        let a = audit(&store_with(&nq), &ns);
        assert_eq!(a.superseded, 2, "bare-edge and both carry the edge: {a:?}");
        assert_eq!(a.status_without_edge, 1, "{a:?}");
        assert_eq!(a.edge_without_status, 1, "{a:?}");
        assert!(!a.is_silent());
    }

    #[test]
    fn a_chain_of_three_is_not_long_and_a_chain_of_four_is() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");

        let three = format!(
            "{}{}{}",
            edge(u, &g, "note/a", "note/b"),
            edge(u, &g, "note/b", "note/c"),
            edge(u, &g, "note/c", "note/d"),
        );
        assert!(
            audit(&store_with(&three), &ns).long_chains.is_empty(),
            "three hops is exactly the limit, not past it"
        );

        let four = three + &edge(u, &g, "note/d", "note/e");
        let a = audit(&store_with(&four), &ns);
        assert_eq!(a.long_chains, vec![format!("{u}note/a")], "{a:?}");
    }

    #[test]
    fn a_cycle_is_reported_and_the_audit_still_terminates() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let nq = format!("{}{}", edge(u, &g, "note/a", "note/b"), edge(u, &g, "note/b", "note/a"));
        let a = audit(&store_with(&nq), &ns);
        assert_eq!(a.cycles.len(), 2, "both members see the loop: {a:?}");
        assert_eq!(a.superseded, 2);
    }

    #[test]
    fn a_correction_naming_nothing_is_counted_and_one_naming_something_is_not() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let nq = format!(
            "<{u}note/loose> <{u}noteType> \"correction\" <{g}> .\n\
             <{u}note/tied> <{u}noteType> \"correction\" <{g}> .\n\
             <{u}note/tied> <{u}{PRED_SUPERSEDES}> <{u}note/old> <{g}> .\n\
             <{u}note/plain> <{u}noteType> \"insight\" <{g}> .\n"
        );
        let a = audit(&store_with(&nq), &ns);
        assert_eq!(
            a.corrections_naming_nothing, 1,
            "only the correction that names nothing counts, and an insight never does: {a:?}"
        );
    }
}
