use std::path::Path;

use crate::config::BaseConfig;
use crate::crud;
use crate::extract::frontmatter;

/// Check if a path is a Claude memory file: ~/.claude/projects/*/memory/*.md
pub fn is_memory_path(path: &Path) -> bool {
    let s = path.to_str().unwrap_or("");
    s.contains("/.claude/projects/") && s.contains("/memory/") && s.ends_with(".md")
}

/// Check if the file is the MEMORY.md index (not an individual memory file).
pub fn is_memory_index(path: &Path) -> bool {
    path.file_name()
        .and_then(|f| f.to_str()) == Some("MEMORY.md")
}

/// Parse Claude's memory file content into (name, description, note_type, body).
/// Falls back gracefully when frontmatter is malformed.
pub fn parse_memory_content(content: &str) -> (String, String, String, String) {
    let pairs = frontmatter::parse_frontmatter(content);
    let body = frontmatter::body_after_frontmatter(content)
        .unwrap_or_else(|| content.to_string());

    let (name, description, note_type) = match pairs {
        Some(ref fm) => {
            let name = fm.iter()
                .find(|(k, _)| k == "name")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let desc = fm.iter()
                .find(|(k, _)| k == "description")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let ntype = fm.iter()
                .find(|(k, _)| k == "type")
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| "insight".into());
            (name, desc, ntype)
        }
        None => (String::new(), String::new(), "insight".into()),
    };

    (name, description, note_type, body)
}

/// Infer a project slug from Claude's memory path.
/// Path pattern: ~/.claude/projects/-home-user-workspace-apps-{project}/memory/slug.md
/// Extracts the last meaningful segment from the project hash directory.
pub fn infer_project_from_memory_path(file_path: &str) -> Option<String> {
    // Split on /memory/ to get the directory part
    let dir_part = file_path.split("/memory/").next()?;
    // Get the last path component (the project hash dir)
    let hash_dir = dir_part.rsplit('/').next()?;
    // The hash dir looks like: -home-chriskahler-chris-ai-systems-apps-base-v2
    // Take the last segment after splitting on '-'
    // But dashes are also used in slugs, so take everything after the last known path separator pattern
    // Strategy: split on common path markers (apps-, clients-)
    if let Some(after_apps) = hash_dir.rsplit_once("-apps-") {
        return Some(after_apps.1.to_string());
    }
    if let Some(after_clients) = hash_dir.rsplit_once("-clients-") {
        return Some(after_clients.1.to_string());
    }
    // Fallback: take the last segment
    hash_dir.rsplit('-').next().map(String::from)
}

/// Map Claude memory type to BASE note type.
pub fn map_memory_type(claude_type: &str) -> &str {
    match claude_type {
        "feedback" => "correction",
        "user" | "project" | "reference" => "insight",
        _ => "insight",
    }
}

/// Handle a memory write intercept (Write/Edit to a memory file).
/// Returns Some((message, blocked)) or None (no intercept / fail-open).
fn memory_write_intercept(
    config: &BaseConfig,
    cwd: &Path,
    event: &serde_json::Value,
) -> Option<(String, bool)> {
    let tool_name = event.get("tool_name").and_then(|v| v.as_str())?;
    if tool_name != "Write" && tool_name != "Edit" {
        return None;
    }

    let file_path = event.get("tool_input")?.get("file_path")?.as_str()?;
    let path = Path::new(file_path);

    if !is_memory_path(path) {
        return None;
    }

    // MEMORY.md index writes: block only in "base" mode (where session-start injection replaces it).
    // In "both" mode, let the flat-file index update proceed — session-start injection comes in Plan 02.
    if is_memory_index(path) {
        if config.memory.mode == "base" {
            return Some((
                "MEMORY.md is managed by BASE. Memory index is injected at session start from the graph.".into(),
                true,
            ));
        }
        return None; // "both" mode: let MEMORY.md write proceed
    }

    // Get content from Write (full content) or Edit (new_string as content to learn)
    let content = if tool_name == "Write" {
        event.get("tool_input")?.get("content")?.as_str()?.to_string()
    } else {
        event.get("tool_input")?.get("new_string")?.as_str()?.to_string()
    };

    let (_name, description, note_type, body) = parse_memory_content(&content);

    let full_text = if description.is_empty() {
        body
    } else {
        format!("{description}\n\n{body}")
    };

    let base_type = map_memory_type(&note_type);
    let domain = "MEMORY";
    let project = infer_project_from_memory_path(file_path);
    let blocked = config.memory.mode == "base";

    match crud::note::learn(
        cwd,
        &config.namespace,
        &full_text,
        base_type,
        Some(domain),
        project.as_deref(),
        None,
    ) {
        Ok(slug) => {
            let mode_note = if blocked { "flat file BLOCKED" } else { "flat file also written" };
            Some((
                format!(
                    "<memory-intercept>\nStored in BASE graph: note/{slug} (domain: {domain}, type: {base_type}, {mode_note})\n</memory-intercept>"
                ),
                blocked,
            ))
        }
        Err(e) => {
            eprintln!("base: memory intercept learn failed: {e}");
            None // Fail open
        }
    }
}

/// Handle a memory read intercept (Read of a memory file).
/// Returns Some((message, blocked)) or None (no intercept).
fn memory_read_intercept(
    config: &BaseConfig,
    cwd: &Path,
    event: &serde_json::Value,
) -> Option<(String, bool)> {
    let tool_name = event.get("tool_name").and_then(|v| v.as_str())?;
    if tool_name != "Read" {
        return None;
    }

    let file_path = event.get("tool_input")?.get("file_path")?.as_str()?;
    let path = Path::new(file_path);

    if !is_memory_path(path) {
        return None;
    }

    // MEMORY.md reads: session-start handles index injection (Plan 02)
    if is_memory_index(path) {
        return None;
    }

    let slug = path.file_stem()?.to_str()?;
    let blocked = config.memory.mode == "base";

    // Convert slug back to search-friendly text (hyphens → spaces)
    let keyword = slug.replace('-', " ");
    let results = crud::note::recall_to_string(cwd, &config.namespace, Some(&keyword), None);
    if results.is_empty() {
        return None;
    }

    Some((
        format!(
            "<base-memory-context>\n\
             Graph recall for \"{slug}\":\n\
             {results}\
             </base-memory-context>"
        ),
        blocked,
    ))
}

/// Public entry point: check memory config, try write then read intercept.
/// Returns Some((message, blocked)) if a memory intercept fired, None otherwise.
pub fn handle_memory(
    config: &BaseConfig,
    cwd: &Path,
    event: &serde_json::Value,
) -> Option<(String, bool)> {
    if !config.memory.enabled || config.memory.mode == "claude" {
        return None;
    }

    memory_write_intercept(config, cwd, event)
        .or_else(|| memory_read_intercept(config, cwd, event))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_memory_path_valid() {
        let p = Path::new("/home/user/.claude/projects/-home-user-workspace/memory/test-note.md");
        assert!(is_memory_path(p));
    }

    #[test]
    fn test_is_memory_path_invalid_no_memory_dir() {
        let p = Path::new("/home/user/.claude/projects/-home-user-workspace/test-note.md");
        assert!(!is_memory_path(p));
    }

    #[test]
    fn test_is_memory_path_invalid_not_claude() {
        let p = Path::new("/home/user/projects/memory/note.md");
        assert!(!is_memory_path(p));
    }

    #[test]
    fn test_is_memory_path_invalid_not_md() {
        let p = Path::new("/home/user/.claude/projects/-home-user/memory/note.txt");
        assert!(!is_memory_path(p));
    }

    #[test]
    fn test_is_memory_index() {
        let p = Path::new("/home/user/.claude/projects/-home-user/memory/MEMORY.md");
        assert!(is_memory_index(p));
    }

    #[test]
    fn test_is_memory_index_false() {
        let p = Path::new("/home/user/.claude/projects/-home-user/memory/some-note.md");
        assert!(!is_memory_index(p));
    }

    #[test]
    fn test_parse_memory_content_with_frontmatter() {
        let content = "\
---
name: feedback_terse-responses
description: Chris asked for terse responses
metadata:
  type: feedback
---

Chris said directly: quit being so long winded.

**Why:** Efficiency.

**How to apply:**
- Be brief.
";
        let (name, desc, ntype, body) = parse_memory_content(content);
        assert_eq!(name, "feedback_terse-responses");
        assert_eq!(desc, "Chris asked for terse responses");
        assert_eq!(ntype, "feedback");
        assert!(body.contains("Chris said directly"));
    }

    #[test]
    fn test_parse_memory_content_malformed() {
        let content = "Just some text without frontmatter.";
        let (name, desc, ntype, body) = parse_memory_content(content);
        assert!(name.is_empty());
        assert!(desc.is_empty());
        assert_eq!(ntype, "insight");
        assert_eq!(body, "Just some text without frontmatter.");
    }

    #[test]
    fn test_infer_project_from_memory_path_apps() {
        let path = "/home/user/.claude/projects/-home-user-chris-ai-systems-apps-base-v2/memory/note.md";
        assert_eq!(infer_project_from_memory_path(path), Some("base-v2".into()));
    }

    #[test]
    fn test_infer_project_from_memory_path_clients() {
        let path = "/home/user/.claude/projects/-home-user-chris-ai-systems-clients-acme/memory/note.md";
        assert_eq!(infer_project_from_memory_path(path), Some("acme".into()));
    }

    #[test]
    fn test_infer_project_from_memory_path_fallback() {
        let path = "/home/user/.claude/projects/-home-user-my-project/memory/note.md";
        let result = infer_project_from_memory_path(path);
        assert!(result.is_some());
    }

    #[test]
    fn test_map_memory_type() {
        assert_eq!(map_memory_type("feedback"), "correction");
        assert_eq!(map_memory_type("user"), "insight");
        assert_eq!(map_memory_type("project"), "insight");
        assert_eq!(map_memory_type("reference"), "insight");
        assert_eq!(map_memory_type("unknown"), "insight");
    }

    #[test]
    fn test_handle_memory_disabled() {
        let config = BaseConfig::default(); // memory.enabled = false
        let cwd = Path::new("/tmp");
        let event = serde_json::json!({
            "tool_name": "Write",
            "tool_input": {
                "file_path": "/home/user/.claude/projects/-test/memory/note.md",
                "content": "test"
            }
        });
        assert!(handle_memory(&config, cwd, &event).is_none());
    }

    #[test]
    fn test_handle_memory_claude_mode() {
        let mut config = BaseConfig::default();
        config.memory.enabled = true;
        config.memory.mode = "claude".into();
        let cwd = Path::new("/tmp");
        let event = serde_json::json!({
            "tool_name": "Write",
            "tool_input": {
                "file_path": "/home/user/.claude/projects/-test/memory/note.md",
                "content": "test"
            }
        });
        assert!(handle_memory(&config, cwd, &event).is_none());
    }

    #[test]
    fn test_handle_memory_non_memory_path() {
        let mut config = BaseConfig::default();
        config.memory.enabled = true;
        config.memory.mode = "both".into();
        let cwd = Path::new("/tmp");
        let event = serde_json::json!({
            "tool_name": "Write",
            "tool_input": {
                "file_path": "/home/user/src/main.rs",
                "content": "fn main() {}"
            }
        });
        assert!(handle_memory(&config, cwd, &event).is_none());
    }

    #[test]
    fn test_handle_memory_index_passthrough_both_mode() {
        let mut config = BaseConfig::default();
        config.memory.enabled = true;
        config.memory.mode = "both".into();
        let cwd = Path::new("/tmp");
        let event = serde_json::json!({
            "tool_name": "Write",
            "tool_input": {
                "file_path": "/home/user/.claude/projects/-test/memory/MEMORY.md",
                "content": "index content"
            }
        });
        let result = handle_memory(&config, cwd, &event);
        assert!(result.is_none(), "MEMORY.md should pass through in both mode");
    }

    #[test]
    fn test_handle_memory_index_blocked_base_mode() {
        let mut config = BaseConfig::default();
        config.memory.enabled = true;
        config.memory.mode = "base".into();
        let cwd = Path::new("/tmp");
        let event = serde_json::json!({
            "tool_name": "Write",
            "tool_input": {
                "file_path": "/home/user/.claude/projects/-test/memory/MEMORY.md",
                "content": "index content"
            }
        });
        let result = handle_memory(&config, cwd, &event);
        assert!(result.is_some());
        let (msg, blocked) = result.unwrap();
        assert!(msg.contains("managed by BASE"));
        assert!(blocked);
    }
}
