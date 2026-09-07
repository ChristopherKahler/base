//! The predicate a record's domain link is written under — one place, during and
//! after the expand-migrate-contract overlap.
//!
//! base grew two conventions for the same fact. `crud/project.rs`, `crud/entity.rs`
//! and `extract/paul_toml.rs` write `hasDomain`; `crud/note.rs` writes `relatedTo`,
//! the same predicate it uses for a note's project and entity links. That split is
//! two conventions, not a design decision (hawk C7, 2026-09-05: `crud/note.rs:28-40`
//! writes the identical predicate for all three link kinds).
//!
//! 0.14.0 makes [`CANONICAL`] the single predicate and keeps reading [`LEGACY`] for
//! the whole overlap window. N+1 stops writing the legacy triple, N+2 drops it. The
//! overlap is what makes rollback survivable: hawk C10 ran v0.12.0 (105 commits back)
//! against a store carrying every artefact this migration writes, in both directions,
//! and it read, analysed and WROTE without complaint — `store::load_graph` is a
//! syntactic N-Quads parse with no schema check, and every read binds only the
//! predicates it wants.
//!
//! ## Why the object IRI, not the predicate, is the discriminator
//!
//! `relatedTo` is generic. In the live workspace store it carries 465 note→domain
//! links, 532 document→entity links and 30 document→project links (hawk C12). Only
//! the object's IRI kind separates them, and it is already in the data — no guessing.
//! So: where the object is a KNOWN domain IRI, [`path`] is exact on its own; where
//! the object is a variable, [`binds_domain`] adds the `domain/` prefix filter.

use crate::config::NamespaceConfig;

/// The predicate 0.14.0 writes for "this record belongs to this domain".
pub const CANONICAL: &str = "hasDomain";

/// The predicate note→domain links were written under before 0.14.0. Read for the
/// whole overlap window; N+1 stops writing it, N+2 drops the triples.
pub const LEGACY: &str = "relatedTo";

/// `(ops:hasDomain|ops:relatedTo)` — a property path matching a domain link from
/// either era. Exact where the object is a known `domain/` IRI; use
/// [`binds_domain`] when the object is a variable.
pub fn path(ns: &NamespaceConfig) -> String {
    let p = &ns.prefix;
    format!("({p}:{CANONICAL}|{p}:{LEGACY})")
}

/// `FILTER EXISTS {{ ?{subj_var} (ops:hasDomain|ops:relatedTo) <domain_iri> }}` —
/// "this record belongs to that domain", in either era, **once**.
///
/// A bare `?n (a|b) <dom>` triple pattern is the obvious spelling and it is wrong
/// here: SPARQL evaluates an alternation as the union of its branches, so a record
/// carrying BOTH predicates — which is exactly what 0.14.0 writes — yields two
/// solutions and `base recall --domain` prints every note twice. Measured, not
/// assumed: `tests/domain_link_test.rs::a_note_carrying_both_predicates_recalls_exactly_once`
/// fails with the triple-pattern form. `EXISTS` is a boolean test, so the overlap
/// costs nothing.
pub fn links_to(ns: &NamespaceConfig, subj_var: &str, domain_iri: &str) -> String {
    let path = path(ns);
    format!("FILTER EXISTS {{ ?{subj_var} {path} <{domain_iri}> }}")
}

/// `http://…#domain/` — the prefix that tells a domain link from `relatedTo`'s two
/// other uses.
pub fn domain_iri_prefix(ns: &NamespaceConfig) -> String {
    format!("{}domain/", ns.uri)
}

/// A graph pattern binding `?{obj_var}` to `?{subj_var}`'s domain, whichever
/// predicate carried it. Ends in a newline; safe to interpolate into a group.
pub fn binds_domain(ns: &NamespaceConfig, subj_var: &str, obj_var: &str) -> String {
    let path = path(ns);
    let dom = domain_iri_prefix(ns);
    format!(
        "?{subj_var} {path} ?{obj_var} .\nFILTER(isIRI(?{obj_var}) && STRSTARTS(STR(?{obj_var}), \"{dom}\"))\n"
    )
}

/// Every record that already has a domain, in EITHER direction, as
/// `record IRI -> domain slug`.
///
/// Direction is the trap. `crud/decision.rs:47` writes `<domain> ops:hasDecision
/// <decision>` — the domain is the SUBJECT — and `domain sync` writes
/// `<domain> ops:hasRule <rule>` the same way. A subject-side-only test therefore
/// reads all 420 decisions and all 79 rules in Chris's store as orphans, and a
/// migration built on it would write 342 redundant links to the wrong domain.
/// hawk measured both directions (C-RE, "decisions are 65% covered, not 0%");
/// this reads both.
///
/// One pass over the store rather than a query per record: the migration asks this
/// of every covered record, and 4,000 SPARQL round trips inside a session-start
/// hook is not a budget. Ties resolve to the lexicographically first slug so two
/// runs of the same store agree.
pub fn domain_index(
    store: &oxigraph::store::Store,
    ns: &NamespaceConfig,
) -> std::collections::HashMap<String, String> {
    use oxigraph::model::Term;
    use oxigraph::sparql::QueryResults;

    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let dom = domain_iri_prefix(ns);
    let path = path(ns);
    let mut out: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut absorb = |q: &str| {
        let Ok(QueryResults::Solutions(sols)) = crate::store::query(store, q) else { return };
        for row in sols.filter_map(|r| r.ok()) {
            let (Some(Term::NamedNode(r)), Some(Term::NamedNode(d))) = (row.get("r"), row.get("d"))
            else {
                continue;
            };
            let Some(slug) = d.as_str().strip_prefix(dom.as_str()) else { continue };
            let key = r.as_str().to_string();
            match out.get(&key) {
                Some(existing) if existing.as_str() <= slug => {}
                _ => {
                    out.insert(key, slug.to_string());
                }
            }
        }
    };

    // Record -> domain: `note relatedTo domain/x`, `project hasDomain domain/x`.
    absorb(&format!(
        "{pfx}\nSELECT ?r ?d WHERE {{ GRAPH ?g {{ ?r {path} ?d .\n\
           FILTER(isIRI(?r) && isIRI(?d) && STRSTARTS(STR(?d), \"{dom}\")) }} }}"
    ));
    // Domain -> record: `domain hasDecision decision/x`, `domain hasRule rule/x`,
    // and any predicate a later release adds — matched by the SUBJECT's IRI kind,
    // not by an allowlist that would go stale.
    absorb(&format!(
        "{pfx}\nSELECT ?r ?d WHERE {{ GRAPH ?g {{ ?d ?p ?r .\n\
           FILTER(isIRI(?r) && isIRI(?d) && STRSTARTS(STR(?d), \"{dom}\") && ?p != {p}:hasDomain) }} }}"
    ));
    out
}

/// The domain IRI `iri` carries, under either predicate, in any graph of `store` — or
/// `None`. Two index lookups on a bound subject, no SPARQL.
///
/// This is how a record takes its parent's domain at creation: a Task its project's, a
/// Handoff the project's it names (kite F7b, vole's ruling of 2026-09-06 that "every
/// record carries a domain" must not wait for the next session start when the parent
/// already knows). The writer reads this off the store it has already loaded and inserts
/// the answer as a constant in its own `INSERT DATA`. The first version appended an
/// `INSERT ... WHERE` with a property path instead, and that cost 5 s per `base task add`
/// on a 60,000-quad store (kite F23): what was meant as a lookup was planned as a scan.
///
/// `None` when the parent has no domain. Inventing the catchall here would give two
/// places ownership of `unfiled`; the session-start delta pass (`migrate.rs`) files such
/// a record exactly as it files everything else that arrived without a link. The object
/// filter keeps `relatedTo`'s entity and project links out, as everywhere in this module.
/// Ties (a parent carrying two domains) resolve to the lexicographically first IRI, so
/// two writes of the same record agree.
pub fn domain_of(store: &oxigraph::store::Store, ns: &NamespaceConfig, iri: &str) -> Option<String> {
    use oxigraph::model::{NamedNodeRef, Term};
    let subject = NamedNodeRef::new(iri).ok()?;
    let dom = domain_iri_prefix(ns);
    let mut found: Vec<String> = Vec::new();
    for pred in [CANONICAL, LEGACY] {
        let pred_iri = format!("{}{pred}", ns.uri);
        let Ok(pred) = NamedNodeRef::new(&pred_iri) else { continue };
        for q in store
            .quads_for_pattern(Some(subject.into()), Some(pred), None, None)
            .filter_map(Result::ok)
        {
            if let Term::NamedNode(d) = q.object
                && d.as_str().starts_with(dom.as_str())
            {
                found.push(d.as_str().to_string());
            }
        }
    }
    found.sort();
    found.into_iter().next()
}

/// The `hasDomain` triple for `subject_iri` if its parent has a domain, as a line ready
/// for an `INSERT DATA` block, else an empty string. See [`domain_of`].
pub fn inherited_triple(
    store: &oxigraph::store::Store,
    ns: &NamespaceConfig,
    subject_iri: &str,
    parent_iri: &str,
) -> String {
    match domain_of(store, ns, parent_iri) {
        Some(d) => format!("             <{subject_iri}> {}:{CANONICAL} <{d}> .\n", ns.prefix),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns() -> NamespaceConfig {
        NamespaceConfig::default()
    }

    #[test]
    fn path_names_the_canonical_predicate_first() {
        // Order is documentation, not semantics: a reader of the generated SPARQL
        // should see which predicate 0.14.0 writes.
        assert_eq!(path(&ns()), "(ops:hasDomain|ops:relatedTo)");
    }

    #[test]
    fn binds_domain_filters_on_the_domain_iri_prefix() {
        let g = binds_domain(&ns(), "n", "d");
        assert!(g.contains("?n (ops:hasDomain|ops:relatedTo) ?d ."));
        assert!(
            g.contains("STRSTARTS(STR(?d), \"http://ops-sys.local/ontology#domain/\")"),
            "relatedTo also carries entity and project links — the object IRI is the \
             only discriminator, and it must be applied: {g}"
        );
    }

    #[test]
    fn links_to_is_a_boolean_test_not_a_triple_pattern() {
        // The overlap writes both predicates; a triple pattern would match twice.
        let f = links_to(&ns(), "n", "http://ops-sys.local/ontology#domain/base");
        assert!(f.starts_with("FILTER EXISTS {"), "{f}");
        assert!(f.contains("?n (ops:hasDomain|ops:relatedTo) <http://ops-sys.local/ontology#domain/base>"));
    }

    #[test]
    fn custom_namespace_is_honoured() {
        let ns = NamespaceConfig { prefix: "mybase".into(), uri: "http://example.com/base#".into() };
        assert_eq!(path(&ns), "(mybase:hasDomain|mybase:relatedTo)");
        assert_eq!(domain_iri_prefix(&ns), "http://example.com/base#domain/");
    }

    #[test]
    fn domain_of_reads_either_predicate_and_only_domain_objects() {
        use oxigraph::store::Store;
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let store = Store::new().unwrap();
        let nq = format!(
            "<{u}project/p1> <{u}hasDomain> <{u}domain/base> <{g}> .\n\
             <{u}note/n1> <{u}relatedTo> <{u}domain/basemode> <{g}> .\n\
             <{u}note/n2> <{u}relatedTo> <{u}entity/e1> <{g}> .\n\
             <{u}project/p2> <{u}hasDomain> <{u}domain/zeta> <{g}> .\n\
             <{u}project/p2> <{u}hasDomain> <{u}domain/alpha> <{g}> .\n"
        );
        store.load_from_reader(oxigraph::io::RdfFormat::NQuads, nq.as_bytes()).unwrap();

        assert_eq!(domain_of(&store, &ns, &format!("{u}project/p1")).as_deref(), Some(format!("{u}domain/base").as_str()));
        assert_eq!(domain_of(&store, &ns, &format!("{u}note/n1")).as_deref(), Some(format!("{u}domain/basemode").as_str()));
        assert_eq!(domain_of(&store, &ns, &format!("{u}note/n2")), None, "an entity link is not a domain");
        assert_eq!(domain_of(&store, &ns, &format!("{u}project/none")), None);
        assert_eq!(
            domain_of(&store, &ns, &format!("{u}project/p2")).as_deref(),
            Some(format!("{u}domain/alpha").as_str()),
            "two domains resolve to the lexicographically first, every time"
        );
        let line = inherited_triple(&store, &ns, &format!("{u}task/t1"), &format!("{u}project/p1"));
        assert_eq!(line, format!("             <{u}task/t1> ops:hasDomain <{u}domain/base> .\n"));
        assert_eq!(inherited_triple(&store, &ns, &format!("{u}task/t2"), &format!("{u}project/none")), "");
    }

    #[test]
    fn domain_index_reads_both_directions() {
        use oxigraph::store::Store;
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let store = Store::new().unwrap();
        let nq = format!(
            "<{u}domain/base> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{u}Domain> <{g}> .\n\
             <{u}note/n1> <{u}relatedTo> <{u}domain/base> <{g}> .\n\
             <{u}project/p1> <{u}hasDomain> <{u}domain/base> <{g}> .\n\
             <{u}domain/base> <{u}hasDecision> <{u}decision/d1> <{g}> .\n\
             <{u}domain/base> <{u}hasRule> <{u}rule/r1> <{g}> .\n\
             <{u}note/n2> <{u}relatedTo> <{u}entity/e1> <{g}> .\n"
        );
        store.load_from_reader(oxigraph::io::RdfFormat::NQuads, nq.as_bytes()).unwrap();

        let idx = domain_index(&store, &ns);
        assert_eq!(idx.get(&format!("{u}note/n1")).map(String::as_str), Some("base"));
        assert_eq!(idx.get(&format!("{u}project/p1")).map(String::as_str), Some("base"));
        assert_eq!(
            idx.get(&format!("{u}decision/d1")).map(String::as_str),
            Some("base"),
            "domain --hasDecision--> decision IS a domain link; missing it reads 420 \
             already-linked decisions as orphans"
        );
        assert_eq!(idx.get(&format!("{u}rule/r1")).map(String::as_str), Some("base"));
        assert!(
            !idx.contains_key(&format!("{u}note/n2")),
            "relatedTo to an entity is not a domain link"
        );
        assert!(!idx.contains_key(&format!("{u}entity/e1")));
    }
}
