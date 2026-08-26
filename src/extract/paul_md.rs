//! PAUL-aware markdown extractor for PLAN.md and SUMMARY.md artifacts.
//!
//! Extracts typed graph entities (Decision, Pattern, FileChange, AcceptanceCriteria)
//! instead of generic Document nodes with hasSection strings.

use crate::config::NamespaceConfig;

/// Check if a path is a PAUL artifact that should use this extractor.
pub fn is_paul_artifact(rel_path: &str) -> bool {
    let lower = rel_path.to_lowercase();
    (lower.contains(".paul/phases/") || lower.contains(".paul\\phases\\"))
        && (lower.ends_with("-plan.md") || lower.ends_with("-summary.md"))
}

/// Extract typed entities from a PAUL PLAN or SUMMARY markdown file.
/// Returns (predicate, value) triples under the document IRI, plus
/// additional entity triples for decisions, patterns, file changes, and ACs.
pub fn extract(content: &str, rel_path: &str, ns: &NamespaceConfig) -> Option<Vec<(String, String)>> {
    // `plan_id_from_path` and the summary/plan pairing below both split on `/`.
    let rel_path = &crate::crud::normalize_path_sep(rel_path);
    let p = &ns.prefix;
    let mut triples = Vec::new();

    // Parse frontmatter
    let (frontmatter, body) = split_frontmatter(content)?;

    // Determine artifact type
    let lower = rel_path.to_lowercase();
    let is_summary = lower.ends_with("-summary.md");
    let artifact_type = if is_summary { "PaulSummary" } else { "PaulPlan" };

    triples.push(("rdf:type".into(), format!("{p}:{artifact_type}")));
    triples.push((format!("{p}:path"), format!("\"{}\"", escape(rel_path))));

    // Extract frontmatter fields
    extract_frontmatter_fields(&frontmatter, &mut triples, ns);

    // Extract body sections based on artifact type
    if is_summary {
        extract_summary_entities(&body, &mut triples, ns, rel_path);
    } else {
        extract_plan_entities(&body, &mut triples, ns, rel_path);
    }

    Some(triples)
}

// ─── Frontmatter parsing ────────────────────────────────────

fn extract_frontmatter_fields(fm: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig) {
    let p = &ns.prefix;

    for line in fm.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() || line == "---" {
            continue;
        }

        // Simple key: value parsing (single-line scalars)
        if let Some((key, val)) = line.split_once(':') {
            let key = key.trim();
            let val = val.trim();
            if val.is_empty() || val == "|" || val == ">" {
                continue; // Skip multi-line values and block scalars
            }

            match key {
                "phase" => {
                    triples.push((format!("{p}:phase"), format!("\"{}\"", escape(val))));
                }
                "plan" => {
                    triples.push((format!("{p}:plan"), format!("\"{}\"", escape(val))));
                }
                "type" | "plan_type" => {
                    triples.push((format!("{p}:planType"), format!("\"{}\"", escape(val))));
                }
                "subsystem" => {
                    triples.push((format!("{p}:subsystem"), format!("\"{}\"", escape(val))));
                }
                "wave" => {
                    triples.push((format!("{p}:wave"), format!("\"{}\"", escape(val))));
                }
                "autonomous" => {
                    triples.push((format!("{p}:autonomous"), format!("\"{}\"", escape(val))));
                }
                "duration" => {
                    triples.push((format!("{p}:duration"), format!("\"{}\"", escape(val))));
                }
                "started" => {
                    triples.push((format!("{p}:started"), format!("\"{}\"^^xsd:dateTime", escape(val))));
                }
                "completed" => {
                    triples.push((format!("{p}:completed"), format!("\"{}\"^^xsd:dateTime", escape(val))));
                }
                "tags" => {
                    for tag in parse_inline_list(val) {
                        triples.push((format!("{p}:hasTag"), format!("\"{}\"", escape(&tag))));
                    }
                }
                "depends_on" => {
                    for dep in parse_inline_list(val) {
                        triples.push((format!("{p}:dependsOn"), format!("\"{}\"", escape(&dep))));
                    }
                }
                "affects" => {
                    for a in parse_inline_list(val) {
                        triples.push((format!("{p}:affects"), format!("\"{}\"", escape(&a))));
                    }
                }
                _ => {}
            }
        }
    }
}

// ─── SUMMARY entity extraction ──────────────────────────────

fn extract_summary_entities(body: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig, rel_path: &str) {
    let p = &ns.prefix;
    let sections = split_sections(body);

    for (heading, content) in &sections {
        let lower = heading.to_lowercase();

        if lower.contains("decisions made") {
            extract_decisions(content, triples, ns, rel_path);
        } else if lower.contains("files created") || lower.contains("files modified") {
            extract_file_changes(content, triples, ns, rel_path);
        } else if lower.contains("acceptance criteria") {
            extract_ac_results(content, triples, ns, rel_path);
        } else if lower.contains("deviations") {
            extract_deviations(content, triples, ns, rel_path);
        } else if lower.contains("accomplishments") {
            // Store as a summary string, not individual entities
            let text = content.lines()
                .filter(|l| !l.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !text.is_empty() {
                triples.push((format!("{p}:accomplishments"), format!("\"{}\"", escape(&text))));
            }
        } else if lower.contains("performance") {
            extract_performance(content, triples, ns);
        } else if lower.contains("next phase") {
            let text = content.lines()
                .filter(|l| !l.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !text.is_empty() {
                triples.push((format!("{p}:nextPhaseReadiness"), format!("\"{}\"", escape(&text))));
            }
        }
    }
}

/// Extract decisions from a markdown table: | Decision | Rationale | Impact |
fn extract_decisions(content: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig, rel_path: &str) {
    let p = &ns.prefix;
    let plan_id = plan_id_from_path(rel_path);

    for row in parse_table_rows(content) {
        if row.len() >= 2 {
            let decision = row[0].trim();
            let rationale = row[1].trim();
            let impact = if row.len() >= 3 { row[2].trim() } else { "" };

            if decision.is_empty() || decision.contains("---") {
                continue;
            }

            let slug = crate::crud::slugify(&format!("{plan_id}-{decision}"));
            let iri = format!("{}decision/{}", ns.uri, slug);

            triples.push((format!("{p}:hasDecision"), format!("<{iri}>")));
            // Decision entity triples (will be inserted under the same graph)
            triples.push((format!("ENTITY@@{iri}@@rdf:type"), format!("{p}:Decision")));
            triples.push((format!("ENTITY@@{iri}@@{p}:description"), format!("\"{}\"", escape(decision))));
            triples.push((format!("ENTITY@@{iri}@@{p}:rationale"), format!("\"{}\"", escape(rationale))));
            if !impact.is_empty() {
                triples.push((format!("ENTITY@@{iri}@@{p}:impact"), format!("\"{}\"", escape(impact))));
            }
            triples.push((format!("ENTITY@@{iri}@@{p}:fromPlan"), format!("\"{}\"", escape(&plan_id))));
        }
    }
}

/// Extract file changes from a markdown table: | File | Change | Purpose |
fn extract_file_changes(content: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig, rel_path: &str) {
    let p = &ns.prefix;
    let plan_id = plan_id_from_path(rel_path);

    for row in parse_table_rows(content) {
        if row.len() >= 2 {
            let file = row[0].trim().trim_matches('`');
            let change = row[1].trim();
            let purpose = if row.len() >= 3 { row[2].trim() } else { "" };

            if file.is_empty() || file.contains("---") || file.to_lowercase() == "file" {
                continue;
            }

            let slug = crate::crud::slugify(&format!("{plan_id}-{file}"));
            let iri = format!("{}file-change/{}", ns.uri, slug);

            triples.push((format!("{p}:hasFileChange"), format!("<{iri}>")));
            triples.push((format!("ENTITY@@{iri}@@rdf:type"), format!("{p}:FileChange")));
            triples.push((format!("ENTITY@@{iri}@@{p}:filePath"), format!("\"{}\"", escape(file))));
            triples.push((format!("ENTITY@@{iri}@@{p}:changeType"), format!("\"{}\"", escape(change))));
            if !purpose.is_empty() {
                triples.push((format!("ENTITY@@{iri}@@{p}:purpose"), format!("\"{}\"", escape(purpose))));
            }
            triples.push((format!("ENTITY@@{iri}@@{p}:fromPlan"), format!("\"{}\"", escape(&plan_id))));
        }
    }
}

/// Extract acceptance criteria results from table: | Criterion | Status | Notes |
fn extract_ac_results(content: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig, rel_path: &str) {
    let p = &ns.prefix;
    let plan_id = plan_id_from_path(rel_path);

    for row in parse_table_rows(content) {
        if row.len() >= 2 {
            let criterion = row[0].trim();
            let status = row[1].trim();
            let notes = if row.len() >= 3 { row[2].trim() } else { "" };

            if criterion.is_empty() || criterion.contains("---") || criterion.to_lowercase() == "criterion" {
                continue;
            }

            let slug = crate::crud::slugify(&format!("{plan_id}-{criterion}"));
            let iri = format!("{}ac-result/{}", ns.uri, slug);

            triples.push((format!("{p}:hasACResult"), format!("<{iri}>")));
            triples.push((format!("ENTITY@@{iri}@@rdf:type"), format!("{p}:AcceptanceCriteriaResult")));
            triples.push((format!("ENTITY@@{iri}@@{p}:criterion"), format!("\"{}\"", escape(criterion))));
            triples.push((format!("ENTITY@@{iri}@@{p}:status"), format!("\"{}\"", escape(status))));
            if !notes.is_empty() {
                triples.push((format!("ENTITY@@{iri}@@{p}:notes"), format!("\"{}\"", escape(notes))));
            }
            triples.push((format!("ENTITY@@{iri}@@{p}:fromPlan"), format!("\"{}\"", escape(&plan_id))));
        }
    }
}

/// Extract deviations as string triples.
fn extract_deviations(content: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig, _rel_path: &str) {
    let p = &ns.prefix;

    // Deviations can be tables or prose — handle both
    let rows = parse_table_rows(content);
    if !rows.is_empty() {
        for row in rows {
            if row.len() >= 2 {
                let deviation = row[0].trim();
                let reason = row[1].trim();
                if !deviation.is_empty() && !deviation.contains("---") {
                    triples.push((format!("{p}:deviation"), format!("\"{}\"", escape(&format!("{deviation}: {reason}")))));
                }
            }
        }
    } else {
        // Prose deviations — extract bullet points
        for line in content.lines() {
            let trimmed = line.trim().trim_start_matches('-').trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                triples.push((format!("{p}:deviation"), format!("\"{}\"", escape(trimmed))));
            }
        }
    }
}

/// Extract performance metrics from the Performance section.
fn extract_performance(content: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig) {
    let p = &ns.prefix;
    for line in content.lines() {
        let trimmed = line.trim().trim_start_matches('-').trim_start_matches('*').trim();
        if let Some((key, val)) = trimmed.split_once(':') {
            let key_lower = key.trim().to_lowercase();
            let val = val.trim();
            match key_lower.as_str() {
                "duration" | "actual duration" => {
                    triples.push((format!("{p}:actualDuration"), format!("\"{}\"", escape(val))));
                }
                "tasks completed" | "tasks" => {
                    triples.push((format!("{p}:tasksCompleted"), format!("\"{}\"", escape(val))));
                }
                "files modified" | "files" => {
                    triples.push((format!("{p}:filesModified"), format!("\"{}\"", escape(val))));
                }
                "git range" | "commits" => {
                    triples.push((format!("{p}:gitRange"), format!("\"{}\"", escape(val))));
                }
                _ => {}
            }
        }
    }
}

// ─── PLAN entity extraction ─────────────────────────────────

fn extract_plan_entities(body: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig, rel_path: &str) {
    let p = &ns.prefix;
    let sections = split_sections(body);

    for (heading, content) in &sections {
        let lower = heading.to_lowercase();

        // AC sections: ## AC-1: Description
        if lower.starts_with("ac-") || lower.contains("ac-") {
            let plan_id = plan_id_from_path(rel_path);
            let slug = crate::crud::slugify(&format!("{plan_id}-{heading}"));
            let iri = format!("{}acceptance-criteria/{}", ns.uri, slug);

            triples.push((format!("{p}:hasAC"), format!("<{iri}>")));
            triples.push((format!("ENTITY@@{iri}@@rdf:type"), format!("{p}:AcceptanceCriteria")));
            triples.push((format!("ENTITY@@{iri}@@{p}:name"), format!("\"{}\"", escape(heading))));

            // Extract file targets from AC content
            for line in content.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("- **File:**")
                    .or_else(|| trimmed.strip_prefix("- file:"))
                {
                    triples.push((format!("ENTITY@@{iri}@@{p}:targetFile"), format!("\"{}\"", escape(rest.trim().trim_matches('`')))));
                }
            }
        } else if lower == "goal" {
            let text = first_paragraph(content);
            if !text.is_empty() {
                triples.push((format!("{p}:goal"), format!("\"{}\"", escape(&text))));
            }
        } else if lower.contains("scope limits") {
            for line in content.lines() {
                let trimmed = line.trim().trim_start_matches('-').trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    triples.push((format!("{p}:scopeLimit"), format!("\"{}\"", escape(trimmed))));
                }
            }
        } else if lower.contains("source files") {
            for line in content.lines() {
                let trimmed = line.trim().trim_start_matches('-').trim().trim_matches('`');
                if !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed.contains('.') {
                    triples.push((format!("{p}:sourceFile"), format!("\"{}\"", escape(trimmed))));
                }
            }
        }
    }
}

// ─── Utilities ──────────────────────────────────────────────

/// Derive a plan identifier like "08-07" from a path like ".paul/phases/08-name/08-07-SUMMARY.md"
fn plan_id_from_path(rel_path: &str) -> String {
    let filename = rel_path.rsplit('/').next().unwrap_or(rel_path);
    // Strip -PLAN.md or -SUMMARY.md suffix
    let stem = filename
        .trim_end_matches(".md")
        .trim_end_matches(".MD")
        .trim_end_matches("-SUMMARY")
        .trim_end_matches("-PLAN");
    stem.to_string()
}

/// Split content into YAML frontmatter and body.
fn split_frontmatter(content: &str) -> Option<(String, String)> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return Some(("".into(), content.into()));
    }

    let after_first = &content[3..];
    let close = after_first.find("\n---")?;
    let fm = after_first[..close].to_string();
    let body = after_first[close + 4..].to_string();
    Some((fm, body))
}

/// Split markdown body into (heading_text, section_content) pairs.
fn split_sections(body: &str) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    let mut current_heading = String::new();
    let mut current_content = String::new();

    for line in body.lines() {
        if line.starts_with("## ") || line.starts_with("# ") {
            if !current_heading.is_empty() {
                sections.push((current_heading.clone(), current_content.clone()));
            }
            current_heading = line.trim_start_matches('#').trim().to_string();
            current_content.clear();
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    if !current_heading.is_empty() {
        sections.push((current_heading, current_content));
    }

    sections
}

/// Parse markdown table rows into Vec<Vec<String>> (skipping header separator rows).
fn parse_table_rows(content: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }
        // Skip separator rows (|---|---|---|)
        if trimmed.contains("---") {
            continue;
        }
        let cells: Vec<String> = trimmed
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().to_string())
            .collect();
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    // Skip the header row (first row is column names)
    if rows.len() > 1 {
        rows[1..].to_vec()
    } else {
        Vec::new()
    }
}

/// Parse inline YAML list: [a, b, c] or "a, b, c"
fn parse_inline_list(val: &str) -> Vec<String> {
    let cleaned = val.trim_matches(|c| c == '[' || c == ']');
    cleaned
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Get the first non-empty paragraph from content.
fn first_paragraph(content: &str) -> String {
    content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .take_while(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Escape special characters for SPARQL string literals.
fn escape(s: &str) -> String {
    crate::crud::escape_sparql_literal(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NamespaceConfig;

    fn ns() -> NamespaceConfig {
        NamespaceConfig {
            prefix: "ops".into(),
            uri: "http://ops-sys.local/ontology#".into(),
        }
    }

    #[test]
    fn detects_paul_artifacts() {
        assert!(is_paul_artifact(".paul/phases/08-dashboard/08-07-SUMMARY.md"));
        assert!(is_paul_artifact(".paul/phases/01-init/01-01-PLAN.md"));
        assert!(!is_paul_artifact(".paul/PROJECT.md"));
        assert!(!is_paul_artifact("src/main.rs"));
    }

    #[test]
    fn extracts_plan_id() {
        assert_eq!(plan_id_from_path(".paul/phases/08-foo/08-07-SUMMARY.md"), "08-07");
        assert_eq!(plan_id_from_path(".paul/phases/01-bar/01-01-PLAN.md"), "01-01");
    }

    #[test]
    fn parses_table_rows() {
        let table = "| Decision | Rationale | Impact |\n|---|---|---|\n| Use overlay | Better UX | Reusable |\n| Add status | User request | New field |\n";
        let rows = parse_table_rows(table);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], "Use overlay");
        assert_eq!(rows[1][1], "User request");
    }

    #[test]
    fn extracts_summary_decisions() {
        let content = "---\nphase: 08-dashboard\nplan: 07\n---\n\n## Decisions Made\n\n| Decision | Rationale | Impact |\n|---|---|---|\n| Use overlay | Better UX | Reusable |\n\n## Next Phase Readiness\n\nReady.\n";
        let triples = extract(content, ".paul/phases/08-dashboard/08-07-SUMMARY.md", &ns()).unwrap();

        assert!(triples.iter().any(|(p, _)| p.contains("hasDecision")));
        assert!(triples.iter().any(|(p, v)| p.contains("ENTITY@@") && v.contains("Decision")));
    }

    #[test]
    fn extracts_inline_list() {
        let vals = parse_inline_list("[\"08-06\", \"08-05\"]");
        assert_eq!(vals, vec!["08-06", "08-05"]);

        let vals2 = parse_inline_list("[svelte, axum, crud]");
        assert_eq!(vals2, vec!["svelte", "axum", "crud"]);
    }
}
