//! The prompt hook's output as named blocks, fitted to `[budget] prompt_bytes` by dropping WHOLE
//! blocks, lowest priority first (BO-01: F1, F2, F7, and D15 for what counts as shown).
//!
//! WHAT THIS REPLACES. `print_measured` kept lines from the top until the budget ran out. Measured
//! on the operator's machine, 2026-09-23 to 2026-10-01: 388 of the 1,027 prompts with something to
//! say were cut, and 4.09 MB of 6.99 MB was lost. Three things were wrong with that cut, and each
//! is a rule here:
//!
//! 1. A line cut ends inside a block. On 2026-10-01 14:11:29 it stopped at line 37 of a 47-line
//!    shell script the reader was being told to arm. Here a block is printed whole or not at all.
//! 2. The order of the text decided what survived, and the order was "handler, then relay, then
//!    task tick": the bracket rules went first and the rules matched to the prompt went last. Here
//!    a fixed [`Priority`] decides, and within one priority the order the hook built them in.
//! 3. The log said how many bytes were cut and not which blocks, so what a prompt lost could not be
//!    named. Every dropped block is a ledger row here, by name, items and bytes.
//!
//! A dropped block leaves one line naming it, its size, and `base hooks show <block>`, which prints
//! it from this session's own file, `.base/hook-output/<session>/prompt-blocks.json`. A command
//! from the block's own source would not do: relay delivery consumes what it delivers
//! (`relay/deliver.rs` marks messages seen, `relay/task_inbox.rs` deletes a reply once announced),
//! so `base relay poll` prints nothing for a relay block the budget dropped.
//!
//! WHAT COUNTS AS SHOWN (D15). The hook records a rule as shown, a domain block or a walk name as
//! injected, and the bracket block as served only when the block carrying that record was printed.
//! Each block carries those records as [`Claim`]s, and the hook applies the claims of the blocks
//! [`fit`] kept, after fitting. A dropped rule stays eligible for the next prompt. Until BO-01 the
//! hook recorded everything it BUILT, before the cut, so a rule the cut removed was never sent again
//! in that session.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::FullOutput;

/// Highest first (F2). The brief named five; the block types it did not list were placed by lynx
/// on 2026-10-01, and each variant says what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// 1. Rules matched to the prompt: the rules of a domain matched by a prompt keyword or by a path
    ///    this session touched, topic rules, and star-command output.
    Matched,
    /// 2. Decisions and graph context matched to the prompt: such a domain's CONTEXT and query
    ///    blocks, and the walk's `<base-context>` blocks.
    Context,
    /// 3. Relay: messages, pings, replies and tasks, then the wake contract.
    Relay,
    /// 4. Global: an always-on domain's rules, CONTEXT and query; `always` rules; the grounding block.
    Global,
    /// 5. The bracket rules, then DEVMODE.
    Bracket,
}

impl Priority {
    /// The number the brief and the per-session file use: 1 is the highest.
    pub fn number(self) -> u8 {
        match self {
            Priority::Matched => 1,
            Priority::Context => 2,
            Priority::Relay => 3,
            Priority::Global => 4,
            Priority::Bracket => 5,
        }
    }
}

/// A record the hook writes into the session only when the block carrying it is printed (D15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// The rule was shown: `SessionState::mark_rule_shown`, with the scope it matched under.
    Rule { id: String, content: u64, scope: Option<String> },
    /// A domain's block, or a walk name, was injected: `SessionState::mark_injected`.
    Injected { key: String, hash: u64 },
    /// This tier's bracket block was shown: `SessionState::mark_bracket_block`.
    BracketBlock,
}

/// One named block of the prompt hook's output.
#[derive(Debug, Clone)]
pub struct PromptBlock {
    /// What the pointer line, the log and `base hooks show` call it. Unique within one output.
    pub id: String,
    pub priority: Priority,
    /// Without the newlines around it: every block starts on its own line and ends with one newline,
    /// so two blocks can never share a line (Example 2, 2026-10-01: a decision line and
    /// `<relay-ping-open>` arrived glued together).
    pub text: String,
    pub items: usize,
    /// What `items` counts, singular: "rule", "record", "message".
    pub noun: &'static str,
    pub claims: Vec<Claim>,
}

impl PromptBlock {
    pub fn new(id: impl Into<String>, priority: Priority, text: &str, items: usize, noun: &'static str) -> Self {
        PromptBlock {
            id: id.into(),
            priority,
            text: text.trim_matches(['\r', '\n']).to_string(),
            items,
            noun,
            claims: Vec::new(),
        }
    }

    pub fn with_claims(mut self, claims: impl IntoIterator<Item = Claim>) -> Self {
        self.claims.extend(claims);
        self
    }

    pub fn bytes(&self) -> usize {
        self.text.len()
    }
}

/// The blocks of one prompt, in the order the hook built them.
#[derive(Debug, Default)]
pub struct PromptBlocks {
    blocks: Vec<PromptBlock>,
}

impl PromptBlocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a block. A block with no text adds nothing. An id already present takes `-2`, `-3`, so
    /// `base hooks show <id>` always names exactly one block.
    pub fn push(&mut self, mut block: PromptBlock) {
        if block.text.trim().is_empty() {
            return;
        }
        let stem = block.id.clone();
        let mut n = 1;
        while self.blocks.iter().any(|b| b.id == block.id) {
            n += 1;
            block.id = format!("{stem}-{n}");
        }
        self.blocks.push(block);
    }

    pub fn extend(&mut self, blocks: impl IntoIterator<Item = PromptBlock>) {
        for b in blocks {
            self.push(b);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &PromptBlock> {
        self.blocks.iter()
    }
}

/// The command a pointer line names for a dropped block.
pub fn show_command(id: &str) -> String {
    format!("base hooks show {id}")
}

/// `1150` as `1,150`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// The line a dropped block leaves: its name, its size, the budget that dropped it, and the command
/// that prints it.
pub fn pointer_line(block: &PromptBlock, key: &str, budget_bytes: usize) -> String {
    format!(
        "[base: withheld {} ({}, {} bytes) over [budget] {key} = {budget_bytes} · full text: {}]",
        block.id,
        counted(block.items, block.noun),
        thousands(block.bytes()),
        show_command(&block.id)
    )
}

/// One line for every dropped block, used only when their pointer lines alone do not fit.
fn aggregate_line(dropped: &[&PromptBlock], key: &str, budget_bytes: usize) -> String {
    let names: Vec<String> = dropped
        .iter()
        .map(|b| format!("{} {} bytes", b.id, thousands(b.bytes())))
        .collect();
    format!(
        "[base: withheld {} ({}) over [budget] {key} = {budget_bytes} · full text: {}]",
        counted(dropped.len(), "block"),
        names.join(", "),
        show_command("<block>")
    )
}

/// The header, then each part, every one on its own lines with a blank line between them and one
/// newline at the end.
fn compose<'a>(header: &'a str, parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut s = String::new();
    for part in std::iter::once(header).chain(parts) {
        if part.is_empty() {
            continue;
        }
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(part);
        s.push('\n');
    }
    s
}

/// One prompt's output after fitting.
#[derive(Debug, Clone)]
pub struct Fitted {
    /// Exactly what to print.
    pub text: String,
    /// The header and every block, in output order: the body of the full-output file.
    pub full_text: String,
    /// Every block, in output order.
    pub blocks: Vec<PromptBlock>,
    /// Per block in [`Fitted::blocks`]: printed whole (true) or dropped (false).
    pub kept: Vec<bool>,
    pub budget_bytes: usize,
}

impl Fitted {
    pub fn emitted_bytes(&self) -> usize {
        self.text.len()
    }

    pub fn full_bytes(&self) -> usize {
        self.full_text.len()
    }

    pub fn kept_blocks(&self) -> impl Iterator<Item = &PromptBlock> {
        self.blocks.iter().zip(&self.kept).filter(|(_, k)| **k).map(|(b, _)| b)
    }

    pub fn dropped_blocks(&self) -> impl Iterator<Item = &PromptBlock> {
        self.blocks.iter().zip(&self.kept).filter(|(_, k)| !**k).map(|(b, _)| b)
    }

    /// The bytes of every dropped block.
    pub fn withheld_bytes(&self) -> usize {
        self.dropped_blocks().map(PromptBlock::bytes).sum()
    }

    /// Something was dropped.
    pub fn lost(&self) -> bool {
        self.kept.iter().any(|k| !k)
    }

    /// Every block was dropped that could be and the output is still over the budget: only a budget
    /// smaller than the header and one line can do that. Reported, never resolved by cutting.
    pub fn still_over_budget(&self) -> bool {
        self.text.len() > self.budget_bytes
    }
}

/// Fit `blocks` under `budget_bytes`, the header first and always kept.
///
/// 1. Blocks go in priority order; within one priority, the order they were pushed.
/// 2. Over budget, whole blocks are dropped from the bottom (the lowest priority, the last within
///    it) until the rest fits. Each leaves its [`pointer_line`]. A block no longer than its own
///    pointer line is passed over, since dropping it would make the output longer.
/// 3. Then each dropped block, highest priority first, is put back if it fits in the room the drops
///    left. Nothing kept is ever dropped to make room, so a lower block never costs a higher one;
///    what it fixes is an oversized block (the 3.4 KB wake contract) taking every block below it
///    down with it.
/// 4. Only when the pointer lines themselves do not fit do they become one line, and the blocks
///    still kept are dropped from the bottom until that fits.
pub fn fit(header: &str, blocks: PromptBlocks, budget_bytes: usize, key: &str) -> Fitted {
    let header = header.trim_matches(['\r', '\n']);
    let mut blocks = blocks.blocks;
    blocks.sort_by_key(|b| b.priority);
    let n = blocks.len();
    let pointers: Vec<String> = blocks.iter().map(|b| pointer_line(b, key, budget_bytes)).collect();
    let mut kept = vec![true; n];

    let render = |kept: &[bool]| -> String {
        compose(
            header,
            (0..n).map(|i| if kept[i] { blocks[i].text.as_str() } else { pointers[i].as_str() }),
        )
    };
    let render_aggregate = |kept: &[bool]| -> String {
        let dropped: Vec<&PromptBlock> = (0..n).filter(|i| !kept[*i]).map(|i| &blocks[i]).collect();
        let line = (!dropped.is_empty()).then(|| aggregate_line(&dropped, key, budget_bytes));
        compose(
            header,
            (0..n).filter(|i| kept[*i]).map(|i| blocks[i].text.as_str()).chain(line.as_deref()),
        )
    };

    let full_text = render(&kept);
    let mut text = full_text.clone();
    if text.len() > budget_bytes {
        for i in (0..n).rev() {
            if text.len() <= budget_bytes {
                break;
            }
            if blocks[i].bytes() <= pointers[i].len() {
                continue;
            }
            kept[i] = false;
            text = render(&kept);
        }
        if text.len() <= budget_bytes {
            for i in 0..n {
                if kept[i] {
                    continue;
                }
                kept[i] = true;
                let trial = render(&kept);
                if trial.len() <= budget_bytes {
                    text = trial;
                } else {
                    kept[i] = false;
                }
            }
        } else {
            text = render_aggregate(&kept);
            for i in (0..n).rev() {
                if text.len() <= budget_bytes {
                    break;
                }
                if kept[i] {
                    kept[i] = false;
                    text = render_aggregate(&kept);
                }
            }
        }
    }
    Fitted { text, full_text, blocks, kept, budget_bytes }
}

/// Print the fitted output: the prompt hook's one writer.
pub fn print(fitted: &Fitted) {
    print!("{}", fitted.text);
}

/// Where a session's prompt blocks are kept: `.base/hook-output/<session>/`, the per-session layout
/// BO-06 (F11) builds on. Never a shared workspace file: two sessions in one workspace each read
/// their own.
pub const SESSION_DIR: &str = "hook-output";
/// The last prompt's blocks, every one, printed or dropped.
pub const BLOCKS_FILE: &str = "prompt-blocks.json";
/// Days a session's blocks file is kept after it was last written.
const KEEP_DAYS: u64 = 7;

#[derive(Debug, Serialize, Deserialize)]
struct BlocksFile {
    written_at: String,
    session_id: String,
    budget_bytes: usize,
    blocks: Vec<BlockRow>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BlockRow {
    id: String,
    priority: u8,
    items: usize,
    /// What `items` counts, singular, as the pointer line says it.
    #[serde(default)]
    noun: String,
    bytes: usize,
    printed: bool,
    text: String,
}

/// Keep `fitted`'s blocks for `session_id`, so `base hooks show <block>` can print one, and drop the
/// files of sessions not written for [`KEEP_DAYS`]. Written on every prompt, so a block the last
/// prompt did not build is never shown as current. A failure comes back as a value, never a panic.
pub fn write_blocks(base: &Path, session_id: &str, fitted: &Fitted) -> FullOutput {
    if !crate::crud::handoff_show::is_file_safe_session_id(session_id) {
        return FullOutput::not_written(format!("session id {session_id:?} is not a file name"));
    }
    let root = base.join(SESSION_DIR);
    prune(&root);
    let file = BlocksFile {
        written_at: crate::crud::now_iso(),
        session_id: session_id.to_string(),
        budget_bytes: fitted.budget_bytes,
        blocks: fitted
            .blocks
            .iter()
            .zip(&fitted.kept)
            .map(|(b, kept)| BlockRow {
                id: b.id.clone(),
                priority: b.priority.number(),
                items: b.items,
                noun: b.noun.to_string(),
                bytes: b.bytes(),
                printed: *kept,
                text: b.text.clone(),
            })
            .collect(),
    };
    let text = serde_json::to_string_pretty(&file).unwrap_or_default();
    super::write_full_output(&root.join(session_id).join(BLOCKS_FILE), &text)
}

/// Remove blocks files older than [`KEEP_DAYS`], and a session folder left empty by that. Other
/// files in a session folder are not this module's and are left alone.
fn prune(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let keep = std::time::Duration::from_secs(KEEP_DAYS * 24 * 60 * 60);
    for entry in entries.flatten() {
        let file = entry.path().join(BLOCKS_FILE);
        let old = std::fs::metadata(&file)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > keep);
        if old {
            let _ = std::fs::remove_file(&file);
            let _ = std::fs::remove_dir(entry.path());
        }
    }
}

/// What `base hooks show` prints: stdout, and a note for stderr when the file was chosen without a
/// session id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub stdout: String,
    pub note: Option<String>,
}

/// `base hooks show [block]`: one block of this session's last prompt exactly as the hook built it,
/// or, with no block named, the list of them. `session` is the session whose file to read
/// (`CLAUDE_CODE_SESSION_ID` inside a session); without one the newest file is read and the note
/// says whose it is.
pub fn show(base: &Path, session: Option<&str>, block: Option<&str>) -> Result<Shown, String> {
    let root = base.join(SESSION_DIR);
    let (path, note) = match session.filter(|s| !s.is_empty()) {
        Some(sid) => {
            if !crate::crud::handoff_show::is_file_safe_session_id(sid) {
                return Err(format!("session id {sid:?} is not a file name"));
            }
            (root.join(sid).join(BLOCKS_FILE), None)
        }
        None => {
            let newest = std::fs::read_dir(&root)
                .ok()
                .into_iter()
                .flat_map(|entries| entries.flatten())
                .map(|e| e.path().join(BLOCKS_FILE))
                .filter_map(|p| std::fs::metadata(&p).and_then(|m| m.modified()).ok().map(|t| (t, p)))
                .max_by_key(|(t, _)| *t)
                .map(|(_, p)| p)
                .ok_or_else(|| format!("no prompt blocks are kept under {}", root.display()))?;
            let note = format!(
                "no session id (CLAUDE_CODE_SESSION_ID or --session), so this is the newest file: {}",
                newest.display()
            );
            (newest, Some(note))
        }
    };
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: BlocksFile = serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
    let Some(id) = block else {
        let mut out = format!(
            "Prompt blocks of session {} at {} ([budget] {} bytes), in output order:\n",
            file.session_id, file.written_at, file.budget_bytes
        );
        for b in &file.blocks {
            out.push_str(&format!(
                "  {}  priority {}  {}  {} bytes  {}\n",
                b.id,
                b.priority,
                counted(b.items, if b.noun.is_empty() { "item" } else { &b.noun }),
                thousands(b.bytes),
                if b.printed { "printed" } else { "withheld" }
            ));
        }
        return Ok(Shown { stdout: out, note });
    };
    match file.blocks.iter().find(|b| b.id == id) {
        Some(b) => Ok(Shown { stdout: format!("{}\n", b.text), note }),
        None => Err(format!(
            "no block {id:?} in session {}'s last prompt ({}); blocks: {}",
            file.session_id,
            file.written_at,
            file.blocks.iter().map(|b| b.id.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "prompt_bytes";
    const HEADER: &str = "<context-bracket>[FRESH] (prompt 3)</context-bracket>";

    fn block(id: &str, p: Priority, bytes: usize) -> PromptBlock {
        let body: String = format!("[{id}]\n").chars().chain(std::iter::repeat_n('x', bytes)).take(bytes).collect();
        let b = PromptBlock::new(id, p, &body, 1, "item");
        assert_eq!(b.bytes(), bytes, "control: {id} is {bytes} bytes");
        b
    }

    fn blocks(list: Vec<PromptBlock>) -> PromptBlocks {
        let mut out = PromptBlocks::new();
        out.extend(list);
        out
    }

    fn kept_ids(f: &Fitted) -> Vec<&str> {
        f.kept_blocks().map(|b| b.id.as_str()).collect()
    }

    fn dropped_ids(f: &Fitted) -> Vec<&str> {
        f.dropped_blocks().map(|b| b.id.as_str()).collect()
    }

    /// The output is the header, then every block, kept or as its pointer line, whole, in order.
    fn assert_whole(f: &Fitted) {
        let mut expect: Vec<String> = vec![HEADER.to_string()];
        for (b, kept) in f.blocks.iter().zip(&f.kept) {
            expect.push(if *kept { b.text.clone() } else { pointer_line(b, KEY, f.budget_bytes) });
        }
        assert_eq!(f.text, format!("{}\n", expect.join("\n\n")), "the output is whole blocks and pointer lines");
    }

    #[test]
    fn under_budget_everything_prints_in_priority_order() {
        let f = fit(
            HEADER,
            blocks(vec![
                block("bracket-rules", Priority::Bracket, 100),
                block("hooks-rules", Priority::Matched, 100),
                block("relay-wake", Priority::Relay, 100),
            ]),
            4000,
            KEY,
        );
        assert_eq!(kept_ids(&f), ["hooks-rules", "relay-wake", "bracket-rules"]);
        assert!(!f.lost());
        assert_eq!(f.text, f.full_text);
        assert!(f.text.starts_with(HEADER), "the header is first");
        assert_whole(&f);
    }

    /// Example 1 (lynx, 2026-10-01, Chris's prompt 3 in session 5b860473): the header, global context
    /// 1,150, a relay ping line 470 and the wake contract, against 4,000. With a wake contract that
    /// fits beside its neighbours and the pointer line, the relay blocks arrive whole and the global
    /// context is dropped with its pointer line.
    ///
    /// The brief's own sizes (wake 3,400) do NOT fit once the pointer line is counted: 57 + 470 +
    /// 3,400 + the ~125-byte pointer for global context is over 4,000. Then the wake contract goes
    /// too, and the global context comes back into the room it left. The brief's arithmetic left the
    /// pointer out; the scope (every dropped block leaves a line, the output fits) decides.
    #[test]
    fn example_one_keeps_the_relay_blocks_whole_and_drops_global_context() {
        let f = fit(
            HEADER,
            blocks(vec![
                block("global-context", Priority::Global, 1150),
                block("relay-tasks", Priority::Relay, 470),
                block("relay-wake", Priority::Relay, 3300),
            ]),
            4000,
            KEY,
        );
        assert_eq!(kept_ids(&f), ["relay-tasks", "relay-wake"]);
        assert_eq!(dropped_ids(&f), ["global-context"]);
        assert!(f.text.len() <= 4000, "{} bytes", f.text.len());
        assert!(f.text.contains(
            "[base: withheld global-context (1 item, 1,150 bytes) over [budget] prompt_bytes = 4000 · full text: base hooks show global-context]"
        ));
        assert_eq!(f.withheld_bytes(), 1150);
        assert_whole(&f);

        let f = fit(
            HEADER,
            blocks(vec![
                block("global-context", Priority::Global, 1150),
                block("relay-tasks", Priority::Relay, 470),
                block("relay-wake", Priority::Relay, 3400),
            ]),
            4000,
            KEY,
        );
        assert_eq!(kept_ids(&f), ["relay-tasks", "global-context"], "the brief's sizes, pointer counted");
        assert_eq!(dropped_ids(&f), ["relay-wake"]);
        assert!(f.text.len() <= 4000, "{} bytes", f.text.len());
        assert_whole(&f);
    }

    /// lynx's condition on the re-admit (2026-10-01): a lower block comes back only into room the
    /// drops left, never by dropping a higher one. Matched 1,000 fits; context 3,500 cannot fit
    /// beside it; global 900 and bracket 600 fit in what is left, and come back.
    #[test]
    fn prompt_submit_readmit_never_costs_a_higher_block() {
        let f = fit(
            HEADER,
            blocks(vec![
                block("hooks-rules", Priority::Matched, 1000),
                block("hooks-context", Priority::Context, 3500),
                block("global-rules", Priority::Global, 900),
                block("bracket-rules", Priority::Bracket, 600),
            ]),
            4000,
            KEY,
        );
        assert_eq!(kept_ids(&f), ["hooks-rules", "global-rules", "bracket-rules"]);
        assert_eq!(dropped_ids(&f), ["hooks-context"]);
        assert!(f.text.len() <= 4000);
        assert_whole(&f);

        // And where room is short, the lower block stays out rather than the higher one going: the
        // higher matched block fills the budget, and a lower block that would fit only in its place
        // is not put back.
        let f = fit(
            HEADER,
            blocks(vec![
                block("hooks-rules", Priority::Matched, 3500),
                block("global-rules", Priority::Global, 900),
            ]),
            4000,
            KEY,
        );
        assert_eq!(kept_ids(&f), ["hooks-rules"], "the higher block stays");
        assert_eq!(dropped_ids(&f), ["global-rules"]);
        assert_whole(&f);
    }

    #[test]
    fn prompt_submit_priority_order_holds() {
        // Five blocks of one size, pushed lowest priority first; the budget fits the header and two.
        let list = vec![
            block("bracket-rules", Priority::Bracket, 1500),
            block("global-rules", Priority::Global, 1500),
            block("relay-wake", Priority::Relay, 1500),
            block("hooks-context", Priority::Context, 1500),
            block("hooks-rules", Priority::Matched, 1500),
        ];
        let f = fit(HEADER, blocks(list), 3500, KEY);
        assert_eq!(kept_ids(&f), ["hooks-rules", "hooks-context"], "the two highest priorities present");
        assert_eq!(dropped_ids(&f), ["relay-wake", "global-rules", "bracket-rules"]);
        assert!(f.text.len() <= 3500);
        assert_whole(&f);

        // Within one priority the order they were built in holds, and the last is dropped first.
        let f = fit(
            HEADER,
            blocks(vec![
                block("relay-inbox", Priority::Relay, 1500),
                block("relay-tasks", Priority::Relay, 1500),
                block("relay-wake", Priority::Relay, 1500),
            ]),
            3500,
            KEY,
        );
        assert_eq!(kept_ids(&f), ["relay-inbox", "relay-tasks"]);
        assert_eq!(dropped_ids(&f), ["relay-wake"]);
    }

    #[test]
    fn a_block_larger_than_the_budget_is_dropped_whole_and_never_cut() {
        let wake = block("relay-wake", Priority::Relay, 3400);
        let wake_text = wake.text.clone();
        let f = fit(HEADER, blocks(vec![wake, block("relay-tasks", Priority::Relay, 200)]), 3000, KEY);
        assert_eq!(dropped_ids(&f), ["relay-wake"]);
        assert!(!f.text.contains(&wake_text[..40]), "no part of the dropped block is printed");
        assert!(f.text.contains("· full text: base hooks show relay-wake]"));
        assert!(f.text.len() <= 3000);
        assert_whole(&f);
    }

    /// A block no longer than its own pointer line is never dropped: the pointer would cost more.
    #[test]
    fn a_block_shorter_than_its_pointer_line_is_kept() {
        let f = fit(
            HEADER,
            blocks(vec![
                block("hooks-rules", Priority::Matched, 3700),
                block("grounding", Priority::Global, 60),
            ]),
            3900,
            KEY,
        );
        assert_eq!(kept_ids(&f), ["hooks-rules", "grounding"]);
        assert!(f.text.len() <= 3900);
    }

    /// When even the pointer lines do not fit, they become one line, and blocks are dropped from the
    /// bottom until that fits. Nothing is cut.
    #[test]
    fn pointer_lines_that_do_not_fit_become_one_line() {
        let list: Vec<PromptBlock> =
            (0..12).map(|i| block(&format!("walk-name-{i}"), Priority::Context, 300)).collect();
        let f = fit(HEADER, blocks(list), 700, KEY);
        assert!(f.text.len() <= 700, "{} bytes:\n{}", f.text.len(), f.text);
        let lines: Vec<&str> = f.text.lines().filter(|l| l.starts_with("[base: withheld")).collect();
        assert_eq!(lines.len(), 1, "one line for every dropped block:\n{}", f.text);
        assert!(lines[0].contains(&format!("{} blocks", f.dropped_blocks().count())));
        for b in f.kept_blocks() {
            assert!(f.text.contains(&b.text), "{} is printed whole", b.id);
        }
    }

    #[test]
    fn ids_are_unique_and_empty_blocks_add_nothing() {
        let mut b = PromptBlocks::new();
        b.push(PromptBlock::new("walk-x", Priority::Context, "<base-context>a</base-context>", 1, "record"));
        b.push(PromptBlock::new("walk-x", Priority::Context, "<base-context>b</base-context>", 1, "record"));
        b.push(PromptBlock::new("grounding", Priority::Global, "\n\n", 1, "instruction"));
        let ids: Vec<&str> = b.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["walk-x", "walk-x-2"]);
    }

    #[test]
    fn thousands_groups_by_three() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1150), "1,150");
        assert_eq!(thousands(202_278), "202,278");
        assert_eq!(thousands(4_090_000), "4,090,000");
    }

    #[test]
    fn show_prints_one_block_from_the_sessions_own_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = fit(
            HEADER,
            blocks(vec![block("hooks-rules", Priority::Matched, 3000), block("global-rules", Priority::Global, 2000)]),
            4000,
            KEY,
        );
        assert!(write_blocks(dir.path(), "sid-a", &f).written_path().is_some());
        let other = fit(HEADER, blocks(vec![block("global-rules", Priority::Global, 10)]), 4000, KEY);
        assert!(write_blocks(dir.path(), "sid-b", &other).written_path().is_some());

        let shown = show(dir.path(), Some("sid-a"), Some("global-rules")).expect("the block");
        let dropped = f.dropped_blocks().next().expect("global-rules was dropped");
        assert_eq!(shown.stdout, format!("{}\n", dropped.text), "session a's own block, exactly");
        assert_eq!(shown.note, None);
        let listed = show(dir.path(), Some("sid-a"), None).expect("the list");
        assert!(listed.stdout.contains("global-rules  priority 4  1 item  2,000 bytes  withheld"), "{}", listed.stdout);
        let err = show(dir.path(), Some("sid-a"), Some("nope")).unwrap_err();
        assert!(err.contains("hooks-rules, global-rules"), "{err}");
        assert!(show(dir.path(), Some("../x"), Some("a")).is_err(), "a path is never a session id");
        assert!(write_blocks(dir.path(), "../x", &f).written_path().is_none());
    }
}
