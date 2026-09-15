//! Deferred state (spec Part C): the clock deferral reads, the notice every block ends with, what
//! `base <type> deferred` lists, the keys it hands out, and the revival `base handoff show` and
//! `base fork show` write.
//!
//! Deferred means open, but paused. Not archived, which means done. Not listed at session start. Never
//! lost: every block counts it and names the command that lists it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Local};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;
use serde::{Deserialize, Serialize};

use crate::config::{BaseConfig, DeferKind, NamespaceConfig};
use crate::crud;

/// The status a deferred record carries.
pub const DEFERRED: &str = "deferred";

/// The reason prefix the deferral pass writes. An operator's own deferral never carries it, and only a
/// reason with it lets the clock revive a task or milestone (see `protocol::reconcile::plan_records`).
pub const AUTO: &str = "auto:";

/// The keys the last `base handoff deferred` or `base fork deferred` printed, beside `sessions.json` in
/// the global `.base` (flag 5a). Never a source of truth (flag 5b): a key is re-verified on every use.
pub const KEYS_FILE: &str = "last-deferred-letters.json";

/// The word a kind goes by in commands and sentences, singular and plural.
pub fn nouns(kind: DeferKind) -> (&'static str, &'static str) {
    match kind {
        DeferKind::Handoff => ("handoff", "handoffs"),
        DeferKind::Fork => ("fork", "forks"),
        DeferKind::Project => ("project", "projects"),
        DeferKind::Task => ("task", "tasks"),
        DeferKind::Milestone => ("milestone", "milestones"),
    }
}

/// C8's notice, the last line of a block, or `None` when nothing of the kind is deferred. The plural
/// form is the spec's rendered form verbatim.
pub fn notice(kind: DeferKind, n: usize) -> Option<String> {
    let (one, many) = nouns(kind);
    match n {
        0 => None,
        1 => Some(format!(
            "1 {one} is marked deferred: it is open, but paused. Run base {one} deferred to list all deferred."
        )),
        n => Some(format!(
            "{n} {many} are marked deferred: they are open, but paused. Run base {one} deferred to list all deferred."
        )),
    }
}

fn parse(s: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Local))
}

/// The clock deferral reads (G0.3): `lastActive`, or a `resurfaceAt` that has PASSED when that is newer,
/// because a snooze that ended counts as a touch at its end (K11, flag 10, built to its default). A
/// `resurfaceAt` still in the future is a snooze running, which [`is_snoozed`] answers, and never a
/// clock value. `None` when neither gives a time.
pub fn clock(last_active: Option<&str>, resurface_at: Option<&str>, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let touched = last_active.and_then(parse);
    let ended = resurface_at.and_then(parse).filter(|r| *r <= now);
    match (touched, ended) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// A `resurfaceAt` in the future: the record is snoozed, and a snoozed record is never deferred (C7).
pub fn is_snoozed(resurface_at: Option<&str>, now: DateTime<Local>) -> bool {
    resurface_at.and_then(parse).is_some_and(|r| r > now)
}

/// Whole days from `then` to `now`.
pub fn days_since(then: DateTime<Local>, now: DateTime<Local>) -> i64 {
    now.signed_duration_since(then).num_days()
}

/// The `rdf:type` filter and the kind test that select one kind's records in a SPARQL group.
fn kind_filter(kind: DeferKind, p: &str) -> String {
    match kind {
        DeferKind::Handoff => format!(
            "?e a {p}:Handoff .\n             OPTIONAL {{ ?e {p}:kind ?kind }}\n             FILTER(!BOUND(?kind) || ?kind != \"fork\")"
        ),
        DeferKind::Fork => format!("?e a {p}:Handoff ; {p}:kind \"fork\" ."),
        DeferKind::Task => format!("?e a {p}:Task ."),
        DeferKind::Milestone => format!("?e a {p}:Milestone ."),
        DeferKind::Project => format!(
            "?e a ?type .\n             FILTER(?type IN ({p}:Project, {p}:App, {p}:Framework, {p}:TrackingProject))"
        ),
    }
}

/// How many distinct records of `kind` in `store` are deferred. Session start counts its handoff and
/// fork notices with this over the store those blocks already read.
pub fn count_in(store: &Store, ns: &NamespaceConfig, kind: DeferKind) -> Result<usize> {
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT (COUNT(DISTINCT ?e) AS ?n) WHERE {{\n\
           GRAPH ?g {{\n\
             {filter}\n\
             ?e {p}:status \"{DEFERRED}\" .\n\
           }}\n\
         }}",
        pfx = crud::prefixes(ns),
        filter = kind_filter(kind, p)
    );
    let QueryResults::Solutions(mut rows) = crate::store::query(store, &sparql)? else {
        return Ok(0);
    };
    let n = rows
        .next()
        .and_then(|r| r.ok())
        .and_then(|r| r.get("n").map(|t| crud::term_display(t.into())))
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    Ok(n)
}

/// One deferred record, read from one tier file so its tier is known rather than guessed.
#[derive(Debug, Clone)]
pub struct Row {
    pub slug: String,
    pub name: String,
    pub project: String,
    pub tier: &'static str,
    pub last_active: Option<String>,
    pub resurface_at: Option<String>,
    pub reason: Option<String>,
    pub deferred_at: Option<String>,
}

fn rows_in(file: &Path, ns: &NamespaceConfig, kind: DeferKind, tier: &'static str) -> Result<Vec<Row>> {
    let store = crate::store::load_or_empty(file)?;
    let p = &ns.prefix;
    let owner = match kind {
        DeferKind::Task => format!("OPTIONAL {{ GRAPH ?og {{ ?owner {p}:hasTask ?e }} }}"),
        DeferKind::Milestone => format!("OPTIONAL {{ GRAPH ?og {{ ?owner {p}:hasMilestone ?e }} }}"),
        _ => String::new(),
    };
    let sparql = format!(
        "{pfx}\nSELECT ?e ?name ?project ?owner ?lastActive ?resurfaceAt ?why ?at WHERE {{\n\
           GRAPH ?g {{\n\
             {filter}\n\
             ?e {p}:status \"{DEFERRED}\" .\n\
             OPTIONAL {{ ?e {p}:name ?name }}\n\
             OPTIONAL {{ ?e {p}:project ?project }}\n\
             OPTIONAL {{ ?e {p}:lastActive ?lastActive }}\n\
             OPTIONAL {{ ?e {p}:resurfaceAt ?resurfaceAt }}\n\
             OPTIONAL {{ ?e {p}:deferredReason ?why }}\n\
             OPTIONAL {{ ?e {p}:deferredAt ?at }}\n\
           }}\n\
           {owner}\n\
         }}",
        pfx = crud::prefixes(ns),
        filter = kind_filter(kind, p)
    );
    let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<Row> = Vec::new();
    for row in solutions.filter_map(|r| r.ok()) {
        let get = |k: &str| {
            row.get(k)
                .map(|t| crud::term_display(t.into()))
                .filter(|s| !s.is_empty())
        };
        let iri = get("e").unwrap_or_default();
        let slug = iri.rsplit('/').next().unwrap_or(&iri).to_string();
        // A subject holding two values for one field comes back as two rows; the first stands.
        if out.iter().any(|r| r.slug == slug) {
            continue;
        }
        let project = get("project")
            .or_else(|| get("owner").map(|o| o.rsplit('/').next().unwrap_or(&o).to_string()))
            .unwrap_or_default();
        out.push(Row {
            name: get("name").unwrap_or_else(|| slug.clone()),
            slug,
            project,
            tier,
            last_active: get("lastActive"),
            resurface_at: get("resurfaceAt"),
            reason: get("why"),
            deferred_at: get("at"),
        });
    }
    Ok(out)
}

/// Every deferred record of `kind`, in every tier, most recently touched first.
pub fn rows(gbl_root: Option<&Path>, cwd: &Path, ns: &NamespaceConfig, kind: DeferKind) -> Result<Vec<Row>> {
    let now = Local::now();
    let mut all = Vec::new();
    for file in crud::all_tier_files(gbl_root, cwd) {
        let tier = crud::tier_label_of_file(&file, gbl_root);
        all.extend(rows_in(&file, ns, kind, tier)?);
    }
    let key = |r: &Row| clock(r.last_active.as_deref(), r.resurface_at.as_deref(), now);
    all.sort_by(|a, b| key(b).cmp(&key(a)).then_with(|| a.slug.cmp(&b.slug)));
    Ok(all)
}

/// The command that brings one record of `kind` back. Handoffs and forks come back through `show`;
/// tasks, milestones and projects by setting their status.
pub fn revive_command(kind: DeferKind, slug: &str) -> String {
    match kind {
        DeferKind::Handoff => format!("base handoff show {slug}"),
        DeferKind::Fork => format!("base fork show {slug}"),
        DeferKind::Task => format!("base task update {slug} --status active"),
        DeferKind::Milestone => format!("base milestone update {slug} --status active"),
        DeferKind::Project => format!("base project update {slug} --status active"),
    }
}

/// `base <type> deferred`: one line per record, carrying (C9) the key where `show` takes one, the slug,
/// the project, the codename where the slug has one, the days untouched, the date deferred, the reason,
/// the tier, and the command that brings it back. Handoff and fork listings keep their keys.
pub fn render(kind: DeferKind, rows: &[Row], now: DateTime<Local>) -> String {
    let (one, many) = nouns(kind);
    if rows.is_empty() {
        return format!("No deferred {many}.\n");
    }
    let keyed = matches!(kind, DeferKind::Handoff | DeferKind::Fork);
    let bring = match kind {
        DeferKind::Handoff | DeferKind::Fork => format!("base {one} show <key or slug>"),
        _ => format!("base {one} update <slug> --status active"),
    };
    let mut s = format!("DEFERRED {} ({}) · bring one back: {bring}\n", many.to_uppercase(), rows.len());
    for (i, r) in rows.iter().enumerate() {
        let mut parts: Vec<String> = Vec::new();
        parts.push(if keyed {
            format!("D{} {}", i + 1, r.slug)
        } else {
            r.slug.clone()
        });
        if !keyed && r.name != r.slug {
            parts.push(r.name.clone());
        }
        if !r.project.is_empty() {
            parts.push(format!("project {}", r.project));
        }
        if let Some(code) = crud::handoff_show::codename_of(&r.slug) {
            parts.push(code.to_string());
        }
        parts.push(match clock(r.last_active.as_deref(), r.resurface_at.as_deref(), now) {
            Some(t) => format!("untouched {}d", days_since(t, now)),
            None => "never touched".to_string(),
        });
        parts.push(match r.deferred_at.as_deref().and_then(parse) {
            Some(at) => format!("deferred {}", at.format("%Y-%m-%d")),
            None => "date deferred not recorded".to_string(),
        });
        parts.push(r.reason.clone().unwrap_or_else(|| "no reason recorded".to_string()));
        parts.push(r.tier.to_string());
        parts.push(revive_command(kind, &r.slug));
        let _ = writeln!(s, "  {}", parts.join(" · "));
    }
    s
}

#[derive(Serialize, Deserialize)]
struct KeysFile {
    written_at: String,
    kind: String,
    keys: BTreeMap<String, String>,
}

/// Keep the keys a handoff or fork listing printed. Written only where the global `.base` already exists,
/// through a temp file and a rename; a failure is named on stderr and never fails the listing.
fn write_keys(kind: DeferKind, rows: &[Row]) {
    let Some(dir) = crate::config::global_base_dir().filter(|d| d.is_dir()) else {
        return;
    };
    let file = KeysFile {
        written_at: crud::now_iso(),
        kind: nouns(kind).0.to_string(),
        keys: rows
            .iter()
            .enumerate()
            .map(|(i, r)| (format!("D{}", i + 1), r.slug.clone()))
            .collect(),
    };
    let text = serde_json::to_string_pretty(&file).unwrap_or_default();
    let kept = crate::emit::write_full_output(&dir.join(KEYS_FILE), &text);
    if let Some(why) = kept.failure() {
        eprintln!("base: could not keep the deferred keys: {why}");
    }
}

/// A query of exactly `D` and one to three digits, in either case.
pub fn as_key(query: &str) -> Option<String> {
    let rest = query.strip_prefix('D').or_else(|| query.strip_prefix('d'))?;
    (!rest.is_empty() && rest.len() <= 3 && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| format!("D{rest}"))
}

/// What a key names, read from the last listing. `Err` carries the sentence that says why it cannot be
/// used; every refusal tells the operator to re-list (flag 5b).
pub fn key_slug(kind: DeferKind, key: &str) -> std::result::Result<(String, String), String> {
    let (one, _) = nouns(kind);
    let relist = format!("re-list: base {one} deferred");
    let Some(dir) = crate::config::global_base_dir() else {
        return Err(format!("no global .base holds the keys file; {relist}"));
    };
    let path = dir.join(KEYS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Err(format!("no base {one} deferred listing has been kept; {relist}")),
    };
    let Ok(file) = serde_json::from_str::<KeysFile>(&text) else {
        return Err(format!("the keys file {} is unreadable; {relist}", path.display()));
    };
    if file.kind != one {
        return Err(format!(
            "the last deferred listing was of {}s, not {one}s; {relist}",
            file.kind
        ));
    }
    match file.keys.get(key) {
        Some(slug) => Ok((slug.clone(), file.written_at)),
        None => Err(format!("key {key} is not in the last listing; {relist}")),
    }
}

/// `base <type> deferred`.
pub fn list(gbl_root: Option<&Path>, cwd: &Path, config: &BaseConfig, kind: DeferKind) -> Result<()> {
    let found = rows(gbl_root, cwd, &config.namespace, kind)?;
    if matches!(kind, DeferKind::Handoff | DeferKind::Fork) {
        write_keys(kind, &found);
    }
    print!("{}", render(kind, &found, Local::now()));
    Ok(())
}

/// Bring a deferred handoff or fork back to open, in every tier holding it: `status "open"`, `lastActive`
/// now, `deferredReason` and `deferredAt` deleted. Only a record that is deferred matches the update, so
/// this never touches an open one (verdicts, AMENDMENTS C). Returns the tier labels written.
pub fn revive_handoff(gbl_root: Option<&Path>, cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<Vec<String>> {
    let iri = crud::build_iri(ns, "handoff", slug);
    let now = crud::now_iso();
    let p = &ns.prefix;
    let sparql = format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:status \"{DEFERRED}\" .\n\
                               <{iri}> {p}:lastActive ?la .\n\
                               <{iri}> {p}:deferredReason ?why .\n\
                               <{iri}> {p}:deferredAt ?at }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:status \"open\" .\n\
                               <{iri}> {p}:lastActive \"{now}\"^^xsd:dateTime }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Handoff ; {p}:status \"{DEFERRED}\" }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:lastActive ?la }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:deferredReason ?why }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:deferredAt ?at }} }} }}"
    );
    crud::handoff::apply_to_tiers(gbl_root, cwd, ns, slug, &sparql)
}
