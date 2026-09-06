//! Prompt-time traversal: resolve the things a prompt NAMES to graph nodes, walk
//! outward a bounded distance, and serve what is attached.
//!
//! What injection served before this was a keyword match against hand-configured
//! trigger lists: everything hung off a *domain* that a *keyword* selected. Naming
//! a project did not surface its client, its people or its files, and a topic with
//! no keywords configured could never fire at all.
//!
//! This is the record-level layer under that domain-level one. Keyword triggers
//! still choose domains; this resolves named things and walks from them, so a
//! domain with no keywords is reachable through anything filed in it that the user
//! names.
//!
//! Deliberately not fuzzy and deliberately not a model call. It runs on every
//! prompt, so it is hash lookups over maps the hook already built.

use std::collections::{HashMap, HashSet};

use crate::config::NamespaceConfig;
use crate::graph_query::{iri_kind, GraphMaps, Node};

/// A name the prompt used, and what it resolved to.
pub struct Resolved {
    pub name: String,
    pub id: String,
    pub kind: String,
    pub hops: usize,
    /// How many records of the chosen kind answered to the name. >1 is a data
    /// smell worth showing in devmode; the choice is still deterministic.
    pub ties: usize,
}

/// One record the walk found, with the relation that led to it.
pub struct Record {
    pub kind: String,
    pub label: String,
    pub relation: String,
    pub id: String,
}

/// Kind priority for ranking. Lower sorts first.
fn kind_priority(kind: &str) -> usize {
    match kind {
        "decision" => 0,
        "rule" => 1,
        "task" => 2,
        "note" => 3,
        "doc" | "document" => 4,
        _ => 5,
    }
}

/// A hub gets two hops; everything else gets one. A project or a domain is the
/// thing a person names when they mean "and all of its context"; a single
/// decision is not.
fn hops_for(kind: &str) -> usize {
    if matches!(kind, "project" | "domain") { 2 } else { 1 }
}

/// Candidate spans from a prompt, longest first, each consuming its span.
///
/// A bare lowercase single word never resolves on its own: `base` in the middle
/// of a sentence is a preposition-shaped noun, not a reference. Quoting it,
/// backticking it, capitalising it, giving it a path shape, or using two or more
/// words all read as naming something, and all resolve.
pub fn candidates(prompt: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let push = |c: String, out: &mut Vec<String>, seen: &mut HashSet<String>| {
        let c = c.trim().trim_matches(|ch: char| ch == ',' || ch == '.' || ch == ':' || ch == ';');
        if c.is_empty() || c.len() > 120 {
            return;
        }
        if seen.insert(c.to_lowercase()) {
            out.push(c.to_string());
        }
    };

    // Quoted and backticked spans are explicit: whatever is inside was named.
    for (open, close) in [('`', '`'), ('"', '"'), ('\'', '\'')] {
        let mut rest = prompt;
        while let Some(a) = rest.find(open) {
            let after = &rest[a + open.len_utf8()..];
            let Some(b) = after.find(close) else { break };
            push(after[..b].to_string(), &mut out, &mut seen);
            rest = &after[b + close.len_utf8()..];
        }
    }

    // Path-shaped tokens.
    for tok in prompt.split_whitespace() {
        let t = tok.trim_matches(|c: char| ",.;:()[]".contains(c));
        if t.contains('/') || t.contains('\\') || (t.contains('.') && !t.ends_with('.')) {
            push(t.to_string(), &mut out, &mut seen);
        }
    }

    // Word n-grams, longest first, so `first client kit` is tried before its
    // parts and consumes them.
    let words: Vec<&str> = prompt
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    let mut consumed = vec![false; words.len()];
    for n in (1..=4).rev() {
        if words.len() < n {
            continue;
        }
        for i in 0..=words.len() - n {
            if consumed[i..i + n].iter().any(|c| *c) {
                continue;
            }
            let span = words[i..i + n].join(" ");
            // The single-word rule: only a capitalised one stands alone here.
            // Quoted / backticked / path-shaped ones were already taken above.
            if n == 1 && span.chars().next().is_some_and(char::is_lowercase) {
                continue;
            }
            for k in i..i + n {
                consumed[k] = true;
            }
            push(span, &mut out, &mut seen);
        }
    }

    out
}

/// Resolve the prompt's named things and walk out from each.
///
/// Returns the resolutions in prompt order and the records found, already
/// deduped against `already_served` (the record IRIs the domain block emitted
/// this prompt) and against each other.
pub fn walk(
    maps: &GraphMaps,
    ns: &NamespaceConfig,
    prompt: &str,
    already_served: &HashSet<String>,
    is_transient: &dyn Fn(&str) -> bool,
    is_superseded: &dyn Fn(&str) -> bool,
) -> Vec<(Resolved, Vec<Record>)> {
    let (nodes, adj) = maps;
    let mut out = Vec::new();
    let mut emitted: HashSet<String> = already_served.clone();

    for name in candidates(prompt) {
        let Some((id, ties)) = crate::graph_tools::resolve_strict(nodes, adj, ns, &name) else {
            continue;
        };
        if !emitted.insert(id.clone()) {
            continue;
        }
        let kind = iri_kind(&id).unwrap_or("record").to_string();
        let hops = hops_for(&kind);

        let mut found: Vec<Record> = Vec::new();
        let mut visited: HashSet<String> = HashSet::from([id.clone()]);
        let mut frontier = vec![id.clone()];
        for _ in 0..hops {
            let mut next = Vec::new();
            for n in &frontier {
                for (nb, rel) in adj.get(n).map(|v| v.as_slice()).unwrap_or(&[]) {
                    if !visited.insert(nb.clone()) {
                        continue;
                    }
                    next.push(nb.clone());
                    if is_transient(nb) || is_superseded(nb) || emitted.contains(nb) {
                        continue;
                    }
                    emitted.insert(nb.clone());
                    found.push(Record {
                        kind: iri_kind(nb).unwrap_or("record").to_string(),
                        label: label_of(nodes, nb),
                        relation: rel.clone(),
                        id: nb.clone(),
                    });
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }

        found.sort_by(|a, b| {
            kind_priority(&a.kind)
                .cmp(&kind_priority(&b.kind))
                .then_with(|| a.label.cmp(&b.label))
                .then_with(|| a.id.cmp(&b.id))
        });

        out.push((
            Resolved { name, id, kind, hops, ties },
            found,
        ));
    }
    out
}

fn label_of(nodes: &HashMap<String, Node>, id: &str) -> String {
    nodes.get(id).map(|n| n.label.clone()).unwrap_or_else(|| id.to_string())
}

/// Render the walk, honouring a byte budget. Returns the block and how many
/// records the budget dropped, so devmode can say what was cut rather than
/// letting it vanish.
pub fn render(walked: &[(Resolved, Vec<Record>)], budget: usize) -> (String, usize) {
    let mut out = String::new();
    let mut dropped = 0usize;

    for (r, records) in walked {
        if records.is_empty() {
            continue;
        }
        let header = format!(
            "<base-context name=\"{}\" kind=\"{}\" hops=\"{}\">\n",
            r.name, r.kind, r.hops
        );
        let mut block = header;
        let mut wrote = 0usize;
        for rec in records {
            let line = format!("  {:<9} {} — {}\n", rec.kind, rec.label, rec.relation);
            if out.len() + block.len() + line.len() + 18 > budget {
                dropped += 1;
                continue;
            }
            block.push_str(&line);
            wrote += 1;
        }
        if wrote == 0 {
            continue;
        }
        block.push_str("</base-context>\n");
        out.push_str(&block);
    }
    (out, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(p: &str) -> Vec<String> {
        candidates(p)
    }

    #[test]
    fn a_bare_lowercase_word_never_stands_alone() {
        assert!(!names("what did we decide about base").contains(&"base".to_string()));
    }

    #[test]
    fn quoting_backticking_or_capitalising_makes_it_a_name() {
        assert!(names("what about `base`").contains(&"base".to_string()));
        assert!(names("what about \"base\"").contains(&"base".to_string()));
        assert!(names("what about Base").contains(&"Base".to_string()));
    }

    #[test]
    fn two_words_stand_alone_without_ceremony() {
        assert!(names("how is first client going").contains(&"first client".to_string()));
    }

    #[test]
    fn the_longest_span_consumes_its_parts() {
        let n = names("status of First Client Kit please");
        assert!(n.contains(&"First Client Kit please".to_string()) || n.iter().any(|s| s.contains("First Client Kit")));
        // "First" alone must not also appear as its own candidate.
        assert!(!n.contains(&"First".to_string()));
    }

    #[test]
    fn a_path_shaped_token_is_a_name() {
        assert!(names("look at src/graph_query.rs").contains(&"src/graph_query.rs".to_string()));
    }

    #[test]
    fn hubs_get_two_hops_and_records_get_one() {
        assert_eq!(hops_for("project"), 2);
        assert_eq!(hops_for("domain"), 2);
        assert_eq!(hops_for("decision"), 1);
    }

    #[test]
    fn decisions_outrank_documents() {
        assert!(kind_priority("decision") < kind_priority("doc"));
        assert!(kind_priority("rule") < kind_priority("note"));
    }

    #[test]
    fn the_budget_drops_records_and_says_how_many() {
        let walked = vec![(
            Resolved {
                name: "p".into(),
                id: "<x/p>".into(),
                kind: "project".into(),
                hops: 2,
                ties: 1,
            },
            (0..50)
                .map(|i| Record {
                    kind: "decision".into(),
                    label: format!("decision number {i}"),
                    relation: "belongsTo".into(),
                    id: format!("<x/d{i}>"),
                })
                .collect(),
        )];
        let (block, dropped) = render(&walked, 300);
        assert!(block.len() <= 300, "budget blown: {}", block.len());
        assert!(dropped > 0, "nothing reported as dropped");
    }

    #[test]
    fn an_empty_walk_renders_nothing() {
        assert_eq!(render(&[], 1000).0, "");
    }
}
