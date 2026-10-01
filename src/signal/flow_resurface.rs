use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::{DeferKind, FlowConfig, NamespaceConfig, SessionStartConfig};
use crate::crud;
use crate::crud::handoff_show::{self, HandoffList};

/// Flow resurface signal: surfaces items that need attention.
/// Three sub-queries: blocked-by scan, deferred orphan scan, mention threshold.
/// (Stale detection removed — [protocol] reconcile owns active→deferred decay.)
/// Returns (content, diagnostics) — diagnostics are no-match tags for each sub-query.
/// Priority 2 (competes with pulse for budget space).
pub fn run(cwd: &Path, ns: &NamespaceConfig, flow: &FlowConfig, hook: &str) -> Result<(String, Vec<String>)> {
    let mut sections: Vec<String> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();

    // NOTE: handoff_scan + reminder_scan are run as their own signals in signal::mod
    // (priority 0, never skipped as unchanged) so they surface EVERY session. Like every
    // block, they are measured against the session-start budget.

    // Sub-query 1: Blocked-by scan
    match blocked_by_scan(cwd, ns) {
        Ok(output) if !output.is_empty() => sections.push(output),
        Ok(_) => diagnostics.push(format!("<{hook}-blocked-scan:no-match>")),
        Err(_) => {}
    }

    // Sub-query 2: Deferred orphan scan
    match deferred_orphan_scan(cwd, ns) {
        Ok(output) if !output.is_empty() => sections.push(output),
        Ok(_) => diagnostics.push(format!("<{hook}-deferred-scan:no-match>")),
        Err(_) => {}
    }

    // Sub-query 3: Mention threshold scan (gated by flow.mentions)
    if flow.mentions {
        match mention_threshold_scan(cwd, ns, flow.mention_threshold) {
            Ok(output) if !output.is_empty() => sections.push(output),
            Ok(_) => diagnostics.push(format!("<{hook}-mentions-scan:no-match>")),
            Err(_) => {}
        }
    }

    let content = if sections.is_empty() {
        String::new()
    } else {
        let mut output = String::from("<flow-resurface>\n");
        output.push_str(&sections.join("\n"));
        output.push_str("\n</flow-resurface>");
        output
    };

    Ok((content, diagnostics))
}

/// Find entities with status "blocked" whose blocker entity has status "completed" or "active".
/// These items just unblocked and need attention.
fn blocked_by_scan(cwd: &Path, ns: &NamespaceConfig) -> Result<String> {
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?name ?blockerName ?blockerStatus WHERE {{\n\
           GRAPH ?g {{\n\
             ?entity {p}:name ?name ;\n\
               {p}:status \"blocked\" ;\n\
               {p}:blockedBy ?blocker .\n\
             ?blocker {p}:name ?blockerName ;\n\
               {p}:status ?blockerStatus .\n\
             FILTER(?blockerStatus IN (\"completed\", \"active\"))\n\
           }}\n\
         }}\n\
         ORDER BY ?name"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let QueryResults::Solutions(solutions) = results else {
        return Ok(String::new());
    };

    let rows: Vec<(String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            (
                row.get("name").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                row.get("blockerName").map(|t| crud::term_display(t.into())).unwrap_or_default(),
            )
        })
        .collect();

    if rows.is_empty() {
        return Ok(String::new());
    }

    let mut output = String::from("[Unblocked]\n");
    for (name, blocker) in &rows {
        output.push_str(&format!("- {name} (was blocked by {blocker})\n"));
    }

    Ok(output)
}

/// Find deferred entities with a resurfaceAt date in the past. A record the deferral pass parked
/// (`deferredReason` starting `auto:`) is left out: every one carries a past `resurfaceAt`, so the
/// first pass would otherwise print all of them here. An operator's own "defer until" still shows.
fn deferred_orphan_scan(cwd: &Path, ns: &NamespaceConfig) -> Result<String> {
    let now_str = chrono::Local::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);

    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?name ?resurfaceAt WHERE {{\n\
           GRAPH ?g {{\n\
             ?entity {p}:name ?name ;\n\
               {p}:status \"deferred\" ;\n\
               {p}:resurfaceAt ?resurfaceAt .\n\
             FILTER(?resurfaceAt < \"{now_str}\"^^xsd:dateTime)\n\
             FILTER NOT EXISTS {{ ?entity {p}:deferredReason ?why . FILTER(STRSTARTS(STR(?why), \"auto:\")) }}\n\
           }}\n\
         }}\n\
         ORDER BY ?resurfaceAt"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let QueryResults::Solutions(solutions) = results else {
        return Ok(String::new());
    };

    let rows: Vec<(String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            (
                row.get("name").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                row.get("resurfaceAt").map(|t| crud::term_display(t.into())).unwrap_or_default(),
            )
        })
        .collect();

    if rows.is_empty() {
        return Ok(String::new());
    }

    let mut output = String::from("[Resurface]\n");
    for (name, resurface_at) in &rows {
        output.push_str(&format!("- {name} (deferred until {resurface_at}, now past due)\n"));
    }

    Ok(output)
}

/// The HANDOFFS block (spec B4, B5): open handoffs whose `resurfaceAt` has passed, across the
/// global and workspace tiers, newest created first, one per project, at most ten, lettered from A.
/// A line is the project, the codename read off the slug (the slug itself when it has no codename
/// shape) and the age. No path: `base handoff show` finds the doc. Returns the block, the list, whose
/// letters the instruction block and the letters file carry, and how many handoffs are deferred.
pub fn handoff_scan(
    cwd: &Path,
    ns: &NamespaceConfig,
    cfg: &SessionStartConfig,
) -> Result<(String, HandoffList, usize)> {
    let Some(store) = crate::store::load_merged(cwd) else {
        return Ok((String::new(), HandoffList::default(), 0));
    };
    let rows = handoff_show::open_handoffs(&store, ns, true, None)?;
    let list = handoff_show::session_start_list(rows, cfg);
    // C8: a block with nothing open and something deferred still renders, so its notice has a place.
    let deferred = crud::deferred::count_in(&store, ns, DeferKind::Handoff)?;
    let notice = crud::deferred::notice(DeferKind::Handoff, deferred);
    if list.shown.is_empty() && notice.is_none() {
        return Ok((String::new(), list, deferred));
    }

    let now = chrono::Local::now();
    let mut out = format!(
        "HANDOFFS ({} open, newest {} shown) · all: base handoff list\n",
        list.open,
        list.shown.len()
    );
    for l in &list.shown {
        let h = &l.handoff;
        let who = handoff_show::codename_of(&h.slug).unwrap_or(&h.slug);
        let older = if l.older > 0 {
            format!(" (+{} older)", l.older)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "  {}) {} · {who} · {}d{older}\n",
            l.letter,
            h.project,
            h.age_days(now)
        ));
    }
    if let Some(line) = notice {
        out.push_str(&format!("  {line}\n"));
    }
    Ok((out.trim_end().to_string(), list, deferred))
}

/// The FORKS block (spec B6): open forks whose `resurfaceAt` has passed, across both tiers. The
/// count, the newest `forks_shown` of them (title, project, age; no path) and the command that
/// lists them all. Forks are additive side-work, several open at once, each summoned by its title
/// (== slug == doc basename). Returns the block, how many are open, how many it lists, and how many
/// forks are deferred.
pub fn fork_scan(
    cwd: &Path,
    ns: &NamespaceConfig,
    cfg: &SessionStartConfig,
) -> Result<(String, usize, usize, usize)> {
    let Some(store) = crate::store::load_merged(cwd) else {
        return Ok((String::new(), 0, 0, 0));
    };
    let now = chrono::Local::now();
    let now_str = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?h ?project ?created WHERE {{\n\
           GRAPH ?g {{\n\
             ?h a {p}:Handoff ;\n\
               {p}:kind \"fork\" ;\n\
               {p}:status \"open\" ;\n\
               {p}:project ?project ;\n\
               {p}:handoffDoc ?doc ;\n\
               {p}:createdAt ?created ;\n\
               {p}:resurfaceAt ?resurfaceAt .\n\
             FILTER(?resurfaceAt <= \"{now_str}\"^^xsd:dateTime)\n\
           }}\n\
         }}\n\
         ORDER BY DESC(?created) ?h",
        pfx = crud::prefixes(ns)
    );

    let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? else {
        return Ok((String::new(), 0, 0, 0));
    };

    let rows: Vec<(String, String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            let get = |k: &str| {
                row.get(k)
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default()
            };
            let h = get("h");
            let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
            (slug, get("project"), get("created"))
        })
        .collect();

    // C8: a block with nothing open and something deferred still renders, so its notice has a place.
    let deferred = crud::deferred::count_in(&store, ns, DeferKind::Fork)?;
    let notice = crud::deferred::notice(DeferKind::Fork, deferred);
    if rows.is_empty() && notice.is_none() {
        return Ok((String::new(), 0, 0, deferred));
    }

    let shown = rows.len().min(cfg.forks_shown);
    let mut out = format!(
        "FORKS ({} open, newest {shown} shown) · all: base fork list\n",
        rows.len()
    );
    for (slug, project, created) in rows.iter().take(shown) {
        let days = chrono::DateTime::parse_from_rfc3339(created)
            .map(|dt| now.signed_duration_since(dt).num_days())
            .unwrap_or(0);
        out.push_str(&format!("  {slug} · {project} · {days}d\n"));
    }
    if let Some(line) = notice {
        out.push_str(&format!("  {line}\n"));
    }

    Ok((out.trim_end().to_string(), rows.len(), shown, deferred))
}

/// The DUE NOW block and what session start needs to keep it on the first screen.
#[derive(Debug, Default)]
pub struct DueNow {
    /// Every due reminder, numbered from 1, oldest due first. Empty when nothing is due.
    pub text: String,
    /// The slug of each numbered reminder, number 1 first: `base reminder archive <number>` and
    /// `snooze <number>` read them back from the letters file session start writes.
    pub slugs: Vec<String>,
    /// The block listing only the first k reminders, k from `slugs.len() - 1` down to 1, each with
    /// k. Session start steps down these when DUE NOW would push the first screen past its limit;
    /// the last one keeps the most overdue reminder, so DUE NOW never shows none.
    pub fits: Vec<(String, usize)>,
}

/// The DUE NOW block (spec B1 row 3): reminders whose `resurfaceAt` time has passed, across both
/// tiers, oldest due first, each numbered.
///
/// UNTIL 2026-10-01 EACH LINE ENDED `· clear: base reminder archive <slug>`, and slugs are built
/// from the reminder's name, so the clause ran to about 105 UTF-16 units a line. Chris's store had
/// five due that day: DUE NOW was 1,219 units of a first screen that measured 2,871 against 2,000
/// (BO-00 B4). The number now stands in for the slug: instruction line 3 says what to run, and
/// `reminder archive|snooze <number>` resolve it through the letters file.
pub fn reminder_scan(cwd: &Path, ns: &NamespaceConfig) -> Result<DueNow> {
    let Some(store) = crate::store::load_merged(cwd) else {
        return Ok(DueNow::default());
    };
    let now_str = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?r ?name ?when WHERE {{\n\
           GRAPH ?g {{\n\
             ?r a {p}:Reminder ;\n\
               {p}:name ?name ;\n\
               {p}:resurfaceAt ?when .\n\
             FILTER(?when <= \"{now_str}\"^^xsd:dateTime)\n\
             FILTER NOT EXISTS {{ ?r {p}:status \"archived\" }}\n\
           }}\n\
         }}\n\
         ORDER BY ?when",
        pfx = crud::prefixes(ns)
    );

    let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? else {
        return Ok(DueNow::default());
    };

    let rows: Vec<(String, String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            let get = |k: &str| {
                row.get(k)
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default()
            };
            let r = get("r");
            let slug = r.rsplit('/').next().unwrap_or(&r).to_string();
            (slug, get("name"), get("when"))
        })
        .collect();

    if rows.is_empty() {
        return Ok(DueNow::default());
    }

    // Flag 6 still holds: a handled reminder is archived, not removed. Line 3 of the instruction
    // block names `base reminder archive <number>`; `remove` is never what session start offers.
    let lines: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(i, (_, name, when))| {
            let n = i + 1;
            let mut line = format!("  {n} {name}");
            // R3/D4: from day 8 the line says when it goes and how to keep it.
            if crud::reminder::days_past(when)
                .is_some_and(|d| d >= crud::reminder::WARN_FROM_DAYS)
                && let Some(on) = crud::reminder::archives_on(when)
            {
                line.push_str(&format!(
                    " · archives {on} unless reset: base reminder snooze {n} <duration>"
                ));
            }
            line
        })
        .collect();
    let total = lines.len();
    let block = |shown: usize| {
        let head = if shown == total {
            format!("DUE NOW ({total}) · all: base reminder list")
        } else {
            format!(
                "DUE NOW ({total}) · {shown} shown · +{} more · all: base reminder list",
                total - shown
            )
        };
        std::iter::once(head)
            .chain(lines[..shown].iter().cloned())
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(DueNow {
        text: block(total),
        slugs: rows.into_iter().map(|(slug, _, _)| slug).collect(),
        fits: (1..total).rev().map(|k| (block(k), k)).collect(),
    })
}

/// Find notes with mentionCount >= threshold — recurring ideas that should be promoted.
fn mention_threshold_scan(cwd: &Path, ns: &NamespaceConfig, threshold: u32) -> Result<String> {
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?text ?count WHERE {{\n\
           GRAPH ?g {{\n\
             ?note a {p}:Note ;\n\
               {p}:noteText ?text ;\n\
               {p}:mentionCount ?count ;\n\
               {p}:status \"active\" .\n\
             FILTER(?count >= {threshold})\n\
           }}\n\
         }}\n\
         ORDER BY DESC(?count)"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let QueryResults::Solutions(solutions) = results else {
        return Ok(String::new());
    };

    let rows: Vec<(String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            let text = row.get("text").map(|t| crud::term_display(t.into())).unwrap_or_default();
            let count = row.get("count").map(|t| crud::term_display(t.into())).unwrap_or_default();
            // Truncate long text for signal display
            let preview = if text.len() > 80 {
                // Back off to a UTF-8 char boundary so a multi-byte char at byte 80
                // doesn't panic the slice.
                let mut cut = 80;
                while cut > 0 && !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                format!("{}...", &text[..cut])
            } else {
                text
            };
            (preview, count)
        })
        .collect();

    if rows.is_empty() {
        return Ok(String::new());
    }

    let mut output = String::from("[Recurring]\n");
    for (preview, count) in &rows {
        output.push_str(&format!("- \"{preview}\" (mentioned {count} times — consider promoting to project)\n"));
    }

    Ok(output)
}
