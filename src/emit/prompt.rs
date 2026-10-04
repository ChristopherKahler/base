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
//! injected, and a bracket rule as sent only when the block carrying that record was printed.
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
    /// One bracket rule was shown, once for the session (BO-03, F3): `SessionState::mark_rule_shown` under its
    /// `session::bracket_rule_key`.
    BracketRule { id: String, content: u64 },
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
    /// The rules and decisions in the block, for the match log (K1, BO-13): served when the block is printed, cut for
    /// the budget when it is dropped. Never read for the output.
    pub logged: Vec<super::match_log::Item>,
    /// A RANKED block's rules or decisions, one part each, best first (BO-18, K7d): the fit withholds a ranked block's
    /// parts lowest score first, and the block prints its head, the parts it kept, its tail and one line naming what it
    /// withheld. Empty for every other block, which is printed whole or dropped whole, as before.
    pub parts: Vec<BlockPart>,
    /// A ranked block's lines before its parts (the header, steering lines).
    pub head: String,
    /// A ranked block's lines after its parts.
    pub tail: String,
    /// Built by the matching (`user_prompt_submit::serve`, BO-20): a shadow candidate's fit puts its own in place of
    /// these. Never read for the output.
    pub matcher: bool,
}

/// One rule or decision of a ranked block: its line, its score, and what printing it records (D15) and logs (K1).
#[derive(Debug, Clone)]
pub struct BlockPart {
    pub text: String,
    pub score: f32,
    pub claims: Vec<Claim>,
    pub logged: Vec<super::match_log::Item>,
}

impl BlockPart {
    pub fn new(text: &str, score: f32) -> Self {
        BlockPart { text: text.trim_matches(['\r', '\n']).to_string(), score, claims: Vec::new(), logged: Vec::new() }
    }

    pub fn with_claims(mut self, claims: impl IntoIterator<Item = Claim>) -> Self {
        self.claims.extend(claims);
        self
    }

    pub fn with_logged(mut self, items: impl IntoIterator<Item = super::match_log::Item>) -> Self {
        self.logged.extend(items);
        self
    }

    pub fn bytes(&self) -> usize {
        self.text.len()
    }
}

/// Lines joined one per line, empty ones left out.
fn lines<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts.into_iter().map(|s| s.trim_matches(['\r', '\n'])).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
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
            logged: Vec::new(),
            parts: Vec::new(),
            head: String::new(),
            tail: String::new(),
            matcher: false,
        }
    }

    /// A ranked block (BO-18): `head`, then `parts` in the order given (the caller puts the best first), then `tail`.
    /// Its text is all of them; `items` counts the parts. With no parts it is a plain block of `head` and `tail`.
    pub fn ranked(
        id: impl Into<String>,
        priority: Priority,
        head: &str,
        parts: Vec<BlockPart>,
        tail: &str,
        noun: &'static str,
    ) -> Self {
        let text = lines(std::iter::once(head).chain(parts.iter().map(|p| p.text.as_str())).chain(std::iter::once(tail)));
        let items = parts.len();
        let mut b = PromptBlock::new(id, priority, &text, items, noun);
        if !b.text.is_empty() && !parts.is_empty() {
            b.head = head.trim_matches(['\r', '\n']).to_string();
            b.tail = tail.trim_matches(['\r', '\n']).to_string();
            b.parts = parts;
        }
        b
    }

    pub fn is_ranked(&self) -> bool {
        !self.parts.is_empty()
    }

    /// Printing it records something: a claim of its own or of one of its parts.
    pub fn has_claims(&self) -> bool {
        !self.claims.is_empty() || self.parts.iter().any(|p| !p.claims.is_empty())
    }

    /// This ranked block as printed with only the parts `kept` marks: its head, those parts, its tail, and, when any
    /// part is withheld, [`partial_line`]. With every part kept, its text.
    fn render_kept(&self, kept: &[bool], key: &str, budget_bytes: usize) -> String {
        let withheld: Vec<&BlockPart> = self.parts.iter().zip(kept).filter(|(_, k)| !**k).map(|(p, _)| p).collect();
        if withheld.is_empty() {
            return self.text.clone();
        }
        let bytes: usize = withheld.iter().map(|p| p.bytes()).sum();
        let line = partial_line(self, withheld.len(), bytes, key, budget_bytes);
        lines(
            std::iter::once(self.head.as_str())
                .chain(self.parts.iter().zip(kept).filter(|(_, k)| **k).map(|(p, _)| p.text.as_str()))
                .chain([self.tail.as_str(), line.as_str()]),
        )
    }

    pub fn with_claims(mut self, claims: impl IntoIterator<Item = Claim>) -> Self {
        self.claims.extend(claims);
        self
    }

    pub fn with_logged(mut self, items: impl IntoIterator<Item = super::match_log::Item>) -> Self {
        self.logged.extend(items);
        self
    }

    pub fn bytes(&self) -> usize {
        self.text.len()
    }
}

/// The blocks of one prompt, in the order the hook built them.
#[derive(Debug, Default, Clone)]
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

    /// [`PromptBlocks::push`], first among the blocks of its priority: the fit keeps push order within a priority, so
    /// this block prints before the others of its rank (BO-15's correction check reads before the rules it is about).
    pub fn push_front(&mut self, block: PromptBlock) {
        let before = self.blocks.len();
        self.push(block);
        if self.blocks.len() > before
            && let Some(b) = self.blocks.pop()
        {
            self.blocks.insert(0, b);
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

/// The line a partly printed ranked block ends with (BO-18): [`pointer_line`]'s shape, with how many of its parts it
/// withheld out of how many, and their bytes (lynx's G0 verdict, condition 1).
pub fn partial_line(block: &PromptBlock, withheld: usize, bytes: usize, key: &str, budget_bytes: usize) -> String {
    format!(
        "[base: withheld {withheld} of {} {} ({} bytes) over [budget] {key} = {budget_bytes} · full text: {}]",
        block.parts.len(),
        block.id,
        thousands(bytes),
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
    /// Per block in [`Fitted::blocks`]: printed, whole or in part (true), or dropped whole (false).
    pub kept: Vec<bool>,
    /// Per block: which of a ranked block's parts were printed. Empty for every other block.
    pub parts_kept: Vec<Vec<bool>>,
    pub budget_bytes: usize,
    /// The budget's key as the pointer lines name it.
    pub key: String,
}

impl Fitted {
    pub fn emitted_bytes(&self) -> usize {
        self.text.len()
    }

    pub fn full_bytes(&self) -> usize {
        self.full_text.len()
    }

    /// The blocks printed, whole or in part.
    pub fn kept_blocks(&self) -> impl Iterator<Item = &PromptBlock> {
        self.blocks.iter().zip(&self.kept).filter(|(_, k)| **k).map(|(b, _)| b)
    }

    /// The blocks dropped whole.
    pub fn dropped_blocks(&self) -> impl Iterator<Item = &PromptBlock> {
        self.blocks.iter().zip(&self.kept).filter(|(_, k)| !**k).map(|(b, _)| b)
    }

    /// The ranked blocks printed with some parts withheld: each block, how many parts it withheld, and their bytes.
    pub fn partial_blocks(&self) -> impl Iterator<Item = (&PromptBlock, usize, usize)> {
        self.blocks.iter().enumerate().filter(|(i, _)| self.kept[*i]).filter_map(|(i, b)| {
            let withheld: Vec<&BlockPart> =
                b.parts.iter().zip(&self.parts_kept[i]).filter(|(_, k)| !**k).map(|(p, _)| p).collect();
            (!withheld.is_empty()).then(|| (b, withheld.len(), withheld.iter().map(|p| p.bytes()).sum()))
        })
    }

    /// Block `i` exactly as printed, or `None` when it was dropped whole.
    pub fn printed_text(&self, i: usize) -> Option<String> {
        let b = self.blocks.get(i)?;
        if !self.kept[i] {
            return None;
        }
        Some(if b.is_ranked() { b.render_kept(&self.parts_kept[i], &self.key, self.budget_bytes) } else { b.text.clone() })
    }

    /// What the printed output records (D15): the claims of every printed block, and of the parts a ranked block
    /// printed. A withheld part's rule stays due.
    pub fn printed_claims(&self) -> Vec<&Claim> {
        let mut out = Vec::new();
        for (i, b) in self.blocks.iter().enumerate().filter(|(i, _)| self.kept[*i]) {
            out.extend(b.claims.iter());
            out.extend(b.parts.iter().zip(&self.parts_kept[i]).filter(|(_, k)| **k).flat_map(|(p, _)| p.claims.iter()));
        }
        out
    }

    /// Every item the blocks log (K1), with its block and whether it was printed: a block's own items with the block,
    /// a part's with the part.
    pub fn logged(&self) -> Vec<(&PromptBlock, &super::match_log::Item, bool)> {
        let mut out = Vec::new();
        for (i, b) in self.blocks.iter().enumerate() {
            out.extend(b.logged.iter().map(|item| (b, item, self.kept[i])));
            for (p, k) in b.parts.iter().zip(&self.parts_kept[i]) {
                out.extend(p.logged.iter().map(|item| (b, item, self.kept[i] && *k)));
            }
        }
        out
    }

    /// The bytes of every dropped block and of every withheld part.
    pub fn withheld_bytes(&self) -> usize {
        self.dropped_blocks().map(PromptBlock::bytes).sum::<usize>() + self.partial_blocks().map(|(_, _, b)| b).sum::<usize>()
    }

    /// Something was dropped or withheld.
    pub fn lost(&self) -> bool {
        self.kept.iter().any(|k| !k) || self.partial_blocks().next().is_some()
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
///    still kept are dropped from the bottom until that fits; then step 3 runs again, since the one
///    line is far shorter than the pointer lines it replaced.
///
/// RANKED BLOCKS (BO-18, K7d, lynx's G0 verdict on Q1). A rule is the unit that is never cut. Within one priority the
/// ranked blocks act as one unit at the place of the first of them, and inside that unit the fit withholds parts, the
/// lowest score first, one at a time; a ranked block with every part withheld is dropped whole and leaves its pointer
/// line. Put back best first. The priority order (F2) is untouched: ranking only reorders and sheds within a priority.
/// Output with no ranked block is what it was before, byte for byte.
pub fn fit(header: &str, blocks: PromptBlocks, budget_bytes: usize, key: &str) -> Fitted {
    let header = header.trim_matches(['\r', '\n']);
    let mut blocks = blocks.blocks;
    blocks.sort_by_key(|b| b.priority);
    let n = blocks.len();
    let units = drop_order(&blocks);
    let mut state = FitState { kept: vec![true; n], parts: blocks.iter().map(|b| vec![true; b.parts.len()]).collect() };

    let (text, full_text) = {
        let pointers: Vec<String> = blocks.iter().map(|b| pointer_line(b, key, budget_bytes)).collect();
        // Block `i` as `s` prints it, or `None` when it is dropped whole.
        let piece = |s: &FitState, i: usize| -> Option<String> {
            if blocks[i].is_ranked() {
                s.parts[i].iter().any(|k| *k).then(|| blocks[i].render_kept(&s.parts[i], key, budget_bytes))
            } else {
                s.kept[i].then(|| blocks[i].text.clone())
            }
        };
        let render = |s: &FitState| -> String {
            let pieces: Vec<String> = (0..n).map(|i| piece(s, i).unwrap_or_else(|| pointers[i].clone())).collect();
            compose(header, pieces.iter().map(String::as_str))
        };
        let render_aggregate = |s: &FitState| -> String {
            let dropped: Vec<&PromptBlock> = (0..n).filter(|i| piece(s, *i).is_none()).map(|i| &blocks[i]).collect();
            let line = (!dropped.is_empty()).then(|| aggregate_line(&dropped, key, budget_bytes));
            let pieces: Vec<String> = (0..n).filter_map(|i| piece(s, i)).collect();
            compose(header, pieces.iter().map(String::as_str).chain(line.as_deref()))
        };

        // Put back, highest priority first (the best part first), every dropped block or withheld part that fits the
        // room left. Nothing kept is dropped for it.
        let readmit = |s: &mut FitState, text: &mut String, render: &dyn Fn(&FitState) -> String| {
            for &u in units.iter().rev() {
                if s.is_kept(u) {
                    continue;
                }
                s.set(u, true);
                let trial = render(s);
                if trial.len() <= budget_bytes {
                    *text = trial;
                } else {
                    s.set(u, false);
                }
            }
        };

        let full_text = render(&state);
        let mut text = full_text.clone();
        if text.len() > budget_bytes {
            // A rule withheld from a ranked block costs the line that names it (or, its last, the block's pointer
            // line), which can be longer than the rule. Withheld alone it would grow the output and push out a block of
            // a higher priority (F2). So, as a block no longer than its pointer line is never dropped, a rule is
            // withheld only in a run that shortens the output: it and, while the output is no shorter, the next rules
            // of the same priority, lowest score first (two short rules of one block go together, and the block leaves
            // its pointer line). A run that never gets shorter is put back and the next rule tried.
            let mut k = 0;
            while k < units.len() && text.len() > budget_bytes {
                let u = units[k];
                k += 1;
                match u {
                    Unit::Block(i) if blocks[i].bytes() <= pointers[i].len() => {}
                    Unit::Block(_) => {
                        state.set(u, false);
                        text = render(&state);
                    }
                    Unit::Part(b, _) => {
                        let priority = blocks[b].priority;
                        let mut run = vec![u];
                        state.set(u, false);
                        let mut trial = render(&state);
                        let mut next = k;
                        while trial.len() >= text.len() {
                            let Some(&Unit::Part(nb, nj)) = units.get(next) else { break };
                            if blocks[nb].priority != priority {
                                break;
                            }
                            state.set(Unit::Part(nb, nj), false);
                            run.push(Unit::Part(nb, nj));
                            trial = render(&state);
                            next += 1;
                        }
                        if trial.len() < text.len() {
                            text = trial;
                            k = next;
                        } else {
                            for x in run {
                                state.set(x, true);
                            }
                        }
                    }
                }
            }
            if text.len() <= budget_bytes {
                readmit(&mut state, &mut text, &render);
            } else {
                text = render_aggregate(&state);
                for &u in &units {
                    if text.len() <= budget_bytes {
                        break;
                    }
                    if state.is_kept(u) {
                        state.set(u, false);
                        text = render_aggregate(&state);
                    }
                }
                // The one line is far shorter than the pointer lines it replaced, so blocks may fit again.
                if text.len() <= budget_bytes {
                    readmit(&mut state, &mut text, &render_aggregate);
                }
            }
        }
        (text, full_text)
    };
    let kept: Vec<bool> =
        (0..n).map(|i| if blocks[i].is_ranked() { state.parts[i].iter().any(|k| *k) } else { state.kept[i] }).collect();
    Fitted { text, full_text, blocks, kept, parts_kept: state.parts, budget_bytes, key: key.to_string() }
}

/// What the fit drops or withholds, one at a time: a plain block, or one part of a ranked block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Block(usize),
    Part(usize, usize),
}

/// Which blocks and parts the fit has kept so far.
struct FitState {
    kept: Vec<bool>,
    parts: Vec<Vec<bool>>,
}

impl FitState {
    fn is_kept(&self, u: Unit) -> bool {
        match u {
            Unit::Block(i) => self.kept[i],
            Unit::Part(i, j) => self.parts[i][j],
        }
    }

    fn set(&mut self, u: Unit, kept: bool) {
        match u {
            Unit::Block(i) => self.kept[i] = kept,
            Unit::Part(i, j) => self.parts[i][j] = kept,
        }
    }
}

/// The order the fit drops in, first dropped first, over blocks already in priority order: the lowest priority first,
/// within one priority the last pushed first, and the ranked blocks of a priority as one unit at the place of the first
/// of them, their parts lowest score first (ties: the later block, then the later part, goes first). With no ranked
/// block this is every block from the last to the first, as before.
fn drop_order(blocks: &[PromptBlock]) -> Vec<Unit> {
    let mut out = Vec::new();
    let mut end = blocks.len();
    while end > 0 {
        let p = blocks[end - 1].priority;
        let mut start = end - 1;
        while start > 0 && blocks[start - 1].priority == p {
            start -= 1;
        }
        let ranked: Vec<usize> = (start..end).filter(|&k| blocks[k].is_ranked()).collect();
        for k in (start..end).rev() {
            if !blocks[k].is_ranked() {
                out.push(Unit::Block(k));
            } else if ranked.first() == Some(&k) {
                let mut parts: Vec<(usize, usize)> =
                    ranked.iter().flat_map(|&b| (0..blocks[b].parts.len()).map(move |j| (b, j))).collect();
                parts.sort_by(|x, y| {
                    let (sx, sy) = (blocks[x.0].parts[x.1].score, blocks[y.0].parts[y.1].score);
                    sx.total_cmp(&sy).then(y.cmp(x))
                });
                out.extend(parts.into_iter().map(|(b, j)| Unit::Part(b, j)));
            }
        }
        end = start;
    }
    out
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
    /// A ranked block printed with parts withheld (BO-18): how many it withheld, their bytes, and the text as printed.
    /// Absent on every other block, so a file with none reads as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    withheld_items: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    withheld_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    printed_text: Option<String>,
}

/// Keep `fitted`'s blocks for `session_id`, so `base hooks show <block>` can print one, and drop the
/// files of sessions not written for [`KEEP_DAYS`]. Written on every prompt, so a block the last
/// prompt did not build is never shown as current. A failure comes back as a value, never a panic.
pub fn write_blocks(base: &Path, session_id: &str, fitted: &Fitted) -> FullOutput {
    if !crate::crud::handoff_show::is_file_safe_session_id(session_id) {
        return FullOutput::not_written(format!("session id {session_id:?} is not a file name"));
    }
    let root = base.join(SESSION_DIR);
    // Old sessions are swept when a NEW session writes its first file, not on every prompt: the hook's hot path pays
    // one existence check, and the sweep runs about once per session.
    if !root.join(session_id).join(BLOCKS_FILE).exists() {
        prune(&root);
    }
    let file = BlocksFile {
        written_at: crate::crud::now_iso(),
        session_id: session_id.to_string(),
        budget_bytes: fitted.budget_bytes,
        blocks: fitted
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let partial = fitted.partial_blocks().find(|(p, _, _)| std::ptr::eq(*p, b));
                BlockRow {
                    id: b.id.clone(),
                    priority: b.priority.number(),
                    items: b.items,
                    noun: b.noun.to_string(),
                    bytes: b.bytes(),
                    printed: fitted.kept[i],
                    text: b.text.clone(),
                    withheld_items: partial.map(|(_, n, _)| n),
                    withheld_bytes: partial.map(|(_, _, bytes)| bytes),
                    printed_text: partial.and_then(|_| fitted.printed_text(i)),
                }
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
                match (b.printed, b.withheld_items) {
                    (true, Some(w)) => format!("printed, {w} of {} withheld", b.items),
                    (true, None) => "printed".to_string(),
                    (false, _) => "withheld".to_string(),
                }
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

    /// Code review (2026-10-01): the one-line mode used to stop there, although its line is far shorter than the
    /// pointer lines it replaced. The matched block it freed room for comes back.
    #[test]
    fn the_one_line_mode_puts_back_what_fits() {
        let mut list = vec![block("hooks-rules", Priority::Matched, 400)];
        list.extend((0..12).map(|i| block(&format!("walk-name-{i}"), Priority::Context, 300)));
        let f = fit(HEADER, blocks(list), 1000, KEY);
        assert!(f.text.len() <= 1000, "{} bytes:\n{}", f.text.len(), f.text);
        assert_eq!(f.text.lines().filter(|l| l.starts_with("[base: withheld")).count(), 1, "one line:\n{}", f.text);
        assert_eq!(kept_ids(&f).first(), Some(&"hooks-rules"), "the matched block was put back:\n{}", f.text);
    }

    /// A ranked block of `scores.len()` rules, each line `bytes` long, a claim and a logged item per rule.
    fn ranked(id: &str, p: Priority, scores: &[f32], bytes: usize) -> PromptBlock {
        let parts = scores
            .iter()
            .enumerate()
            .map(|(j, s)| {
                let line: String = format!("  {j}. {id} rule scored {s} ").chars().chain(std::iter::repeat('r')).take(bytes).collect();
                BlockPart::new(&line, *s)
                    .with_claims([Claim::Rule { id: format!("{id}-{j}"), content: 0, scope: None }])
                    .with_logged([super::super::match_log::Item::rule(&format!("{id}-{j}"), id)])
            })
            .collect();
        PromptBlock::ranked(id, p, &format!("[DOMAIN: {id}]"), parts, "", "rule")
    }

    fn printed_rule_ids(f: &Fitted) -> Vec<String> {
        f.printed_claims()
            .into_iter()
            .filter_map(|c| match c {
                Claim::Rule { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect()
    }

    /// BO-18 (K7d, lynx's G0 Q1): a ranked block that does not fit keeps its best rules and withholds the rest, lowest
    /// score first, ending with one line in the pointer line's shape; only the printed rules are recorded as shown
    /// (D15) and logged as served.
    #[test]
    fn a_ranked_block_sheds_its_lowest_rule_first_and_names_what_it_withheld() {
        let b = ranked("tools-rules", Priority::Matched, &[5.0, 4.0, 3.0, 2.0, 1.0], 300);
        let full = b.text.len();
        let f = fit(HEADER, blocks(vec![b]), 1300, KEY);
        assert!(f.text.len() <= 1300, "{} bytes:\n{}", f.text.len(), f.text);
        assert!(full > 1300, "control: the block alone is over the budget");
        assert_eq!(printed_rule_ids(&f), ["tools-rules-0", "tools-rules-1", "tools-rules-2"], "the three best:\n{}", f.text);
        assert_eq!(kept_ids(&f), ["tools-rules"], "printed in part, not dropped");
        let last = f.text.trim_end().lines().last().unwrap_or_default();
        assert_eq!(
            last,
            "[base: withheld 2 of 5 tools-rules (600 bytes) over [budget] prompt_bytes = 1300 · full text: base hooks show tools-rules]"
        );
        assert!(f.text.contains("  0. tools-rules") && !f.text.contains("  3. tools-rules"), "{}", f.text);
        let (_, n, bytes) = f.partial_blocks().next().expect("one partial block");
        assert_eq!((n, bytes), (2, 600));
        assert_eq!(f.withheld_bytes(), 600);
        assert!(f.lost());
        let served: Vec<&str> = f.logged().into_iter().filter(|(_, _, p)| *p).map(|(_, i, _)| i.id.as_str()).collect();
        assert_eq!(served, ["tools-rules-0", "tools-rules-1", "tools-rules-2"]);
        assert_eq!(f.printed_text(0).as_deref(), Some(f.text.split_once("\n\n").map(|x| x.1.trim_end()).unwrap_or_default()));
    }

    /// The ranked blocks of one priority shed as one unit, by score across blocks; a plain block pushed after the first
    /// of them still goes before them, one pushed before them after them (today's order otherwise).
    #[test]
    fn ranked_rules_of_one_priority_go_lowest_score_first_across_blocks() {
        let f = fit(
            HEADER,
            blocks(vec![
                block("check-line", Priority::Matched, 300),
                ranked("a-rules", Priority::Matched, &[9.0, 2.0], 300),
                ranked("b-rules", Priority::Matched, &[5.0, 1.0], 300),
                block("linked-mode", Priority::Matched, 300),
            ]),
            1500,
            KEY,
        );
        assert!(f.text.len() <= 1500, "{} bytes:\n{}", f.text.len(), f.text);
        assert_eq!(dropped_ids(&f), ["linked-mode"], "pushed after the first ranked block: dropped before any rule");
        assert!(kept_ids(&f).contains(&"check-line"), "pushed before them: kept longest");
        let score = |id: &str| match id {
            "a-rules-0" => 9.0,
            "b-rules-0" => 5.0,
            "a-rules-1" => 2.0,
            _ => 1.0,
        };
        let printed = printed_rule_ids(&f);
        let withheld: Vec<&str> =
            ["a-rules-0", "a-rules-1", "b-rules-0", "b-rules-1"].into_iter().filter(|id| !printed.iter().any(|p| p == id)).collect();
        assert!(withheld.len() >= 2, "control: rules from both blocks were withheld:\n{}", f.text);
        for p in &printed {
            for w in &withheld {
                assert!(score(p) >= score(w), "{p} printed and {w} withheld, against their scores:\n{}", f.text);
            }
        }
    }

    /// F2 stands: a lower priority's best rule goes before a higher priority's worst.
    #[test]
    fn ranking_never_crosses_priorities() {
        let f = fit(
            HEADER,
            blocks(vec![
                ranked("global-rules", Priority::Global, &[50.0, 40.0], 400),
                ranked("tools-rules", Priority::Matched, &[0.5, 0.1], 400),
            ]),
            1100,
            KEY,
        );
        assert!(f.text.len() <= 1100, "{} bytes:\n{}", f.text.len(), f.text);
        assert_eq!(printed_rule_ids(&f), ["tools-rules-0", "tools-rules-1"], "{}", f.text);
        assert_eq!(dropped_ids(&f), ["global-rules"], "every part withheld: dropped whole, with its pointer line");
        assert!(f.text.contains("[base: withheld global-rules (2 rules, "), "{}", f.text);
    }

    /// A short rule whose withholding would cost more than it saves (the block's pointer line is longer than the block)
    /// is not withheld, so the fit drops the next block instead of a higher-priority one (F2). Before this guard the
    /// fit withheld it, dropped the middle block, still did not fit, dropped the top block, and the readmission could
    /// not put the top block back while the lower ones printed.
    #[test]
    fn a_rule_shorter_than_its_pointer_line_is_not_withheld_over_a_higher_block() {
        let top = block("top-rules", Priority::Matched, 1000);
        let mid = block("mid-context", Priority::Context, 400);
        let low = ranked("low-rules", Priority::Relay, &[1.0], 30);
        let full = fit(HEADER, blocks(vec![top.clone(), mid.clone(), low.clone()]), usize::MAX, KEY).full_text.len();
        let over = 266;
        let budget = full - over;
        let mid_saves = mid.bytes() - pointer_line(&mid, KEY, budget).len();
        let low_costs = pointer_line(&low, KEY, budget).len() - low.bytes();
        assert!(over > mid_saves - low_costs && over < mid_saves, "control: {mid_saves} saved, {low_costs} cost, {over} over");
        let f = fit(HEADER, blocks(vec![top, mid, low]), budget, KEY);
        assert!(f.text.len() <= budget, "{} bytes:\n{}", f.text.len(), f.text);
        assert_eq!(kept_ids(&f), ["top-rules", "low-rules"], "{}", f.text);
        assert_eq!(dropped_ids(&f), ["mid-context"]);
        assert_eq!(printed_rule_ids(&f), ["low-rules-0"]);
    }

    /// Two rules each shorter than the line naming a withheld rule: withheld one at a time neither shortens the output,
    /// withheld together their block leaves its pointer line, which does. They go together, and the higher priority's
    /// block stays whole (a guard that judged each rule alone kept both and cut the higher block instead).
    #[test]
    fn short_rules_of_one_block_are_withheld_together() {
        let top = ranked("top-rules", Priority::Matched, &[2.0, 1.0], 400);
        let low = ranked("low-rules", Priority::Global, &[2.0, 1.0], 70);
        let full = fit(HEADER, blocks(vec![top.clone(), low.clone()]), usize::MAX, KEY).full_text.len();
        let budget = full - (low.bytes() - pointer_line(&low, KEY, full).len());
        assert!(low.parts.iter().all(|p| p.bytes() < 110), "control: each low rule is shorter than a withheld-rules line");
        let f = fit(HEADER, blocks(vec![top, low]), budget, KEY);
        assert!(f.text.len() <= budget, "{} bytes:\n{}", f.text.len(), f.text);
        assert_eq!(dropped_ids(&f), ["low-rules"], "{}", f.text);
        assert_eq!(printed_rule_ids(&f), ["top-rules-0", "top-rules-1"], "the higher block whole:\n{}", f.text);
    }

    /// Readmission puts the best withheld rule back first: dropping a large low rule makes room a better one fills.
    #[test]
    fn readmission_puts_the_best_withheld_rule_back_first() {
        let mut list = ranked("tools-rules", Priority::Matched, &[3.0, 2.0, 1.0], 200);
        list.parts[2].text = format!("{}{}", list.parts[2].text, "x".repeat(800));
        let list = PromptBlock::ranked("tools-rules", Priority::Matched, "[DOMAIN: tools-rules]", list.parts, "", "rule");
        let f = fit(HEADER, blocks(vec![list]), 700, KEY);
        assert!(f.text.len() <= 700, "{} bytes:\n{}", f.text.len(), f.text);
        assert_eq!(printed_rule_ids(&f), ["tools-rules-0", "tools-rules-1"], "{}", f.text);
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
