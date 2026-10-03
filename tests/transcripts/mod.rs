//! Claude Code transcripts in their real line shapes, built from short event lists, for BO-15's correction detector
//! tests and its replay check. The shapes were read on Claude Code 2.1.288 transcripts (2026-10-03): a typed prompt
//! carries `turnOrigin: "human"`, a task notification `task_notification`; an interrupt carries
//! `interruptedMessageId`; a refused tool call `toolDenialKind`; CLAUDE.md, hook output and reminders arrive as
//! `attachment` lines and `isMeta` user lines. Every text here is made up for the test.
#![allow(dead_code)]

use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

/// One thing that happens in a session.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ev {
    /// A prompt the user typed.
    Prompt(String),
    /// A task notification Claude Code queued as a prompt.
    Notification(String),
    /// A text block of the AI's reply.
    Text(String),
    /// A thinking block.
    Thinking(String),
    /// A Write tool call on this file, and its result.
    Write(String),
    /// An Edit tool call on this file, and its result.
    Edit(String),
    /// A Read whose result is this content.
    Read { file: String, content: String },
    /// The user stopped the turn.
    Interrupt,
    /// The user stopped the turn during a tool call.
    InterruptToolUse,
    /// The user refused a tool call.
    Denial,
    /// A hook or a permission rule refused a tool call: not the user.
    RuleDenial,
    /// CLAUDE.md, as Claude Code attaches it at the start.
    Instructions(String),
    /// A hook's additional context.
    HookContext(String),
    /// A system reminder (an `isMeta` user line).
    Reminder(String),
    /// A subagent's text.
    SidechainText(String),
    /// Claude Code noticed this file changed outside the AI's own write.
    FileChanged(String),
}

/// The transcript's JSONL lines for `events`, in session `session`.
pub fn lines(session: &str, events: &[Ev]) -> Vec<String> {
    lines_from(session, events, 0)
}

/// [`lines`], numbered after `first` lines already written, so uuids and times keep rising.
pub fn lines_from(session: &str, events: &[Ev], first: usize) -> Vec<String> {
    let mut out: Vec<Value> = Vec::new();
    let mut n = first;
    let mut next = || {
        n += 1;
        n
    };
    let base = |kind: &str, i: usize| {
        json!({
            "parentUuid": null,
            "isSidechain": false,
            "type": kind,
            "uuid": format!("00000000-0000-4000-8000-{i:012}"),
            "timestamp": format!("2026-10-03T10:{:02}:{:02}.000Z", (i / 60) % 60, i % 60),
            "userType": "external",
            "entrypoint": "cli",
            "cwd": "C:\\work",
            "sessionId": session,
            "version": "2.1.288",
        })
    };
    let with = |mut v: Value, extra: Value| {
        if let (Some(obj), Some(more)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in more {
                obj.insert(k.clone(), x.clone());
            }
        }
        v
    };
    let mut tool = 0usize;
    for e in events {
        let i = next();
        match e {
            Ev::Prompt(text) => out.push(with(base("user", i), json!({
                "promptId": format!("p{i}"),
                "message": {"role": "user", "content": text},
                "permissionMode": "bypassPermissions",
                "origin": {"kind": "human"},
                "promptSource": "typed",
                "turnOrigin": "human",
            }))),
            Ev::Notification(text) => out.push(with(base("user", i), json!({
                "promptId": format!("p{i}"),
                "message": {"role": "user", "content": format!("<task-notification>\n<task-id>t{i}</task-id>\n<event>{text}</event>\n</task-notification>")},
                "origin": {"kind": "task-notification", "producer": "session-task"},
                "promptSource": "system",
                "turnOrigin": "task_notification",
            }))),
            Ev::Text(text) => out.push(with(base("assistant", i), json!({
                "message": {"role": "assistant", "content": [{"type": "text", "text": text}], "stop_reason": "end_turn"},
            }))),
            Ev::Thinking(text) => out.push(with(base("assistant", i), json!({
                "message": {"role": "assistant", "content": [{"type": "thinking", "thinking": text}]},
            }))),
            Ev::Write(file) | Ev::Edit(file) => {
                tool += 1;
                let name = if matches!(e, Ev::Write(_)) { "Write" } else { "Edit" };
                out.push(with(base("assistant", i), json!({
                    "message": {"role": "assistant", "content": [{"type": "tool_use", "id": format!("toolu_{tool}"), "name": name,
                        "input": {"file_path": file, "content": "x"}}], "stop_reason": "tool_use"},
                })));
                let j = next();
                out.push(with(base("user", j), json!({
                    "message": {"role": "user", "content": [{"tool_use_id": format!("toolu_{tool}"), "type": "tool_result",
                        "content": format!("The file {file} has been updated successfully.")}]},
                    "toolUseResult": {"filePath": file},
                })));
            }
            Ev::Read { file, content } => {
                tool += 1;
                out.push(with(base("assistant", i), json!({
                    "message": {"role": "assistant", "content": [{"type": "tool_use", "id": format!("toolu_{tool}"), "name": "Read",
                        "input": {"file_path": file}}], "stop_reason": "tool_use"},
                })));
                let j = next();
                out.push(with(base("user", j), json!({
                    "message": {"role": "user", "content": [{"tool_use_id": format!("toolu_{tool}"), "type": "tool_result", "content": content}]},
                    "toolUseResult": {"type": "text", "file": {"filePath": file, "content": content}},
                })));
            }
            Ev::Interrupt => out.push(with(base("user", i), json!({
                "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]},
            }))),
            Ev::InterruptToolUse => out.push(with(base("user", i), json!({
                "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user for tool use]"}]},
                "interruptedMessageId": format!("msg_{i}"),
            }))),
            Ev::Denial | Ev::RuleDenial => {
                tool += 1;
                out.push(with(base("assistant", i), json!({
                    "message": {"role": "assistant", "content": [{"type": "tool_use", "id": format!("toolu_{tool}"), "name": "Bash",
                        "input": {"command": "rm -rf build"}}], "stop_reason": "tool_use"},
                })));
                let j = next();
                let (text, kind, result) = if matches!(e, Ev::Denial) {
                    ("The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.", "user-rejected", "User rejected tool use")
                } else {
                    ("PreToolUse:Bash hook error: blocked by a project rule", "permission-rule", "Error: blocked")
                };
                out.push(with(base("user", j), json!({
                    "message": {"role": "user", "content": [{"type": "tool_result", "content": text, "is_error": true, "tool_use_id": format!("toolu_{tool}")}]},
                    "toolUseResult": result,
                    "toolDenialKind": kind,
                })));
            }
            Ev::Instructions(text) => out.push(with(base("attachment", i), json!({
                "attachment": {"type": "instructions", "files": [{"path": "C:\\Users\\someone\\.claude\\CLAUDE.md", "type": "User", "content": text}]},
            }))),
            Ev::HookContext(text) => out.push(with(base("attachment", i), json!({
                "attachment": {"type": "hook_additional_context", "content": [text], "hookName": "UserPromptSubmit", "hookEvent": "UserPromptSubmit"},
            }))),
            Ev::Reminder(text) => out.push(with(base("user", i), json!({
                "isMeta": true,
                "message": {"role": "user", "content": [{"type": "text", "text": format!("<system-reminder>\n{text}\n</system-reminder>")}]},
            }))),
            Ev::SidechainText(text) => {
                let mut v = with(base("assistant", i), json!({
                    "message": {"role": "assistant", "content": [{"type": "text", "text": text}]},
                }));
                v["isSidechain"] = json!(true);
                out.push(v);
            }
            Ev::FileChanged(file) => out.push(with(base("attachment", i), json!({
                "attachment": {"type": "edited_text_file", "filename": file, "snippet": "1\tchanged by hand"},
            }))),
        }
    }
    out.iter().map(Value::to_string).collect()
}

/// Write the transcript for `events` to `path`, one line each.
pub fn write(path: &Path, session: &str, events: &[Ev]) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("transcript folder");
    }
    let mut text = lines(session, events).join("\n");
    text.push('\n');
    std::fs::write(path, text).expect("transcript written");
}

/// Add `events` to the end of the transcript at `path` (created when missing), as a session does while it runs.
pub fn append(path: &Path, session: &str, events: &[Ev]) {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("transcript folder");
    }
    let already = std::fs::read_to_string(path).map(|t| t.lines().count()).unwrap_or(0);
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("transcript open");
    for line in lines_from(session, events, already) {
        writeln!(f, "{line}").expect("transcript line");
    }
}
