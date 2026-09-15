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

/// The date a reminder due at `when` archives itself, for the warning line.
pub fn archives_on(when: &str) -> Option<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(when).ok()?;
    Some(
        (parsed.with_timezone(&chrono::Local).date_naive()
            + chrono::Duration::days(AUTO_ARCHIVE_DAYS))
        .format("%Y-%m-%d")
        .to_string(),
    )
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

fn mutate_file_if_holds(
    path: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    sparql: &str,
) -> Result<bool> {
    crate::store::with_graph_lock(path, || {
        let store = crate::store::load_or_empty(path)?;
        if !store_holds(&store, ns, slug) {
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
    let mut changed = Vec::new();
    for f in crud::all_tier_files(gbl_root, cwd) {
        if mutate_file_if_holds(&f, ns, slug, sparql)? {
            changed.push(crud::tier_label_of_file(&f, gbl_root).to_string());
        }
    }
    Ok(changed)
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
/// resets the due date" is this, with no second field left out of step.
pub fn snooze(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    surface_at: &str,
    due_date: &str,
) -> Result<Vec<String>> {
    let iri = crud::build_iri(ns, "reminder", slug);
    let p = &ns.prefix;
    let sparql = format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt ?old .\n\
                               <{iri}> {p}:dueDate ?oldDue .\n\
                               <{iri}> {p}:status ?oldStatus .\n\
                               <{iri}> {p}:archivedAt ?oldAt .\n\
                               <{iri}> {p}:archivedReason ?oldWhy }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt \"{surface_at}\"^^xsd:dateTime }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Reminder }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt ?old }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:dueDate ?oldDue }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:status ?oldStatus }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:archivedAt ?oldAt }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:archivedReason ?oldWhy }} }} }}"
    );
    let changed = apply_to_tiers(gbl_root, cwd, ns, slug, &sparql)?;

    // The rewritten dueDate lands in a second pass, and only where the first one changed
    // something: a reminder with no dueDate must not gain one from a snooze.
    if !changed.is_empty() {
        let restore = format!(
            "INSERT {{ GRAPH ?g {{ <{iri}> {p}:dueDate \"{due_date}\"^^xsd:date }} }}\n\
             WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Reminder ; {p}:resurfaceAt ?w }} }}"
        );
        apply_to_tiers(gbl_root, cwd, ns, slug, &restore)?;
    }
    Ok(changed)
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

/// One row of `list`, read from one tier file so the tier is known rather than guessed.
struct Row {
    slug: String,
    name: String,
    when: String,
    tier: String,
    archived: bool,
}

fn rows_in(path: &Path, ns: &NamespaceConfig, tier: &str) -> Result<Vec<Row>> {
    let store = crate::store::load_or_empty(path)?;
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?r ?name ?when ?status WHERE {{\n\
           GRAPH ?g {{\n\
             ?r a {p}:Reminder ;\n\
               {p}:name ?name ;\n\
               {p}:resurfaceAt ?when .\n\
             OPTIONAL {{ ?r {p}:status ?status }}\n\
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
            Row {
                slug: iri.rsplit('/').next().unwrap_or(&iri).to_string(),
                name: get("name"),
                when: get("when"),
                tier: tier.to_string(),
                archived: get("status") == ARCHIVED,
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

/// Archive every reminder that is [`AUTO_ARCHIVE_DAYS`] or more past due, in every tier, and
/// return the slugs archived.
///
/// Deliberately NOT inside `protocol::reconcile`: that pass is gated on `[protocol] enabled`
/// and holds a workspace lock, and reminders are neither protocol-gated nor workspace-only.
/// R3 says the reminder is archived, never deleted, so this writes a status and a reason and
/// leaves the record whole.
pub fn auto_archive_pass(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
) -> Result<Vec<String>> {
    let mut archived = Vec::new();
    for slug in overdue_for_auto_archive(gbl_root, cwd, ns)? {
        let reason = format!("auto: {AUTO_ARCHIVE_DAYS}d past due");
        if !archive(gbl_root, cwd, ns, &slug, Some(&reason))?.is_empty() {
            archived.push(slug);
        }
    }
    Ok(archived)
}

/// Every live reminder at or past [`AUTO_ARCHIVE_DAYS`], in every tier, for the reconcile pass
/// to archive. Reading and writing are separate so the pass can report what it did.
pub fn overdue_for_auto_archive(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for f in crud::all_tier_files(gbl_root, cwd) {
        let tier = crud::tier_label_of_file(&f, gbl_root);
        for r in rows_in(&f, ns, tier)? {
            if r.archived {
                continue;
            }
            if days_past(&r.when).is_some_and(|d| d >= AUTO_ARCHIVE_DAYS) && !out.contains(&r.slug)
            {
                out.push(r.slug);
            }
        }
    }
    Ok(out)
}
