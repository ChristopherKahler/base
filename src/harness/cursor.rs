//! Cursor.
//!
//! Cursor reads `~/.cursor/hooks.json` (or `.cursor/hooks.json` per project) and,
//! when "Include Third-Party Plugins, Skills, and Other Configs" is on — the
//! default — it ALSO reads `~/.claude/settings.json` and maps Claude Code's hook
//! events onto its own. That compatibility is why base already half-works in
//! Cursor, and why this module has to be careful not to wire a second copy of
//! the same hook into a machine where both paths are live: with both, every base
//! hook runs twice, injecting twice and writing the match log twice.
//!
//! Event mapping (Cursor's name ← Claude's):
//!
//! | Cursor | Claude Code |
//! |---|---|
//! | `sessionStart` | `SessionStart` |
//! | `beforeSubmitPrompt` | `UserPromptSubmit` |
//! | `preToolUse` | `PreToolUse` |
//! | `postToolUse` | `PostToolUse` |
//! | `stop` | `Stop` |
//! | `sessionEnd` | `SessionEnd` |
//!
//! Cursor's hook file is the operator's: this machine already has rtk and
//! ponytail entries in it. Wiring appends and never rewrites an entry it did not
//! write; un-wiring removes only entries whose command names `base hook`, as the
//! exact inverse byte splice, so a wire→unwire round trip is byte-identical.
//!
//! Payload: Cursor follows Claude's shape (`tool_name`, `tool_input`,
//! `session_id`, `cwd`) with its own tool names (`Shell` for `Bash`, `Write` for
//! `Edit`, `Read` for `Read`, `Grep` for `Grep`). It has no `Glob`, so a Glob
//! matcher must never be wired here, and a Glob-shaped matcher simply never
//! fires rather than erroring.
//!
//! The two payload edges differ from Claude Code's:
//!
//! - **Inbound** renames Cursor's tool names onto the ones the engine's matchers
//!   read, and lifts Cursor's `conversation_id` onto `session_id` (the engine's
//!   key) without destroying the host's own. Everything else is untouched.
//! - **Outbound** is one line shaped per event: `additional_context` for the
//!   session's opening context and the prompt, and the engine's own
//!   `hookSpecificOutput` envelope for the tool hooks. `stop` and `sessionEnd`
//!   have no injection field and are left silent.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Binding, Harness, WireOutcome};

/// Cursor's own event name for each canonical event base serves, in wire order.
///
/// The command still passes the canonical name (what the engine dispatches on);
/// this table exists for the key under `hooks`.
const WIRED: [(&str, &str); 6] = [
    ("sessionStart", "session-start"),
    ("beforeSubmitPrompt", "user-prompt-submit"),
    ("preToolUse", "pre-tool-use"),
    ("postToolUse", "post-tool-use"),
    ("stop", "stop"),
    ("sessionEnd", "session-end"),
];

/// Long enough for a cold hook on a large graph, short enough that a wedged hook
/// cannot stall a turn. Cursor's `timeout` is in seconds.
const TIMEOUT_SEC: u64 = 10;

/// The substring every command base writes contains, and the marker un-wiring
/// looks for. Deliberately the same spelling `base uninstall` uses for Claude
/// Code's entries.
const BASE_COMMAND: &str = "base hook";

/// Cursor's payload in the engine's shape.
///
/// Two renames, and nothing else: the runtime tool name onto the Claude name the
/// engine's matchers read, and Cursor's `conversation_id` onto `session_id`
/// (which the engine reads and the per-session dedup is namespaced by) while
/// keeping the host's own key. `cwd`, `tool_input` and every other field survive
/// untouched, and an unknown tool name passes through — a matcher that does not
/// know it simply never fires, which is the correct fail-open answer.
pub fn inbound(_event: &str, raw: Value) -> Value {
    let Value::Object(mut map) = raw else {
        // Not a payload shape the engine can read, but also not ours to break:
        // every reader in the engine is a `.get` that returns `None`.
        return raw;
    };

    if let Some(value) = map.remove("tool_name") {
        let normalized = match value.as_str() {
            Some(name) => Value::from(claude_tool_name(name)),
            None => value,
        };
        map.insert("tool_name".to_string(), normalized);
    }

    if !map.contains_key("session_id")
        && let Some(id) = map.get("conversation_id").cloned()
    {
        map.insert("session_id".to_string(), id);
    }

    Value::Object(map)
}

/// Cursor's runtime tool names in the Claude names the engine matches on.
///
/// Only the two Cursor spells differently are mapped. An unknown name passes
/// through. `Glob` is intentionally absent: Cursor has no such tool, so nothing
/// ever asks the engine about one there.
fn claude_tool_name(name: &str) -> &str {
    match name {
        "Shell" => "Bash",
        "Write" => "Edit",
        _ => name,
    }
}

/// What Cursor's native hooks read, one line, shaped per event.
///
/// The engine's answer is unwrapped with [`super::context_of`] first, so the
/// model never sees an envelope inside the text. `session-start` and the prompt
/// carry Cursor's `additional_context`; the tool hooks carry the engine's own
/// `hookSpecificOutput` envelope, which Cursor accepts. `stop` and `session-end`
/// have no field to carry context, and base's output on them is empty anyway.
pub fn outbound(event: &str, text: &str) -> Option<String> {
    let context = super::context_of(text);
    if context.trim().is_empty() {
        return None;
    }
    let payload = match event {
        "session-start" | "user-prompt-submit" => json!({ "additional_context": context }),
        "pre-tool-use" | "post-tool-use" => json!({
            "hookSpecificOutput": {
                "hookEventName": hook_event_name(event),
                "additionalContext": context,
            }
        }),
        _ => return None,
    };
    Some(payload.to_string())
}

/// The Claude `hookEventName` for a tool event, in the envelope Cursor accepts.
fn hook_event_name(event: &str) -> &'static str {
    match event {
        "post-tool-use" => "PostToolUse",
        _ => "PreToolUse",
    }
}

pub struct CursorBinding;

pub static BINDING: CursorBinding = CursorBinding;

impl Binding for CursorBinding {
    fn harness(&self) -> Harness {
        Harness::Cursor
    }

    fn detected(&self, home: &Path) -> bool {
        home.join(".cursor").is_dir()
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".cursor").join("hooks.json")
    }

    /// Append base's entries to `~/.cursor/hooks.json`, byte-preserving.
    ///
    /// Two guards run first. If Claude Code's settings already carry base's
    /// hooks, Cursor imports them by default and a native copy would double
    /// every hook, so the file is left alone and the reason is the note. If the
    /// commands are already present, nothing is written.
    fn wire(&self, home: &Path, _exe: &Path) -> Result<WireOutcome> {
        let path = self.config_path(home);

        if claude_settings_carry_base(home) {
            return Ok(WireOutcome::unchanged(
                path,
                "Cursor already imports base's Claude Code hooks; native hooks.json left alone",
            ));
        }

        let original = match read_optional(&path)? {
            Some(text) => text,
            // A fresh machine: the schema requires `version`, and Cursor reloads
            // on save, so an empty but valid file is the one to grow.
            None => "{\n  \"version\": 1,\n  \"hooks\": {}\n}\n".to_string(),
        };

        // A command already in the file, anywhere, is never added again — an
        // entry the user moved or wrapped still counts as wired.
        let missing: Vec<(&str, &str)> = WIRED
            .iter()
            .copied()
            .filter(|(_, canonical)| !original.contains(&command_for(canonical)))
            .collect();
        if missing.is_empty() {
            return Ok(WireOutcome::unchanged(path, "already wired"));
        }

        let entries: Vec<(&str, Value)> =
            missing.iter().map(|(event, canonical)| (*event, entry(canonical))).collect();
        let Some(updated) = crate::settings_json::add_hook_entries(&original, &entries) else {
            let wanted: Vec<String> =
                missing.iter().map(|(event, canonical)| format!("{event}: {}", command_for(canonical))).collect();
            anyhow::bail!(
                "{} could not take base's hook entries without rewriting the rest of it, so nothing was written; add these under \"hooks\" by hand: {}",
                path.display(),
                wanted.join("; ")
            );
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        write_atomic(&path, &updated)?;
        Ok(WireOutcome {
            path,
            changed: entries.len(),
            note: format!("added {}", missing.iter().map(|(event, _)| *event).collect::<Vec<_>>().join(", ")),
        })
    }

    /// Remove base's entries, as the exact inverse byte splice.
    fn unwire(&self, home: &Path) -> Result<WireOutcome> {
        let path = self.config_path(home);
        let Some(original) = read_optional(&path)? else {
            return Ok(WireOutcome::unchanged(path, "not wired"));
        };
        if !original.contains(BASE_COMMAND) {
            return Ok(WireOutcome::unchanged(path, "no base hooks"));
        }
        let Some((updated, removed)) = remove_base_entries(&original) else {
            anyhow::bail!(
                "{} contains base's hooks but could not be edited back without rewriting the rest of it; remove the entries naming `{BASE_COMMAND}` by hand",
                path.display()
            );
        };
        write_atomic(&path, &updated)?;
        Ok(WireOutcome {
            path,
            changed: removed,
            note: format!("removed {removed} base hook {}", if removed == 1 { "entry" } else { "entries" }),
        })
    }
}

/// The command base writes for one event.
///
/// PATH-relative on purpose, like [`crate::install::HOOK_TABLE`]: `base install`
/// puts the binary on `PATH`, and a config that pins an absolute path breaks
/// when the binary moves. Cursor runs command hooks in the user's shell, so a
/// PATH lookup resolves.
fn command_for(canonical: &str) -> String {
    format!("{BASE_COMMAND} {canonical} --harness cursor")
}

/// One hook entry, appended to `hooks[event]`.
fn entry(canonical: &str) -> Value {
    json!({ "command": command_for(canonical), "timeout": TIMEOUT_SEC })
}

/// Whether Claude Code's settings already wire base, so Cursor imports them.
///
/// A text test, not a parse: the only question is whether the same commands are
/// live through Cursor's compatibility path, and every spelling base writes
/// contains [`BASE_COMMAND`].
fn claude_settings_carry_base(home: &Path) -> bool {
    let settings = home.join(".claude").join("settings.json");
    fs::read_to_string(settings).is_ok_and(|text| text.contains(BASE_COMMAND))
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming {} into place", tmp.display()))
}

/// Whether one hook entry is one base wrote.
fn is_base_value(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(BASE_COMMAND))
}

fn is_base_text(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|value| is_base_value(&value))
}

/// Every hook entry whose command names base, spliced out as text so the bytes
/// of every other tool's entry — and of the file's own formatting — are exactly
/// what they were. `None` when the text is not one this can edit.
///
/// The last step re-parses the result and compares it with the original minus
/// exactly base's entries: if the splice left the file unreadable or changed
/// anything else, nothing is returned and the caller refuses to write. That is
/// the same proof [`crate::settings_json::add_hook_entries`] makes in the other
/// direction, and it is what makes wire→unwire byte-identical.
fn remove_base_entries(text: &str) -> Option<(String, usize)> {
    let original: Value = serde_json::from_str(text).ok()?;
    let sc = Scan { b: text.as_bytes() };
    let root = sc.value(0)?;
    let top = sc.members(root)?;
    let (_, _, hooks) = top.iter().find(|(key, _, _)| key == "hooks")?;
    let events = sc.members(*hooks)?;

    let mut expected = original;
    let mut edits: Vec<Span> = Vec::new();
    let mut drop_members: Vec<usize> = Vec::new();
    let mut removed = 0usize;

    for (index, (name, _, array)) in events.iter().enumerate() {
        let elements = sc.elements(*array)?;
        let is_base: Vec<bool> =
            elements.iter().map(|e| is_base_text(&text[e.start..e.end])).collect();
        let bases = is_base.iter().filter(|b| **b).count();
        if bases == 0 {
            continue;
        }
        removed += bases;

        // What a correct splice must parse back to.
        if let Some(array) = expected
            .get_mut("hooks")
            .and_then(Value::as_object_mut)
            .and_then(|hooks| hooks.get_mut(name))
            .and_then(Value::as_array_mut)
        {
            array.retain(|entry| !is_base_value(entry));
        }

        if bases == elements.len() {
            // Every entry was base's, so the event key goes too — base created
            // it unless the user kept an empty array there.
            drop_members.push(index);
            if let Some(hooks) = expected.get_mut("hooks").and_then(Value::as_object_mut) {
                hooks.remove(name);
            }
            continue;
        }

        // Runs of base entries, removed with the comma joining them to their
        // neighbours. A run that reached the end would have been all-base above,
        // so a leading run always has an element after it.
        let mut i = 0usize;
        while i < elements.len() {
            if !is_base[i] {
                i += 1;
                continue;
            }
            let run = i;
            while i < elements.len() && is_base[i] {
                i += 1;
            }
            if run > 0 {
                edits.push(Span { start: elements[run - 1].end, end: elements[i - 1].end });
            } else {
                edits.push(Span { start: elements[0].start, end: elements[i].start });
            }
        }
    }

    if removed == 0 {
        return None;
    }

    // Event keys base created, removed as whole members — grouped so a run of
    // them is one splice and the commas around it stay balanced.
    let mut m = 0usize;
    while m < drop_members.len() {
        let first = drop_members[m];
        let mut last = first;
        while m + 1 < drop_members.len() && drop_members[m + 1] == last + 1 {
            m += 1;
            last = drop_members[m];
        }
        if first == 0 {
            if last + 1 < events.len() {
                edits.push(Span { start: events[first].1, end: events[last + 1].1 });
            } else {
                edits.push(Span { start: events[first].1, end: events[last].2.end });
            }
        } else {
            edits.push(Span { start: events[first - 1].2.end, end: events[last].2.end });
        }
        m += 1;
    }

    // Applied from the end, so earlier offsets stay true.
    edits.sort_by_key(|e| std::cmp::Reverse(e.start));
    let mut out = text.to_string();
    for edit in edits {
        out.replace_range(edit.start..edit.end, "");
    }
    (serde_json::from_str::<Value>(&out).ok()? == expected).then_some((out, removed))
}

/// One JSON value's span in `text`: `start..end` byte offsets.
#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

/// Just enough JSON structure to find where base's entries are. The same
/// technique as [`crate::settings_json`], and for the same reason: `serde_json`
/// sorts keys and has its own spacing, so writing a parsed file back rewrites
/// every byte the other tools put in it.
struct Scan<'a> {
    b: &'a [u8],
}

impl<'a> Scan<'a> {
    fn ws(&self, mut i: usize) -> usize {
        while i < self.b.len() && matches!(self.b[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        i
    }

    /// The end of the string starting at `i` (a `"`), past its closing quote.
    fn string(&self, i: usize) -> Option<usize> {
        if self.b.get(i) != Some(&b'"') {
            return None;
        }
        let mut j = i + 1;
        while j < self.b.len() {
            match self.b[j] {
                b'\\' => j += 2,
                b'"' => return Some(j + 1),
                _ => j += 1,
            }
        }
        None
    }

    /// The span of the value starting at `i` (after whitespace).
    fn value(&self, i: usize) -> Option<Span> {
        let start = self.ws(i);
        let end = match *self.b.get(start)? {
            b'"' => self.string(start)?,
            b'{' | b'[' => self.container(start)?,
            _ => {
                let mut j = start;
                while j < self.b.len()
                    && !matches!(self.b[j], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
                {
                    j += 1;
                }
                if j == start {
                    return None;
                }
                j
            }
        };
        Some(Span { start, end })
    }

    /// The end of the object or array starting at `i`, past its closing bracket.
    fn container(&self, i: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut j = i;
        while j < self.b.len() {
            match self.b[j] {
                b'"' => {
                    j = self.string(j)?;
                    continue;
                }
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j + 1);
                    }
                }
                _ => {}
            }
            j += 1;
        }
        None
    }

    /// `(key, key start, value span)` for each member of the object at `obj`.
    fn members(&self, obj: Span) -> Option<Vec<(String, usize, Span)>> {
        if self.b.get(obj.start) != Some(&b'{') {
            return None;
        }
        let mut out = Vec::new();
        let mut i = self.ws(obj.start + 1);
        if self.b.get(i) == Some(&b'}') {
            return Some(out);
        }
        loop {
            let key_start = i;
            let key_end = self.string(i)?;
            let key: String = serde_json::from_slice(&self.b[key_start..key_end]).ok()?;
            i = self.ws(key_end);
            if self.b.get(i) != Some(&b':') {
                return None;
            }
            let v = self.value(i + 1)?;
            out.push((key, key_start, v));
            i = self.ws(v.end);
            match self.b.get(i)? {
                b',' => i = self.ws(i + 1),
                b'}' => return Some(out),
                _ => return None,
            }
        }
    }

    /// The elements of the array at `arr`.
    fn elements(&self, arr: Span) -> Option<Vec<Span>> {
        if self.b.get(arr.start) != Some(&b'[') {
            return None;
        }
        let mut out = Vec::new();
        let mut i = self.ws(arr.start + 1);
        if self.b.get(i) == Some(&b']') {
            return Some(out);
        }
        loop {
            let v = self.value(i)?;
            out.push(v);
            i = self.ws(v.end);
            match self.b.get(i)? {
                b',' => i = self.ws(i + 1),
                b']' => return Some(out),
                _ => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_shapes_itself_per_event_and_stays_on_one_line() {
        let session = outbound("session-start", "line one\nline two").unwrap();
        assert_eq!(session.lines().count(), 1, "{session}");
        assert_eq!(
            serde_json::from_str::<Value>(&session).unwrap(),
            json!({ "additional_context": "line one\nline two" })
        );

        let tool = outbound("pre-tool-use", "ctx").unwrap();
        assert_eq!(tool.lines().count(), 1, "{tool}");
        let parsed: Value = serde_json::from_str(&tool).unwrap();
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(parsed["hookSpecificOutput"]["additionalContext"], "ctx");
    }

    #[test]
    fn outbound_unwraps_the_engines_claude_envelope() {
        let envelope =
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"ctx"}}"#;
        let parsed: Value = serde_json::from_str(&outbound("post-tool-use", envelope).unwrap()).unwrap();
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert_eq!(
            parsed["hookSpecificOutput"]["additionalContext"], "ctx",
            "the model must see context, not an envelope"
        );
    }

    #[test]
    fn outbound_prints_nothing_for_an_empty_emission_or_a_silent_event() {
        assert_eq!(outbound("session-start", "   \n"), None);
        assert_eq!(outbound("stop", "side effects only"), None);
        assert_eq!(outbound("session-end", "side effects only"), None);
    }

    #[test]
    fn inbound_maps_only_the_two_names_cursor_spells_differently() {
        assert_eq!(claude_tool_name("Shell"), "Bash");
        assert_eq!(claude_tool_name("Write"), "Edit");
        assert_eq!(claude_tool_name("Read"), "Read");
        assert_eq!(claude_tool_name("Task"), "Task", "an unknown name passes through");
    }
}
