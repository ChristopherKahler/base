//! `base rule test` (K2, D3, BO-14): every rule's test prompts, replayed through the prompt hook's own matching.
//!
//! WHY THIS EXISTS. Nothing checked whether a domain's keywords fire on the prompts they are meant for, or stay quiet on
//! the ones they are not, so "is the config right" was a feeling. On 2026-10-01 the operator asked about the prompt
//! hook being cut off and the hook injections; the `base` domain had 12 rules and no prompt keywords, so none of them
//! was served (F8). A rule now carries prompts that must serve it (`fires_on`) and prompts that must not (`quiet_on`),
//! and this module says which of them the matcher gets wrong.
//!
//! WHAT "SERVED" MEANS. The first prompt of a fresh session, through the steps `user_prompt_submit::collect` takes, in
//! its order:
//!
//! 1. No domains at all: only rules with matchers of their own can be served.
//! 2. A star command in the prompt: the hook serves the command's rules and nothing else, so no rule here is served.
//! 3. A rule with matchers of its own: [`rules::select_unrecorded`] with an empty session at FRESH, so
//!    `topic_min_score` and `topic_max` cut exactly as they cut in the hook. Such a rule is never served through its
//!    domain: the hook takes its id out of the domain block.
//! 4. Any other rule: its domain is in [`matcher::match_domains_auto`] with no touched paths (a fresh session has
//!    none): the always-on domains and the keyword matches, with `exclude` and `auto_inject = false` honoured.
//!
//! Not replayed: the prompt budget and the session's dedup. Whether a matched block fits depends on everything else
//! that prompt matched, not on the rule's own config, and the config is what a rule test tests.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::command::CommandDef;
use crate::config::BaseConfig;
use crate::domain::matcher::{self, MatchReason, TriggerContext};
use crate::domain::rules::{self, Converted, CutReason, Event, RuleTests, SelectContext, StoredTests, Why};
use crate::domain::session::{Bracket, SessionState};
use crate::domain::DomainDef;

/// One rule as a test sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRef {
    /// The full [`rules::rule_id`].
    pub id: String,
    pub domain: String,
    pub text: String,
}

impl RuleRef {
    /// `base.9f2c1a7b`: the domain and the first 8 hex characters of the id, as `base rule list` prints it and
    /// `base rule update` takes it.
    pub fn short(&self) -> String {
        short_ref(&self.domain, &self.id)
    }
}

/// `<domain>.<first 8 of id>`.
pub fn short_ref(domain: &str, id: &str) -> String {
    format!("{domain}.{}", &id[..id.len().min(8)])
}

/// Everything the prompt hook's matching reads, loaded once for a whole run.
pub struct Bench<'a> {
    pub config: &'a BaseConfig,
    pub domains: Vec<DomainDef>,
    /// Rules with matchers of their own, as the hook reads them.
    pub converted: Vec<Converted>,
    pub commands: Vec<CommandDef>,
    pub ctx: TriggerContext,
    /// Each domain's rules, as `rules_for_domain` reads them, in the domains' order.
    pub by_domain: Vec<(String, Vec<RuleRef>)>,
    /// Every stored test, by rule id.
    pub tests: HashMap<String, StoredTests>,
    keywords: HashMap<String, Vec<String>>,
}

impl<'a> Bench<'a> {
    /// Load what the prompt hook would read from `cwd`: `domains.toml` synced into the graph first, as every hook and
    /// `base context` do, then the domains, the merged store, the rules with matchers, the star commands, the
    /// registered projects and the stored tests.
    pub fn load(config: &'a BaseConfig, cwd: &Path) -> Self {
        crate::hook::user_prompt_submit::ensure_domain_sync_pub(config, cwd);
        let domains = crate::domain::load_domains(cwd);
        let store = crate::store::load_merged(cwd);
        let converted = rules::rules_with_matchers(store.as_ref(), config, &domains);
        let ctx = TriggerContext {
            home: crate::home::home_root().map(|h| h.display().to_string()),
            registered: store
                .as_ref()
                .map(|s| crate::domain::registered_projects(s, &config.namespace, cwd))
                .unwrap_or_default(),
        };
        let by_domain = domains
            .iter()
            .map(|d| {
                let rules = rules::rules_for_domain(store.as_ref(), config, d)
                    .into_iter()
                    .map(|r| RuleRef { id: r.id, domain: r.domain, text: r.text })
                    .collect();
                (d.name.clone(), rules)
            })
            .collect();
        let tests = rules::rule_tests(store.as_ref(), config, &domains);
        Self::from_parts(config, domains, converted, crate::command::load_commands(cwd), ctx, by_domain, tests)
    }

    /// A bench from parts already loaded: the seam the unit tests drive.
    pub fn from_parts(
        config: &'a BaseConfig,
        domains: Vec<DomainDef>,
        converted: Vec<Converted>,
        commands: Vec<CommandDef>,
        ctx: TriggerContext,
        by_domain: Vec<(String, Vec<RuleRef>)>,
        tests: HashMap<String, StoredTests>,
    ) -> Self {
        let keywords = domains.iter().map(|d| (d.name.clone(), d.prompt_keywords.clone())).collect();
        Bench { config, domains, converted, commands, ctx, by_domain, tests, keywords }
    }

    /// Every rule a prompt could serve, each once, in domain order: each domain's rules, the rules with matchers of
    /// domains `domains.toml` does not hold, then rules that carry tests and are found nowhere else (their domain is
    /// gone, so every `fires_on` of theirs misses).
    pub fn rules(&self) -> Vec<RuleRef> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<RuleRef> = Vec::new();
        let mut push = |r: RuleRef, out: &mut Vec<RuleRef>| {
            if seen.insert(r.id.clone()) {
                out.push(r);
            }
        };
        for (domain, rules) in &self.by_domain {
            for r in rules {
                push(r.clone(), &mut out);
            }
            for c in self.converted.iter().filter(|c| c.rule.domain == *domain) {
                push(RuleRef { id: c.rule.id.clone(), domain: c.rule.domain.clone(), text: c.rule.text.clone() }, &mut out);
            }
        }
        for c in &self.converted {
            push(RuleRef { id: c.rule.id.clone(), domain: c.rule.domain.clone(), text: c.rule.text.clone() }, &mut out);
        }
        let mut stray: Vec<(&String, &StoredTests)> = self.tests.iter().collect();
        stray.sort_by(|a, b| (&a.1.domain, a.0).cmp(&(&b.1.domain, b.0)));
        for (id, s) in stray {
            push(RuleRef { id: id.clone(), domain: s.domain.clone(), text: s.text.clone() }, &mut out);
        }
        out
    }

    /// Would the prompt hook serve `rule` on `prompt`, as the first prompt of a fresh session, and why.
    pub fn judge(&self, rule: &RuleRef, prompt: &str) -> Verdict {
        // The hook's order (`user_prompt_submit::collect`): with no domains it serves matcher rules only, before it
        // looks for star commands; with domains, a star command returns before anything is matched.
        if !self.domains.is_empty() {
            let stars: Vec<String> = crate::command::match_commands(prompt, &self.commands)
                .into_iter()
                .filter(|c| !crate::command::format_command_output(c).is_empty())
                .map(|c| format!("*{}", c.name))
                .collect();
            if !stars.is_empty() {
                return Verdict { served: false, why: format!("star command {} passes every rule by", stars.join(" ")) };
            }
        }
        match self.converted.iter().find(|c| c.rule.id == rule.id) {
            Some(c) => self.judge_matchers(c, prompt),
            None => self.judge_domain(rule, prompt),
        }
    }

    /// A rule with matchers of its own: [`rules::select_unrecorded`], as `matcher_blocks` calls it.
    fn judge_matchers(&self, c: &Converted, prompt: &str) -> Verdict {
        let cx = SelectContext {
            bracket: Bracket::Fresh,
            now: SessionState::now_secs(),
            home: self.ctx.home.as_deref(),
            keywords: &self.keywords,
            rules: &self.config.rules,
        };
        let selection =
            rules::select_unrecorded(&self.converted, &Event::Prompt { text: prompt }, &SessionState::default(), &cx);
        let score = selection.scores.iter().find(|s| s.id == c.rule.id);
        let words = score.map(|s| s.words.join(", ")).unwrap_or_default();
        if let Some(served) = selection.served.iter().find(|s| s.rule.id == c.rule.id) {
            let why = match &served.why {
                Why::Topic(s) => format!("topic score {s:.2} (words: {words})"),
                other => format!("served: {}", other.label()),
            };
            return Verdict { served: true, why };
        }
        let min = self.config.rules.topic_min_score;
        let why = match selection.cut.iter().find(|x| x.id == c.rule.id) {
            Some(x) if x.reason == CutReason::TopicLimit => {
                format!("topic score {:.2}, cut by topic_max ({}) under higher-scoring rules", x.score, self.config.rules.topic_max)
            }
            Some(x) => format!("topic score {:.2} under topic_min_score {min:.2} (words: {words})", x.score),
            None if !c.matchers.iter().any(|m| matches!(m.kind, rules::Kind::Topic | rules::Kind::Always)) => {
                "its matchers are place or action only, and no prompt serves those".to_string()
            }
            None => format!("topic score 0: no word of its own, of its text or of its domain's keywords is in the prompt (minimum {min:.2})"),
        };
        Verdict { served: false, why }
    }

    /// Any other rule: its domain, through [`matcher::match_domains_auto`] with no touched paths.
    fn judge_domain(&self, rule: &RuleRef, prompt: &str) -> Verdict {
        let matched = matcher::match_domains_auto(prompt, &self.domains, &[], &self.ctx);
        let served = matched.iter().any(|m| m.domain.name == rule.domain);
        let list: Vec<String> = matched
            .iter()
            .map(|m| match m.reason {
                MatchReason::Always => format!("{}(always)", m.domain.name),
                _ => format!("{}(keyword: {})", m.domain.name, m.keywords.join(", ")),
            })
            .collect();
        let mut why = format!("matched: {}", if list.is_empty() { "nothing".to_string() } else { list.join(", ") });
        if !served {
            why.push_str(&format!(" · {}", self.why_not(&rule.domain, &prompt.to_lowercase())));
        }
        Verdict { served, why }
    }

    /// Why `domain` did not come in on a prompt, in the order `is_matched` decides.
    fn why_not(&self, domain: &str, prompt_lower: &str) -> String {
        let Some(d) = self.domains.iter().find(|d| d.name == domain) else {
            return format!("{domain}: not in domains.toml, so no prompt brings it in");
        };
        if !d.auto_inject {
            return format!("{domain}: auto_inject = false");
        }
        if let Some(p) = d.exclude.iter().find(|p| prompt_lower.contains(&p.to_lowercase())) {
            return format!("{domain}: excluded by \"{p}\"");
        }
        if d.prompt_keywords.is_empty() {
            return format!("{domain}: has no prompt keywords");
        }
        format!("{domain}: no keyword matched")
    }

    /// Run the tests `filter` picks. An unknown domain or rule, or a rule id that fits two rules, is an error.
    pub fn run(&self, filter: &Filter) -> Result<Report, String> {
        let all = self.rules();
        let slug = crate::crud::slugify;
        let picked: Vec<&RuleRef> = match filter {
            Filter::All => all.iter().collect(),
            Filter::Domain(d) => {
                let want = slug(d);
                let picked: Vec<&RuleRef> = all.iter().filter(|r| slug(&r.domain) == want).collect();
                if picked.is_empty() && !self.domains.iter().any(|x| slug(&x.name) == want) {
                    return Err(format!("no domain '{d}': base domain list shows them"));
                }
                picked
            }
            Filter::Rule { domain, id } => {
                let picked: Vec<&RuleRef> = all
                    .iter()
                    .filter(|r| r.id.starts_with(id.as_str()) && domain.as_deref().is_none_or(|d| slug(&r.domain) == slug(d)))
                    .collect();
                match picked.len() {
                    0 => return Err(format!("no rule '{}' (ids come from base rule list --domain <domain>)", filter_label(filter))),
                    1 => picked,
                    n => {
                        let names: Vec<String> = picked.iter().map(|r| r.short()).collect();
                        return Err(format!("'{}' fits {n} rules: {}; give more of the id", filter_label(filter), names.join(", ")));
                    }
                }
            }
        };

        let mut tested: Vec<Tested> = Vec::new();
        for r in &picked {
            let Some(stored) = self.tests.get(&r.id) else { continue };
            let mut outcomes: Vec<Outcome> = Vec::new();
            for (list, expect) in [(&stored.tests.fires_on, Expect::Fires), (&stored.tests.quiet_on, Expect::Quiet)] {
                for prompt in list {
                    outcomes.push(Outcome { prompt: prompt.clone(), expect, verdict: self.judge(r, prompt) });
                }
            }
            tested.push(Tested { rule: (*r).clone(), outcomes });
        }

        // K2d: per domain, how many of its rules have no tests.
        let mut coverage: Vec<Coverage> = Vec::new();
        for r in &picked {
            let has = self.tests.contains_key(&r.id);
            match coverage.iter_mut().find(|c| c.domain == r.domain) {
                Some(c) => {
                    c.total += 1;
                    c.untested += usize::from(!has);
                }
                None => coverage.push(Coverage { domain: r.domain.clone(), untested: usize::from(!has), total: 1 }),
            }
        }
        if let Filter::Domain(d) = filter
            && coverage.is_empty()
        {
            coverage.push(Coverage { domain: d.clone(), untested: 0, total: 0 });
        }
        Ok(Report { tested, coverage, per_domain: matches!(filter, Filter::All) })
    }
}

/// `<domain>.<id>` or `<id>`, as the user gave it.
fn filter_label(filter: &Filter) -> String {
    match filter {
        Filter::Rule { domain: Some(d), id } => format!("{d}.{id}"),
        Filter::Rule { domain: None, id } => id.clone(),
        Filter::Domain(d) => d.clone(),
        Filter::All => String::new(),
    }
}

/// Which tests `base rule test` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    All,
    Domain(String),
    /// A rule id or the start of one, in a domain when given.
    Rule { domain: Option<String>, id: String },
}

/// Would the hook serve the rule, and the one line that says why.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub served: bool,
    pub why: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// A `fires_on` prompt: the rule must be served.
    Fires,
    /// A `quiet_on` prompt: the rule must not be served.
    Quiet,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub prompt: String,
    pub expect: Expect,
    pub verdict: Verdict,
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.verdict.served == (self.expect == Expect::Fires)
    }

    /// `MISS`, `FALSE FIRE`, `ok (fires)`, `ok (quiet)`.
    pub fn label(&self) -> &'static str {
        match (self.expect, self.passed()) {
            (Expect::Fires, true) => "ok (fires)",
            (Expect::Quiet, true) => "ok (quiet)",
            (Expect::Fires, false) => "MISS",
            (Expect::Quiet, false) => "FALSE FIRE",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tested {
    pub rule: RuleRef,
    pub outcomes: Vec<Outcome>,
}

/// K2d: one domain's rules, and how many of them carry no tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub domain: String,
    pub untested: usize,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub tested: Vec<Tested>,
    pub coverage: Vec<Coverage>,
    /// Every domain was run: the coverage prints as a list, one line per domain.
    pub per_domain: bool,
}

impl Report {
    pub fn misses(&self) -> usize {
        self.count(Expect::Fires)
    }

    pub fn false_fires(&self) -> usize {
        self.count(Expect::Quiet)
    }

    fn count(&self, expect: Expect) -> usize {
        self.tested.iter().flat_map(|t| &t.outcomes).filter(|o| o.expect == expect && !o.passed()).count()
    }

    /// Any miss or false fire: `base rule test` exits 1.
    pub fn failed(&self) -> bool {
        self.misses() + self.false_fires() > 0
    }

    /// `3 rules tested, 1 miss, 0 false fires`.
    pub fn summary(&self) -> String {
        let n = self.tested.len();
        format!(
            "{n} {} tested, {} {}, {} {}",
            plural(n, "rule", "rules"),
            self.misses(),
            plural(self.misses(), "miss", "misses"),
            self.false_fires(),
            plural(self.false_fires(), "false fire", "false fires"),
        )
    }

    /// What `base rule test` prints: each tested rule (one line when it passes, every prompt when it does not, with
    /// the reason under each failure), then the summary and K2d's count of rules without tests.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for t in &self.tested {
            let ok = |e: Expect| t.outcomes.iter().filter(|o| o.expect == e && o.passed()).count();
            let has = |e: Expect| t.outcomes.iter().any(|o| o.expect == e);
            if t.outcomes.iter().all(Outcome::passed) {
                let mut parts: Vec<String> = Vec::new();
                if has(Expect::Fires) {
                    parts.push(format!("ok {} {}", ok(Expect::Fires), plural(ok(Expect::Fires), "fire", "fires")));
                }
                if has(Expect::Quiet) {
                    parts.push(format!("ok {} quiet", ok(Expect::Quiet)));
                }
                out.push_str(&format!("{}   {}   \"{}\"\n", t.rule.short(), parts.join(" · "), clip(&t.rule.text, 60)));
                continue;
            }
            out.push_str(&format!("{}   \"{}\"\n", t.rule.short(), clip(&t.rule.text, 60)));
            for o in &t.outcomes {
                out.push_str(&format!("  {:<12}\"{}\"\n", o.label(), clip(&o.prompt, 100)));
                if !o.passed() {
                    out.push_str(&format!("              {}\n", o.verdict.why));
                }
            }
        }
        out.push_str(&self.summary());
        if self.per_domain {
            let with_rules: Vec<&Coverage> = self.coverage.iter().filter(|c| c.total > 0).collect();
            if with_rules.iter().all(|c| c.untested == 0) {
                out.push_str("\nevery rule has tests\n");
            } else {
                out.push_str("\nrules with no tests, per domain (untested of total):\n");
                let w = with_rules.iter().map(|c| c.domain.chars().count()).max().unwrap_or(0);
                for c in with_rules {
                    out.push_str(&format!("  {:<w$}  {} of {}\n", c.domain, c.untested, c.total));
                }
            }
        } else {
            for c in &self.coverage {
                out.push_str(&format!(
                    " · {} {} in {} {} no tests",
                    c.untested,
                    plural(c.untested, "rule", "rules"),
                    c.domain,
                    plural(c.untested, "has", "have")
                ));
            }
            out.push('\n');
        }
        out
    }
}

fn plural<'s>(n: usize, one: &'s str, many: &'s str) -> &'s str {
    if n == 1 { one } else { many }
}

/// `text` cut to `max` characters, with `…` when cut, on one line.
fn clip(text: &str, max: usize) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let s: String = one_line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", s.trim_end())
}

/// The one line a command that changes `domain`'s prompt matching prints afterwards ("on every config change", K2,
/// lynx's G0 ruling): that domain's test result, or `None` when none of its rules carries tests, so a config without
/// tests sees no change. It never changes the command's exit code.
pub fn after_change_line(config: &BaseConfig, cwd: &Path, domain: &str) -> Option<String> {
    after_change_lines(config, cwd, &[domain.to_string()]).into_iter().next()
}

/// [`after_change_line`] for several domains over one load, one line per domain that has tests.
pub fn after_change_lines(config: &BaseConfig, cwd: &Path, domains: &[String]) -> Vec<String> {
    if !any_tests(cwd) {
        return Vec::new();
    }
    let bench = Bench::load(config, cwd);
    let mut out = Vec::new();
    for domain in domains {
        let Ok(report) = bench.run(&Filter::Domain(domain.clone())) else { continue };
        if report.tested.is_empty() {
            continue;
        }
        let flag = if report.failed() { " (FAILS)" } else { "" };
        out.push(format!("rule tests, {domain}: {}{flag} · base rule test --domain {domain}", report.summary()));
    }
    out
}

/// `2 fires-on · 1 quiet-on`: what a rule's tests hold, for `rule add`, `rule update` and `rule list`.
pub fn tests_line(t: &RuleTests) -> String {
    format!("{} fires-on · {} quiet-on", t.fires_on.len(), t.quiet_on.len())
}

/// Refuse a test list over K2a's caps (3 `fires_on`, 2 `quiet_on`), naming the rule and how to start over, and a
/// prompt in both lists, which no config can ever pass.
pub fn check_caps(rule: &str, tests: &RuleTests) -> Result<(), String> {
    if let Some(p) = tests.fires_on.iter().find(|p| tests.quiet_on.contains(p)) {
        return Err(format!("{rule}: \"{p}\" cannot be both a --fires-on and a --quiet-on prompt"));
    }
    let over = |n: usize, max: usize, what: &str| {
        (n > max).then(|| format!("{rule} would hold {n} {what} prompts; a rule holds at most {max}"))
    };
    match over(tests.fires_on.len(), rules::MAX_FIRES_ON, "--fires-on")
        .or_else(|| over(tests.quiet_on.len(), rules::MAX_QUIET_ON, "--quiet-on"))
    {
        Some(msg) => Err(format!("{msg} (--clear-tests starts the lists over)")),
        None => Ok(()),
    }
}

/// Whether any rule anywhere carries test prompts: a domains.toml rule with `fires_on` or `quiet_on`, or a test literal
/// in either tier's graph file. File reads and a byte search only, no store load and no sync, so a command that changed
/// a domain pays for [`Bench::load`] only when there are tests to run. It can say yes when the only literals left are
/// on a rule that was removed; the full run then finds nothing and prints nothing.
pub fn any_tests(cwd: &Path) -> bool {
    if crate::domain::load_domains(cwd).iter().flat_map(|d| &d.rules).any(|r| {
        let (f, q) = r.tests();
        !f.is_empty() || !q.is_empty()
    }) {
        return true;
    }
    let graphs = [
        crate::config::find_workspace_base(cwd).map(|b| b.join("graph.nq")),
        crate::home::home_root().map(|h| h.join(".base-gbl").join(".base").join("graph.nq")),
    ];
    // The predicate IRI's tail, `firesOn>`, under whatever namespace the store uses. A literal that happens to contain
    // it costs one full load and nothing else.
    graphs.into_iter().flatten().any(|g| {
        std::fs::read_to_string(&g).is_ok_and(|text| rules::TEST_PREDICATES.iter().any(|p| text.contains(&format!("{p}>"))))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::rules::{Matcher, ServedRule};

    fn domain(name: &str, mode: &str, keywords: &[&str]) -> DomainDef {
        let toml = format!(
            "name = \"{name}\"\nmode = \"{mode}\"\nprompt_keywords = [{}]\n",
            keywords.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", ")
        );
        toml::from_str(&toml).expect("a domain")
    }

    fn rule(domain: &str, text: &str) -> RuleRef {
        RuleRef { id: rules::rule_id(domain, text), domain: domain.into(), text: text.into() }
    }

    fn stored(r: &RuleRef, fires_on: &[&str], quiet_on: &[&str]) -> (String, StoredTests) {
        let tests = RuleTests {
            fires_on: fires_on.iter().map(|s| s.to_string()).collect(),
            quiet_on: quiet_on.iter().map(|s| s.to_string()).collect(),
        };
        (r.id.clone(), StoredTests { domain: r.domain.clone(), text: r.text.clone(), tests })
    }

    fn converted(domain: &str, text: &str, matchers: Vec<Matcher>) -> Converted {
        Converted {
            rule: ServedRule {
                id: rules::rule_id(domain, text),
                domain: domain.into(),
                text: text.into(),
                rationale: None,
                rendered: text.into(),
                content_hash: rules::content_hash(text),
                iri: None,
            },
            matchers,
        }
    }

    #[test]
    fn a_keyword_domain_rule_misses_until_its_keyword_is_there() {
        let config = BaseConfig::default();
        let r = rule("base", "Measure twice, cut once.");
        let tests: HashMap<_, _> =
            [stored(&r, &["the user prompt submit is cut off"], &["what reminders do i need"])].into_iter().collect();
        let by_domain = vec![("GLOBAL".to_string(), Vec::new()), ("base".to_string(), vec![r.clone()])];
        let without = vec![domain("GLOBAL", "always", &[]), domain("base", "triggered", &[])];
        let bench = Bench::from_parts(&config, without, Vec::new(), Vec::new(), TriggerContext::default(), by_domain.clone(), tests.clone());
        let report = bench.run(&Filter::Domain("base".into())).unwrap();
        assert_eq!((report.misses(), report.false_fires()), (1, 0), "{report:?}");
        let why = &report.tested[0].outcomes[0].verdict.why;
        assert_eq!(why, "matched: GLOBAL(always) · base: has no prompt keywords");

        let with = vec![domain("GLOBAL", "always", &[]), domain("base", "triggered", &["user prompt submit"])];
        let bench = Bench::from_parts(&config, with, Vec::new(), Vec::new(), TriggerContext::default(), by_domain, tests);
        let report = bench.run(&Filter::Domain("base".into())).unwrap();
        assert!(!report.failed(), "{}", report.render());
    }

    #[test]
    fn a_rule_with_matchers_is_judged_by_select_and_never_by_its_domain() {
        let config = BaseConfig::default();
        let c = converted("base", "Relay pings use line breaks.", vec![Matcher::for_topic(vec!["relay ping".into()])]);
        let r = RuleRef { id: c.rule.id.clone(), domain: "base".into(), text: c.rule.text.clone() };
        // The domain's keyword is in the quiet prompt, and the rule still stays quiet: a converted rule leaves the
        // domain block. Its own phrase fires it.
        let tests: HashMap<_, _> = [stored(&r, &["send a relay ping to the orchestrator"], &["base status please"])].into_iter().collect();
        let domains = vec![domain("base", "triggered", &["base"])];
        let bench = Bench::from_parts(&config, domains, vec![c], Vec::new(), TriggerContext::default(), vec![("base".into(), Vec::new())], tests);
        let report = bench.run(&Filter::All).unwrap();
        assert!(!report.failed(), "{}", report.render());
        assert!(report.tested[0].outcomes[0].verdict.why.starts_with("topic score"), "{report:?}");
    }

    #[test]
    fn a_star_command_passes_every_rule_by() {
        let config = BaseConfig::default();
        let r = rule("GLOBAL", "Always on.");
        let tests: HashMap<_, _> = [stored(&r, &["*audit the hooks"], &[])].into_iter().collect();
        let commands = vec![CommandDef { name: "audit".into(), description: String::new(), rules: vec!["look hard".into()] }];
        let bench = Bench::from_parts(
            &config,
            vec![domain("GLOBAL", "always", &[])],
            Vec::new(),
            commands,
            TriggerContext::default(),
            vec![("GLOBAL".into(), vec![r])],
            tests,
        );
        let report = bench.run(&Filter::All).unwrap();
        assert_eq!(report.misses(), 1);
        assert_eq!(report.tested[0].outcomes[0].verdict.why, "star command *audit passes every rule by");
    }

    #[test]
    fn caps_refuse_past_three_and_two() {
        let t = |f: usize, q: usize| RuleTests { fires_on: vec!["x".into(); f], quiet_on: vec!["y".into(); q] };
        assert!(check_caps("base.1234", &t(3, 2)).is_ok());
        assert!(check_caps("base.1234", &t(0, 0)).is_ok(), "no minimum");
        assert!(check_caps("base.1234", &t(4, 0)).unwrap_err().contains("at most 3"));
        assert!(check_caps("base.1234", &t(1, 3)).unwrap_err().contains("at most 2"));
        let both = RuleTests { fires_on: vec!["same".into()], quiet_on: vec!["same".into()] };
        assert!(check_caps("base.1234", &both).unwrap_err().contains("cannot be both"));
    }
}
