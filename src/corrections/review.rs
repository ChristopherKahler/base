//! `base rule review` (K5, BO-16; D8): the queue of rule proposals, each shown with the prompts behind it and its
//! replay (K6), approved, edited or rejected one keystroke at a time.
//!
//! THE QUEUE (K5a, D8). Proposals are `ops:RuleProposal` records in the graph of the tier the correcting session stood
//! in (BO-15), so they outlive the session that made them: session start counts the pending ones left from earlier
//! sessions. Review reads both tiers' graphs, keeps each proposal with the tier it is in, and writes its status there:
//! `pending`, then `approved`, `rejected` or `edited`, with `reviewedAt`. An edit keeps the proposed fields and adds the
//! edited ones, so a later pass can see what the user changed.
//!
//! WHAT APPROVING DOES (K5d). A keyword gap adds the keywords where its target is served from: a domain's
//! `prompt_keywords` in the `domains.toml` that holds it, a rule's own topic words where the rule lives, a global
//! decision's own keywords. A new rule is written with its keywords as its own words and its prompt as its first
//! `fires_on` test. A rewrite never overwrites: the new wording is a new graph rule (or decision) written with the
//! supersede edge pair to the old one. A rule from `domains.toml` moves to the graph to be rewritten, because its graph
//! copy is rebuilt on every sync and an edge on it would not last (G0 question 2): the old wording is kept there as a
//! superseded graph rule and its line leaves the file.
//!
//! NEVER OFFERED AGAIN (K5e). A rejection keeps the proposal's fingerprint (kind, target and change), and
//! `base rule propose` refuses a proposal whose fingerprint was rejected ([`rejected`]).
//!
//! TOO BROAD (K6b, lynx's G0 verdict on question 8). The one-key review applies a TOO BROAD change with the flag in
//! front of the user. `--approve` and `--edit`, which an AI runs for the user, refuse one unless `--broad-ok` is given,
//! naming the share, so the AI has to show the flag first.

use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use oxigraph::model::TermRef;
use oxigraph::sparql::QueryResults;

use crate::config::BaseConfig;
use crate::crud;
use crate::domain::replay::{self, Change, Outcome, Target};
use crate::domain::rules::{self, Matcher, RuleTests};
use crate::domain::tier::Tier;

/// One proposal as stored.
#[derive(Debug, Clone, Default)]
pub struct Stored {
    pub id: String,
    pub iri: String,
    /// The named graph that holds it.
    pub graph: String,
    /// A working directory in the tier that holds it: writes to it go there.
    pub tier_cwd: PathBuf,
    pub tier: Option<Tier>,
    pub status: String,
    /// `keyword-gap`, `rewrite`, `new-rule`.
    pub kind: String,
    pub meaning: String,
    pub target_kind: String,
    pub target_id: String,
    pub target_domain: String,
    pub target_text: Option<String>,
    pub text: Option<String>,
    pub keywords: Vec<String>,
    pub fires_on: Option<String>,
    pub prompt: Option<String>,
    pub marker: Option<String>,
    pub signals: Vec<String>,
    pub session: Option<String>,
    pub title: Option<String>,
    pub turn: Option<String>,
    pub fingerprint: Option<String>,
    pub created: Option<String>,
    pub reviewed: Option<String>,
    /// `correction` or `tune` (BO-17); empty on a proposal written before BO-17 (a correction's).
    pub origin: String,
    /// What the rule pass saw behind it.
    pub evidence: Vec<String>,
    /// A drop-keyword proposal's keywords to take away.
    pub dropped: Vec<String>,
    /// A merge's second rule.
    pub merge_target: Option<String>,
    /// A split's second part.
    pub second_text: Option<String>,
    pub second_keywords: Vec<String>,
}

impl Stored {
    /// `keyword gap`, `new rule`, `rewrite`.
    pub fn kind_label(&self) -> String {
        self.kind.replace('-', " ")
    }

    /// The change approving it makes, as replay runs it.
    pub fn change(&self) -> Result<Change, String> {
        let target = |kind: &str, id: &str| match kind {
            "domain" => Ok(Target::Domain(id.to_string())),
            "rule" => Ok(Target::Rule(id.to_string())),
            "decision" => Ok(Target::Decision(id.to_string())),
            other => Err(format!("{}: a target of kind '{other}' is not one this build applies", self.id)),
        };
        let rule_only = |what: &str| -> Result<Target, String> {
            match self.target_kind.as_str() {
                "rule" => Ok(Target::Rule(self.target_id.clone())),
                k => Err(format!("{}: a {what} applies to a rule, not a {k}", self.id)),
            }
        };
        let wording = || self.text.clone().filter(|t| !t.trim().is_empty()).ok_or_else(no_wording);
        match self.kind.as_str() {
            "keyword-gap" => Ok(Change::keywords(target(&self.target_kind, &self.target_id)?, self.keywords.clone(), Vec::new())),
            "new-rule" => Ok(Change::keywords(
                Target::NewRule {
                    domain: self.target_id.clone(),
                    text: self.text.clone().ok_or_else(|| format!("{}: a new rule with no wording", self.id))?,
                },
                self.keywords.clone(),
                Vec::new(),
            )),
            "rewrite" => Ok(Change { text: Some(wording()?), ..Change::keywords(target(&self.target_kind, &self.target_id)?, Vec::new(), Vec::new()) }),
            // BO-17's kinds. A drop-keyword proposal takes its keywords away and adds the narrower ones it carries.
            "drop-keyword" => {
                if self.dropped.is_empty() {
                    return Err(format!("{}: a drop-keyword proposal with no keyword to drop", self.id));
                }
                Ok(Change::keywords(target(&self.target_kind, &self.target_id)?, self.keywords.clone(), self.dropped.clone()))
            }
            "merge" => {
                let other = self.merge_target.clone().ok_or_else(|| format!("{}: a merge with no second rule", self.id))?;
                Ok(Change { text: Some(wording()?), merge: Some(other), ..Change::keywords(rule_only("merge")?, Vec::new(), Vec::new()) })
            }
            "split" => {
                let second = self.second_text.clone().filter(|t| !t.trim().is_empty());
                let second = second.ok_or_else(|| format!("{}: a split with no second part", self.id))?;
                Ok(Change {
                    text: Some(wording()?),
                    second: Some((second, self.second_keywords.clone())),
                    ..Change::keywords(rule_only("split")?, self.keywords.clone(), Vec::new())
                })
            }
            "retire" => Ok(Change { retire: true, ..Change::keywords(rule_only("retirement")?, Vec::new(), Vec::new()) }),
            other => Err(format!("{}: proposal kind '{other}' is not one this build applies", self.id)),
        }
    }

    /// The change with the user's edits: new wording and keywords, each only when given.
    pub fn edited(&self, text: Option<&str>, keywords: Option<&[String]>) -> Result<Change, String> {
        // Refused rather than dropped: the edit is stored as what the user changed.
        let has_text = text.is_some_and(|t| !t.trim().is_empty());
        if matches!(self.kind.as_str(), "keyword-gap" | "drop-keyword") && has_text {
            return Err(format!("{} is a {}: it changes keywords, not wording; give --keywords", self.id, self.kind_label()));
        }
        if matches!(self.kind.as_str(), "rewrite" | "merge") && keywords.is_some() {
            return Err(format!("{} is a {}: it changes the wording, not keywords; give --text", self.id, self.kind_label()));
        }
        if self.kind == "retire" && (has_text || keywords.is_some()) {
            return Err(format!("{} retires a rule: there is nothing to edit; approve or reject it", self.id));
        }
        let mut p = self.clone();
        if let Some(t) = text.map(str::trim).filter(|t| !t.is_empty()) {
            p.text = Some(t.to_string());
        }
        if let Some(k) = keywords {
            p.keywords = k.to_vec();
        }
        p.change()
    }
}

/// The tiers to read, each as a working directory in it, never the same graph twice: `rule update`'s own list.
fn tier_cwds(cwd: &Path) -> Vec<(Tier, PathBuf)> {
    crud::rule::tier_cwds(cwd)
}

/// Every proposal in both tiers' graphs, lowest id first.
pub fn load(config: &BaseConfig, cwd: &Path) -> Vec<Stored> {
    let ns = &config.namespace;
    let q = format!(
        "{}\nSELECT ?x ?g ?pred ?obj WHERE {{ GRAPH ?g {{ ?x a {p}:RuleProposal ; ?pred ?obj }} }}",
        crud::prefixes(ns),
        p = ns.prefix
    );
    let mut out: Vec<Stored> = Vec::new();
    for (tier, c) in tier_cwds(cwd) {
        let Ok(path) = crud::workspace_graph_path(&c) else { continue };
        if !path.is_file() {
            continue;
        }
        let Ok(store) = crate::store::load_graph(&path) else { continue };
        let Ok(QueryResults::Solutions(rows)) = crate::store::query(&store, &q) else { continue };
        let mut by: HashMap<String, Stored> = HashMap::new();
        for row in rows.filter_map(Result::ok) {
            let named = |k: &str| match row.get(k).map(Into::into) {
                Some(TermRef::NamedNode(n)) => Some(n.as_str().to_string()),
                _ => None,
            };
            let lit = |k: &str| match row.get(k).map(Into::into) {
                Some(TermRef::Literal(l)) => Some(l.value().to_string()),
                _ => None,
            };
            let (Some(x), Some(g), Some(pred)) = (named("x"), named("g"), named("pred")) else { continue };
            let s = by.entry(x.clone()).or_insert_with(|| Stored {
                iri: x.clone(),
                graph: g.clone(),
                tier_cwd: c.clone(),
                tier: Some(tier),
                ..Stored::default()
            });
            let Some(v) = lit("obj") else { continue };
            match pred.rsplit(['#', '/']).next().unwrap_or_default() {
                "proposalId" => s.id = v,
                "status" => s.status = v,
                "proposalKind" => s.kind = v,
                "meaning" => s.meaning = v,
                "targetKind" => s.target_kind = v,
                "targetId" => s.target_id = v,
                "targetDomain" => s.target_domain = v,
                "targetText" => s.target_text = Some(v),
                "proposedText" => s.text = Some(v),
                "proposedKeyword" => s.keywords.push(v),
                "firesOn" => s.fires_on = Some(v),
                "triggerPrompt" => s.prompt = Some(v),
                "markerLine" => s.marker = Some(v),
                "signal" => s.signals.push(v),
                "fromSession" => s.session = Some(v),
                "fromTitle" => s.title = Some(v),
                "fromTurn" => s.turn = Some(v),
                "fingerprint" => s.fingerprint = Some(v),
                "createdAt" => s.created = Some(v),
                "reviewedAt" => s.reviewed = Some(v),
                "proposalOrigin" => s.origin = v,
                "evidence" => s.evidence.push(v),
                "droppedKeyword" => s.dropped.push(v),
                "mergeTarget" => s.merge_target = Some(v),
                "secondText" => s.second_text = Some(v),
                "secondKeyword" => s.second_keywords.push(v),
                _ => {}
            }
        }
        for mut s in by.into_values().filter(|s| !s.id.is_empty()) {
            s.keywords.sort();
            s.signals.sort();
            s.dropped.sort();
            s.second_keywords.sort();
            s.evidence.sort();
            out.push(s);
        }
    }
    out.sort_by(|a, b| number(&a.id).cmp(&number(&b.id)).then_with(|| a.id.cmp(&b.id)));
    out
}

fn number(id: &str) -> u32 {
    id.trim_start_matches("p-").parse().unwrap_or(u32::MAX)
}

/// `p-0007` for `p-0007`, `p-7`, `7` or `0007`.
pub fn normalize_id(id: &str) -> String {
    let t = id.trim();
    match t.trim_start_matches("p-").parse::<u32>() {
        Ok(n) => format!("p-{n:04}"),
        Err(_) => t.to_string(),
    }
}

/// The rejected proposal with this fingerprint, in `store` (K5e): its id and when it was rejected.
pub fn rejected(store: &oxigraph::store::Store, ns: &crate::config::NamespaceConfig, fingerprint: &str) -> Option<(String, String)> {
    let q = format!(
        "{}\nSELECT ?id ?when WHERE {{ GRAPH ?g {{ ?x a {p}:RuleProposal ; {p}:status \"rejected\" ; {p}:fingerprint \"{fp}\" ; \
         {p}:proposalId ?id . OPTIONAL {{ ?x {p}:reviewedAt ?when }} }} }}",
        crud::prefixes(ns),
        p = ns.prefix,
        fp = crud::escape_sparql_literal(fingerprint)
    );
    let Ok(QueryResults::Solutions(mut rows)) = crate::store::query(store, &q) else { return None };
    let row = rows.next()?.ok()?;
    let lit = |k: &str| match row.get(k).map(Into::into) {
        Some(TermRef::Literal(l)) => Some(l.value().to_string()),
        _ => None,
    };
    Some((lit("id")?, lit("when").map(|w| w.chars().take(10).collect()).unwrap_or_else(|| "an earlier review".to_string())))
}

/// A proposal with this fingerprint in `store`, in any status, with its id and status (BO-17: the rule pass never
/// writes the same proposal twice, and never one the user rejected, K5e).
pub fn fingerprint_known(store: &oxigraph::store::Store, ns: &crate::config::NamespaceConfig, fingerprint: &str) -> Option<(String, String)> {
    let q = format!(
        "{}\nSELECT ?id ?st WHERE {{ GRAPH ?g {{ ?x a {p}:RuleProposal ; {p}:fingerprint \"{fp}\" ; {p}:proposalId ?id ; {p}:status ?st }} }}",
        crud::prefixes(ns),
        p = ns.prefix,
        fp = crud::escape_sparql_literal(fingerprint)
    );
    let Ok(QueryResults::Solutions(mut rows)) = crate::store::query(store, &q) else { return None };
    let row = rows.next()?.ok()?;
    let lit = |k: &str| match row.get(k).map(Into::into) {
        Some(TermRef::Literal(l)) => Some(l.value().to_string()),
        _ => None,
    };
    Some((lit("id")?, lit("st")?))
}

/// The count session start shows (K5b): pending proposals in `store`.
pub fn pending_count(store: &oxigraph::store::Store, ns: &crate::config::NamespaceConfig) -> usize {
    let q = format!(
        "{}\nSELECT (COUNT(DISTINCT ?x) AS ?n) WHERE {{ GRAPH ?g {{ ?x a {p}:RuleProposal ; {p}:status \"pending\" }} }}",
        crud::prefixes(ns),
        p = ns.prefix
    );
    let Ok(QueryResults::Solutions(mut rows)) = crate::store::query(store, &q) else { return 0 };
    rows.next()
        .and_then(Result::ok)
        .and_then(|r| r.get("n").map(|t| crud::term_display(t.into())))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// The session start line (K5b).
pub fn session_start_line(pending: usize) -> String {
    format!("rule proposals: {pending} pending · base rule review")
}

// ─── Acting on one ───────────────────────────────────────────────────────────

/// What `base rule review` was asked to do.
#[derive(Debug, Clone)]
pub enum Action {
    List,
    Approve { id: String, broad_ok: bool },
    Reject { id: String, reason: Option<String> },
    Edit { id: String, text: Option<String>, keywords: Option<String>, broad_ok: bool },
}

/// Run `base rule review` with a flag, or list on a pipe. On a terminal with no flag it runs [`interactive`]. What to
/// print, or the refusal (exit 1).
pub fn run(config: &BaseConfig, cwd: &Path, action: &Action) -> Result<String, String> {
    match action {
        Action::List => Ok(listing(config, cwd, false)),
        Action::Approve { id, broad_ok } => {
            let p = pending_one(config, cwd, id)?;
            let change = p.change()?;
            approve(config, cwd, &p, &change, "approved", *broad_ok, &[])
        }
        Action::Reject { id, reason } => {
            let p = pending_one(config, cwd, id)?;
            reject(config, &p, reason.as_deref())
        }
        Action::Edit { id, text, keywords, broad_ok } => {
            if text.is_none() && keywords.is_none() {
                return Err("--edit needs --text \"...\" or --keywords \"a, b\" (or both)".to_string());
            }
            let p = pending_one(config, cwd, id)?;
            let kw = keywords.as_deref().map(crate::domain::global_decisions::parse_keywords);
            let change = p.edited(text.as_deref(), kw.as_deref())?;
            let mut extra: Vec<(&str, String)> = Vec::new();
            if let Some(t) = text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                extra.push(("editedText", t.to_string()));
            }
            for k in kw.iter().flatten() {
                extra.push(("editedKeyword", k.clone()));
            }
            approve(config, cwd, &p, &change, "edited", *broad_ok, &extra)
        }
    }
}

/// The pending proposal `id` names, or why not.
fn pending_one(config: &BaseConfig, cwd: &Path, id: &str) -> Result<Stored, String> {
    let want = normalize_id(id);
    let all = load(config, cwd);
    let Some(p) = all.into_iter().find(|p| p.id == want) else {
        return Err(format!("no proposal {want} in this workspace or the global tier (base rule review lists them)"));
    };
    if p.status != "pending" {
        let when = p.reviewed.as_deref().map(|w| format!(" on {}", w.chars().take(10).collect::<String>())).unwrap_or_default();
        return Err(format!("{} is {}{when}, not pending: nothing changed", p.id, p.status));
    }
    Ok(p)
}

/// Replay, then apply `change` and store `status`. A TOO BROAD change needs `broad_ok`.
fn approve(
    config: &BaseConfig,
    cwd: &Path,
    p: &Stored,
    change: &Change,
    status: &str,
    broad_ok: bool,
    extra: &[(&str, String)],
) -> Result<String, String> {
    let outcome = replay::run(config, cwd, change)?;
    if outcome.too_broad() && !broad_ok {
        return Err(format!(
            "{} is TOO BROAD: it would be served on {} of the last {} prompts (limit {:.0}%). Show the user its replay \
             (base rule replay {}) first; with their yes, run this again with --broad-ok. Nothing changed.",
            p.id,
            outcome.percent(),
            outcome.replayed,
            outcome.limit * 100.0,
            p.id
        ));
    }
    apply_and_store(config, cwd, p, change, status, extra, &outcome)
}

fn apply_and_store(
    config: &BaseConfig,
    cwd: &Path,
    p: &Stored,
    change: &Change,
    status: &str,
    extra: &[(&str, String)],
    outcome: &Outcome,
) -> Result<String, String> {
    // Claimed first, under its tier's lock and only while still pending, so a second review of the same proposal stops
    // here before anything is applied twice. A failed apply puts it back to pending.
    set_status(config, p, status, extra)?;
    let applied = match apply(config, cwd, p, change) {
        Ok(a) => a,
        Err(e) => {
            return Err(match reopen(config, p) {
                Ok(()) => format!("{e} (nothing applied; {} is still pending)", p.id),
                Err(back) => format!("{e}; and putting {} back to pending failed: {back}", p.id),
            });
        }
    };
    // A change that took a line out of `domains.toml` (a rewrite, merge, split or retirement moves the rule to the
    // graph) leaves that line's synced copy in the graph until the next sync; sync now, as every hook would first, so
    // `rule list` right after says what the hooks will serve.
    crate::hook::user_prompt_submit::ensure_domain_sync_pub(config, cwd);
    let verb = if status == "edited" { "edited and applied" } else { "approved" };
    let mut s = format!("{verb} {}: {}\n", p.id, applied.line);
    if let Err(e) = write_fields(config, p, None, &[], &[("appliedTo", applied.to.clone())]) {
        s.push_str(&format!("  (where it was written was not recorded on {}: {e})\n", p.id));
    }
    for n in &applied.notes {
        s.push_str(&format!("  {n}\n"));
    }
    let replayed = if outcome.too_broad() { format!("replay {} · {}", outcome.percent(), outcome.flag()) } else { outcome.summary() };
    s.push_str(&format!("  {replayed}\n"));
    if let Some(d) = applied.domain.as_deref()
        && let Some(line) = crate::domain::rule_test::after_change_line(config, cwd, d)
    {
        s.push_str(&format!("  {line}\n"));
    }
    Ok(s)
}

fn reject(config: &BaseConfig, p: &Stored, reason: Option<&str>) -> Result<String, String> {
    let mut fields: Vec<(&str, String)> = Vec::new();
    if let Some(r) = reason.map(str::trim).filter(|r| !r.is_empty()) {
        fields.push(("rejectReason", r.to_string()));
    }
    set_status(config, p, "rejected", &fields)?;
    Ok(format!("rejected {}; this proposal will not be offered again\n", p.id))
}

/// Store `status` on `p`, with `reviewedAt` and `fields`, in the graph that holds it, under that tier's lock, only while
/// it is still pending there.
fn set_status(config: &BaseConfig, p: &Stored, status: &str, fields: &[(&str, String)]) -> Result<(), String> {
    let mut insert: Vec<(&str, String)> = vec![("status", status.to_string()), ("reviewedAt", crud::now_iso())];
    insert.extend(fields.iter().cloned());
    write_fields(config, p, Some("pending"), &["status"], &insert)
}

/// An approval whose change could not be applied: pending again, with nothing of the review left on it.
fn reopen(config: &BaseConfig, p: &Stored) -> Result<(), String> {
    let review = ["status", "reviewedAt", "rejectReason", "editedText", "editedKeyword", "appliedTo"];
    write_fields(config, p, None, &review, &[("status", "pending".to_string())])
}

/// On `p`, in the graph that holds it, under that tier's lock: remove every value of `delete`, then add `insert`
/// (`reviewedAt` as a dateTime). With `require`, only while its status is that, else nothing is written.
fn write_fields(
    config: &BaseConfig,
    p: &Stored,
    require: Option<&str>,
    delete: &[&str],
    insert: &[(&str, String)],
) -> Result<(), String> {
    let ns = &config.namespace;
    let pfx = &ns.prefix;
    let (iri, graph) = (p.iri.clone(), p.graph.clone());
    let mut lines = String::new();
    for (pred, v) in insert {
        let value = if *pred == "reviewedAt" {
            format!("\"{v}\"^^xsd:dateTime")
        } else {
            format!("\"{}\"", crud::escape_sparql_literal(v))
        };
        lines.push_str(&format!("    <{iri}> {pfx}:{pred} {value} .\n"));
    }
    let mut sparql = String::new();
    for pred in delete {
        sparql.push_str(&format!(
            "DELETE {{ GRAPH <{graph}> {{ <{iri}> {pfx}:{pred} ?v }} }} WHERE {{ GRAPH <{graph}> {{ <{iri}> {pfx}:{pred} ?v }} }} ;\n"
        ));
    }
    sparql.push_str(&format!("INSERT DATA {{ GRAPH <{graph}> {{\n{lines}  }} }}"));
    let id = p.id.clone();
    let require = require.map(String::from);
    crud::load_read_then_mutate(&p.tier_cwd, ns, |store| {
        if let Some(want) = require.as_deref() {
            let ask = format!("{}\nASK {{ GRAPH <{graph}> {{ <{iri}> {pfx}:status \"{want}\" }} }}", crud::prefixes(ns));
            if !matches!(crate::store::query(store, &ask)?, QueryResults::Boolean(true)) {
                anyhow::bail!("{id} is no longer {want} (another review got to it first); nothing changed");
            }
        }
        Ok(sparql)
    })
    .map_err(|e| format!("{e:#}"))
}

// ─── Applying (K5d) ──────────────────────────────────────────────────────────

/// What an approval wrote.
struct Applied {
    /// One line: what changed.
    line: String,
    /// Further lines the user needs: a rule that moved from the file to the graph.
    notes: Vec<String>,
    /// Where: `domains.toml (workspace tier)`, `graph (global tier)`.
    to: String,
    /// The domain whose rule tests to run afterwards.
    domain: Option<String>,
}

fn quoted(list: &[String]) -> String {
    list.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", ")
}

fn noun(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

/// The working directory of a tier: the review's own for the workspace, `~/.base-gbl` for the global tier.
fn cwd_of(cwd: &Path, tier: Tier) -> PathBuf {
    match tier {
        Tier::Workspace => cwd.to_path_buf(),
        Tier::Global => crate::home::home_root().map(|h| h.join(".base-gbl")).unwrap_or_else(|| cwd.to_path_buf()),
    }
}

fn apply(config: &BaseConfig, cwd: &Path, p: &Stored, change: &Change) -> Result<Applied, String> {
    let ns = &config.namespace;
    // BO-17's kinds first: each names a rule, and a retirement changes no wording or keyword.
    if let Target::Rule(r) = &change.target {
        if change.retire {
            return retire_rule(config, cwd, r);
        }
        if let (Some(other), Some(text)) = (change.merge.as_deref(), change.text.as_deref()) {
            return merge_rules(config, cwd, r, other, text);
        }
        if let (Some((second, words)), Some(text)) = (&change.second, change.text.as_deref()) {
            return split_rule(config, cwd, r, text, &change.add, second, words);
        }
    }
    if change.text.is_none() && change.add.is_empty() && change.drop.is_empty() && !matches!(change.target, Target::NewRule { .. }) {
        return Err(format!("{}: no keyword to add or drop and no new wording; nothing to change", p.id));
    }
    match (&change.target, &change.text) {
        (Target::Domain(name), None) => apply_domain_keywords(cwd, p, name, &change.add, &change.drop),
        (Target::Rule(r), None) => apply_rule_words(config, cwd, r, &change.add, &change.drop),
        (Target::Decision(slug), None) => apply_decision_keywords(config, cwd, slug, &change.add, &change.drop),
        (Target::NewRule { domain, text }, _) => {
            let matchers = if change.add.is_empty() { Vec::new() } else { vec![Matcher::for_topic(change.add.clone())] };
            let mut tests = RuleTests::default();
            if let Some(f) = p.fires_on.as_deref().or(p.prompt.as_deref()) {
                tests.add("firesOn", f);
            }
            let domain = crate::domain::canonical_name(cwd, domain);
            crud::rule::add_with_tests(&p.tier_cwd, ns, &domain, text, None, None, &matchers, &tests)
                .map_err(|e| format!("{e:#}"))?;
            let short = crate::domain::rule_test::short_ref(&domain, &rules::rule_id(&domain, text));
            let words = if change.add.is_empty() { String::new() } else { format!(" · its words {}", quoted(&change.add)) };
            let to = format!("graph ({} tier)", p.tier.unwrap_or(Tier::Workspace).label());
            Ok(Applied {
                line: format!("new rule {short} added to domain {domain}, in {to}{words} · tests: {}", crate::domain::rule_test::tests_line(&tests)),
                notes: Vec::new(),
                to,
                domain: Some(domain),
            })
        }
        (Target::Rule(r), Some(text)) => rewrite_rule(config, cwd, r, text),
        (Target::Decision(slug), Some(text)) => rewrite_decision(config, cwd, p, slug, text),
        (Target::Domain(name), Some(_)) => Err(format!("domain {name} has no wording to rewrite")),
    }
}

/// The tier whose `domains.toml` holds `name`: the workspace's when it does (it overlays the global one), else the
/// global one's; `None` when neither file does.
fn domain_file_tier(cwd: &Path, name: &str) -> Option<(Tier, String)> {
    let want = crud::slugify(name);
    for tier in [Tier::Workspace, Tier::Global] {
        let Some(file) = crate::domain::tier::domains_toml_for(cwd, tier).filter(|f| f.is_file()) else { continue };
        if let Some(d) = crate::domain::load_domains_file(&file, None).into_iter().find(|d| crud::slugify(&d.name) == want) {
            return Some((tier, d.name));
        }
    }
    None
}

fn apply_domain_keywords(cwd: &Path, p: &Stored, name: &str, add: &[String], drop: &[String]) -> Result<Applied, String> {
    let (tier, name) = domain_file_tier(cwd, name)
        .unwrap_or((p.tier.unwrap_or(Tier::Workspace), crate::domain::canonical_name(cwd, name)));
    let global = tier == Tier::Global;
    let existing: Vec<String> = crate::domain::load_domains(cwd)
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| d.prompt_keywords)
        .unwrap_or_default();
    let mut added: Vec<String> = Vec::new();
    for k in add.iter().map(|k| k.trim()).filter(|k| !k.is_empty()) {
        if existing.iter().any(|e| e.eq_ignore_ascii_case(k)) {
            continue;
        }
        crate::domain::add_trigger(cwd, global, &name, Some(k), None).map_err(|e| format!("{e:#}"))?;
        added.push(k.to_string());
    }
    let mut dropped: Vec<String> = Vec::new();
    for k in drop {
        if let Some(e) = existing.iter().find(|e| e.trim().eq_ignore_ascii_case(k.trim())) {
            crate::domain::remove_trigger(cwd, global, &name, Some(e), None).map_err(|e| format!("{e:#}"))?;
            dropped.push(e.clone());
        }
    }
    let to = format!("domains.toml ({} tier)", tier.label());
    let mut parts: Vec<String> = Vec::new();
    if !added.is_empty() {
        parts.push(format!("{} {} added to domain {name}", noun(added.len(), "keyword", "keywords"), quoted(&added)));
    }
    if !dropped.is_empty() {
        parts.push(format!("{} {} dropped from domain {name}", noun(dropped.len(), "keyword", "keywords"), quoted(&dropped)));
    }
    if parts.is_empty() {
        parts.push(format!("domain {name} already had these keywords; nothing to change"));
    }
    Ok(Applied { line: format!("{}, in {to}", parts.join("; ")), notes: Vec::new(), to, domain: Some(name) })
}

/// The one rule `spec` names, with where it lives.
fn found_rule(config: &BaseConfig, cwd: &Path, spec: &str) -> Result<crud::rule::FoundRule, String> {
    let (domain, id) = crud::rule::parse_rule_ref(spec)?;
    let found = crud::rule::find(cwd, &config.namespace, domain.as_deref(), &id).map_err(|e| format!("{e:#}"))?;
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!("no rule '{spec}' in either tier any more")),
        many => Err(format!("'{spec}' fits {} rules; give more of the id", many.len())),
    }
}

/// A graph rule's rationale and matchers.
fn graph_rule_parts(config: &BaseConfig, tier_cwd: &Path, iri: &str) -> (Option<String>, Vec<Matcher>) {
    let ns = &config.namespace;
    let q = format!("{}\nSELECT ?pred ?obj WHERE {{ GRAPH ?g {{ <{iri}> ?pred ?obj }} }}", crud::prefixes(ns));
    let mut rationale = None;
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Ok(store) = crud::load_workspace_graph(tier_cwd)
        && let Ok(QueryResults::Solutions(rows)) = crate::store::query(&store, &q)
    {
        for row in rows.filter_map(Result::ok) {
            let (Some(pred), Some(obj)) = (row.get("pred"), row.get("obj")) else { continue };
            let pred = crud::term_display(pred.into());
            let local = pred.trim_end_matches('>').rsplit(['#', '/']).next().unwrap_or_default().to_string();
            let v = crud::term_display(obj.into());
            if local == "rationale" {
                rationale = Some(v);
            } else if rules::MATCH_PREDICATES.contains(&local.as_str()) {
                pairs.push((local, v));
            }
        }
    }
    (rationale, rules::matchers_from_literals(pairs.iter().map(|(p, v)| (p.as_str(), v.as_str()))))
}

/// Replace a graph rule's matcher literals with `matchers`, under its tier's lock.
fn store_graph_matchers(config: &BaseConfig, tier_cwd: &Path, iri: &str, matchers: &[Matcher]) -> Result<(), String> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let preds = rules::MATCH_PREDICATES.iter().map(|m| format!("{p}:{m}")).collect::<Vec<_>>().join(", ");
    let inserts: String = rules::matcher_literals(matchers)
        .iter()
        .map(|(pred, v)| format!("<{iri}> {p}:{pred} \"{}\" .\n", crud::escape_sparql_literal(v)))
        .collect();
    let mut sparql = format!(
        "{}\nDELETE {{ GRAPH ?g {{ <{iri}> ?mp ?mv }} }} WHERE {{ GRAPH ?g {{ <{iri}> ?mp ?mv . FILTER(?mp IN ({preds})) }} }}",
        crud::prefixes(ns)
    );
    if !inserts.is_empty() {
        sparql.push_str(&format!(" ;\nINSERT {{ GRAPH ?g {{\n{inserts}}} }}\nWHERE {{ GRAPH ?g {{ <{iri}> {p}:ruleText ?text }} }}"));
    }
    let (store, trig_path, _lock) = crud::lock_and_load(tier_cwd).map_err(|e| format!("{e:#}"))?;
    crate::store::update_and_write(&store, &trig_path, &sparql, crate::store::Scope::Wide, crate::store::Intent::Knowledge)
        .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

fn apply_rule_words(config: &BaseConfig, cwd: &Path, spec: &str, add: &[String], drop: &[String]) -> Result<Applied, String> {
    let rule = found_rule(config, cwd, spec)?;
    let short = crate::domain::rule_test::short_ref(&rule.domain, &rule.id);
    let Some((home, _)) = rule.homes_for(false).into_iter().next().cloned() else {
        return Err(format!("{short} has no home base can write (an extension's rule, or a synced copy of a line that is gone)"));
    };
    let to = home.label();
    match &home {
        crud::rule::TestHome::Toml { file, .. } => {
            let now: Vec<Matcher> = crate::domain::load_domains_file(file, None)
                .into_iter()
                .filter(|d| crud::slugify(&d.name) == crud::slugify(&rule.domain))
                .flat_map(|d| d.rules)
                .find(|r| rules::rule_id(&rule.domain, r.text()) == rule.id)
                .map(|r| r.matchers().to_vec())
                .unwrap_or_default();
            let next = replay::edit_topic_words(&now, add, drop);
            if !crate::domain::set_rule_matchers(file, &rule.domain, &rule.id, &next).map_err(|e| format!("{e:#}"))? {
                return Err(format!("{short} changed after it was read; nothing was written"));
            }
        }
        crud::rule::TestHome::Graph { cwd: tier_cwd, iri, .. } => {
            let iri = iri.trim_start_matches('<').trim_end_matches('>').to_string();
            let (_, now) = graph_rule_parts(config, tier_cwd, &iri);
            let next = replay::edit_topic_words(&now, add, drop);
            store_graph_matchers(config, tier_cwd, &iri, &next)?;
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if !add.is_empty() {
        parts.push(format!("{} {} added to rule {short}", noun(add.len(), "word", "words"), quoted(add)));
    }
    if !drop.is_empty() {
        parts.push(format!("{} {} dropped from rule {short}", noun(drop.len(), "word", "words"), quoted(drop)));
    }
    Ok(Applied { line: format!("{}, in {to}", parts.join("; ")), notes: Vec::new(), to, domain: Some(rule.domain) })
}

fn apply_decision_keywords(config: &BaseConfig, cwd: &Path, slug: &str, add: &[String], drop: &[String]) -> Result<Applied, String> {
    let ns = &config.namespace;
    let (tier, tier_cwd) = decision_tier(config, cwd, slug)?;
    // Its keywords in every graph, whatever domain it is under: `update_with` replaces the whole list.
    let iri = crud::build_iri(ns, "decision", slug);
    let q = format!(
        "{}\nSELECT DISTINCT ?kw WHERE {{ GRAPH ?g {{ <{iri}> {p}:{kwp} ?kw }} }}",
        crud::prefixes(ns),
        p = ns.prefix,
        kwp = crate::domain::global_decisions::PRED_KEYWORD
    );
    let mut now: Vec<String> = Vec::new();
    if let Some(store) = crate::store::load_merged(cwd)
        && let Ok(QueryResults::Solutions(rows)) = crate::store::query(&store, &q)
    {
        now.extend(rows.filter_map(Result::ok).filter_map(|r| r.get("kw").map(|k| crud::term_display(k.into()))));
    }
    now.sort();
    let next = replay::edit_list(&now, add, drop);
    crud::decision::update_with(&tier_cwd, ns, slug, None, None, None, None, Some(&next)).map_err(|e| format!("{e:#}"))?;
    let to = format!("graph ({} tier)", tier.label());
    Ok(Applied {
        line: format!("decision {slug} keywords now {}, in {to}", if next.is_empty() { "none".to_string() } else { quoted(&next) }),
        notes: Vec::new(),
        to,
        domain: None,
    })
}

/// The tier whose graph holds decision `slug`.
fn decision_tier(config: &BaseConfig, cwd: &Path, slug: &str) -> Result<(Tier, PathBuf), String> {
    let ns = &config.namespace;
    let iri = crud::build_iri(ns, "decision", slug);
    let ask = format!("{}\nASK {{ GRAPH ?g {{ <{iri}> a {}:Decision }} }}", crud::prefixes(ns), ns.prefix);
    for (tier, c) in tier_cwds(cwd) {
        if let Ok(store) = crud::load_workspace_graph(&c)
            && matches!(crate::store::query(&store, &ask), Ok(QueryResults::Boolean(true)))
        {
            return Ok((tier, c));
        }
    }
    Err(format!("no decision '{slug}' in either tier any more"))
}

/// Rewrite a rule (K5d): the new wording as a new graph rule with the old one's rationale, matchers and tests, written
/// with the supersede edge pair to the old graph rule. A `domains.toml` rule first moves to the graph (G0 question 2).
fn rewrite_rule(config: &BaseConfig, cwd: &Path, spec: &str, text: &str) -> Result<Applied, String> {
    let ns = &config.namespace;
    let text = text.trim();
    if text.is_empty() {
        return Err(no_wording());
    }
    let rule = found_rule(config, cwd, spec)?;
    let short = crate::domain::rule_test::short_ref(&rule.domain, &rule.id);
    let Some((home, tests)) = rule.homes_for(false).into_iter().next().cloned() else {
        return Err(format!("{short} has no home base can write (an extension's rule, or a synced copy of a line that is gone)"));
    };
    let mut notes: Vec<String> = Vec::new();
    let (tier_cwd, tier, old_slug, rationale, matchers) = match &home {
        crud::rule::TestHome::Graph { cwd: tier_cwd, tier, iri } => {
            let iri = iri.trim_start_matches('<').trim_end_matches('>').to_string();
            let (rationale, matchers) = graph_rule_parts(config, tier_cwd, &iri);
            let slug = iri.strip_prefix(&crud::build_iri(ns, "rule", "")).unwrap_or_default().to_string();
            (tier_cwd.clone(), *tier, slug, rationale, matchers)
        }
        crud::rule::TestHome::Toml { file, tier } => {
            let entry = crate::domain::load_domains_file(file, None)
                .into_iter()
                .filter(|d| crud::slugify(&d.name) == crud::slugify(&rule.domain))
                .flat_map(|d| d.rules)
                .find(|r| rules::rule_id(&rule.domain, r.text()) == rule.id)
                .ok_or_else(|| format!("{short} changed after it was read; nothing was written"))?;
            let tier_cwd = cwd_of(cwd, *tier);
            let rationale = entry.rationale().map(String::from);
            let matchers = entry.matchers().to_vec();
            // The old wording as a graph rule first: the supersede edge needs a record that sync does not rebuild.
            let index = crud::rule::add_with_tests(&tier_cwd, ns, &rule.domain, &rule.text, rationale.as_deref(), None, &matchers, &tests)
                .map_err(|e| format!("{e:#}"))?;
            (tier_cwd, *tier, format!("{}/cli-{index}", crud::slugify(&rule.domain)), rationale, matchers)
        }
    };
    if old_slug.is_empty() {
        return Err(format!("{short}: its graph record has no rule IRI to supersede"));
    }
    if let Err(e) = crud::rule::add_with_tests(&tier_cwd, ns, &rule.domain, text, rationale.as_deref(), Some(&old_slug), &matchers, &tests) {
        // A file rule's graph copy was made for this write alone: without the new wording superseding it, it would be
        // served beside its own line, so it goes again.
        let mut msg = format!("{e:#}");
        if matches!(home, crud::rule::TestHome::Toml { .. })
            && let Some(index) = old_slug.rsplit("cli-").next().and_then(|i| i.parse::<u32>().ok())
            && let Err(undo) = crud::rule::remove(&tier_cwd, ns, &rule.domain, index)
        {
            msg.push_str(&format!("; and its graph copy {old_slug} could not be removed ({undo:#}): remove it with base rule remove"));
        }
        return Err(msg);
    }
    let new_short = crate::domain::rule_test::short_ref(&rule.domain, &rules::rule_id(&rule.domain, text));
    if let crud::rule::TestHome::Toml { file, .. } = &home {
        match crate::domain::remove_rule(file, &rule.domain, &rule.id) {
            Ok(true) => notes.push(format!(
                "the rule moved from {} to the graph ({} tier): the new wording is {new_short}, and the old one is kept \
                 there superseded (base rule list --domain {} --include-superseded)",
                file.display(),
                tier.label(),
                rule.domain
            )),
            Ok(false) | Err(_) => notes.push(format!(
                "the new wording {new_short} is in the graph and the old one is superseded there, but its line is still in \
                 {}: remove it from that file by hand",
                file.display()
            )),
        }
    }
    let to = format!("graph ({} tier)", tier.label());
    Ok(Applied {
        line: format!("rule {short} rewritten as {new_short}, in {to}; the old wording is superseded, not overwritten"),
        notes,
        to,
        domain: Some(rule.domain),
    })
}

/// Rewrite a decision (K5d): a new decision in the old one's domain, its rationale and keywords kept, written with the
/// supersede edge pair to the old one.
fn rewrite_decision(config: &BaseConfig, cwd: &Path, p: &Stored, slug: &str, text: &str) -> Result<Applied, String> {
    let ns = &config.namespace;
    let text = text.trim();
    if text.is_empty() {
        return Err(no_wording());
    }
    let (tier, tier_cwd) = decision_tier(config, cwd, slug)?;
    let iri = crud::build_iri(ns, "decision", slug);
    let q = format!(
        "{}\nSELECT ?r ?kw WHERE {{ GRAPH ?g {{ <{iri}> a {p}:Decision . OPTIONAL {{ <{iri}> {p}:rationale ?r }} }} \
         OPTIONAL {{ GRAPH ?k {{ <{iri}> {p}:{kwp} ?kw }} }} }}",
        crud::prefixes(ns),
        p = ns.prefix,
        kwp = crate::domain::global_decisions::PRED_KEYWORD
    );
    let mut rationale = String::new();
    let mut keywords: Vec<String> = Vec::new();
    if let Ok(store) = crud::load_workspace_graph(&tier_cwd)
        && let Ok(QueryResults::Solutions(rows)) = crate::store::query(&store, &q)
    {
        for row in rows.filter_map(Result::ok) {
            if let Some(r) = row.get("r") {
                rationale = crud::term_display(r.into());
            }
            if let Some(k) = row.get("kw").map(|k| crud::term_display(k.into()))
                && !keywords.contains(&k)
            {
                keywords.push(k);
            }
        }
    }
    let domain = if p.target_domain.is_empty() { slug.split('.').next().unwrap_or_default().to_string() } else { p.target_domain.clone() };
    let new_slug = crud::decision::log_with(&tier_cwd, ns, &domain, text, &rationale, None, Some(slug)).map_err(|e| format!("{e:#}"))?;
    if !keywords.is_empty() {
        crud::decision::update_with(&tier_cwd, ns, &new_slug, None, None, None, None, Some(&keywords)).map_err(|e| format!("{e:#}"))?;
    }
    let to = format!("graph ({} tier)", tier.label());
    Ok(Applied {
        line: format!("decision {slug} rewritten as {new_slug}, in {to}; the old one is superseded, not overwritten"),
        notes: Vec::new(),
        to,
        domain: None,
    })
}

// ─── BO-17's kinds: merge, split, retire ─────────────────────────────────────

/// A rule as a graph record, where a supersede edge or a retirement mark lasts. A `domains.toml` rule is copied to the
/// graph of its file's tier first, its rationale, matchers and tests kept, and its line leaves the file: its graph copy
/// is rebuilt on every sync, so nothing written on that copy would last (BO-16's G0 question 2).
struct GraphRule {
    domain: String,
    short: String,
    tier_cwd: PathBuf,
    tier: Tier,
    /// `<domain>/cli-N`.
    slug: String,
    iri: String,
    rationale: Option<String>,
    matchers: Vec<Matcher>,
    tests: RuleTests,
    /// Where the rule moved from, when it was a `domains.toml` line.
    moved: Option<String>,
}

fn as_graph_rule(config: &BaseConfig, cwd: &Path, spec: &str) -> Result<GraphRule, String> {
    let ns = &config.namespace;
    let rule = found_rule(config, cwd, spec)?;
    let short = crate::domain::rule_test::short_ref(&rule.domain, &rule.id);
    let Some((home, tests)) = rule.homes_for(false).into_iter().next().cloned() else {
        return Err(format!("{short} has no home base can write (an extension's rule, or a synced copy of a line that is gone)"));
    };
    match &home {
        crud::rule::TestHome::Graph { cwd: tier_cwd, tier, iri } => {
            let iri = iri.trim_start_matches('<').trim_end_matches('>').to_string();
            let (rationale, matchers) = graph_rule_parts(config, tier_cwd, &iri);
            let slug = iri.strip_prefix(&crud::build_iri(ns, "rule", "")).unwrap_or_default().to_string();
            if slug.is_empty() {
                return Err(format!("{short}: its graph record has no rule IRI"));
            }
            Ok(GraphRule {
                domain: rule.domain.clone(),
                short,
                tier_cwd: tier_cwd.clone(),
                tier: *tier,
                slug,
                iri,
                rationale,
                matchers,
                tests,
                moved: None,
            })
        }
        crud::rule::TestHome::Toml { file, tier } => {
            let entry = crate::domain::load_domains_file(file, None)
                .into_iter()
                .filter(|d| crud::slugify(&d.name) == crud::slugify(&rule.domain))
                .flat_map(|d| d.rules)
                .find(|r| rules::rule_id(&rule.domain, r.text()) == rule.id)
                .ok_or_else(|| format!("{short} changed after it was read; nothing was written"))?;
            let tier_cwd = cwd_of(cwd, *tier);
            let rationale = entry.rationale().map(String::from);
            let matchers = entry.matchers().to_vec();
            let index = crud::rule::add_with_tests(&tier_cwd, ns, &rule.domain, &rule.text, rationale.as_deref(), None, &matchers, &tests)
                .map_err(|e| format!("{e:#}"))?;
            let slug = format!("{}/cli-{index}", crud::slugify(&rule.domain));
            let moved = match crate::domain::remove_rule(file, &rule.domain, &rule.id) {
                Ok(true) => format!("the rule moved from {} to the graph ({} tier)", file.display(), tier.label()),
                Ok(false) | Err(_) => format!(
                    "the rule was copied to the graph ({} tier), but its line is still in {}: remove it from that file by hand",
                    tier.label(),
                    file.display()
                ),
            };
            Ok(GraphRule {
                domain: rule.domain.clone(),
                short,
                iri: crud::build_iri(ns, "rule", &slug),
                tier_cwd,
                tier: *tier,
                slug,
                rationale,
                matchers,
                tests,
                moved: Some(moved),
            })
        }
    }
}

/// The graph that holds `iri` in `tier_cwd`'s tier.
fn graph_of(config: &BaseConfig, tier_cwd: &Path, iri: &str) -> Result<String, String> {
    let ns = &config.namespace;
    let q = format!("{}\nSELECT ?g WHERE {{ GRAPH ?g {{ <{iri}> {}:ruleText ?t }} }} LIMIT 1", crud::prefixes(ns), ns.prefix);
    let store = crud::load_workspace_graph(tier_cwd).map_err(|e| format!("{e:#}"))?;
    match crate::store::query(&store, &q) {
        Ok(QueryResults::Solutions(mut rows)) => rows
            .next()
            .and_then(Result::ok)
            .and_then(|r| match r.get("g").map(Into::into) {
                Some(TermRef::NamedNode(n)) => Some(n.as_str().to_string()),
                _ => None,
            })
            .ok_or_else(|| format!("no graph holds {iri} any more")),
        _ => Err(format!("could not read the graph that holds {iri}")),
    }
}

/// Retire a rule (BO-17, lynx's G0 ruling on question 3): never deleted. It gets `retiredAt` and `status "retired"`,
/// no hook and no `select` serves it (`supersede::sparql_exclude_superseded` leaves it out), `base rule list
/// --include-superseded` still lists it, and `base rule unretire` brings it back.
fn retire_rule(config: &BaseConfig, cwd: &Path, spec: &str) -> Result<Applied, String> {
    let ns = &config.namespace;
    let g = as_graph_rule(config, cwd, spec)?;
    let graph = graph_of(config, &g.tier_cwd, &g.iri)?;
    let p = &ns.prefix;
    let sparql = format!(
        "{}\nINSERT DATA {{ GRAPH <{graph}> {{\n  <{iri}> {p}:{at} \"{now}\"^^xsd:dateTime .\n  <{iri}> {p}:status \"{st}\" .\n}} }}",
        crud::prefixes(ns),
        iri = g.iri,
        at = crate::supersede::PRED_RETIRED_AT,
        now = crud::now_iso(),
        st = crate::supersede::STATUS_RETIRED,
    );
    let (store, trig_path, _lock) = crud::lock_and_load(&g.tier_cwd).map_err(|e| format!("{e:#}"))?;
    crate::store::update_and_write(&store, &trig_path, &sparql, crate::store::Scope::Wide, crate::store::Intent::Knowledge)
        .map_err(|e| format!("{e:#}"))?;
    let to = format!("graph ({} tier)", g.tier.label());
    Ok(Applied {
        line: format!(
            "rule {} retired, in {to}: no hook serves it now; it stays listed (base rule list --domain {} --include-superseded) \
             and base rule unretire {} brings it back",
            g.short, g.domain, g.short
        ),
        notes: g.moved.into_iter().collect(),
        to,
        domain: Some(g.domain),
    })
}

/// Merge two rules (BO-17): one new graph rule of the merged wording, in the first rule's domain and tier, carrying
/// both rules' matchers and tests, superseding both. Both must be in one tier (a supersede edge never crosses tiers).
fn merge_rules(config: &BaseConfig, cwd: &Path, spec: &str, other: &str, text: &str) -> Result<Applied, String> {
    let ns = &config.namespace;
    let text = text.trim();
    if text.is_empty() {
        return Err(no_wording());
    }
    let a = as_graph_rule(config, cwd, spec)?;
    let b = as_graph_rule(config, cwd, other)?;
    if a.tier != b.tier {
        return Err(format!(
            "{} is in the {} tier and {} in the {} tier: a merge supersedes both, and a supersede edge never crosses tiers",
            a.short,
            a.tier.label(),
            b.short,
            b.tier.label()
        ));
    }
    let matchers = crate::domain::replay::union_matchers(&a.matchers, &b.matchers);
    let mut tests = a.tests.clone();
    for f in &b.tests.fires_on {
        tests.add("firesOn", f);
    }
    for q in &b.tests.quiet_on {
        tests.add("quietOn", q);
    }
    tests.fires_on.truncate(3);
    tests.quiet_on.truncate(2);
    let rationale = match (&a.rationale, &b.rationale) {
        (Some(x), Some(y)) if x != y => Some(format!("{x} {y}")),
        (Some(x), _) | (None, Some(x)) => Some(x.clone()),
        _ => None,
    };
    let index = crud::rule::add_with_tests(&a.tier_cwd, ns, &a.domain, text, rationale.as_deref(), Some(&a.slug), &matchers, &tests)
        .map_err(|e| format!("{e:#}"))?;
    let new_iri = crud::build_iri(ns, "rule", &format!("{}/cli-{index}", crud::slugify(&a.domain)));
    let graph = crud::workspace_graph_iri(ns, &crud::workspace_slug(&a.tier_cwd));
    let tier_label = crud::tier_label(&a.tier_cwd);
    let b_slug = b.slug.clone();
    crud::load_read_then_mutate(&a.tier_cwd, ns, |store| {
        let old = crud::supersede::resolve_slug(store, ns, &b_slug, &tier_label)?;
        crud::supersede::link_statement(store, ns, &graph, &old, &new_iri)
    })
    .map_err(|e| format!("{e:#}; the merged rule is written and supersedes {}, but not {}", a.short, b.short))?;
    let new_short = crate::domain::rule_test::short_ref(&a.domain, &rules::rule_id(&a.domain, text));
    let to = format!("graph ({} tier)", a.tier.label());
    Ok(Applied {
        line: format!("rules {} and {} merged as {new_short}, in {to}; both old ones are superseded, not deleted", a.short, b.short),
        notes: a.moved.into_iter().chain(b.moved).collect(),
        to,
        domain: Some(a.domain),
    })
}

/// Split a rule (BO-17): the first part rewrites it (the supersede edge pair, as a rewrite), the second is a new rule
/// beside it, on its own keywords. A record with two successors is a defect (`supersede::resolve_head`), so only the
/// first part supersedes the old wording.
fn split_rule(
    config: &BaseConfig,
    cwd: &Path,
    spec: &str,
    text: &str,
    first_words: &[String],
    second: &str,
    second_words: &[String],
) -> Result<Applied, String> {
    let ns = &config.namespace;
    let rule = found_rule(config, cwd, spec)?;
    let mut applied = rewrite_rule(config, cwd, spec, text)?;
    let tier = match rule.homes_for(false).into_iter().next().map(|(h, _)| h.tier()) {
        Some(t) => t,
        None => Tier::Workspace,
    };
    let tier_cwd = cwd_of(cwd, tier);
    let first = crate::domain::rule_test::short_ref(&rule.domain, &rules::rule_id(&rule.domain, text.trim()));
    if !first_words.is_empty() {
        let more = apply_rule_words(config, cwd, &first, first_words, &[])?;
        applied.notes.push(more.line);
    }
    let matchers = if second_words.is_empty() { Vec::new() } else { vec![Matcher::for_topic(second_words.to_vec())] };
    crud::rule::add_with_tests(&tier_cwd, ns, &rule.domain, second.trim(), Some(&format!("split from {first}")), None, &matchers, &RuleTests::default())
        .map_err(|e| format!("{e:#}; the first part is written ({first}), the second is not"))?;
    let second_short = crate::domain::rule_test::short_ref(&rule.domain, &rules::rule_id(&rule.domain, second.trim()));
    let words = if second_words.is_empty() { String::new() } else { format!(" · its words {}", quoted(second_words)) };
    applied.line = format!(
        "rule {} split: the first part is {first} (it supersedes the old wording), the second is {second_short}{words}, in {}",
        crate::domain::rule_test::short_ref(&rule.domain, &rule.id),
        applied.to
    );
    Ok(applied)
}

/// `base rule unretire <domain>.<id>`: clear a retirement, so the rule is served again (BO-17).
pub fn unretire(config: &BaseConfig, cwd: &Path, spec: &str) -> Result<String, String> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let (domain, id) = crud::rule::parse_rule_ref(spec)?;
    let q = format!(
        "{}\nSELECT ?g ?rule ?text ?d WHERE {{ GRAPH ?g {{ ?rule {p}:{at} ?when ; {p}:ruleText ?text . ?d {p}:hasRule ?rule }} }}",
        crud::prefixes(ns),
        at = crate::supersede::PRED_RETIRED_AT
    );
    let names: HashMap<String, String> = crate::domain::load_domains(cwd)
        .into_iter()
        .map(|d| (crud::build_iri(ns, "domain", &crud::slugify(&d.name)), d.name))
        .collect();
    let mut found: Vec<(Tier, PathBuf, String, String, String)> = Vec::new();
    for (tier, c) in tier_cwds(cwd) {
        let Ok(store) = crud::load_workspace_graph(&c) else { continue };
        let Ok(QueryResults::Solutions(rows)) = crate::store::query(&store, &q) else { continue };
        for row in rows.filter_map(Result::ok) {
            // IRIs whole (a display form shortens them), literals by value.
            let get = |k: &str| match row.get(k).map(Into::into) {
                Some(TermRef::NamedNode(n)) => Some(n.as_str().to_string()),
                Some(TermRef::Literal(l)) => Some(l.value().to_string()),
                _ => None,
            };
            let (Some(g), Some(rule), Some(text), Some(d)) = (get("g"), get("rule"), get("text"), get("d")) else { continue };
            let d = d.trim_start_matches('<').trim_end_matches('>').to_string();
            let dname = names.get(&d).cloned().unwrap_or_else(|| d.rsplit('/').next().unwrap_or_default().to_string());
            let rid = rules::rule_id(&dname, &text);
            if rid.starts_with(&id) && domain.as_deref().is_none_or(|w| crud::slugify(w) == crud::slugify(&dname)) {
                let strip = |s: String| s.trim_start_matches('<').trim_end_matches('>').to_string();
                found.push((tier, c.clone(), strip(g), strip(rule), crate::domain::rule_test::short_ref(&dname, &rid)));
            }
        }
    }
    let (tier, c, graph, iri, short) = match found.as_slice() {
        [one] => one.clone(),
        [] => return Err(format!("no retired rule '{spec}' in either tier")),
        many => return Err(format!("'{spec}' fits {} retired rules; give more of the id", many.len())),
    };
    let sparql = format!(
        "{}\nDELETE WHERE {{ GRAPH <{graph}> {{ <{iri}> {p}:{at} ?w }} }} ;\nDELETE DATA {{ GRAPH <{graph}> {{ <{iri}> {p}:status \"{st}\" }} }}",
        crud::prefixes(ns),
        at = crate::supersede::PRED_RETIRED_AT,
        st = crate::supersede::STATUS_RETIRED,
    );
    let (store, trig_path, _lock) = crud::lock_and_load(&c).map_err(|e| format!("{e:#}"))?;
    crate::store::update_and_write(&store, &trig_path, &sparql, crate::store::Scope::Wide, crate::store::Intent::Knowledge)
        .map_err(|e| format!("{e:#}"))?;
    Ok(format!("rule {short} is served again (its retirement cleared, in the graph, {} tier)\n", tier.label()))
}

fn no_wording() -> String {
    "a rewrite needs its new wording: e (edit) in base rule review, or base rule review --edit <id> --text \"...\"".to_string()
}

// ─── Showing ─────────────────────────────────────────────────────────────────

/// One proposal as the review shows it: `[1/3] p-0007 keyword gap · base · add "x" · replay 1.8% · from a correction
/// (2026-10-01, lynx)`, then its target, its evidence and its replay.
pub fn render_one(p: &Stored, at: usize, of: usize, outcome: Result<&Outcome, &String>) -> String {
    let what = match p.kind.as_str() {
        "keyword-gap" => format!("add {}", quoted(&p.keywords)),
        "new-rule" => {
            let words = if p.keywords.is_empty() { String::new() } else { format!(" · words {}", quoted(&p.keywords)) };
            format!("\"{}\"{words}", replay::clip(p.text.as_deref().unwrap_or(""), 60))
        }
        "rewrite" => match p.text.as_deref() {
            Some(t) => format!("new wording \"{}\"", replay::clip(t, 60)),
            None => "no new wording yet".to_string(),
        },
        "drop-keyword" => {
            let add = if p.keywords.is_empty() { String::new() } else { format!(" · add {}", quoted(&p.keywords)) };
            format!("drop {}{add}", quoted(&p.dropped))
        }
        "merge" => format!(
            "with rule {} · \"{}\"",
            p.merge_target.as_deref().unwrap_or("?"),
            replay::clip(p.text.as_deref().unwrap_or(""), 60)
        ),
        "split" => format!(
            "into \"{}\" and \"{}\"",
            replay::clip(p.text.as_deref().unwrap_or(""), 40),
            replay::clip(p.second_text.as_deref().unwrap_or(""), 40)
        ),
        "retire" => "retire it".to_string(),
        _ => String::new(),
    };
    let target = match p.target_kind.as_str() {
        "domain" => p.target_id.clone(),
        k => format!("{k} {}", p.target_id),
    };
    let replay = match outcome {
        Ok(o) => o.summary(),
        Err(_) => "replay failed".to_string(),
    };
    let date = p.created.as_deref().map(|c| c.chars().take(10).collect::<String>()).unwrap_or_default();
    let who = p.title.clone().or_else(|| p.session.as_deref().map(|s| s.chars().take(8).collect())).unwrap_or_default();
    let from = match (p.meaning.as_str(), p.origin.as_str()) {
        ("manual", _) => "given by hand",
        ("pattern", _) => "from the rule pass",
        (_, "tune") => "from a correction the rule pass found",
        _ => "from a correction",
    }
    .to_string();
    let ctx = [date, who].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
    let ctx = if ctx.is_empty() { String::new() } else { format!(" ({ctx})") };
    let mut s = format!("[{at}/{of}] {} {} · {target} · {what} · {replay} · {from}{ctx}\n", p.id, p.kind_label());
    let pad = "      ";
    if let Some(t) = &p.target_text {
        s.push_str(&format!("{pad}now: {}\n", replay::clip(t, 160)));
    }
    if let Some(pr) = &p.prompt {
        s.push_str(&format!("{pad}prompt: \"{}\"\n", replay::clip(pr, 160)));
    }
    if let Some(m) = &p.marker {
        s.push_str(&format!("{pad}marker: {}\n", replay::clip(m, 160)));
    }
    if !p.signals.is_empty() {
        s.push_str(&format!("{pad}signals: {}\n", p.signals.join(" · ")));
    }
    for e in &p.evidence {
        s.push_str(&format!("{pad}evidence: {}\n", replay::clip(e, 160)));
    }
    match outcome {
        Ok(o) => {
            for line in replay::render_body(o).lines() {
                s.push_str(&format!("{pad}{line}\n"));
            }
        }
        Err(e) => s.push_str(&format!("{pad}replay: {e}\n")),
    }
    s
}

/// Every pending proposal with its replay. `terminal` leaves out the footer the one-key loop replaces.
pub fn listing(config: &BaseConfig, cwd: &Path, terminal: bool) -> String {
    let pending: Vec<Stored> = load(config, cwd).into_iter().filter(|p| p.status == "pending").collect();
    if pending.is_empty() {
        return "no rule proposals pending\n".to_string();
    }
    let mut s = String::new();
    let replayer = replay::Replayer::load(config, cwd);
    for (i, p) in pending.iter().enumerate() {
        let outcome = p.change().and_then(|c| replayer.run(&c));
        s.push_str(&render_one(p, i + 1, pending.len(), outcome.as_ref()));
    }
    if !terminal {
        s.push_str(
            "act on one: base rule review --approve <id> [--broad-ok] · --reject <id> [--reason \"...\"] · \
             --edit <id> --text \"...\" --keywords \"a, b\"\n",
        );
    }
    s
}

// ─── The one-key loop (K5c) ──────────────────────────────────────────────────

/// Standard input and output are both a terminal: `base rule review` takes one key per proposal.
pub fn on_terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// One key from the terminal, without Enter and without echo. `None` at end of input.
#[cfg(windows)]
fn read_key() -> Option<char> {
    unsafe extern "C" {
        fn _getch() -> i32;
    }
    // SAFETY: `_getch` takes no arguments and returns one key from the console. It leaves the console mode as it
    // found it, and Ctrl-C stays a signal, so an interrupted review leaves the terminal as it was.
    let c = unsafe { _getch() };
    if c == 0 || c == 0xE0 {
        // A function or arrow key arrives as two codes; the second is read and the key ignored.
        // SAFETY: as above.
        let _ = unsafe { _getch() };
        return Some('\0');
    }
    char::from_u32(c as u32)
}

/// One key from the terminal, without Enter and without echo. `None` at end of input. `stty` puts the terminal in
/// non-canonical mode with no echo and no signal keys around a one-byte read, and puts it back however the read ends:
/// Ctrl-C arrives as a byte (3) and ends the review, so no signal can leave the terminal changed.
#[cfg(not(windows))]
fn read_key() -> Option<char> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let saved = Command::new("stty").arg("-g").stdin(Stdio::inherit()).output().ok().filter(|o| o.status.success());
    let Some(saved) = saved.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()) else {
        return read_line().and_then(|l| l.chars().next());
    };
    struct Restore(String);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = Command::new("stty").arg(&self.0).stdin(Stdio::inherit()).status();
        }
    }
    let _restore = Restore(saved);
    let raw = Command::new("stty").args(["-icanon", "-echo", "-isig", "min", "1", "time", "0"]).stdin(Stdio::inherit()).status();
    if !raw.is_ok_and(|s| s.success()) {
        return read_line().and_then(|l| l.chars().next());
    }
    let mut b = [0u8; 1];
    match std::io::stdin().lock().read(&mut b) {
        Ok(1) => Some(b[0] as char),
        _ => None,
    }
}

/// One line from standard input, without its newline. `None` at end of input.
fn read_line() -> Option<String> {
    let mut s = String::new();
    match std::io::stdin().lock().read_line(&mut s) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(s.trim_end_matches(['\r', '\n']).to_string()),
    }
}

fn ask(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    read_line()
}

/// The key `keys` allows, asked with `prompt` until one comes. `None` to stop: `q`, Ctrl-C, Ctrl-D or end of input.
fn choose(prompt: &str, keys: &[char]) -> Option<char> {
    loop {
        print!("{prompt}");
        let _ = std::io::stdout().flush();
        let k = read_key()?.to_ascii_lowercase();
        if matches!(k, 'q' | '\u{3}' | '\u{4}') {
            println!("q");
            return None;
        }
        if keys.contains(&k) {
            println!("{k}");
            return Some(k);
        }
        println!();
    }
}

/// `base rule review` on a terminal (K5c): each pending proposal, one key: `a` approve, `e` edit, `r` reject, `s` skip,
/// `q` stop. A TOO BROAD change is applied with its flag in front of the user (lynx's G0 verdict on question 8).
pub fn interactive(config: &BaseConfig, cwd: &Path) {
    let pending: Vec<Stored> = load(config, cwd).into_iter().filter(|p| p.status == "pending").collect();
    if pending.is_empty() {
        println!("no rule proposals pending");
        return;
    }
    let of = pending.len();
    let mut left = of;
    // Reloaded after every change applied, so each later replay runs against the config as it now is.
    let mut replayer = replay::Replayer::load(config, cwd);
    for (i, p) in pending.iter().enumerate() {
        let change = p.change();
        let outcome = change.as_ref().map_err(Clone::clone).and_then(|c| replayer.run(c));
        print!("{}", render_one(p, i + 1, of, outcome.as_ref()));
        let Some(k) = choose("      [a]pprove [e]dit [r]eject [s]kip [q]uit > ", &['a', 'e', 'r', 's']) else {
            println!("stopped; {left} left pending");
            return;
        };
        let result = match k {
            'a' => match (&change, &outcome) {
                (Ok(c), Ok(o)) => apply_and_store(config, cwd, p, c, "approved", &[], o),
                (Err(e), _) | (_, Err(e)) => Err(e.clone()),
            },
            'r' => reject(config, p, None),
            'e' => match edit_here(config, cwd, p, &replayer) {
                Ok(Some(text)) => Ok(text),
                Ok(None) => {
                    println!("stopped; {left} left pending");
                    return;
                }
                Err(e) => Err(e),
            },
            _ => Ok("skipped\n".to_string()),
        };
        match result {
            Ok(text) => {
                for line in text.lines() {
                    println!("      {line}");
                }
                if k != 's' && !text.starts_with("skipped") {
                    left -= 1;
                    if k != 'r' {
                        replayer = replay::Replayer::load(config, cwd);
                    }
                }
            }
            Err(e) => println!("      not done: {e}"),
        }
    }
}

/// `e`: the wording and the keywords (Enter keeps each), the edited change's replay, then apply or skip. `Ok(None)`
/// when the user stops the review at that prompt (`q`, Ctrl-C, Ctrl-D).
fn edit_here(config: &BaseConfig, cwd: &Path, p: &Stored, replayer: &replay::Replayer<'_>) -> Result<Option<String>, String> {
    let mut extra: Vec<(&str, String)> = Vec::new();
    let mut text: Option<String> = None;
    if !matches!(p.kind.as_str(), "keyword-gap" | "drop-keyword" | "retire") {
        let now = p.text.as_deref().unwrap_or("");
        let t = ask(&format!("      wording (Enter keeps \"{}\"): ", replay::clip(now, 60))).unwrap_or_default();
        if !t.trim().is_empty() {
            extra.push(("editedText", t.trim().to_string()));
            text = Some(t);
        }
    }
    let mut keywords: Option<Vec<String>> = None;
    if !matches!(p.kind.as_str(), "rewrite" | "merge" | "retire") {
        let k = ask(&format!("      keywords (Enter keeps {}): ", quoted(&p.keywords))).unwrap_or_default();
        if !k.trim().is_empty() {
            let list = crate::domain::global_decisions::parse_keywords(&k);
            extra.extend(list.iter().map(|w| ("editedKeyword", w.clone())));
            keywords = Some(list);
        }
    }
    let change = p.edited(text.as_deref(), keywords.as_deref())?;
    let outcome = replayer.run(&change)?;
    for line in replay::render(&format!("edited: {}", change.describe()), &outcome).lines() {
        println!("      {line}");
    }
    match choose("      [a]pply [s]kip > ", &['a', 's']) {
        Some('a') => apply_and_store(config, cwd, p, &change, "edited", &extra, &outcome).map(Some),
        Some(_) => Ok(Some("skipped\n".to_string())),
        None => Ok(None),
    }
}
