//! C1, the prompt's wording, and C2's repeat check: both read only the prompt.
//!
//! C1 only flags. "don't" and "never" are in most people's ordinary prompts (on Chris's 1,074 later human prompts since
//! 2026-09-23 the default list flags 272), so a phrase is never a verdict: it asks the AI, in one line, whether the
//! prompt was a correction (C4), and the AI that read the exchange decides.

use sha2::{Digest, Sha256};

/// Two prompts each need this many content words before they can count as the same request: "continue" twice is not
/// a repeat.
pub const REPEAT_MIN_WORDS: usize = 4;

/// The prompt as C1 compares it: lowercased, a curly apostrophe read as a straight one (phones and speech-to-text
/// write `don’t`).
pub fn normalize(text: &str) -> String {
    text.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'")
}

/// Is `phrase` in `text` as whole words? `text` is already [`normalize`]d. A word boundary is checked only on a side
/// where the phrase has a letter or a digit: `no,` ends in a comma, which already ends the word.
pub fn phrase_hit(text: &str, phrase: &str) -> bool {
    let needle = normalize(phrase.trim());
    if needle.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let check_before = needle.chars().next().is_some_and(is_word);
    let check_after = needle.chars().next_back().is_some_and(is_word);
    let mut from = 0;
    while let Some(pos) = text[from..].find(&needle) {
        let at = from + pos;
        let end = at + needle.len();
        let before_ok = !check_before || text[..at].chars().next_back().is_none_or(|c| !is_word(c));
        let after_ok = !check_after || text[end..].chars().next().is_none_or(|c| !is_word(c));
        if before_ok && after_ok {
            return true;
        }
        // One character on, never one byte: a prompt with a multi-byte character must not split.
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// C1: the configured phrases `prompt` carries, each once, in the configured order.
pub fn matched(prompt: &str, phrases: &[String]) -> Vec<String> {
    let text = normalize(prompt);
    let mut out: Vec<String> = Vec::new();
    for p in phrases.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        if phrase_hit(&text, p) && !out.iter().any(|o| o.eq_ignore_ascii_case(p)) {
            out.push(p.to_string());
        }
    }
    out
}

fn hash64(text: &str) -> u64 {
    let d = Sha256::digest(text.as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    u64::from_be_bytes(b)
}

/// A prompt's content words (`rules::content_words`), each as a stable hash, sorted: what the repeat check keeps of a
/// prompt between two hook runs, so no prompt text is stored for it.
pub fn word_hashes(prompt: &str) -> Vec<u64> {
    let mut out: Vec<u64> = crate::domain::rules::content_words(prompt).iter().map(|w| hash64(w)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// A hash of a prompt's whole text, whitespace collapsed: how `base rule propose` knows the prompt it read from the
/// transcript is the one the prompt hook counted.
pub fn text_hash(prompt: &str) -> String {
    format!("{:016x}", hash64(&prompt.split_whitespace().collect::<Vec<_>>().join(" ")))
}

/// The share of content words two prompts have in common (Jaccard), or `None` when either has fewer than
/// [`REPEAT_MIN_WORDS`].
pub fn similarity(a: &[u64], b: &[u64]) -> Option<f32> {
    if a.len() < REPEAT_MIN_WORDS || b.len() < REPEAT_MIN_WORDS {
        return None;
    }
    let both = a.iter().filter(|h| b.binary_search(h).is_ok()).count();
    let either = a.len() + b.len() - both;
    Some(both as f32 / either as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Vec<String> {
        crate::config::CorrectionsConfig::default().phrases
    }

    #[test]
    fn whole_words_only_and_punctuation_ends_a_word() {
        let t = normalize("Quite right, quitting time, NO, that's wrong");
        assert!(!phrase_hit(&t, "quit"), "quite and quitting are not quit");
        assert!(phrase_hit(&t, "no,"));
        assert!(phrase_hit(&t, "wrong"));
        assert!(phrase_hit(&normalize("no,that"), "no,"), "a phrase ending in a comma needs no space after it");
        assert!(!phrase_hit(&normalize("piano, guitar"), "no,"), "the start of a phrase is still a word start");
    }

    #[test]
    fn curly_apostrophes_match() {
        assert_eq!(matched("Don\u{2019}t do that", &defaults()), ["don't"]);
        assert_eq!(matched("that\u{2019}s not what I asked", &defaults()), ["that's not", "not what i asked"]);
    }

    #[test]
    fn the_example_corrections_flag() {
        let one = "The installed version is not 0.15.2. Now, quit trying to correct me and rephrase everything.";
        assert_eq!(matched(one, &defaults()), ["quit"]);
        let two = "I've been saying this until I'm fucking blue in the face, that I don't want broad paths.";
        assert_eq!(matched(two, &defaults()), ["don't", "fucking"]);
        assert!(matched("use the other file", &defaults()).is_empty(), "Example 4: no phrase");
    }

    #[test]
    fn similarity_needs_four_words_each() {
        let a = word_hashes("close down all of the infra for this, off for the weekend");
        let b = word_hashes("close down all infra, off for weekend");
        assert!(similarity(&a, &b).is_some_and(|s| s >= 0.5), "{:?}", similarity(&a, &b));
        assert_eq!(similarity(&word_hashes("continue"), &word_hashes("continue")), None);
        let c = word_hashes("write the release notes for the dashboard");
        assert!(similarity(&a, &c).is_some_and(|s| s < 0.5));
        assert_eq!(text_hash("a  b\nc"), text_hash("a b c"));
    }
}
