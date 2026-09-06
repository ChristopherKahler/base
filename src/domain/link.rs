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

/// `FILTER NOT EXISTS { … }` — matches a record that carries no domain link in
/// either era. This is what the migration means by "still unmigrated": it is
/// computed from the store, never from a stamp (G0 verdict A2).
pub fn no_domain_link(ns: &NamespaceConfig, subj_var: &str) -> String {
    let inner = binds_domain(ns, subj_var, "__dom");
    format!("FILTER NOT EXISTS {{ {inner} }}\n")
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
    fn no_domain_link_wraps_the_binding_in_not_exists() {
        let f = no_domain_link(&ns(), "s");
        assert!(f.starts_with("FILTER NOT EXISTS {"));
        assert!(f.contains("?s (ops:hasDomain|ops:relatedTo) ?__dom"));
    }
}
