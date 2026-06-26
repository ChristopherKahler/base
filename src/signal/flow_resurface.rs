use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::{FlowConfig, NamespaceConfig};
use crate::crud;

/// Flow resurface signal: surfaces items that need attention.
/// Three sub-queries: blocked-by scan, deferred orphan scan, mention threshold.
/// (Stale detection removed — [protocol] reconcile owns active→deferred decay.)
/// Returns (content, diagnostics) — diagnostics are no-match tags for each sub-query.
/// Priority 2 (competes with pulse for budget space).
pub fn run(cwd: &Path, ns: &NamespaceConfig, flow: &FlowConfig, hook: &str) -> Result<(String, Vec<String>)> {
    let mut sections: Vec<String> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();

    // NOTE: handoff_scan + reminder_scan are run as their own signals in signal::mod
    // (priority 0, budget- and suppression-exempt) so they surface EVERY session.

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

/// Find deferred entities with a resurfaceAt date in the past.
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

/// Find OPEN handoffs whose `resurfaceAt` is in the past, across global + workspace
/// tiers, and render the lettered "pick up where you left off" delegation block.
pub fn handoff_scan(cwd: &Path, ns: &NamespaceConfig) -> Result<String> {
    let Some(store) = crate::store::load_merged(cwd) else {
        return Ok(String::new());
    };
    let now = chrono::Local::now();
    let now_str = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?h ?project ?doc ?created WHERE {{\n\
           GRAPH ?g {{\n\
             ?h a {p}:Handoff ;\n\
               {p}:status \"open\" ;\n\
               {p}:project ?project ;\n\
               {p}:handoffDoc ?doc ;\n\
               {p}:createdAt ?created ;\n\
               {p}:resurfaceAt ?resurfaceAt .\n\
             OPTIONAL {{ ?h {p}:kind ?kind }}\n\
             FILTER(!BOUND(?kind) || ?kind != \"fork\")\n\
             FILTER(?resurfaceAt <= \"{now_str}\"^^xsd:dateTime)\n\
           }}\n\
         }}\n\
         ORDER BY ?created",
        pfx = crud::prefixes(ns)
    );

    let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? else {
        return Ok(String::new());
    };

    let rows: Vec<(String, String, String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            let get = |k: &str| {
                row.get(k)
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default()
            };
            let h = get("h");
            let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
            (slug, get("project"), get("doc"), get("created"))
        })
        .collect();

    if rows.is_empty() {
        return Ok(String::new());
    }

    let mut out = String::from("[Pick up where you left off]\n");
    let mut letter_map: Vec<String> = Vec::new();
    for (i, (slug, project, doc, created)) in rows.iter().enumerate() {
        let letter = (b'A' + (i as u8 % 26)) as char;
        let days = chrono::DateTime::parse_from_rfc3339(created)
            .map(|dt| now.signed_duration_since(dt).num_days())
            .unwrap_or(0);
        out.push_str(&format!("{letter}) {project} · {doc} · {days}d\n"));
        letter_map.push(format!("{letter}={slug}"));
    }
    out.push_str(&format!(
        "BEHAVIOR: Render this as the FIRST thing in your reply — a clean lettered list \
         (project · path · age), nothing prepended. Pure delegation: no \"is this stale?\" \
         prompts. Operator replies with a letter → read that doc and resume; \
         \"snooze <letter> <N>d\" → run `base handoff snooze <slug> <N>`; \
         \"archive <letter>\" → run `base handoff archive <slug>`. Letter→slug: {}.",
        letter_map.join(", ")
    ));

    Ok(out)
}

/// Find OPEN forks (kind = "fork") whose `resurfaceAt` is in the past, across
/// global + workspace tiers, and render the "Forks" block. Forks are additive
/// parallel side-work — multiple surface at once, each summoned by its title
/// (== slug == doc basename), distinct from the single Handoff resume line.
pub fn fork_scan(cwd: &Path, ns: &NamespaceConfig) -> Result<String> {
    let Some(store) = crate::store::load_merged(cwd) else {
        return Ok(String::new());
    };
    let now = chrono::Local::now();
    let now_str = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?h ?project ?doc ?created WHERE {{\n\
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
         ORDER BY ?created",
        pfx = crud::prefixes(ns)
    );

    let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? else {
        return Ok(String::new());
    };

    let rows: Vec<(String, String, String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            let get = |k: &str| {
                row.get(k)
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default()
            };
            let h = get("h");
            let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
            (slug, get("project"), get("doc"), get("created"))
        })
        .collect();

    if rows.is_empty() {
        return Ok(String::new());
    }

    let mut out = String::from("[Forks]\n");
    for (slug, project, doc, created) in &rows {
        let days = chrono::DateTime::parse_from_rfc3339(created)
            .map(|dt| now.signed_duration_since(dt).num_days())
            .unwrap_or(0);
        out.push_str(&format!("- {slug} · {project} · {doc} · {days}d\n"));
    }
    out.push_str(
        "BEHAVIOR: These are open parallel side-work forks — independent of the continuity \
         handoff and of each other. To pick one up, name its title (the first field, == doc \
         basename) → read that doc and build the feature autonomously. Multiple can stay open \
         at once; do not treat these as a single lettered choice. \"snooze <title> <N>d\" → run \
         `base fork snooze <title> <N>`; \"archive <title>\" → run `base fork archive <title>`.",
    );

    Ok(out)
}

/// Surface reminders whose `resurfaceAt` time has passed, across global + workspace tiers.
pub fn reminder_scan(cwd: &Path, ns: &NamespaceConfig) -> Result<String> {
    let Some(store) = crate::store::load_merged(cwd) else {
        return Ok(String::new());
    };
    let now_str = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?r ?name WHERE {{\n\
           GRAPH ?g {{\n\
             ?r a {p}:Reminder ;\n\
               {p}:name ?name ;\n\
               {p}:resurfaceAt ?when .\n\
             FILTER(?when <= \"{now_str}\"^^xsd:dateTime)\n\
           }}\n\
         }}\n\
         ORDER BY ?when",
        pfx = crud::prefixes(ns)
    );

    let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? else {
        return Ok(String::new());
    };

    let rows: Vec<(String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            let get = |k: &str| {
                row.get(k)
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default()
            };
            let r = get("r");
            let slug = r.rsplit('/').next().unwrap_or(&r).to_string();
            (slug, get("name"))
        })
        .collect();

    if rows.is_empty() {
        return Ok(String::new());
    }

    let mut out = String::from("[Reminders]\n");
    for (slug, name) in &rows {
        out.push_str(&format!("- {name}  (clear: base reminder remove {slug})\n"));
    }
    out.push_str(
        "BEHAVIOR: These reminders are due now — surface them to the operator in your first reply. \
         Clear a handled one with `base reminder remove <slug>`.",
    );
    Ok(out)
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
                format!("{}...", &text[..80])
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
