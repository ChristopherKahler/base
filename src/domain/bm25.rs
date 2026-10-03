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
//!
//! TWO TOKENIZERS, ONE SCORER. [`words`] is what BO-15's `propose` counts, and its fit margin was measured on it.
//! [`terms`] is K7b's, for ranking rules against a prompt (BO-18): joined and split hyphenated terms, bigrams, a stopword
//! list and Porter stems, so "the prompt hook is cutting things off" meets a rule whose test prompt says "cut off".

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// How fast a word's count stops adding: 1.2, the usual value.
pub const K1: f32 = 1.2;
/// How much a long document is held back: 0.75, the usual value.
pub const B: f32 = 0.75;

/// Raised whenever [`terms`] or [`stem`] changes what they return, so an index counted with the old ones is rebuilt
/// rather than scored against a prompt split the new way (`score_index`).
pub const TERMS_VERSION: u32 = 1;

/// The words [`terms`] drops: short English function words, apostrophes taken out (`don't` is `dont`). A piece of one
/// character is dropped as well, so `a` and `i` need no entry.
pub const TERM_STOPWORDS: &[&str] = &[
    "about", "all", "also", "am", "an", "and", "any", "are", "as", "at", "be", "been", "but", "by", "can", "could", "did",
    "do", "does", "dont", "for", "from", "get", "got", "had", "has", "have", "he", "her", "here", "him", "his", "how",
    "if", "im", "in", "into", "is", "it", "its", "ive", "just", "me", "my", "no", "not", "now", "of", "on", "or", "our",
    "she", "so", "some", "than", "that", "thats", "the", "their", "them", "then", "there", "these", "they", "this",
    "those", "to", "too", "up", "us", "very", "was", "we", "were", "what", "when", "where", "which", "who", "why", "will",
    "with", "would", "you", "your", "youre",
];

/// K7b: the terms of a text as BM25 counts them for prompt scoring, repeats kept.
///
/// Lowercased and split on anything that is not a letter or a digit. Pieces joined by `-`, `_` or `.` with a letter or
/// digit on both sides (`pre-tool`, `base-gbl`, `domains.toml`, `0.16.0`) are also kept joined, as written; an
/// apostrophe between two letters joins with nothing (`don't` is `dont`). Each piece of two characters or more that is
/// not a stopword is stemmed ([`stem`]) and kept, and each pair of neighbouring kept pieces is kept as a bigram, `"a b"`:
/// `user prompt submit` gives `prompt submit`, and `pre-tool` gives `pre tool`, so a spaced and a hyphenated spelling
/// meet. A dropped piece or a mark that ends a phrase (`, ; : ! ? ( ) [ ] { } < > "` and quotes, a `.` that is not
/// inside a term, a line break) breaks the pairing, so no bigram spans a word that is not there.
pub fn terms(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut out: Vec<String> = Vec::new();
    // The last piece kept in the current phrase: the left half of the next bigram.
    let mut prev: Option<String> = None;
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_alphanumeric() {
            if ends_phrase(chars[i]) {
                prev = None;
            }
            i += 1;
            continue;
        }
        let mut parts: Vec<String> = vec![String::new()];
        let mut joined = String::new();
        while i < chars.len() {
            let c = chars[i];
            let next_alnum = chars.get(i + 1).is_some_and(|n| n.is_alphanumeric());
            if c.is_alphanumeric() {
                parts.last_mut().expect("one part at least").push(c);
                joined.push(c);
            } else if is_apostrophe(c)
                && chars.get(i + 1).is_some_and(|n| n.is_alphabetic())
                && i > 0
                && chars[i - 1].is_alphabetic()
            {
                // `don't`: the word goes on.
            } else if matches!(c, '-' | '_' | '.') && next_alnum {
                joined.push(c);
                parts.push(String::new());
            } else {
                break;
            }
            i += 1;
        }
        if parts.len() > 1 {
            out.push(joined);
        }
        for p in parts {
            if p.chars().count() < 2 || TERM_STOPWORDS.contains(&p.as_str()) {
                prev = None;
                continue;
            }
            let s = stem(&p);
            if let Some(a) = &prev {
                out.push(format!("{a} {s}"));
            }
            out.push(s.clone());
            prev = Some(s);
        }
    }
    out
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{2018}')
}

/// A mark that ends a phrase for [`terms`]: no bigram is made across it.
fn ends_phrase(c: char) -> bool {
    matches!(
        c,
        ',' | ';' | ':' | '!' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '"' | '`' | '.' | '\n' | '\r'
            | '\u{201c}' | '\u{201d}'
    ) || is_apostrophe(c)
}

/// Porter's stemmer (M. F. Porter, "An algorithm for suffix stripping", 1980), as his reference C implementation runs
/// it (`bli` to `ble` and `logi` to `log` in step 2). Written out here because K7 adds no dependency. A word of two
/// letters or fewer, or one holding anything other than `a` to `z`, is returned as it is: a version, a path piece or a
/// word in another script is matched exactly.
pub fn stem(word: &str) -> String {
    if word.len() <= 2 || !word.bytes().all(|b| b.is_ascii_lowercase()) {
        return word.to_string();
    }
    let mut w: Vec<u8> = word.as_bytes().to_vec();
    porter_1a(&mut w);
    porter_1b(&mut w);
    porter_1c(&mut w);
    porter_step(&mut w, STEP2, 0);
    porter_step(&mut w, STEP3, 0);
    porter_4(&mut w);
    porter_5(&mut w);
    String::from_utf8(w).unwrap_or_else(|_| word.to_string())
}

/// Is `w[i]` a consonant? A `y` is one at the start of a word or after a vowel.
fn cons(w: &[u8], i: usize) -> bool {
    match w[i] {
        b'a' | b'e' | b'i' | b'o' | b'u' => false,
        b'y' => i == 0 || !cons(w, i - 1),
        _ => true,
    }
}

/// Porter's m: the number of vowel-consonant sequences in `w`, as `[C](VC)^m[V]`.
fn measure(w: &[u8]) -> usize {
    let n = w.len();
    let mut i = 0;
    while i < n && cons(w, i) {
        i += 1;
    }
    let mut m = 0;
    loop {
        while i < n && !cons(w, i) {
            i += 1;
        }
        if i >= n {
            return m;
        }
        while i < n && cons(w, i) {
            i += 1;
        }
        m += 1;
    }
}

fn has_vowel(w: &[u8]) -> bool {
    (0..w.len()).any(|i| !cons(w, i))
}

/// Ends with a double consonant (`tt`, `ss`).
fn double_cons(w: &[u8]) -> bool {
    let n = w.len();
    n >= 2 && w[n - 1] == w[n - 2] && cons(w, n - 1)
}

/// Ends consonant, vowel, consonant, the last not `w`, `x` or `y` (`hop`, `wil`).
fn cvc(w: &[u8]) -> bool {
    let n = w.len();
    n >= 3 && cons(w, n - 3) && !cons(w, n - 2) && cons(w, n - 1) && !matches!(w[n - 1], b'w' | b'x' | b'y')
}

fn porter_1a(w: &mut Vec<u8>) {
    if w.ends_with(b"sses") || w.ends_with(b"ies") {
        w.truncate(w.len() - 2);
    } else if !w.ends_with(b"ss") && w.ends_with(b"s") {
        w.pop();
    }
}

fn porter_1b(w: &mut Vec<u8>) {
    if w.ends_with(b"eed") {
        if measure(&w[..w.len() - 3]) > 0 {
            w.pop();
        }
        return;
    }
    let cut = if w.ends_with(b"ed") && has_vowel(&w[..w.len() - 2]) {
        2
    } else if w.ends_with(b"ing") && has_vowel(&w[..w.len() - 3]) {
        3
    } else {
        return;
    };
    w.truncate(w.len() - cut);
    if w.ends_with(b"at") || w.ends_with(b"bl") || w.ends_with(b"iz") {
        w.push(b'e');
    } else if double_cons(w) && !matches!(w[w.len() - 1], b'l' | b's' | b'z') {
        w.pop();
    } else if measure(w) == 1 && cvc(w) {
        w.push(b'e');
    }
}

fn porter_1c(w: &mut [u8]) {
    let n = w.len();
    if w.ends_with(b"y") && has_vowel(&w[..n - 1]) {
        w[n - 1] = b'i';
    }
}

/// Step 2: (m > 0) suffix to replacement. Ordered as the reference implementation tests them, so the longer of two
/// suffixes that overlap is tried first; the first suffix that matches decides the step, applied or not.
const STEP2: &[(&str, &str)] = &[
    ("ational", "ate"), ("tional", "tion"), ("enci", "ence"), ("anci", "ance"), ("izer", "ize"), ("bli", "ble"),
    ("alli", "al"), ("entli", "ent"), ("eli", "e"), ("ousli", "ous"), ("ization", "ize"), ("ation", "ate"),
    ("ator", "ate"), ("alism", "al"), ("iveness", "ive"), ("fulness", "ful"), ("ousness", "ous"), ("aliti", "al"),
    ("iviti", "ive"), ("biliti", "ble"), ("logi", "log"),
];

/// Step 3: (m > 0) suffix to replacement.
const STEP3: &[(&str, &str)] = &[
    ("icate", "ic"), ("ative", ""), ("alize", "al"), ("iciti", "ic"), ("ical", "ic"), ("ful", ""), ("ness", ""),
];

/// Step 4: (m > 1) suffix removed; `ion` only after `s` or `t`.
const STEP4: &[&str] = &[
    "al", "ance", "ence", "er", "ic", "able", "ible", "ant", "ement", "ment", "ent", "ion", "ou", "ism", "ate", "iti",
    "ous", "ive", "ize",
];

fn porter_step(w: &mut Vec<u8>, rules: &[(&str, &str)], min_m: usize) {
    for (suffix, to) in rules {
        if w.ends_with(suffix.as_bytes()) {
            let stem = w.len() - suffix.len();
            if measure(&w[..stem]) > min_m {
                w.truncate(stem);
                w.extend_from_slice(to.as_bytes());
            }
            return;
        }
    }
}

fn porter_4(w: &mut Vec<u8>) {
    for suffix in STEP4 {
        if w.ends_with(suffix.as_bytes()) {
            let stem = w.len() - suffix.len();
            if *suffix == "ion" && !(stem > 0 && matches!(w[stem - 1], b's' | b't')) {
                // `ion` not after `s` or `t`: the reference implementation then tries `ou`, which cannot end the word.
                return;
            }
            if measure(&w[..stem]) > 1 {
                w.truncate(stem);
            }
            return;
        }
    }
}

fn porter_5(w: &mut Vec<u8>) {
    if w.ends_with(b"e") {
        let stem = &w[..w.len() - 1];
        let m = measure(stem);
        if m > 1 || (m == 1 && !cvc(stem)) {
            w.pop();
        }
    }
    if w.ends_with(b"ll") && measure(w) > 1 {
        w.pop();
    }
}

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

/// A set of documents, counted once, that any number of queries can be scored against. Serializable so the prompt
/// hook can load a counted corpus instead of counting it (`score_index`, K7e).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

    fn has(terms: &[String], t: &str) -> bool {
        terms.iter().any(|x| x == t)
    }

    /// K7b, the scope's own cases, then the edges: stopwords and one-character pieces dropped, bigrams never across a
    /// dropped word or a comma, stems on pieces and not on joined terms.
    #[test]
    fn bm25_tokenizer() {
        let t = terms("Our pre-tool hook is pretty good");
        for want in ["pre-tool", "pre", "tool", "pre tool", "hook", "pretti", "good", "tool hook"] {
            assert!(has(&t, want), "{want:?} missing from {t:?}");
        }
        assert!(!has(&t, "our") && !has(&t, "is"), "stopwords dropped: {t:?}");
        assert!(!has(&t, "hook pretti"), "no bigram across a dropped stopword: {t:?}");

        let t = terms("the user prompt submit being cut off");
        for want in ["user", "prompt", "submit", "user prompt", "prompt submit", "cut", "off", "cut off", "be"] {
            assert!(has(&t, want), "{want:?} missing from {t:?}");
        }
        assert!(!has(&t, "the"), "{t:?}");

        // Joined forms are kept as written; the pieces are stemmed.
        let t = terms("Edit domains.toml under ~/.base-gbl for 0.16.0, then the hooks.");
        for want in ["domains.toml", "domain", "toml", "base-gbl", "base", "gbl", "0.16.0", "16", "hook", "edit"] {
            assert!(has(&t, want), "{want:?} missing from {t:?}");
        }
        assert!(!has(&t, "0"), "a one-character piece is dropped: {t:?}");
        assert!(!has(&t, "0.16.0 hook") && !has(&t, "16 hook"), "a comma ends the phrase: {t:?}");

        // An apostrophe between letters joins; elsewhere it ends a phrase.
        let t = terms("don't touch the rule's text");
        assert!(!has(&t, "don") && !has(&t, "dont"), "dont is a stopword: {t:?}");
        assert!(has(&t, "rule") && !has(&t, "s"), "the rule's is one word: {t:?}");

        // Example 3's near miss meets its test prompt on stems.
        let prompt = terms("the prompt hook is cutting things off");
        let test = terms("being cut off at a high rate");
        for shared in ["cut", "off"] {
            assert!(has(&prompt, shared) && has(&test, shared), "{shared:?}: {prompt:?} / {test:?}");
        }
        assert!(!has(&prompt, "cut off"), "a word between them, so no bigram: {prompt:?}");
        assert_eq!(stem("injections"), "inject");
        assert_eq!(stem("injection"), stem("inject"), "K7 explained: injection meets inject");
        assert_eq!(stem("hooks"), "hook");
        assert_eq!(stem("cutting"), "cut");
    }

    /// Porter's stemmer against pairs from his paper's examples and his published vocabulary and output lists
    /// (tartarus.org/martin/PorterStemmer: voc.txt and output.txt), run through every step.
    #[test]
    fn porter_matches_the_published_pairs() {
        let pairs: &[(&str, &str)] = &[
            ("caresses", "caress"), ("ponies", "poni"), ("ties", "ti"), ("caress", "caress"), ("cats", "cat"),
            ("feed", "feed"), ("agreed", "agre"), ("plastered", "plaster"), ("bled", "bled"), ("motoring", "motor"),
            ("sing", "sing"), ("conflated", "conflat"), ("troubled", "troubl"), ("sized", "size"), ("hopping", "hop"),
            ("tanned", "tan"), ("falling", "fall"), ("hissing", "hiss"), ("fizzed", "fizz"), ("failing", "fail"),
            ("filing", "file"), ("happy", "happi"), ("sky", "sky"), ("relational", "relat"), ("conditional", "condit"),
            ("rational", "ration"), ("valenci", "valenc"), ("hesitanci", "hesit"), ("digitizer", "digit"),
            ("conformabli", "conform"), ("radicalli", "radic"), ("differentli", "differ"), ("vileli", "vile"),
            ("analogousli", "analog"), ("vietnamization", "vietnam"), ("predication", "predic"), ("operator", "oper"),
            ("feudalism", "feudal"), ("decisiveness", "decis"), ("hopefulness", "hope"), ("callousness", "callous"),
            ("formaliti", "formal"), ("sensitiviti", "sensit"), ("sensibiliti", "sensibl"), ("triplicate", "triplic"),
            ("formative", "form"), ("formalize", "formal"), ("electriciti", "electr"), ("electrical", "electr"),
            ("hopeful", "hope"), ("goodness", "good"), ("revival", "reviv"), ("allowance", "allow"),
            ("inference", "infer"), ("airliner", "airlin"), ("gyroscopic", "gyroscop"), ("adjustable", "adjust"),
            ("defensible", "defens"), ("irritant", "irrit"), ("replacement", "replac"), ("adjustment", "adjust"),
            ("dependent", "depend"), ("adoption", "adopt"), ("homologou", "homolog"), ("communism", "commun"),
            ("activate", "activ"), ("angulariti", "angular"), ("homologous", "homolog"), ("effective", "effect"),
            ("bowdlerize", "bowdler"), ("probate", "probat"), ("rate", "rate"), ("cease", "ceas"), ("controll", "control"),
            ("roll", "roll"), ("generalizations", "gener"), ("oscillators", "oscil"), ("abandoned", "abandon"),
            ("abase", "abas"), ("abated", "abat"), ("abatement", "abat"), ("abates", "abat"), ("abbess", "abbess"),
            ("abbeys", "abbei"), ("abbots", "abbot"), ("abbreviated", "abbrevi"),
        ];
        assert!(pairs.len() >= 40, "lynx's G0 verdict: at least 40 published pairs");
        let wrong: Vec<String> = pairs
            .iter()
            .filter(|(w, want)| stem(w) != *want)
            .map(|(w, want)| format!("{w}: {} (want {want})", stem(w)))
            .collect();
        assert!(wrong.is_empty(), "{} of {} pairs differ: {wrong:?}", wrong.len(), pairs.len());
        assert_eq!(stem("v2"), "v2", "a word holding a digit is kept as it is");
        assert_eq!(stem("über"), "über", "a word outside a to z is kept as it is");
        assert_eq!(stem("is"), "is", "two letters are kept as they are");
    }
}
