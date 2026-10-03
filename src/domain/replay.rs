//! `base rule replay` (K6, BO-16): a rule change run over the user's own recent prompts before anyone approves it.
//!
//! WHY. BO-15 turns corrections into pending proposals and BO-17 will add its own. Nothing should change what base
//! serves without evidence of what the change would do: which of the user's real prompts would start serving the rule,
//! which would stop, and how many in all. A keyword that would fire on a large share of every prompt is flagged before
//! it ships (K6b).
//!
//! HOW. Two benches ([`Bench`], BO-14's `base rule test` matching): the live config, and the same config with the
//! change made in memory. Each of the last `[tune] replay_prompts` prompts in the match log is judged on both as the
//! first prompt of a fresh session (a neutral session state: no dedup, FRESH), in the prompt hook's own order. What is
//! judged follows the change's target, because a proposal's target is not always a rule (`propose::gap_target`):
//!
//! - a domain: served when the domain matches the prompt (the rules it serves through its keywords);
//! - a rule: as [`Bench::judge`] says (its own matchers through `select`, or its domain);
//! - a decision of an always-on domain: on its own keywords (BO-03); any other decision through its domain;
//! - a new rule: never before; after, a rule with a topic matcher of the proposal's keywords.
//!
//! THE PROMPTS. Prompt rows of the cwd tier's log and the global tier's (where a session's rows can be), merged by
//! time. Task notifications reach the prompt hook as prompts but are no one's words, and a row kept under
//! `[log] prompt_text = matched | off` has no text to replay: both are left out and counted.

use std::path::Path;

use oxigraph::model::TermRef;
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::BaseConfig;
use crate::crud;
use crate::domain::global_decisions::{keyword_hit, GlobalDecisions};
use crate::domain::rule_test::{Bench, RuleRef};
use crate::domain::rules::{self, Converted, Kind, Matcher};
use crate::emit::match_log;

/// What a change is made to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A domain: its prompt keywords.
    Domain(String),
    /// A rule as `base rule list` prints it, `<domain>.<id>`: its own topic words, or its wording.
    Rule(String),
    /// A decision by slug: its own keywords (a decision of an always-on domain, BO-03), or its wording.
    Decision(String),
    /// A rule that does not exist yet: its domain and wording. The change's added keywords are its own topic words.
    NewRule { domain: String, text: String },
}

/// One change to what base serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub target: Target,
    pub add: Vec<String>,
    pub drop: Vec<String>,
    /// New wording: a rewrite's.
    pub text: Option<String>,
}

impl Change {
    /// `domain base · add keyword "user prompt submit"`: the target and what changes, for a header line.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = vec![match &self.target {
            Target::Domain(d) => format!("domain {d}"),
            Target::Rule(r) => format!("rule {r}"),
            Target::Decision(s) => format!("decision {s}"),
            Target::NewRule { domain, text } => format!("domain {domain} · new rule \"{}\"", clip(text, 80)),
        }];
        let quoted = |list: &[String]| list.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", ");
        let noun = |n: usize| if n == 1 { "keyword" } else { "keywords" };
        if !self.add.is_empty() {
            let verb = if matches!(self.target, Target::NewRule { .. }) { "its" } else { "add" };
            parts.push(format!("{verb} {} {}", noun(self.add.len()), quoted(&self.add)));
        }
        if !self.drop.is_empty() {
            parts.push(format!("drop {} {}", noun(self.drop.len()), quoted(&self.drop)));
        }
        if let Some(t) = &self.text
            && !matches!(self.target, Target::NewRule { .. })
        {
            parts.push(format!("new wording \"{}\"", clip(t, 80)));
        }
        parts.join(" · ")
    }
}

/// The prompts a replay runs over.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prompts {
    /// Each prompt's time and text, oldest first.
    pub rows: Vec<(String, String)>,
    /// Task notifications among the rows read, left out.
    pub notifications: usize,
    /// Rows with no text (`[log] prompt_text`), left out.
    pub textless: usize,
}

/// The last `n` prompt rows across the tiers a session's rows can be in, merged by time, with task notifications and
/// rows without text left out and counted.
pub fn recent_prompts(cwd: &Path, n: usize) -> Prompts {
    let mut rows: Vec<match_log::Row> = Vec::new();
    for dir in crate::corrections::propose::row_dirs(cwd) {
        rows.extend(match_log::last_prompt_rows(&dir, n).unwrap_or_default());
    }
    let at = |ts: &str| chrono::DateTime::parse_from_rfc3339(ts).map(|t| t.timestamp()).unwrap_or(0);
    rows.sort_by(|a, b| at(&a.ts).cmp(&at(&b.ts)).then_with(|| a.ts.cmp(&b.ts)));
    let skip = rows.len().saturating_sub(n);
    let mut out = Prompts::default();
    for r in rows.into_iter().skip(skip) {
        match r.text {
            Some(t) if t.trim_start().starts_with("<task-notification>") => out.notifications += 1,
            Some(t) if !t.trim().is_empty() => out.rows.push((r.ts, t)),
            _ => out.textless += 1,
        }
    }
    out
}

/// What a replay found.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub replayed: usize,
    /// The first and last replayed prompt's date.
    pub from: Option<String>,
    pub to: Option<String>,
    pub notifications: usize,
    pub textless: usize,
    /// The prompts the change starts serving it on, newest first.
    pub newly: Vec<String>,
    /// The prompts it stops serving it on, newest first.
    pub stops: Vec<String>,
    pub served_before: usize,
    pub served_after: usize,
    /// `[tune] broad_share`.
    pub limit: f32,
}

impl Outcome {
    /// Served after the change, over the prompts replayed.
    pub fn share(&self) -> f32 {
        if self.replayed == 0 { 0.0 } else { self.served_after as f32 / self.replayed as f32 }
    }

    /// K6b: served on more than `[tune] broad_share` of the prompts, and the change is what adds some of them. A
    /// change that adds no prompt is never flagged, so the rules of an always-on domain do not flag every change.
    pub fn too_broad(&self) -> bool {
        self.replayed > 0 && !self.newly.is_empty() && self.share() > self.limit
    }

    /// `1.8%`.
    pub fn percent(&self) -> String {
        format!("{:.1}%", self.share() * 100.0)
    }

    /// `TOO BROAD (limit 25%)`.
    pub fn flag(&self) -> String {
        format!("TOO BROAD (limit {:.0}%)", self.limit * 100.0)
    }

    /// `replay 1.8%`, `replay 31.0% TOO BROAD`, `replay: no prompts yet`: the review's header names it in one word.
    pub fn summary(&self) -> String {
        if self.replayed == 0 {
            return "replay: no prompts yet".to_string();
        }
        let flag = if self.too_broad() { " TOO BROAD" } else { "" };
        format!("replay {}{flag}", self.percent())
    }
}

/// Run `change` over the last `[tune] replay_prompts` prompts from `cwd`.
pub fn run(config: &BaseConfig, cwd: &Path, change: &Change) -> Result<Outcome, String> {
    Replayer::load(config, cwd).run(change)
}

/// The live config and the prompts, loaded once for several replays (`base rule review` replays every pending
/// proposal; a store load costs a second on a large graph).
pub struct Replayer<'a> {
    config: &'a BaseConfig,
    live: Bench<'a>,
    store: Option<Store>,
    prompts: Prompts,
}

impl<'a> Replayer<'a> {
    pub fn load(config: &'a BaseConfig, cwd: &Path) -> Self {
        let prompts = recent_prompts(cwd, config.tune.replay_prompts);
        let (live, store) = Bench::load_with_store(config, cwd);
        Replayer { config, live, store, prompts }
    }

    pub fn run(&self, change: &Change) -> Result<Outcome, String> {
        replay(self.config, &self.live, self.store.as_ref(), change, &self.prompts)
    }
}

/// [`run`] on a bench and prompts already loaded: the seam the tests and `rule review` drive.
pub fn replay(
    config: &BaseConfig,
    live: &Bench<'_>,
    store: Option<&Store>,
    change: &Change,
    prompts: &Prompts,
) -> Result<Outcome, String> {
    let plan = plan(config, live, store, change)?;
    let mut out = Outcome {
        replayed: prompts.rows.len(),
        from: prompts.rows.first().map(|(ts, _)| ts.chars().take(10).collect()),
        to: prompts.rows.last().map(|(ts, _)| ts.chars().take(10).collect()),
        notifications: prompts.notifications,
        textless: prompts.textless,
        newly: Vec::new(),
        stops: Vec::new(),
        served_before: 0,
        served_after: 0,
        limit: config.tune.broad_share,
    };
    for (_, text) in prompts.rows.iter().rev() {
        let before = served(live, &plan.before, text);
        let after = served(&plan.after, &plan.after_probe, text);
        out.served_before += usize::from(before);
        out.served_after += usize::from(after);
        if after && !before {
            out.newly.push(text.clone());
        }
        if before && !after {
            out.stops.push(text.clone());
        }
    }
    Ok(out)
}

/// What is judged on a prompt.
#[derive(Debug, Clone)]
enum Probe {
    /// Never served: a rule that does not exist.
    Nothing,
    /// The domain matches the prompt: the rules it serves through its keywords.
    Domain(String),
    /// A rule, as [`Bench::judge`] decides.
    Rule(RuleRef),
    /// A decision of an always-on domain, on its own keywords.
    Keywords(Vec<String>),
}

fn served(bench: &Bench<'_>, probe: &Probe, prompt: &str) -> bool {
    match probe {
        Probe::Nothing => false,
        // No rule with matchers has an empty id, so `judge` asks the domain.
        Probe::Domain(d) => bench.judge(&RuleRef { id: String::new(), domain: d.clone(), text: String::new() }, prompt).served,
        Probe::Rule(r) => bench.judge(r, prompt).served,
        Probe::Keywords(k) => bench.star_commands(prompt).is_empty() && keyword_hit(k, prompt),
    }
}

/// The changed bench, and what to judge before and after.
struct Plan<'a> {
    after: Bench<'a>,
    before: Probe,
    after_probe: Probe,
}

/// `list` with `add` appended (whole phrases, case ignored) and `drop` removed.
pub fn edit_list(list: &[String], add: &[String], drop: &[String]) -> Vec<String> {
    let mut out: Vec<String> =
        list.iter().filter(|k| !drop.iter().any(|d| d.trim().eq_ignore_ascii_case(k.trim()))).cloned().collect();
    for a in add.iter().map(|a| a.trim()).filter(|a| !a.is_empty()) {
        if !out.iter().any(|k| k.trim().eq_ignore_ascii_case(a)) {
            out.push(a.to_string());
        }
    }
    out
}

/// A rule's matchers with its topic words edited: the words go into its topic matcher (one is added when it has
/// none); a topic matcher left with no words goes, since an empty one would score the rule's text instead (F11).
pub fn edit_topic_words(matchers: &[Matcher], add: &[String], drop: &[String]) -> Vec<Matcher> {
    let mut out: Vec<Matcher> = matchers.to_vec();
    match out.iter().position(|m| m.kind == Kind::Topic) {
        Some(i) => {
            let had = !out[i].words.is_empty();
            out[i].words = edit_list(&out[i].words, add, drop);
            if had && out[i].words.is_empty() {
                out.remove(i);
            }
        }
        None if !add.is_empty() => out.push(Matcher::for_topic(edit_list(&[], add, &[]))),
        None => {}
    }
    out
}

fn plan<'a>(config: &BaseConfig, live: &Bench<'a>, store: Option<&Store>, change: &Change) -> Result<Plan<'a>, String> {
    match &change.target {
        Target::Domain(name) => {
            if change.text.is_some() {
                return Err(format!("domain {name} has no wording to change"));
            }
            let want = crud::slugify(name);
            let mut domains = live.domains.clone();
            let Some(d) = domains.iter_mut().find(|d| crud::slugify(&d.name) == want) else {
                return Err(format!("no domain '{name}' (base domain list shows them)"));
            };
            d.prompt_keywords = edit_list(&d.prompt_keywords, &change.add, &change.drop);
            let name = d.name.clone();
            let after = live.changed(domains, live.converted.clone());
            Ok(Plan { after, before: Probe::Domain(name.clone()), after_probe: Probe::Domain(name) })
        }
        Target::Rule(r) => {
            let rule = find_rule(live, r)?;
            let mut converted = live.converted.clone();
            let at = converted.iter().position(|c| c.rule.id == rule.id);
            let mut after_ref = rule.clone();
            if !change.add.is_empty() || !change.drop.is_empty() {
                match at {
                    Some(i) => {
                        converted[i].matchers = edit_topic_words(&converted[i].matchers, &change.add, &change.drop);
                        if converted[i].matchers.is_empty() {
                            converted.remove(i);
                        }
                    }
                    None if !change.add.is_empty() => converted.push(Converted {
                        rule: rules::build(&rule.domain, rule.text.clone(), None, None),
                        matchers: edit_topic_words(&[], &change.add, &[]),
                    }),
                    None => {}
                }
            }
            if let Some(text) = change.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                after_ref = RuleRef { id: rules::rule_id(&rule.domain, text), domain: rule.domain.clone(), text: text.to_string() };
                if let Some(c) = converted.iter_mut().find(|c| c.rule.id == rule.id) {
                    c.rule = rules::build(&rule.domain, text.to_string(), c.rule.rationale.clone(), c.rule.iri.clone());
                }
            }
            let after = live.changed(live.domains.clone(), converted);
            Ok(Plan { after, before: Probe::Rule(rule), after_probe: Probe::Rule(after_ref) })
        }
        Target::Decision(slug) => {
            let store = store.ok_or_else(|| "no graph here, so no decision to replay".to_string())?;
            let global = GlobalDecisions::load(store, config, &live.domains);
            let after = live.changed(live.domains.clone(), live.converted.clone());
            if let Some(g) = global.by_slug(slug) {
                let edited = edit_list(&g.keywords, &change.add, &change.drop);
                return Ok(Plan { after, before: Probe::Keywords(g.keywords.clone()), after_probe: Probe::Keywords(edited) });
            }
            // Any other decision is served through its domain's CONTEXT, so its own keywords change nothing.
            let domain = decision_domain(config, store, &live.domains, slug)
                .ok_or_else(|| format!("no decision '{slug}' (slugs come from base decision search --keyword <word>)"))?;
            Ok(Plan { after, before: Probe::Domain(domain.clone()), after_probe: Probe::Domain(domain) })
        }
        Target::NewRule { domain, text } => {
            let mut converted = live.converted.clone();
            let rule = rules::build(domain, text.clone(), None, None);
            let after_ref = RuleRef { id: rule.id.clone(), domain: domain.clone(), text: text.clone() };
            let after_probe = if change.add.is_empty() {
                // `rule add` without words: a rule with no matchers, served through its domain.
                Probe::Domain(domain.clone())
            } else {
                converted.push(Converted { rule, matchers: edit_topic_words(&[], &change.add, &[]) });
                Probe::Rule(after_ref)
            };
            let after = live.changed(live.domains.clone(), converted);
            Ok(Plan { after, before: Probe::Nothing, after_probe })
        }
    }
}

/// The one rule `spec` (`<domain>.<id>`, or an id) names among the bench's rules.
pub fn find_rule(bench: &Bench<'_>, spec: &str) -> Result<RuleRef, String> {
    let (domain, id) = crud::rule::parse_rule_ref(spec)?;
    let want = domain.as_deref().map(crud::slugify);
    let found: Vec<RuleRef> = bench
        .rules()
        .into_iter()
        .filter(|r| r.id.starts_with(&id) && want.as_deref().is_none_or(|w| crud::slugify(&r.domain) == w))
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!("no rule '{spec}' (ids come from base rule list --domain <domain>)")),
        many => Err(format!(
            "'{spec}' fits {} rules: {}; give more of the id",
            many.len(),
            many.iter().map(RuleRef::short).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The configured domain a decision is filed under, by name.
fn decision_domain(config: &BaseConfig, store: &Store, domains: &[crate::domain::DomainDef], slug: &str) -> Option<String> {
    let ns = &config.namespace;
    let iri = crud::build_iri(ns, "decision", slug);
    let q = format!(
        "{}\nSELECT ?dom WHERE {{ GRAPH ?g {{ ?dom {p}:hasDecision <{iri}> }} }}",
        crud::prefixes(ns),
        p = ns.prefix
    );
    let Ok(QueryResults::Solutions(rows)) = crate::store::query(store, &q) else { return None };
    let iris: Vec<String> = rows
        .filter_map(Result::ok)
        .filter_map(|r| match r.get("dom").map(Into::into) {
            Some(TermRef::NamedNode(n)) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect();
    domains
        .iter()
        .find(|d| iris.contains(&crud::build_iri(ns, "domain", &crud::slugify(&d.name))))
        .map(|d| d.name.clone())
}

/// What `base rule replay` prints: the header line, then [`render_body`].
pub fn render(header: &str, o: &Outcome) -> String {
    format!("{header}\n{}", render_body(o))
}

/// The replay's lines: how many prompts and when, the prompts it starts and stops serving on, and the share.
pub fn render_body(o: &Outcome) -> String {
    let mut s = String::new();
    let mut left: Vec<String> = Vec::new();
    if o.notifications > 0 {
        left.push(format!("{} task {} left out", o.notifications, plural(o.notifications, "notification", "notifications")));
    }
    if o.textless > 0 {
        left.push(format!("{} with no text left out ([log] prompt_text)", o.textless));
    }
    let left: String = left.iter().map(|l| format!(" · {l}")).collect();
    if o.replayed == 0 {
        s.push_str(&format!("no prompts in the match log yet: nothing to replay{left}\n"));
        return s;
    }
    let range = match (&o.from, &o.to) {
        (Some(a), Some(b)) if a == b => format!(" ({a})"),
        (Some(a), Some(b)) => format!(" ({a} to {b})"),
        _ => String::new(),
    };
    s.push_str(&format!("replayed {} {}{range}{left}\n", o.replayed, plural(o.replayed, "prompt", "prompts")));
    for (label, list) in [("newly served on", &o.newly), ("stops serving on", &o.stops)] {
        if list.is_empty() {
            s.push_str(&format!("  {label} 0\n"));
            continue;
        }
        s.push_str(&format!("  {label} {} {}, e.g.:\n", list.len(), plural(list.len(), "prompt", "prompts")));
        for p in list.iter().take(EXAMPLES) {
            s.push_str(&format!("    \"{}\"\n", clip(p, 120)));
        }
    }
    let flag = if o.too_broad() { format!("  {}", o.flag()) } else { String::new() };
    s.push_str(&format!("  share: {} / {} = {}{flag}\n", o.served_after, o.replayed, o.percent()));
    s
}

/// Examples printed per list.
const EXAMPLES: usize = 3;

fn plural<'s>(n: usize, one: &'s str, many: &'s str) -> &'s str {
    if n == 1 { one } else { many }
}

/// `text` on one line, cut to `max` characters with `…`.
pub fn clip(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let s: String = one.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", s.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn keyword_lists_edit_as_whole_phrases_case_ignored() {
        assert_eq!(edit_list(&w(&["base", "hooks"]), &w(&["Hooks", "user prompt submit"]), &w(&["BASE"])), w(&["hooks", "user prompt submit"]));
    }

    #[test]
    fn topic_words_edit_and_an_emptied_matcher_goes() {
        let place = Matcher::for_place("C:/x");
        let got = edit_topic_words(&[place.clone(), Matcher::for_topic(w(&["a"]))], &[], &w(&["a"]));
        assert_eq!(got, vec![place.clone()], "the topic matcher emptied by the drop goes");
        let got = edit_topic_words(std::slice::from_ref(&place), &w(&["b"]), &[]);
        assert_eq!(got, vec![place, Matcher::for_topic(w(&["b"]))], "a rule with no topic matcher gets one");
    }

    #[test]
    fn too_broad_needs_new_prompts_as_well_as_a_high_share() {
        let o = |after: usize, newly: usize| Outcome {
            replayed: 100,
            from: None,
            to: None,
            notifications: 0,
            textless: 0,
            newly: vec![String::new(); newly],
            stops: Vec::new(),
            served_before: after - newly,
            served_after: after,
            limit: 0.25,
        };
        assert!(o(30, 5).too_broad());
        assert!(!o(25, 5).too_broad(), "at the limit is not over it");
        assert!(!o(100, 0).too_broad(), "an always-on domain: served everywhere before and after, nothing added");
        assert_eq!(o(30, 5).summary(), "replay 30.0% TOO BROAD");
    }
}
