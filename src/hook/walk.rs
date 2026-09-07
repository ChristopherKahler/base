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
    /// ISO 8601, or empty. Empty sorts LAST rather than sorting as ancient.
    pub touched: String,
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

/// Candidate spans from a prompt: longest match wins, per start position.
///
/// `known` answers "is this a name the graph has?". Without it there is nothing
/// to be longest-match ABOUT, and the first version of this function proved it:
/// it partitioned the prompt into n-grams blindly, so `how is first client kit
/// going` came out as `[how is first client][kit going]` -- a span nobody named,
/// and the real name lost between two windows. Consuming a span has to be
/// conditional on it HITTING, or it is just a partition with extra steps.
///
/// So: at each start position try n = 4 down to 1; on a hit, take it and jump
/// past it; on no hit, advance one word and try again.
///
/// A bare lowercase single word never resolves on its own: `base` mid-sentence
/// is a word, not a reference. Quoting it, backticking it, capitalising it,
/// giving it a path shape, or using two or more words all read as naming
/// something.
pub fn candidates(prompt: &str, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let mut push = |c: &str, out: &mut Vec<String>, seen: &mut HashSet<String>| {
        let c = c.trim().trim_matches(|ch: char| ",.;:!?".contains(ch));
        if c.is_empty() || c.len() > 120 {
            return;
        }
        if seen.insert(c.to_lowercase()) {
            out.push(c.to_string());
        }
    };

    // Quoted and backticked spans are explicit: whatever is inside was named,
    // so they are taken whether or not the graph knows them yet.
    for (open, close) in [('`', '`'), ('"', '"')] {
        let mut rest = prompt;
        while let Some(a) = rest.find(open) {
            let after = &rest[a + open.len_utf8()..];
            let Some(b) = after.find(close) else { break };
            push(&after[..b].to_string(), &mut out, &mut seen);
            rest = &after[b + close.len_utf8()..];
        }
    }

    // Path-shaped tokens.
    for tok in prompt.split_whitespace() {
        let t = tok.trim_matches(|c: char| ",.;:()[]".contains(c));
        if t.contains('/') || t.contains('\\') {
            push(t, &mut out, &mut seen);
        }
    }

    // Longest match per start position, against the index.
    let words: Vec<&str> = prompt
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.'))
        .filter(|w| !w.is_empty())
        .collect();

    let mut i = 0usize;
    while i < words.len() {
        let mut took = 0usize;
        for n in (1..=4).rev() {
            if i + n > words.len() {
                continue;
            }
            let span = words[i..i + n].join(" ");
            // The single-word rule. Quoted / backticked / path-shaped words were
            // already taken above and do not come through here.
            if n == 1 && span.chars().next().is_some_and(char::is_lowercase) {
                continue;
            }
            if known(&span) {
                push(&span, &mut out, &mut seen);
                took = n;
                break;
            }
        }
        i += if took > 0 { took } else { 1 };
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

    // The index the longest-match runs against: every label the graph carries,
    // normalised once. Built from maps the hook already holds, so a candidate
    // check is a hash lookup, not a scan.
    let mut index: HashSet<String> = HashSet::new();
    for n in nodes.values() {
        index.insert(n.label.to_lowercase());
        // The slug too. `resolve_strict` tries the slug BEFORE the label, so an
        // index of labels alone leaves that half of the resolver unreachable:
        // `first-client-kit` in prose would fail this gate and never be offered
        // to the resolver that knows how to read it.
        index.insert(crate::crud::slugify(&n.label));
    }
    let known = |span: &str| {
        let s = span.to_lowercase();
        index.contains(&s) || index.contains(&crate::crud::slugify(span))
    };

    for name in candidates(prompt, &known) {
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
                        touched: nodes.get(nb).map(|n| n.touched.clone()).unwrap_or_default(),
                    });
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }

        // Kind, then recency, then id. A record with no timestamp sorts after
        // every record that has one -- "unknown" is not "old", and guessing
        // either way would put it somewhere it does not belong. The id tail
        // makes the order total, so the same graph renders the same block on
        // every run.
        found.sort_by(|a, b| {
            kind_priority(&a.kind)
                .cmp(&kind_priority(&b.kind))
                .then_with(|| a.touched.is_empty().cmp(&b.touched.is_empty()))
                .then_with(|| b.touched.cmp(&a.touched))
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

    fn mknode(label: &str, touched: &str) -> Node {
        Node {
            label: label.to_string(),
            ntype: String::new(),
            source: String::new(),
            summary: String::new(),
            touched: touched.to_string(),
        }
    }

    /// Insert a node under `id`. Returns nothing, because every caller wants the
    /// map populated rather than the pair.
    fn put(map: &mut HashMap<String, Node>, id: &str, label: &str) {
        map.insert(id.to_string(), mknode(label, ""));
    }

    fn put_dated(map: &mut HashMap<String, Node>, id: &str, label: &str, touched: &str) {
        map.insert(id.to_string(), mknode(label, touched));
    }

    fn ns() -> NamespaceConfig {
        NamespaceConfig::default()
    }

    fn no(_id: &str) -> bool {
        false
    }

    /// The index a test resolves against. Longest-match is meaningless without
    /// one, which is what the first version of `candidates` got wrong.
    fn idx<'a>(known: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |s: &str| known.iter().any(|k| k.eq_ignore_ascii_case(s))
    }

    fn names(p: &str, known: &[&str]) -> Vec<String> {
        candidates(p, &idx(known))
    }

    #[test]
    fn a_bare_lowercase_word_never_stands_alone() {
        assert!(!names("what did we decide about base", &["base"]).contains(&"base".to_string()));
    }

    #[test]
    fn quoting_backticking_or_capitalising_makes_it_a_name() {
        assert!(names("what about `base`", &["base"]).contains(&"base".to_string()));
        assert!(names("what about \"base\"", &["base"]).contains(&"base".to_string()));
        assert!(names("what about Base", &["Base"]).contains(&"Base".to_string()));
    }

    /// kite's case. The old positional partition took `how is first client` at
    /// i=0 and left `kit going`, losing the name entirely.
    #[test]
    fn a_name_in_the_middle_of_a_sentence_is_found_whole() {
        let n = names("how is first client kit going", &["first client kit"]);
        assert_eq!(n, vec!["first client kit".to_string()], "got {n:?}");
    }

    /// kite's case. Nothing in the index means nothing named.
    #[test]
    fn a_sentence_naming_nothing_yields_nothing() {
        assert!(names("how is everything going today", &["first client kit"]).is_empty());
    }

    /// kite's case. The longer name wins at the position both start at, and the
    /// walk resumes AFTER it rather than re-reading its tail.
    #[test]
    fn the_longer_name_wins_and_the_scan_resumes_after_it() {
        let n = names(
            "compare first client kit and Renda today",
            &["first client", "first client kit", "Renda"],
        );
        assert_eq!(n, vec!["first client kit".to_string(), "Renda".to_string()], "got {n:?}");
    }

    #[test]
    fn a_path_shaped_token_is_a_name() {
        assert!(names("look at src/graph_query.rs", &[]).contains(&"src/graph_query.rs".to_string()));
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
                    touched: String::new(),
                })
                .collect(),
        )];
        let (block, dropped) = render(&walked, 300);
        assert!(block.len() <= 300, "budget blown: {}", block.len());
        assert!(dropped > 0, "nothing reported as dropped");
    }

    /// F3, first half. A record the domain block already served must not arrive
    /// a second time under a `<base-context>` heading. `already_served` is the
    /// IRIs those blocks printed; before this it was an empty set, which meant
    /// the dedup the design promised did not exist.
    #[test]
    fn a_record_the_domain_block_served_is_not_served_again() {
        let mut nodes: HashMap<String, Node> = HashMap::new();
        put(&mut nodes, "<x/project/kit>", "first client kit");
        put(&mut nodes, "<x/decision/d1>", "stripe over paddle");
        put(&mut nodes, "<x/decision/d2>", "weekly invoicing");
        let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
        adj.insert(
            "<x/project/kit>".into(),
            vec![
                ("<x/decision/d1>".into(), "belongsTo".into()),
                ("<x/decision/d2>".into(), "belongsTo".into()),
            ],
        );
        let maps = (nodes, adj);
        let ns = ns();

        // Nothing served yet: both decisions come through.
        let all = walk(&maps, &ns, "`first client kit`", &HashSet::new(), &no, &no);
        assert_eq!(all[0].1.len(), 2, "expected both decisions, got {:?}", all[0].1.len());

        // d1 already served by the domain block: only d2 comes through.
        let served: HashSet<String> = HashSet::from(["<x/decision/d1>".to_string()]);
        let some = walk(&maps, &ns, "`first client kit`", &served, &no, &no);
        assert_eq!(some[0].1.len(), 1, "domain-served record was served twice");
        assert_eq!(some[0].1[0].id, "<x/decision/d2>");
    }

    /// A transient record never reaches a prompt, whatever it is attached to.
    #[test]
    fn a_transient_record_is_never_walked_into() {
        let mut nodes: HashMap<String, Node> = HashMap::new();
        put(&mut nodes, "<x/project/kit>", "first client kit");
        // A real ping IRI under the real namespace, judged by the real seam: the
        // hook passes `ontology::transient::is_transient_iri`, so the test does too.
        let ping = format!("<{}ping/p1>", ns().uri);
        put(&mut nodes, &ping, "a ping");
        let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
        adj.insert("<x/project/kit>".into(), vec![(ping.clone(), "mentions".into())]);
        let maps = (nodes, adj);
        let transient = |id: &str| crate::ontology::transient::is_transient_iri(&ns(), id);
        let out = walk(&maps, &ns(), "`first client kit`", &HashSet::new(), &transient, &no);
        assert!(out[0].1.is_empty(), "a ping reached the prompt: {:?}", out[0].1.len());
    }

    /// The recency row of the design-to-code map. Kind first, then most
    /// recently touched, then id. A record with no timestamp sorts AFTER every
    /// record that has one: unknown is not old, and guessing either way puts it
    /// somewhere it does not belong.
    #[test]
    fn records_rank_by_kind_then_recency_with_undated_last() {
        let mut nodes: HashMap<String, Node> = HashMap::new();
        put(&mut nodes, "<x/project/kit>", "first client kit");
        for (id, label, touched) in [
            ("<x/decision/old>", "an old decision", "2026-01-01T00:00:00Z"),
            ("<x/decision/new>", "a new decision", "2026-09-01T00:00:00Z"),
            ("<x/decision/undated>", "an undated decision", ""),
            ("<x/doc/recent>", "a very recent doc", "2026-09-05T00:00:00Z"),
        ] {
            put_dated(&mut nodes, id, label, touched);
        }
        let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
        adj.insert(
            "<x/project/kit>".into(),
            vec![
                ("<x/decision/old>".into(), "belongsTo".into()),
                ("<x/decision/new>".into(), "belongsTo".into()),
                ("<x/decision/undated>".into(), "belongsTo".into()),
                ("<x/doc/recent>".into(), "documents".into()),
            ],
        );
        let out = walk(&(nodes, adj), &ns(), "`first client kit`", &HashSet::new(), &no, &no);
        let ids: Vec<&str> = out[0].1.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "<x/decision/new>",     // decisions outrank docs
                "<x/decision/old>",     // newer decision first
                "<x/decision/undated>", // undated sorts after both dated ones
                "<x/doc/recent>",       // a doc, however recent, ranks below decisions
            ],
            "got {ids:?}"
        );
    }

    /// The rendered shape. The relation is not decoration: it is the answer to
    /// "why is this line here", and an agent that cannot see it has to guess.
    #[test]
    fn each_block_names_the_thing_and_shows_the_relation() {
        let walked = vec![(
            Resolved {
                name: "first client kit".into(),
                id: "<x/project/kit>".into(),
                kind: "project".into(),
                hops: 2,
                ties: 1,
            },
            vec![Record {
                kind: "decision".into(),
                label: "stripe over paddle".into(),
                relation: "belongsTo".into(),
                id: "<x/decision/d1>".into(),
                touched: "2026-09-01T00:00:00Z".into(),
            }],
        )];
        let (block, dropped) = render(&walked, 4000);
        assert_eq!(dropped, 0);
        assert!(block.starts_with("<base-context name=\"first client kit\" kind=\"project\" hops=\"2\">"), "{block}");
        assert!(block.contains("stripe over paddle"), "{block}");
        assert!(block.contains("belongsTo"), "the relation is missing: {block}");
        assert!(block.trim_end().ends_with("</base-context>"), "{block}");
    }

    /// F18. `resolve_strict` tries the slug BEFORE the label, so an index of
    /// labels alone left that half of the resolver unreachable from the hook:
    /// `first-client-kit` in prose failed the gate before the resolver that
    /// knows how to read it ever saw it. Two halves that did not meet.
    #[test]
    fn a_node_is_found_by_its_slug_as_well_as_its_label() {
        let mut nodes: HashMap<String, Node> = HashMap::new();
        put(&mut nodes, "<x/project/kit>", "First Client Kit");
        let adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let maps = (nodes, adj);

        for spelling in ["`First Client Kit`", "`first-client-kit`", "First Client Kit"] {
            let out = walk(&maps, &ns(), spelling, &HashSet::new(), &no, &no);
            assert_eq!(out.len(), 1, "{spelling:?} resolved to {} things", out.len());
            assert_eq!(out[0].0.id, "<x/project/kit>", "{spelling:?}");
        }
    }

    #[test]
    fn an_empty_walk_renders_nothing() {
        assert_eq!(render(&[], 1000).0, "");
    }
}
