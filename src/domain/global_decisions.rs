//! Global decisions: the decisions of an always-on domain, and when each one is served (BO-03, F5).
//!
//! WHY. An always-on domain matches every prompt, so before BO-03 every decision filed under it rode every
//! prompt's `[GLOBAL CONTEXT]` block whatever the prompt was about. On 2026-10-01 a prompt about the prompt
//! hook being cut off received five decisions about Claude Code profile mirroring, a second account, security
//! practice, the auto-memory archive and grazer, and none of them related to it.
//!
//! THE RULE (locked under D12). A global decision is served on a prompt only when the prompt contains one of
//! that decision's keywords (`base decision update <slug> --keywords "a, b"`, stored as `ops:decisionKeyword`).
//! A global decision with no keywords is served at session start only, never on a prompt. Global RULES are
//! not touched: `[DOMAIN: GLOBAL]` is unchanged.
//!
//! "Global" here is "linked by `hasDecision` from a domain whose mode is `always`". The shipped install has one
//! such domain, `GLOBAL`; a machine with another always-on domain has the same problem with it, so the rule
//! follows the mode and not the name. The prompt hook already ranks every always-on domain as global (F2).
//!
//! HOW THE PROMPT SERVES THEM. An always-on domain's CONTEXT block no longer lists its decisions at all: it
//! would change with every prompt's keywords, and the block is deduped by its whole text, so each change would
//! send the rest of it again. The decisions a prompt names by keyword go in their own block instead
//! ([`GlobalDecisions::prompt_block`]), each one once per session, and again after a DEPLETED or CRITICAL
//! force-refresh, as a domain's CONTEXT is. A decision that is ALSO filed under a configured domain that is not
//! always-on (`elsewhere`) is that domain's as well: its CONTEXT keeps it when it matches, and the walk treats it
//! as any record. A `hasDecision` edge from anything else (a plan, a domain no longer configured) does not count.
//!
//! One place answers "is this record a global decision, and does this text name it", for every surface that
//! serves decisions on a prompt: the domain neighbourhood (`domain::query`), the decisions block, the
//! prompt-time walk (`hook::walk`), `base context`, which previews what a prompt with that text receives, and
//! session start.

use std::collections::HashMap;

use oxigraph::model::TermRef;

use crate::config::BaseConfig;
use crate::crud;
use crate::domain::DomainDef;

/// The predicate a decision's keywords are stored under, as a local name under the namespace prefix.
pub const PRED_KEYWORD: &str = "decisionKeyword";

/// One global decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalDecision {
    /// `<iri>`, the key the walk and the neighbourhood use.
    pub id: String,
    /// The slug `base decision update` addresses: `{domain}.{decision}`.
    pub slug: String,
    pub name: String,
    /// The always-on domain it is filed under, as `domains.toml` names it; the first by name when there are two,
    /// so the heading it is listed under is the same on every run.
    pub domain: String,
    /// Sorted, lowercased, deduplicated.
    pub keywords: Vec<String>,
    /// Also filed under a configured domain that is not always-on.
    pub elsewhere: bool,
}

impl GlobalDecision {
    /// The record [`GlobalDecisions::prompt_block`] claims when it prints this decision: once per session, and
    /// again if its text changes.
    pub fn claim_key(&self) -> (String, u64) {
        (format!("global-decision:{}", self.id), crate::domain::session::rules_hash(std::slice::from_ref(&self.name)))
    }
}

/// Every live global decision in a store, by id. Superseded and transient records are not in it.
#[derive(Debug, Default, Clone)]
pub struct GlobalDecisions {
    by_id: HashMap<String, GlobalDecision>,
}

/// True when `text` contains one of `keywords` as whole words, case ignored: the same test a domain's
/// `prompt_keywords` meet (`matcher::contains_word`), so `CLAUDE_CONFIG_DIR` and `second account` match as
/// written and `port` does not match `support`.
pub fn keyword_hit(keywords: &[String], text: &str) -> bool {
    hit_lower(keywords, &text.to_lowercase())
}

/// [`keyword_hit`] on a text already lowercased, so a caller testing many decisions lowercases it once.
fn hit_lower(keywords: &[String], lower: &str) -> bool {
    keywords
        .iter()
        .map(|k| k.trim().to_lowercase())
        .any(|k| crate::domain::matcher::contains_word(lower, &k))
}

/// Split `--keywords "a, b, c"` into the stored list: trimmed, blanks dropped, case-insensitive duplicates
/// dropped, in the order given. An empty or blank argument is an empty list, which clears the keywords.
pub fn parse_keywords(arg: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in arg.split(',').map(str::trim).filter(|k| !k.is_empty()) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(k)) {
            out.push(k.to_string());
        }
    }
    out
}

impl GlobalDecisions {
    /// Read the live decisions of every always-on domain in `domains` from `store`. One query.
    pub fn load(store: &oxigraph::store::Store, config: &BaseConfig, domains: &[DomainDef]) -> Self {
        let ns = &config.namespace;
        let p = &ns.prefix;
        let always: HashMap<String, &str> = domains
            .iter()
            .filter(|d| d.is_always())
            .map(|d| (crud::build_iri(ns, "domain", &crud::slugify(&d.name)), d.name.as_str()))
            .collect();
        if always.is_empty() {
            return Self::default();
        }
        // The configured domains that are not always-on: only an edge from one of these makes a decision `elsewhere`.
        let others: std::collections::HashSet<String> = domains
            .iter()
            .filter(|d| !d.is_always())
            .map(|d| crud::build_iri(ns, "domain", &crud::slugify(&d.name)))
            .collect();
        let list = always.keys().map(|iri| format!("<{iri}>")).collect::<Vec<_>>().join(", ");
        // Both filters INSIDE the GRAPH group, beside the pattern they constrain (`supersede::sparql_exclude_superseded`
        // says why). The always-on edge, the name, the other domains' edges and the keywords are each read from any
        // graph: an edge need not be written where the decision's name is (`base decision update` writes keywords
        // where the type triple is, and a domain may be linked from another tier).
        let no_transient = crate::ontology::transient::sparql_exclude(ns, "d");
        let no_superseded = crate::supersede::sparql_exclude_superseded(ns, "d");
        let sparql = format!(
            "{pfx}\n\
             SELECT ?dom ?d ?name ?od ?kw WHERE {{\n\
               GRAPH ?ag {{ ?dom {p}:hasDecision ?d . FILTER(?dom IN ({list})) }}\n\
               GRAPH ?g {{\n\
                 ?d {p}:name ?name .\n\
                 {no_transient}{no_superseded}\
               }}\n\
               OPTIONAL {{ GRAPH ?og {{ ?od {p}:hasDecision ?d }} }}\n\
               OPTIONAL {{ GRAPH ?kg {{ ?d {p}:{PRED_KEYWORD} ?kw }} }}\n\
             }}",
            pfx = crud::prefixes(ns),
        );
        let Ok(oxigraph::sparql::QueryResults::Solutions(rows)) = crate::store::query(store, &sparql) else {
            return Self::default();
        };
        let mut by_id: HashMap<String, GlobalDecision> = HashMap::new();
        for row in rows.filter_map(|r| r.ok()) {
            let named = |k: &str| match row.get(k).map(|t| t.into()) {
                Some(TermRef::NamedNode(n)) => Some(n.as_str().to_string()),
                _ => None,
            };
            let literal = |k: &str| match row.get(k).map(|t| t.into()) {
                Some(TermRef::Literal(l)) => Some(l.value().to_string()),
                _ => None,
            };
            let (Some(dom), Some(d), Some(name)) = (named("dom"), named("d"), literal("name")) else {
                continue;
            };
            let id = format!("<{d}>");
            let entry = by_id.entry(id.clone()).or_insert_with(|| GlobalDecision {
                id,
                slug: crud::slug_of(&d),
                name,
                domain: String::new(),
                keywords: Vec::new(),
                elsewhere: false,
            });
            if let Some(n) = always.get(&dom)
                && (entry.domain.is_empty() || *n < entry.domain.as_str())
            {
                entry.domain = (*n).to_string();
            }
            if named("od").is_some_and(|od| others.contains(&od)) {
                entry.elsewhere = true;
            }
            if let Some(kw) = literal("kw").map(|k| k.trim().to_lowercase()).filter(|k| !k.is_empty())
                && !entry.keywords.contains(&kw)
            {
                entry.keywords.push(kw);
            }
        }
        for d in by_id.values_mut() {
            d.keywords.sort();
        }
        Self { by_id }
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// True when `id` (`<iri>`) is a global decision. An always-on domain's CONTEXT leaves every one of them out:
    /// [`GlobalDecisions::prompt_block`] serves them.
    pub fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    pub fn get(&self, id: &str) -> Option<&GlobalDecision> {
        self.by_id.get(id)
    }

    /// Every one of them, in no set order.
    pub fn all(&self) -> impl Iterator<Item = &GlobalDecision> {
        self.by_id.values()
    }

    /// The global decision `base decision update` addresses as `slug`.
    /// Decision `slug` as a proposal would leave it (BO-20, a shadow candidate's copy): `add` and `drop` made to its
    /// keywords (kept sorted, lowercased, once each), and its name changed when `name` is given. A slug this set does
    /// not hold is left alone.
    pub fn edit(&mut self, slug: &str, add: &[String], drop: &[String], name: Option<&str>) {
        let Some(d) = self.by_id.values_mut().find(|d| d.slug == slug) else { return };
        let mut keywords: Vec<String> =
            crate::domain::replay::edit_list(&d.keywords, add, drop).into_iter().map(|k| k.trim().to_lowercase()).collect();
        keywords.sort();
        keywords.dedup();
        d.keywords = keywords;
        if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
            d.name = n.to_string();
        }
    }

    pub fn by_slug(&self, slug: &str) -> Option<&GlobalDecision> {
        self.by_id.values().find(|d| d.slug == slug)
    }

    /// True when `id` is a global decision filed only under always-on domains and a prompt with this text does
    /// NOT receive it: it has no keywords, or none of them is in the text. False for every other record, so the
    /// walk can ask it about anything it is about to list.
    pub fn withheld_from(&self, id: &str, text: &str) -> bool {
        self.by_id.get(id).is_some_and(|d| !d.elsewhere && !keyword_hit(&d.keywords, text))
    }

    /// The global decisions a prompt with this text names by keyword, by domain and then name, leaving out those
    /// `skip` says were already served.
    pub fn matched<'a>(&'a self, text: &str, skip: &dyn Fn(&GlobalDecision) -> bool) -> Vec<&'a GlobalDecision> {
        let lower = text.to_lowercase();
        let mut out: Vec<&GlobalDecision> =
            self.by_id.values().filter(|d| hit_lower(&d.keywords, &lower) && !skip(d)).collect();
        out.sort_by(|a, b| a.domain.cmp(&b.domain).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// The prompt's decisions block: one `[<DOMAIN> CONTEXT · decisions matched by keyword]` group per domain, the
    /// line shape the CONTEXT block uses, and the decisions it lists. Empty when the prompt names none.
    pub fn prompt_block<'a>(&'a self, text: &str, skip: &dyn Fn(&GlobalDecision) -> bool) -> (String, Vec<&'a GlobalDecision>) {
        let list = self.matched(text, skip);
        (render(&list, "decisions matched by keyword"), list)
    }

    /// The global decisions with no keywords and no other domain, which only session start serves; by domain, then
    /// name.
    pub fn without_keywords(&self) -> Vec<&GlobalDecision> {
        let mut out: Vec<&GlobalDecision> =
            self.by_id.values().filter(|d| d.keywords.is_empty() && !d.elsewhere).collect();
        out.sort_by(|a, b| a.domain.cmp(&b.domain).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// Session start's block (F5's fallback): [`GlobalDecisions::without_keywords`], grouped as the prompt's block
    /// is. Empty when there are none.
    pub fn session_start_block(&self) -> (String, usize) {
        let list = self.without_keywords();
        (render(&list, "decisions with no keywords, shown at session start only"), list.len())
    }
}

fn render(list: &[&GlobalDecision], what: &str) -> String {
    let mut out = String::new();
    let mut domain: Option<&str> = None;
    for d in list {
        if domain != Some(d.domain.as_str()) {
            out.push_str(&format!("[{} CONTEXT · {what}]\n", d.domain));
            domain = Some(d.domain.as_str());
        }
        out.push_str(&format!("  - Decision: {}\n", d.name));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_match_as_whole_words_case_ignored() {
        let kws = vec!["CLAUDE_CONFIG_DIR".to_string(), "second account".to_string(), "port".to_string()];
        assert!(keyword_hit(&kws, "set up a Second Account on the laptop"));
        assert!(keyword_hit(&kws, "point claude_config_dir at it"));
        assert!(keyword_hit(&kws, "which port does it use?"));
        assert!(!keyword_hit(&kws, "support for the prompt hook"), "port inside support is not the word port");
        assert!(!keyword_hit(&[], "anything at all"), "no keywords match nothing");
    }

    #[test]
    fn keyword_argument_parses_to_a_clean_list() {
        assert_eq!(parse_keywords("grazer, browser , ,Claude in Chrome, GRAZER"), vec!["grazer", "browser", "Claude in Chrome"]);
        assert!(parse_keywords("  ").is_empty(), "a blank argument clears");
    }
}
