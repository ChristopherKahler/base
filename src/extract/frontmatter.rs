use crate::config::NamespaceConfig;

/// Extract YAML frontmatter and markdown body content into (predicate, value) triples.
/// Returns None if no valid frontmatter found.
/// Extract with an optional project slug override for .paul/ docs
/// whose path starts at .paul/ (no parent directory to derive from).
pub fn extract_with_project(content: &str, file_path: &str, ns: &NamespaceConfig, project_hint: Option<&str>) -> Option<Vec<(String, String)>> {
    let fm = parse_frontmatter(content)?;
    if fm.is_empty() {
        return None;
    }

    // Normalized here rather than relying on the caller, so this function is
    // correct for any path it is handed — including the `/`-splitting below,
    // which on Windows would otherwise fail to split at all and name the
    // document `service\CONFIG-KEYS` instead of `CONFIG-KEYS`.
    let file_path = &crate::crud::normalize_path_sep(file_path);

    let p = &ns.prefix;
    let mut triples = Vec::new();

    // Always: type and path
    triples.push(("rdf:type".into(), format!("{p}:Document")));
    triples.push((format!("{p}:path"), format!("\"{}\"", escape(file_path))));

    // Map frontmatter fields to predicates
    for (key, value) in &fm {
        match key.to_lowercase().as_str() {
            "name" | "title" => {
                triples.push((format!("{p}:name"), format!("\"{}\"", escape(value))));
            }
            "description" | "about" | "summary" => {
                triples.push((format!("{p}:description"), format!("\"{}\"", escape(value))));
            }
            "status" => {
                triples.push((format!("{p}:status"), format!("\"{}\"", escape(value))));
            }
            "priority" => {
                triples.push((format!("{p}:priority"), format!("\"{}\"", escape(value))));
            }
            "type" => {
                triples.push((format!("{p}:documentType"), format!("\"{}\"", escape(value))));
            }
            "tags" => {
                for tag in parse_list(value) {
                    triples.push((format!("{p}:hasTag"), format!("\"{}\"", escape(&tag))));
                }
            }
            "relatedto" => {
                for entity in parse_list(value) {
                    let entity_slug = entity
                        .replace(['/', '\\', '.', ' '], "-")
                        .to_lowercase();
                    triples.push((
                        format!("{p}:relatedTo"),
                        format!("<{}entity/{}>", ns.uri, entity_slug),
                    ));
                }
            }
            _ => {
                triples.push((
                    format!("{p}:description"),
                    format!("\"{}: {}\"", escape(key), escape(value)),
                ));
            }
        }
    }

    // If no name was set from frontmatter, derive from filename
    if !fm.iter().any(|(k, _)| k == "name" || k == "title") {
        let filename = file_path
            .rsplit('/')
            .next()
            .unwrap_or(file_path)
            .trim_end_matches(".md");
        triples.push((format!("{p}:name"), format!("\"{}\"", filename)));
    }

    // Extract body content (headings, links, wikilinks, @-mentions)
    if let Some(body) = body_after_frontmatter(content) {
        extract_body(&body, &mut triples, ns);
    }

    // Auto-link .paul/ documents to their project
    if file_path.contains(".paul/") {
        // Derive project slug from path before .paul/, or use project_hint from cwd
        let path_derived = file_path
            .split(".paul/")
            .next()
            .unwrap_or("")
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .replace([' ', '.'], "-")
            .to_lowercase();

        let project_slug = if !path_derived.is_empty() {
            path_derived
        } else if let Some(hint) = project_hint {
            hint.to_lowercase().replace([' ', '.'], "-")
        } else {
            "unknown".to_string()
        };

        if !project_slug.is_empty() && project_slug != "unknown" {
            triples.push((
                format!("{p}:relatedTo"),
                format!("<{}project/{}>", ns.uri, project_slug),
            ));
        }

        // Link PLAN ↔ SUMMARY pairs in same phase directory
        if file_path.ends_with("-PLAN.md") {
            let summary_path = file_path.replace("-PLAN.md", "-SUMMARY.md");
            let summary_slug = summary_path
                .replace(['/', '\\', '.'], "-")
                .to_lowercase();
            triples.push((
                format!("{p}:relatedTo"),
                format!("<{}document/{}>", ns.uri, summary_slug),
            ));
        } else if file_path.ends_with("-SUMMARY.md") {
            let plan_path = file_path.replace("-SUMMARY.md", "-PLAN.md");
            let plan_slug = plan_path
                .replace(['/', '\\', '.'], "-")
                .to_lowercase();
            triples.push((
                format!("{p}:relatedTo"),
                format!("<{}document/{}>", ns.uri, plan_slug),
            ));
        }
    }

    Some(triples)
}

/// Parse YAML frontmatter between --- delimiters.
/// Returns key-value pairs. Handles simple `key: value` lines only.
pub fn parse_frontmatter(content: &str) -> Option<Vec<(String, String)>> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }

    let after_first = &trimmed[3..];
    let end_pos = after_first.find("\n---")?;
    let fm_text = &after_first[..end_pos];

    let mut pairs = Vec::new();
    for line in fm_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_string();
            let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
            if !key.is_empty() && !value.is_empty() {
                pairs.push((key, value));
            }
        }
    }

    if pairs.is_empty() {
        None
    } else {
        Some(pairs)
    }
}

/// Parse a comma-separated list value, handling optional bracket syntax.
/// "rust, sparql, hooks" → ["rust", "sparql", "hooks"]
/// "[rust, sparql, hooks]" → ["rust", "sparql", "hooks"]
fn parse_list(value: &str) -> Vec<String> {
    let trimmed = value.trim().trim_start_matches('[').trim_end_matches(']');
    trimmed
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Get the markdown body content after the frontmatter closing delimiter.
pub fn body_after_frontmatter(content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_first = &trimmed[3..];
    let end_pos = after_first.find("\n---")?;
    let after_fm = &after_first[end_pos + 4..]; // skip past \n---
    let body = after_fm
        .trim_start_matches('\n')
        .trim_start_matches('\r');
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// Extract structured content from markdown body: headings, links, wikilinks, @-mentions.
fn extract_body(body: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig) {
    let p = &ns.prefix;
    let mut in_code_block = false;

    for line in body.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }

        // Headings → hasSection
        if let Some(heading) = parse_heading(trimmed) {
            triples.push((format!("{p}:hasSection"), format!("\"{}\"", escape(&heading))));
        }

        extract_markdown_links(trimmed, triples, ns);
        extract_wikilinks(trimmed, triples, ns);
        extract_at_mentions(trimmed, triples, ns);
    }
}

/// Parse a markdown heading line into its text content.
fn parse_heading(line: &str) -> Option<String> {
    if !line.starts_with('#') {
        return None;
    }
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes > 6 {
        return None;
    }
    let rest = line[hashes..].trim();
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

/// Extract `[text](path)` markdown links, creating references to document IRIs.
/// Only extracts local file paths, not URLs.
fn extract_markdown_links(line: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig) {
    let p = &ns.prefix;
    let mut remaining = line;

    while let Some(bracket_start) = remaining.find('[') {
        // Skip image links ![alt](src)
        if bracket_start > 0 && remaining.as_bytes()[bracket_start - 1] == b'!' {
            remaining = &remaining[bracket_start + 1..];
            continue;
        }

        let after_bracket = &remaining[bracket_start + 1..];
        let Some(bracket_end) = after_bracket.find(']') else {
            break;
        };

        let after_close = &after_bracket[bracket_end + 1..];
        if !after_close.starts_with('(') {
            remaining = &after_bracket[bracket_end + 1..];
            continue;
        }

        let paren_content = &after_close[1..];
        let Some(paren_end) = paren_content.find(')') else {
            break;
        };

        let path = paren_content[..paren_end].trim();

        if is_local_path(path) {
            let clean = path.trim_start_matches("./");
            let slug = crate::crud::slugify(clean);
            triples.push((
                format!("{p}:references"),
                format!("<{}document/{}>", ns.uri, slug),
            ));
        }

        remaining = &paren_content[paren_end + 1..];
    }
}

/// Extract `[[wikilink]]` references, creating entity edges.
fn extract_wikilinks(line: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig) {
    let p = &ns.prefix;
    let mut remaining = line;

    while let Some(start) = remaining.find("[[") {
        let after = &remaining[start + 2..];
        let Some(end) = after.find("]]") else {
            break;
        };

        let link = after[..end].trim();
        if !link.is_empty() {
            let slug = crate::crud::slugify(link);
            triples.push((
                format!("{p}:references"),
                format!("<{}entity/{}>", ns.uri, slug),
            ));
        }

        remaining = &after[end + 2..];
    }
}

/// Extract `@path/to/file` references, creating document edges.
/// Only matches paths (must contain `/` or `.`). Ignores emails.
fn extract_at_mentions(line: &str, triples: &mut Vec<(String, String)>, ns: &NamespaceConfig) {
    let p = &ns.prefix;
    let mut remaining = line;

    while let Some(at_pos) = remaining.find('@') {
        // Must be at start of string or preceded by whitespace
        if at_pos > 0 {
            let prev = remaining.as_bytes()[at_pos - 1];
            if prev != b' ' && prev != b'\t' {
                remaining = &remaining[at_pos + 1..];
                continue;
            }
        }

        let after_at = &remaining[at_pos + 1..];

        // Collect path characters
        let path_len = after_at
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '/' || *c == '.' || *c == '-' || *c == '_' || *c == '~')
            .count();

        if path_len == 0 {
            remaining = after_at;
            continue;
        }

        let path = &after_at[..path_len];

        // Must look like a file path (contains / or .)
        if path.contains('/') || path.contains('.') {
            let clean = path
                .trim_start_matches("~/")
                .trim_start_matches("./")
                .trim_start_matches('~');
            let slug = crate::crud::slugify(clean);
            triples.push((
                format!("{p}:references"),
                format!("<{}document/{}>", ns.uri, slug),
            ));
        }

        remaining = &after_at[path_len..];
    }
}

fn is_local_path(path: &str) -> bool {
    !path.starts_with("http://")
        && !path.starts_with("https://")
        && !path.starts_with("mailto:")
        && !path.starts_with('#')
        && !path.is_empty()
}

/// Escape special characters for SPARQL string literals.
fn escape(s: &str) -> String {
    crate::crud::escape_sparql_literal(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns() -> NamespaceConfig {
        NamespaceConfig::default()
    }

    // --- Existing tests (preserved) ---

    #[test]
    fn parse_simple_frontmatter() {
        let content = "---\ntitle: My Doc\nstatus: active\n---\n\n# Content";
        let triples = extract_with_project(content, "docs/test.md", &ns(), None).unwrap();
        assert!(triples.iter().any(|(p, v)| p.contains("name") && v.contains("My Doc")));
        assert!(triples.iter().any(|(p, v)| p.contains("status") && v.contains("active")));
        assert!(triples.iter().any(|(p, v)| p.contains("path") && v.contains("docs/test.md")));
    }

    #[test]
    fn no_frontmatter_returns_none() {
        let content = "# Just a heading\n\nSome text";
        assert!(extract_with_project(content, "test.md", &ns(), None).is_none());
    }

    #[test]
    fn empty_frontmatter_returns_none() {
        let content = "---\n---\n\nContent";
        assert!(extract_with_project(content, "test.md", &ns(), None).is_none());
    }

    // --- Tags: individual triples ---

    #[test]
    fn tags_emit_individual_triples() {
        let content = "---\ntitle: Tagged\ntags: rust, sparql, hooks\n---\n";
        let triples = extract_with_project(content, "test.md", &ns(), None).unwrap();
        let tags: Vec<_> = triples
            .iter()
            .filter(|(p, _)| p.contains("hasTag"))
            .collect();
        assert_eq!(tags.len(), 3);
        assert!(tags.iter().any(|(_, v)| v.contains("rust")));
        assert!(tags.iter().any(|(_, v)| v.contains("sparql")));
        assert!(tags.iter().any(|(_, v)| v.contains("hooks")));
    }

    #[test]
    fn tags_bracket_syntax() {
        let content = "---\ntitle: Tagged\ntags: [alpha, beta]\n---\n";
        let triples = extract_with_project(content, "test.md", &ns(), None).unwrap();
        let tags: Vec<_> = triples
            .iter()
            .filter(|(p, _)| p.contains("hasTag"))
            .collect();
        assert_eq!(tags.len(), 2);
        assert!(tags.iter().any(|(_, v)| v.contains("alpha")));
        assert!(tags.iter().any(|(_, v)| v.contains("beta")));
    }

    // --- relatedTo: entity edges ---

    #[test]
    fn related_to_creates_entity_edges() {
        let content = "---\ntitle: Plan\nrelatedTo: [signal-mod, hook-engine]\n---\n";
        let n = ns();
        let triples = extract_with_project(content, "test.md", &n, None).unwrap();
        let edges: Vec<_> = triples
            .iter()
            .filter(|(p, _)| p.contains("relatedTo"))
            .collect();
        assert_eq!(edges.len(), 2);
        assert!(edges
            .iter()
            .any(|(_, v)| v.contains(&format!("{}entity/signal-mod", n.uri))));
        assert!(edges
            .iter()
            .any(|(_, v)| v.contains(&format!("{}entity/hook-engine", n.uri))));
    }

    // --- documentType ---

    #[test]
    fn type_field_maps_to_document_type() {
        let content = "---\ntitle: Decision Log\ntype: decision\n---\n";
        let triples = extract_with_project(content, "test.md", &ns(), None).unwrap();
        assert!(triples
            .iter()
            .any(|(p, v)| p.contains("documentType") && v.contains("decision")));
    }

    // --- Body: headings ---

    #[test]
    fn headings_extracted_as_sections() {
        let content = "---\ntitle: Doc\n---\n\n# Top Level\n\nText.\n\n## Sub Section\n\nMore text.\n\n### Deep\n";
        let triples = extract_with_project(content, "test.md", &ns(), None).unwrap();
        let sections: Vec<_> = triples
            .iter()
            .filter(|(p, _)| p.contains("hasSection"))
            .collect();
        assert_eq!(sections.len(), 3);
        assert!(sections.iter().any(|(_, v)| v.contains("Top Level")));
        assert!(sections.iter().any(|(_, v)| v.contains("Sub Section")));
        assert!(sections.iter().any(|(_, v)| v.contains("Deep")));
    }

    #[test]
    fn headings_in_code_blocks_ignored() {
        let content = "---\ntitle: Doc\n---\n\n# Real Heading\n\n```python\n# Comment not heading\n```\n\n## Also Real\n";
        let triples = extract_with_project(content, "test.md", &ns(), None).unwrap();
        let sections: Vec<_> = triples
            .iter()
            .filter(|(p, _)| p.contains("hasSection"))
            .collect();
        assert_eq!(sections.len(), 2);
        assert!(sections.iter().any(|(_, v)| v.contains("Real Heading")));
        assert!(sections.iter().any(|(_, v)| v.contains("Also Real")));
    }

    // --- Body: markdown links ---

    #[test]
    fn markdown_links_create_references() {
        let content =
            "---\ntitle: Doc\n---\n\nSee [the plan](docs/PLAN.md) and [config](src/config.rs).\n";
        let n = ns();
        let triples = extract_with_project(content, "test.md", &n, None).unwrap();
        let refs: Vec<_> = triples
            .iter()
            .filter(|(p, _)| p.contains("references"))
            .collect();
        assert_eq!(refs.len(), 2);
        assert!(refs
            .iter()
            .any(|(_, v)| v.contains(&format!("{}document/docs-plan-md", n.uri))));
        assert!(refs
            .iter()
            .any(|(_, v)| v.contains(&format!("{}document/src-config-rs", n.uri))));
    }

    #[test]
    fn external_links_ignored() {
        let content =
            "---\ntitle: Doc\n---\n\n[Google](https://google.com) and [local](readme.md).\n";
        let triples = extract_with_project(content, "test.md", &ns(), None).unwrap();
        let refs: Vec<_> = triples
            .iter()
            .filter(|(p, _)| p.contains("references"))
            .collect();
        assert_eq!(refs.len(), 1);
        assert!(refs.iter().any(|(_, v)| v.contains("readme-md")));
    }

    // --- Body: wikilinks ---

    #[test]
    fn wikilinks_create_entity_references() {
        let content = "---\ntitle: Doc\n---\n\nRelated to [[signal-engine]] and [[hook system]].\n";
        let n = ns();
        let triples = extract_with_project(content, "test.md", &n, None).unwrap();
        let refs: Vec<_> = triples
            .iter()
            .filter(|(p, v)| p.contains("references") && v.contains("entity/"))
            .collect();
        assert_eq!(refs.len(), 2);
        assert!(refs
            .iter()
            .any(|(_, v)| v.contains(&format!("{}entity/signal-engine", n.uri))));
        assert!(refs
            .iter()
            .any(|(_, v)| v.contains(&format!("{}entity/hook-system", n.uri))));
    }

    // --- Body: @-mentions ---

    #[test]
    fn at_mentions_create_document_references() {
        let content =
            "---\ntitle: Doc\n---\n\nSee @.paul/STATE.md and also @src/extract/mod.rs for details.\n";
        let triples = extract_with_project(content, "test.md", &ns(), None).unwrap();
        let refs: Vec<_> = triples
            .iter()
            .filter(|(p, v)| p.contains("references") && v.contains("document/"))
            .collect();
        assert!(refs.len() >= 2);
    }

    #[test]
    fn email_addresses_not_matched_as_mentions() {
        let content = "---\ntitle: Doc\n---\n\nContact user@email.com for info.\n";
        let triples = extract_with_project(content, "test.md", &ns(), None).unwrap();
        let refs: Vec<_> = triples
            .iter()
            .filter(|(p, _)| p.contains("references"))
            .collect();
        assert_eq!(refs.len(), 0);
    }

    // --- parse_list ---

    #[test]
    fn parse_list_comma_separated() {
        assert_eq!(parse_list("a, b, c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_list_bracket_syntax() {
        assert_eq!(parse_list("[x, y]"), vec!["x", "y"]);
    }

    #[test]
    fn parse_list_single_value() {
        assert_eq!(parse_list("solo"), vec!["solo"]);
    }

    // --- Combined: frontmatter + body ---

    #[test]
    fn full_document_extraction() {
        let content = "\
---
title: Architecture Decision
type: decision
status: active
tags: [rust, graph, hooks]
relatedTo: [signal-engine, config-module]
---

# Overview

The [[signal-engine]] drives all hook behavior.

## Implementation

See [the config](src/config.rs) for namespace setup.
Reference @src/extract/mod.rs for the sync pipeline.

```rust
// This heading should be ignored
# not a heading
```

## Testing

Run tests with `cargo test`.
";
        let n = ns();
        let triples = extract_with_project(content, "docs/arch.md", &n, None).unwrap();

        // Frontmatter
        assert!(triples.iter().any(|(p, v)| p.contains("name") && v.contains("Architecture Decision")));
        assert!(triples.iter().any(|(p, v)| p.contains("documentType") && v.contains("decision")));
        assert!(triples.iter().any(|(p, v)| p.contains("status") && v.contains("active")));

        // Tags (3 individual)
        let tags: Vec<_> = triples.iter().filter(|(p, _)| p.contains("hasTag")).collect();
        assert_eq!(tags.len(), 3);

        // relatedTo (2 entity edges)
        let related: Vec<_> = triples.iter().filter(|(p, _)| p.contains("relatedTo")).collect();
        assert_eq!(related.len(), 2);

        // Sections (3: Overview, Implementation, Testing — code block heading excluded)
        let sections: Vec<_> = triples.iter().filter(|(p, _)| p.contains("hasSection")).collect();
        assert_eq!(sections.len(), 3);

        // References (wikilink + markdown link + @-mention)
        let refs: Vec<_> = triples.iter().filter(|(p, _)| p.contains("references")).collect();
        assert!(refs.len() >= 3);
    }
}
