//! Usage signals (K8, BO-19; D6, F14c): per rule and per decision, read from the match log (BO-13) and the correction
//! signals in it (BO-15): how often it was served, how often a correction followed, when it was last served. `base
//! doctor` lists what needs attention (dead, noisy and ignored rules, decisions to review, the correction detector's
//! record) and `base rule stats` prints the numbers for every rule.
//!
//! SERVED MEANS PRINTED (D15). A rule or decision counts as served only where a row lists it under `served`: what the
//! hook printed whole. A `cut` entry never counts, nor a BM25 score: the reader does not even build `scores`
//! ([`LeanRow`] has no such field, so serde passes over it).
//!
//! WHY NOISY IS NOT A PRINTED SHARE. A rule is printed once per session (`session.claim_rule`), so the share of
//! prompts it is printed on is capped by sessions over prompts: on the operator's log of 2026-10-03, 61 typed prompts
//! in about 25 sessions, so no rule could reach 25%. "One word pulls in a whole domain" is read where it happens: each
//! prompt row's `matched` list names the domain and the keyword whatever the dedup did afterwards. Only a keyword the
//! domain still has counts, so narrowing a keyword clears the line at once rather than 30 days later. Path matches are
//! not keyword breadth (D1: a session in a project folder matches it on every prompt by design; doctor's trigger
//! faults judge paths). Task notifications are left out: on that same log they were 646 of 707 prompt rows.
//!
//! CORRECTED AFTER: the same or the next turn. A serving on prompt N of a session counts when a signal row of that
//! session with `prompt_num` N or N + 1 carries C1, C2, or a C3 `UPDATED` or `CORRECTED` (`MISREAD` is a
//! misunderstanding and `DEFERRED` a disagreement, neither a rule the AI ignored). The hooks log C1 and the repeat
//! check on the prompt that carries them, an interrupt, a refusal or an edited file on the turn they happened in, and
//! a marker on the turn whose reply carried it (`corrections::on_prompt`, `on_stop`), so N and N + 1 catch each.
//!
//! ADVICE ONLY. Nothing here is one of the conjuncts of `DoctorReport::healthy`: a user who updates must not see
//! doctor go UNHEALTHY because of usage counts (Chris, 2026-10-03).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDate};
use oxigraph::store::Store;
use serde::{Deserialize, Serialize};

use crate::config::BaseConfig;
use crate::crud;
use crate::domain::DomainDef;
use crate::emit::match_log;

/// Typed prompts the window must hold before noisy is judged (lynx's G0 ruling, question 2): a new user's first three
/// prompts must not make a domain 67% noisy.
pub const NOISY_MIN_PROMPTS: usize = 100;

/// Lines each list prints before it says how many more there are.
const SHOWN: usize = 10;

/// A rule, by its id (`rules::rule_id`), or a decision, by its slug.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Rule(String),
    Decision(String),
}

// ─── Reading the log ─────────────────────────────────────────────────────────

/// A row as this reader needs it. No `scores` field: serde skips a prompt row's BM25 scores (a median of 76 per row
/// since BO-18) without building them.
#[derive(Deserialize)]
struct LeanRow {
    ts: String,
    #[serde(default)]
    session: Option<String>,
    event: String,
    #[serde(default)]
    prompt_num: Option<u32>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    matched: Vec<LeanMatched>,
    #[serde(default)]
    served: Vec<LeanItem>,
    #[serde(default)]
    cut: Vec<LeanItem>,
    #[serde(default)]
    signals: Vec<LeanSignal>,
}

#[derive(Deserialize)]
struct LeanMatched {
    domain: String,
    by: String,
    #[serde(default)]
    value: Option<String>,
}

/// A served item, or a cut one (its `reason` then set).
#[derive(Deserialize)]
struct LeanItem {
    id: String,
    kind: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
struct LeanSignal {
    layer: String,
    kind: String,
}

impl LeanItem {
    fn key(&self) -> Option<Key> {
        match self.kind.as_str() {
            "rule" => Some(Key::Rule(self.id.clone())),
            "decision" => Some(Key::Decision(self.id.clone())),
            _ => None,
        }
    }
}

/// A signal that says the user corrected the AI: C1, C2, or C3 `UPDATED` / `CORRECTED`.
fn corrects(s: &LeanSignal) -> bool {
    match s.layer.as_str() {
        "C1" | "C2" => true,
        "C3" => matches!(s.kind.as_str(), "UPDATED" | "CORRECTED"),
        _ => false,
    }
}

/// One time an item was served.
#[derive(Debug, Clone)]
struct Serving {
    day: NaiveDate,
    /// Seconds since the epoch, for a decision whose servings count only after its last update.
    at: i64,
    session: Option<u32>,
    /// The prompt it was served on; for a file row, the session's latest prompt read before it.
    turn: Option<u32>,
}

/// What the match log says, over every file of it in the tiers read.
#[derive(Debug, Default)]
pub struct Scan {
    pub today: NaiveDate,
    pub window_days: u64,
    /// The oldest row's local date. `None`: no row at all.
    pub oldest: Option<NaiveDate>,
    /// Prompt rows in the window: typed by a person, a machine's (task notifications and the like), kept with no text.
    pub typed: usize,
    pub machine: usize,
    pub textless: usize,
    servings: HashMap<Key, Vec<Serving>>,
    /// (session, turn) pairs a correcting signal was logged on.
    signals: HashSet<(u32, u32)>,
    /// Cut entries in the window, any reason but `not matched`: the item was in reach and not printed.
    withheld: HashMap<Key, usize>,
    /// The oldest day a row named the item, served or cut.
    first_named: HashMap<Key, NaiveDate>,
    /// Rows in the window that matched each domain (by slug) by anything but `always`.
    pub domain_matched: HashMap<String, usize>,
    /// Typed prompts in the window that matched each domain (by slug) by a keyword it still has.
    pub keyword_prompts: HashMap<String, usize>,
    /// The same, per keyword.
    pub keyword_hits: HashMap<String, BTreeMap<String, usize>>,
    /// Typed prompts in the window that served or withheld each rule (by id).
    pub rule_reach: HashMap<String, usize>,
    /// Typed prompts in the window, star commands aside, whose text holds one of a global decision's keywords.
    pub decision_reach: HashMap<String, usize>,
    /// The same, per keyword.
    pub decision_hits: HashMap<String, BTreeMap<String, usize>>,
}

/// What a scan compares the log against: the configuration as it is now.
#[derive(Debug, Default, Clone)]
pub struct Current {
    /// Each domain's prompt keywords, by the domain's slug.
    pub keywords: HashMap<String, Vec<String>>,
    /// Old names (a renamed domain's alias) to the slug the domain has now.
    pub aliases: HashMap<String, String>,
    /// The global decisions with keywords: slug and keywords.
    pub decisions: Vec<(String, Vec<String>)>,
}

impl Current {
    /// The slug a logged domain name has now.
    fn slug(&self, logged: &str) -> String {
        let s = crud::slugify(logged);
        self.aliases.get(&s).cloned().unwrap_or(s)
    }

    /// Read from the domains and the store, as the prompt hook would.
    pub fn from(domains: &[DomainDef], store: Option<&Store>, config: &BaseConfig) -> Self {
        let mut out = Current::default();
        for d in domains {
            let slug = crud::slugify(&d.name);
            for a in &d.aliases {
                out.aliases.insert(crud::slugify(a), slug.clone());
            }
            out.keywords.insert(slug, d.prompt_keywords.clone());
        }
        if let Some(store) = store {
            let global = crate::domain::global_decisions::GlobalDecisions::load(store, config, domains);
            out.decisions =
                global.all().filter(|d| !d.keywords.is_empty()).map(|d| (d.slug.clone(), d.keywords.clone())).collect();
        }
        out
    }
}

impl Scan {
    /// The first day of the window.
    pub fn window_start(&self) -> NaiveDate {
        self.today - chrono::Days::new(self.window_days.max(1) - 1)
    }

    fn in_window(&self, day: NaiveDate) -> bool {
        day >= self.window_start()
    }

    /// Days the log covers: today minus the oldest row's day, plus one. 0 with no row.
    pub fn days_covered(&self) -> u64 {
        self.oldest.map(|o| (self.today - o).num_days().max(0) as u64 + 1).unwrap_or(0)
    }

    /// The counts of one item, every serving counted.
    pub fn counts(&self, key: &Key) -> Counts {
        self.counts_since(key, None)
    }

    /// The counts of one item, counting servings at or after `since` (seconds since the epoch) for `served_all` and
    /// `corrected_after`: a decision reworded with `base decision update` starts again.
    pub fn counts_since(&self, key: &Key, since: Option<i64>) -> Counts {
        let mut c = Counts {
            withheld_window: self.withheld.get(key).copied().unwrap_or(0),
            first_named: self.first_named.get(key).copied(),
            ..Counts::default()
        };
        for s in self.servings.get(key).into_iter().flatten() {
            if self.in_window(s.day) {
                c.served_window += 1;
            }
            c.last_served = c.last_served.max(Some(s.day));
            if since.is_some_and(|t| s.at < t) {
                continue;
            }
            c.served_all += 1;
            if let (Some(sess), Some(n)) = (s.session, s.turn)
                && (self.signals.contains(&(sess, n)) || self.signals.contains(&(sess, n + 1)))
            {
                c.corrected_after += 1;
            }
        }
        c
    }

    /// Every decision the log shows served, by slug.
    pub fn decisions(&self) -> impl Iterator<Item = &str> {
        self.servings.keys().filter_map(|k| match k {
            Key::Decision(s) => Some(s.as_str()),
            Key::Rule(_) => None,
        })
    }
}

/// One item's numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub served_window: usize,
    pub served_all: usize,
    pub corrected_after: usize,
    pub last_served: Option<NaiveDate>,
    /// Cut in the window for any reason but `not matched`.
    pub withheld_window: usize,
    pub first_named: Option<NaiveDate>,
}

/// The local day of a row's time, and its seconds since the epoch.
fn when(ts: &str) -> Option<(NaiveDate, i64)> {
    let t = DateTime::parse_from_rfc3339(ts).ok()?;
    Some((t.with_timezone(&Local).date_naive(), t.timestamp()))
}

/// Read every file of the log in `dirs` (each tier's `.base`), oldest file first, as of `today`.
pub fn scan(dirs: &[PathBuf], today: NaiveDate, window_days: u64, current: &Current) -> Scan {
    let mut out = Scan { today, window_days: window_days.max(1), ..Scan::default() };
    let mut sessions: HashMap<String, u32> = HashMap::new();
    let mut last_prompt: HashMap<u32, u32> = HashMap::new();
    for dir in dirs {
        for path in match_log::files(dir) {
            let Ok(file) = std::fs::File::open(&path) else { continue };
            for line in std::io::BufReader::new(file).split(b'\n').map_while(Result::ok) {
                let Ok(text) = std::str::from_utf8(&line) else { continue };
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                // Most rows are tool calls that served nothing: passed over before any parse.
                if text.contains("\"event\":\"file\"") && text.contains("\"served\":[]") && !text.contains("\"matched\":[{") {
                    if let Some(day) = ts_day(text) {
                        out.oldest = Some(out.oldest.map_or(day, |o| o.min(day)));
                    }
                    continue;
                }
                let Ok(row) = serde_json::from_str::<LeanRow>(text) else { continue };
                take(&mut out, row, current, &mut sessions, &mut last_prompt);
            }
        }
    }
    out
}

/// A row's day from its `ts` field, without parsing the rest.
fn ts_day(text: &str) -> Option<NaiveDate> {
    let at = text.find("\"ts\":\"")? + 6;
    let end = text[at..].find('"')? + at;
    when(&text[at..end]).map(|(d, _)| d)
}

fn take(out: &mut Scan, row: LeanRow, current: &Current, sessions: &mut HashMap<String, u32>, last_prompt: &mut HashMap<u32, u32>) {
    let Some((day, at)) = when(&row.ts) else { return };
    out.oldest = Some(out.oldest.map_or(day, |o| o.min(day)));
    let next = sessions.len() as u32;
    let session = row.session.as_ref().map(|s| *sessions.entry(s.clone()).or_insert(next));
    let window = out.in_window(day);

    if row.event == "signal" {
        if let (Some(s), Some(n)) = (session, row.prompt_num)
            && row.signals.iter().any(corrects)
        {
            out.signals.insert((s, n));
        }
        return;
    }
    let turn = match row.event.as_str() {
        "prompt" => {
            if let (Some(s), Some(n)) = (session, row.prompt_num) {
                last_prompt.insert(s, n);
            }
            row.prompt_num
        }
        _ => session.and_then(|s| last_prompt.get(&s).copied()),
    };

    for item in &row.served {
        let Some(key) = item.key() else { continue };
        out.first_named.entry(key.clone()).and_modify(|d| *d = (*d).min(day)).or_insert(day);
        out.servings.entry(key).or_default().push(Serving { day, at, session, turn });
    }
    for item in &row.cut {
        let Some(key) = item.key() else { continue };
        out.first_named.entry(key.clone()).and_modify(|d| *d = (*d).min(day)).or_insert(day);
        if window && item.reason.as_deref() != Some("not matched") {
            *out.withheld.entry(key).or_default() += 1;
        }
    }
    if !window {
        return;
    }
    for m in row.matched.iter().filter(|m| m.by != "always") {
        *out.domain_matched.entry(current.slug(&m.domain)).or_default() += 1;
    }
    if row.event != "prompt" {
        return;
    }
    let text = match &row.text {
        None => {
            out.textless += 1;
            return;
        }
        Some(t) if crate::domain::transcript::machine_prompt(t) => {
            out.machine += 1;
            return;
        }
        Some(t) => t,
    };
    out.typed += 1;
    // Keyword breadth: each domain once per prompt, each keyword once per prompt, only keywords the domain still has.
    let mut domains: HashSet<String> = HashSet::new();
    let mut hits: HashSet<(String, String)> = HashSet::new();
    for m in row.matched.iter().filter(|m| m.by == "keyword") {
        let slug = current.slug(&m.domain);
        let Some(kw) = m.value.as_deref().map(str::trim) else { continue };
        let still = current.keywords.get(&slug).is_some_and(|list| list.iter().any(|k| k.trim().eq_ignore_ascii_case(kw)));
        if still {
            hits.insert((slug.clone(), kw.to_lowercase()));
            domains.insert(slug);
        }
    }
    for d in domains {
        *out.keyword_prompts.entry(d).or_default() += 1;
    }
    for (d, kw) in hits {
        *out.keyword_hits.entry(d).or_default().entry(kw).or_default() += 1;
    }
    // A rule in reach on this prompt: served, or cut by anything but its matchers not firing.
    let reach: HashSet<&str> = row
        .served
        .iter()
        .chain(row.cut.iter().filter(|c| c.reason.as_deref() != Some("not matched")))
        .filter(|i| i.kind == "rule")
        .map(|i| i.id.as_str())
        .collect();
    for id in reach {
        *out.rule_reach.entry(id.to_string()).or_default() += 1;
    }
    // A global decision on its own keywords, as BO-16's replay judges one: a star command passes every rule by.
    if !row.matched.iter().any(|m| m.by == "command") {
        for (slug, kws) in &current.decisions {
            let hit: Vec<&String> =
                kws.iter().filter(|k| crate::domain::global_decisions::keyword_hit(std::slice::from_ref(*k), text)).collect();
            if hit.is_empty() {
                continue;
            }
            *out.decision_reach.entry(slug.clone()).or_default() += 1;
            let per = out.decision_hits.entry(slug.clone()).or_default();
            for k in hit {
                *per.entry(k.trim().to_lowercase()).or_default() += 1;
            }
        }
    }
}

// ─── What the rules and decisions are now ────────────────────────────────────

/// One rule as the hooks would serve it now.
#[derive(Debug, Clone)]
pub struct RuleNow {
    pub id: String,
    pub domain: String,
    pub text: String,
    /// It has matchers of its own (`select` serves it, not its domain's triggers).
    pub own_matchers: bool,
    /// Its domain is always on.
    pub always: bool,
    /// Its domain injects (`auto_inject`): a rule of one that does not is off, not dead.
    pub injects: bool,
}

impl RuleNow {
    /// `<domain>.<first 8 of id>`, as `base rule list` and `base rule test` take it.
    pub fn short(&self) -> String {
        crate::domain::rule_test::short_ref(&self.domain, &self.id)
    }
}

/// Every rule of every domain, once each, in the domains' order.
pub fn rules_now(domains: &[DomainDef], store: Option<&Store>, config: &BaseConfig) -> Vec<RuleNow> {
    let converted: HashSet<String> =
        crate::domain::rules::rules_with_matchers(store, config, domains).into_iter().map(|c| c.rule.id).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for d in domains {
        for r in crate::domain::rules::rules_for_domain(store, config, d) {
            if !seen.insert(r.id.clone()) {
                continue;
            }
            out.push(RuleNow {
                own_matchers: converted.contains(&r.id),
                id: r.id,
                domain: d.name.clone(),
                text: r.text,
                always: d.is_always(),
                injects: d.auto_inject,
            });
        }
    }
    out
}

/// A decision's name and dates, read from the store.
#[derive(Debug, Clone, Default)]
pub struct DecisionNow {
    pub name: String,
    /// Its `updatedAt`, else its `createdAt`.
    pub changed: Option<DateTime<chrono::FixedOffset>>,
    pub updated: Option<DateTime<chrono::FixedOffset>>,
    pub superseded: bool,
}

/// The decisions among `slugs` the store holds, by slug.
pub fn decisions_now(store: &Store, config: &BaseConfig, slugs: &[&str]) -> HashMap<String, DecisionNow> {
    let mut out: HashMap<String, DecisionNow> = HashMap::new();
    if slugs.is_empty() {
        return out;
    }
    let ns = &config.namespace;
    let p = &ns.prefix;
    let values: String = slugs.iter().map(|s| format!("<{}> ", crud::build_iri(ns, "decision", s))).collect();
    let q = format!(
        "{}\nSELECT ?d ?name ?created ?updated ?status ?by WHERE {{\n\
           VALUES ?d {{ {values}}}\n\
           GRAPH ?g {{ ?d a {p}:Decision }}\n\
           OPTIONAL {{ GRAPH ?g1 {{ ?d {p}:name ?name }} }}\n\
           OPTIONAL {{ GRAPH ?g2 {{ ?d {p}:createdAt ?created }} }}\n\
           OPTIONAL {{ GRAPH ?g3 {{ ?d {p}:updatedAt ?updated }} }}\n\
           OPTIONAL {{ GRAPH ?g4 {{ ?d {p}:status ?status }} }}\n\
           OPTIONAL {{ GRAPH ?g5 {{ ?d {p}:{} ?by }} }}\n\
         }}",
        crud::prefixes(ns),
        crate::supersede::PRED_SUPERSEDED_BY,
    );
    let Ok(oxigraph::sparql::QueryResults::Solutions(rows)) = crate::store::query(store, &q) else { return out };
    let date = |s: Option<String>| s.and_then(|v| DateTime::parse_from_rfc3339(v.trim()).ok());
    for row in rows.filter_map(Result::ok) {
        let lit = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
        let Some(id) = lit("d").map(|d| crud::slug_of(&d)) else { continue };
        let e = out.entry(id).or_default();
        if e.name.is_empty() {
            e.name = lit("name").unwrap_or_default();
        }
        let created = date(lit("created"));
        let updated = date(lit("updated"));
        e.updated = e.updated.max(updated);
        e.changed = e.changed.max(updated.or(created)).max(e.updated);
        e.superseded |= lit("by").is_some() || lit("status").as_deref() == Some(crate::supersede::STATUS_SUPERSEDED);
    }
    out
}

// ─── The doctor section ──────────────────────────────────────────────────────

/// How much the log covers, as the section's first line says it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogSpan {
    pub days: u64,
    pub since: String,
    pub typed: usize,
    pub machine: usize,
    pub textless: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Dead {
    /// An injecting domain that matched nothing in the window: every rule of it went unserved.
    Domain { domain: String, rules: usize },
    Rule {
        rule: String,
        domain: String,
        text: String,
        last_served: Option<String>,
        withheld: usize,
        own_matchers: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Noisy {
    Domain { domain: String, prompts: usize, of: usize, keywords: Vec<(String, usize)>, rules: usize },
    Rule { rule: String, text: String, prompts: usize, of: usize },
    Decision { decision: String, name: String, prompts: usize, of: usize, keywords: Vec<(String, usize)> },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ignored {
    /// `rule` or `decision`.
    pub kind: String,
    /// The rule's `<domain>.<id>` or the decision's slug.
    pub id: String,
    pub text: String,
    pub served: usize,
    pub corrected: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Review {
    pub decision: String,
    pub name: String,
    pub served: usize,
    /// Days since its last update or its creation; `None` when it carries neither date.
    pub unchanged_days: Option<i64>,
}

/// The correction detector's record over the window (BO-17's backstop, K8c).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DetectorLine {
    pub judged: usize,
    pub corrections: usize,
    pub misses: usize,
}

/// The thresholds the section was judged at, so the text can name them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Limits {
    pub dead_days: u64,
    pub broad_share: f32,
    pub ignored_after: usize,
    pub review_served: usize,
    pub review_days: i64,
    pub noisy_min_prompts: usize,
}

impl Default for Limits {
    fn default() -> Self {
        let dc = crate::config::DoctorConfig::default();
        Limits {
            dead_days: dc.dead_days,
            broad_share: crate::config::TuneConfig::default().broad_share,
            ignored_after: dc.ignored_after,
            review_served: dc.review_served,
            review_days: dc.review_days,
            noisy_min_prompts: NOISY_MIN_PROMPTS,
        }
    }
}

/// `base doctor`'s rules-and-decisions section. Advice only: never read by `DoctorReport::healthy`. The default is the
/// section of a store with no match log yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Section {
    /// `None`: no match log yet.
    pub log: Option<LogSpan>,
    /// `None`: not judged, the log covers fewer days than `dead_days`.
    pub dead: Option<Vec<Dead>>,
    /// Rules not served and never named in the log: they did not exist when their domain last matched.
    pub too_new: usize,
    /// `None`: not judged, fewer typed prompts in the window than [`NOISY_MIN_PROMPTS`].
    pub noisy: Option<Vec<Noisy>>,
    pub ignored: Vec<Ignored>,
    pub review: Vec<Review>,
    pub detector: DetectorLine,
    pub limits: Limits,
}

/// What a section is built from: the log scanned against the configuration as it is now.
pub struct Inputs<'a> {
    pub config: &'a BaseConfig,
    pub domains: &'a [DomainDef],
    pub store: Option<&'a Store>,
    pub scan: &'a Scan,
    pub detector: crate::corrections::tune_pass::Detector,
    pub now: DateTime<Local>,
}

/// The tiers whose logs a cwd's sessions write to: its own and the global one.
pub fn log_dirs(cwd: &Path) -> Vec<PathBuf> {
    crate::corrections::propose::row_dirs(cwd)
}

/// Scan `cwd`'s logs and build the section. Read only: the store is the caller's, loaded with `store::load_merged`.
pub fn section_for(cwd: &Path, config: &BaseConfig, domains: &[DomainDef], store: Option<&Store>) -> Section {
    let now = Local::now();
    let current = Current::from(domains, store, config);
    let scan = scan(&log_dirs(cwd), now.date_naive(), config.doctor.dead_days, &current);
    let since = scan.window_start();
    let detector = crate::corrections::tune_pass::detector_totals(Some(since));
    build(&Inputs { config, domains, store, scan: &scan, detector, now })
}

pub fn build(i: &Inputs) -> Section {
    let dc = &i.config.doctor;
    let limits = Limits {
        dead_days: dc.dead_days.max(1),
        broad_share: i.config.tune.broad_share,
        ignored_after: dc.ignored_after.max(1),
        review_served: dc.review_served.max(1),
        review_days: dc.review_days,
        noisy_min_prompts: NOISY_MIN_PROMPTS,
    };
    let scan = i.scan;
    let detector = DetectorLine { judged: i.detector.judged, corrections: i.detector.corrections, misses: i.detector.misses };
    let Some(oldest) = scan.oldest else {
        return Section { log: None, dead: None, too_new: 0, noisy: None, ignored: Vec::new(), review: Vec::new(), detector, limits };
    };
    let log = LogSpan {
        days: scan.days_covered(),
        since: oldest.format("%Y-%m-%d").to_string(),
        typed: scan.typed,
        machine: scan.machine,
        textless: scan.textless,
    };
    let rules = rules_now(i.domains, i.store, i.config);
    let start = scan.window_start();

    // Dead (rules only): judged once the log covers `dead_days`.
    let (dead, too_new) = if log.days < limits.dead_days {
        (None, 0)
    } else {
        let mut by_domain: Vec<(String, usize)> = Vec::new();
        let mut lines: Vec<Dead> = Vec::new();
        let mut too_new = 0;
        for r in rules.iter().filter(|r| r.injects) {
            let c = scan.counts(&Key::Rule(r.id.clone()));
            if c.served_window > 0 {
                continue;
            }
            let reached = r.always || r.own_matchers || scan.domain_matched.get(&crud::slugify(&r.domain)).is_some_and(|n| *n > 0);
            if !reached {
                match by_domain.iter_mut().find(|(d, _)| *d == r.domain) {
                    Some((_, n)) => *n += 1,
                    None => by_domain.push((r.domain.clone(), 1)),
                }
                continue;
            }
            let named_before = c.first_named.is_some_and(|d| d < start);
            if r.own_matchers || c.withheld_window > 0 || named_before {
                lines.push(Dead::Rule {
                    rule: r.short(),
                    domain: r.domain.clone(),
                    text: r.text.clone(),
                    last_served: c.last_served.map(|d| d.format("%Y-%m-%d").to_string()),
                    withheld: c.withheld_window,
                    own_matchers: r.own_matchers,
                });
            } else {
                too_new += 1;
            }
        }
        let mut dead: Vec<Dead> = by_domain.into_iter().map(|(domain, rules)| Dead::Domain { domain, rules }).collect();
        dead.extend(lines);
        (Some(dead), too_new)
    };

    // Noisy: judged once the window holds enough typed prompts.
    let of = scan.typed;
    let over = |n: usize| of > 0 && n as f32 / of as f32 > limits.broad_share;
    let noisy = (of >= NOISY_MIN_PROMPTS).then(|| {
        let mut out: Vec<Noisy> = Vec::new();
        for d in i.domains.iter().filter(|d| d.auto_inject && !d.is_always()) {
            let slug = crud::slugify(&d.name);
            let n = scan.keyword_prompts.get(&slug).copied().unwrap_or(0);
            if !over(n) {
                continue;
            }
            let mut keywords: Vec<(String, usize)> =
                scan.keyword_hits.get(&slug).map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect()).unwrap_or_default();
            keywords.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let count = rules.iter().filter(|r| crud::slugify(&r.domain) == slug && !r.own_matchers).count();
            out.push(Noisy::Domain { domain: d.name.clone(), prompts: n, of, keywords, rules: count });
        }
        for r in rules.iter().filter(|r| r.own_matchers && r.injects) {
            let n = scan.rule_reach.get(&r.id).copied().unwrap_or(0);
            if over(n) {
                out.push(Noisy::Rule { rule: r.short(), text: r.text.clone(), prompts: n, of });
            }
        }
        let global = i.store.map(|s| crate::domain::global_decisions::GlobalDecisions::load(s, i.config, i.domains));
        for (slug, n) in &scan.decision_reach {
            if over(*n) {
                let name = global.as_ref().and_then(|g| g.by_slug(slug)).map(|d| d.name.clone()).unwrap_or_default();
                let mut keywords: Vec<(String, usize)> = scan
                    .decision_hits
                    .get(slug)
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())
                    .unwrap_or_default();
                keywords.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                out.push(Noisy::Decision { decision: slug.clone(), name, prompts: *n, of, keywords });
            }
        }
        out.sort_by(|a, b| noisy_n(b).cmp(&noisy_n(a)).then_with(|| noisy_name(a).cmp(noisy_name(b))));
        out
    });

    // Decisions: their names and dates, for the ignored and review lists.
    let decision_slugs: Vec<&str> = {
        let mut v: Vec<&str> = scan.decisions().collect();
        v.sort();
        v
    };
    let now_decisions = i.store.map(|s| decisions_now(s, i.config, &decision_slugs)).unwrap_or_default();

    // Ignored: rules and decisions, over the whole log.
    let mut ignored: Vec<Ignored> = Vec::new();
    for r in &rules {
        let c = scan.counts(&Key::Rule(r.id.clone()));
        if c.corrected_after >= limits.ignored_after {
            ignored.push(Ignored { kind: "rule".into(), id: r.short(), text: r.text.clone(), served: c.served_all, corrected: c.corrected_after });
        }
    }
    for slug in &decision_slugs {
        let Some(d) = now_decisions.get(*slug).filter(|d| !d.superseded) else { continue };
        let c = scan.counts_since(&Key::Decision(slug.to_string()), d.updated.map(|t| t.timestamp()));
        if c.corrected_after >= limits.ignored_after {
            ignored.push(Ignored { kind: "decision".into(), id: slug.to_string(), text: d.name.clone(), served: c.served_all, corrected: c.corrected_after });
        }
    }
    ignored.sort_by(|a, b| b.corrected.cmp(&a.corrected).then_with(|| a.id.cmp(&b.id)));

    // Review (F14c): decisions served often and not changed in a long time.
    let mut review: Vec<Review> = Vec::new();
    for slug in &decision_slugs {
        let Some(d) = now_decisions.get(*slug).filter(|d| !d.superseded) else { continue };
        let c = scan.counts(&Key::Decision(slug.to_string()));
        if c.served_all < limits.review_served {
            continue;
        }
        let unchanged = d.changed.map(|t| (i.now.date_naive() - t.with_timezone(&Local).date_naive()).num_days());
        if unchanged.is_none_or(|days| days >= limits.review_days) {
            review.push(Review { decision: slug.to_string(), name: d.name.clone(), served: c.served_all, unchanged_days: unchanged });
        }
    }
    review.sort_by(|a, b| b.served.cmp(&a.served).then_with(|| a.decision.cmp(&b.decision)));

    Section { log: Some(log), dead, too_new, noisy, ignored, review, detector, limits }
}

fn noisy_n(n: &Noisy) -> usize {
    match n {
        Noisy::Domain { prompts, .. } | Noisy::Rule { prompts, .. } | Noisy::Decision { prompts, .. } => *prompts,
    }
}

fn noisy_name(n: &Noisy) -> &str {
    match n {
        Noisy::Domain { domain, .. } => domain,
        Noisy::Rule { rule, .. } => rule,
        Noisy::Decision { decision, .. } => decision,
    }
}

// ─── The next steps the section prints ───────────────────────────────────────

/// A command-line argument as a shell reads it back: double-quoted when it holds a space or a quote.
fn arg(s: &str) -> String {
    if !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') {
        return s.to_string();
    }
    format!("\"{}\"", s.replace('"', "\\\""))
}

/// What a list past its first lines points to.
pub const CMD_DOCTOR_JSON: &str = "base doctor --json";
/// What runs a rule pass, which the detector line counts.
pub const CMD_TUNE: &str = "base tune";

pub fn cmd_test_domain(domain: &str) -> String {
    format!("base rule test --domain {}", arg(domain))
}

pub fn cmd_test_rule(rule: &str) -> String {
    format!("base rule test --rule {}", arg(rule))
}

pub fn cmd_hooks_show(block: &str) -> String {
    format!("base hooks show {}", arg(block))
}

pub fn cmd_replay_drop(domain: &str, keyword: &str) -> String {
    format!("base rule replay --domain {} --drop-keyword {}", arg(domain), arg(keyword))
}

pub fn cmd_replay_decision_drop(slug: &str, keyword: &str) -> String {
    format!("base rule replay --decision {} --drop-keyword {}", arg(slug), arg(keyword))
}

pub fn cmd_propose_rewrite(rule: &str) -> String {
    format!("base rule propose --rule {} --text \"...\"", arg(rule))
}

pub fn cmd_decision_update(slug: &str) -> String {
    format!("base decision update {} --name \"...\"", arg(slug))
}

pub fn cmd_decision_supersede(slug: &str) -> String {
    let domain = slug.split_once('.').map(|(d, _)| d).unwrap_or("GLOBAL");
    format!("base decision log --domain {} --decision \"...\" --rationale \"...\" --supersedes {}", arg(domain), arg(slug))
}

/// One of each next step the section and `base rule stats` print, built by the functions above on invented names: the
/// CLI's own parser reads each in the binary's tests (lynx's G0 condition).
pub fn next_step_examples() -> Vec<String> {
    vec![
        cmd_test_domain("tools"),
        cmd_test_domain("two words"),
        cmd_test_rule("tools.1a2b3c4d"),
        cmd_hooks_show("tools-rules"),
        cmd_replay_drop("tools", "hook"),
        cmd_replay_drop("tools", "user prompt submit"),
        cmd_replay_decision_drop("global.keep-notes-short", "notes"),
        cmd_propose_rewrite("tools.1a2b3c4d"),
        cmd_decision_update("global.keep-notes-short"),
        cmd_decision_supersede("global.keep-notes-short"),
        CMD_DOCTOR_JSON.to_string(),
        CMD_TUNE.to_string(),
    ]
}

// ─── Rendering ───────────────────────────────────────────────────────────────

/// `1,257`.
fn thousands(n: usize) -> String {
    crate::emit::prompt::thousands(n)
}

fn plural<'s>(n: usize, one: &'s str, many: &'s str) -> &'s str {
    if n == 1 { one } else { many }
}

/// `text` on one line, cut to `max` characters with `...`.
fn clip(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let head: String = one.chars().take(max).collect();
    format!("{}...", head.trim_end())
}

fn percent(n: usize, of: usize) -> String {
    if of == 0 { "0%".into() } else { format!("{:.0}%", n as f64 * 100.0 / of as f64) }
}

/// `   +31 more · base doctor --json lists them all`.
fn more_line(out: &mut String, total: usize) {
    if total > SHOWN {
        out.push_str(&format!("     +{} more · {CMD_DOCTOR_JSON} lists them all\n", total - SHOWN));
    }
}

/// The section as `base doctor` prints it.
pub fn render(s: &Section) -> String {
    let l = &s.limits;
    let mut out = String::from("\n─── rules and decisions, from the match log ───\n");
    let Some(log) = &s.log else {
        out.push_str("   no match log yet: base writes it from the next prompt on, and this section fills in as it grows\n");
        return out;
    };
    out.push_str(&format!(
        "   match log: {} {} (since {}) · {} typed {}, {} task {} · {} with no text\n",
        log.days,
        plural(log.days as usize, "day", "days"),
        log.since,
        thousands(log.typed),
        plural(log.typed, "prompt", "prompts"),
        thousands(log.machine),
        plural(log.machine, "notification", "notifications"),
        thousands(log.textless),
    ));

    match &s.dead {
        None => out.push_str(&format!(
            "   dead (not served in {d} days): not judged yet · the match log covers {n} {day}, fewer than [doctor] dead_days = {d}\n",
            d = l.dead_days,
            n = log.days,
            day = plural(log.days as usize, "day", "days"),
        )),
        Some(dead) => {
            let rules: usize = dead.iter().map(|d| match d { Dead::Domain { rules, .. } => *rules, Dead::Rule { .. } => 1 }).sum();
            let new = if s.too_new > 0 { format!(" · {} more first seen under {} days ago", s.too_new, l.dead_days) } else { String::new() };
            out.push_str(&format!("   dead (not served in {} days): {rules}{new}\n", l.dead_days));
            for d in dead.iter().take(SHOWN) {
                out.push_str(&dead_line(d, l.dead_days));
            }
            more_line(&mut out, dead.len());
        }
    }

    match &s.noisy {
        None => out.push_str(&format!(
            "   noisy: not judged yet · {} typed {} in the last {} days, fewer than {}\n",
            thousands(log.typed),
            plural(log.typed, "prompt", "prompts"),
            l.dead_days,
            l.noisy_min_prompts
        )),
        Some(noisy) => {
            out.push_str(&format!(
                "   noisy (matched by keyword on more than {:.0}% of typed prompts, last {} days): {}\n",
                l.broad_share * 100.0,
                l.dead_days,
                noisy.len()
            ));
            for n in noisy.iter().take(SHOWN) {
                out.push_str(&noisy_line(n));
            }
            more_line(&mut out, noisy.len());
        }
    }

    out.push_str(&format!("   ignored (served, then corrected {}+ times): {}\n", l.ignored_after, s.ignored.len()));
    for i in s.ignored.iter().take(SHOWN) {
        let next = match i.kind.as_str() {
            "decision" => format!("reword it: {}", cmd_decision_update(&i.id)),
            _ => format!("reword it: {}", cmd_propose_rewrite(&i.id)),
        };
        out.push_str(&format!(
            "     {} \"{}\"   served {} · corrected after {} · {next}\n",
            i.id,
            clip(&i.text, 40),
            i.served,
            i.corrected
        ));
    }
    more_line(&mut out, s.ignored.len());

    out.push_str(&format!("   review (served {}+ times, unchanged {}+ days): {}\n", l.review_served, l.review_days, s.review.len()));
    for r in s.review.iter().take(SHOWN) {
        let age = match r.unchanged_days {
            Some(d) => format!("unchanged {d} {}", plural(d as usize, "day", "days")),
            None => "no date on record".to_string(),
        };
        out.push_str(&format!(
            "     {} \"{}\"   served {} · {age} · still true? {} · or replace it: {}\n",
            r.decision,
            clip(&r.name, 40),
            r.served,
            cmd_decision_update(&r.decision),
            cmd_decision_supersede(&r.decision)
        ));
    }
    more_line(&mut out, s.review.len());

    out.push_str(&detector_line(&s.detector, l.dead_days));
    out
}

fn dead_line(d: &Dead, days: u64) -> String {
    match d {
        Dead::Domain { domain, rules } => format!(
            "     domain {domain} · not matched in {days} days · its {rules} {} not served · check its triggers: {}\n",
            plural(*rules, "rule", "rules"),
            cmd_test_domain(domain)
        ),
        Dead::Rule { rule, domain, text, last_served, withheld, own_matchers } => {
            let last = match last_served {
                Some(d) => format!("last {d}"),
                None => "never served in the log".to_string(),
            };
            let next = if *withheld > 0 {
                format!(
                    "withheld {withheld} {} by the budget: {}",
                    plural(*withheld, "time", "times"),
                    cmd_hooks_show(&format!("{}-rules", crud::slugify(domain)))
                )
            } else if *own_matchers {
                format!("its own matchers did not fire: {}", cmd_test_rule(rule))
            } else {
                format!("check its triggers: {}", cmd_test_domain(domain))
            };
            format!("     {rule} \"{}\"   0 served · {last} · {next}\n", clip(text, 40))
        }
    }
}

fn noisy_line(n: &Noisy) -> String {
    match n {
        Noisy::Domain { domain, prompts, of, keywords, rules } => {
            let by: Vec<String> = keywords.iter().take(3).map(|(k, c)| format!("{k} {}", percent(*c, *of))).collect();
            let next = match keywords.first() {
                Some((k, _)) => format!("narrow its keywords: {}", cmd_replay_drop(domain, k)),
                None => format!("check its triggers: {}", cmd_test_domain(domain)),
            };
            format!(
                "     domain {domain} · {} ({} of {}) · by keyword {} · its {rules} {} · {next}\n",
                percent(*prompts, *of),
                thousands(*prompts),
                thousands(*of),
                by.join(", "),
                plural(*rules, "rule", "rules")
            )
        }
        Noisy::Rule { rule, text, prompts, of } => format!(
            "     {rule} \"{}\"   {} ({} of {}) · narrow its words: {}\n",
            clip(text, 40),
            percent(*prompts, *of),
            thousands(*prompts),
            thousands(*of),
            cmd_test_rule(rule)
        ),
        Noisy::Decision { decision, name, prompts, of, keywords } => {
            let by: Vec<String> = keywords.iter().take(3).map(|(k, c)| format!("{k} {}", percent(*c, *of))).collect();
            let next = match keywords.first() {
                Some((k, _)) => format!(" · narrow its keywords: {}", cmd_replay_decision_drop(decision, k)),
                None => String::new(),
            };
            format!(
                "     {decision} \"{}\"   {} ({} of {}) · by keyword {}{next}\n",
                clip(name, 40),
                percent(*prompts, *of),
                thousands(*prompts),
                thousands(*of),
                by.join(", ")
            )
        }
    }
}

/// K8c: `correction detector, last 30 days: 15 caught · 11 missed (found by the backstop) · 58% caught`.
fn detector_line(d: &DetectorLine, days: u64) -> String {
    if d.judged == 0 {
        return format!("   correction detector: no rule pass in the last {days} days ({CMD_TUNE} runs one)\n");
    }
    if d.corrections == 0 {
        return format!(
            "   correction detector, last {days} days: {} {} judged, no correction found\n",
            thousands(d.judged),
            plural(d.judged, "turn", "turns")
        );
    }
    let caught = d.corrections.saturating_sub(d.misses);
    format!(
        "   correction detector, last {days} days: {caught} caught · {} missed (found by the backstop) · {} caught\n",
        d.misses,
        percent(caught, d.corrections)
    )
}

// ─── base rule stats ─────────────────────────────────────────────────────────

/// One rule's line of `base rule stats`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatsRow {
    pub rule: String,
    pub domain: String,
    pub id: String,
    pub served_window: usize,
    pub window_days: u64,
    pub served_all: usize,
    pub corrected_after: usize,
    pub last_served: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    /// `None`: no match log yet.
    pub log: Option<LogSpan>,
    pub window_days: u64,
    pub rules: Vec<StatsRow>,
}

/// `base rule stats`: every rule of every domain `domains` holds (or only `domain`'s), with its numbers.
pub fn stats(scan: &Scan, rules: &[RuleNow], domain: Option<&str>) -> Stats {
    let want = domain.map(crud::slugify);
    let rows = rules
        .iter()
        .filter(|r| want.as_deref().is_none_or(|w| crud::slugify(&r.domain) == w))
        .map(|r| {
            let c = scan.counts(&Key::Rule(r.id.clone()));
            StatsRow {
                rule: r.short(),
                domain: r.domain.clone(),
                id: r.id.clone(),
                served_window: c.served_window,
                window_days: scan.window_days,
                served_all: c.served_all,
                corrected_after: c.corrected_after,
                last_served: c.last_served.map(|d| d.format("%Y-%m-%d").to_string()),
            }
        })
        .collect();
    let log = scan.oldest.map(|o| LogSpan {
        days: scan.days_covered(),
        since: o.format("%Y-%m-%d").to_string(),
        typed: scan.typed,
        machine: scan.machine,
        textless: scan.textless,
    });
    Stats { log, window_days: scan.window_days, rules: rows }
}

/// Example 2's table, with one line above it saying what the log covers.
pub fn render_stats(s: &Stats) -> String {
    let mut out = match &s.log {
        Some(l) => format!("from the match log: {} {} (since {})\n", l.days, plural(l.days as usize, "day", "days"), l.since),
        None => "no match log yet: base writes it from the next prompt on\n".to_string(),
    };
    if s.rules.is_empty() {
        out.push_str("no rules\n");
        return out;
    }
    let window = format!("served {}d", s.window_days);
    let head = ["rule", window.as_str(), "served all", "corrected after", "last served"];
    let cells: Vec<[String; 5]> = s
        .rules
        .iter()
        .map(|r| {
            [
                r.rule.clone(),
                r.served_window.to_string(),
                r.served_all.to_string(),
                r.corrected_after.to_string(),
                r.last_served.clone().unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    let mut width = head.map(|h| h.chars().count());
    for row in &cells {
        for (w, c) in width.iter_mut().zip(row) {
            *w = (*w).max(c.chars().count());
        }
    }
    let line = |row: [&str; 5]| -> String {
        let parts: Vec<String> = row.iter().zip(width).map(|(c, w)| format!(" {c:<w$} ")).collect();
        format!("|{}|\n", parts.join("|"))
    };
    out.push_str(&line(head));
    for row in &cells {
        out.push_str(&line([&row[0], &row[1], &row[2], &row[3], &row[4]]));
    }
    out
}

/// `base rule stats` from `cwd`: the rules as the hooks read them, the logs of its tiers.
pub fn stats_for(cwd: &Path, config: &BaseConfig, domain: Option<&str>) -> Stats {
    let domains = crate::domain::load_domains(cwd);
    let store = crate::store::load_merged(cwd);
    let current = Current::from(&domains, store.as_ref(), config);
    let scan = scan(&log_dirs(cwd), Local::now().date_naive(), config.doctor.dead_days, &current);
    let rules = rules_now(&domains, store.as_ref(), config);
    stats(&scan, &rules, domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_are_quoted_only_when_a_shell_needs_it() {
        assert_eq!(arg("tools"), "tools");
        assert_eq!(arg("user prompt submit"), "\"user prompt submit\"");
        assert_eq!(cmd_replay_drop("tools", "a b"), "base rule replay --domain tools --drop-keyword \"a b\"");
    }

    #[test]
    fn a_young_window_starts_today_and_counts_days_inclusive() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        let s = Scan { today, window_days: 30, oldest: Some(today), ..Scan::default() };
        assert_eq!(s.days_covered(), 1);
        assert_eq!(s.window_start(), NaiveDate::from_ymd_opt(2026, 9, 4).unwrap());
        let none = Scan { today, window_days: 30, ..Scan::default() };
        assert_eq!(none.days_covered(), 0);
    }
}
