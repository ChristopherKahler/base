use std::path::Path;

fn setup_workspace(dir: &Path) {
    let base_dir = dir.join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(
        base_dir.join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n\n[memory]\nenabled = true\nmode = \"both\"\n",
    )
    .unwrap();
    std::fs::write(
        base_dir.join("domains.toml"),
        "[[domain]]\nname = \"MEMORY\"\nmode = \"triggered\"\nprompt_keywords = [\"memory\"]\nrules = [\"Memory domain\"]\n",
    )
    .unwrap();
    let config = base::config::BaseConfig::load(dir);
    base::domain::sync::sync_domains_to_graph(&config, dir, None).unwrap();
}

fn setup_workspace_disabled(dir: &Path) {
    let base_dir = dir.join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(
        base_dir.join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n\n[memory]\nenabled = false\n",
    )
    .unwrap();
}

fn setup_workspace_claude_mode(dir: &Path) {
    let base_dir = dir.join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(
        base_dir.join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n\n[memory]\nenabled = true\nmode = \"claude\"\n\n[signal]\nenabled = false\n",
    )
    .unwrap();
}

#[test]
fn memory_write_intercept_stores_to_graph() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace(tmp.path());
    let config = base::config::BaseConfig::load(tmp.path());

    let event = serde_json::json!({
        "tool_name": "Write",
        "tool_input": {
            "file_path": "/home/user/.claude/projects/-home-user-chris-ai-systems-apps-base-v2/memory/feedback_terse.md",
            "content": "---\nname: feedback_terse\ndescription: Chris wants terse responses\nmetadata:\n  type: feedback\n---\n\nChris said: be brief.\n\n**Why:** Efficiency.\n\n**How to apply:**\n- Short sentences.\n"
        }
    });

    let result = base::hook::memory::handle_memory(&config, tmp.path(), &event);
    assert!(result.is_some(), "Should intercept memory write");

    let (msg, blocked) = result.unwrap();
    assert!(msg.contains("Stored in BASE graph"), "Message: {msg}");
    assert!(msg.contains("correction"), "feedback type should map to correction: {msg}");
    assert!(!blocked, "mode=both should not block");

    // Verify it's actually in the graph
    let ns = &config.namespace;
    let recall_output = base::crud::note::recall_to_string(tmp.path(), ns, Some("brief"), None);
    assert!(!recall_output.is_empty(), "Should find the note via recall");
}

#[test]
fn memory_write_intercept_disabled_passes_through() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace_disabled(tmp.path());
    let config = base::config::BaseConfig::load(tmp.path());

    let event = serde_json::json!({
        "tool_name": "Write",
        "tool_input": {
            "file_path": "/home/user/.claude/projects/-test/memory/note.md",
            "content": "---\nname: test\ndescription: test\nmetadata:\n  type: user\n---\n\nTest note.\n"
        }
    });

    let result = base::hook::memory::handle_memory(&config, tmp.path(), &event);
    assert!(result.is_none(), "Disabled memory should not intercept");
}

#[test]
fn memory_write_intercept_claude_mode_passes_through() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace_claude_mode(tmp.path());
    let config = base::config::BaseConfig::load(tmp.path());

    let event = serde_json::json!({
        "tool_name": "Write",
        "tool_input": {
            "file_path": "/home/user/.claude/projects/-test/memory/note.md",
            "content": "---\nname: test\ndescription: test\nmetadata:\n  type: user\n---\n\nTest note.\n"
        }
    });

    let result = base::hook::memory::handle_memory(&config, tmp.path(), &event);
    assert!(result.is_none(), "Claude mode should not intercept");
}

#[test]
fn non_memory_write_not_intercepted() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace(tmp.path());
    let config = base::config::BaseConfig::load(tmp.path());

    let event = serde_json::json!({
        "tool_name": "Write",
        "tool_input": {
            "file_path": "/home/user/src/main.rs",
            "content": "fn main() {}"
        }
    });

    let result = base::hook::memory::handle_memory(&config, tmp.path(), &event);
    assert!(result.is_none(), "Non-memory path should not intercept");
}

#[test]
fn memory_index_write_passthrough_in_both_mode() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace(tmp.path()); // mode = "both"
    let config = base::config::BaseConfig::load(tmp.path());

    let event = serde_json::json!({
        "tool_name": "Write",
        "tool_input": {
            "file_path": "/home/user/.claude/projects/-test/memory/MEMORY.md",
            "content": "- [Note](file.md) — hook"
        }
    });

    let result = base::hook::memory::handle_memory(&config, tmp.path(), &event);
    assert!(result.is_none(), "MEMORY.md write should pass through in 'both' mode");
}

#[test]
fn memory_index_write_blocked_in_base_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(
        base_dir.join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n\n[memory]\nenabled = true\nmode = \"base\"\n",
    ).unwrap();
    let config = base::config::BaseConfig::load(tmp.path());

    let event = serde_json::json!({
        "tool_name": "Write",
        "tool_input": {
            "file_path": "/home/user/.claude/projects/-test/memory/MEMORY.md",
            "content": "- [Note](file.md) — hook"
        }
    });

    let result = base::hook::memory::handle_memory(&config, tmp.path(), &event);
    assert!(result.is_some());
    let (msg, blocked) = result.unwrap();
    assert!(msg.contains("managed by BASE"));
    assert!(blocked, "MEMORY.md write should be blocked in 'base' mode");
}

#[test]
fn memory_read_intercept_enriches() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace(tmp.path());
    let config = base::config::BaseConfig::load(tmp.path());
    let ns = &config.namespace;

    // First, learn a note so there's something to recall
    base::crud::note::learn(
        tmp.path(), ns, "Always test before claiming done", "correction",
        Some("MEMORY"), None, None,
    ).unwrap();

    // Now simulate a Read of a memory file with a matching slug
    let event = serde_json::json!({
        "tool_name": "Read",
        "tool_input": {
            "file_path": "/home/user/.claude/projects/-test/memory/always-test-before-claiming-done.md"
        }
    });

    let result = base::hook::memory::handle_memory(&config, tmp.path(), &event);
    assert!(result.is_some(), "Should intercept memory read with graph results");

    let (msg, blocked) = result.unwrap();
    assert!(msg.contains("<base-memory-context>"));
    assert!(msg.contains("always"));
    assert!(!blocked, "mode=both should not block reads");
}

#[test]
fn recall_to_string_returns_formatted_output() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace(tmp.path());
    let config = base::config::BaseConfig::load(tmp.path());
    let ns = &config.namespace;

    base::crud::note::learn(
        tmp.path(), ns, "Use Rust for performance", "insight", None, None, None,
    ).unwrap();

    let output = base::crud::note::recall_to_string(tmp.path(), ns, Some("rust"), None);
    assert!(!output.is_empty());
    assert!(output.contains("insight"));
    assert!(output.contains("Rust"));
}

#[test]
fn recall_to_string_empty_when_no_match() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace(tmp.path());
    let config = base::config::BaseConfig::load(tmp.path());
    let ns = &config.namespace;

    let output = base::crud::note::recall_to_string(tmp.path(), ns, Some("nonexistent-xyz"), None);
    assert!(output.is_empty());
}
