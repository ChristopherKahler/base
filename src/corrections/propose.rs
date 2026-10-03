//! `base rule propose` (BO-15, K3): one correction, sorted three ways, kept as a pending proposal for review (BO-16).
//!
//! THE TURN. `--from-turn` reads the session's own transcript (`CLAUDE_CODE_SESSION_ID`, which Claude Code sets for
//! every tool command): the last prompt a person typed, the marker the AI wrote at the start of its reply, and the
//! signals of that turn and the one before it, read the way the hooks read them ([`super::turns`]), plus the session's
//! own signal rows (a file changed after the AI wrote it is only known there). `--transcript` and `--prompt <n>` name
//! a past turn instead.
//!
//! THE SORT. Every rule and decision base holds is a candidate. One fits when `--rule` or `--decision` names it, or
//! when BM25 (`domain::bm25`) over the candidates scores it at [`FIT_MIN_SCORE`] or more with [`FIT_MIN_TERMS`]
//! shared words and [`FIT_MIN_MARGIN`] times the next candidate; `--new` says none fits. Then:
//! - it fits and was served to this session up to and including that prompt (a `served` entry on one of the session's
//!   match-log rows; a decision also when the printed part of the session's session start carried it): **rewrite**,
//!   the AI had it and did it anyway;
//! - it fits and was not served: **keyword gap**, its triggers missed;
//! - nothing fits: **new rule**.
//!
//! "Served on that prompt" is read as "served to this session by that prompt" (lynx's G0 verdict, question 2): a rule
//! is shown once per session (D15) and stays in the AI's context, so a rule served on prompt 2 and broken on prompt 9
//! is a rewrite, not a keyword gap.
//!
//! THE STORE. K5a: proposals live in the graph, as `ops:RuleProposal` records in the tier the cwd writes (F22), status
//! `pending`. One proposal per turn: running the command again for the same session and prompt replaces it, id kept.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use oxigraph::model::TermRef;
use oxigraph::sparql::QueryResults;
use sha2::{Digest, Sha256};

use crate::config::BaseConfig;
use crate::crud;
use crate::domain::bm25;
use crate::domain::transcript;
use super::clip;
use crate::emit::match_log::{self, Row, Signal};

/// The least score a candidate needs to fit (with [`FIT_MIN_TERMS`] and [`FIT_MIN_MARGIN`]): on a store of a few
/// records it keeps a word or two in common from counting as a fit.
pub const FIT_MIN_SCORE: f32 = 12.0;
/// The words a candidate must share with the correction to fit: one shared word is a coincidence.
pub const FIT_MIN_TERMS: usize = 2;
/// The best candidate fits only when it scores at least this many times the next one. Set from BO-15's gate 4 on
/// Chris's store (98 marked corrections against about 1,500 rules and decisions): a long prompt adds points to every
/// record that shares a few project words, so raw scores ran from 13 to 259 with the clearest misses above 140 and real
/// fits as low as 36, and no fixed floor told them apart. What a real fit does is stand out: the ones that named the
/// very ruling the AI broke scored 2.3 to 7 times the next record, and the clear misses at most 1.8 times, but one.
pub const FIT_MIN_MARGIN: f32 = 2.0;
/// Added when a `--keywords` phrase of two or more words is in a candidate's text whole.
const PHRASE_BONUS: f32 = 3.0;
/// The candidates printed and kept on a proposal.
const SHOWN_CANDIDATES: usize = 3;
/// How many of the log's recent prompts the keyword suggestion compares against.
const RECENT_PROMPTS: usize = 500;

/// What `base rule propose` was given.
#[derive(Debug, Clone, Default)]
pub struct Args {
    pub from_turn: bool,
    pub text: Option<String>,
    pub keywords: Option<String>,
    pub example: Option<String>,
    pub rule: Option<String>,
    pub decision: Option<String>,
    pub new: bool,
    pub domain: Option<String>,
    pub dry_run: bool,
    pub transcript: Option<String>,
    pub prompt: Option<u32>,
}

/// The three kinds (K3). BO-17 adds its own (drop keyword, merge, split, retire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    KeywordGap,
    Rewrite,
    NewRule,
}

impl Kind {
    /// As stored: `keyword-gap`, `rewrite`, `new-rule`.
    pub fn slug(self) -> &'static str {
        match self {
            Kind::KeywordGap => "keyword-gap",
            Kind::Rewrite => "rewrite",
            Kind::NewRule => "new-rule",
        }
    }

    /// As printed: `keyword gap`, `rewrite`, `new rule`.
    pub fn label(self) -> &'static str {
        match self {
            Kind::KeywordGap => "keyword gap",
            Kind::Rewrite => "rewrite",
            Kind::NewRule => "new rule",
        }
    }
}

/// A rule or decision the correction may be about.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// `rule` or `decision`.
    pub kind: &'static str,
    /// As the match log names it: a rule's full id (`rules::rule_id`), a decision's slug.
    pub id: String,
    /// As a person types it: `base.9f2c1a7b` for a rule, the slug for a decision.
    pub show: String,
    pub domain: String,
    /// A rule's text, a decision's name.
    pub text: String,
    /// A rule that carries matchers of its own, served on them and not through its domain (F1).
    pub has_matchers: bool,
    /// A decision filed under an always-on domain, served by its own keywords (BO-03).
    pub global: bool,
    /// What BM25 scores: the text, the rationale, the rule's own words or the decision's keywords.
    pub doc: String,
    pub score: f32,
    pub matched: Vec<String>,
}

/// What a proposal changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// `domain`, `rule` or `decision`.
    pub kind: &'static str,
    /// The domain's name, `<domain>.<id>` for a rule, the slug for a decision.
    pub id: String,
    pub domain: String,
    /// What the target says now, for the reader: a rule's text or a decision's name. Empty for a new rule's domain.
    pub what: String,
}

/// One proposal, written or about to be.
#[derive(Debug, Clone)]
pub struct Proposal {
    /// `p-0003`; `None` until written.
    pub id: Option<String>,
    pub kind: Kind,
    /// `wrong` (UPDATED, CORRECTED), `misread` (MISREAD), `unmarked` (no marker), `manual` (no `--from-turn`).
    pub meaning: &'static str,
    pub target: Target,
    /// Why the target is the one: the domain's source for a new rule, served or not for the other two.
    pub why: String,
    pub text: Option<String>,
    pub keywords: Vec<String>,
    /// The keywords were suggested from the prompt, not given.
    pub suggested: bool,
    /// The prompt the change must serve on: its first `fires_on` test.
    pub example: String,
    pub prompt: String,
    pub marker: Option<String>,
    pub signals: Vec<Signal>,
    pub session: Option<String>,
    pub title: Option<String>,
    /// The prompt number the hooks counted, or the prompt's place in the transcript when the hooks never saw it.
    pub turn: Option<u32>,
    pub candidates: Vec<Candidate>,
    pub note: Option<String>,
    pub warnings: Vec<String>,
    pub fingerprint: String,
    pub turn_key: String,
    /// It replaced the turn's earlier proposal.
    pub replaced: bool,
}

/// What the correction is, read from its turn or from the flags.
struct Evidence {
    prompt: String,
    signals: Vec<Signal>,
    session: Option<String>,
    turn: Option<u32>,
    manual: bool,
}

/// Run `base rule propose`: sort the correction, and write the proposal unless `--dry-run`.
pub fn run(config: &BaseConfig, cwd: &Path, args: &Args) -> Result<Proposal, String> {
    let ev = evidence(config, cwd, args)?;
    let c3: Vec<&Signal> = ev.signals.iter().filter(|s| s.layer == "C3").collect();
    if !c3.is_empty() && c3.iter().all(|s| s.kind == "DEFERRED") {
        return Err("this turn's marker is DEFERRED: the AI held its position. A disagreement is logged \
                    (base log corrections), never proposed as a rule."
            .to_string());
    }
    let meaning = if c3.iter().any(|s| s.kind != "MISREAD" && s.kind != "DEFERRED") {
        "wrong"
    } else if c3.iter().any(|s| s.kind == "MISREAD") {
        "misread"
    } else if ev.manual {
        "manual"
    } else {
        "unmarked"
    };
    let marker = c3.iter().find(|s| s.kind != "DEFERRED").and_then(|s| s.value.clone());
    let given_keywords: Vec<String> =
        args.keywords.as_deref().map(crate::domain::global_decisions::parse_keywords).unwrap_or_default();
    let text = args.text.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(String::from);

    let domains = crate::domain::load_domains(cwd);
    // Domains synced first, as every hook does, then both tiers loaded once: the candidates and the next id read it.
    crate::hook::user_prompt_submit::ensure_domain_sync_pub(config, cwd);
    let store = crate::store::load_merged(cwd);
    let mut candidates = load_candidates(config, store.as_ref(), &domains);
    rank(&mut candidates, text.as_deref(), &given_keywords, marker.as_deref(), &ev.prompt);
    let fit = pick(args, &candidates)?;

    let rows = ev.session.as_deref().map(|s| session_rows(cwd, s)).unwrap_or_default();
    let trigger_row = ev
        .turn
        .and_then(|n| rows.iter().rposition(|r| r.event == "prompt" && r.prompt_num == Some(n)));
    let upto: &[Row] = match trigger_row {
        Some(i) => &rows[..=i],
        None => &rows,
    };
    let trigger = trigger_row.map(|i| &rows[i]);

    let (keywords, suggested) = if given_keywords.is_empty() {
        (suggest(&ev.prompt, &config.corrections, &recent_prompts(cwd)), true)
    } else {
        (given_keywords, false)
    };
    let example = args
        .example
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(String::from)
        .unwrap_or_else(|| ev.prompt.clone());

    let mut note = None;
    let (kind, target, why) = match fit {
        Some(c) => {
            let served = served_in(upto, &c.id)
                || (c.kind == "decision" && ev.session.as_deref().is_some_and(|s| printed_at_session_start(cwd, s, &c.text)));
            if served {
                let t = Target { kind: c.kind, id: c.show.clone(), domain: c.domain.clone(), what: c.text.clone() };
                (Kind::Rewrite, t, "served to this session, and corrected anyway".to_string())
            } else {
                note = cut_note(trigger, c);
                (Kind::KeywordGap, gap_target(c), format!("fits {} {}, not served to this session", c.kind, c.show))
            }
        }
        None => {
            // Asked first: the AI that runs the command bare usually lands here, and the records closest to the
            // correction are what it needs to choose between --text and naming one.
            if text.is_none() {
                return Err(format!(
                    "no rule or decision base holds stands out as the one this correction is about, so this is a new \
                     rule and needs its wording: give --text \"...\". If it is about one of these, name it instead \
                     with --rule <domain>.<id> or --decision <slug>. Closest: {}",
                    closest(&candidates)
                ));
            }
            let (domain, why) = new_rule_domain(args, cwd, trigger, &candidates, &domains)?;
            (Kind::NewRule, Target { kind: "domain", id: domain.clone(), domain, what: String::new() }, why)
        }
    };

    let mut warnings = Vec::new();
    let lower = example.to_lowercase();
    if kind != Kind::Rewrite
        && !keywords.is_empty()
        && !keywords.iter().any(|k| crate::domain::matcher::contains_word(&lower, &k.to_lowercase()))
    {
        warnings.push("none of the keywords is in the example, so its fires_on test would miss".to_string());
    }
    let session = ev.session.clone();
    let fingerprint = fingerprint(kind, &target, &keywords, text.as_deref());
    let turn_key = turn_key(session.as_deref(), &ev, &fingerprint);
    let mut proposal = Proposal {
        id: None,
        kind,
        meaning,
        why,
        fingerprint,
        turn_key,
        target,
        text,
        keywords,
        suggested,
        example: crate::scrub::scrub(&example),
        prompt: crate::scrub::scrub(&ev.prompt),
        marker,
        signals: ev.signals,
        title: session.as_deref().and_then(crate::relay::session_registry::title_of),
        session,
        turn: ev.turn,
        candidates: candidates.into_iter().take(SHOWN_CANDIDATES).collect(),
        note,
        warnings,
        replaced: false,
    };
    if !args.dry_run {
        let highest = store.as_ref().map(|s| max_id(s, &config.namespace)).unwrap_or(0);
        write(config, cwd, highest, &mut proposal)?;
    }
    Ok(proposal)
}

/// The correction's prompt and signals: from the turn (`--from-turn`), or from the flags alone (K3's manual override).
fn evidence(config: &BaseConfig, cwd: &Path, args: &Args) -> Result<Evidence, String> {
    if !args.from_turn {
        let Some(example) = args.example.as_deref().map(str::trim).filter(|e| !e.is_empty()) else {
            return Err("without --from-turn, give --example \"<a prompt the change must serve on>\"".to_string());
        };
        if args.text.is_none() && args.keywords.is_none() {
            return Err("without --from-turn, give --text \"...\" or --keywords \"a, b\"".to_string());
        }
        return Ok(Evidence {
            prompt: example.to_string(),
            signals: Vec::new(),
            session: crate::relay::env_session_id(),
            turn: None,
            manual: true,
        });
    }
    // A transcript named with --transcript is its own session's, whoever runs the command: gate 4 and BO-17 read past
    // turns from inside another Claude Code session, whose CLAUDE_CODE_SESSION_ID is not the transcript's.
    let session = match args.transcript.as_deref() {
        Some(t) => Path::new(t).file_stem().and_then(|s| s.to_str()).map(String::from),
        None => crate::relay::env_session_id(),
    };
    let path = transcript_for(cwd, args.transcript.as_deref(), session.as_deref())?;
    let events = transcript::read_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let turns = super::turns(events, &config.corrections);
    let at = match args.prompt {
        Some(n) => turns.iter().position(|t| t.human_num == Some(n)).ok_or_else(|| {
            let count = turns.iter().filter(|t| t.human_num.is_some()).count();
            format!("{} holds {count} prompts typed by a person, so there is no prompt {n}", path.display())
        })?,
        None => turns
            .iter()
            .rposition(|t| t.human_num.is_some())
            .ok_or_else(|| format!("{} holds no prompt typed by a person yet", path.display()))?,
    };
    let t = &turns[at];
    let mut signals = t.signals.clone();
    // The turn before: an interrupt or a refusal ends a turn with no Stop, and a file it wrote that changed before this
    // prompt is the user's own fix; that is what this prompt answers. Its repeat was about the prompt before it.
    if at > 0 {
        for s in turns[at - 1].signals.iter().filter(|s| carried_over(s)) {
            super::push_once(&mut signals, s.clone());
        }
    }
    // The prompt number the hooks counted: the session's state knows it when its last prompt is this one.
    let state = session
        .as_deref()
        .and_then(|s| super::state_path(cwd, s))
        .map(|p| super::load_state(&p));
    let hash = super::phrases::text_hash(&t.prompt);
    let turn = match state.as_ref().and_then(|s| s.last_human.as_ref()) {
        Some(last) if last.text_hash == hash => last.num,
        _ => t.num,
    };
    // The session's own signal rows: this turn's, and the C2 of the turn before (as from the transcript above). A file
    // changed after the AI wrote it is known only there. The turn before's own phrase or marker is not this turn's: a
    // DEFERRED reply right after a MISREAD one is still only a disagreement.
    if let Some(sid) = session.as_deref() {
        for row in session_rows(cwd, sid).iter().filter(|r| r.event == "signal") {
            let this_turn = row.prompt_num == Some(turn);
            if this_turn || row.prompt_num.is_some_and(|n| n + 1 == turn) {
                for s in row.signals.iter().filter(|s| this_turn || carried_over(s)) {
                    super::push_once(&mut signals, s.clone());
                }
            }
        }
    }
    Ok(Evidence { prompt: t.prompt.clone(), signals, session, turn: Some(turn), manual: false })
}

/// A signal of the turn before that belongs to this turn's correction: a C2 other than the turn's own repeat.
fn carried_over(s: &Signal) -> bool {
    s.layer == "C2" && s.kind != "repeat"
}

/// The session's transcript: `--transcript`, else the one the hooks recorded, else
/// `<claude config>/projects/*/<session>.jsonl`.
fn transcript_for(cwd: &Path, given: Option<&str>, session: Option<&str>) -> Result<PathBuf, String> {
    if let Some(t) = given {
        return Ok(PathBuf::from(t));
    }
    let Some(sid) = session else {
        return Err("no session: run this inside a Claude Code session (it sets CLAUDE_CODE_SESSION_ID), or pass \
                    --transcript <path>"
            .to_string());
    };
    if let Some(t) = super::state_path(cwd, sid).map(|p| super::load_state(&p)).and_then(|s| s.transcript)
        && Path::new(&t).is_file()
    {
        return Ok(PathBuf::from(t));
    }
    if crate::crud::handoff_show::is_file_safe_session_id(sid)
        && let Some(config_dir) = crate::claude_md::user_file().and_then(|f| f.parent().map(Path::to_path_buf))
    {
        for dir in std::fs::read_dir(config_dir.join("projects")).into_iter().flatten().flatten() {
            let f = dir.path().join(format!("{sid}.jsonl"));
            if f.is_file() {
                return Ok(f);
            }
        }
    }
    Err(format!("no transcript found for session {sid}: pass --transcript <path>"))
}

/// The tier folders a session's rows can be in: the cwd's, then the global tier's.
fn row_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in [crate::crud::handoff_show::session_start_dir(cwd), crate::config::global_base_dir().filter(|d| d.is_dir())]
        .into_iter()
        .flatten()
    {
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// The newest match-log files a session's rows are read from: the live file and the archives of the days before it
/// (one archive per day, `match_log::retain`). A session older than that is past what a correction is about.
const SESSION_FILES: usize = 4;

/// Every match-log row of `session` (an id, or the start of one), oldest first, from both tiers' newest
/// [`SESSION_FILES`] files. A line is parsed only when it names the session.
pub fn session_rows(cwd: &Path, session: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for dir in row_dirs(cwd) {
        for file in match_log::files(&dir).iter().rev().take(SESSION_FILES) {
            let Ok(text) = std::fs::read_to_string(file) else { continue };
            for line in text.lines().filter(|l| l.contains(session)) {
                if let Ok(row) = serde_json::from_str::<Row>(line)
                    && row.session.as_deref().is_some_and(|s| s.starts_with(session))
                {
                    rows.push(row);
                }
            }
        }
    }
    rows.sort_by(|a, b| a.ts.cmp(&b.ts));
    rows
}

/// The text of the log's recent prompts, for the keyword suggestion.
fn recent_prompts(cwd: &Path) -> Vec<String> {
    let Some(dir) = crate::crud::handoff_show::session_start_dir(cwd) else {
        return Vec::new();
    };
    match_log::last_rows(&dir, RECENT_PROMPTS * 8, &match_log::Filter::default())
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.event == "prompt")
        .filter_map(|r| r.text)
        .rev()
        .take(RECENT_PROMPTS)
        .collect()
}

/// Every rule and decision in both tiers, superseded ones left out: rules from the graph (synced from `domains.toml`
/// first, as every hook does), and any `domains.toml` rule the graph does not hold yet.
fn load_candidates(
    config: &BaseConfig,
    store: Option<&oxigraph::store::Store>,
    domains: &[crate::domain::DomainDef],
) -> Vec<Candidate> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let pfx = crud::prefixes(ns);
    let names: HashMap<String, String> = domains
        .iter()
        .map(|d| (crud::build_iri(ns, "domain", &crud::slugify(&d.name)), d.name.clone()))
        .collect();
    let always: HashSet<String> =
        domains.iter().filter(|d| d.is_always()).map(|d| crud::slugify(&d.name)).collect();
    let domain_name = |iri: &str| names.get(iri).cloned().unwrap_or_else(|| iri.rsplit('/').next().unwrap_or_default().to_string());
    let mut out: Vec<Candidate> = Vec::new();
    if let Some(store) = store {
        // Rules: text, rationale, matchers. One row per matcher value, folded per rule.
        let no_superseded = crate::supersede::sparql_exclude_superseded(ns, "rule");
        let sparql = format!(
            "{pfx}\nSELECT ?domain ?rule ?text ?rationale ?mp ?mv WHERE {{\n\
               GRAPH ?g {{\n\
                 ?domain {p}:hasRule ?rule .\n\
                 ?rule {p}:ruleText ?text .\n\
                 OPTIONAL {{ ?rule {p}:rationale ?rationale }}\n\
                 OPTIONAL {{ ?rule ?mp ?mv . FILTER(?mp IN ({p}:matchKind, {p}:matchPlace, {p}:matchTool, {p}:matchCommand, {p}:matchWord)) }}\n\
                 {no_superseded}\
               }}\n\
             }}"
        );
        let mut at: HashMap<String, usize> = HashMap::new();
        if let Ok(QueryResults::Solutions(rows)) = crate::store::query(store, &sparql) {
            for row in rows.filter_map(Result::ok) {
                let named = |k: &str| match row.get(k).map(Into::into) {
                    Some(TermRef::NamedNode(n)) => Some(n.as_str().to_string()),
                    _ => None,
                };
                let lit = |k: &str| match row.get(k).map(Into::into) {
                    Some(TermRef::Literal(l)) => Some(l.value().to_string()),
                    _ => None,
                };
                let (Some(d), Some(text)) = (named("domain"), lit("text")) else { continue };
                if text.trim().is_empty() {
                    continue;
                }
                let domain = domain_name(&d);
                let id = crate::domain::rules::rule_id(&domain, &text);
                let i = *at.entry(id.clone()).or_insert_with(|| {
                    out.push(Candidate {
                        kind: "rule",
                        show: crate::domain::rule_test::short_ref(&domain, &id),
                        id,
                        domain: domain.clone(),
                        doc: text.clone(),
                        text: text.clone(),
                        has_matchers: false,
                        global: false,
                        score: 0.0,
                        matched: Vec::new(),
                    });
                    out.len() - 1
                });
                if let Some(r) = lit("rationale")
                    && !out[i].doc.contains(&r)
                {
                    out[i].doc.push_str(&format!("\n{r}"));
                }
                if let (Some(mp), Some(mv)) = (named("mp"), lit("mv")) {
                    out[i].has_matchers = true;
                    if mp.ends_with("matchWord") && !out[i].doc.contains(&mv) {
                        out[i].doc.push_str(&format!("\n{mv}"));
                    }
                }
            }
        }
        // Decisions: name, rationale, keywords, and the domains they are filed under.
        let no_superseded = crate::supersede::sparql_exclude_superseded(ns, "d");
        let kw = crate::domain::global_decisions::PRED_KEYWORD;
        let sparql = format!(
            "{pfx}\nSELECT ?d ?name ?rationale ?status ?kw ?dom WHERE {{\n\
               GRAPH ?g {{\n\
                 ?d a {p}:Decision ; {p}:name ?name .\n\
                 OPTIONAL {{ ?d {p}:rationale ?rationale }}\n\
                 OPTIONAL {{ ?d {p}:status ?status }}\n\
                 {no_superseded}\
               }}\n\
               OPTIONAL {{ GRAPH ?kg {{ ?d {p}:{kw} ?kw }} }}\n\
               OPTIONAL {{ GRAPH ?dg {{ ?dom {p}:hasDecision ?d }} }}\n\
             }}"
        );
        let mut decisions: HashMap<String, usize> = HashMap::new();
        if let Ok(QueryResults::Solutions(rows)) = crate::store::query(store, &sparql) {
            for row in rows.filter_map(Result::ok) {
                let named = |k: &str| match row.get(k).map(Into::into) {
                    Some(TermRef::NamedNode(n)) => Some(n.as_str().to_string()),
                    _ => None,
                };
                let lit = |k: &str| match row.get(k).map(Into::into) {
                    Some(TermRef::Literal(l)) => Some(l.value().to_string()),
                    _ => None,
                };
                let (Some(d), Some(name)) = (named("d"), lit("name")) else { continue };
                if lit("status").is_some_and(|s| s.eq_ignore_ascii_case("superseded")) {
                    continue;
                }
                let slug = crud::slug_of(&d);
                let i = *decisions.entry(slug.clone()).or_insert_with(|| {
                    out.push(Candidate {
                        kind: "decision",
                        id: slug.clone(),
                        show: slug.clone(),
                        domain: String::new(),
                        doc: name.clone(),
                        text: name.clone(),
                        has_matchers: false,
                        global: false,
                        score: 0.0,
                        matched: Vec::new(),
                    });
                    out.len() - 1
                });
                if let Some(r) = lit("rationale")
                    && !out[i].doc.contains(&r)
                {
                    out[i].doc.push_str(&format!("\n{r}"));
                }
                if let Some(k) = lit("kw")
                    && !out[i].doc.contains(&k)
                {
                    out[i].doc.push_str(&format!("\n{k}"));
                }
                if let Some(dom) = named("dom") {
                    let dname = domain_name(&dom);
                    let is_always = always.contains(&crud::slugify(&dname));
                    // A decision filed under an always-on domain and another: the other one names it.
                    if out[i].domain.is_empty() || (out[i].global && !is_always) {
                        out[i].domain = dname;
                        out[i].global = is_always;
                    }
                }
            }
        }
    }
    // A `domains.toml` rule the graph does not hold yet.
    for d in domains {
        for r in d.rules.iter().filter(|r| !r.text().trim().is_empty()) {
            let id = crate::domain::rules::rule_id(&d.name, r.text());
            if out.iter().any(|c| c.id == id) {
                continue;
            }
            let mut doc = r.text().to_string();
            if let Some(why) = r.rationale() {
                doc.push_str(&format!("\n{why}"));
            }
            out.push(Candidate {
                kind: "rule",
                show: crate::domain::rule_test::short_ref(&d.name, &id),
                id,
                domain: d.name.clone(),
                text: r.text().to_string(),
                has_matchers: !r.matchers().is_empty(),
                global: false,
                doc,
                score: 0.0,
                matched: Vec::new(),
            });
        }
    }
    out
}

/// Score every candidate against the correction (BM25, `domain::bm25`), best first. The query is the new wording,
/// the keywords twice (they are the correction's own triggers), the marker line and the prompt; a two-word keyword
/// found whole in a candidate adds [`PHRASE_BONUS`].
fn rank(cands: &mut [Candidate], text: Option<&str>, keywords: &[String], marker: Option<&str>, prompt: &str) {
    let corpus = bm25::Corpus::new(cands.iter().map(|c| bm25::words(&c.doc)));
    let mut query: Vec<String> = Vec::new();
    if let Some(t) = text {
        query.extend(bm25::words(t));
    }
    for k in keywords {
        let w = bm25::words(k);
        query.extend(w.iter().cloned());
        query.extend(w);
    }
    if let Some(m) = marker {
        query.extend(bm25::words(m.split_once(':').map(|(_, rest)| rest).unwrap_or(m)));
    }
    query.extend(bm25::words(prompt));
    let phrases: Vec<String> =
        keywords.iter().filter(|k| k.split_whitespace().count() >= 2).map(|k| k.to_lowercase()).collect();
    for (i, c) in cands.iter_mut().enumerate() {
        let r = corpus.score(i, &query);
        c.score = r.score;
        c.matched = r.matched;
        let lower = c.doc.to_lowercase();
        for ph in &phrases {
            if crate::domain::matcher::contains_word(&lower, ph) {
                c.score += PHRASE_BONUS;
            }
        }
    }
    cands.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.show.cmp(&b.show)));
}

/// The candidate that fits: the one `--rule` or `--decision` names, none with `--new`, else the best when it scores
/// [`FIT_MIN_SCORE`] with [`FIT_MIN_TERMS`] shared words and at least [`FIT_MIN_MARGIN`] times the next candidate.
fn pick<'a>(args: &Args, cands: &'a [Candidate]) -> Result<Option<&'a Candidate>, String> {
    if args.new {
        return Ok(None);
    }
    if let Some(r) = args.rule.as_deref() {
        let (domain, id) = crate::crud::rule::parse_rule_ref(r)?;
        let found: Vec<&Candidate> = cands
            .iter()
            .filter(|c| c.kind == "rule" && c.id.starts_with(&id))
            .filter(|c| domain.as_deref().is_none_or(|d| crud::slugify(d) == crud::slugify(&c.domain)))
            .collect();
        return match found.as_slice() {
            [one] => Ok(Some(*one)),
            [] => Err(format!("no rule '{r}' (ids come from base rule list --domain <domain>)")),
            many => Err(format!(
                "'{r}' fits {} rules: {}; give more of the id",
                many.len(),
                many.iter().map(|c| c.show.as_str()).collect::<Vec<_>>().join(", ")
            )),
        };
    }
    if let Some(d) = args.decision.as_deref() {
        let d = d.trim();
        let exact = cands.iter().find(|c| c.kind == "decision" && c.id == d);
        let found: Vec<&Candidate> = match exact {
            Some(c) => vec![c],
            None => cands.iter().filter(|c| c.kind == "decision" && c.id.starts_with(d)).collect(),
        };
        return match found.as_slice() {
            [one] => Ok(Some(*one)),
            [] => Err(format!("no decision '{d}' (slugs come from base decision search --keyword <word>)")),
            many => Err(format!("'{d}' fits {} decisions; give more of the slug", many.len())),
        };
    }
    let next = cands.get(1).map_or(0.0, |c| c.score);
    Ok(cands
        .first()
        .filter(|c| c.score >= FIT_MIN_SCORE && c.matched.len() >= FIT_MIN_TERMS && c.score >= FIT_MIN_MARGIN * next))
}

/// `id` is in a `served` list of one of `rows`.
fn served_in(rows: &[Row], id: &str) -> bool {
    rows.iter().any(|r| r.served.iter().any(|i| i.id == id))
}

/// A decision named `name` was in the printed part of this session's session start: the session's full output holds
/// it and the session's record does not list the `global-decisions` block as withheld. Session start writes nothing to
/// the match log, and a global decision with no keywords is shown only there (BO-03).
fn printed_at_session_start(cwd: &Path, session: &str, name: &str) -> bool {
    let needle: String = name.chars().take(80).collect();
    for dir in row_dirs(cwd) {
        let Some(own) = crate::emit::session_files::session_dir(&dir, session) else { continue };
        let full = std::fs::read_to_string(own.join(crate::emit::session_files::SESSION_START_FILE)).unwrap_or_default();
        if !full.contains(&needle) {
            continue;
        }
        let log = std::fs::read_to_string(dir.join("hook-output.jsonl")).unwrap_or_default();
        let record = log
            .lines()
            .rev()
            .filter(|l| l.contains(session))
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|v| v["hook"] == "session-start" && v["session_id"] == session);
        if let Some(v) = record {
            let withheld = v["withheld"].as_array().is_some_and(|w| w.iter().any(|b| b["block"] == "global-decisions"));
            return !withheld;
        }
    }
    false
}

/// For a keyword gap: what the triggering prompt's row says about the candidate, when it says anything.
fn cut_note(trigger: Option<&Row>, c: &Candidate) -> Option<String> {
    let row = trigger?;
    if let Some(cut) = row.cut.iter().find(|x| x.item.id == c.id) {
        return Some(format!(
            "it matched that prompt and was cut ({}, {}): the trigger worked and the budget did not",
            cut.reason, cut.limit
        ));
    }
    let slug = crud::slugify(&c.domain);
    row.matched
        .iter()
        .find(|m| !c.domain.is_empty() && crud::slugify(&m.domain) == slug)
        .map(|m| format!("its domain {} matched that prompt ({})", c.domain, m.by))
}

/// What a keyword gap adds keywords to: a rule with matchers of its own takes them as its own words; a decision under
/// an always-on domain as its own keywords (BO-03); anything else is served through its domain, which takes them.
fn gap_target(c: &Candidate) -> Target {
    let own = (c.kind == "rule" && c.has_matchers) || (c.kind == "decision" && c.global);
    if own || c.domain.is_empty() {
        return Target { kind: c.kind, id: c.show.clone(), domain: c.domain.clone(), what: c.text.clone() };
    }
    Target { kind: "domain", id: c.domain.clone(), domain: c.domain.clone(), what: format!("{} {}: {}", c.kind, c.show, c.text) }
}

/// The domain a new rule goes into, and why: `--domain`; a domain the prompt matched by keyword or path; the closest
/// record's; the cwd's project; the first always-on domain.
fn new_rule_domain(
    args: &Args,
    cwd: &Path,
    trigger: Option<&Row>,
    cands: &[Candidate],
    domains: &[crate::domain::DomainDef],
) -> Result<(String, String), String> {
    if let Some(d) = args.domain.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        return Ok((crate::domain::canonical_name(cwd, d), "named with --domain".to_string()));
    }
    if let Some(m) = trigger.and_then(|r| {
        r.matched.iter().find(|m| matches!(m.by.as_str(), "keyword" | "path" | "parent" | "file_keyword"))
    }) {
        return Ok((m.domain.clone(), format!("the prompt matched it by {}", m.by)));
    }
    if let Some(c) = cands.first().filter(|c| c.score > 0.0 && !c.domain.is_empty()) {
        return Ok((c.domain.clone(), format!("the closest record's, {} {}", c.kind, c.show)));
    }
    let ctx = crate::domain::trigger_context(cwd);
    let here = crate::domain::matcher::resolve_trigger(&cwd.display().to_string(), None, ctx.home.as_deref());
    if let Some(owner) = here.as_deref().and_then(|h| crate::domain::matcher::owners(h, &ctx.registered).into_iter().next()) {
        return Ok((owner.slug.clone(), format!("this folder's project, {}", owner.name)));
    }
    if let Some(d) = domains.iter().find(|d| d.is_always()) {
        return Ok((d.name.clone(), "the always-on domain".to_string()));
    }
    Err("no domain to put the rule in: name one with --domain".to_string())
}

/// Keywords suggested from the prompt: its words, version strings first, then the rarest across the log's recent
/// prompts, then the longest; at most five. The C1 phrases and stopwords never are.
fn suggest(prompt: &str, cc: &crate::config::CorrectionsConfig, recent: &[String]) -> Vec<String> {
    let skip: HashSet<String> = cc.phrases.iter().flat_map(|p| bm25::words(p)).collect();
    let mut words: Vec<String> = Vec::new();
    for w in bm25::words(prompt) {
        let long_enough = w.chars().count() >= 4 || w.chars().any(|c| c.is_ascii_digit());
        if long_enough && !skip.contains(&w) && !words.contains(&w) {
            words.push(w);
        }
    }
    let seen: Vec<HashSet<String>> = recent.iter().map(|t| bm25::words(t).into_iter().collect()).collect();
    let df = |w: &str| seen.iter().filter(|s| s.contains(w)).count();
    let version = |w: &str| w.contains('.') && w.chars().any(|c| c.is_ascii_digit());
    let mut ranked: Vec<(usize, String)> = words.into_iter().enumerate().collect();
    ranked.sort_by(|(ia, a), (ib, b)| {
        version(b)
            .cmp(&version(a))
            .then(df(a).cmp(&df(b)))
            .then(b.chars().count().cmp(&a.chars().count()))
            .then(ia.cmp(ib))
    });
    ranked.into_iter().map(|(_, w)| w).take(5).collect()
}

fn hex16(text: &str) -> String {
    Sha256::digest(text.as_bytes())[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Kind, target and change, hashed: the key BO-16's K5e keeps for a rejected proposal, so the same one is never
/// offered again.
fn fingerprint(kind: Kind, target: &Target, keywords: &[String], text: Option<&str>) -> String {
    let mut kw: Vec<String> = keywords.iter().map(|k| k.trim().to_lowercase()).collect();
    kw.sort();
    let text = text.map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()).unwrap_or_default();
    hex16(&format!("{}|{}:{}|{}|{}", kind.slug(), target.kind, target.id.to_lowercase(), kw.join(","), text))
}

/// The turn, hashed: one proposal per turn, replaced when the command runs again for it. A turn read from the
/// transcript is its session, its prompt number and its text, so the same words sent again on a later prompt (the C2
/// repeat) are a turn of their own. A manual proposal has no turn: it is keyed on what it proposes, so the same one run
/// again replaces itself and a different one is added beside it.
fn turn_key(session: Option<&str>, ev: &Evidence, fingerprint: &str) -> String {
    let s = session.unwrap_or("-");
    if ev.manual {
        return hex16(&format!("{s}|manual|{fingerprint}"));
    }
    hex16(&format!("{s}|{}|{}", ev.turn.unwrap_or(0), super::phrases::text_hash(&ev.prompt)))
}

/// The highest `p-NNNN` number in `store`.
fn max_id(store: &oxigraph::store::Store, ns: &crate::config::NamespaceConfig) -> u32 {
    let q = format!(
        "{}\nSELECT ?id WHERE {{ GRAPH ?g {{ ?x a {p}:RuleProposal ; {p}:proposalId ?id }} }}",
        crud::prefixes(ns),
        p = ns.prefix
    );
    let Ok(QueryResults::Solutions(rows)) = crate::store::query(store, &q) else { return 0 };
    rows.filter_map(Result::ok)
        .filter_map(|r| match r.get("id").map(Into::into) {
            Some(TermRef::Literal(l)) => l.value().trim_start_matches("p-").parse::<u32>().ok(),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// Write `prop` into the cwd's tier (the global tier outside every workspace), status `pending`. A pending proposal
/// for the same turn in that graph is replaced and keeps its id; otherwise the id is the highest in either tier
/// (`highest`, read from the merged store, and the write tier as it is now) plus one.
fn write(config: &BaseConfig, cwd: &Path, highest: u32, prop: &mut Proposal) -> Result<(), String> {
    let ns = &config.namespace;
    let p = ns.prefix.clone();
    let write_cwd = match crate::config::find_workspace_base(cwd) {
        Some(_) => cwd.to_path_buf(),
        None => crate::home::home_root()
            .map(|h| h.join(".base-gbl"))
            .ok_or_else(|| "no workspace here and no home folder for the global tier".to_string())?,
    };
    let graph = crud::workspace_graph_iri(ns, &crud::workspace_slug(&write_cwd));
    let key = prop.turn_key.clone();
    let mut chosen = String::new();
    let mut replaced = false;
    let snapshot = prop.clone();
    crud::load_read_then_mutate(&write_cwd, ns, |store| {
        let q = format!(
            "{}\nSELECT ?id WHERE {{ GRAPH <{graph}> {{ ?x a {p}:RuleProposal ; {p}:turnKey \"{key}\" ; \
             {p}:status \"pending\" ; {p}:proposalId ?id }} }}",
            crud::prefixes(ns)
        );
        let existing = match crate::store::query(store, &q)? {
            QueryResults::Solutions(mut rows) => rows.next().transpose()?.and_then(|r| match r.get("id").map(Into::into) {
                Some(TermRef::Literal(l)) => Some(l.value().to_string()),
                _ => None,
            }),
            _ => None,
        };
        let id = match existing {
            Some(id) => {
                replaced = true;
                id
            }
            None => format!("p-{:04}", highest.max(max_id(store, ns)) + 1),
        };
        chosen = id.clone();
        let iri = crud::build_iri(ns, "proposal", &id);
        let mut sparql = String::new();
        if replaced {
            sparql.push_str(&format!("DELETE WHERE {{ GRAPH <{graph}> {{ <{iri}> ?pred ?obj }} }} ;\n"));
        }
        sparql.push_str(&format!("INSERT DATA {{\n  GRAPH <{graph}> {{\n{}  }}\n}}", triples(&snapshot, &id, &iri, &p)));
        Ok(sparql)
    })
    .map_err(|e| format!("{e:#}"))?;
    prop.id = Some(chosen);
    prop.replaced = replaced;
    Ok(())
}

/// The proposal's triples, for `INSERT DATA`.
fn triples(prop: &Proposal, id: &str, iri: &str, p: &str) -> String {
    let lit = |pred: &str, v: &str| format!("      {p}:{pred} \"{}\" ;\n", crud::escape_sparql_literal(v));
    let mut s = format!("    <{iri}> rdf:type {p}:RuleProposal ;\n");
    s.push_str(&lit("proposalId", id));
    s.push_str(&lit("status", "pending"));
    s.push_str(&lit("proposalKind", prop.kind.slug()));
    s.push_str(&lit("meaning", prop.meaning));
    s.push_str(&lit("targetKind", prop.target.kind));
    s.push_str(&lit("targetId", &prop.target.id));
    s.push_str(&lit("targetDomain", &prop.target.domain));
    if !prop.target.what.is_empty() {
        s.push_str(&lit("targetText", &prop.target.what));
    }
    if let Some(t) = &prop.text {
        s.push_str(&lit("proposedText", t));
    }
    for k in &prop.keywords {
        s.push_str(&lit("proposedKeyword", k));
    }
    s.push_str(&lit("firesOn", &prop.example));
    s.push_str(&lit("triggerPrompt", &prop.prompt));
    if let Some(m) = &prop.marker {
        s.push_str(&lit("markerLine", m));
    }
    for sig in &prop.signals {
        s.push_str(&lit("signal", &sig.label()));
    }
    if let Some(v) = &prop.session {
        s.push_str(&lit("fromSession", v));
    }
    if let Some(v) = &prop.title {
        s.push_str(&lit("fromTitle", v));
    }
    if let Some(n) = prop.turn {
        s.push_str(&lit("fromTurn", &n.to_string()));
    }
    for c in &prop.candidates {
        s.push_str(&lit("candidate", &format!("{} {} {:.1}", c.kind, c.show, c.score)));
    }
    if let Some(n) = &prop.note {
        s.push_str(&lit("note", n));
    }
    s.push_str(&lit("fingerprint", &prop.fingerprint));
    s.push_str(&lit("turnKey", &prop.turn_key));
    s.push_str(&format!("      {p}:createdAt \"{}\"^^xsd:dateTime .\n", crud::now_iso()));
    s
}

/// The three best candidates as the output prints them: `decision x (98.9) · rule base.9f2c1a7b (87.6) · ...`.
fn closest(cands: &[Candidate]) -> String {
    let list: Vec<String> =
        cands.iter().take(SHOWN_CANDIDATES).map(|c| format!("{} {} ({:.1})", c.kind, c.show, c.score)).collect();
    if list.is_empty() { "none".to_string() } else { list.join(" · ") }
}

/// What `base rule propose` prints.
pub fn render(prop: &Proposal, dry_run: bool) -> String {
    let mut s = String::new();
    let id = prop.id.as_deref().unwrap_or("(not written)");
    s.push_str(&format!("proposal {id} · {} · {} {}\n", prop.kind.label(), prop.target.kind, prop.target.id));
    s.push_str(&format!("  why: {}\n", prop.why));
    if !prop.target.what.is_empty() {
        s.push_str(&format!("  now: {}\n", clip(&prop.target.what, 160)));
    }
    match (prop.kind, &prop.text) {
        (Kind::NewRule, Some(t)) => s.push_str(&format!("  text: {}\n", clip(t, 240))),
        (Kind::Rewrite, Some(t)) => s.push_str(&format!("  new text: {}\n", clip(t, 240))),
        (Kind::Rewrite, None) => s.push_str("  new text: none given; the marker line says what was wrong\n"),
        _ => {}
    }
    if !prop.keywords.is_empty() {
        let tail = if prop.suggested { " (suggested from the prompt; --keywords chooses)" } else { "" };
        let verb = if prop.kind == Kind::KeywordGap { "add keywords" } else { "keywords" };
        s.push_str(&format!("  {verb}: {}{tail}\n", prop.keywords.join(", ")));
    }
    s.push_str(&format!("  example, its first fires_on test: \"{}\"\n", clip(&prop.example, 160)));
    match prop.meaning {
        "misread" => s.push_str("  meaning: MISREAD, the AI misunderstood the ask (a context gap)\n"),
        "unmarked" => s.push_str("  meaning: no marker in the reply\n"),
        "manual" => s.push_str("  meaning: given by hand, no turn read\n"),
        _ => {}
    }
    let mut evidence: Vec<String> = Vec::new();
    if let Some(sid) = &prop.session {
        let short: String = sid.chars().take(8).collect();
        let turn = prop.turn.map(|n| format!(" prompt {n}")).unwrap_or_default();
        let title = prop.title.as_deref().map(|t| format!(" ({t})")).unwrap_or_default();
        evidence.push(format!("session {short}{turn}{title}"));
    }
    if prop.signals.is_empty() {
        evidence.push("no signal".to_string());
    } else {
        evidence.push(super::labels(&prop.signals));
    }
    s.push_str(&format!("  evidence: {}\n", evidence.join(" · ")));
    if let Some(m) = &prop.marker {
        s.push_str(&format!("  marker: {}\n", clip(m, 200)));
    }
    if let Some(n) = &prop.note {
        s.push_str(&format!("  note: {n}\n"));
    }
    for w in &prop.warnings {
        s.push_str(&format!("  warning: {w}\n"));
    }
    if !prop.candidates.is_empty() {
        let tail = if prop.kind == Kind::NewRule {
            format!(
                " · none fits (a fit scores {FIT_MIN_SCORE:.0} or more, shares {FIT_MIN_TERMS} words, and scores \
                 {FIT_MIN_MARGIN:.0} times the next)"
            )
        } else {
            String::new()
        };
        s.push_str(&format!("  closest: {}{tail}\n", closest(&prop.candidates)));
    }
    if dry_run {
        s.push_str("  dry run: nothing written\n");
    } else {
        let replaced = if prop.replaced { "; it replaces this turn's earlier proposal" } else { "" };
        s.push_str(&format!(
            "  pending review ({id}){replaced}. Wrong kind or target? Run it again with --rule <domain>.<id>, \
             --decision <slug> or --new: this turn's proposal is replaced.\n"
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(show: &str, score: f32, shared: usize) -> Candidate {
        Candidate {
            kind: "decision",
            id: show.to_string(),
            show: show.to_string(),
            domain: "d".to_string(),
            text: show.to_string(),
            has_matchers: false,
            global: false,
            doc: String::new(),
            score,
            matched: (0..shared).map(|i| format!("w{i}")).collect(),
        }
    }

    /// The score pairs are gate 4's, on Chris's store: a correction naming the very ruling it broke (112.4 against the
    /// next record's 39.0) and one whose top two records were both only on its topic (146.2 against 131.2).
    #[test]
    fn a_fit_stands_out_from_the_next_candidate() {
        let args = Args::default();
        let fit = |c: &[Candidate]| pick(&args, c).expect("no flag names a target").map(|c| c.show.clone());
        assert_eq!(fit(&[cand("ruling", 112.4, 5), cand("next", 39.0, 3)]), Some("ruling".to_string()));
        assert_eq!(fit(&[cand("topic", 146.2, 6), cand("next", 131.2, 6)]), None, "a high score that does not stand out");
        assert_eq!(fit(&[cand("only", 30.0, 3)]), Some("only".to_string()), "nothing beside it");
        assert_eq!(fit(&[cand("low", 11.0, 3)]), None, "under the floor");
        assert_eq!(fit(&[cand("one-word", 40.0, 1)]), None, "one shared word");
        let new = Args { new: true, ..Args::default() };
        assert!(pick(&new, &[cand("ruling", 112.4, 5)]).unwrap().is_none(), "--new: none fits");
    }
}
