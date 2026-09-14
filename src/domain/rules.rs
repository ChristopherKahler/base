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

use std::collections::{HashMap, HashSet};

use oxigraph::model::TermRef;
use oxigraph::store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::BaseConfig;
use crate::crud;
use crate::domain::session::{Bracket, ReShow, SessionState};
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

/// Render the rules this event should carry, under `header`, numbered by each rule's place in the
/// domain's merged two-tier list. That number is NOT a `base rule list` index, which is per tier
/// (`petrel`'s F3 on `8977379`); the pointer line names the listing that has them.
///
/// `shown` is the subset that survived the per-rule dedup, paired with each rule's
/// index in the full list. `total` is how many the domain has, so the pointer line can
/// say what was held back rather than leaving the reader to wonder.
///
/// Empty when nothing survived: a header with no rules under it costs the reader a
/// line and tells them nothing.
pub fn render_block(header: &str, shown: &[(usize, &ServedRule)], total: usize, domain: &str) -> String {
    if shown.is_empty() {
        return String::new();
    }
    let mut out = format!("[{header}: {domain}]\n");
    for (i, rule) in shown {
        out.push_str(&format!("  {i}. {}\n", rule.rendered));
    }
    let withheld = total.saturating_sub(shown.len());
    if withheld > 0 {
        // F16's shape. One line in place of the rules this session has already been
        // told, which is the whole point of dedup per rule rather than per block.
        out.push_str(&format!(
            "  ({withheld} more {domain} rule(s) already served this session · all: base rule list --domain {domain})\n"
        ));
    }
    out
}

/// Collapse whitespace so that a reflow of a rule in `domains.toml` is the same rule.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

/// A rule's stable identity: the first 8 bytes, as hex, of SHA-256 over the domain's slug, U+0001 and the
/// normalised text. See the module docs for why it is not the IRI.
///
/// SHA-256 rather than `DefaultHasher`, whose output Rust does not promise across releases: an id that moved on
/// a toolchain upgrade would re-serve every rule to every open session (A6, `auk` 2026-09-14).
pub fn rule_id(domain: &str, text: &str) -> String {
    let d = sha256(format!("{}\u{1}{}", crud::slugify(domain), normalize(text)).as_bytes());
    d[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// A hash of the rendered rule, for "has this exact text been served already": the first 8 bytes of its
/// SHA-256, big-endian.
pub fn content_hash(rendered: &str) -> u64 {
    let d = sha256(rendered.as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    u64::from_be_bytes(b)
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

// ─── Commit 4: the four kinds and their matchers ─────────────────────────────
//
// F1, the principle Chris locked: make the single rule the unit, and serve it when it applies. A rule may carry
// matchers of its own, and a rule that does is served on them and not through its domain's triggers. A rule that
// carries none is served exactly as before, through its domain (K4, and `auk`'s HARD RULE of 2026-09-14: never
// dropped). The rulings this code follows are in the lane brief, section COMMIT 4, part 4.

/// When a rule matters (F2). Every rule answers one question, and there are four answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Session start, and again when the context bracket changes tier.
    Always,
    /// The first tool call touching a folder, or a file the rule names.
    Place,
    /// Right before a tool or a command runs.
    Action,
    /// When the prompt is about the subject.
    Topic,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "always" => Some(Self::Always),
            "place" => Some(Self::Place),
            "action" => Some(Self::Action),
            "topic" => Some(Self::Topic),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Place => "place",
            Self::Action => "action",
            Self::Topic => "topic",
        }
    }
}

/// One reason a rule fires. A rule may carry several, and any one hitting is enough (F3).
///
/// In `domains.toml` this is a `[[domain.rules.match]]` table (F12). In the graph it is flat literals on the rule
/// (see [`matcher_literals`]), so a rule reads back the same from either place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Matcher {
    pub kind: Kind,
    /// A folder, or a file when the rule names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// A tool name, MCP tools included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// A command, matched per [`command_hit`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Topic words and phrases.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
}

impl Matcher {
    fn bare(kind: Kind) -> Self {
        Self { kind, place: None, tool: None, command: None, words: Vec::new() }
    }

    pub fn always() -> Self {
        Self::bare(Kind::Always)
    }

    pub fn for_place(place: &str) -> Self {
        Self { place: Some(place.to_string()), ..Self::bare(Kind::Place) }
    }

    pub fn for_tool(tool: &str) -> Self {
        Self { tool: Some(tool.to_string()), ..Self::bare(Kind::Action) }
    }

    pub fn for_command(command: &str) -> Self {
        Self { command: Some(command.to_string()), ..Self::bare(Kind::Action) }
    }

    /// A topic matcher. With no words, the rule's text and its domain's keywords carry the topic (F11's fallback).
    pub fn for_topic(words: Vec<String>) -> Self {
        Self { words, ..Self::bare(Kind::Topic) }
    }
}

// ─── The flat graph shape (G0 2.2) ───────────────────────────────────────────

/// The predicates a matcher flattens into, as local names under the namespace prefix.
pub const MATCH_PREDICATES: [&str; 5] = ["matchKind", "matchPlace", "matchTool", "matchCommand", "matchWord"];

/// A rule's matchers as `(predicate, value)` pairs: the shape `base domain sync` and `base rule add` write.
pub fn matcher_literals(matchers: &[Matcher]) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let mut push = |pred: &'static str, value: &str| {
        let value = value.trim();
        if !value.is_empty() && !out.iter().any(|(p, v)| *p == pred && v == value) {
            out.push((pred, value.to_string()));
        }
    };
    for m in matchers {
        push("matchKind", m.kind.as_str());
        match m.kind {
            Kind::Always => {}
            Kind::Place => {
                if let Some(p) = &m.place {
                    push("matchPlace", p);
                }
            }
            Kind::Action => {
                if let Some(t) = &m.tool {
                    push("matchTool", t);
                }
                if let Some(c) = &m.command {
                    push("matchCommand", c);
                }
            }
            Kind::Topic => {
                for w in &m.words {
                    push("matchWord", w);
                }
            }
        }
    }
    out
}

/// Rebuild matchers from flat pairs, in one fixed order: places, tools, commands, one topic, always.
///
/// A kind with nothing to match is dropped rather than kept as a matcher that can never fire: a place kind with no
/// place, an action kind with no tool and no command. A topic kind with no words stays, because its rule's text and
/// its domain's keywords carry the topic. Several topic matchers are one word list, because the graph shape is flat
/// and a rule must score the same whether it was read from `domains.toml` or from the graph (A5).
pub fn matchers_from_literals<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Matcher> {
    let (mut kinds, mut places, mut tools, mut commands, mut words) =
        (Vec::new(), Vec::<String>::new(), Vec::<String>::new(), Vec::<String>::new(), Vec::<String>::new());
    for (pred, value) in pairs {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let list = match pred {
            "matchKind" => {
                if let Some(k) = Kind::parse(value)
                    && !kinds.contains(&k)
                {
                    kinds.push(k);
                }
                continue;
            }
            "matchPlace" => &mut places,
            "matchTool" => &mut tools,
            "matchCommand" => &mut commands,
            "matchWord" => &mut words,
            _ => continue,
        };
        if !list.iter().any(|v| v == value) {
            list.push(value.to_string());
        }
    }
    places.sort();
    tools.sort();
    commands.sort();
    words.sort();
    let mut out: Vec<Matcher> = places.iter().map(|p| Matcher::for_place(p)).collect();
    out.extend(tools.iter().map(|t| Matcher::for_tool(t)));
    out.extend(commands.iter().map(|c| Matcher::for_command(c)));
    if !words.is_empty() || kinds.contains(&Kind::Topic) {
        out.push(Matcher::for_topic(words));
    }
    if kinds.contains(&Kind::Always) {
        out.push(Matcher::always());
    }
    out
}

/// The same fold, for matchers written by hand in `domains.toml`.
pub fn normalize_matchers(matchers: &[Matcher]) -> Vec<Matcher> {
    let pairs = matcher_literals(matchers);
    matchers_from_literals(pairs.iter().map(|(p, v)| (*p, v.as_str())))
}

/// Matchers from `base rule add`'s flags (F11). A kind a value flag already implies need not be given. A kind that
/// needs a value it was not given is refused, never stored as a matcher that cannot fire.
pub fn matchers_from_flags(
    kinds: &[String],
    places: &[String],
    tools: &[String],
    commands: &[String],
    words: Option<&str>,
) -> Result<Vec<Matcher>, String> {
    let given = |v: &[String]| v.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).map(String::from).collect::<Vec<_>>();
    let mut out: Vec<Matcher> = given(places).iter().map(|p| Matcher::for_place(p)).collect();
    out.extend(given(tools).iter().map(|t| Matcher::for_tool(t)));
    out.extend(given(commands).iter().map(|c| Matcher::for_command(c)));
    let word_list: Vec<String> = words
        .map(|w| w.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect())
        .unwrap_or_default();
    if !word_list.is_empty() {
        out.push(Matcher::for_topic(word_list));
    }
    for k in kinds {
        let kind = Kind::parse(k).ok_or_else(|| format!("unknown --kind '{k}': use always, place, action or topic"))?;
        let has = |out: &[Matcher]| out.iter().any(|m| m.kind == kind);
        match kind {
            Kind::Always if !has(&out) => out.push(Matcher::always()),
            Kind::Topic if !has(&out) => out.push(Matcher::for_topic(Vec::new())),
            Kind::Place if !has(&out) => return Err("--kind place needs --place <folder or file>".into()),
            Kind::Action if !has(&out) => return Err("--kind action needs --tool <name> or --command <command>".into()),
            _ => {}
        }
    }
    Ok(normalize_matchers(&out))
}

/// One line naming a rule's matchers, for `base rule list` (F11: it "shows each rule's kinds and matchers").
pub fn describe_matchers(matchers: &[Matcher]) -> String {
    matchers
        .iter()
        .map(|m| match m.kind {
            Kind::Always => "always".to_string(),
            Kind::Place => format!("place {}", m.place.as_deref().unwrap_or_default()),
            Kind::Action => match (&m.tool, &m.command) {
                (Some(t), _) => format!("action tool {t}"),
                (None, Some(c)) => format!("action command \"{c}\""),
                (None, None) => "action".to_string(),
            },
            Kind::Topic if m.words.is_empty() => "topic (its text and its domain's keywords)".to_string(),
            Kind::Topic => format!("topic \"{}\"", m.words.join("\", \"")),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

// ─── Commands (F5, A1, A4) ───────────────────────────────────────────────────

/// Split a command line into words the way a shell would: whitespace separates words, and single or double quotes
/// keep a word together. A backslash is literal, because on this machine it is far more often a Windows path
/// separator than an escape.
pub fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            None => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

/// The separate commands in one line: split on `&&`, `||`, `;`, `|` and newlines, never inside quotes.
fn statements(s: &str) -> Vec<String> {
    fn flush(out: &mut Vec<String>, cur: &mut String) {
        let t = cur.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
        cur.clear();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            cur.push(c);
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                cur.push(c);
            }
            ';' | '\n' | '\r' => flush(&mut out, &mut cur),
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                flush(&mut out, &mut cur);
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                flush(&mut out, &mut cur);
            }
            _ => cur.push(c),
        }
    }
    flush(&mut out, &mut cur);
    out
}

/// The program a word names: `C:/x/base.exe`, `/usr/bin/base` and `base` are all `base`.
pub fn program_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
    base.strip_suffix(".exe").map(str::to_string).unwrap_or(base)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// Words back into one line, re-quoting any word the shell would otherwise split or cut.
fn join_words(words: &[String]) -> Option<String> {
    match words {
        [] => None,
        [one] => Some(one.clone()),
        _ => Some(
            words
                .iter()
                .map(|w| {
                    if w.is_empty() || w.contains(|c: char| c.is_whitespace() || "'\";|&".contains(c)) {
                        if w.contains('\'') { format!("\"{w}\"") } else { format!("'{w}'") }
                    } else {
                        w.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
        ),
    }
}

/// The command inside a wrapper, when the words are one.
fn unwrap_command(words: &[String]) -> Option<String> {
    let rest = words.get(1..)?;
    match program_name(words.first()?).as_str() {
        // `wsl -- cmd …` and `wsl -e cmd …` run a command directly, with no inner shell.
        "wsl" => {
            let i = rest.iter().position(|w| w == "--" || w == "-e" || w == "--exec")?;
            join_words(&rest[i + 1..])
        }
        "bash" | "sh" | "zsh" | "dash" => {
            let i = rest.iter().position(|w| w.len() > 1 && w.starts_with('-') && !w.starts_with("--") && w[1..].contains('c'))?;
            rest.get(i + 1).cloned()
        }
        "powershell" | "pwsh" => {
            let i = rest.iter().position(|w| matches!(w.to_ascii_lowercase().as_str(), "-command" | "-c" | "-file" | "-f"))?;
            join_words(&rest[i + 1..])
        }
        "cmd" => {
            let i = rest.iter().position(|w| w.eq_ignore_ascii_case("/c") || w.eq_ignore_ascii_case("/k"))?;
            join_words(&rest[i + 1..])
        }
        _ => None,
    }
}

/// The commands a Bash or PowerShell call actually runs, each as shell words, wrappers taken off (F5, A4).
///
/// Leading `NAME=value` assignments and PowerShell's `&` call operator are dropped. A wrapper is kept as a part of
/// its own as well as unwrapped, since a rule may be tied to `wsl` itself.
pub fn command_parts(raw: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut queue: Vec<(String, u8)> = vec![(raw.to_string(), 0)];
    while let Some((text, depth)) = queue.pop() {
        for stmt in statements(&text) {
            let mut words = shell_words(&stmt);
            let skip = words.iter().take_while(|w| w.as_str() == "&" || is_assignment(w)).count();
            words.drain(..skip);
            if words.is_empty() {
                continue;
            }
            if depth < 6
                && let Some(inner) = unwrap_command(&words)
            {
                queue.push((inner, depth + 1));
            }
            if !out.contains(&words) {
                out.push(words);
            }
        }
    }
    out
}

/// Does a command matcher's `needle` match one normalised command (A1, ruled by `auk` 2026-09-14)?
///
/// The needle's leading words, up to its first flag, must be the command's leading words in order, the first
/// compared as a program name. A word that follows a flag in the command is skipped while matching them, since it
/// is that flag's value: `git -C /dir merge x` is a `git merge`, and `git log --grep merge` is not. After that,
/// every remaining needle word must be present: a flag followed by a word needs that same word right after the
/// flag (or `--flag=value`), and a lone word needs to appear. So `base relay ping --from shrike --to chris` matches
/// `base relay ping --to chris`, and `git merge-base` does not match `git merge`.
pub fn command_hit(needle: &str, part: &[String]) -> bool {
    let n = shell_words(needle);
    let (Some(first), Some(prog)) = (n.first(), part.first()) else {
        return false;
    };
    if program_name(prog) != program_name(first) {
        return false;
    }
    let lead = n.iter().take_while(|w| !w.starts_with('-')).count();
    let mut j = 1;
    let mut after_flag = false;
    for w in &part[1..] {
        if j == lead {
            break;
        }
        if w.starts_with('-') {
            after_flag = true;
        } else if w.eq_ignore_ascii_case(&n[j]) {
            j += 1;
            after_flag = false;
        } else if after_flag {
            after_flag = false;
        } else {
            return false;
        }
    }
    if j < lead {
        return false;
    }
    let flag_value = |flag: &str, value: &str| {
        part.windows(2).any(|p| p[0].eq_ignore_ascii_case(flag) && p[1].eq_ignore_ascii_case(value))
            || part.iter().any(|p| p.eq_ignore_ascii_case(&format!("{flag}={value}")))
    };
    let mut i = lead;
    while i < n.len() {
        let w = &n[i];
        if w.starts_with('-') {
            if let Some((flag, value)) = w.split_once('=') {
                if !flag_value(flag, value) {
                    return false;
                }
                i += 1;
            } else if let Some(value) = n.get(i + 1).filter(|v| !v.starts_with('-')) {
                if !flag_value(w.as_str(), value.as_str()) {
                    return false;
                }
                i += 2;
            } else {
                let with_eq = format!("{}=", w.to_ascii_lowercase());
                if !part.iter().any(|p| p.eq_ignore_ascii_case(w) || p.to_ascii_lowercase().starts_with(&with_eq)) {
                    return false;
                }
                i += 1;
            }
        } else {
            if !part[1..].iter().any(|p| p.eq_ignore_ascii_case(w)) {
                return false;
            }
            i += 1;
        }
    }
    true
}

// ─── Places (F4) ─────────────────────────────────────────────────────────────

fn path_components(p: &str) -> Vec<&str> {
    p.split(['/', '\\']).filter(|c| !c.is_empty() && *c != "." && *c != "~").collect()
}

/// A place names a file when its last segment has a dot after its first character: `commands.toml` is a file,
/// `ping-chat-hub` and `.base-gbl` are folders.
fn names_file(place: &str) -> bool {
    let t = place.trim();
    !t.ends_with(['/', '\\']) && path_components(t).last().is_some_and(|last| last.char_indices().any(|(i, c)| c == '.' && i > 0))
}

/// Does a touched `path` lie in `place` (F4)?
///
/// Folders are the default. A file is a matcher only when the rule names that file, and then the path's trailing
/// segments must be the place's segments. A relative folder names that folder wherever it sits, so `ping-chat-hub`
/// and `Documents/renda-group` match on whole path segments, never on a substring. A `~` or absolute folder is
/// resolved and compared with `path_under`, the seam the domain matcher uses.
pub fn place_hit(path: &str, place: &str, home: Option<&str>) -> bool {
    let (pc, ac) = (path_components(place), path_components(path));
    if pc.is_empty() || ac.is_empty() || pc.len() > ac.len() {
        return false;
    }
    let same = |a: &[&str], b: &[&str]| a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y));
    if names_file(place) {
        return same(&ac[ac.len() - pc.len()..], pc.as_slice());
    }
    let t = place.trim();
    if domain::matcher::is_absolute(t) || t == "~" || t.starts_with("~/") || t.starts_with("~\\") {
        return domain::matcher::resolve_trigger(t, None, home).is_some_and(|r| domain::matcher::path_under(path, &r));
    }
    ac.windows(pc.len()).any(|w| same(w, pc.as_slice()))
}

// ─── Topics (F6, A5) ─────────────────────────────────────────────────────────

/// A topic phrase of the rule's own: 1.0 each (A5, `auk` 2026-09-14).
pub const TOPIC_OWN_PHRASE: f32 = 1.0;
/// A phrase from the domain's `prompt_keywords`: 0.5 each.
pub const TOPIC_KEYWORD_PHRASE: f32 = 0.5;
/// A content word shared with the rule's text: 0.25 each.
pub const TOPIC_TEXT_WORD: f32 = 0.25;
/// Rule-text words together never score more than this, which is below the default minimum of 0.75: words from a
/// long rule text can rank a rule, and can never fire it on their own.
pub const TOPIC_TEXT_CAP: f32 = 0.5;

/// Words too common to carry a topic. Kept short: a long list starts deciding what a rule is about.
const STOPWORDS: &[&str] = &[
    "and", "are", "but", "can", "does", "for", "from", "get", "has", "have", "how", "its", "not", "that", "the",
    "then", "there", "they", "this", "use", "was", "what", "when", "where", "which", "why", "will", "with", "you",
    "your",
];

/// The content words of a text: lowercased, split on anything that is not a letter, a digit, `-` or `_`, three
/// characters or more, stopwords removed.
pub fn content_words(text: &str) -> HashSet<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .map(|w| w.trim_matches(['-', '_']).to_lowercase())
        .filter(|w| w.chars().count() >= 3 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// How well a topic rule matches a prompt (F6, LOCKED: the rule's text, its own topic words and its domain's
/// keywords are all scored; weighted per A5). No model call.
///
/// Own words and domain keywords match as whole phrases with `contains_word`, so `ping chris` is one phrase, not two
/// loose words. Text words match as a set, capped at [`TOPIC_TEXT_CAP`] in total.
pub fn topic_score(prompt: &str, own_words: &[String], rule_text: &str, domain_keywords: &[String]) -> f32 {
    let lower = prompt.to_lowercase();
    let phrases = |list: &[String]| {
        list.iter()
            .map(|w| w.trim().to_lowercase())
            .filter(|w| !w.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|w| domain::matcher::contains_word(&lower, w))
            .count() as f32
    };
    let text_hits = content_words(rule_text).intersection(&content_words(prompt)).count() as f32;
    phrases(own_words) * TOPIC_OWN_PHRASE
        + phrases(domain_keywords) * TOPIC_KEYWORD_PHRASE
        + (text_hits * TOPIC_TEXT_WORD).min(TOPIC_TEXT_CAP)
}

// ─── Loading the rules that carry matchers ───────────────────────────────────

/// A rule that carries matchers of its own. K4's word for it is "converted": until an operator approves a
/// conversion, no rule on a machine is one, and every rule keeps being served through its domain.
#[derive(Debug, Clone, PartialEq)]
pub struct Converted {
    pub rule: ServedRule,
    pub matchers: Vec<Matcher>,
}

/// One rule's rows from the graph, before its matchers are folded.
struct RawRule {
    iri: String,
    domain: String,
    text: String,
    rationale: Option<String>,
    pairs: Vec<(String, String)>,
}

/// Every converted rule across both tiers, with superseded and empty ones dropped (F10, F13).
///
/// One query, not one per domain, because this runs on every tool call. The graph is the live path, and
/// `domains.toml` is the fallback for a store that holds no matcher at all: the same precedence as
/// [`rules_for_domain`]. A domain with `auto_inject = false` stays out, as it stays out of every automatic
/// injection (F29 D3). A rule whose domain has no `domains.toml` entry is kept: its matchers are its own trigger.
pub fn rules_with_matchers(store: Option<&Store>, config: &BaseConfig, domains: &[DomainDef]) -> Vec<Converted> {
    let from_graph = store.map(|s| converted_from_graph(s, config, domains)).unwrap_or_default();
    if from_graph.is_empty() { converted_from_toml(domains) } else { from_graph }
}

fn converted_from_toml(domains: &[DomainDef]) -> Vec<Converted> {
    let mut out = Vec::new();
    for d in domains.iter().filter(|d| d.auto_inject) {
        for r in d.rules.iter().filter(|r| !r.text().trim().is_empty()) {
            let matchers = normalize_matchers(r.matchers());
            if !matchers.is_empty() {
                let rule = build(&d.name, r.text().to_string(), r.rationale().map(String::from), None);
                out.push(Converted { rule, matchers });
            }
        }
    }
    out
}

fn converted_from_graph(store: &Store, config: &BaseConfig, domains: &[DomainDef]) -> Vec<Converted> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let pfx = crud::prefixes(ns);
    // Inside the GRAPH group, beside the pattern it constrains, for the reason `from_graph` gives.
    let no_superseded = crate::supersede::sparql_exclude_superseded(ns, "rule");
    let sparql = format!(
        "{pfx}\n\
         SELECT ?domain ?rule ?text ?rationale ?mp ?mv WHERE {{\n\
           GRAPH ?g {{\n\
             ?domain {p}:hasRule ?rule .\n\
             ?rule {p}:ruleText ?text .\n\
             ?rule ?mp ?mv .\n\
             FILTER(?mp IN ({p}:matchKind, {p}:matchPlace, {p}:matchTool, {p}:matchCommand, {p}:matchWord))\n\
             OPTIONAL {{ ?rule {p}:rationale ?rationale }}\n\
             {no_superseded}\
           }}\n\
         }}"
    );
    let Ok(oxigraph::sparql::QueryResults::Solutions(rows)) = crate::store::query(store, &sparql) else {
        return Vec::new();
    };

    let mut raw: Vec<RawRule> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for row in rows.filter_map(|r| r.ok()) {
        let named = |k: &str| {
            row.get(k).and_then(|t| match t.into() {
                TermRef::NamedNode(n) => Some(n.as_str().to_string()),
                _ => None,
            })
        };
        let literal = |k: &str| {
            row.get(k).and_then(|t| match t.into() {
                TermRef::Literal(l) => Some(l.value().to_string()),
                _ => None,
            })
        };
        let (Some(domain), Some(iri), Some(text), Some(mp), Some(mv)) =
            (named("domain"), named("rule"), literal("text"), named("mp"), literal("mv"))
        else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let pred = mp.rsplit(['#', '/']).next().unwrap_or_default().to_string();
        let i = match index.get(&iri) {
            Some(i) => *i,
            None => {
                index.insert(iri.clone(), raw.len());
                let rationale = literal("rationale").filter(|r| !r.is_empty());
                raw.push(RawRule { iri, domain, text, rationale, pairs: Vec::new() });
                raw.len() - 1
            }
        };
        raw[i].pairs.push((pred, mv));
    }

    let known: HashMap<String, (&str, bool)> = domains
        .iter()
        .map(|d| (crud::build_iri(ns, "domain", &crud::slugify(&d.name)), (d.name.as_str(), d.auto_inject)))
        .collect();
    let mut out: Vec<Converted> = Vec::new();
    for r in raw {
        let (name, auto_inject) = match known.get(&r.domain) {
            Some((n, a)) => ((*n).to_string(), *a),
            None => (r.domain.rsplit('/').next().unwrap_or_default().to_string(), true),
        };
        if !auto_inject {
            continue;
        }
        let matchers = matchers_from_literals(r.pairs.iter().map(|(p, v)| (p.as_str(), v.as_str())));
        if matchers.is_empty() {
            continue;
        }
        let rule = build(&name, r.text, r.rationale, Some(format!("<{}>", r.iri)));
        // One rule, one entry, across both tiers: the same text declared in two tiers is one rule, and it carries
        // the matchers of both.
        match out.iter_mut().find(|c| c.rule.domain == rule.domain && c.rule.rendered == rule.rendered) {
            Some(existing) => {
                let mut pairs = matcher_literals(&existing.matchers);
                pairs.extend(matcher_literals(&matchers));
                existing.matchers = matchers_from_literals(pairs.iter().map(|(p, v)| (*p, v.as_str())));
            }
            None => out.push(Converted { rule, matchers }),
        }
    }
    out
}

// ─── What an event serves (G0 section 3) ─────────────────────────────────────

/// What is happening, as the three hooks see it.
pub enum Event<'a> {
    SessionStart,
    Prompt {
        text: &'a str,
    },
    /// A tool call about to run. `command` is the Bash or PowerShell command when the tool carries one (A2).
    PreTool {
        tool: &'a str,
        paths: &'a [String],
        command: Option<&'a str>,
    },
}

/// Why a rule was served: the header the reader sees, and half of a place or action rule's dedup key.
#[derive(Debug, Clone, PartialEq)]
pub enum Why {
    Always,
    Place(String),
    Action(String),
    Topic(f32),
}

impl Why {
    pub fn kind(&self) -> Kind {
        match self {
            Why::Always => Kind::Always,
            Why::Place(_) => Kind::Place,
            Why::Action(_) => Kind::Action,
            Why::Topic(_) => Kind::Topic,
        }
    }

    /// The dedup scope (F8): a place rule is once per place, an action rule once per action.
    pub fn scope(&self) -> Option<&str> {
        match self {
            Why::Place(s) | Why::Action(s) => Some(s),
            Why::Always | Why::Topic(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Served {
    pub rule: ServedRule,
    pub why: Why,
}

#[derive(Debug, Default)]
pub struct Selection {
    pub served: Vec<Served>,
    /// Per domain, how many topic rules matched this prompt and were cut by `topic_max`: F6's pointer line.
    pub topic_withheld: Vec<(String, usize)>,
}

/// What [`select`] needs beyond the rules and the event.
pub struct SelectContext<'a> {
    pub bracket: Bracket,
    pub now: u64,
    pub home: Option<&'a str>,
    /// Domain name to its `prompt_keywords`, for topic scoring.
    pub keywords: &'a HashMap<String, Vec<String>>,
    pub rules: &'a crate::config::RulesConfig,
}

/// The converted rules this event serves, deduped per rule and capped, recorded as shown (G0 section 3).
///
/// The order is fixed. Match each rule against the event, keeping its first hit, so two matchers hitting at once
/// still show it once (F3). Ask the dedup record whether it is due, WITHOUT recording. Cap topic hits at
/// `topic_max` by score and count what the cap cut, per domain. Only then record what is returned, so a rule the
/// cap cut is never marked as shown and arrives on a later prompt.
pub fn select(converted: &[Converted], event: &Event<'_>, session: &mut SessionState, cx: &SelectContext<'_>) -> Selection {
    let parts = match event {
        Event::PreTool { command: Some(c), .. } => command_parts(c),
        _ => Vec::new(),
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut topics: Vec<(usize, Why)> = Vec::new();
    let mut others: Vec<(usize, Why)> = Vec::new();
    for (i, c) in converted.iter().enumerate() {
        if !seen.insert(c.rule.id.as_str()) {
            continue;
        }
        let Some(why) = first_hit(c, event, &parts, cx) else {
            continue;
        };
        let reshow = match why {
            Why::Action(_) => ReShow::Throttle { secs: cx.rules.action_throttle_minutes.saturating_mul(60) },
            _ => ReShow::PerSession { on_tier_change: cx.rules.reshow_on_tier_change },
        };
        if !session.rule_due(&c.rule.id, c.rule.content_hash, cx.bracket, why.scope(), reshow, cx.now) {
            continue;
        }
        if matches!(why, Why::Topic(_)) { topics.push((i, why)) } else { others.push((i, why)) }
    }

    let score = |w: &Why| if let Why::Topic(s) = w { *s } else { 0.0 };
    topics.sort_by(|a, b| score(&b.1).total_cmp(&score(&a.1)));
    let mut topic_withheld: Vec<(String, usize)> = Vec::new();
    for (i, _) in topics.iter().skip(cx.rules.topic_max) {
        let domain = &converted[*i].rule.domain;
        match topic_withheld.iter_mut().find(|(d, _)| d == domain) {
            Some((_, n)) => *n += 1,
            None => topic_withheld.push((domain.clone(), 1)),
        }
    }
    topics.truncate(cx.rules.topic_max);

    let mut served = Vec::new();
    for (i, why) in others.into_iter().chain(topics) {
        let rule = &converted[i].rule;
        session.mark_rule_shown(&rule.id, rule.content_hash, cx.bracket, why.scope(), cx.now);
        served.push(Served { rule: rule.clone(), why });
    }
    Selection { served, topic_withheld }
}

fn first_hit(c: &Converted, event: &Event<'_>, parts: &[Vec<String>], cx: &SelectContext<'_>) -> Option<Why> {
    let has = |k: Kind| c.matchers.iter().any(|m| m.kind == k);
    match event {
        Event::SessionStart => has(Kind::Always).then_some(Why::Always),
        Event::Prompt { text } => {
            if has(Kind::Always) {
                return Some(Why::Always);
            }
            if !has(Kind::Topic) {
                return None;
            }
            let own: Vec<String> =
                c.matchers.iter().filter(|m| m.kind == Kind::Topic).flat_map(|m| m.words.iter().cloned()).collect();
            let keywords = cx.keywords.get(&c.rule.domain).map(Vec::as_slice).unwrap_or_default();
            let s = topic_score(text, &own, &c.rule.text, keywords);
            (s > 0.0 && s >= cx.rules.topic_min_score).then_some(Why::Topic(s))
        }
        Event::PreTool { tool, paths, .. } => c.matchers.iter().find_map(|m| match m.kind {
            Kind::Place => m
                .place
                .as_ref()
                .filter(|place| paths.iter().any(|path| place_hit(path, place, cx.home)))
                .map(|place| Why::Place(place.clone())),
            Kind::Action => m
                .tool
                .as_ref()
                .filter(|t| t.eq_ignore_ascii_case(tool))
                .or_else(|| m.command.as_ref().filter(|cmd| parts.iter().any(|part| command_hit(cmd, part))))
                .map(|a| Why::Action(a.clone())),
            Kind::Always | Kind::Topic => None,
        }),
    }
}

/// What [`select`] returned, as the reader receives it (F16's shapes): one header per reason, the rules under it in
/// the order `select` chose them, then F6's pointer line for every domain whose topic rules the cap withheld.
///
/// Empty when nothing was served, which is also when nothing was withheld: the cap only withholds past `topic_max`.
pub fn render_selection(selection: &Selection) -> String {
    let mut groups: Vec<(String, Vec<&ServedRule>)> = Vec::new();
    for served in &selection.served {
        let header = match &served.why {
            Why::Always => "[base rules · always]".to_string(),
            Why::Place(place) => format!("[base rule · place: {place}]"),
            Why::Action(action) => format!("[base rule · before: {action}]"),
            Why::Topic(_) => format!("[base rules · topic: {}]", served.rule.domain),
        };
        match groups.iter_mut().find(|(h, _)| *h == header) {
            Some((_, rules)) => rules.push(&served.rule),
            None => groups.push((header, vec![&served.rule])),
        }
    }
    let mut out = String::new();
    for (header, rules) in groups {
        out.push_str(&header);
        out.push('\n');
        for rule in rules {
            out.push_str(&format!("  - {}\n", rule.rendered));
        }
    }
    for (domain, withheld) in &selection.topic_withheld {
        out.push_str(&format!("  ({withheld} more {domain} rules · all: base rule list --domain {domain})\n"));
    }
    out
}

#[cfg(test)]
mod model_tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        shell_words(s)
    }

    #[test]
    fn rule_id_is_sha256_and_matches_an_independent_implementation() {
        // Vectors computed with Python hashlib over slug + U+0001 + normalised text, first 8 bytes as hex
        // (`c4/vectors.py` in the lane harness). Same value in every process, on every platform, on every Rust
        // release, which DefaultHasher does not promise (A6).
        assert_eq!(rule_id("probe", "FIRST RULE"), "c632df7506cf2f27");
        assert_eq!(rule_id("Base Config", "  keep   the  tier "), "2a025f6e4a4cce6c");
        assert_eq!(rule_id("base-config", "keep the tier"), "2a025f6e4a4cce6c", "slug and whitespace fold");
        assert_eq!(content_hash("FIRST RULE"), 12_579_418_905_532_764_516);
        assert_ne!(rule_id("probe", "FIRST RULE"), rule_id("probe", "FIRST RULE."), "an edited rule is a new rule");
    }

    #[test]
    fn command_matching_follows_a1() {
        let hit = |needle: &str, cmd: &str| command_parts(cmd).iter().any(|p| command_hit(needle, p));
        assert!(hit("base relay ping --to chris", "base relay ping --from shrike --to chris --msg 'x'"), "flag order");
        assert!(!hit("base relay ping --to chris", "base relay ping --to auk --msg 'tell --to chris'"), "quoted words");
        assert!(hit("git merge", "git merge feature"));
        assert!(!hit("git merge", "git merge-base main HEAD"), "no match inside a word");
        assert!(hit("git merge", "git -C /home/x/repo merge feature"), "a flag's value is skipped");
        assert!(!hit("git merge", "git log --grep merge"), "a subcommand is not a flag value");
        assert!(hit("base rule add", "base rule -g add --domain d --text t"), "a flag between subcommand words");
        assert!(hit("curl -X POST http://127.0.0.1:7799/api/spawn", "curl -s -X POST -d '{}' http://127.0.0.1:7799/api/spawn"));
        assert!(hit("respawn.ps1", "& 'C:/Users/Chris/.base-gbl/scripts/respawn.ps1' -Codename shrike"), "PowerShell &");
        assert!(hit("respawn.ps1", "powershell -NoProfile -File C:\\Users\\Chris\\.base-gbl\\scripts\\respawn.ps1 -Codename x"));
    }

    #[test]
    fn command_parts_see_through_the_wrappers_this_machine_uses() {
        let parts = command_parts("MSYS_NO_PATHCONV=1 wsl -- bash -c 'cd /x && base relay ping --to chris' ; echo done");
        assert!(parts.contains(&w("base relay ping --to chris")), "VAR=value, wsl --, bash -c, and a && b: {parts:?}");
        assert!(parts.contains(&w("cd /x")) && parts.contains(&w("echo done")), "{parts:?}");
        assert!(parts.iter().any(|p| p.first().is_some_and(|f| f == "wsl")), "the wrapper stays a part: {parts:?}");
        let ps = command_parts("powershell -NoProfile -Command \"base relay ping --to chris --msg 'a; b'\"");
        assert!(ps.contains(&w("base relay ping --to chris --msg 'a; b'")), "a quoted ; is not a separator: {ps:?}");
    }

    #[test]
    fn places_match_whole_segments_and_files_only_when_named() {
        assert!(place_hit(r"C:\Users\Chris\.base-gbl\commands.toml", "commands.toml", None));
        assert!(place_hit(r"\\wsl.localhost\Ubuntu\home\u\.base-gbl\commands.toml", "commands.toml", None));
        assert!(!place_hit("/home/u/.base-gbl/commands.toml.bak", "commands.toml", None));
        assert!(place_hit("C:/Users/Chris/tools/ping-chat-hub/src/hub.py", "ping-chat-hub", None));
        assert!(!place_hit("C:/Users/Chris/tools/ping-chat-hub-old/hub.py", "ping-chat-hub", None), "not a substring");
        assert!(place_hit("C:/Users/Chris/Documents/renda-group/a.md", "Documents/renda-group", None));
        assert!(place_hit("/home/u/.base-gbl/lore/x.md", "~/.base-gbl/lore", Some("/home/u")));
        assert!(!place_hit("/home/v/.base-gbl/lore/x.md", "~/.base-gbl/lore", Some("/home/u")));
    }

    #[test]
    fn one_common_word_from_long_rule_text_cannot_fire_a_topic_rule() {
        let text = "Before any base write, resolve and state the tier the working directory lands in";
        let min = 0.75;
        let one_word = topic_score("which tier are we on", &[], text, &[]);
        assert!(one_word > 0.0, "control: the shared word is seen: {one_word}");
        assert!(one_word < min, "one text word must not reach the minimum: {one_word}");
        let many_words = topic_score("base write tier directory resolve state lands", &[], text, &[]);
        assert!(many_words <= TOPIC_TEXT_CAP && many_words < min, "text alone is capped: {many_words}");
        let own = topic_score("do we state the tier first?", &["state the tier".into()], text, &[]);
        assert!(own >= min, "the rule's own phrase fires it: {own}");
        let kw = topic_score("a base config question about the tier", &[], text, &["base config".into()]);
        assert!(kw >= min, "a domain keyword plus a text word fires it: {kw}");
        // The knock-out of the minimum (set `topic_min_score = 0` and the one-word case must fire) needs `select`,
        // which reads the minimum from config. It belongs in select's test, not here, where the minimum is a local.
    }

    #[test]
    fn matchers_fold_the_same_from_toml_flags_and_graph_literals() {
        let flags = matchers_from_flags(
            &["always".into()],
            &["commands.toml".into()],
            &[],
            &["base relay ping --to chris".into()],
            Some("ping chris, relay ping"),
        )
        .unwrap();
        let back = matchers_from_literals(matcher_literals(&flags).iter().map(|(p, v)| (*p, v.as_str())));
        assert_eq!(flags, back, "flags and graph read back identically");
        assert_eq!(flags.len(), 4, "place, command, one topic, always: {flags:?}");
        assert!(matchers_from_flags(&["place".into()], &[], &[], &[], None).is_err(), "a place kind needs a place");
        assert!(matchers_from_flags(&["action".into()], &[], &[], &[], None).is_err(), "an action kind needs a value");
        assert!(matchers_from_flags(&["nonsense".into()], &[], &[], &[], None).is_err());
        assert_eq!(matchers_from_flags(&["topic".into()], &[], &[], &[], None).unwrap(), vec![Matcher::for_topic(vec![])]);
    }
}
