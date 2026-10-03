pub mod ast_hint;
pub mod automap;
pub mod flow;
pub mod memory;
pub mod post_tool_use;
pub mod pre_tool_use;
pub mod session_start;
pub mod stop;
pub mod user_prompt_submit;
pub mod walk;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

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
    /// The cwd the host reported for this event — whether it follows a
    /// session's `cd` is a question the log can now answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Pre-tool-use: AST file map was injected for this file
    pub ast_injected: bool,
    /// Pre-tool-use: an AST hint was given, which happens only on a code search (F20)
    pub grep_intercepted: bool,
    /// Post-tool-use: section-specific AST context was injected (partial read)
    pub section_context: bool,
    /// Post-tool-use: an extension "inject" nudge was emitted this call
    pub nudged: bool,
    /// Pre-tool-use: number of standards injected for this mutation
    pub standards_injected: usize,
    /// User-prompt-submit: the bracket-rules block went out on this prompt.
    /// Under K1 that is true once per tier per session, so a log showing it true
    /// on consecutive prompts at one tier is the defect returning.
    pub bracket_rules_injected: bool,
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
///
/// Inside one of base's own headless `claude -p` calls (`BASE_HEADLESS`, set by `llm`) every event returns here before
/// anything is parsed, printed or written: no config, no log row, no session file, no relay title (F27, BO-08). The
/// payload is still drained, unread, as every hook run drained it before: a prompt hook's payload carries the whole
/// extraction prompt, and the host should never be left writing it into a pipe nobody reads.
pub fn dispatch(event: &str) {
    if crate::llm::headless() {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
        return;
    }
    let outcome = run(event);
    let (success, data, error) = match &outcome.result {
        Ok(d) => (true, Some(d), None),
        Err(e) => (false, None, Some(format!("{e:#}"))),
    };
    log_hook_event(
        event,
        success,
        data,
        error,
        &outcome.cwd,
        outcome.cwd_from_payload,
    );
    if let Err(e) = outcome.result {
        eprintln!("base hook {event}: {e:#}");
    }
}

/// What `run` established before it could fail.
///
/// The cwd has to survive the error arm. `log_hook_event` picks the tier from it, and a
/// hook that FAILED is precisely the event `hook_failure_summary` has to find in the
/// right workspace — so reading the cwd back out of `HookEventData` cannot work: that is
/// `None` on exactly the arm the fix exists for (#77).
struct HookRun {
    cwd: PathBuf,
    /// False when the host sent no `cwd` and the process cwd was used instead, so the
    /// log can say which input chose the tier rather than implying the host named one.
    cwd_from_payload: bool,
    result: anyhow::Result<HookEventData>,
}

fn run(event: &str) -> HookRun {
    let stdin_json = match read_stdin() {
        Ok(v) => v,
        Err(e) => {
            // No payload at all, so the process cwd is the only input there is — and the
            // log will say so rather than implying the host reported it.
            return HookRun {
                cwd: std::env::current_dir().unwrap_or_default(),
                cwd_from_payload: false,
                result: Err(e),
            };
        }
    };

    let payload_cwd = stdin_json
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(PathBuf::from);
    let cwd_from_payload = payload_cwd.is_some();
    let cwd = payload_cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let result = run_event(event, &stdin_json, &cwd);
    HookRun {
        cwd,
        cwd_from_payload,
        result,
    }
}

fn run_event(
    event: &str,
    stdin_json: &serde_json::Value,
    cwd: &Path,
) -> anyhow::Result<HookEventData> {
    let cwd = cwd.to_path_buf();
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
            // Everything session start says is collected, measured against
            // `[budget] session_start_bytes`, and printed ONCE (rank 00). The untrimmed text
            // goes to `.base/hook-output/<session>/session-start.md` and `.base/last-session-start.md`
            // before anything prints (BO-06, F11).
            let mut out = session_start::SessionOutput::new();
            out.set_session(session_id.as_deref());
            let handled = session_start::handle(&config, &cwd, session_id.as_deref(), &mut out);
            if handled.is_ok() {
                // Relay inbox push: pending messages addressed to this session, due now
                // (unregistered sessions get a one-line notice that a relay is live, which
                // ranks in the tail: an invitation to join is not due now).
                if let Some(block) = crate::relay::deliver::deliver(&cwd, session_id.as_deref(), true) {
                    let kind = if crate::relay::deliver::is_notice(&block) { "relay-notice" } else { "relay-inbox" };
                    out.push(kind, &block, 1);
                }
                // Session-targeted task relay: refresh liveness + announce any tasks and pings for this
                // session (new ones in full, unanswered ones listed in one line), as two blocks that rank
                // apart: the items delivered to this session and the one-line watcher nudge (spec B1, BO-04).
                if let Some(sid) = session_id.as_deref() {
                    let (tasks, wake) = relay_task_parts(sid, &cwd, &config.relay, crate::relay::task_inbox::Phase::SessionStart);
                    if let Some(block) = tasks {
                        out.push("relay-tasks", &block, 1);
                    }
                    if let Some(block) = wake {
                        out.push("relay-wake", &block, 1);
                    }
                }
            }
            // A handler error still prints what it collected first: those sites had printed
            // before the error, and the unhealthy-graph warning is exactly what precedes one.
            let rendered = out.finish(&config, &cwd);
            // Rank 10 (spec A7): what is about to print is kept for `base doctor`, beside the full-output file, before
            // the print and before `handled?`, so a session start whose handler failed is still on record.
            if let Some(dir) = crate::crud::handoff_show::session_start_dir(&cwd) {
                let record = crate::emit::record::record_of(
                    &rendered,
                    "session-start",
                    session_id.as_deref(),
                );
                if let Err(why) = crate::emit::record::keep(&dir, &record) {
                    eprintln!("base: session start could not keep its output record: {why}");
                }
            }
            print!("{}", rendered.text);
            // The match log's retention (K1e), after the print. The hook still exits only after it, so it is kept cheap:
            // at most one rename a day and the deletion of archive files past `[log] prompt_days`; the log being
            // appended to is never rewritten (see `match_log::retain`).
            let _ = std::io::stdout().flush();
            if let Some(dir) = crate::crud::handoff_show::session_start_dir(&cwd)
                && let Err(why) = crate::emit::match_log::retain(&dir, config.log.prompt_days, chrono::Local::now())
            {
                eprintln!("base: session start could not prune the match log: {why}");
            }
            handled?;
            Ok(HookEventData { session_id, ..Default::default() })
        }
        "pre-tool-use" => {
            let (mut data, mut context, trace) = pre_tool_use::handle_traced(&config, &cwd, stdin_json)?;
            let (tool_name, file_path) = extract_tool_context(stdin_json);
            data.tool_name = tool_name;
            data.file_path = file_path;
            // No relay text on a tool call (BO-04, F4 and F13b). Pings, tasks and the watcher nudge used to ride here
            // too, so on 2026-10-01 ordinary Bash and ToolSearch calls carried the whole 3.4 KB wake contract and a
            // "Reply RIGHT NOW" line, and a withdrawn message reappeared mid-turn. Relay content is delivered at
            // session start and on the next prompt; a session that must hear a ping mid-turn runs the inbox watcher
            // (`base relay arm`), whose Monitor wakes it.
            //
            // The one exception (lynx's amendment to F13b): a run that has said it cannot keep a watcher
            // (`BASE_NO_WAKE_NUDGE`: Agent SDK runs, workers with no Monitor tool) has no other way to hear a question
            // mid-run, so it is given NEW items once, in the same plain form, and never a watcher line or the script.
            // Shown once across prompt and tool calls: what a tool call shows, the next prompt does not.
            if crate::relay::monitorless() {
                if let Some(sid) = session_id.as_deref()
                    && let Some(part) = crate::relay::task_inbox::deliver_deferred(sid, crate::relay::task_inbox::Phase::Tool)
                {
                    context.push('\n');
                    context.push_str(&with_star_commands(part.commit(), &cwd));
                }
                if let Some(block) = crate::relay::deliver::deliver_mid_turn(&cwd, session_id.as_deref()) {
                    context.push('\n');
                    context.push_str(&block);
                }
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
            // The match log's row (K1), after the print, never in the way of it (K1g).
            let _ = std::io::stdout().flush();
            if let Some(row) = crate::emit::match_log::file_row(trace, session_id.as_deref())
                && let Some(dir) = crate::crud::handoff_show::session_start_dir(&cwd)
                && let Err(why) = crate::emit::match_log::append(&dir, &row)
            {
                eprintln!("base: the tool hook could not write its match log row: {why}");
            }
            data.session_id = session_id;
            Ok(data)
        }
        "post-tool-use" => {
            let (mut data, context) = post_tool_use::handle(&config, &cwd, stdin_json)?;
            let (tool_name, file_path) = extract_tool_context(stdin_json);
            data.tool_name = tool_name;
            data.file_path = file_path;
            // PostToolUse context only reaches the model through the JSON envelope —
            // plain stdout is transcript-only on this event, exactly as on pre-tool-use.
            // Every block this handler produces used to go out as plain stdout, so the
            // section AST context, the extension nudges and both directory-move blocks
            // were captured by the host and never delivered (#75).
            let context = context.trim().to_string();
            if !context.is_empty() {
                let envelope = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PostToolUse",
                        "additionalContext": context,
                    }
                });
                println!("{envelope}");
            }
            data.session_id = session_id;
            Ok(data)
        }
        "user-prompt-submit" => {
            // Everything this event says is collected here as named blocks and printed ONCE, fitted to
            // `[budget] prompt_bytes` (rank 00), exactly as session start does above.
            //
            // WHAT WAS WRONG, AND WHY MEASURING ONE EMITTER WOULD HAVE BEEN WORSE THAN MEASURING
            // NONE. This arm used to hold THREE SEQUENTIAL EMITTERS, each blind to the others'
            // spend: `handle` printed at its four return sites, then the relay inbox push printed,
            // then the task tick printed. A budget cannot be enforced by any one of them, because
            // none knows what the next two are about to add — and a writer reporting nothing withheld
            // after seeing a third of the output is a POSITIVE CLAIM THAT NOTHING WAS LOST, made over
            // output that overflows anyway. That reassurance is what stops anyone looking.
            //
            // Measured on Chris's install: 13.3 KB emitted here and 2 KB delivered, the whole relay
            // wake contract and every global domain rule past the second lost inline, with no notice
            // of any kind. The same session later emitted 26 KB — the overflow GROWS as a session
            // does, so the trim is the mechanism and not a backstop.
            //
            // AND THEN THE ONE WRITER CUT LINES (BO-01). Rank 00's writer kept whole lines from the top
            // until the budget ran out. On 2026-10-01 it stopped at line 37 of a 47-line wake script, and
            // the order of the text — handler, relay, task tick — decided what survived, so the bracket
            // rules went first and the rules matched to the prompt last. Now each part is a named block
            // with a priority, the fit drops whole blocks lowest priority first, each leaves a pointer
            // line, and the session records as shown only what was printed (D15).
            let mut sink = user_prompt_submit::PromptSink::default();
            let handled = user_prompt_submit::collect(&config, &cwd, stdin_json, &mut sink);
            // Each relay block's side effects (messages marked seen, a reply deleted once announced, a ping
            // recorded as delivered, the wake nudge throttled) are held back with the block's id and run only if
            // that block is printed. A dropped relay block stays pending and arrives, whole, at the next tool call.
            let mut relay_commits: Vec<(&'static str, Vec<crate::relay::Commit>)> = Vec::new();
            // Both relay blocks stay gated on the handler succeeding, exactly as the `?` used to
            // gate them: on an error neither used to run, and this is not the change that alters it.
            if handled.is_ok() {
                // Relay inbox push for messages that arrived mid-session. Runs in
                // the dispatcher (not the handler) so star-command and empty-prompt
                // early returns can't swallow a pending delivery. Silent when
                // unregistered — the session-start notice already ran.
                use crate::emit::prompt::{Priority, PromptBlock};
                if let Some(part) = crate::relay::deliver::deliver_deferred(&cwd, session_id.as_deref(), false) {
                    sink.blocks.push(PromptBlock::new("relay-inbox", Priority::Relay, &part.text, part.items, "message"));
                    relay_commits.push(("relay-inbox", part.commits));
                }
                // Session-targeted task relay: refresh liveness + deliver new tasks and pings, then the
                // watcher nudge as a block of its own, last in priority 3. Since BO-04 it is one line per title
                // (it was the 3.4 KB wake contract), so it no longer competes with the pings above it.
                if let Some(sid) = session_id.as_deref() {
                    let (tasks, wake) = relay_task_parts_deferred(
                        sid,
                        &cwd,
                        &config.relay,
                        crate::relay::task_inbox::Phase::Prompt,
                    );
                    if let Some(part) = tasks {
                        sink.blocks.push(PromptBlock::new("relay-tasks", Priority::Relay, &part.text, part.items, "item"));
                        relay_commits.push(("relay-tasks", part.commits));
                    }
                    if let Some(part) = wake {
                        sink.blocks.push(PromptBlock::new("relay-wake", Priority::Relay, &part.text, part.items, "watcher nudge"));
                        relay_commits.push(("relay-wake", part.commits));
                    }
                }
            }
            // Corrections (BO-15): C1 on this prompt, C2 and C3 in what the session wrote since the last read (a turn
            // the user interrupted or refused ends with no Stop), and the C4 line first among the blocks of its rank
            // when one fired. After `collect`, so every return site of the handler gets it.
            let correction = crate::corrections::on_prompt(
                &config,
                &cwd,
                stdin_json,
                session_id.as_deref(),
                sink.prompt_num,
                &sink.prompt,
            );
            if let Some(block) = correction.as_ref().and_then(|c| c.block.clone()) {
                sink.blocks.push_front(block);
            }
            // Fitted to `[budget] prompt_bytes` under the key the operator actually wrote; then D15: what will be
            // printed is recorded as shown, and nothing else.
            let (fitted, committed) = sink.fit_and_commit(&config);
            for (id, commits) in relay_commits {
                if fitted.kept_blocks().any(|b| b.id == id) {
                    crate::relay::run_commits(commits);
                }
            }
            // Keep what was measured, as session start does (rank 10). The untrimmed text and this session's
            // blocks are written BEFORE the print, so every pointer line names a block `base hooks show` can
            // already print. An empty emission leaves the previous full-output file alone.
            let dir = crate::crud::handoff_show::session_start_dir(&cwd);
            if let Some(d) = dir.as_ref() {
                // The session's own `prompt-submit.md` and the workspace's latest copy (BO-06, F11), and neither when
                // `[budget] write_full_output = false`: until BO-06 this wrote the workspace file whatever it said.
                if config.budget.write_full_output && !fitted.full_text.is_empty() {
                    let _ = crate::emit::session_files::write(
                        d,
                        session_id.as_deref(),
                        crate::emit::session_files::PROMPT_FILE,
                        crate::emit::record::PROMPT_FULL_FILE,
                        &fitted.full_text,
                    );
                }
                if let Some(sid) = session_id.as_deref()
                    && let Some(why) = crate::emit::prompt::write_blocks(d, sid, &fitted).failure()
                {
                    eprintln!("base: the prompt hook could not keep its blocks for `base hooks show`: {why}");
                }
            }
            // THE SINGLE EXIT. It runs before `handled?` for the same reason session start's does:
            // the sites this replaced had already printed by the time an error could be seen, so
            // dropping their text on an error would be a regression dressed as a refactor.
            crate::emit::prompt::print(&fitted);
            let _ = std::io::stdout().flush();
            // The corrections state and its signal rows, after the print and never in the way of it: a C2 signal is
            // answered only by a C4 line that printed.
            if let Some(c) = correction {
                let printed = fitted.kept_blocks().any(|b| b.id == crate::corrections::CHECK_BLOCK);
                c.commit(printed);
            }
            if let Some(dir) = dir {
                let record = crate::emit::record::record_of_prompt(&fitted, "user-prompt-submit", session_id.as_deref());
                if let Err(why) = crate::emit::record::keep(&dir, &record) {
                    eprintln!("base: the prompt hook could not keep its output record: {why}");
                }
                // The match log's row (K1): what matched, what the printed blocks served, what the budget, the topic
                // cap and the walk cut. Written after the print and never in the way of it (K1g).
                if !sink.prompt.is_empty() {
                    let row = crate::emit::match_log::prompt_row(
                        std::mem::take(&mut sink.trace),
                        &fitted,
                        config.budget.key_as_written("prompt_bytes"),
                        session_id.as_deref(),
                        &sink.prompt,
                        sink.prompt_num,
                        config.log.prompt_text_mode(),
                    );
                    if let Err(why) = crate::emit::match_log::append(&dir, &row) {
                        eprintln!("base: the prompt hook could not write its match log row: {why}");
                    }
                }
            }
            let mut data = handled?;
            data.rules_injected = committed.rules;
            data.bracket_rules_injected = committed.bracket_block;
            data.session_id = session_id;
            Ok(data)
        }
        "stop" => {
            // Corrections (BO-15) first: C3 in what the turn's AI wrote, and the files it wrote, kept for the next
            // prompt. Fail-open, so the code-map step below always runs.
            crate::corrections::on_stop(&config, &cwd, stdin_json, session_id.as_deref());
            stop::handle(&config, &cwd)?;
            // No relay delivery at a turn's end (BO-04, F13b: relay content appears at session start and on a prompt
            // only). This arm used to run the task tick and print its block as a `systemMessage` for the operator,
            // and in doing so it recorded a new ping as delivered before any prompt had shown it to the model.
            Ok(HookEventData { session_id, ..Default::default() })
        }
        _ => Ok(HookEventData::default()),
    }
    .map(|mut data| {
        data.cwd = Some(cwd.display().to_string());
        data
    })
}

/// Star commands inside relayed pings resolve exactly like typed prompts (Chris directive 2026-08-17, spoken pings from
/// the hub): scan the delivered block and append every matched command mode's rules.
fn with_star_commands(block: String, cwd: &std::path::Path) -> String {
    let commands = crate::command::load_commands(cwd);
    let matched = crate::command::match_commands(&block, &commands);
    if matched.is_empty() {
        return block;
    }
    let extra: String = matched
        .iter()
        .map(|c| crate::command::format_command_output(c))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{block}\n{extra}")
}

/// Session-targeted task relay at a boundary (session start, prompt): ensure this session has an auto-assigned
/// codename and a fresh heartbeat, then return its two halves, kept apart: the tasks and pings delivered to this
/// session, and the one-line watcher nudge. Session start places them as two blocks.
/// Fail-open — a broken inbox or registry never blocks the hook.
fn relay_task_parts(
    session_id: &str,
    cwd: &std::path::Path,
    relay: &crate::config::RelayConfig,
    phase: crate::relay::task_inbox::Phase,
) -> (Option<String>, Option<String>) {
    let (tasks, wake) = relay_task_parts_deferred(session_id, cwd, relay, phase);
    (tasks.map(crate::relay::Part::commit), wake.map(crate::relay::Part::commit))
}

/// [`relay_task_parts`] with each half's inbox writes and nudge stamps held back as commits: the prompt hook runs a
/// half's commits only if it prints that half (BO-01).
type DeferredPart = Option<crate::relay::Part>;

fn relay_task_parts_deferred(
    session_id: &str,
    cwd: &std::path::Path,
    relay: &crate::config::RelayConfig,
    phase: crate::relay::task_inbox::Phase,
) -> (DeferredPart, DeferredPart) {
    // `[relay] enabled = false` stops the auto-codename; a session that
    // registered itself still keeps its liveness fresh.
    let _ = crate::relay::session_registry::touch_with(session_id, cwd, relay.enabled);
    let delivered = crate::relay::task_inbox::deliver_deferred(session_id, phase).map(|mut part| {
        part.text = with_star_commands(part.text, cwd);
        part
    });
    // Watcher nudge (BO-04, F4b): one line per title with no current inbox watcher, never the script. Once per
    // session, and once more each time a watcher dies; session start always says it, since a fresh context has not
    // seen it. `[relay] wake_nudge = false` (or `enabled = false`) never injects it.
    let wake = (relay.enabled && relay.wake_nudge)
        .then(|| {
            crate::relay::wake::nudge_lines_deferred(
                session_id,
                matches!(phase, crate::relay::task_inbox::Phase::SessionStart),
            )
        })
        .flatten();
    (delivered, wake)
}

/// The hook log is bounded by its own writer (#22): over this size the last
/// [`HOOK_LOG_KEEP_LINES`] lines are kept and the rest dropped, whether or not a
/// dashboard is ever opened. Same numbers the dashboard's rotation always used.
pub const HOOK_LOG_CAP_BYTES: u64 = 10 * 1024 * 1024;
pub const HOOK_LOG_KEEP_LINES: usize = 5000;

/// Truncate `hook-events.jsonl` under `base_dir` to its last lines once it is over the cap.
pub fn rotate_hook_log(base_dir: &std::path::Path) {
    let log_path = base_dir.join("hook-events.jsonl");
    let Ok(meta) = std::fs::metadata(&log_path) else { return };
    if meta.len() < HOOK_LOG_CAP_BYTES {
        return;
    }
    let Ok(content) = std::fs::read_to_string(&log_path) else { return };
    let lines: Vec<&str> = content.lines().collect();
    let keep = lines.len().saturating_sub(HOOK_LOG_KEEP_LINES);
    let tail: String = lines[keep..].join("\n") + "\n";
    let tmp = base_dir.join("hook-events.jsonl.tmp");
    if std::fs::write(&tmp, tail).is_ok() {
        let _ = std::fs::rename(&tmp, &log_path);
    }
}

/// How many trailing events a failure summary reads. A trail, not history.
pub const HOOK_FAILURE_WINDOW: usize = 200;

/// #20: hooks fail open by design, so a broken hook looks like a quiet one. This reads the
/// last [`HOOK_FAILURE_WINDOW`] events of a tier's log and names the failures, or `None`
/// when there are none, so `doctor` and session start can say it happened.
/// What the hook trail says, for `doctor` and for session start.
///
/// `broken_now` is the half that counts against health: a hook whose MOST RECENT
/// event in the window failed is failing today. An older failure followed by
/// successes is history — it stays in the line so an operator can see it happened,
/// but treating it as unhealthy would hold `doctor` red for days after one
/// transient miss (auk's ruling, 2026-09-07).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookTrail {
    pub summary: String,
    pub broken_now: bool,
}

pub fn hook_failure_summary(base_dir: &std::path::Path) -> Option<HookTrail> {
    let content = std::fs::read_to_string(base_dir.join("hook-events.jsonl")).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let window = &lines[lines.len().saturating_sub(HOOK_FAILURE_WINDOW)..];
    let mut failed = 0usize;
    let mut last: Option<(String, String, String)> = None;
    // The latest outcome per hook NAME, in window order: the last write wins, so
    // after the loop this holds each hook's most recent result.
    let mut latest: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for line in window {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let hook = v.get("hook").and_then(|h| h.as_str()).unwrap_or("?").to_string();
        let ok = v.get("success").and_then(|s| s.as_bool()) != Some(false);
        latest.insert(hook.clone(), ok);
        if !ok {
            failed += 1;
            last = Some((
                hook,
                v.get("ts").and_then(|t| t.as_str()).unwrap_or("?").to_string(),
                v.get("error").and_then(|e| e.as_str()).unwrap_or("(no error text; older binary)").to_string(),
            ));
        }
    }
    let (hook, ts, err) = last?;
    let broken_now = latest.values().any(|ok| !ok);
    let tail = if broken_now {
        "That hook has not succeeded since."
    } else {
        "It has succeeded since, so this is history rather than a live fault."
    };
    Some(HookTrail {
        summary: format!(
            "hooks: {failed} failed of the last {} run(s); last: {hook} at {ts}: {err}. Hooks fail open, so the session never saw it. {tail}",
            window.len()
        ),
        broken_now,
    })
}

/// The tier log dirs a report can read: the workspace `.base` from `cwd`, then the global one.
pub fn hook_log_dirs(cwd: &std::path::Path) -> Vec<(&'static str, std::path::PathBuf)> {
    let mut out = Vec::new();
    if let Some(ws) = crate::config::find_workspace_base(cwd) {
        out.push(("workspace", ws));
    }
    if let Some(g) = crate::home::home_root().map(|h| h.join(".base-gbl").join(".base")).filter(|p| p.is_dir()) {
        out.push(("global", g));
    }
    out
}

/// Append one event line, trimming the file first when it is over the cap. The writer
/// owns its own bound; nothing else has to run for the trail to stay bounded.
pub fn append_hook_event(base_dir: &std::path::Path, event: &serde_json::Value) {
    rotate_hook_log(base_dir);
    use std::io::Write;
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(base_dir.join("hook-events.jsonl"))
        .and_then(|mut f| writeln!(f, "{}", event));
}

/// Append a hook event to the JSONL log file. Fire-and-forget — never blocks hooks.
fn log_hook_event(
    hook: &str,
    success: bool,
    data: Option<&HookEventData>,
    error: Option<String>,
    payload_cwd: &std::path::Path,
    cwd_from_payload: bool,
) {
    // #77: the tier comes from the cwd the HOST reported, not from wherever this process
    // happens to be standing. A host that runs hooks from its own directory (Codex sets
    // the child's cwd from its own request; Gemini CLI and Cursor do not specify) used to
    // log workspace A's event into workspace B's tier, so a failing hook in A was
    // invisible from A — while the line itself named A the whole time.
    let base_dir = match crate::config::find_workspace_base(payload_cwd)
        .or_else(|| {
            crate::home::home_root().map(|h| h.join(".base-gbl").join(".base")).filter(|p| p.is_dir())
        }) {
        Some(d) => d,
        None => return,
    };

    let ts = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let empty = Vec::new();

    let event = serde_json::json!({
        "ts": ts,
        "hook": hook,
        "success": success,
        "error": error,
        "cwd": data.and_then(|d| d.cwd.clone()),
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
        // Both of these were set on the struct and never written out, so an extension
        // nudge was invisible in BOTH directions: the model never received it, and the
        // log never recorded that it had fired (#75).
        "nudged": data.map(|d| d.nudged).unwrap_or(false),
        "standards_injected": data.map(|d| d.standards_injected).unwrap_or(0),
        // Absent is not the same as empty: name the input that chose the tier.
        "cwd_source": if cwd_from_payload { "payload" } else { "process" },
    });

    append_hook_event(&base_dir, &event);
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
