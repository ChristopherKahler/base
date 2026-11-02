//! GitHub Copilot CLI.
//!
//! User-level hooks live in `~/.copilot/hooks/*.json` (honouring `COPILOT_HOME`
//! when set) and repo-level ones in `.github/hooks/*.json`, which must be
//! committed. base writes one file of its own, `base.json`, so un-wiring is a
//! delete rather than a merge: no other tool's hooks share that file.
//!
//! ```json
//! { "version": 1, "hooks": {
//!   "sessionStart": [{ "type": "command",
//!     "bash": "base hook session-start --harness copilot",
//!     "powershell": "base hook session-start --harness copilot",
//!     "timeoutSec": 10 }] } }
//! ```
//!
//! Events: `sessionStart`, `sessionEnd`, `userPromptSubmitted`, `preToolUse`,
//! `postToolUse`, `errorOccurred`, `agentStop`. Injection is a single line,
//! `{"additionalContext": "..."}`, on stdout.
//!
//! Copilot's payload is not Claude's: a `preToolUse` hook receives
//! `sessionId`, `toolName` and `toolArgs`, and `toolArgs` is documented as a
//! JSON **string**. [`inbound`] copies those onto the keys the engine reads and
//! leaves the host's own keys in place, so a payload that already carries the
//! engine's names (`session_id`, `tool_input`) passes through unchanged. This
//! coalescing is deliberately one-directional: base never removes a field a host
//! sent, because the engine's readers are all `.get` lookups that ignore what
//! they do not know.
//!
//! Known upstream bugs to design around (all open as of 2026-10):
//!
//! - `sessionStart` `additionalContext` is ignored on some versions (#2142).
//! - `sessionStart` fires per prompt rather than once per session (#2491).
//! - `preToolUse` ignores `updatedInput` (#2013).
//!
//! So: Copilot is injection-only. Nothing may depend on a `sessionStart` hook
//! having run, the engine must tolerate `session-start` arriving many times,
//! and every hook fails open.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Binding, Harness, WireOutcome};

/// One entry per canonical event, keyed by Copilot's own event name.
///
/// The config is written with camelCase names, so Copilot sends the camelCase
/// payload and [`inbound`] does the translating. The command still passes the
/// canonical event name, which is what the engine dispatches on.
const WIRED: [(&str, &str); 6] = [
    ("sessionStart", "session-start"),
    ("userPromptSubmitted", "user-prompt-submit"),
    ("preToolUse", "pre-tool-use"),
    ("postToolUse", "post-tool-use"),
    ("agentStop", "stop"),
    ("sessionEnd", "session-end"),
];

/// Long enough for a cold hook on a large graph, short enough that a wedged
/// hook cannot stall the agent. Copilot's default is 30 seconds; base's hooks
/// are budgeted to return in well under this.
const TIMEOUT_SEC: u64 = 10;

/// The substring every command base writes contains, and the marker un-wiring
/// looks for. Deliberately the same spelling `base uninstall` uses for Claude
/// Code's entries.
const BASE_COMMAND: &str = "base hook";

/// Copilot's payload in the engine's shape.
///
/// Three coalesces, and nothing else: `sessionId` → `session_id`, `toolName` →
/// `tool_name` (runtime names normalized to the Claude names the matchers read
/// literally — `pre_tool_use.rs` compares against `"Bash"`, `"Edit"`,
/// `"PowerShell"`), `toolArgs` → `tool_input` (parsed when it is the JSON string
/// Copilot documents, passed through otherwise). Every other field survives
/// untouched, and the host's own keys are kept rather than moved: the engine
/// only ever reads, so a payload arriving in Claude's dialect is already right
/// and an unknown field costs nothing.
pub fn inbound(_event: &str, raw: Value) -> Value {
    let Value::Object(mut map) = raw else {
        // Not a payload shape the engine can read, but also not ours to break:
        // every reader in the engine is a `.get` that returns `None`.
        return raw;
    };

    if let Some(value) = map.get("sessionId").cloned() {
        map.entry("session_id").or_insert(value);
    }

    if let Some(value) = map.get("toolName").cloned() {
        let normalized = match value.as_str() {
            Some(name) => Value::from(claude_tool_name(name)),
            None => value,
        };
        map.entry("tool_name").or_insert(normalized);
    }

    if let Some(value) = map.get("toolArgs").cloned() {
        map.entry("tool_input").or_insert(parse_tool_args(value));
    }

    Value::Object(map)
}

/// One line of `{"additionalContext": …}`, which is all Copilot reads.
pub fn outbound(_event: &str, text: &str) -> Option<String> {
    let context = super::context_of(text);
    if context.trim().is_empty() {
        return None;
    }
    Some(json!({ "additionalContext": context }).to_string())
}

/// Copilot's runtime tool names in the Claude names the engine matches on.
///
/// The table is Copilot's own (docs: "Tool names for hook matching"). An
/// unknown name passes through: a matcher that does not know it does nothing,
/// which is the correct fail-open answer.
fn claude_tool_name(name: &str) -> &str {
    match name {
        "bash" | "powershell" | "shell" => "Bash",
        "view" => "Read",
        "create" => "Write",
        "edit" | "str_replace_editor" | "apply_patch" => "Edit",
        "grep" | "rg" => "Grep",
        "glob" => "Glob",
        "web_fetch" => "WebFetch",
        "web_search" => "WebSearch",
        "ask_user" => "AskUserQuestion",
        "update_todo" => "TodoWrite",
        "task" => "Agent",
        _ => name,
    }
}

/// `toolArgs` is documented as a JSON string in the camelCase dialect, and as
/// an object in the PascalCase one. Parse the string; if it is not JSON, keep
/// it as-is rather than inventing a shape the host did not send.
fn parse_tool_args(value: Value) -> Value {
    if let Value::String(text) = &value
        && let Ok(parsed) = serde_json::from_str::<Value>(text)
    {
        return parsed;
    }
    value
}

pub struct CopilotBinding;

pub static BINDING: CopilotBinding = CopilotBinding;

impl Binding for CopilotBinding {
    fn harness(&self) -> Harness {
        Harness::Copilot
    }

    fn detected(&self, home: &Path) -> bool {
        copilot_home(home).is_dir()
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        copilot_home(home).join("hooks").join("base.json")
    }

    /// Write `base.json`, preserving any entry base did not write.
    ///
    /// The file is base's own, so a stale base entry is replaced rather than
    /// duplicated — but a user who added their own hook to it keeps that hook.
    /// A second call changes nothing.
    fn wire(&self, home: &Path, _exe: &Path) -> Result<WireOutcome> {
        let path = self.config_path(home);
        let original = read_config(&path)?;
        let mut document = original.clone().unwrap_or_else(|| json!({ "version": 1, "hooks": {} }));

        // The events this call added, and the base entries it dropped (a stale
        // one, or a duplicate of the one it re-added). Both are "changes" for
        // the transcript; the document comparison below is what decides whether
        // anything was written at all.
        let mut added: Vec<&'static str> = Vec::new();
        let mut dropped = 0usize;

        {
            let object = document
                .as_object_mut()
                .with_context(|| format!("{} is not a JSON object; left untouched", path.display()))?;
            object.insert("version".to_string(), json!(1));
            let hooks = object
                .entry("hooks")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .with_context(|| {
                    format!("{} is not mergeable: hooks is a non-object; left untouched", path.display())
                })?;

            // Events base no longer wires — a future release could drop one —
            // still lose their stale base entry. Another tool's entries in the
            // same event stay exactly where they are, and an event base empties
            // is removed rather than left as an empty array.
            let stale: Vec<String> = hooks
                .keys()
                .filter(|event| !WIRED.iter().any(|&(name, _)| name == event.as_str()))
                .cloned()
                .collect();
            for event in stale {
                if let Some(entries) = hooks.get_mut(&event).and_then(Value::as_array_mut) {
                    let before = entries.len();
                    entries.retain(|existing| !is_base_entry(existing));
                    dropped += before - entries.len();
                    if entries.is_empty() {
                        hooks.remove(&event);
                    }
                }
            }

            for (event, canonical) in WIRED {
                let desired = entry(canonical);
                match hooks.get_mut(event).and_then(Value::as_array_mut) {
                    Some(entries) => {
                        // Exactly one base entry, the current one, and it keeps
                        // its place: a rewrite that reorders a file the operator
                        // has been reading is a diff nobody asked for.
                        let mut kept = false;
                        let mut merged = Vec::with_capacity(entries.len() + 1);
                        for existing in entries.drain(..) {
                            if is_base_entry(&existing) {
                                if !kept && existing == desired {
                                    merged.push(desired.clone());
                                    kept = true;
                                } else {
                                    dropped += 1;
                                }
                            } else {
                                merged.push(existing);
                            }
                        }
                        if !kept {
                            merged.push(desired);
                            added.push(event);
                        }
                        *entries = merged;
                    }
                    None => {
                        hooks.insert(event.to_string(), json!([desired]));
                        added.push(event);
                    }
                }
            }
        }

        if original.as_ref() == Some(&document) {
            return Ok(WireOutcome::unchanged(path, "already wired"));
        }

        let changed = added.len() + dropped;
        write_config(&path, &document)?;
        let note = if added.is_empty() {
            format!("updated {changed} base hook {}", if changed == 1 { "entry" } else { "entries" })
        } else {
            format!("added {}", added.join(", "))
        };
        Ok(WireOutcome { path, changed, note })
    }

    /// Remove base's entries. The file is deleted only when every command in it
    /// is base's; a file that has grown a foreign hook is edited, not deleted.
    fn unwire(&self, home: &Path) -> Result<WireOutcome> {
        let path = self.config_path(home);
        let Some(mut document) = read_config(&path)? else {
            return Ok(WireOutcome::unchanged(path, "not wired"));
        };

        if all_base(&document) {
            let removed = entry_count(&document);
            let note = format!("removed {removed} base hook {}", if removed == 1 { "entry" } else { "entries" });
            fs::remove_file(&path)
                .with_context(|| format!("could not remove {}", path.display()))?;
            return Ok(WireOutcome { path, changed: removed, note });
        }

        let object = document
            .as_object_mut()
            .with_context(|| format!("{} is not a JSON object; left untouched", path.display()))?;
        let Some(hooks) = object.get_mut("hooks") else {
            return Ok(WireOutcome::unchanged(path, "no base hooks"));
        };
        let hooks = hooks
            .as_object_mut()
            .with_context(|| format!("{}.hooks is not a JSON object; left untouched", path.display()))?;

        let mut removed = 0usize;
        let mut empty = Vec::new();
        for (event, value) in hooks.iter_mut() {
            let entries = value
                .as_array_mut()
                .with_context(|| format!("{}.hooks.{event} is not an array; left untouched", path.display()))?;
            let before = entries.len();
            entries.retain(|existing| !is_base_entry(existing));
            removed += before - entries.len();
            if entries.is_empty() {
                empty.push(event.clone());
            }
        }
        for event in empty {
            hooks.remove(&event);
        }

        if removed == 0 {
            return Ok(WireOutcome::unchanged(path, "no base hooks"));
        }

        write_config(&path, &document)?;
        Ok(WireOutcome {
            path,
            changed: removed,
            note: format!("removed {removed} base hook {}", if removed == 1 { "entry" } else { "entries" }),
        })
    }
}

/// The command and entry base writes for one event.
///
/// PATH-relative on purpose, like [`crate::install::HOOK_TABLE`]: `base install`
/// puts the binary on `PATH`, and a config that pins an absolute path breaks
/// when the binary moves. The `exe` argument exists for a host that cannot rely
/// on `PATH`; Copilot runs command hooks in the user's shell, so it can.
fn entry(canonical: &str) -> Value {
    let command = format!("{BASE_COMMAND} {canonical} --harness copilot");
    json!({
        "type": "command",
        "bash": command,
        "powershell": command,
        "timeoutSec": TIMEOUT_SEC,
    })
}

/// Whether one hook entry is one base wrote.
///
/// Every command field on the entry has to name base; an entry that mixes a
/// base command with a foreign one is not clearly ours and is left alone.
fn is_base_entry(entry: &Value) -> bool {
    let Some(object) = entry.as_object() else {
        return false;
    };
    let mut seen = false;
    for key in ["bash", "powershell", "command", "exec"] {
        if let Some(command) = object.get(key).and_then(Value::as_str) {
            seen = true;
            if !command.contains(BASE_COMMAND) {
                return false;
            }
        }
    }
    seen
}

/// Whether every hook entry in the document is base's, so the file can go.
fn all_base(document: &Value) -> bool {
    let Some(hooks) = document.get("hooks").and_then(Value::as_object) else {
        return false;
    };
    let mut entries = 0usize;
    for value in hooks.values() {
        let Some(list) = value.as_array() else {
            return false;
        };
        for entry in list {
            if !is_base_entry(entry) {
                return false;
            }
            entries += 1;
        }
    }
    entries > 0
}

fn entry_count(document: &Value) -> usize {
    document
        .get("hooks")
        .and_then(Value::as_object)
        .map(|hooks| {
            hooks
                .values()
                .filter_map(Value::as_array)
                .map(Vec::len)
                .sum()
        })
        .unwrap_or(0)
}

fn read_config(path: &Path) -> Result<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON; left untouched", path.display()))
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
    }
}

fn write_config(path: &Path, document: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(document)
        .with_context(|| format!("could not encode {}", path.display()))?;
    text.push('\n');
    fs::write(path, text).with_context(|| format!("could not write {}", path.display()))
}

/// `COPILOT_HOME` when set, else `~/.copilot`.
fn copilot_home(home: &Path) -> PathBuf {
    match std::env::var_os("COPILOT_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home.join(".copilot"),
    }
}
