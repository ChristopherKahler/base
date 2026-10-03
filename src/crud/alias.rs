//! Old names after `base project rename` (BO-24, R4): where an old project or domain name points now, and the one
//! line a command prints when it reads an old name as the new one.
//!
//! Two stores hold an alias, written together by the rename: the graph (`<project/new> ops:alias "old"`, and the
//! same on the domain) and `domains.toml` (the domain's `aliases`). Slug lookups read the graph, because they have
//! already loaded it; `--domain` reads `domains.toml` (`domain::canonical_name`), because a store load on every
//! `--domain` would cost each of those commands a second parse of the graph.

use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::NamespaceConfig;
use crate::crud;

/// Printed on stderr whenever a command reads an old name as the new one, so stdout (and `--json`) stays the answer.
pub fn notice(old: &str, new: &str) {
    eprintln!("{old} is now {new}");
}

/// The slug an old project or domain name goes by now: the record carrying `ops:alias "<old>"` in `store`. A
/// project and its domain share the name, so either answers.
pub fn renamed_to(store: &Store, ns: &NamespaceConfig, old: &str) -> Option<String> {
    let p = &ns.prefix;
    let q = format!(
        "{}\nSELECT ?x WHERE {{ GRAPH ?g {{ ?x {p}:alias \"{}\" }} }}",
        crud::prefixes(ns),
        crud::escape_sparql_literal(old)
    );
    let Ok(QueryResults::Solutions(rows)) = crate::store::query(store, &q) else {
        return None;
    };
    let kinds = [crud::build_iri(ns, "project", ""), crud::build_iri(ns, "domain", "")];
    rows.filter_map(|r| r.ok())
        .filter_map(|row| match row.get("x")? {
            oxigraph::model::Term::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        })
        .find_map(|iri| kinds.iter().find_map(|k| iri.strip_prefix(k.as_str()).map(String::from)))
}

/// `input` with an old name in front read as the new one, as `(old, new, rewritten)`: `vintrix` becomes `vintryx`,
/// and `vintrix.some-decision` becomes `vintryx.some-decision`, the `{name}.{rest}` key decisions, tasks and
/// milestones carry. `None` when the part before the first dot is no old name.
pub fn rewrite(store: &Store, ns: &NamespaceConfig, input: &str) -> Option<(String, String, String)> {
    let (head, rest) = match input.split_once('.') {
        Some((h, r)) => (h, Some(r)),
        None => (input, None),
    };
    let old = crud::slugify(head);
    let new = renamed_to(store, ns, head).or_else(|| renamed_to(store, ns, &old))?;
    let rewritten = match rest {
        Some(r) => format!("{new}.{r}"),
        None => new.clone(),
    };
    Some((old, new, rewritten))
}
