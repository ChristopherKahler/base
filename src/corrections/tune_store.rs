//! The rule pass's store check (BO-17, K4a, G0 question 2): patterns no single correction shows. Once a day, or with
//! `base tune --store`, one Haiku call judges three kinds of candidate the match log points at:
//!
//! - **merge**: two rules served together on 5 or more rows whose words overlap (Jaccard 0.4 or more): do they say the
//!   same thing, and what is the one rule that says both;
//! - **split**: a rule served 3 or more times, two or more sentences and 240 or more characters: is it two instructions;
//! - **drop keyword**: a domain keyword that brought its domain on 3 or more recent prompts: which of those prompts were
//!   about the domain, and what narrower words would still bring it on those (BO-14's `hook` and `session start`).
//!
//! **Retire** needs no call: a rule no prompt or file row served while the match log spans 30 days (K8's "dead after 30
//! days"). Until the log is that long, nothing is proposed for retirement, and the check says so.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::propose;
use super::tune;
use super::tune_pass::{keep_cached, parse_json, plural, quote, write_proposal, Args, Calls, StoreCheck, Written};
use crate::config::BaseConfig;
use crate::domain::rule_test::{short_ref, Bench, RuleRef};
use crate::emit::match_log::{self, Row};

/// The store check runs once in this long.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// A rule is dead when no row of a match log this many days long served it (K8).
pub const RETIRE_DAYS: i64 = 30;
const MERGE_MIN_TOGETHER: usize = 5;
const MERGE_MIN_OVERLAP: f32 = 0.4;
const SPLIT_MIN_SERVED: usize = 3;
const SPLIT_MIN_CHARS: usize = 240;
const KEYWORD_MIN_PROMPTS: usize = 3;
const TOP_MERGE: usize = 5;
const TOP_SPLIT: usize = 5;
const TOP_KEYWORDS: usize = 8;
const KEYWORD_EXAMPLES: usize = 6;
const TOP_RETIRE: usize = 10;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Last {
    at: String,
}

fn last_path() -> Option<std::path::PathBuf> {
    tune::own_dir().map(|d| d.join("store.json"))
}

/// The store check is due: never run, or run more than a day ago.
fn due() -> bool {
    let Some(at) = last_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<Last>(&t).ok())
    else {
        return true;
    };
    chrono::DateTime::parse_from_rfc3339(&at.at)
        .map(|t| (chrono::Local::now().timestamp() - t.timestamp()) as u64 > EVERY.as_secs())
        .unwrap_or(true)
}

/// The rows the check reads: the last `[tune] replay_prompts` prompts' worth of prompt and file rows, both tiers.
fn rows(config: &BaseConfig, cwd: &Path) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    for dir in propose::row_dirs(cwd) {
        out.extend(
            match_log::last_rows(&dir, config.tune.replay_prompts * 8, &match_log::Filter::default())
                .unwrap_or_default()
                .into_iter()
                .filter(|r| r.event == "prompt" || r.event == "file"),
        );
    }
    out
}

fn is_notification(text: &str) -> bool {
    text.trim_start().starts_with("<task-notification>")
}

fn words(text: &str) -> HashSet<String> {
    crate::domain::bm25::words(text).into_iter().collect()
}

fn overlap(a: &str, b: &str) -> f32 {
    let (a, b) = (words(a), words(b));
    let union = a.union(&b).count();
    if union == 0 { 0.0 } else { a.intersection(&b).count() as f32 / union as f32 }
}

fn sentences(text: &str) -> usize {
    text.split(['.', ';', '!', '?']).filter(|s| s.split_whitespace().count() >= 3).count()
}

/// The candidates, read from the log.
#[derive(Debug, Default)]
struct Candidates {
    merge: Vec<(RuleRef, RuleRef, usize, f32)>,
    split: Vec<(RuleRef, usize)>,
    keywords: Vec<(String, String, usize, Vec<String>)>,
    /// The prompts the window held, for the evidence lines.
    prompts: usize,
}

fn candidates(bench: &Bench<'_>, rows: &[Row]) -> Candidates {
    let rules: HashMap<String, RuleRef> = bench.rules().into_iter().map(|r| (r.id.clone(), r)).collect();
    let mut served: HashMap<String, usize> = HashMap::new();
    let mut together: HashMap<(String, String), usize> = HashMap::new();
    let mut kw: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let always: HashSet<String> = bench.domains.iter().filter(|d| d.is_always()).map(|d| d.name.clone()).collect();
    let mut prompts = 0;
    for r in rows {
        let mut ids: Vec<&str> = r.served.iter().filter(|i| i.kind == "rule").map(|i| i.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        for id in &ids {
            *served.entry(id.to_string()).or_default() += 1;
        }
        for (n, a) in ids.iter().enumerate() {
            for b in &ids[n + 1..] {
                *together.entry((a.to_string(), b.to_string())).or_default() += 1;
            }
        }
        if r.event == "prompt"
            && let Some(text) = r.text.as_deref().filter(|t| !is_notification(t))
        {
            prompts += 1;
            for m in r.matched.iter().filter(|m| m.by == "keyword" && !always.contains(&m.domain)) {
                if let Some(v) = &m.value {
                    let list = kw.entry((m.domain.clone(), v.clone())).or_default();
                    if !list.iter().any(|t| t == text) {
                        list.push(text.to_string());
                    }
                }
            }
        }
    }
    let mut merge: Vec<(RuleRef, RuleRef, usize, f32)> = together
        .into_iter()
        .filter(|(_, n)| *n >= MERGE_MIN_TOGETHER)
        .filter_map(|((a, b), n)| {
            let (ra, rb) = (rules.get(&a)?, rules.get(&b)?);
            let o = overlap(&ra.text, &rb.text);
            (o >= MERGE_MIN_OVERLAP).then(|| (ra.clone(), rb.clone(), n, o))
        })
        .collect();
    merge.sort_by(|x, y| y.2.cmp(&x.2).then(x.0.id.cmp(&y.0.id)));
    merge.truncate(TOP_MERGE);
    let mut split: Vec<(RuleRef, usize)> = served
        .iter()
        .filter(|(_, n)| **n >= SPLIT_MIN_SERVED)
        .filter_map(|(id, n)| rules.get(id).map(|r| (r.clone(), *n)))
        .filter(|(r, _)| r.text.chars().count() >= SPLIT_MIN_CHARS && sentences(&r.text) >= 2)
        .collect();
    split.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.id.cmp(&y.0.id)));
    split.truncate(TOP_SPLIT);
    let mut keywords: Vec<(String, String, usize, Vec<String>)> = kw
        .into_iter()
        .filter(|(_, list)| list.len() >= KEYWORD_MIN_PROMPTS)
        .map(|((d, k), list)| {
            let n = list.len();
            let examples = list.into_iter().rev().take(KEYWORD_EXAMPLES).collect();
            (d, k, n, examples)
        })
        .collect();
    keywords.sort_by(|x, y| y.2.cmp(&x.2).then(x.1.cmp(&y.1)));
    keywords.truncate(TOP_KEYWORDS);
    Candidates { merge, split, keywords, prompts }
}

/// Rules no row served while the log spans [`RETIRE_DAYS`]: the oldest row's date, and the rules. `Err` names how
/// many days the log spans when it is shorter.
fn dead_rules(bench: &Bench<'_>, cwd: &Path) -> Result<(String, usize, usize, Vec<RuleRef>), i64> {
    let dirs = propose::row_dirs(cwd);
    let mut oldest: Option<chrono::DateTime<chrono::FixedOffset>> = None;
    for dir in &dirs {
        for f in match_log::files(dir) {
            let first = std::fs::read_to_string(&f).ok().and_then(|t| t.lines().next().map(String::from));
            let ts = first.and_then(|l| serde_json::from_str::<serde_json::Value>(&l).ok()).and_then(|v| v["ts"].as_str().map(String::from));
            if let Some(t) = ts.and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                && oldest.is_none_or(|o| t < o)
            {
                oldest = Some(t);
            }
        }
    }
    let days = oldest.map(|o| (chrono::Local::now().timestamp() - o.timestamp()) / 86_400).unwrap_or(0);
    if days < RETIRE_DAYS {
        return Err(days);
    }
    let mut served: HashSet<String> = HashSet::new();
    let (mut prompts, mut files) = (0usize, 0usize);
    for dir in &dirs {
        for f in match_log::files(dir) {
            for line in std::fs::read_to_string(&f).unwrap_or_default().lines() {
                if line.contains("\"event\":\"prompt\"") {
                    prompts += 1;
                } else if line.contains("\"event\":\"file\"") {
                    files += 1;
                } else {
                    continue;
                }
                if let Ok(row) = serde_json::from_str::<Row>(line) {
                    served.extend(row.served.into_iter().filter(|i| i.kind == "rule").map(|i| i.id));
                }
            }
        }
    }
    let inert: HashSet<String> = bench.domains.iter().filter(|d| !d.auto_inject).map(|d| d.name.clone()).collect();
    let mut dead: Vec<RuleRef> = bench
        .rules()
        .into_iter()
        .filter(|r| !served.contains(&r.id) && !inert.contains(&r.domain))
        .collect();
    dead.truncate(TOP_RETIRE);
    let since = oldest.map(|o| o.format("%Y-%m-%d").to_string()).unwrap_or_default();
    Ok((since, prompts, files, dead))
}

const STORE_HEADER: &str = "You are reviewing the rules of base, a tool that gives an AI short rules at the right \
moment. Judge each candidate below and answer with one JSON object and nothing else.";

fn store_prompt(bench: &Bench<'_>, c: &Candidates) -> String {
    let mut s = format!("{STORE_HEADER}\n\n");
    if !c.merge.is_empty() {
        s.push_str("MERGE: pairs of rules often served together whose wording overlaps. For each: \"same\": true if they \
                    say the same thing, and \"text\": one rule that says all of both.\n");
        for (n, (a, b, together, _)) in c.merge.iter().enumerate() {
            s.push_str(&format!("[m{}] served together on {together} prompts\n  A: {}\n  B: {}\n", n + 1, quote(&a.text, 500), quote(&b.text, 500)));
        }
        s.push('\n');
    }
    if !c.split.is_empty() {
        s.push_str("SPLIT: rules that may hold two separate instructions. For each: \"split\": true if so, and \"parts\": \
                    two rules, each {\"text\", \"keywords\": [2 to 4 words that should bring it back]}.\n");
        for (n, (r, served)) in c.split.iter().enumerate() {
            s.push_str(&format!("[s{}] served on {served} prompts: {}\n", n + 1, quote(&r.text, 800)));
        }
        s.push('\n');
    }
    if !c.keywords.is_empty() {
        s.push_str("KEYWORDS: keywords that brought a domain's rules on many prompts. For each: \"about\": one true or false \
                    per prompt listed (is that prompt about this domain?), and \"instead\": narrower words or phrases, \
                    copied from the prompts that are about it, that would still bring the domain on those (empty if \
                    none).\n");
        for (n, (d, k, count, examples)) in c.keywords.iter().enumerate() {
            let about: Vec<String> = bench
                .by_domain
                .iter()
                .find(|(name, _)| name == d)
                .map(|(_, rules)| rules.iter().take(3).map(|r| quote(&r.text, 160)).collect())
                .unwrap_or_default();
            s.push_str(&format!("[k{}] domain {d}, keyword \"{k}\", brought it on {count} prompts.\n", n + 1));
            if !about.is_empty() {
                s.push_str(&format!("  The domain's rules say: {}\n", about.join(" / ")));
            }
            for (j, p) in examples.iter().enumerate() {
                s.push_str(&format!("  {}. {}\n", j + 1, quote(p, 300)));
            }
        }
        s.push('\n');
    }
    s.push_str(
        "Answer in this shape, with your own values, one entry per candidate:\n{\"merge\":[{\"id\":\"m1\",\"same\":true,\
         \"text\":\"...\"}],\"split\":[{\"id\":\"s1\",\"split\":true,\"parts\":[{\"text\":\"...\",\"keywords\":[\"...\"]},\
         {\"text\":\"...\",\"keywords\":[\"...\"]}]}],\"keywords\":[{\"id\":\"k1\",\"about\":[true,false],\"instead\":[\"...\"]}]}\n",
    );
    s
}

#[derive(Debug, Default, Deserialize)]
struct StoreAnswer {
    #[serde(default)]
    merge: Vec<MergeAnswer>,
    #[serde(default)]
    split: Vec<SplitAnswer>,
    #[serde(default)]
    keywords: Vec<KeywordAnswer>,
}

#[derive(Debug, Default, Deserialize)]
struct MergeAnswer {
    id: String,
    #[serde(default)]
    same: bool,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Default, Deserialize)]
struct SplitAnswer {
    id: String,
    #[serde(default)]
    split: bool,
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Debug, Default, Deserialize)]
struct Part {
    #[serde(default)]
    text: String,
    #[serde(default)]
    keywords: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KeywordAnswer {
    id: String,
    #[serde(default)]
    about: Vec<bool>,
    #[serde(default)]
    instead: Vec<String>,
}

fn index(id: &str, prefix: char) -> Option<usize> {
    id.trim().strip_prefix(prefix)?.parse::<usize>().ok()?.checked_sub(1)
}

fn rule_target(r: &RuleRef) -> propose::Target {
    propose::Target { kind: "rule", id: short_ref(&r.domain, &r.id), domain: r.domain.clone(), what: r.text.clone() }
}

/// Run the store check when it is due (or `--store`), adding what it writes to `written` and `notes`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check(
    config: &BaseConfig,
    cwd: &Path,
    args: &Args,
    bench: &Bench<'_>,
    calls: &mut Calls<'_>,
    sorter: &mut propose::Sorter<'_>,
    written: &mut Vec<Written>,
    notes: &mut Vec<String>,
) -> StoreCheck {
    let mut out = StoreCheck { due: args.store || due(), ..StoreCheck::default() };
    if !out.due {
        return out;
    }
    let rows = rows(config, cwd);
    let c = candidates(bench, &rows);
    out.merge = c.merge.len();
    out.split = c.split.len();
    out.keywords = c.keywords.len();
    let dead = dead_rules(bench, cwd);
    match &dead {
        Ok((_, _, _, list)) => out.retire = list.len(),
        Err(days) => out.note = Some(format!("retire: the match log covers {days} {} (needs {RETIRE_DAYS})", plural(*days as usize, "day", "days"))),
    }
    let any = !(c.merge.is_empty() && c.split.is_empty() && c.keywords.is_empty());
    let prompt = store_prompt(bench, &c);
    let mut answered: Option<StoreAnswer> = None;
    if any {
        if !calls.can(std::slice::from_ref(&prompt)) {
            notes.push(format!("store check: left for the next pass ({} calls at most per pass)", calls.max));
            return out;
        }
        match calls.ask(&prompt) {
            None => {}
            Some(Ok(text)) => match parse_json::<StoreAnswer>(&text) {
                Some(a) => {
                    keep_cached(&prompt, &text);
                    answered = Some(a);
                }
                None => notes.push("store check: the judge's answer was not the JSON asked for; it runs again next pass".to_string()),
            },
            Some(Err(e)) => notes.push(format!("store check: the judge's call failed ({}); it runs again next pass", super::clip(&e, 160))),
        }
    }
    if args.dry_run {
        return out;
    }
    if any && answered.is_none() {
        return out;
    }
    out.ran = true;
    let total = c.prompts;
    let mut put = |kind: propose::Kind, target: propose::Target, why: String, parts: propose::PatternParts, what: String, evidence: String| {
        let mut prop = propose::pattern(kind, target, why, parts);
        if let Some(id) = write_proposal(sorter, config, &mut prop, notes) {
            written.push(Written { id, kind, domain: prop.target.domain.clone(), what, evidence });
        }
    };
    if let Some(a) = &answered {
        for m in a.merge.iter().filter(|m| m.same && !m.text.trim().is_empty()) {
            let Some((ra, rb, together, o)) = index(&m.id, 'm').and_then(|i| c.merge.get(i)) else { continue };
            let (sa, sb) = (short_ref(&ra.domain, &ra.id), short_ref(&rb.domain, &rb.id));
            let parts = propose::PatternParts {
                text: Some(m.text.trim().to_string()),
                evidence: vec![
                    format!("served together on {together} of the last {total} prompts"),
                    format!("word overlap {o:.2}"),
                    format!("{sb}: {}", super::clip(&rb.text, 160)),
                ],
                merge_with: Some(sb.clone()),
                ..propose::PatternParts::default()
            };
            put(propose::Kind::Merge, rule_target(ra), format!("{sa} and {sb} say the same thing"), parts,
                format!("rules {sa} and {sb} say the same thing"), format!("served together {together} times"));
        }
        for sp in a.split.iter().filter(|s| s.split) {
            let Some((r, served)) = index(&sp.id, 's').and_then(|i| c.split.get(i)) else { continue };
            let parts: Vec<&Part> = sp.parts.iter().filter(|p| !p.text.trim().is_empty()).collect();
            let [first, second] = parts.as_slice() else { continue };
            let clean = |k: &[String]| k.iter().map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect::<Vec<_>>();
            let short = short_ref(&r.domain, &r.id);
            let pp = propose::PatternParts {
                text: Some(first.text.trim().to_string()),
                keywords: clean(&first.keywords),
                evidence: vec![format!("served on {served} of the last {total} prompts; the judge read it as two instructions")],
                second: Some((second.text.trim().to_string(), clean(&second.keywords))),
                ..propose::PatternParts::default()
            };
            put(propose::Kind::Split, rule_target(r), format!("{short} holds two instructions"), pp,
                format!("rule {short} into two"), format!("served {served} times"));
        }
        for k in &a.keywords {
            let Some((domain, kw, count, examples)) = index(&k.id, 'k').and_then(|i| c.keywords.get(i)) else { continue };
            let n = examples.len().min(k.about.len());
            if n < KEYWORD_MIN_PROMPTS {
                continue;
            }
            let on: Vec<&String> = examples.iter().zip(&k.about).filter(|(_, a)| **a).map(|(p, _)| p).collect();
            if on.len() * 3 > n {
                continue;
            }
            let instead: Vec<String> = k
                .instead
                .iter()
                .map(|w| w.trim().to_string())
                .filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case(kw))
                .filter(|w| on.iter().any(|p| crate::domain::matcher::contains_word(&p.to_lowercase(), &w.to_lowercase())))
                .collect();
            let example = on.iter().find(|p| instead.iter().any(|w| crate::domain::matcher::contains_word(&p.to_lowercase(), &w.to_lowercase())));
            let mut evidence = vec![format!(
                "brought domain {domain} on {count} of the last {total} prompts; of {n} the judge read, {} {} about it",
                on.len(),
                if on.len() == 1 { "was" } else { "were" }
            )];
            if let Some(off) = examples.iter().zip(&k.about).find(|(_, a)| !**a).map(|(p, _)| p) {
                evidence.push(format!("off topic: \"{}\"", super::clip(off, 120)));
            }
            let what = if instead.is_empty() {
                format!("drop \"{kw}\"")
            } else {
                format!("drop \"{kw}\" · add {}", instead.iter().map(|w| format!("\"{w}\"")).collect::<Vec<_>>().join(", "))
            };
            let target = propose::Target { kind: "domain", id: domain.clone(), domain: domain.clone(), what: String::new() };
            let pp = propose::PatternParts {
                keywords: instead.clone(),
                example: example.map(|p| p.to_string()),
                evidence,
                dropped: vec![kw.clone()],
                ..propose::PatternParts::default()
            };
            put(propose::Kind::DropKeyword, target, format!("\"{kw}\" brings {domain} where it does not belong"), pp, what,
                format!("{} of {n} prompts on topic", on.len()));
        }
    }
    if let Ok((since, prompts, files, list)) = dead {
        for r in list {
            let short = short_ref(&r.domain, &r.id);
            let pp = propose::PatternParts {
                evidence: vec![format!("served on 0 of {prompts} prompts and {files} file touches since {since}")],
                ..propose::PatternParts::default()
            };
            put(propose::Kind::Retire, rule_target(&r), format!("{short} was never served in {RETIRE_DAYS} days"), pp,
                format!("rule {short}"), format!("0 of {prompts} prompts"));
        }
    }
    if let Some(p) = last_path() {
        let _ = tune::write_json(&p, &Last { at: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false) });
    }
    out
}
