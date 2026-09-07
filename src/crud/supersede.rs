//! `base graph supersede <old> <new>` and the `--supersedes` flag on the three
//! knowledge writers: the CLI-facing half of [`crate::supersede`].
//!
//! Two entry points, deliberately unequal (G0 verdict, 2026-09-06):
//!
//! - the CRUD flags take a **slug** and resolve it against the store the write has
//!   already loaded, refusing anything that is not exactly one record;
//! - `graph supersede` takes free text and resolves it through `graph_tools`'
//!   label resolver, which needs a fully loaded `load_graph` and federates every
//!   AST map — affordable there because it is already a `graph` subcommand, and
//!   not affordable on a write path that today loads one store and runs one
//!   SPARQL.
//!
//! Every refusal happens BEFORE anything is inserted, and each refusal test
//! asserts the store is unchanged afterwards: a refusal that half-wrote is worse
//! than no refusal at all.

use std::path::Path;

use anyhow::{bail, Result};
use oxigraph::store::Store;

use crate::config::NamespaceConfig;
use crate::crud;
use crate::supersede;

/// Resolve a slug to exactly one record IRI in `store`.
///
/// Matches `<ns>{kind}/{slug}` under ANY kind — a decision, a rule and a note are
/// all supersedable and no kind list would stay current. Zero matches and two
/// matches are both errors, and the two-match error names both candidates so the
/// operator can retype an unambiguous one; guessing would silently correct the
/// wrong record.
pub fn resolve_slug(store: &Store, ns: &NamespaceConfig, input: &str) -> Result<String> {
    // Two candidate tails, not one. `crud::slugify` is for turning free text into a
    // slug; applied to a slug that already exists it can change it -- `decision log`
    // builds `{domain}.{decision}`, and re-slugifying that dotted form matches no
    // record. So the input is tried VERBATIM as well, which is the form every base
    // command prints and therefore the form an operator pastes back.
    let slug = crud::slugify(input);
    let tails = [format!("/{input}"), format!("/{slug}")];
    let mut hits: Vec<String> = store
        .iter()
        .filter_map(|q| q.ok())
        .filter_map(|q| match q.subject {
            oxigraph::model::Subject::NamedNode(n) => Some(n.into_string()),
            _ => None,
        })
        .filter(|s| s.starts_with(&ns.uri) && tails.iter().any(|t| s.ends_with(t)))
        .collect();
    hits.sort();
    hits.dedup();

    match hits.len() {
        1 => Ok(hits.remove(0)),
        0 => bail!("no record matches '{input}' (slug '{slug}') — nothing was written"),
        _ => bail!(
            "'{input}' (slug '{slug}') matches {} records and would be ambiguous — \
             name one of them exactly: {}. Nothing was written.",
            hits.len(),
            hits.join(", ")
        ),
    }
}

/// Write "`new_iri` supersedes `old_iri`" into `graph_iri`, refusing a self-reference
/// and any edge that would close a cycle.
///
/// The caller owns the store and the write, so this composes into a writer's own
/// single `INSERT DATA` (the `--supersedes` flags) as easily as it stands alone
/// (`graph supersede`). Returns the statement to apply; it writes nothing itself.
pub fn link_statement(
    store: &Store,
    ns: &NamespaceConfig,
    graph_iri: &str,
    old_iri: &str,
    new_iri: &str,
) -> Result<String> {
    if old_iri == new_iri {
        bail!("a record cannot supersede itself ({old_iri}) — nothing was written");
    }
    if supersede::would_cycle(store, ns, old_iri, new_iri) {
        bail!(
            "{new_iri} already descends from {old_iri}; superseding it would close a \
             cycle — nothing was written"
        );
    }
    Ok(supersede::link_update(ns, graph_iri, old_iri, new_iri))
}

/// `base graph supersede <old> <new>` — the standalone primitive.
///
/// The edge pair lands in the tier of the NEW record (G0 verdict): a workspace
/// correction to a global note puts both edges in the workspace graph, where
/// [`supersede::resolve_head`] finds them on a merged read.
pub fn supersede(cwd: &Path, ns: &NamespaceConfig, old: &str, new: &str) -> Result<(String, String)> {
    let (store, trig_path) = crud::load_workspace_store(cwd)?;
    let old_iri = resolve_slug(&store, ns, old)?;
    let new_iri = resolve_slug(&store, ns, new)?;
    let graph = crud::workspace_graph_iri(ns, &crud::workspace_slug(cwd));

    let statement = link_statement(&store, ns, &graph, &old_iri, &new_iri)?;
    let full = format!("{}\n{}", crud::prefixes(ns), statement);
    crate::store::update_and_write(
        &store,
        &trig_path,
        &full,
        crate::store::Scope::Target,
        crate::store::Intent::Knowledge,
    )?;
    Ok((old_iri, new_iri))
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

    fn one_note(u: &str, g: &str, slug: &str) -> String {
        format!(
            "<{u}note/{slug}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{u}Note> <{g}> .\n"
        )
    }

    #[test]
    fn a_slug_matching_nothing_is_refused_by_name() {
        let ns = ns();
        let g = format!("{}graph/ws/t", ns.uri);
        let store = store_with(&one_note(&ns.uri, &g, "alpha"));
        let err = resolve_slug(&store, &ns, "missing").unwrap_err().to_string();
        assert!(err.contains("no record matches 'missing'"), "{err}");
        assert!(err.contains("nothing was written"), "{err}");
    }

    #[test]
    fn an_ambiguous_slug_is_refused_and_names_every_candidate() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        // Same slug under two kinds: a note and a decision both called `alpha`.
        let nq = format!(
            "{}<{u}decision/alpha> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{u}Decision> <{g}> .\n",
            one_note(u, &g, "alpha")
        );
        let err = resolve_slug(&store_with(&nq), &ns, "alpha").unwrap_err().to_string();
        assert!(err.contains("matches 2 records"), "{err}");
        assert!(err.contains("note/alpha") && err.contains("decision/alpha"), "{err}");
    }

    #[test]
    fn a_slug_matching_one_record_of_any_kind_resolves() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let nq = format!(
            "<{u}decision/use-rust> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{u}Decision> <{g}> .\n"
        );
        assert_eq!(
            resolve_slug(&store_with(&nq), &ns, "Use Rust").unwrap(),
            format!("{u}decision/use-rust"),
            "the input is slugified, and no kind list gates the match"
        );
    }

    #[test]
    fn a_self_reference_is_refused_before_any_statement_is_built() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let store = store_with(&one_note(u, &g, "alpha"));
        let a = format!("{u}note/alpha");
        let before = store.len().unwrap();
        let err = link_statement(&store, &ns, &g, &a, &a).unwrap_err().to_string();
        assert!(err.contains("cannot supersede itself"), "{err}");
        assert_eq!(store.len().unwrap(), before, "a refusal must not write");
    }

    #[test]
    fn an_edge_that_would_close_a_cycle_is_refused() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        // a -> b -> c already exists; making A the successor of C closes the loop.
        let nq = format!(
            "<{u}note/a> <{u}supersededBy> <{u}note/b> <{g}> .\n\
             <{u}note/b> <{u}supersededBy> <{u}note/c> <{g}> .\n"
        );
        let store = store_with(&nq);
        let before = store.len().unwrap();
        // old = c, new = a: the new edge is `c supersededBy a`, and a already
        // reaches c through b. Passing these the other way round is a diamond.
        let err = link_statement(&store, &ns, &g, &format!("{u}note/c"), &format!("{u}note/a"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("would close a"), "{err}");
        assert_eq!(store.len().unwrap(), before, "a refusal must not write");
    }

    #[test]
    fn a_fresh_successor_for_the_head_is_the_normal_case() {
        let ns = ns();
        let u = &ns.uri;
        let g = format!("{u}graph/ws/t");
        let nq = format!("<{u}note/a> <{u}supersededBy> <{u}note/b> <{g}> .\n");
        let s = link_statement(&store_with(&nq), &ns, &g, &format!("{u}note/b"), &format!("{u}note/c"))
            .unwrap();
        assert!(s.contains("<http://ops-sys.local/ontology#note/c> ops:supersedes"), "{s}");
        assert!(s.contains("ops:status \"superseded\""), "{s}");
    }
}
