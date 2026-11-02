//! Antigravity CLI — the Google DeepMind agent host.
//!
//! Antigravity CLI (`agy`) discovers customizations hierarchically from
//! `~/.gemini/config/`, workspace roots (`.agents/`), and directory rules.
//! Lifecycle hooks live in `hooks.json` under plugin packages:
//! `~/.gemini/config/plugins/<name>/hooks.json` accompanied by a manifest
//! `plugin.json`.
//!
//! Events supported and mapped:
//!
//! | Antigravity | base event | Mechanism |
//! |---|---|---|
//! | `SessionStart` | `session-start` | Flat handler list, injects `injectSteps: [systemMessage]` |
//! | `PreInvocation` | `user-prompt-submit` | Flat handler list, injects `injectSteps: [ephemeralMessage]` |
//! | `PreToolUse` | `pre-tool-use` | Grouped with matcher `*`, answers `decision: allow` |
//! | `PostToolUse` | `post-tool-use` | Grouped with matcher `*`, answers `{}` |
//! | `Stop` | `stop` | Flat handler list, answers `{}` |
//!
//! Payload translation:
//! - **Inbound**: Antigravity uses protojson camelCase keys.
//!   `conversationId` maps to `session_id`, `workspacePaths[0]` maps to `cwd`,
//!   `lastUserInput` maps to `prompt`, and `toolCall.name`/`toolCall.args` map
//!   to `tool_name`/`tool_input` (with `run_command` -> `Bash`, `CommandLine` ->
//!   `command`, `Path` -> `file_path`, etc.).
//! - **Outbound**: Protojson requires valid JSON on stdout. Empty emissions
//!   answer `{}` (or empty `injectSteps` / `decision: allow`).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Binding, Harness, WireOutcome};

/// The command substring every entry base writes contains.
const BASE_COMMAND: &str = "base hook";

/// Hook execution timeout in seconds.
const TIMEOUT_SEC: u64 = 10;

/// The plugin folder name under `~/.gemini/config/plugins/`.
const PLUGIN_NAME: &str = "base";

/// Antigravity's payload in the engine's shape.
pub fn inbound(_event: &str, raw: Value) -> Value {
    let Value::Object(mut map) = raw else {
        return raw;
    };

    if let Some(session) = map.get("conversationId").cloned() {
        map.entry("session_id").or_insert(session);
    }

    if let Some(cwd) = map
        .get("workspacePaths")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .cloned()
    {
        map.entry("cwd").or_insert(cwd);
    }

    if let Some(prompt) = map.get("lastUserInput").cloned() {
        map.entry("prompt").or_insert(prompt);
    }

    if let Some(tool_call) = map.get("toolCall").and_then(|v| v.as_object()).cloned() {
        if let Some(name) = tool_call.get("name").and_then(|v| v.as_str()) {
            let normalized = claude_tool_name(name);
            map.entry("tool_name").or_insert_with(|| Value::from(normalized));
        }

        if let Some(args) = tool_call.get("args").and_then(|v| v.as_object()).cloned() {
            let mut tool_input = args;
            if let Some(cmd) = tool_input.get("CommandLine").cloned() {
                tool_input.entry("command".to_string()).or_insert(cmd);
            }
            if let Some(path) = tool_input.get("Path").cloned() {
                tool_input.entry("file_path".to_string()).or_insert(path);
            }
            map.entry("tool_input").or_insert(Value::Object(tool_input));
        }
    }

    Value::Object(map)
}

/// One line of JSON structured for Antigravity's protojson expectations.
pub fn outbound(event: &str, text: &str) -> Option<String> {
    let context = super::context_of(text);
    let trimmed = context.trim();

    let payload = match event {
        "session-start" => {
            if trimmed.is_empty() {
                json!({ "injectSteps": [] })
            } else {
                json!({
                    "injectSteps": [
                        {
                            "systemMessage": {
                                "systemMessage": trimmed
                            }
                        }
                    ]
                })
            }
        }
        "user-prompt-submit" => {
            if trimmed.is_empty() {
                json!({ "injectSteps": [] })
            } else {
                json!({
                    "injectSteps": [
                        {
                            "ephemeralMessage": trimmed
                        }
                    ]
                })
            }
        }
        "pre-tool-use" => json!({ "decision": "allow" }),
        "post-tool-use" | "stop" | "session-end" => json!({}),
        _ => return None,
    };
    Some(payload.to_string())
}

/// Antigravity tool names normalized onto the Claude names the engine matches.
fn claude_tool_name(name: &str) -> &str {
    match name {
        "run_command" | "bash" | "shell" => "Bash",
        "read_file" | "view_file" => "Read",
        "edit_file" => "Edit",
        "write_file" => "Write",
        "search_web" => "WebSearch",
        _ => name,
    }
}

pub struct AntigravityBinding;

pub static BINDING: AntigravityBinding = AntigravityBinding;

impl AntigravityBinding {
    fn plugin_dir(&self, home: &Path) -> PathBuf {
        home.join(".gemini").join("config").join("plugins").join(PLUGIN_NAME)
    }

    fn plugin_json_path(&self, home: &Path) -> PathBuf {
        self.plugin_dir(home).join("plugin.json")
    }

    fn hooks_json_path(&self, home: &Path) -> PathBuf {
        self.plugin_dir(home).join("hooks.json")
    }
}

impl Binding for AntigravityBinding {
    fn harness(&self) -> Harness {
        Harness::Antigravity
    }

    fn detected(&self, home: &Path) -> bool {
        home.join(".gemini").is_dir()
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        self.hooks_json_path(home)
    }

    fn wire(&self, home: &Path, _exe: &Path) -> Result<WireOutcome> {
        let hooks_path = self.hooks_json_path(home);
        let plugin_path = self.plugin_json_path(home);
        let plugin_dir = self.plugin_dir(home);

        let desired_plugin = serde_json::to_string_pretty(&json!({
            "name": "base",
            "version": "1.0.0",
            "description": "Base hook integration for Antigravity"
        }))? + "\n";

        let desired_hooks = serde_json::to_string_pretty(&json!({
            "base": {
                "enabled": true,
                "SessionStart": [
                    {
                        "type": "command",
                        "command": format!("{BASE_COMMAND} session-start --harness antigravity"),
                        "timeout": TIMEOUT_SEC
                    }
                ],
                "PreInvocation": [
                    {
                        "type": "command",
                        "command": format!("{BASE_COMMAND} user-prompt-submit --harness antigravity"),
                        "timeout": TIMEOUT_SEC
                    }
                ],
                "PreToolUse": [
                    {
                        "matcher": "*",
                        "hooks": [
                            {
                                "type": "command",
                                "command": format!("{BASE_COMMAND} pre-tool-use --harness antigravity"),
                                "timeout": TIMEOUT_SEC
                            }
                        ]
                    }
                ],
                "PostToolUse": [
                    {
                        "matcher": "*",
                        "hooks": [
                            {
                                "type": "command",
                                "command": format!("{BASE_COMMAND} post-tool-use --harness antigravity"),
                                "timeout": TIMEOUT_SEC
                            }
                        ]
                    }
                ],
                "Stop": [
                    {
                        "type": "command",
                        "command": format!("{BASE_COMMAND} stop --harness antigravity"),
                        "timeout": TIMEOUT_SEC
                    }
                ]
            }
        }))? + "\n";

        let hooks_current = fs::read_to_string(&hooks_path).ok();
        let plugin_current = fs::read_to_string(&plugin_path).ok();

        if hooks_current.as_deref() == Some(&desired_hooks) && plugin_current.as_deref() == Some(&desired_plugin) {
            return Ok(WireOutcome::unchanged(hooks_path, "already wired"));
        }

        fs::create_dir_all(&plugin_dir)
            .with_context(|| format!("creating {}", plugin_dir.display()))?;

        fs::write(&plugin_path, &desired_plugin)
            .with_context(|| format!("writing {}", plugin_path.display()))?;

        fs::write(&hooks_path, &desired_hooks)
            .with_context(|| format!("writing {}", hooks_path.display()))?;

        Ok(WireOutcome {
            path: hooks_path,
            changed: 5,
            note: "wrote Antigravity plugin and hooks".to_string(),
        })
    }

    fn unwire(&self, home: &Path) -> Result<WireOutcome> {
        let hooks_path = self.hooks_json_path(home);
        let plugin_path = self.plugin_json_path(home);
        let plugin_dir = self.plugin_dir(home);

        if !hooks_path.exists() {
            return Ok(WireOutcome::unchanged(hooks_path, "not wired"));
        }

        let current = fs::read_to_string(&hooks_path)
            .with_context(|| format!("reading {}", hooks_path.display()))?;

        if !current.contains(BASE_COMMAND) {
            return Ok(WireOutcome::unchanged(hooks_path, "no base hooks"));
        }

        let _ = fs::remove_file(&hooks_path);
        let _ = fs::remove_file(&plugin_path);
        let _ = fs::remove_dir(&plugin_dir);

        Ok(WireOutcome {
            path: hooks_path,
            changed: 5,
            note: "removed Antigravity plugin and hooks".to_string(),
        })
    }
}
