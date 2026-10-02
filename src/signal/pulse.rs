use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;
use crate::signal::counts::Counts;

/// Pulse signal: a one-glance summary of the working set. Priority 2.
///
/// It prints the counts session start counted once (BO-06, F10), so each number here is the number the header and the
/// block with the same label print. Until BO-06 it ran four queries of its own over the workspace tier only and
/// disagreed with both: `Tasks: 145 open` beside a TASKS block of every working task in both tiers, `Reminders: 4
/// overdue` beside a DUE NOW of 5. "Tasks" now says `active`, as TASKS does, and "Reminders" says `due`, as DUE NOW
/// does. Empty when there is nothing to count.
pub fn render(counts: &Counts) -> String {
    let p = &counts.projects;
    if p.active + p.blocked + p.completed + counts.tasks.active + counts.reminders_due + counts.decisions_week == 0 {
        return String::new();
    }
    let mut lines = Vec::new();
    // A number whose scan failed is left out, never printed as 0 (BO-06 review).
    if counts.known("projects") {
        lines.push(format!("Projects: {} active, {} blocked, {} completed", p.active, p.blocked, p.completed));
    }
    if counts.known("tasks") && counts.tasks.active > 0 {
        lines.push(format!("Tasks: {} active", counts.tasks.active));
    }
    if counts.known("due") && counts.reminders_due > 0 {
        lines.push(format!("Reminders: {} due", counts.reminders_due));
    }
    if counts.decisions_week > 0 {
        lines.push(format!("Decisions: {} this week", counts.decisions_week));
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("[Workspace Pulse]\n{}", lines.join("\n"))
}

/// Decisions logged in the last seven days, in both tiers: the one count the pulse shows that no block lists. Until
/// BO-06 it read the workspace tier only, while every other number at session start reads both.
pub fn decisions_this_week(cwd: &Path, ns: &NamespaceConfig) -> Result<usize> {
    let Some(store) = crate::store::load_merged(cwd) else {
        return Ok(0);
    };
    let p = &ns.prefix;
    let cutoff = chrono::Local::now()
        .checked_sub_signed(chrono::Duration::days(7))
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
        .unwrap_or_default();
    let sparql = format!(
        "{pfx}\nSELECT (COUNT(DISTINCT ?d) AS ?count) WHERE {{\n\
           GRAPH ?g {{ ?d a {p}:Decision ; {p}:createdAt ?ts . FILTER(?ts > \"{cutoff}\"^^xsd:dateTime) }}\n\
         }}",
        pfx = crud::prefixes(ns)
    );
    let QueryResults::Solutions(mut solutions) = crate::store::query(&store, &sparql)? else {
        return Ok(0);
    };
    Ok(solutions
        .next()
        .and_then(|row| row.ok())
        .and_then(|row| row.get("count").map(|t| crud::term_display(t.into())))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}
