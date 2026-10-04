//! The candidate's runs inside the hooks (K9b, K9c, K9d).
//!
//! THE ONE FUNCTION. The prompt hook decides what it serves in `user_prompt_submit::serve`; the candidate runs that same
//! function on the same `World` (what live read: domains, store, global decisions, the index, the touched paths) with
//! its own `View` (a proposals candidate's changes, made to copies) and `Matching` (its `[match]` settings, and the
//! index and prompt counts it scores with). Its blocks take the place of live's matching blocks in live's own list, the
//! walk, the bracket rules, the relay and the correction blocks staying live's, and that list is fitted to the same
//! budget under the same header. What it would have printed, against what live printed, is the row's `shadow` entry.
//!
//! LIVE IS NEVER TOUCHED. It runs after live's output is printed and flushed, on a copy of the session state live
//! decided with (taken before anything was recorded in it), and nothing it decides is saved.
//!
//! THE TIME LIMIT (K9d). A clock starts when the candidate starts. It is checked after the candidate's index and
//! changes are loaded, before each domain whose rules or CONTEXT no live run read, after the serve, and after the fit.
//! Past `[shadow] max_ms` the run stops and the row says `skipped: slow`; a step is never cut in the middle, so a run
//! overshoots by at most one step.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::config::BaseConfig;
use crate::crud;
use crate::domain::global_decisions::GlobalDecisions;
use crate::domain::replay::{edit_list, edit_topic_words, union_matchers, Change, Target};
use crate::domain::rules::{self, Converted, ServedRule};
use crate::domain::score_index::{self, PromptWeights, ScoreIndex};
use crate::domain::DomainDef;
use crate::emit::match_log::{Shadow, Trace};
use crate::emit::prompt::{Fitted, PromptBlocks};
use crate::hook::user_prompt_submit::{self as ups, Kept, Matching, View, World};

use super::Active;

/// `[shadow] max_ms` from now.
fn deadline(config: &BaseConfig, started: Instant) -> Instant {
    started + Duration::from_millis(config.shadow.max_ms)
}

/// The rule and decision ids a fitted output printed.
pub fn printed_ids(f: &Fitted) -> BTreeSet<String> {
    f.logged()
        .into_iter()
        .filter(|(_, i, printed)| *printed && (i.kind == "rule" || i.kind == "decision"))
        .map(|(_, i, _)| i.id.clone())
        .collect()
}

/// The row's entry for a pick of `candidate` against live's, both as printed ids.
pub fn entry(name: &str, live: &BTreeSet<String>, candidate: &BTreeSet<String>, ms: u64) -> Shadow {
    Shadow {
        candidate: name.to_string(),
        adds: candidate.difference(live).cloned().collect(),
        drops: live.difference(candidate).cloned().collect(),
        skipped: None,
        ms,
    }
}

fn slow(name: &str, ms: u64) -> Shadow {
    Shadow { candidate: name.to_string(), skipped: Some("slow".to_string()), ms, ..Shadow::default() }
}

/// The folder live's index is read from for `cwd` (`score_index::index_dir`).
fn index_base(cwd: &Path) -> Option<PathBuf> {
    score_index::index_dir(cwd)
}

/// Where a proposals candidate keeps its own index, beside live's: `<tier>/.base/shadow/<name>/`.
pub fn own_index_dir(cwd: &Path, name: &str) -> Option<PathBuf> {
    index_base(cwd).map(|d| d.join(super::DIR).join(name))
}

/// The candidate's run on one prompt (K9b): its pick against `live`'s, or `skipped: slow`.
///
/// `blocks` is live's list before its fit (cloned only while a shadow runs), `front` how many blocks the hook put
/// first after the matching ran (the correction check and the rule-pass line), `header` live's bracket line.
#[allow(clippy::too_many_arguments)] // the hook's own state, handed over from the dispatcher
pub fn prompt(
    config: &BaseConfig,
    cwd: &Path,
    active: &Active,
    kept: &Kept,
    blocks: PromptBlocks,
    front: usize,
    header: &str,
    live: &Fitted,
) -> Shadow {
    let started = Instant::now();
    let until = deadline(config, started);
    let name = &active.version.name;
    let pick = (|| -> Result<BTreeSet<String>, ups::Slow> {
        let v = &active.version;
        let w = &kept.world;
        let own_index;
        let index: Option<&ScoreIndex> = if !v.settings.bm25 {
            None
        } else if v.is_proposals() {
            own_index = own_index_dir(cwd, name).and_then(|d| ScoreIndex::load(&d));
            own_index.as_ref()
        } else if w.index.is_some() {
            w.index.as_ref()
        } else {
            own_index = index_base(cwd).and_then(|d| ScoreIndex::load(&d));
            own_index.as_ref()
        };
        let own_weights;
        let weights: Option<&PromptWeights> = if v.settings.bm25 && v.settings.prompt_idf && v.settings.min_score.is_some() {
            if w.weights.is_some() {
                w.weights.as_ref()
            } else {
                own_weights = index_base(cwd).and_then(|d| PromptWeights::load(&d));
                own_weights.as_ref()
            }
        } else {
            None
        };
        let changes = v.changes();
        let view = if changes.is_empty() { View::live(w) } else { proposals_view(w, config, &changes) };
        ups::in_time(Some(until))?;
        let matching = Matching { settings: &v.settings, index, weights };
        let mut scratch = Trace::default();
        let served = ups::serve(config, cwd, w, &view, &matching, &kept.session, &mut scratch, Some(until))?;
        ups::in_time(Some(until))?;
        let mut list = blocks;
        // Grounding rides on any fresh injection (Phase 30): the walk is live's, the rest the candidate's.
        let walked = list.iter().any(|b| !b.matcher && b.id.starts_with("walk-"));
        let wants_grounding = config.grounding.enabled && (served.injected_any || walked);
        list.replace_matcher(served.blocks, front);
        match (list.has("grounding"), wants_grounding) {
            (true, false) => list.remove("grounding"),
            (false, true) => list.insert_after(ups::grounding_prompt_block(), |b| b.matcher || b.id.starts_with("walk-")),
            _ => {}
        }
        let fitted = crate::emit::prompt::fit(header, list, config.budget.prompt_bytes, config.budget.key_as_written("prompt_bytes"));
        ups::in_time(Some(until))?;
        Ok(printed_ids(&fitted))
    })();
    let ms = started.elapsed().as_millis() as u64;
    match pick {
        Ok(cand) => entry(name, &printed_ids(live), &cand, ms),
        Err(ups::Slow) => slow(name, ms),
    }
}

/// The candidate's run on one tool call (K9b): the same rule serving live ran (`pre_tool_use::serve_file`), on the
/// session as it was before live's, with its changes made. A `[match]` setting never reaches this path, so a matcher
/// candidate picks what live picks here; a proposals candidate can differ.
pub fn file(config: &BaseConfig, kept: &crate::hook::pre_tool_use::FileKept) -> Shadow {
    use crate::hook::pre_tool_use::{serve_file, FileView};
    let started = Instant::now();
    let until = deadline(config, started);
    let name = &kept.active.version.name;
    let pick = (|| -> Result<BTreeSet<String>, ups::Slow> {
        let w = &kept.world;
        let changes = kept.active.version.changes();
        let view = if changes.is_empty() {
            FileView::live(w)
        } else {
            let rules_of = |d: &DomainDef| w.rules_of(config, d);
            let c = apply_changes(&w.domains, &w.converted, &GlobalDecisions::default(), &rules_of, &changes);
            FileView { domains: Cow::Owned(c.domains), converted: Cow::Owned(c.converted), rule_edits: c.edits }
        };
        ups::in_time(Some(until))?;
        let mut session = kept.session.clone();
        let served = serve_file(config, w, &view, &mut session, Some(until))?;
        Ok(served.printed())
    })();
    let ms = started.elapsed().as_millis() as u64;
    match pick {
        Ok(cand) => entry(name, &kept.live, &cand, ms),
        Err(ups::Slow) => slow(name, ms),
    }
}

// ─── A proposals candidate, in memory ──────────────────────────────────────

/// What a proposals candidate matches with: copies of live's domains, rules with matchers and global decisions with the
/// changes made, and each domain's rules taken away and added.
pub struct Changed {
    pub domains: Vec<DomainDef>,
    pub converted: Vec<Converted>,
    pub global: GlobalDecisions,
    pub edits: HashMap<String, (HashSet<String>, Vec<ServedRule>)>,
}

/// `changes` made to copies, the way `replay::plan` makes one to a bench (BO-16) and the way approving would write it:
/// a domain's keywords; a rule's own words (a rule with none gets a topic matcher of them and leaves its domain's block,
/// as `rules_with_matchers` would have it); a rewrite, merge or split; a retirement; a global decision's keywords or
/// wording; a new rule, on its words or, with none, through its domain. `rules_of` reads a domain's rules.
pub fn apply_changes(
    domains: &[DomainDef],
    converted: &[Converted],
    global: &GlobalDecisions,
    rules_of: &dyn Fn(&DomainDef) -> Rc<Vec<ServedRule>>,
    changes: &[Change],
) -> Changed {
    let mut out = Changed {
        domains: domains.to_vec(),
        converted: converted.to_vec(),
        global: global.clone(),
        edits: HashMap::new(),
    };
    for c in changes {
        match &c.target {
            Target::Domain(name) => {
                let want = crud::slugify(name);
                if !out.domains.iter().any(|d| crud::slugify(&d.name) == want)
                    && let Ok(fresh) = serde_json::from_value::<DomainDef>(serde_json::json!({ "name": name }))
                {
                    out.domains.push(fresh);
                }
                if let Some(d) = out.domains.iter_mut().find(|d| crud::slugify(&d.name) == want) {
                    d.prompt_keywords = edit_list(&d.prompt_keywords, &c.add, &c.drop);
                }
            }
            Target::Decision(slug) => out.global.edit(slug, &c.add, &c.drop, c.text.as_deref()),
            Target::NewRule { domain, text } => {
                let Some(d) = out.domains.iter().find(|d| crud::slugify(&d.name) == crud::slugify(domain)) else { continue };
                let rule = rules::build(&d.name, text.clone(), None, None);
                if c.add.is_empty() {
                    out.edits.entry(d.name.clone()).or_default().1.push(rule);
                } else if d.auto_inject {
                    out.converted.push(Converted { rule, matchers: edit_topic_words(&[], &c.add, &[]) });
                }
            }
            Target::Rule(spec) => {
                let Some(rule) = find_rule(&out, rules_of, spec) else { continue };
                change_rule(&mut out, rules_of, &rule, c);
            }
        }
    }
    out
}

/// The rule `spec` (`<domain>.<id>`, the id or its start) names: among the rules with matchers first, then its domain's.
fn find_rule(out: &Changed, rules_of: &dyn Fn(&DomainDef) -> Rc<Vec<ServedRule>>, spec: &str) -> Option<ServedRule> {
    let (domain, id) = crud::rule::parse_rule_ref(spec).ok()?;
    let in_domain = |name: &str| domain.as_deref().is_none_or(|d| crud::slugify(d) == crud::slugify(name));
    if let Some(c) = out.converted.iter().find(|c| c.rule.id.starts_with(&id) && in_domain(&c.rule.domain)) {
        return Some(c.rule.clone());
    }
    out.domains
        .iter()
        .filter(|d| in_domain(&d.name))
        .flat_map(|d| rules_of(d).iter().cloned().collect::<Vec<_>>())
        .find(|r| r.id.starts_with(&id))
}

fn change_rule(out: &mut Changed, rules_of: &dyn Fn(&DomainDef) -> Rc<Vec<ServedRule>>, rule: &ServedRule, c: &Change) {
    let domain = rule.domain.clone();
    let take = |out: &mut Changed, r: &ServedRule| {
        out.converted.retain(|x| x.rule.id != r.id);
        out.edits.entry(r.domain.clone()).or_default().0.insert(r.id.clone());
    };
    if c.retire {
        take(out, rule);
        return;
    }
    let at = out.converted.iter().position(|x| x.rule.id == rule.id);
    if !c.add.is_empty() || !c.drop.is_empty() {
        match at {
            Some(i) => {
                out.converted[i].matchers = edit_topic_words(&out.converted[i].matchers, &c.add, &c.drop);
                if out.converted[i].matchers.is_empty() {
                    out.converted.remove(i);
                }
            }
            None if !c.add.is_empty() => {
                out.converted.push(Converted { rule: rule.clone(), matchers: edit_topic_words(&[], &c.add, &[]) });
            }
            None => {}
        }
    }
    // A merge: the second rule goes, and its words join the first's.
    let mut merged_words: Vec<rules::Matcher> = Vec::new();
    if let Some(other) = c.merge.as_deref()
        && let Some(o) = find_rule(out, rules_of, other)
    {
        if let Some(j) = out.converted.iter().position(|x| x.rule.id == o.id) {
            merged_words = out.converted.remove(j).matchers;
        }
        take(out, &o);
    }
    if let Some(text) = c.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let new = rules::build(&domain, text.to_string(), rule.rationale.clone(), None);
        match out.converted.iter_mut().find(|x| x.rule.id == rule.id) {
            Some(x) => {
                x.rule = new;
                x.matchers = union_matchers(&x.matchers, &merged_words);
            }
            None if !merged_words.is_empty() => {
                out.edits.entry(domain.clone()).or_default().0.insert(rule.id.clone());
                out.converted.push(Converted { rule: new, matchers: merged_words });
            }
            None => {
                let e = out.edits.entry(domain.clone()).or_default();
                e.0.insert(rule.id.clone());
                e.1.push(new);
            }
        }
    }
    if let Some((text, words)) = &c.second {
        let part = rules::build(&domain, text.clone(), None, None);
        if words.is_empty() {
            out.edits.entry(domain.clone()).or_default().1.push(part);
        } else {
            out.converted.push(Converted { rule: part, matchers: edit_topic_words(&[], words, &[]) });
        }
    }
}

/// The view a proposals candidate serves with, on the world live read.
pub fn proposals_view<'w>(w: &'w World, config: &BaseConfig, changes: &[Change]) -> View<'w> {
    let rules_of = |d: &DomainDef| w.rules_of(config, d);
    let c = apply_changes(&w.domains, &w.converted, &w.global, &rules_of, changes);
    View {
        domains: Cow::Owned(c.domains),
        converted: Cow::Owned(c.converted),
        global: Cow::Owned(c.global),
        rule_edits: c.edits,
    }
}

// ─── The indexes a shadow needs ────────────────────────────────────────────

/// Refresh what live and the running candidate score with, from `cwd`: live's index whenever either scores with BM25
/// (a BM25 candidate beside a keyword-only live still needs a current one), the prompt-IDF counts while either admits
/// with them, and a proposals candidate's own index, counted with its changes made. Called wherever live's index is
/// counted (session start, the commands that change matching) and at `base shadow start`. Best effort.
pub fn refresh_indexes(config: &BaseConfig, cwd: &Path) {
    let active = super::active();
    let cand = active.as_ref().map(|a| &a.version.settings);
    // Live's own index is counted by its own refresh when live scores; here only what a shadow adds. Nothing is loaded
    // when nothing is wanted: no shadow and no prompt IDF is the common case, and it must cost nothing.
    let live_index = !config.matching.bm25 && cand.is_some_and(|c| c.bm25);
    let idf = (config.matching.bm25 && config.matching.prompt_idf) || cand.is_some_and(|c| c.bm25 && c.prompt_idf);
    let own = active.as_ref().filter(|a| a.version.is_proposals() && a.version.settings.bm25);
    if !live_index && !idf && own.is_none() {
        return;
    }
    let Some(dir) = index_base(cwd) else { return };
    score_index::refresh_prompt_weights(config, cwd, &dir, idf);
    if !live_index && own.is_none() {
        return;
    }
    let domains = crate::domain::load_domains(cwd);
    let store = crate::store::load_merged(cwd);
    if live_index && let Err(why) = score_index::refresh(store.as_ref(), config, &domains, &dir) {
        eprintln!("base: could not build the rule index in {}: {why}", dir.display());
    }
    if let Some(active) = own
        && let Some(own) = own_index_dir(cwd, &active.version.name)
    {
        let sources = candidate_sources(config, store.as_ref(), &domains, &active.version.changes());
        let index = ScoreIndex::build(&sources);
        if ScoreIndex::load(&own).is_some_and(|old| old.inputs == index.inputs) {
            return;
        }
        if let Err(why) = std::fs::create_dir_all(&own).and_then(|_| index.save(&own)) {
            eprintln!("base: could not build the candidate's rule index in {}: {why}", own.display());
        }
    }
}

/// A proposals candidate's scoring texts: live's ([`score_index::sources_from`]) with its changes made.
pub fn candidate_sources(
    config: &BaseConfig,
    store: Option<&oxigraph::store::Store>,
    domains: &[DomainDef],
    changes: &[Change],
) -> Vec<score_index::Source> {
    let converted = rules::rules_with_matchers(store, config, domains);
    let global = store.map(|s| GlobalDecisions::load(s, config, domains)).unwrap_or_default();
    let cache: std::cell::RefCell<HashMap<String, Rc<Vec<ServedRule>>>> = Default::default();
    let rules_of = |d: &DomainDef| {
        if let Some(r) = cache.borrow().get(&d.name) {
            return Rc::clone(r);
        }
        let r = Rc::new(rules::rules_for_domain(store, config, d));
        cache.borrow_mut().insert(d.name.clone(), Rc::clone(&r));
        r
    };
    let c = apply_changes(domains, &converted, &global, &rules_of, changes);
    let lists: Vec<Vec<ServedRule>> = c
        .domains
        .iter()
        .map(|d| {
            let base = rules_of(d);
            match c.edits.get(&d.name) {
                None => base.as_ref().clone(),
                Some((gone, added)) => base.iter().filter(|r| !gone.contains(&r.id)).cloned().chain(added.iter().cloned()).collect(),
            }
        })
        .collect();
    let tests = rules::rule_tests(store, config, domains);
    let mut sources = score_index::sources_from(store, config, &c.domains, &lists, &c.converted, &tests);
    // A decision's keywords as the changes leave them.
    for s in sources.iter_mut().filter(|s| s.doc.kind == score_index::DocKind::Decision) {
        if let Some(d) = c.global.get(&s.doc.id) {
            s.keywords = d.keywords.clone();
            s.wording = vec![d.name.clone()];
        }
    }
    sources
}
