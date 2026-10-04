use std::path::Path;

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::NamespaceConfig;
use crate::crud;

/// The status a handled or timed-out reminder carries. No reminder carries any status before
/// this rank, so "no status" keeps meaning live and nothing has to be migrated.
pub const ARCHIVED: &str = "archived";

/// Whole days past due at which a reminder archives itself, and the day the warning starts.
/// Stated once here because three call sites read them: the reconcile pass, the DUE NOW line,
/// and the tests.
pub const AUTO_ARCHIVE_DAYS: i64 = 10;
pub const WARN_FROM_DAYS: i64 = 8;

/// Whole days a reminder's warning must have been shown before it archives itself (BO-27, V4): R3's span from the
/// first warning day to the archive day. Measured from the session start that first printed the warning, recorded as
/// `warnedAt` on the reminder, so a user who opens no session in that span, or who upgrades from a build with no
/// warning at all, still sees it for this long.
pub const WARN_GRACE_DAYS: i64 = AUTO_ARCHIVE_DAYS - WARN_FROM_DAYS;

/// What [`set`] did with a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOutcome {
    /// No tier held the slug: a new reminder.
    Created,
    /// A live reminder held it: its one surface time moved.
    Moved,
    /// An archived reminder held it: live again, at the new time.
    Revived,
}

/// `base reminder add`: a new reminder, or, when a tier already holds the slug, that reminder's one clock moved to
/// `surface_at` and its archive cleared (BO-27, V5). `add` alone is an `INSERT DATA`, so before this a re-added name
/// kept its archive and never surfaced, and a re-added live one gained a second surface time. A reminder is its slug in
/// every command (`snooze`, `archive`, `remove`), so a name that slugs to one a tier holds is that reminder; the name
/// returned is the one it is stored under, which is what the command prints.
pub fn set(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    name: &str,
    surface_at: &str,
    due_date: Option<&str>,
) -> Result<(String, SetOutcome, String)> {
    let slug = crud::slugify(name);
    let held = find(gbl_root, cwd, ns, &slug)?;
    let Some(first) = held.first() else {
        return Ok((add(cwd, ns, name, surface_at, due_date)?, SetOutcome::Created, name.to_string()));
    };
    let stored = first.name.clone();
    let outcome = if held.iter().any(|r| r.archived) { SetOutcome::Revived } else { SetOutcome::Moved };
    let date = match due_date {
        Some(d) => d.to_string(),
        None => surface_time(surface_at)
            .map(|t| t.format("%Y-%m-%d").to_string())
            .with_context(|| format!("not a time: {surface_at}"))?,
    };
    snooze(gbl_root, cwd, ns, &slug, surface_at, &date)?;
    Ok((slug, outcome, stored))
}

/// Insert a reminder as given. [`set`] is the command's path: it checks first whether a tier holds the slug.
pub fn add(
    cwd: &Path,
    ns: &NamespaceConfig,
    name: &str,
    surface_at: &str,
    due_date: Option<&str>,
) -> Result<String> {
    let slug = crud::slugify(name);
    let iri = crud::build_iri(ns, "reminder", &slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;
    let name = crud::escape_sparql_literal(name);

    // Optional dueDate triple — display/back-compat only; surfacing is driven by resurfaceAt.
    let due_triple = match due_date {
        Some(d) => format!(
            "               {p}:dueDate \"{}\"^^xsd:date ;\n",
            crud::escape_sparql_literal(d)
        ),
        None => String::new(),
    };

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> rdf:type {p}:Reminder ;\n\
               {p}:name \"{name}\" ;\n\
               {p}:resurfaceAt \"{surface_at}\"^^xsd:dateTime ;\n\
{due_triple}               {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
               {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
           }}\n\
         }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)?;
    Ok(slug)
}

/// Whole days between `when` and now, positive when `when` is in the past. The same
/// calendar-day arithmetic the handoff age uses, so "9 days overdue" means the same thing
/// everywhere in the product.
pub fn days_past(when: &str) -> Option<i64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(when).ok()?;
    Some(
        (chrono::Local::now().date_naive() - parsed.with_timezone(&chrono::Local).date_naive())
            .num_days(),
    )
}

/// The one rule for a due reminder (BO-06, F10c): live, and its `resurfaceAt` has passed. DUE NOW lists exactly these
/// and the header and the pulse count exactly these. Until BO-06 the pulse counted "overdue" its own way, a `dueDate`
/// before today in the workspace tier, archived reminders included, so on 2026-10-01 it said 4 beside a DUE NOW of 5.
/// An `xsd:dateTime` with no timezone (`2026-09-30T09:00:00`; base writes an offset, another writer may not) is read as
/// local time. A time that does not parse at all is never due.
pub fn is_due(resurface_at: &str, archived: bool, now: chrono::DateTime<chrono::Local>) -> bool {
    !archived && surface_time(resurface_at).is_some_and(|when| when <= now)
}

/// A stored `resurfaceAt`: RFC 3339, or a timezone-less `xsd:dateTime` read as local time.
fn surface_time(value: &str) -> Option<chrono::DateTime<chrono::Local>> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(t.with_timezone(&chrono::Local));
    }
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
}

/// The date a reminder due at `when` archives itself, for the warning line: the later of [`AUTO_ARCHIVE_DAYS`] after
/// it was due and [`WARN_GRACE_DAYS`] after its warning was first shown (`warned`; today when it is being shown for the
/// first time). Before BO-27 it was due + 10 alone, which for a reminder already past day 10 is a date gone by.
pub fn archives_on(when: &str, warned: Option<&str>) -> Option<String> {
    let due = local_date(when)? + chrono::Duration::days(AUTO_ARCHIVE_DAYS);
    let shown = warned.and_then(local_date).unwrap_or_else(|| chrono::Local::now().date_naive())
        + chrono::Duration::days(WARN_GRACE_DAYS);
    Some(due.max(shown).format("%Y-%m-%d").to_string())
}

/// The local calendar date of a stored time, read as [`surface_time`] reads one: RFC 3339, or a timezone-less
/// `xsd:dateTime` as local time. A `warnedAt` another writer stored without an offset still counts.
fn local_date(when: &str) -> Option<chrono::NaiveDate> {
    surface_time(when).map(|t| t.date_naive())
}

/// Whole calendar days since a stored time, by [`local_date`].
fn days_since(when: &str) -> Option<i64> {
    local_date(when).map(|d| (chrono::Local::now().date_naive() - d).num_days())
}

/// Does this store hold reminder `slug`? Asked per tier file, so a command never reports a
/// tier it did not actually change.
fn store_holds(store: &Store, ns: &NamespaceConfig, slug: &str) -> bool {
    let iri = crud::build_iri(ns, "reminder", slug);
    let p = &ns.prefix;
    let ask = format!(
        "{}\nASK {{ GRAPH ?g {{ <{iri}> a {p}:Reminder }} }}",
        crud::prefixes(ns)
    );
    matches!(store.query(&ask), Ok(QueryResults::Boolean(true)))
}

/// Does this store hold reminder `slug` archived?
fn store_archived(store: &Store, ns: &NamespaceConfig, slug: &str) -> bool {
    let iri = crud::build_iri(ns, "reminder", slug);
    let p = &ns.prefix;
    let ask = format!(
        "{}
ASK {{ GRAPH ?g {{ <{iri}> a {p}:Reminder ; {p}:status \"{ARCHIVED}\" }} }}",
        crud::prefixes(ns)
    );
    matches!(store.query(&ask), Ok(QueryResults::Boolean(true)))
}

/// Run `sparql` on one tier file when `wanted` says its store is one to change, under the graph lock from load to
/// write. Returns whether it ran.
fn mutate_file_where(
    path: &Path,
    ns: &NamespaceConfig,
    sparql: &str,
    wanted: &dyn Fn(&Store) -> bool,
) -> Result<bool> {
    crate::store::with_graph_lock(path, || {
        let store = crate::store::load_or_empty(path)?;
        if !wanted(&store) {
            return Ok(false);
        }
        let full = format!("{}\n{}", crud::prefixes(ns), sparql);
        crate::store::update_and_write(
            &store,
            path,
            &full,
            crate::store::Scope::Target,
            crate::store::Intent::Knowledge,
        )
        .with_context(|| format!("reminder update failed: {full}"))?;
        Ok(true)
    })
}

/// Run `sparql` against every tier file that actually holds `slug`, and return the tier labels
/// that changed.
///
/// An empty vec means no tier held it and the caller must NOT print success. This is the same
/// rule `crud::handoff` follows for issue #72: running the UPDATE over each tier and printing
/// success unconditionally makes a no-op and a real change indistinguishable from outside.
fn apply_to_tiers(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    sparql: &str,
) -> Result<Vec<String>> {
    let files = apply_where(gbl_root, cwd, ns, sparql, &|s| store_holds(s, ns, slug))?;
    Ok(labels(gbl_root, &files))
}

/// Run `sparql` against every tier file whose store `wanted` accepts, and return the files it ran on.
fn apply_where(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    sparql: &str,
    wanted: &dyn Fn(&Store) -> bool,
) -> Result<Vec<std::path::PathBuf>> {
    let mut changed = Vec::new();
    for f in crud::all_tier_files(gbl_root, cwd) {
        if mutate_file_where(&f, ns, sparql, wanted)? {
            changed.push(f);
        }
    }
    Ok(changed)
}

fn labels(gbl_root: Option<&Path>, files: &[std::path::PathBuf]) -> Vec<String> {
    files.iter().map(|f| crud::tier_label_of_file(f, gbl_root).to_string()).collect()
}

/// The tier files a lookup searched, for the not-found sentence.
pub fn searched_tiers(gbl_root: Option<&Path>, cwd: &Path) -> Vec<String> {
    crud::all_tier_files(gbl_root, cwd)
        .iter()
        .map(|f| format!("{} ({})", f.display(), crud::tier_label_of_file(f, gbl_root)))
        .collect()
}

/// Move a reminder's surface time to `surface_at`, in every tier holding the slug.
///
/// Three things move together, because a reminder has one clock: `resurfaceAt` is the clock,
/// `dueDate` is rewritten when present so `list` never shows a stale date beside a fresh one,
/// and `status` is dropped so snoozing an archived reminder brings it back. D3's "a snooze
/// resets the due date" is this, with no second field left out of step. `warnedAt` goes with
/// them (BO-27, V4): a reset reminder that falls overdue again earns a fresh warning.
pub fn snooze(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    surface_at: &str,
    due_date: &str,
) -> Result<Vec<String>> {
    reset(gbl_root, cwd, ns, slug, surface_at, due_date, false)
}

/// [`snooze`]'s one clock move, in every tier holding the slug, or with `only_archived` only in the tiers where it is
/// archived ([`unarchive`]: a live copy in another tier keeps its own clock and warning).
fn reset(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    surface_at: &str,
    due_date: &str,
    only_archived: bool,
) -> Result<Vec<String>> {
    let iri = crud::build_iri(ns, "reminder", slug);
    let p = &ns.prefix;
    let sparql = format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt ?old .\n\
                               <{iri}> {p}:dueDate ?oldDue .\n\
                               <{iri}> {p}:status ?oldStatus .\n\
                               <{iri}> {p}:archivedAt ?oldAt .\n\
                               <{iri}> {p}:archivedReason ?oldWhy .\n\
                               <{iri}> {p}:warnedAt ?oldWarned }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt \"{surface_at}\"^^xsd:dateTime }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Reminder }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt ?old }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:dueDate ?oldDue }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:status ?oldStatus }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:archivedAt ?oldAt }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:archivedReason ?oldWhy }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:warnedAt ?oldWarned }} }} }}"
    );
    let wanted = |s: &Store| store_holds(s, ns, slug) && (!only_archived || store_archived(s, ns, slug));
    let changed = apply_where(gbl_root, cwd, ns, &sparql, &wanted)?;

    // The rewritten dueDate lands in a second pass, and only where the first one changed
    // something: a reminder with no dueDate must not gain one from a snooze.
    let restore = format!(
        "INSERT {{ GRAPH ?g {{ <{iri}> {p}:dueDate \"{due_date}\"^^xsd:date }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Reminder ; {p}:resurfaceAt ?w }} }}"
    );
    for f in &changed {
        mutate_file_where(f, ns, &restore, &|s| store_holds(s, ns, slug))?;
    }
    Ok(labels(gbl_root, &changed))
}

/// Archive a reminder in every tier holding the slug: it stops surfacing and is KEPT.
/// `remove` is still the hard delete (D5), and the two stay separate on purpose (R4).
pub fn archive(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    reason: Option<&str>,
) -> Result<Vec<String>> {
    let iri = crud::build_iri(ns, "reminder", slug);
    let now = crud::now_iso();
    let p = &ns.prefix;
    let why = match reason {
        Some(r) => format!(
            "                               <{iri}> {p}:archivedReason \"{}\" .\n",
            crud::escape_sparql_literal(r)
        ),
        None => String::new(),
    };
    let sparql = format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:status \"{ARCHIVED}\" .\n\
                               <{iri}> {p}:archivedAt \"{now}\"^^xsd:dateTime .\n\
{why}                          }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Reminder }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }} }}"
    );
    apply_to_tiers(gbl_root, cwd, ns, slug, &sparql)
}

/// What [`unarchive`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unarchived {
    /// No tier holds the slug.
    NotFound,
    /// It is live already; nothing was changed.
    NotArchived,
    /// Live again in these tiers, surfacing from `surface_at`.
    Restored { tiers: Vec<String>, surface_at: String },
}

/// `base reminder unarchive` (BO-27, V5): an archived reminder is live again. It surfaces from its own time when that
/// is still ahead, else from now, and its archive and its warning are cleared ([`snooze`] is the one place a reminder's
/// clock moves). Before this there was no way back from an archive except a snooze, which no message named.
pub fn unarchive(gbl_root: Option<&Path>, cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<Unarchived> {
    let held = find(gbl_root, cwd, ns, slug)?;
    if held.is_empty() {
        return Ok(Unarchived::NotFound);
    }
    if !held.iter().any(|r| r.archived) {
        return Ok(Unarchived::NotArchived);
    }
    let now = chrono::Local::now();
    let at = held
        .iter()
        .filter(|r| r.archived)
        .filter_map(|r| surface_time(&r.when))
        .max()
        .filter(|t| *t > now)
        .unwrap_or(now);
    let surface_at = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let tiers = reset(gbl_root, cwd, ns, slug, &surface_at, &at.format("%Y-%m-%d").to_string(), true)?;
    Ok(Unarchived::Restored { tiers, surface_at })
}

/// Record that session start just showed each reminder's warning, in every tier file that holds one of them (BO-27,
/// V4). Only a reminder with no `warnedAt` gets one, so the first showing is kept and never moved: the archive counts
/// [`WARN_GRACE_DAYS`] from it. Returns the tiers written.
pub fn mark_warned(gbl_root: Option<&Path>, cwd: &Path, ns: &NamespaceConfig, slugs: &[String]) -> Result<Vec<String>> {
    if slugs.is_empty() {
        return Ok(Vec::new());
    }
    let p = &ns.prefix;
    let now = crud::now_iso();
    let values: Vec<String> = slugs.iter().map(|s| format!("<{}>", crud::build_iri(ns, "reminder", s))).collect();
    let sparql = format!(
        "INSERT {{ GRAPH ?g {{ ?r {p}:warnedAt \"{now}\"^^xsd:dateTime }} }}
         WHERE  {{ VALUES ?r {{ {values} }}
           GRAPH ?g {{ ?r a {p}:Reminder }}
           FILTER NOT EXISTS {{ GRAPH ?g {{ ?r {p}:warnedAt ?w }} }} }}",
        values = values.join(" "),
    );
    let files = apply_where(gbl_root, cwd, ns, &sparql, &|store| slugs.iter().any(|s| store_holds(store, ns, s)))?;
    Ok(labels(gbl_root, &files))
}

/// One row of `list`, read from one tier file so the tier is known rather than guessed.
struct Row {
    slug: String,
    name: String,
    when: String,
    tier: String,
    archived: bool,
    /// When session start first showed its archive warning (BO-27, V4).
    warned: Option<String>,
}

/// Every row of `slug`, one per tier file that holds it.
fn find(gbl_root: Option<&Path>, cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<Vec<Row>> {
    let mut out = Vec::new();
    for f in crud::all_tier_files(gbl_root, cwd) {
        let tier = crud::tier_label_of_file(&f, gbl_root);
        out.extend(rows_in(&f, ns, tier)?.into_iter().filter(|r| r.slug == slug));
    }
    Ok(out)
}

fn rows_in(path: &Path, ns: &NamespaceConfig, tier: &str) -> Result<Vec<Row>> {
    let store = crate::store::load_or_empty(path)?;
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?r ?name ?when ?status ?warned WHERE {{\n\
           GRAPH ?g {{\n\
             ?r a {p}:Reminder ;\n\
               {p}:name ?name ;\n\
               {p}:resurfaceAt ?when .\n\
             OPTIONAL {{ ?r {p}:status ?status }}\n\
             OPTIONAL {{ ?r {p}:warnedAt ?warned }}\n\
           }}\n\
         }}\n\
         ORDER BY ?when",
        pfx = crud::prefixes(ns)
    );
    let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? else {
        return Ok(Vec::new());
    };
    Ok(solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            let get = |k: &str| {
                row.get(k)
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default()
            };
            let iri = get("r");
            let warned = get("warned");
            Row {
                slug: iri.rsplit('/').next().unwrap_or(&iri).to_string(),
                name: get("name"),
                when: get("when"),
                tier: tier.to_string(),
                archived: get("status") == ARCHIVED,
                warned: (!warned.is_empty()).then_some(warned),
            }
        })
        .collect())
}

/// When a reminder is next due, in the operator's words.
fn due_column(when: &str) -> String {
    match days_past(when) {
        Some(d) if d > 0 => format!("{d}d overdue"),
        Some(0) => "today".to_string(),
        Some(d) => format!("in {}d", -d),
        None => "—".to_string(),
    }
}

/// List reminders across BOTH tiers.
///
/// The slug column is not decoration. Flag 6 makes `base reminder archive <slug>` the clear
/// command on DUE NOW, and before this rank `list` printed the name and never the slug — so the
/// listing could not tell the operator what to type. A command you cannot construct from the
/// listing in front of you is not a command.
pub fn list(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    archived: bool,
) -> Result<()> {
    let mut rows: Vec<Row> = Vec::new();
    for f in crud::all_tier_files(gbl_root, cwd) {
        let tier = crud::tier_label_of_file(&f, gbl_root);
        rows.extend(rows_in(&f, ns, tier)?);
    }
    rows.retain(|r| r.archived == archived);
    rows.sort_by(|a, b| a.when.cmp(&b.when));

    if rows.is_empty() {
        println!(
            "No {}reminders.",
            if archived { "archived " } else { "" }
        );
        return Ok(());
    }

    println!("| slug | name | tier | surfaces at | due |");
    println!("|------|------|------|-------------|-----|");
    for r in &rows {
        println!(
            "| {} | {} | {} | {} | {} |",
            r.slug,
            r.name,
            r.tier,
            r.when,
            due_column(&r.when)
        );
    }
    Ok(())
}

pub fn remove(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let iri = crud::build_iri(ns, "reminder", slug);

    // Hard delete: remove all triples about this reminder
    let sparql = format!("DELETE WHERE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}");

    crud::load_and_mutate(cwd, ns, &sparql)
}

/// A reminder the auto-archive pass archived, for the line session start prints about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoArchived {
    pub slug: String,
    pub name: String,
    /// Whole days past due when it was archived.
    pub days: i64,
    /// The local date its warning was first shown, `YYYY-MM-DD`.
    pub warned_on: String,
}

impl AutoArchived {
    /// The one line session start prints for it, with its undo (BO-27, V4), in the shape of an upgrade's lines.
    pub fn line(&self) -> String {
        format!(
            "reminder: archived '{}' ({}d overdue, warned {}) · undo: base reminder unarchive {}",
            self.name, self.days, self.warned_on, self.slug
        )
    }
}

/// Archive every reminder that is [`AUTO_ARCHIVE_DAYS`] or more past due and whose warning was
/// shown [`WARN_GRACE_DAYS`] or more days ago, in every tier, and return what was archived.
///
/// Deliberately NOT inside `protocol::reconcile`: that pass is gated on `[protocol] enabled`
/// and holds a workspace lock, and reminders are neither protocol-gated nor workspace-only.
/// R3 says the reminder is archived, never deleted, so this writes a status and a reason and
/// leaves the record whole.
pub fn auto_archive_pass(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
) -> Result<Vec<AutoArchived>> {
    let mut archived = Vec::new();
    for due in overdue_for_auto_archive(gbl_root, cwd, ns)? {
        let reason = format!("auto: {}d past due, warned {}", due.days, due.warned_on);
        if !archive(gbl_root, cwd, ns, &due.slug, Some(&reason))?.is_empty() {
            archived.push(due);
        }
    }
    Ok(archived)
}

/// Every live reminder at or past [`AUTO_ARCHIVE_DAYS`] whose warning was first shown at least
/// [`WARN_GRACE_DAYS`] ago, in every tier, for the pass to archive. Reading and writing are
/// separate so the pass can report what it did.
///
/// THE WARNING IS THE GATE (BO-27, V4). Until BO-27 this took every reminder past day 10, and the
/// day-8 warning was only text: nothing checked it had been seen. A user upgrading from a build
/// with no warning, or one who opened no session between day 8 and day 10, lost reminders at a
/// session start with no line anywhere. A reminder past day 10 that was never warned is warned at
/// the next session start and archived no sooner than [`WARN_GRACE_DAYS`] later.
pub fn overdue_for_auto_archive(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
) -> Result<Vec<AutoArchived>> {
    let mut out: Vec<AutoArchived> = Vec::new();
    for f in crud::all_tier_files(gbl_root, cwd) {
        let tier = crud::tier_label_of_file(&f, gbl_root);
        for r in rows_in(&f, ns, tier)? {
            if r.archived || out.iter().any(|d| d.slug == r.slug) {
                continue;
            }
            let Some(days) = days_past(&r.when).filter(|d| *d >= AUTO_ARCHIVE_DAYS) else { continue };
            let Some(warned) = r.warned.as_deref().filter(|w| days_since(w).is_some_and(|d| d >= WARN_GRACE_DAYS))
            else {
                continue;
            };
            let warned_on = local_date(warned).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
            out.push(AutoArchived { slug: r.slug, name: r.name, days, warned_on });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BO-06 review finding 4: the due rule reads a `resurfaceAt` with no timezone as local time, as the old SPARQL
    /// comparison ordered it, instead of never counting it. Controls: an RFC 3339 time, an archived reminder, garbage.
    #[test]
    fn is_due_reads_a_time_with_no_timezone_as_local() {
        let now = chrono::Local::now();
        let yesterday = (now - chrono::Duration::days(1)).naive_local();
        let tomorrow = (now + chrono::Duration::days(1)).naive_local();
        for (value, archived, due) in [
            (yesterday.format("%Y-%m-%dT%H:%M:%S").to_string(), false, true),
            (yesterday.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(), false, true),
            (tomorrow.format("%Y-%m-%dT%H:%M:%S").to_string(), false, false),
            ((now - chrono::Duration::hours(1)).to_rfc3339(), false, true),
            ((now - chrono::Duration::hours(1)).to_rfc3339(), true, false),
            ("not a time".to_string(), false, false),
        ] {
            assert_eq!(is_due(&value, archived, now), due, "{value:?}, archived {archived}");
        }
    }

    /// BO-27 code review: a `warnedAt` stored with no timezone, as another writer may store an `xsd:dateTime`, still
    /// counts its days, so it cannot hold a reminder back from its archive forever. Controls: RFC 3339, and garbage.
    #[test]
    fn a_warning_time_with_no_timezone_still_counts() {
        let now = chrono::Local::now();
        let two_ago = (now - chrono::Duration::days(2)).naive_local();
        assert_eq!(days_since(&two_ago.format("%Y-%m-%dT%H:%M:%S").to_string()), Some(2));
        assert_eq!(days_since(&(now - chrono::Duration::days(2)).to_rfc3339()), Some(2));
        assert_eq!(days_since("not a time"), None);
        let due = (now - chrono::Duration::days(15)).to_rfc3339();
        let on = (now + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
        let warned = (now - chrono::Duration::days(1)).naive_local().format("%Y-%m-%dT%H:%M:%S").to_string();
        assert_eq!(archives_on(&due, Some(&warned)).as_deref(), Some(on.as_str()), "first shown yesterday: tomorrow");
    }
}
