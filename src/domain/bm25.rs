//! BM25 (Okapi): how well a query's words fit each document of a set, with rare words counting more than common ones
//! and a long document not winning by length alone.
//!
//! WHY ITS OWN MODULE. BO-15's `base rule propose` asks which existing rule or decision a correction is about
//! (`corrections::propose`). BO-18 (K7, D9) ranks rules against prompts with BM25. One scorer serves both, so it is
//! not wired to the prompt matcher here: documents go in, ranked scores come out, and the caller decides what a
//! document is and what score is enough (lynx's G0 verdict on BO-15).
//!
//! THE FORMULA. For a query word `w` in document `d`:
//! `idf(w) * tf * (K1 + 1) / (tf + K1 * (1 - B + B * len(d) / avg_len))`, summed over the query's words, with
//! `idf(w) = ln(1 + (N - df + 0.5) / (df + 0.5))`, which is never negative. A word the query repeats counts once per
//! repeat, so a caller weights a word by repeating it.

use std::collections::HashMap;

/// How fast a word's count stops adding: 1.2, the usual value.
pub const K1: f32 = 1.2;
/// How much a long document is held back: 0.75, the usual value.
pub const B: f32 = 0.75;

/// The words of a text as BM25 counts them, repeats kept (a word's count in a document is its term frequency):
/// lowercased; split on anything that is not a letter, a digit, `-`, `_` or `.`; `-`, `_` and `.` trimmed from both
/// ends; kept when three characters or longer, or when it holds a digit, so a version such as `0.16` stays one word;
/// the rule matcher's stopwords removed.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.'))
        .map(|w| w.trim_matches(['-', '_', '.']).to_lowercase())
        .filter(|w| !w.is_empty())
        .filter(|w| w.chars().count() >= 3 || w.chars().any(|c| c.is_ascii_digit()))
        .filter(|w| !crate::domain::rules::STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// One document's place in a ranking.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    /// The document's index in the order the documents were given.
    pub doc: usize,
    pub score: f32,
    /// The query's words the document holds, each once, in query order.
    pub matched: Vec<String>,
}

/// A set of documents, counted once, that any number of queries can be scored against.
#[derive(Debug, Clone, Default)]
pub struct Corpus {
    /// Each document's word counts.
    docs: Vec<HashMap<String, u32>>,
    /// Each document's length in words.
    lens: Vec<u32>,
    /// How many documents hold each word.
    df: HashMap<String, u32>,
    avg_len: f32,
}

impl Corpus {
    /// Count `docs`, each given as its words (see [`words`]).
    pub fn new(docs: impl IntoIterator<Item = Vec<String>>) -> Self {
        let mut corpus = Corpus::default();
        for doc in docs {
            let mut counts: HashMap<String, u32> = HashMap::new();
            for w in &doc {
                *counts.entry(w.clone()).or_default() += 1;
            }
            for w in counts.keys() {
                *corpus.df.entry(w.clone()).or_default() += 1;
            }
            corpus.lens.push(doc.len() as u32);
            corpus.docs.push(counts);
        }
        let total: u64 = corpus.lens.iter().map(|l| u64::from(*l)).sum();
        corpus.avg_len = if corpus.docs.is_empty() { 0.0 } else { total as f32 / corpus.docs.len() as f32 };
        corpus
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// How rare `word` is across the documents: high for a word few documents hold, near zero for one most hold.
    pub fn idf(&self, word: &str) -> f32 {
        let n = self.docs.len() as f32;
        let df = self.df.get(word).copied().unwrap_or(0) as f32;
        (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
    }

    /// Document `doc`'s score for `query` (its words, see [`words`]), and the query words it holds.
    pub fn score(&self, doc: usize, query: &[String]) -> Ranked {
        let mut score = 0.0f32;
        let mut matched: Vec<String> = Vec::new();
        let (Some(counts), Some(len)) = (self.docs.get(doc), self.lens.get(doc)) else {
            return Ranked { doc, score, matched };
        };
        let norm = if self.avg_len > 0.0 { *len as f32 / self.avg_len } else { 1.0 };
        for w in query {
            let Some(tf) = counts.get(w).map(|t| *t as f32) else { continue };
            score += self.idf(w) * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * norm));
            if !matched.contains(w) {
                matched.push(w.clone());
            }
        }
        Ranked { doc, score, matched }
    }

    /// Every document that holds at least one query word, best first; equal scores keep the documents' order.
    pub fn rank(&self, query: &[String]) -> Vec<Ranked> {
        let mut out: Vec<Ranked> = (0..self.docs.len())
            .map(|d| self.score(d, query))
            .filter(|r| r.score > 0.0)
            .collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.doc.cmp(&b.doc)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(docs: &[&str]) -> Corpus {
        Corpus::new(docs.iter().map(|d| words(d)))
    }

    #[test]
    fn words_keep_versions_and_drop_stopwords() {
        assert_eq!(words("The installed version is 0.15.2, not 0.16."), ["installed", "version", "0.15.2", "0.16"]);
        assert_eq!(words("auto_inject = false; path-trigger"), ["auto_inject", "false", "path-trigger"]);
    }

    #[test]
    fn a_rare_word_outranks_a_common_one() {
        let c = corpus(&[
            "path triggers name the project folder",
            "the project keeps its rules",
            "the project keeps its decisions",
            "the project keeps its notes",
        ]);
        let ranked = c.rank(&words("project path"));
        assert_eq!(ranked[0].doc, 0, "the one document with the rare word comes first: {ranked:?}");
        assert_eq!(ranked[0].matched, ["project", "path"]);
        assert!(c.idf("path") > c.idf("project"));
    }

    #[test]
    fn a_long_document_does_not_win_by_length() {
        let long = format!("relay ping {}", "filler words here ".repeat(40));
        let c = corpus(&["relay ping", &long, "something else"]);
        let ranked = c.rank(&words("relay ping"));
        assert_eq!(ranked[0].doc, 0, "{ranked:?}");
        assert_eq!(ranked.len(), 2, "a document with no query word is not ranked");
    }

    #[test]
    fn an_empty_corpus_ranks_nothing() {
        let c = Corpus::new(Vec::<Vec<String>>::new());
        assert!(c.rank(&words("anything")).is_empty());
        assert_eq!(c.score(3, &words("anything")).score, 0.0);
    }
}
