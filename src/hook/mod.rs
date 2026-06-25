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

    match event {
        "session-start" => {
            session_start::handle(&config, &cwd)?;
            Ok(HookEventData { session_id, ..Default::default() })
        }
        "pre-tool-use" => {
            let mut data = pre_tool_use::handle(&config, &cwd, &stdin_json)?;
            let (tool_name, file_path) = extract_tool_context(&stdin_json);
            data.tool_name = tool_name;
            data.file_path = file_path;
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
            data.session_id = session_id;
            Ok(data)
        }
        "stop" => {
            stop::handle(&config, &cwd)?;
            Ok(HookEventData { session_id, ..Default::default() })
        }
        _ => Ok(HookEventData::default()),
    }
}

/// Append a hook event to the JSONL log file. Fire-and-forget — never blocks hooks.
fn log_hook_event(hook: &str, success: bool, data: Option<&HookEventData>) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let base_dir = match crate::config::find_workspace_base(&cwd)
        .or_else(|| {
            dirs::home_dir().map(|h| h.join(".base-gbl").join(".base")).filter(|p| p.is_dir())
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
