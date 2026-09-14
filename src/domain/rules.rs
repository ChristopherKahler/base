//! The rules of a domain, read once, as a list.
//!
//! Before this module the two serving surfaces each had their own copy of the same
//! SPARQL and their own idea of what they had served, and the copies had drifted:
//!
//! - `src/hook/user_prompt_submit.rs` hashed the whole assembled domain block, sorted,
//!   and stored it under the domain name. One changed line re-served every rule in the
//!   domain.
//! - `src/hook/pre_tool_use.rs` computed its key from `domain_def.rendered_rules()`,
//!   which renders the **TOML**, and then served `query_rules_from_graph`, which reads
//!   the **graph**. The key and the payload came from different sources, so a rule
//!   edited in the graph re-served under an unchanged hash and a TOML edit re-injected
//!   text that had not changed. Found by `petrel`, verified by `auk` at `0ace1ba`.
//! - The pre-tool copy also sorted with `ORDER BY ?pri`, a string sort, so rule 10 came
//!   before rule 2 and the two surfaces disagreed about order on any domain with more
//!   than ten rules.
//!
//! One reader ends all three by construction: what is hashed IS what is rendered,
//! because both come from the same `Vec<ServedRule>`.
//!
//! ## Identity is the text, not the IRI
//!
//! A rule that came from `domains.toml` is stored at `rule/{domain}/{i}`, where `i` is
//! its position in the file (`domain/sync.rs`). Reorder the file and every IRI below
//! the moved line points at a different rule. The sync collector also deletes and
//! re-inserts every such rule on every sync. So the IRI is not a stable name for a
//! rule, and neither dedup nor a matcher can be keyed on it.
//!
//! [`rule_id`] hashes the domain and the rule's normalised text instead. It survives a
//! reorder, it survives the sync rewrite, and the same rule declared in two tiers gets
//! one id. An EDITED rule gets a different id, which is the behaviour F8 asks for: a
//! rule whose text changes is shown again.
//!
//! Rationale is deliberately not part of the id. A rationale edit must re-show the rule
//! but must not orphan its matchers, so identity is the text and [`ServedRule::content_hash`]
//! separately covers the rendered `text — because rationale` string.

use std::hash::{Hash, Hasher};

use oxigraph::model::TermRef;
use oxigraph::store::Store;

use crate::config::BaseConfig;
use crate::crud;
use crate::domain::{self, DomainDef};

/// One rule as every serving surface wants it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedRule {
    /// Stable identity: a hash of the domain and the rule's normalised text.
    pub id: String,
    /// The domain that shelves it.
    pub domain: String,
    /// The instruction, without rationale.
    pub text: String,
    /// The reason, when the rule carries one.
    pub rationale: Option<String>,
    /// `text — because rationale`, exactly as a reader receives it.
    pub rendered: String,
    /// A hash of `rendered`. A text or rationale edit changes it, which is what
    /// makes an edited rule show again (F8).
    pub content_hash: u64,
    /// The rule's IRI in `<full-iri>` form, when it came from the graph. The
    /// prompt-time walk dedups against these, so a record cannot arrive twice
    /// under two headings. `None` for a rule read from `domains.toml`, which has
    /// no IRI until the next sync.
    pub iri: Option<String>,
}

/// Collapse whitespace so that a reflow of a rule in `domains.toml` is the same rule.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn hash64<T: Hash>(v: T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// A rule's stable identity. See the module docs for why it is not the IRI.
pub fn rule_id(domain: &str, text: &str) -> String {
    format!("{:016x}", hash64((crud::slugify(domain), normalize(text))))
}

/// A hash of the rendered rule, for "has this exact text been served already".
pub fn content_hash(rendered: &str) -> u64 {
    hash64(rendered)
}

fn build(domain: &str, text: String, rationale: Option<String>, iri: Option<String>) -> ServedRule {
    let rendered = domain::render_rule(&text, rationale.as_deref());
    ServedRule {
        id: rule_id(domain, &text),
        domain: domain.to_string(),
        content_hash: content_hash(&rendered),
        rendered,
        text,
        rationale,
        iri,
    }
}

/// Every live rule of `domain_def`, in priority order.
///
/// The graph is the live render path; the `domains.toml` copy is the fallback for a
/// store that has not been synced, which is the same precedence both hooks used
/// before this module existed.
///
/// Superseded rules are already gone. That filter belongs here because both callers
/// are SERVING surfaces: serving a rule a later rule corrected hands the reader both
/// halves of a contradiction with nothing to tell them apart. It does NOT belong in
/// storage, in `base graph supersede`, or in an explicit query command, and
/// `--include-superseded` is untouched — base keeps superseded records on purpose,
/// because the superseded record is the drift evidence (`auk`, 2026-09-14).
///
/// Rules with no usable text are also gone (F13). Real examples from the operator's
/// store: `document a1 — references`, `document a2 — references`.
pub fn rules_for_domain(
    store: Option<&Store>,
    config: &BaseConfig,
    domain_def: &DomainDef,
) -> Vec<ServedRule> {
    let from_graph = store
        .map(|s| from_graph(s, config, domain_def))
        .unwrap_or_default();
    if from_graph.is_empty() {
        from_toml(domain_def)
    } else {
        from_graph
    }
}

fn from_toml(domain_def: &DomainDef) -> Vec<ServedRule> {
    domain_def
        .rules
        .iter()
        .filter(|r| !r.text().trim().is_empty())
        .map(|r| {
            build(
                &domain_def.name,
                r.text().to_string(),
                r.rationale().map(String::from),
                None,
            )
        })
        .collect()
}

fn from_graph(store: &Store, config: &BaseConfig, domain_def: &DomainDef) -> Vec<ServedRule> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let domain_slug = crud::slugify(&domain_def.name);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);
    let pfx = crud::prefixes(ns);

    // The superseded filter goes INSIDE the `GRAPH ?g` group, beside the pattern it
    // constrains. A triple pattern outside every GRAPH group is matched against the
    // default graph, where base keeps nothing, so `NOT EXISTS` is always true and the
    // filter excludes nothing while reading like a working one. That shipped once, as
    // F16 in `crud/note.rs` on 2026-09-06.
    let no_superseded = crate::supersede::sparql_exclude_superseded(ns, "rule");

    // `xsd:integer(?pri)`, never a bare `?pri`: a string sort compares "10" against
    // "2" and puts the eleventh rule second (#29).
    let sparql = format!(
        "{pfx}\n\
         SELECT ?rule ?text ?rationale WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:ruleText ?text .\n\
             OPTIONAL {{ ?rule {p}:priority ?pri }}\n\
             OPTIONAL {{ ?rule {p}:rationale ?rationale }}\n\
             {no_superseded}\
           }}\n\
         }}\n\
         ORDER BY xsd:integer(?pri)"
    );

    let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) =
        crate::store::query(store, &sparql)
    else {
        return Vec::new();
    };

    let mut out: Vec<ServedRule> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in solutions.filter_map(|r| r.ok()) {
        let Some(TermRef::Literal(lit)) = row.get("text").map(Into::into) else {
            continue;
        };
        let text = lit.value().to_string();
        if text.trim().is_empty() {
            continue;
        }
        let rationale = row.get("rationale").and_then(|t| match t.into() {
            TermRef::Literal(l) => {
                let v = l.value().to_string();
                (!v.is_empty()).then_some(v)
            }
            _ => None,
        });
        let iri = row.get("rule").and_then(|t| match t.into() {
            TermRef::NamedNode(n) => Some(format!("<{}>", n.as_str())),
            _ => None,
        });
        let rule = build(&domain_def.name, text, rationale, iri);

        // One rule, one line, whatever the graph topology. The SPARQL above matches
        // inside an UNBOUND `GRAPH ?g` and the store is a MERGE of both tiers, so a
        // domain declared once in the global `domains.toml` is synced into both
        // tiers' graphs and the same rule is a distinct quad in each — rendering
        // every rule twice. Deduping on the rendered text rather than with SPARQL
        // DISTINCT is deliberate: DISTINCT would have to project `?pri` to keep
        // `ORDER BY` legal, and differing priorities across tiers would defeat it.
        if seen.insert(rule.rendered.clone()) {
            out.push(rule);
        }
    }
    out
}
