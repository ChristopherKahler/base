pub mod flow;
pub mod memory;
pub mod post_tool_use;
pub mod pre_tool_use;
pub mod session_start;
pub mod stop;
pub mod user_prompt_submit;

use std::io::Read;
use std::path::PathBuf;

use crate::config::BaseConfig;

/// Data captured from hook execution for event logging.
#[derive(Debug, Default, serde::Serialize)]
pub struct HookEventData {
    pub domains_matched: Vec<String>,
    pub rules_injected: usize,
    pub suppressed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_num: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Pre-tool-use: AST file map was injected for this file
    pub ast_injected: bool,
    /// Pre-tool-use: grep/find was intercepted with ast-hint
    pub grep_intercepted: bool,
    /// Post-tool-use: section-specific AST context was injected (partial read)
    pub section_context: bool,
    /// Post-tool-use: an extension "inject" nudge was emitted this call
    pub nudged: bool,
    /// Pre-tool-use: number of standards injected for this mutation
    pub standards_injected: usize,
}

/// Extract tool name and file path from hook event JSON.
fn extract_tool_context(event: &serde_json::Value) -> (Option<String>, Option<String>) {
    let tool = event.get("tool_name")
        .or_else(|| event.get("tool").and_then(|t| t.get("name")))
        .and_then(|v| v.as_str())
        .map(String::from);

    let file = event.get("tool_input")
        .and_then(|ti| {
            ti.get("file_path")
                .or_else(|| ti.get("path"))
                .or_else(|| ti.get("command"))
        })
        .and_then(|v| v.as_str())
        .map(String::from);

    (tool, file)
}

/// Entry point for all hook events. Fail-open: any error → stderr only, exit 0, empty stdout.
pub fn dispatch(event: &str) {
    let result = run(event);
    let (success, data) = match &result {
        Ok(d) => (true, Some(d)),
        Err(_) => (false, None),
    };
    log_hook_event(event, success, data);
    if let Err(e) = result {
        eprintln!("base hook {event}: {e:#}");
    }
}

fn run(event: &str) -> anyhow::Result<HookEventData> {
    let stdin_json = read_stdin()?;

    let cwd = stdin_json
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let config = BaseConfig::load(&cwd);

    let session_id = stdin_json
        .get("session_id")
        .or_else(|| stdin_json.get("sessionId"))
        .and_then(|v| v.as_str())
        .map(String::from);

    // Bind the process before any SessionState::load, so domain/AST/standards
    // dedup is namespaced per session. Without this, a second Claude session in
    // the same workspace marks a domain injected and the first one stops seeing it.
    crate::domain::session::set_process_session(session_id.as_deref());

    match event {
        "session-start" => {
            session_start::handle(&config, &cwd, session_id.as_deref())?;
            // Relay inbox push: pending messages addressed to this session
            // (unregistered sessions get a one-line notice that a relay is live).
            if let Some(block) = crate::relay::deliver::deliver(&cwd, session_id.as_deref(), true, false) {
                print!("{block}");
            }
            // Session-targeted task relay: refresh liveness + announce any tasks
            // assigned to this session (loud, re-announced on each new session).
            if let Some(sid) = session_id.as_deref()
                && let Some(block) = relay_task_tick(sid, &cwd, crate::relay::task_inbox::Phase::SessionStart, true)
            {
                print!("{block}");
            }
            Ok(HookEventData { session_id, ..Default::default() })
        }
        "pre-tool-use" => {
            let (mut data, mut context) = pre_tool_use::handle(&config, &cwd, &stdin_json)?;
            let (tool_name, file_path) = extract_tool_context(&stdin_json);
            data.tool_name = tool_name;
            data.file_path = file_path;
            // Mid-turn task-relay scan: a pending task or ping fires loud
            // immediately so an autonomous run picks it up without waiting for
            // the next prompt.
            if let Some(sid) = session_id.as_deref()
                && let Some(block) = relay_task_tick(sid, &cwd, crate::relay::task_inbox::Phase::Tool, false)
            {
                context.push('\n');
                context.push_str(&block);
            }
            // Mid-turn spool push: a question from a squad peer must interrupt
            // the running turn, not queue behind a prompt boundary that an
            // autonomous run may never hit.
            if let Some(block) = crate::relay::deliver::deliver(&cwd, session_id.as_deref(), false, true) {
                context.push('\n');
                context.push_str(&block);
            }
            // PreToolUse context only reaches the model through the JSON
            // envelope — plain stdout is transcript-only on this event.
            let context = context.trim().to_string();
            if !context.is_empty() {
                let envelope = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "additionalContext": context,
                    }
                });
                println!("{envelope}");
            }
            data.session_id = session_id;
            Ok(data)
        }
        "post-tool-use" => {
            let mut data = post_tool_use::handle(&config, &cwd, &stdin_json)?;
            let (tool_name, file_path) = extract_tool_context(&stdin_json);
            data.tool_name = tool_name;
            data.file_path = file_path;
            data.session_id = session_id;
            Ok(data)
        }
        "user-prompt-submit" => {
            let mut data = user_prompt_submit::handle(&config, &cwd, &stdin_json)?;
            // Relay inbox push for messages that arrived mid-session. Runs in
            // the dispatcher (not the handler) so star-command and empty-prompt
            // early returns can't swallow a pending delivery. Silent when
            // unregistered — the session-start notice already ran.
            if let Some(block) = crate::relay::deliver::deliver(&cwd, session_id.as_deref(), false, false) {
                print!("{block}");
            }
            // Session-targeted task relay: refresh liveness + deliver assigned tasks.
            if let Some(sid) = session_id.as_deref()
                && let Some(block) = relay_task_tick(sid, &cwd, crate::relay::task_inbox::Phase::Prompt, true)
            {
                print!("{block}");
            }
            data.session_id = session_id;
            Ok(data)
        }
        "stop" => {
            stop::handle(&config, &cwd)?;
            // End-of-turn nudge for any still-open relayed task (terse, throttled).
            if let Some(sid) = session_id.as_deref()
                && let Some(block) = relay_task_tick(sid, &cwd, crate::relay::task_inbox::Phase::Stop, false)
            {
                print!("{block}");
            }
            Ok(HookEventData { session_id, ..Default::default() })
        }
        _ => Ok(HookEventData::default()),
    }
}

/// Session-targeted task-relay tick: on boundary events (session-start, prompt)
/// ensure this session has an auto-assigned codename and a fresh heartbeat, then
/// deliver any tasks/pings assigned to it, returning the injection block. The
/// caller decides the output channel — plain stdout on boundary events, the
/// JSON additionalContext envelope on pre-tool-use.
/// Fail-open — a broken inbox or registry never blocks the hook.
fn relay_task_tick(
    session_id: &str,
    cwd: &std::path::Path,
    phase: crate::relay::task_inbox::Phase,
    boundary: bool,
) -> Option<String> {
    if boundary {
        let _ = crate::relay::session_registry::touch(session_id, cwd);
    }
    let delivered = crate::relay::task_inbox::deliver(session_id, phase);
    // Star commands inside relayed pings resolve exactly like typed prompts
    // (Chris directive 2026-08-17, spoken pings from the hub): scan the
    // delivery block and append every matched command mode's rules.
    let delivered = delivered.map(|block| {
        let commands = crate::command::load_commands(cwd);
        let matched = crate::command::match_commands(&block, &commands);
        if matched.is_empty() {
            block
        } else {
            let extra: String = matched
                .iter()
                .map(|c| crate::command::format_command_output(c))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{block}\n{extra}")
        }
    });
    // Wake contract: any of this session's titles with a stale .watching
    // sentinel gets its Monitor arming block re-injected — forced at
    // session-start, throttled mid-turn. Stop is excluded: arming belongs at
    // starts of activity, not turn ends.
    let wake = (!matches!(phase, crate::relay::task_inbox::Phase::Stop))
        .then(|| {
            crate::relay::wake::arm_blocks_for(
                session_id,
                matches!(phase, crate::relay::task_inbox::Phase::SessionStart),
            )
        })
        .flatten();
    match (delivered, wake) {
        (Some(d), Some(w)) => Some(format!("{d}\n{w}")),
        (a, b) => a.or(b),
    }
}

/// Append a hook event to the JSONL log file. Fire-and-forget — never blocks hooks.
fn log_hook_event(hook: &str, success: bool, data: Option<&HookEventData>) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let base_dir = match crate::config::find_workspace_base(&cwd)
        .or_else(|| {
            crate::home::home_root().map(|h| h.join(".base-gbl").join(".base")).filter(|p| p.is_dir())
        }) {
        Some(d) => d,
        None => return,
    };

    let log_path = base_dir.join("hook-events.jsonl");
    let ts = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let empty = Vec::new();

    let event = serde_json::json!({
        "ts": ts,
        "hook": hook,
        "success": success,
        "domains_matched": data.map(|d| &d.domains_matched).unwrap_or(&empty),
        "rules_injected": data.map(|d| d.rules_injected).unwrap_or(0),
        "suppressed": data.map(|d| d.suppressed).unwrap_or(0),
        "prompt_num": data.and_then(|d| d.prompt_num),
        "prompt_text": data.and_then(|d| d.prompt_text.as_deref()),
        "tool_name": data.and_then(|d| d.tool_name.as_deref()),
        "file_path": data.and_then(|d| d.file_path.as_deref()),
        "session_id": data.and_then(|d| d.session_id.as_deref()),
        "ast_injected": data.map(|d| d.ast_injected).unwrap_or(false),
        "grep_intercepted": data.map(|d| d.grep_intercepted).unwrap_or(false),
        "section_context": data.map(|d| d.section_context).unwrap_or(false),
    });

    use std::io::Write;
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .and_then(|mut f| writeln!(f, "{}", event));
}

fn read_stdin() -> anyhow::Result<serde_json::Value> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    if buf.trim().is_empty() {
        Ok(serde_json::Value::Object(serde_json::Map::new()))
    } else {
        Ok(serde_json::from_str(&buf)?)
    }
}
