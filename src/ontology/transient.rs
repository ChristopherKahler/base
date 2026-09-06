//! Record kinds that are session traffic, not knowledge — excluded from every
//! read surface by ONE list.
//!
//! Chris's ruling 4 (2026-09-06): relay pings are *"excluded from analyze AND from
//! every other read surface by ONE shared rule … make sure the tool as a whole is
//! built to exclude pings in the same way"*. Not per-command. 5,012 of the global
//! tier's 5,248 typed records are relay `Ping`s (hawk, 2026-09-05); left in, they
//! are the tier's defining mass and every community, count and neighbourhood is
//! about them rather than about the operator's knowledge.
//!
//! Adding a kind to [`TRANSIENT_KINDS`] excludes it from all four readers at once:
//!
//! | reader | what it serves |
//! |---|---|
//! | `graph_query::load_graph` | every `base graph` command (query/analyze/get-node/neighbors/path) |
//! | `domain::query::query_domain_from_graph` | prompt-time domain injection |
//! | `crud::note` recall reads | `base recall`, `base learn --list`, hook memory injection |
//! | `dashboard::api` nodes/edges | the Command Center graph view |
//!
//! No command carries its own filter. `tests/transient_kinds_test.rs` asserts each
//! reader honours the list with a fixture Ping.

use crate::config::NamespaceConfig;

/// One transient record kind, named both ways a reader can see it.
///
/// A relay ping is written as `<ns>ping/<slug>` typed `<ns>Ping`
/// (`relay/task_inbox.rs`). A SPARQL reader that has the subject bound filters on
/// the RDF class; an in-memory reader holding only an IRI filters on the kind
/// segment. Both discriminators are already in the data — neither is inferred.
pub struct TransientKind {
    /// Local name of the RDF class, e.g. `Ping` in `ops:Ping`.
    pub class: &'static str,
    /// IRI kind segment, e.g. `ping` in `…#ping/lark-1757`.
    pub iri_kind: &'static str,
}

/// The list. One place; four readers.
pub const TRANSIENT_KINDS: [TransientKind; 1] = [TransientKind { class: "Ping", iri_kind: "ping" }];

/// SPARQL that removes every transient kind for an already-bound subject variable.
///
/// `var` is the variable name without its `?`. Returns `""` when the list is empty,
/// so a caller can interpolate it unconditionally.
///
/// `FILTER NOT EXISTS` rather than a negated `?type` comparison because most reads
/// never bind the subject's class — requiring one would turn an OPTIONAL match into
/// a mandatory one and silently drop untyped records.
pub fn sparql_exclude(ns: &NamespaceConfig, var: &str) -> String {
    let p = &ns.prefix;
    TRANSIENT_KINDS
        .iter()
        .map(|k| format!("FILTER NOT EXISTS {{ ?{var} a {p}:{} }}\n", k.class))
        .collect()
}

/// Is this IRI a transient record? Matches the `{ns.uri}{kind}/` prefix that
/// `crud::build_iri` writes, so it cannot be fooled by a slug containing "ping".
///
/// Accepts an IRI with or without angle brackets — `graph_query` carries them,
/// `dashboard` does not.
pub fn is_transient_iri(ns: &NamespaceConfig, iri: &str) -> bool {
    let bare = iri.strip_prefix('<').unwrap_or(iri);
    let bare = bare.strip_suffix('>').unwrap_or(bare);
    let Some(loc) = bare.strip_prefix(ns.uri.as_str()) else { return false };
    let Some((kind, slug)) = loc.split_once('/') else { return false };
    !slug.is_empty() && TRANSIENT_KINDS.iter().any(|k| k.iri_kind == kind)
}

/// Is this RDF class local name a transient kind? Takes the short form
/// (`Ping`) or a full IRI (`http://…#Ping`).
pub fn is_transient_class(class: &str) -> bool {
    let local = class
        .rsplit_once('#')
        .or_else(|| class.rsplit_once('/'))
        .or_else(|| class.rsplit_once(':'))
        .map(|(_, s)| s)
        .unwrap_or(class);
    TRANSIENT_KINDS.iter().any(|k| k.class == local)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns() -> NamespaceConfig {
        NamespaceConfig::default()
    }

    #[test]
    fn ping_iri_is_transient_note_iri_is_not() {
        let ns = ns();
        assert!(is_transient_iri(&ns, "http://ops-sys.local/ontology#ping/lark-booted"));
        assert!(is_transient_iri(&ns, "<http://ops-sys.local/ontology#ping/lark-booted>"));
        assert!(!is_transient_iri(&ns, "http://ops-sys.local/ontology#note/ping-latency-is-high"));
        assert!(!is_transient_iri(&ns, "http://ops-sys.local/ontology#domain/base"));
    }

    #[test]
    fn a_slug_containing_ping_is_not_transient() {
        // The discriminator is the kind segment, never a substring of the IRI.
        assert!(!is_transient_iri(&ns(), "http://ops-sys.local/ontology#document/ping-hub-notes"));
    }

    #[test]
    fn foreign_namespace_is_never_transient() {
        assert!(!is_transient_iri(&ns(), "http://example.com/other#ping/whatever"));
    }

    #[test]
    fn class_matches_short_and_full_form() {
        assert!(is_transient_class("Ping"));
        assert!(is_transient_class("http://ops-sys.local/ontology#Ping"));
        assert!(is_transient_class("ops:Ping"));
        assert!(!is_transient_class("Note"));
        assert!(!is_transient_class("http://ops-sys.local/ontology#Note"));
    }

    #[test]
    fn sparql_exclude_names_every_kind_in_the_list() {
        let f = sparql_exclude(&ns(), "n");
        for k in TRANSIENT_KINDS.iter() {
            assert!(
                f.contains(&format!("?n a ops:{}", k.class)),
                "filter must exclude {} — got {f}",
                k.class
            );
        }
        assert!(f.starts_with("FILTER NOT EXISTS"));
    }

    #[test]
    fn custom_namespace_is_honoured_both_ways() {
        let ns = NamespaceConfig { prefix: "mybase".into(), uri: "http://example.com/base#".into() };
        assert!(is_transient_iri(&ns, "http://example.com/base#ping/x"));
        assert!(!is_transient_iri(&ns, "http://ops-sys.local/ontology#ping/x"));
        assert!(sparql_exclude(&ns, "s").contains("?s a mybase:Ping"));
    }
}
