//! The BM25 index the prompt hook ranks rules and decisions by (K7, D9, BO-18).
//!
//! WHY. A domain's rules were served when one of its keywords was in the prompt, all of them, all equal: one word pulled
//! in a whole domain, a near miss pulled in nothing, and a tight budget had no way to choose. Each rule and each global
//! decision a prompt can be served now has a scoring text (K7a), counted once here, and the prompt hook scores every one
//! of them against the prompt (K7c): rules no keyword brought are admitted on their score, and every block lists its
//! rules best first, so the budget sheds the weakest first (K7d).
//!
//! THE SCORING TEXT (K7a). A rule: its text and rationale, its domain's `prompt_keywords`, its own topic words when it
//! has matchers of its own, and its `fires_on` test prompts (BO-14). A global decision with keywords: its name and its
//! keywords. `quiet_on` prompts never; a global decision with no keywords is no document, since only session start
//! serves it (F5).
//!
//! THE CACHE (K7e). The index is counted at sync time (session start, `base domain sync`, `base sync`, and every
//! command that changes prompt matching) and written to `<tier>/.base/bm25-index.json`, keyed by a SHA-256 of every
//! scoring text: a refresh whose texts are unchanged writes nothing. The graph files cannot key it: the tool hook
//! rewrites them on every call that touches a project (`lastActive`). The prompt hook only loads it, never counts it;
//! with no file the prompt is served keyword-only, exactly as before this module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::BaseConfig;
use crate::domain::bm25;
use crate::domain::rules::{Converted, ServedRule, StoredTests};
use crate::domain::DomainDef;

/// The index file, in a tier's `.base`.
pub const FILE: &str = "bm25-index.json";

/// Raised when the file's shape changes, so an old file is ignored and rebuilt rather than misread.
const FORMAT: u32 = 1;

/// What a document is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    Rule,
    Decision,
}

/// A document of the index: a rule by its stable id (`rules::rule_id`), a global decision by its `<iri>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRef {
    pub kind: DocKind,
    pub id: String,
    pub domain: String,
}

/// One document before it is counted, its scoring text kept in its parts so a replay can change one part (a domain's
/// keywords) and a rule test can hold one prompt out (lynx's G0 Q5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub doc: DocRef,
    /// A rule's text and rationale; a decision's name.
    pub wording: Vec<String>,
    /// A rule's domain's `prompt_keywords`; a decision's own keywords.
    pub keywords: Vec<String>,
    /// A rule's own topic words, when it carries matchers of its own.
    pub own_words: Vec<String>,
    /// A rule's `fires_on` test prompts.
    pub fires_on: Vec<String>,
}

impl Source {
    fn terms(&self, hold_out: Option<&str>) -> Vec<String> {
        let held = |p: &String| hold_out.is_some_and(|h| h.trim() == p.trim());
        self.wording
            .iter()
            .chain(&self.keywords)
            .chain(&self.own_words)
            .chain(self.fires_on.iter().filter(|p| !held(p)))
            .flat_map(|t| bm25::terms(t))
            .collect()
    }
}

/// The counted documents, as the prompt hook loads them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScoreIndex {
    pub format: u32,
    pub terms_version: u32,
    /// The SHA-256 of every scoring text ([`inputs_hash`]): what the cache is keyed by.
    pub inputs: String,
    pub docs: Vec<DocRef>,
    corpus: bm25::Corpus,
}

/// A prompt's scores: every document that shares a term with it, by id.
#[derive(Debug, Clone, Default)]
pub struct Scores {
    by_id: HashMap<String, Scored>,
    /// The documents in score order, best first.
    pub ranked: Vec<Scored>,
}

/// One document's score for one prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub doc: DocRef,
    pub score: f32,
    /// The prompt's terms the document holds, in the prompt's order.
    pub terms: Vec<String>,
}

impl Scores {
    /// The score of `id`, 0 when it shares no term with the prompt or is not in the index.
    pub fn get(&self, id: &str) -> f32 {
        self.by_id.get(id).map_or(0.0, |s| s.score)
    }

    pub fn scored(&self, id: &str) -> Option<&Scored> {
        self.by_id.get(id)
    }

    pub fn is_empty(&self) -> bool {
        self.ranked.is_empty()
    }
}

/// A prompt's terms as the index scores them: [`bm25::terms`], each once (lynx's G0 Q8), so a long prompt that repeats a
/// word does not weigh it once per repeat.
pub fn query_terms(prompt: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    bm25::terms(prompt).into_iter().filter(|t| seen.insert(t.clone())).collect()
}

/// Scoring texts split into terms once, to be counted many times with one `fires_on` prompt held out each time
/// (`base rule test` judges every `fires_on` prompt so): only the held-out rule's text is split again.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// The sources in the index's order: by kind, then id.
    sources: Vec<Source>,
    terms: Vec<Vec<String>>,
    inputs: String,
}

impl Prepared {
    pub fn new(sources: &[Source]) -> Self {
        let mut sorted: Vec<Source> = sources.to_vec();
        sorted.sort_by(|a, b| (a.doc.kind, &a.doc.id).cmp(&(b.doc.kind, &b.doc.id)));
        let terms = sorted.iter().map(|s| s.terms(None)).collect();
        Prepared { inputs: inputs_hash(&sorted), sources: sorted, terms }
    }

    /// The index of every source.
    pub fn build(&self) -> ScoreIndex {
        self.build_holding_out(None)
    }

    /// The index with one `fires_on` prompt left out of one rule's text: `(rule id, prompt)`. A rule test judges a
    /// `fires_on` prompt this way, or the prompt would pass by matching itself (lynx's G0 Q5).
    pub fn build_holding_out(&self, hold_out: Option<(&str, &str)>) -> ScoreIndex {
        let corpus = bm25::Corpus::new(self.sources.iter().zip(&self.terms).map(|(s, t)| match hold_out {
            Some((id, prompt)) if id == s.doc.id => s.terms(Some(prompt)),
            _ => t.clone(),
        }));
        ScoreIndex {
            format: FORMAT,
            terms_version: bm25::TERMS_VERSION,
            inputs: self.inputs.clone(),
            docs: self.sources.iter().map(|s| s.doc.clone()).collect(),
            corpus,
        }
    }
}

impl ScoreIndex {
    /// Count `sources`.
    pub fn build(sources: &[Source]) -> Self {
        Prepared::new(sources).build()
    }

    /// Count `sources` with one `fires_on` prompt held out of one rule's text ([`Prepared::build_holding_out`]).
    pub fn build_holding_out(sources: &[Source], hold_out: Option<(&str, &str)>) -> Self {
        Prepared::new(sources).build_holding_out(hold_out)
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Every document's score for `prompt` (BM25, K1 1.2, B 0.75: `bm25::Corpus`), best first.
    pub fn scores(&self, prompt: &str) -> Scores {
        let query = query_terms(prompt);
        let mut out = Scores::default();
        for r in self.corpus.rank(&query) {
            let Some(doc) = self.docs.get(r.doc) else { continue };
            let s = Scored { doc: doc.clone(), score: r.score, terms: r.matched };
            out.by_id.insert(doc.id.clone(), s.clone());
            out.ranked.push(s);
        }
        out
    }

    /// The index in `base_dir`, or `None` when there is none, it does not parse, or it was counted by another format or
    /// another tokenizer: the caller then serves keyword-only.
    pub fn load(base_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(base_dir.join(FILE)).ok()?;
        let index: ScoreIndex = serde_json::from_str(&text).ok()?;
        (index.format == FORMAT && index.terms_version == bm25::TERMS_VERSION).then_some(index)
    }

    /// Write the index into `base_dir` through a temp file and a rename, retried while another process holds the file
    /// (`store::rename_with_retry`: two sessions can refresh at once).
    pub fn save(&self, base_dir: &Path) -> std::io::Result<()> {
        let path = base_dir.join(FILE);
        let tmp = base_dir.join(format!("{FILE}.tmp-{}", std::process::id()));
        let json = serde_json::to_string(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, json)?;
        crate::store::rename_with_retry(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

/// The content hash the cache is keyed by: SHA-256 over the format, the tokenizer's version and every source (kind, id,
/// domain and each part of its scoring text), sorted by kind and id, as hex.
pub fn inputs_hash(sources: &[Source]) -> String {
    let mut sorted: Vec<&Source> = sources.iter().collect();
    sorted.sort_by(|a, b| (a.doc.kind, &a.doc.id).cmp(&(b.doc.kind, &b.doc.id)));
    let mut h = Sha256::new();
    h.update(format!("format {FORMAT} terms {}\u{1e}", bm25::TERMS_VERSION).as_bytes());
    for s in sorted {
        let kind = match s.doc.kind {
            DocKind::Rule => "rule",
            DocKind::Decision => "decision",
        };
        h.update(format!("{kind}\u{1f}{}\u{1f}{}\u{1f}", s.doc.id, s.doc.domain).as_bytes());
        for (label, part) in [("w", &s.wording), ("k", &s.keywords), ("o", &s.own_words), ("f", &s.fires_on)] {
            for t in part {
                h.update(format!("{label}\u{1f}{t}\u{1f}").as_bytes());
            }
        }
        h.update(b"\x1e");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// One rule's scoring text (K7a): its text and rationale, its domain's `prompt_keywords`, its own topic words, and its
/// `fires_on` prompts from `tests`. The one recipe: the prompt hook's index and `base rule test` both count this.
pub fn rule_source(rule: &ServedRule, keywords: &[String], own_words: Vec<String>, tests: &HashMap<String, StoredTests>) -> Source {
    let mut wording = vec![rule.text.clone()];
    wording.extend(rule.rationale.clone());
    Source {
        doc: DocRef { kind: DocKind::Rule, id: rule.id.clone(), domain: rule.domain.clone() },
        wording,
        keywords: keywords.to_vec(),
        own_words,
        fires_on: tests.get(&rule.id).map(|t| t.tests.fires_on.clone()).unwrap_or_default(),
    }
}

/// A rule's own topic words, each once, in the order its topic matchers hold them.
pub fn own_words(c: &Converted) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in c.matchers.iter().filter(|m| m.kind == crate::domain::rules::Kind::Topic).flat_map(|m| &m.words) {
        if !out.contains(w) {
            out.push(w.clone());
        }
    }
    out
}

/// Every document a prompt can be served, with its scoring text (K7a): each domain's rules as the prompt hook reads
/// them (`rules::rules_for_domain`, superseded and empty ones gone), the rules that carry matchers of their own
/// (`rules::rules_with_matchers`), each rule's `fires_on` (`rules::rule_tests`), and the global decisions that have
/// keywords (`GlobalDecisions`).
pub fn sources(store: Option<&oxigraph::store::Store>, config: &BaseConfig, domains: &[DomainDef]) -> Vec<Source> {
    let tests = crate::domain::rules::rule_tests(store, config, domains);
    let lists: Vec<Vec<ServedRule>> = domains.iter().map(|d| crate::domain::rules::rules_for_domain(store, config, d)).collect();
    let converted = crate::domain::rules::rules_with_matchers(store, config, domains);
    sources_from(store, config, domains, &lists, &converted, &tests)
}

/// [`sources`] from what a caller has read already: each domain's rules (`rules_for_domain`, one list per domain),
/// the rules with matchers and the stored tests; `store` is read only for the global decisions.
pub fn sources_from(
    store: Option<&oxigraph::store::Store>,
    config: &BaseConfig,
    domains: &[DomainDef],
    lists: &[Vec<ServedRule>],
    converted: &[Converted],
    tests: &HashMap<String, StoredTests>,
) -> Vec<Source> {
    let keywords: HashMap<&str, &[String]> = domains.iter().map(|d| (d.name.as_str(), d.prompt_keywords.as_slice())).collect();
    let keywords_of = |domain: &str| keywords.get(domain).copied().unwrap_or_default();
    let mut out: Vec<Source> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for r in lists.iter().flatten() {
        at.entry(r.id.clone()).or_insert_with(|| {
            out.push(rule_source(r, keywords_of(&r.domain), Vec::new(), tests));
            out.len() - 1
        });
    }
    for c in converted {
        match at.get(&c.rule.id) {
            Some(&i) => out[i].own_words = own_words(c),
            None => {
                at.insert(c.rule.id.clone(), out.len());
                out.push(rule_source(&c.rule, keywords_of(&c.rule.domain), own_words(c), tests));
            }
        }
    }
    if let Some(store) = store {
        let global = crate::domain::global_decisions::GlobalDecisions::load(store, config, domains);
        let mut decisions: Vec<&crate::domain::global_decisions::GlobalDecision> =
            global.all().filter(|d| !d.keywords.is_empty()).collect();
        decisions.sort_by(|a, b| a.id.cmp(&b.id));
        for d in decisions {
            out.push(Source {
                doc: DocRef { kind: DocKind::Decision, id: d.id.clone(), domain: d.domain.clone() },
                wording: vec![d.name.clone()],
                keywords: d.keywords.clone(),
                own_words: Vec::new(),
                fires_on: Vec::new(),
            });
        }
    }
    out
}

/// Can a prompt serve `domain`'s rules on their score alone (K7d): a domain of `domains.toml` with `auto_inject`, not
/// always-on, and not vetoed by one of its `exclude` patterns in the prompt (a substring, as the keyword matcher has it).
/// The prompt hook and `base rule test` both ask this.
pub fn admits_by_score(domain: &DomainDef, prompt_lower: &str) -> bool {
    domain.auto_inject && !domain.is_always() && !domain.exclude.iter().any(|p| prompt_lower.contains(&p.to_lowercase()))
}

/// Where the prompt hook looks for the index from `cwd`: the workspace's `.base`, else the global tier's (the same
/// resolution as the hook's session file).
pub fn index_dir(cwd: &Path) -> Option<PathBuf> {
    crate::config::find_workspace_base(cwd)
        .or_else(|| crate::home::home_root().map(|h| h.join(".base-gbl").join(".base")).filter(|p| p.is_dir()))
}

/// What a refresh did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refreshed {
    /// The scoring texts hash as the file says: nothing written.
    Unchanged,
    /// Counted and written, this many documents.
    Written(usize),
}

/// Count the index for `store` and write it into `base_dir`, unless the file there was counted from the same texts.
pub fn refresh(
    store: Option<&oxigraph::store::Store>,
    config: &BaseConfig,
    domains: &[DomainDef],
    base_dir: &Path,
) -> std::io::Result<Refreshed> {
    let sources = sources(store, config, domains);
    let inputs = inputs_hash(&sources);
    if ScoreIndex::load(base_dir).is_some_and(|old| old.inputs == inputs) {
        return Ok(Refreshed::Unchanged);
    }
    let index = ScoreIndex::build(&sources);
    index.save(base_dir)?;
    Ok(Refreshed::Written(index.len()))
}

/// [`refresh`] from `cwd`: its domains, its merged store and its index folder. For the commands that change prompt
/// matching; best-effort, so a failure is one line on stderr and never fails the command that changed the rule.
pub fn refresh_for(config: &BaseConfig, cwd: &Path) {
    if !config.matching.bm25 {
        return;
    }
    let Some(dir) = index_dir(cwd) else { return };
    // No domain sync here: a command that only reads (a `doctor --fix` plan, a review listing) must write nothing but
    // this file. Keywords are read from `domains.toml` itself, and a domain whose graph holds no rules yet is read from
    // the file too (`rules::rules_for_domain`), so what this reads is current.
    let domains = crate::domain::load_domains(cwd);
    let store = crate::store::load_merged(cwd);
    if let Err(why) = refresh(store.as_ref(), config, &domains, &dir) {
        eprintln!("base: could not refresh the rule index {}: {why}", dir.join(FILE).display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(domain: &str, text: &str, keywords: &[&str], fires_on: &[&str]) -> Source {
        Source {
            doc: DocRef { kind: DocKind::Rule, id: crate::domain::rules::rule_id(domain, text), domain: domain.into() },
            wording: vec![text.into()],
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            own_words: Vec::new(),
            fires_on: fires_on.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn decision(id: &str, name: &str, keywords: &[&str]) -> Source {
        Source {
            doc: DocRef { kind: DocKind::Decision, id: format!("<http://x/decision/{id}>"), domain: "GLOBAL".into() },
            wording: vec![name.into()],
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            own_words: Vec::new(),
            fires_on: Vec::new(),
        }
    }

    /// Example 2: a word most documents hold adds little, a word one document holds adds a lot (BM25's IDF), so a
    /// prompt sharing the rare word ranks that document above one sharing only the common word.
    #[test]
    fn bm25_rare_terms_weigh_more() {
        let sources = vec![
            rule("tools", "The base graph keeps one record per decision.", &[], &[]),
            rule("tools", "The base workspace tier holds the session file.", &[], &[]),
            rule("tools", "The base relay names each session by its title.", &[], &[]),
            rule("tools", "The base sync reads every markdown file.", &[], &[]),
            rule("tools", "An injection over the budget is named, never cut.", &[], &[]),
            rule("tools", "The base doctor names a broken trigger.", &[], &[]),
        ];
        let index = ScoreIndex::build(&sources);
        assert!(index.corpus.idf(&bm25::stem("injection")) > 2.0 * index.corpus.idf("base"), "rare outweighs common");
        let scores = index.scores("what does base do with an injection");
        assert_eq!(scores.ranked[0].doc.id, sources[4].doc.id, "the one document with the rare word first: {:?}", scores.ranked);
        let base_only = scores.get(&sources[0].doc.id);
        assert!(scores.ranked[0].score > 2.0 * base_only, "{} against {base_only}", scores.ranked[0].score);
    }

    /// Examples 1 and 4, in invented words of the same shape: a prompt about the prompt-submit hook losing what it
    /// injects ranks the hook rule (its text, its domain's keywords and its test prompts) first and over a `min_score`
    /// of 6, and two unrelated global decisions about other tools score under it.
    #[test]
    fn bm25_ranks_hook_rule_above_profile_decision_for_prompt_3() {
        let hook = rule(
            "tools",
            "The prompt hook trims its output block by block and names every block it withheld.",
            &["hook", "hooks", "session start", "prompt submit", "user prompt submit", "pre-tool", "injection"],
            &["output from the prompt submit step arrives truncated", "the hook drops whole blocks of injected text"],
        );
        let sources = vec![
            hook.clone(),
            rule("tools", "Name the folder a file is written to.", &["folder"], &[]),
            rule("garden", "Water the tomatoes before noon.", &["garden"], &[]),
            decision("invoices", "Invoices are exported as spreadsheet files every Friday afternoon.", &["invoice export"]),
            decision("staging", "The staging database is reset to its seed every night.", &["staging database"]),
        ];
        let index = ScoreIndex::build(&sources);
        let prompt = "lately the prompt submit hook truncates much of the injected text, whole blocks gone; session start \
                      output arrives complete and so does pre-tool";
        let scores = index.scores(prompt);
        assert_eq!(scores.ranked.first().map(|s| s.doc.id.as_str()), Some(hook.doc.id.as_str()), "{:?}", scores.ranked);
        // An explicit threshold: `[match] min_score` has no default, so by default no score serves a rule (Q7 ruling).
        let min = 6.0;
        assert!(scores.get(&hook.doc.id) >= min, "the hook rule passes min_score {min}: {}", scores.get(&hook.doc.id));
        for d in &sources[3..] {
            assert!(scores.get(&d.doc.id) < min, "{} under min_score: {}", d.doc.id, scores.get(&d.doc.id));
            assert!(scores.get(&d.doc.id) < scores.get(&hook.doc.id));
        }
    }

    #[test]
    fn the_hash_follows_every_part_of_the_text_and_not_the_order() {
        let a = rule("tools", "Name the folder you write to.", &["folder"], &["where did that file go"]);
        let b = rule("tools", "Say which tier a write lands in.", &["tier"], &[]);
        let base = inputs_hash(&[a.clone(), b.clone()]);
        assert_eq!(base, inputs_hash(&[b.clone(), a.clone()]), "order does not change the key");
        let mut c = a.clone();
        c.fires_on.push("one more test prompt".into());
        assert_ne!(base, inputs_hash(&[c, b.clone()]), "a new test prompt changes it");
        let mut d = a.clone();
        d.keywords.push("directory".into());
        assert_ne!(base, inputs_hash(&[d, b]), "a new keyword changes it");
    }

    #[test]
    fn a_held_out_prompt_does_not_score_its_own_rule() {
        let r = rule("tools", "Name the folder you write to.", &[], &["the zebra quartz file went missing"]);
        let other = rule("tools", "Say which tier a write lands in.", &[], &[]);
        let all = ScoreIndex::build(&[r.clone(), other.clone()]);
        assert!(all.scores("zebra quartz").get(&r.doc.id) > 0.0, "control: the test prompt is in the text");
        let held = ScoreIndex::build_holding_out(&[r.clone(), other.clone()], Some((&r.doc.id, "the zebra quartz file went missing")));
        assert_eq!(held.scores("zebra quartz").get(&r.doc.id), 0.0, "held out, it scores nothing");

        // `Prepared` splits every text once and splits again only the held-out rule's: the index it counts is the one a
        // fresh count of the texts without that prompt gives, score for score.
        let second = rule("tools", "Name the folder you write to.", &[], &["the zebra quartz file went missing", "which folder holds the notes"]);
        let prepared = Prepared::new(&[second.clone(), other.clone()]);
        let mut without = second.clone();
        without.fires_on.remove(0);
        let fresh = ScoreIndex::build(&[without, other.clone()]);
        for prompt in ["zebra quartz", "which folder holds the notes", "write the tier"] {
            let got = prepared.build_holding_out(Some((&r.doc.id, "the zebra quartz file went missing"))).scores(prompt);
            let want = fresh.scores(prompt);
            for id in [&r.doc.id, &other.doc.id] {
                assert_eq!(got.get(id), want.get(id), "{prompt:?} {id}");
            }
        }
        assert_eq!(prepared.build().inputs, ScoreIndex::build(&[second, other]).inputs, "the same key as a direct count");
    }

    #[test]
    fn a_saved_index_loads_back_and_an_unchanged_refresh_keeps_the_file() {
        let dir = std::env::temp_dir().join(format!("base-score-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sources = vec![rule("tools", "Name the folder you write to.", &["folder"], &[])];
        let index = ScoreIndex::build(&sources);
        index.save(&dir).unwrap();
        let back = ScoreIndex::load(&dir).expect("it loads");
        assert_eq!(back.inputs, index.inputs);
        assert!(back.scores("which folder").get(&sources[0].doc.id) > 0.0);
        let mut stale = back.clone();
        stale.terms_version += 1;
        stale.save(&dir).unwrap();
        assert!(ScoreIndex::load(&dir).is_none(), "another tokenizer's index is not used");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
