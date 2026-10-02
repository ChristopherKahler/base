//! The AST hint (F20, BO-07): a code search is pointed at the code map that can answer it, and nothing else is.
//!
//! Measured in session 5b860473 on 2026-10-01: base's pre-tool hook put an `<ast-hint>` on 45 of the 119 tool calls
//! it saw, and almost none were code searches. It fired on any command that piped into `grep` (`base handoff list |
//! grep 0160`), on searches over TOML and markdown, and in folders no map covers, because it checked the session's
//! cwd for a map instead of the folder searched. Its suggested query was the raw pattern (`--contains "\[budget\]"`,
//! `--contains "^paths|Documents"`), which the map cannot look up.
//!
//! The rule, in order:
//!   * Only a search program fires: grep (egrep, fgrep), rg, ag, ack, find, fd and Select-String. Never `ls`, `cat`,
//!     `head`, `base`, `git`, `cargo` or anything else (F20b), and never a search that filters another command's
//!     output (`… | grep x`): that searches text, not files.
//!   * What is searched decides (F20a, F20b): the files named, the `--include` / `-g` / `-t` / `-name` filters, and
//!     the folders. A search confined to docs, config or data (`.md`, `.toml`, `.json`, …) never fires. A search over
//!     source files, or over a folder inside an app with a code map, does; a folder named bare counts only when it
//!     holds source files, so a folder of markdown under a mapped tier is a docs search.
//!   * The map is the searched folder's, never the cwd's. When it is not the map `base ast query` reads from the cwd,
//!     the suggestion carries `--target`.
//!   * The suggested query is a plain name (F20c): regex syntax stripped, the longest name-like token kept.
//!   * With no map, only a search that names source files is told so, and the text is true for that folder (BO-00's
//!     review, lynx): a build is under way, it has been failing, the tree is too large to map unattended, or base
//!     never maps that place on its own. It is said once per session per app. A folder that is in no app hears
//!     nothing.
//!
//! Parsing is shell-shaped, not a shell: quotes, escapes, pipes, list operators, redirections, heredoc bodies and
//! `cd` are followed; variables and command substitutions are never expanded, so a target spelled with one is
//! treated as unknown.

use std::path::{Path, PathBuf};

use crate::domain::session::SessionState;
use crate::hook::automap::{self, MapPlan};

/// Which shell's quoting a command is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// POSIX-style: Claude Code's Bash tool (Git Bash on Windows) and context-mode's shell commands.
    Bash,
    /// The PowerShell tool: backtick escapes, and a backslash is a path separator.
    PowerShell,
}

/// What a search looks through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// File contents (grep, rg, ag, ack, Select-String): the query is `--contains`.
    Content,
    /// File names (find, fd): the query is `--file`.
    Names,
}

/// A code search, resolved: everything the hint needs and nothing it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSearch {
    pub kind: Kind,
    /// The query to suggest, already a plain name (F20c). `None` when the pattern has no name-like token.
    pub name: Option<String>,
    /// The folders searched, in the order named; the cwd (or the last `cd`) when the search names none.
    pub folders: Vec<Searched>,
    /// The search names source files: by extension, or through a filter that admits only code.
    pub names_code: bool,
}

/// One folder a search reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Searched {
    pub folder: PathBuf,
    /// Known to be a search over source here: a source file was named in it, or a filter admits only code. A folder
    /// named bare is checked for source files before it counts (F20b: a folder of docs is a docs search).
    pub holds_code: bool,
}

/// The `<ast-hint>` for a tool call, and whether `session` was marked.
#[derive(Debug, Default)]
pub struct Hint {
    pub text: Option<String>,
    pub marked: bool,
}

/// The `<ast-hint>` for this tool call, if it has one. Reads the filesystem (which folders have a map, whether a bare
/// folder holds source files) and, for a code search in an app with no map, starts that app's first map the way any
/// first contact does, so that the text saying a build is under way is true. A no-map text is said once per session
/// per app, and again only if it changes: session 5b860473 searched one worktree eight times, and eight copies of
/// the same 330 bytes told it nothing new after the first.
pub fn hint(event: &serde_json::Value, cwd: &Path, session: &mut SessionState) -> Hint {
    let home = crate::home::home_root();
    for (command, shell) in tool_commands(event) {
        for search in code_searches(&command, shell, cwd, home.as_deref()) {
            match hint_for(&search, cwd) {
                Some(Said::Mapped(text)) => return Hint { text: Some(text), marked: false },
                Some(Said::Unmapped { root, text }) => {
                    let key = format!("ast-hint-no-map{}{}", '\u{1f}', root.display());
                    let version = crate::domain::session::rules_hash(std::slice::from_ref(&text));
                    if !session.has_ast_injected(&key, version) {
                        session.mark_ast_injected(&key, version);
                        return Hint { text: Some(text), marked: true };
                    }
                }
                None => {}
            }
        }
    }
    Hint::default()
}

/// The shell commands a tool call is about to run, each with the shell that parses it: Bash's and PowerShell's
/// `command`, context-mode's batch `commands[].command`, and `ctx_execute` code written in a shell. Every other tool
/// runs no command, including `ctx_execute_file`, which reads one file (as `cat` does).
pub fn tool_commands(event: &serde_json::Value) -> Vec<(String, Shell)> {
    let tool = event.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    let Some(input) = event.get("tool_input") else {
        return Vec::new();
    };
    let text = |key: &str| input.get(key).and_then(|v| v.as_str()).map(str::to_string);
    match tool {
        "Bash" => text("command").map(|c| vec![(c, Shell::Bash)]).unwrap_or_default(),
        "PowerShell" => text("command").map(|c| vec![(c, Shell::PowerShell)]).unwrap_or_default(),
        t if t.ends_with("ctx_batch_execute") => input
            .get("commands")
            .and_then(|v| v.as_array())
            .map(|cmds| {
                cmds.iter()
                    .filter_map(|c| c.get("command").and_then(|v| v.as_str()))
                    .map(|c| (c.to_string(), Shell::Bash))
                    .collect()
            })
            .unwrap_or_default(),
        t if t.ends_with("ctx_execute") => {
            let shell = match text("language").unwrap_or_default().to_ascii_lowercase().as_str() {
                "shell" | "bash" | "sh" | "zsh" => Shell::Bash,
                "powershell" | "pwsh" => Shell::PowerShell,
                _ => return Vec::new(),
            };
            text("code").map(|c| vec![(c, shell)]).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// Every code search in `command`, in order. Relative paths resolve against `cwd` until a `cd` moves it; `~` is
/// `home`. Pure apart from `is_dir` / `is_file` checks on the paths named.
pub fn code_searches(command: &str, shell: Shell, cwd: &Path, home: Option<&Path>) -> Vec<CodeSearch> {
    let simples = split(command, shell);
    let mut base = cwd.to_path_buf();
    let mut out = Vec::new();
    for (k, simple) in simples.iter().enumerate() {
        let Some(head) = program(&simple.words, shell) else {
            continue;
        };
        let args = &simple.words[head.index + 1..];
        if is_cd(&head.name, shell) {
            match args.iter().find(|a| !a.starts_with('-') || a.as_str() == "-") {
                // `cd -` goes back to a folder this command line never named.
                Some(t) if t == "-" => {}
                Some(t) => {
                    if let Some(t) = resolve(t, &base, home) {
                        base = t.folder;
                    }
                }
                None => {
                    if let Some(h) = home
                        && shell == Shell::Bash
                    {
                        base = h.to_path_buf();
                    }
                }
            }
            continue;
        }
        let Some(mut spec) = search_spec(&head.name, args, shell) else {
            continue;
        };
        if spec.targets.is_empty() && !spec.default_cwd {
            // No file named and none implied: it reads its input. Through `xargs`, or Select-String fed by
            // Get-ChildItem, that input is a list of files from the command before it; otherwise it is text.
            let fed = simple.piped_in
                && (head.via_xargs || head.name == "select-string" || head.name == "sls")
                && k > 0;
            match fed.then(|| producer(&simples[k - 1], shell)).flatten() {
                Some((targets, filters)) => {
                    spec.targets = targets;
                    spec.filters.extend(filters);
                    if spec.targets.is_empty() {
                        spec.default_cwd = true;
                    }
                }
                None => continue,
            }
        }
        if let Some(search) = classify(spec, &base, home) {
            out.push(search);
        }
    }
    out
}

// ─── The hint text ───────────────────────────────────────────

/// What the hint says about one search.
enum Said {
    Mapped(String),
    Unmapped { root: PathBuf, text: String },
}

fn hint_for(search: &CodeSearch, cwd: &Path) -> Option<Said> {
    for s in &search.folders {
        let Some(ttl) = crate::config::find_ast_ttl(&s.folder) else {
            continue;
        };
        // A bare folder counts only when it holds source files: `grep -rn x ~/.base-gbl/handoffs` searches
        // markdown, whatever map the folder above it carries. A bounded probe (automap's), and only here, where a map
        // would otherwise make the hint fire.
        if !s.holds_code && !automap::has_code_files(&s.folder) {
            continue;
        }
        let cwd_map = crate::config::find_ast_ttl(cwd);
        let target = (cwd_map.as_deref() != Some(ttl.as_path())).then(|| map_root(&ttl));
        return Some(Said::Mapped(render_mapped(search.kind, search.name.as_deref(), target.as_deref())));
    }
    if !search.names_code {
        return None;
    }
    let first = search.folders.iter().find(|s| s.holds_code)?;
    let root = crate::config::ast_app_root(&first.folder)?;
    // First contact, as a Bash command naming the folder already is: a first map starts here when the app has
    // none. `None` means a map appeared since the check above.
    let plan = automap::bash_first_contact(&root)?;
    let base_ast = root.join(".base-ast");
    let failing = plan != MapPlan::Build
        && base_ast.join(".last-error").is_file()
        && !base_ast.join(".building").is_file();
    let text = render_unmapped(&root, plan, failing)?;
    Some(Said::Unmapped { root, text })
}

/// The folder a map belongs to: the parent of `.base-ast/` (or of a legacy `.base/`).
fn map_root(ttl: &Path) -> PathBuf {
    ttl.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(|| ttl.to_path_buf())
}

/// The hint when a code map covers the search.
pub fn render_mapped(kind: Kind, name: Option<&str>, target: Option<&Path>) -> String {
    let target = target.map(|t| format!(" --target \"{}\"", t.display())).unwrap_or_default();
    match name {
        Some(name) => {
            let mode = match kind {
                Kind::Content => "--contains",
                Kind::Names => "--file",
            };
            format!(
                "<ast-hint>\nA code map covers this search. Try:\n  base ast query{target} {mode} \"{name}\"\n\
                 The graph knows file locations, line numbers, and call relationships.\n</ast-hint>"
            )
        }
        None => format!(
            "<ast-hint>\nA code map covers this search. Try `base ast query{target}` for code navigation.\n\
             Modes: --contains <name>, --file <path>, --calls <name>, --imports <path>\n</ast-hint>"
        ),
    }
}

/// The hint for a search over source files in an app with no code map, chosen by what first contact decided for
/// that app. `None` where nothing true and useful can be said: home and workspace hubs are never apps.
pub fn render_unmapped(root: &Path, plan: MapPlan, failing: bool) -> Option<String> {
    let r = root.display();
    let body = match plan {
        MapPlan::SkipHome | MapPlan::SkipHub => return None,
        MapPlan::SkipNeverMap(why) => format!(
            "No code map covers {r}, and base never maps it automatically ({why}). Search the files directly, or \
             build one by hand: base sync --ast --target \"{r}\""
        ),
        MapPlan::NeedsConfirm => format!(
            "No code map covers {r}: it is too large to map without asking. {} has the counts and the command \
             that builds it.",
            root.join(".base-ast").join(".needs-confirm").display()
        ),
        _ if failing => format!(
            "No code map covers {r}: base's automatic build has been failing, and {} says why. Search the files \
             directly.",
            root.join(".base-ast").join(".last-error").display()
        ),
        MapPlan::Build | MapPlan::Refresh | MapPlan::Debounced => format!(
            "No code map covers {r} yet. base is building one in the background; `base ast query --target \"{r}\"` \
             answers once it lands. Search the files directly until then."
        ),
    };
    Some(format!("<ast-hint>\n{body}\n</ast-hint>"))
}

// ─── The plain name (F20c) ───────────────────────────────────

/// The name to look up for a search pattern: regex syntax stripped, then the longest name-like token, which is
/// letters, digits and underscores, at least 3 long and holding at least one letter. A tie goes to the first. So
/// `fn select(` gives `select`, `user_prompt_submit::handle` gives `user_prompt_submit`, `\[budget\]` gives
/// `budget`, `^\s*\$scrub\s*=` gives `scrub`, and `TODO` gives `TODO`. Shell variables (`$p`, `${name}`) are not
/// names: the pattern they hold is unknown.
pub fn query_name(pattern: &str) -> Option<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\\' && next.is_some() {
            // An escape is syntax (\b \w \s \d), a code point (\x41, \u{…}, \p{L}) or escaped punctuation
            // (\. \( \$): never part of a name.
            flush(&mut cur, &mut tokens);
            i += 2;
            if matches!(next, Some('x' | 'u' | 'p' | 'P' | 'N')) {
                if chars.get(i) == Some(&'{') {
                    while i < chars.len() && chars[i] != '}' {
                        i += 1;
                    }
                    i += 1;
                } else if matches!(next, Some('x' | 'u')) {
                    while i < chars.len() && chars[i].is_ascii_hexdigit() {
                        i += 1;
                    }
                } else if matches!(next, Some('p' | 'P')) {
                    i += 1;
                }
            }
            continue;
        }
        if c == '[' && next == Some(':') {
            // A POSIX class, [:alpha:]: its name is syntax.
            flush(&mut cur, &mut tokens);
            i += 2;
            while i < chars.len() && !(chars[i] == ':' && chars.get(i + 1) == Some(&']')) {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == '(' && next == Some('?') {
            // An inline flag or group marker, (?i) (?: (?P<name>: syntax up to its end.
            flush(&mut cur, &mut tokens);
            i += 2;
            while i < chars.len() && !matches!(chars[i], ')' | ':' | '>' | '=' | '!') {
                i += 1;
            }
            i += 1;
            continue;
        }
        if c == '$' && next.is_some_and(|n| n == '{' || n == '_' || n.is_ascii_alphabetic()) {
            flush(&mut cur, &mut tokens);
            i += 1;
            if chars[i] == '{' {
                while i < chars.len() && chars[i] != '}' {
                    i += 1;
                }
                i += 1;
            } else {
                while i < chars.len() && (chars[i] == '_' || chars[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
            }
            continue;
        }
        if c.is_ascii_alphanumeric() || c == '_' {
            cur.push(c);
        } else {
            flush(&mut cur, &mut tokens);
        }
        i += 1;
    }
    flush(&mut cur, &mut tokens);
    let mut best: Option<String> = None;
    for t in tokens {
        if t.len() >= 3
            && t.chars().any(|c| c.is_ascii_alphabetic())
            && best.as_ref().is_none_or(|b| t.len() > b.len())
        {
            best = Some(t);
        }
    }
    best
}

fn flush(cur: &mut String, tokens: &mut Vec<String>) {
    if !cur.is_empty() {
        tokens.push(std::mem::take(cur));
    }
}

// ─── Splitting a command line ────────────────────────────────

/// One simple command: its words with quoting removed, and whether its input is a pipe.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Simple {
    words: Vec<String>,
    piped_in: bool,
}

#[derive(Default)]
struct Lexer {
    out: Vec<Simple>,
    cur: Simple,
    word: String,
    in_word: bool,
    /// The next word is a redirection's target, not an argument.
    drop_next: bool,
}

impl Lexer {
    fn push(&mut self, c: char) {
        self.word.push(c);
        self.in_word = true;
    }
    fn end_word(&mut self) {
        if self.in_word {
            let w = std::mem::take(&mut self.word);
            if self.drop_next {
                self.drop_next = false;
            } else {
                self.cur.words.push(w);
            }
            self.in_word = false;
        }
    }
    fn end_simple(&mut self, piped_next: bool) {
        self.end_word();
        self.drop_next = false;
        let done = std::mem::replace(&mut self.cur, Simple { words: Vec::new(), piped_in: piped_next });
        if !done.words.is_empty() {
            self.out.push(done);
        }
    }
}

/// Split a command line into simple commands, as far as these rules need the shell's grammar.
fn split(command: &str, shell: Shell) -> Vec<Simple> {
    let chars: Vec<char> = command.chars().collect();
    let n = chars.len();
    let escape = match shell {
        Shell::Bash => '\\',
        Shell::PowerShell => '`',
    };
    let mut lx = Lexer::default();
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == escape {
            match next {
                Some('\n') => i += 2,
                Some(e) => {
                    lx.push(e);
                    i += 2;
                }
                None => i += 1,
            }
            continue;
        }
        match c {
            '\'' => {
                lx.in_word = true;
                i += 1;
                loop {
                    if i >= n {
                        break;
                    }
                    if chars[i] == '\'' {
                        // PowerShell spells a quote inside single quotes as ''.
                        if shell == Shell::PowerShell && chars.get(i + 1) == Some(&'\'') {
                            lx.word.push('\'');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    lx.word.push(chars[i]);
                    i += 1;
                }
            }
            '"' => {
                lx.in_word = true;
                i += 1;
                while i < n && chars[i] != '"' {
                    let d = chars[i];
                    if d == escape && i + 1 < n {
                        let e = chars[i + 1];
                        // Inside double quotes Bash keeps a backslash unless it escapes one of these.
                        if shell == Shell::Bash && !matches!(e, '$' | '`' | '"' | '\\' | '\n') {
                            lx.word.push(d);
                        }
                        if e != '\n' {
                            lx.word.push(e);
                        }
                        i += 2;
                        continue;
                    }
                    if d == '$' && chars.get(i + 1) == Some(&'(') {
                        i = copy_balanced(&chars, i, &mut lx.word);
                        continue;
                    }
                    lx.word.push(d);
                    i += 1;
                }
                i += 1;
            }
            '$' if next == Some('(') => {
                lx.in_word = true;
                i = copy_balanced(&chars, i, &mut lx.word);
            }
            '`' if shell == Shell::Bash => {
                lx.push('`');
                i += 1;
                while i < n && chars[i] != '`' {
                    lx.word.push(chars[i]);
                    i += 1;
                }
                lx.word.push('`');
                i += 1;
            }
            ' ' | '\t' | '\r' => {
                lx.end_word();
                i += 1;
            }
            '\n' => {
                lx.end_simple(false);
                i += 1;
                for (delim, strip_tabs) in std::mem::take(&mut heredocs) {
                    while i < n {
                        let start = i;
                        while i < n && chars[i] != '\n' {
                            i += 1;
                        }
                        let line: String = chars[start..i].iter().collect();
                        i += 1;
                        let line = if strip_tabs { line.trim_start_matches('\t') } else { line.as_str() };
                        if line.trim_end_matches('\r') == delim {
                            break;
                        }
                    }
                }
            }
            ';' => {
                lx.end_simple(false);
                i += 1;
            }
            '&' if next == Some('&') => {
                lx.end_simple(false);
                i += 2;
            }
            '&' if next == Some('>') => {
                lx.end_word();
                i += 2;
                if chars.get(i) == Some(&'>') {
                    i += 1;
                }
                lx.drop_next = true;
            }
            '&' => {
                lx.end_simple(false);
                i += 1;
            }
            '|' if next == Some('|') => {
                lx.end_simple(false);
                i += 2;
            }
            '|' => {
                lx.end_simple(true);
                i += if next == Some('&') { 2 } else { 1 };
            }
            '(' | ')' => {
                lx.end_simple(false);
                i += 1;
            }
            '<' | '>' => {
                // A file-descriptor number (2>) or PowerShell's * (*>) right before is part of the operator.
                if lx.in_word && !lx.word.is_empty() && lx.word.chars().all(|d| d.is_ascii_digit() || d == '*') {
                    lx.word.clear();
                    lx.in_word = false;
                } else {
                    lx.end_word();
                }
                if c == '<' && next == Some('<') && chars.get(i + 2) != Some(&'<') && shell == Shell::Bash {
                    // A heredoc: its delimiter word now, its body skipped from the next line.
                    i += 2;
                    let strip_tabs = chars.get(i) == Some(&'-');
                    if strip_tabs {
                        i += 1;
                    }
                    while i < n && matches!(chars[i], ' ' | '\t') {
                        i += 1;
                    }
                    let mut delim = String::new();
                    while i < n && !matches!(chars[i], ' ' | '\t' | '\n' | ';' | '|' | '&' | ')') {
                        if !matches!(chars[i], '\'' | '"' | '\\') {
                            delim.push(chars[i]);
                        }
                        i += 1;
                    }
                    if !delim.is_empty() {
                        heredocs.push((delim, strip_tabs));
                    }
                    continue;
                }
                i += 1;
                while i < n && matches!(chars[i], '<' | '>' | '&' | '|') {
                    i += 1;
                }
                if chars[i - 1] == '&' && chars.get(i).is_some_and(|d| d.is_ascii_digit() || *d == '-') {
                    // >&2, 2>&1: a descriptor, not a file.
                    while i < n && (chars[i].is_ascii_digit() || chars[i] == '-') {
                        i += 1;
                    }
                } else {
                    lx.drop_next = true;
                }
            }
            '#' if !lx.in_word => {
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
            }
            '@' if shell == Shell::PowerShell && !lx.in_word && matches!(next, Some('\'' | '"')) => {
                // A here-string, @' … '@: its body is one word, never commands.
                let close = [next.unwrap_or('\''), '@'];
                i += 2;
                lx.in_word = true;
                while i < n && !(chars[i] == close[0] && chars.get(i + 1) == Some(&close[1])) {
                    lx.word.push(chars[i]);
                    i += 1;
                }
                i += 2;
            }
            _ => {
                lx.push(c);
                i += 1;
            }
        }
    }
    lx.end_simple(false);
    lx.out
}

/// Copy a `$( … )` substitution, starting at its `$`, into `word`; returns the index after its `)`.
fn copy_balanced(chars: &[char], start: usize, word: &mut String) -> usize {
    let mut depth = 0usize;
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        word.push(c);
        i += 1;
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    i
}

// ─── Which program a simple command runs ─────────────────────

struct Head {
    /// Lower-case, without a folder or `.exe`.
    name: String,
    index: usize,
    /// Run through `xargs`: its files arrive on stdin as names.
    via_xargs: bool,
}

/// The program a simple command runs, past `VAR=x` assignments, `sudo`, `env`, `xargs` and the shell keywords that
/// can stand before a command (`if`, `then`, `do`, …). `None` for words that start no command (`for x in …`).
fn program(words: &[String], shell: Shell) -> Option<Head> {
    let mut i = 0;
    let mut via_xargs = false;
    loop {
        let w = words.get(i)?;
        let lw = w.to_ascii_lowercase();
        if shell == Shell::Bash && is_assignment(w) {
            i += 1;
            continue;
        }
        match lw.as_str() {
            "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "!" | "{" | "}" | "time" | "exec"
            | "command" | "builtin" | "nohup" | "sudo" | "&" | "." => i += 1,
            "env" => {
                i += 1;
                while words.get(i).is_some_and(|w| w.starts_with('-') || is_assignment(w)) {
                    i += 1;
                }
            }
            "xargs" => {
                via_xargs = true;
                i += 1;
                while let Some(w) = words.get(i) {
                    if !w.starts_with('-') {
                        break;
                    }
                    // -n 1, -I {}, -P 4 … take a value unless it is attached (-n1).
                    let takes = matches!(w.as_str(), "-n" | "-L" | "-P" | "-I" | "-d" | "-s" | "-E" | "-a");
                    i += if takes { 2 } else { 1 };
                }
            }
            "for" | "case" | "function" | "select" | "fi" | "done" | "esac" | "in" => return None,
            _ => break,
        }
    }
    let word = &words[i];
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
    let name = base.strip_suffix(".exe").unwrap_or(&base).to_string();
    Some(Head { name, index: i, via_xargs })
}

fn is_assignment(w: &str) -> bool {
    match w.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
                && !name.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

fn is_cd(name: &str, shell: Shell) -> bool {
    match shell {
        Shell::Bash => matches!(name, "cd" | "pushd"),
        Shell::PowerShell => matches!(name, "cd" | "chdir" | "set-location" | "sl" | "pushd" | "push-location"),
    }
}

// ─── What each search program searches ───────────────────────

/// A search command's arguments, sorted.
#[derive(Debug, Default)]
struct Spec {
    kind: Option<Kind>,
    patterns: Vec<String>,
    /// Operands naming files or folders.
    targets: Vec<String>,
    /// File-name globs that restrict what is searched: `--include`, `-g`, `-name`, `-Include`, `-Filter`, `-e ext`.
    filters: Vec<String>,
    /// Language names that restrict what is searched: `rg -t rust`, `ack --type=rust`, `ag --rust`.
    types: Vec<String>,
    /// Searches the folder it starts in when no target is named (grep -r, rg, ag, ack, find, fd).
    default_cwd: bool,
}

fn search_spec(name: &str, args: &[String], shell: Shell) -> Option<Spec> {
    match (name, shell) {
        ("grep" | "egrep" | "fgrep", _) => Some(grep(args)),
        ("rg", _) => rg(args),
        ("ag", _) => Some(ag_ack(args, true)),
        ("ack" | "ack-grep", _) => Some(ag_ack(args, false)),
        // In PowerShell, `find` is Windows' find.exe, a different program.
        ("find", Shell::Bash) => find(args),
        ("fd" | "fdfind", _) => fd(args),
        ("select-string" | "sls", Shell::PowerShell) => Some(select_string(args)),
        _ => None,
    }
}

/// `--name=value` or `--name value`: the value, and how many words it used.
fn long_value(arg: &str, rest: &[String]) -> (Option<String>, usize) {
    match arg.split_once('=') {
        Some((_, v)) => (Some(v.to_string()), 1),
        None => (rest.first().cloned(), 2),
    }
}

fn grep(args: &[String]) -> Spec {
    let mut spec = Spec { kind: Some(Kind::Content), ..Spec::default() };
    let mut operands: Vec<String> = Vec::new();
    let mut have_pattern_option = false;
    let mut recursive = false;
    let mut i = 0;
    let mut options_over = false;
    while i < args.len() {
        let a = &args[i];
        if options_over || !a.starts_with('-') || a == "-" {
            operands.push(a.clone());
            i += 1;
            continue;
        }
        if a == "--" {
            options_over = true;
            i += 1;
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            let (value, used) = long_value(a, &args[i + 1..]);
            match name {
                "regexp" => {
                    have_pattern_option = true;
                    spec.patterns.extend(value);
                    i += used;
                }
                "file" => {
                    have_pattern_option = true;
                    i += used;
                }
                "include" => {
                    spec.filters.extend(value);
                    i += used;
                }
                "directories" => {
                    recursive |= value.as_deref() == Some("recurse");
                    i += used;
                }
                "exclude" | "exclude-dir" | "exclude-from" | "max-count" | "context" | "after-context"
                | "before-context" | "label" | "binary-files" | "devices" | "group-separator" => i += used,
                "recursive" | "dereference-recursive" => {
                    recursive = true;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        // A cluster of short options: -rn, -B2, -e PATTERN, -rne PATTERN.
        let cluster: Vec<char> = a[1..].chars().collect();
        let mut used = 1;
        for (j, &o) in cluster.iter().enumerate() {
            if matches!(o, 'e' | 'f' | 'm' | 'A' | 'B' | 'C' | 'd' | 'D') {
                let attached: String = cluster[j + 1..].iter().collect();
                let value = if attached.is_empty() {
                    used = 2;
                    args.get(i + 1).cloned()
                } else {
                    Some(attached)
                };
                match o {
                    'e' => {
                        have_pattern_option = true;
                        spec.patterns.extend(value);
                    }
                    'f' => have_pattern_option = true,
                    'd' => recursive |= value.as_deref() == Some("recurse"),
                    _ => {}
                }
                break;
            }
            if matches!(o, 'r' | 'R') {
                recursive = true;
            }
        }
        i += used;
    }
    if !have_pattern_option && !operands.is_empty() {
        spec.patterns.push(operands.remove(0));
    }
    spec.targets = operands;
    spec.default_cwd = recursive;
    spec
}

fn rg(args: &[String]) -> Option<Spec> {
    let mut spec = Spec { kind: Some(Kind::Content), default_cwd: true, ..Spec::default() };
    let mut operands: Vec<String> = Vec::new();
    let mut have_pattern_option = false;
    let mut i = 0;
    let mut options_over = false;
    while i < args.len() {
        let a = &args[i];
        if options_over || !a.starts_with('-') || a == "-" {
            operands.push(a.clone());
            i += 1;
            continue;
        }
        if a == "--" {
            options_over = true;
            i += 1;
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            let (value, used) = long_value(a, &args[i + 1..]);
            match name {
                // A listing of files or types, not a search.
                "files" | "type-list" => return None,
                "regexp" => {
                    have_pattern_option = true;
                    spec.patterns.extend(value);
                    i += used;
                }
                "file" => {
                    have_pattern_option = true;
                    i += used;
                }
                "glob" | "iglob" => {
                    spec.filters.extend(value.filter(|g| !g.starts_with('!')));
                    i += used;
                }
                "type" => {
                    spec.types.extend(value);
                    i += used;
                }
                "type-not" | "type-add" | "type-clear" | "after-context" | "before-context" | "context"
                | "max-count" | "max-columns" | "threads" | "replace" | "encoding" | "max-depth" | "max-filesize"
                | "pre" | "pre-glob" | "sort" | "sortr" | "colors" | "context-separator"
                | "field-context-separator" | "field-match-separator" | "path-separator" | "engine"
                | "dfa-size-limit" | "regex-size-limit" | "ignore-file" | "hostname-bin" | "hyperlink-format"
                | "generate" => i += used,
                // --color takes its value only as --color=never.
                _ => i += 1,
            }
            continue;
        }
        let cluster: Vec<char> = a[1..].chars().collect();
        let mut used = 1;
        for (j, &o) in cluster.iter().enumerate() {
            if matches!(o, 'e' | 'f' | 'g' | 't' | 'T' | 'A' | 'B' | 'C' | 'm' | 'M' | 'j' | 'r' | 'E' | 'd') {
                let attached: String = cluster[j + 1..].iter().collect();
                let value = if attached.is_empty() {
                    used = 2;
                    args.get(i + 1).cloned()
                } else {
                    Some(attached)
                };
                match o {
                    'e' => {
                        have_pattern_option = true;
                        spec.patterns.extend(value);
                    }
                    'f' => have_pattern_option = true,
                    'g' => spec.filters.extend(value.filter(|g| !g.starts_with('!'))),
                    't' => spec.types.extend(value),
                    _ => {}
                }
                break;
            }
        }
        i += used;
    }
    if !have_pattern_option && !operands.is_empty() {
        spec.patterns.push(operands.remove(0));
    }
    spec.targets = operands;
    Some(spec)
}

/// ag and ack: `PATTERN [PATH…]`, recursive from the cwd, with language switches (`--rust`, `--type=rust`).
fn ag_ack(args: &[String], ag: bool) -> Spec {
    let mut spec = Spec { kind: Some(Kind::Content), default_cwd: true, ..Spec::default() };
    let mut operands: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if !a.starts_with('-') || a == "-" {
            operands.push(a.clone());
            i += 1;
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            let (value, used) = long_value(a, &args[i + 1..]);
            match name {
                "type" => {
                    spec.types.extend(value);
                    i += used;
                }
                "file-search-regex" | "ignore" | "ignore-dir" | "ignore-directory" | "path-to-ignore" | "after"
                | "before" | "context" | "max-count" | "depth" | "pager" | "workers" | "type-set" | "type-add"
                | "match" => i += used,
                other => {
                    if type_kind(other).is_some() {
                        spec.types.push(other.to_string());
                    }
                    i += 1;
                }
            }
            continue;
        }
        let o = a.chars().nth(1).unwrap_or(' ');
        let takes = if ag { matches!(o, 'A' | 'B' | 'C' | 'G' | 'm' | 'p') } else { matches!(o, 'A' | 'B' | 'C' | 'm' | 't') };
        if o == 'g' {
            // -g PATTERN lists the files whose names match: a names search.
            spec.kind = Some(Kind::Names);
            let (value, used) = if a.len() > 2 { (Some(a[2..].to_string()), 1) } else { (args.get(i + 1).cloned(), 2) };
            spec.patterns.extend(value);
            i += used;
            continue;
        }
        if o == 't' && !ag {
            let (value, used) = if a.len() > 2 { (Some(a[2..].to_string()), 1) } else { (args.get(i + 1).cloned(), 2) };
            spec.types.extend(value);
            i += used;
            continue;
        }
        i += if takes && a.len() == 2 { 2 } else { 1 };
    }
    if spec.kind == Some(Kind::Content) && !operands.is_empty() {
        spec.patterns.push(operands.remove(0));
    }
    spec.targets = operands;
    spec
}

/// GNU find: `[PATH…] EXPRESSION`. A search only with a name test; `-type d` looks for folders, not code.
fn find(args: &[String]) -> Option<Spec> {
    let mut spec = Spec { kind: Some(Kind::Names), default_cwd: true, ..Spec::default() };
    let mut i = 0;
    while i < args.len() && matches!(args[i].as_str(), "-H" | "-L" | "-P") {
        i += 1;
    }
    while i < args.len() && !args[i].starts_with(['-', '(', '!', ',']) {
        spec.targets.push(args[i].clone());
        i += 1;
    }
    let mut folders_only = false;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-name" | "-iname" | "-path" | "-ipath" | "-wholename" | "-iwholename" | "-regex" | "-iregex" => {
                if let Some(v) = args.get(i + 1) {
                    spec.patterns.push(v.clone());
                    if matches!(a, "-name" | "-iname") {
                        spec.filters.push(v.clone());
                    }
                }
                i += 2;
            }
            "-type" | "-xtype" => {
                folders_only |= args.get(i + 1).is_some_and(|t| t == "d");
                i += 2;
            }
            "-exec" | "-execdir" | "-ok" | "-okdir" => {
                i += 1;
                while i < args.len() && args[i] != ";" && args[i] != "+" {
                    i += 1;
                }
                i += 1;
            }
            "-maxdepth" | "-mindepth" | "-mtime" | "-mmin" | "-atime" | "-amin" | "-ctime" | "-cmin" | "-size"
            | "-newer" | "-user" | "-group" | "-perm" | "-printf" | "-fprint" | "-fprintf" | "-regextype"
            | "-samefile" | "-inum" | "-links" | "-used" | "-fstype" | "-context" | "-uid" | "-gid" => i += 2,
            _ => i += 1,
        }
    }
    (!spec.patterns.is_empty() && !folders_only).then_some(spec)
}

/// fd: `[PATTERN] [PATH…]`, with `-e EXT` filters. A search only with a pattern or an extension.
fn fd(args: &[String]) -> Option<Spec> {
    let mut spec = Spec { kind: Some(Kind::Names), default_cwd: true, ..Spec::default() };
    let mut operands: Vec<String> = Vec::new();
    let mut folders_only = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if !a.starts_with('-') {
            operands.push(args[i].clone());
            i += 1;
            continue;
        }
        let (name, value, used) = if let Some(long) = a.strip_prefix("--") {
            let (v, u) = long_value(a, &args[i + 1..]);
            (long.split('=').next().unwrap_or(long).to_string(), v, u)
        } else if a.len() > 2 {
            (a[1..2].to_string(), Some(a[2..].to_string()), 1)
        } else {
            (a[1..].to_string(), args.get(i + 1).cloned(), 2)
        };
        match name.as_str() {
            "e" | "extension" => {
                spec.filters.extend(value.map(|e| format!("*.{}", e.trim_start_matches('.'))));
                i += used;
            }
            "t" | "type" => {
                folders_only |= value.as_deref().is_some_and(|t| t == "d" || t == "directory");
                i += used;
            }
            "x" | "exec" | "X" | "exec-batch" => break,
            "E" | "exclude" | "d" | "max-depth" | "min-depth" | "exact-depth" | "c" | "color" | "j" | "threads"
            | "S" | "size" | "changed-within" | "changed-before" | "o" | "owner" | "base-directory"
            | "search-path" | "path-separator" | "max-results" | "ignore-file" => i += used,
            _ => i += 1,
        }
    }
    if !operands.is_empty() {
        spec.patterns.push(operands.remove(0));
    }
    spec.targets = operands;
    ((!spec.patterns.is_empty() || !spec.filters.is_empty()) && !folders_only).then_some(spec)
}

/// PowerShell's Select-String: `[-Pattern] <p> [-Path] <files>`, with names abbreviable and arrays comma-separated.
fn select_string(args: &[String]) -> Spec {
    let mut spec = Spec { kind: Some(Kind::Content), ..Spec::default() };
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let Some(param) = a.strip_prefix('-').filter(|p| p.starts_with(|c: char| c.is_ascii_alphabetic())) else {
            positional.push(a.clone());
            i += 1;
            continue;
        };
        let (pname, attached) = match param.split_once(':') {
            Some((p, v)) => (p.to_ascii_lowercase(), Some(v.to_string())),
            None => (param.to_ascii_lowercase(), None),
        };
        let named = |full: &str| full.starts_with(pname.as_str());
        let takes = ["pattern", "path", "literalpath", "include", "exclude", "context", "encoding", "inputobject", "culture"]
            .iter()
            .find(|full| named(full));
        let Some(full) = takes else {
            i += 1;
            continue;
        };
        let (value, used) = match attached {
            Some(v) => (Some(v), 1),
            None => (args.get(i + 1).cloned(), 2),
        };
        let values = value.map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>());
        match *full {
            "pattern" => spec.patterns.extend(values.unwrap_or_default()),
            "path" | "literalpath" => spec.targets.extend(values.unwrap_or_default()),
            "include" => spec.filters.extend(values.unwrap_or_default()),
            _ => {}
        }
        i += used;
    }
    let mut positional = positional.into_iter();
    if spec.patterns.is_empty() {
        spec.patterns.extend(positional.next());
    }
    if spec.targets.is_empty() {
        spec.targets.extend(positional.flat_map(|p| p.split(',').map(str::to_string).collect::<Vec<_>>()));
    }
    spec
}

/// The files the command before a pipe hands on as names: find or fd (into `xargs`), `ls`, or Get-ChildItem
/// (into Select-String). `(targets, filters)`, or `None` when it is not a file listing.
fn producer(simple: &Simple, shell: Shell) -> Option<(Vec<String>, Vec<String>)> {
    let head = program(&simple.words, shell)?;
    let args = &simple.words[head.index + 1..];
    match (head.name.as_str(), shell) {
        ("find", Shell::Bash) => find(args).map(|s| (s.targets, s.filters)),
        ("fd" | "fdfind", _) => fd(args).map(|s| (s.targets, s.filters)),
        ("ls", Shell::Bash) => Some((args.iter().filter(|a| !a.starts_with('-')).cloned().collect(), Vec::new())),
        ("get-childitem" | "gci" | "ls" | "dir" | "get-item" | "gi", Shell::PowerShell) => {
            let mut targets = Vec::new();
            let mut filters = Vec::new();
            let mut positional = 0;
            let mut i = 0;
            while i < args.len() {
                let a = &args[i];
                match a.strip_prefix('-').map(str::to_ascii_lowercase) {
                    Some(p) if (p.len() >= 2 && "path".starts_with(p.as_str())) || p == "literalpath" => {
                        targets.extend(args.get(i + 1).map(|v| v.split(',').map(str::to_string).collect::<Vec<_>>()).unwrap_or_default());
                        i += 2;
                    }
                    Some(p) if p.len() >= 2 && ("filter".starts_with(p.as_str()) || "include".starts_with(p.as_str())) => {
                        filters.extend(args.get(i + 1).map(|v| v.split(',').map(str::to_string).collect::<Vec<_>>()).unwrap_or_default());
                        i += 2;
                    }
                    Some(p) if matches!(p.as_str(), "exclude" | "depth" | "attributes") => i += 2,
                    Some(_) => i += 1,
                    None => {
                        // Positional: -Path, then -Filter.
                        if positional == 0 {
                            targets.extend(a.split(',').map(str::to_string));
                        } else if positional == 1 {
                            filters.push(a.clone());
                        }
                        positional += 1;
                        i += 1;
                    }
                }
            }
            Some((targets, filters))
        }
        _ => None,
    }
}

// ─── Classifying what is searched ────────────────────────────

/// A named target, resolved.
struct Target {
    /// The folder searched: the folder named, a file's folder, or a glob's literal prefix.
    folder: PathBuf,
    /// The extension of a named file or glob, lower case; `None` for a folder.
    ext: Option<String>,
    /// A file with no extension (Makefile, Dockerfile): never source.
    plain_file: bool,
}

/// Resolve a target word: `~` is home, Git Bash's `/c/…` is `C:/…` on Windows, relative is against `base`, and a
/// glob stops at its first wildcard component. `None` for a word holding a variable or a substitution.
fn resolve(word: &str, base: &Path, home: Option<&Path>) -> Option<Target> {
    if word.is_empty() || word == "-" || word.contains(['$', '`']) || word.starts_with('@') {
        return None;
    }
    let mut w = word.to_string();
    if w == "~" || w.starts_with("~/") || w.starts_with("~\\") {
        let home = home?;
        w = format!("{}{}", home.display(), &w[1..]);
    } else if cfg!(windows)
        && let Some(rest) = w.strip_prefix('/')
        && let Some((drive, tail)) = rest.split_once('/')
        && drive.len() == 1
        && drive.chars().all(|c| c.is_ascii_alphabetic())
    {
        w = format!("{}:/{tail}", drive.to_ascii_uppercase());
    }
    let parts: Vec<&str> = w.split(['/', '\\']).collect();
    let glob_at = parts.iter().position(|p| p.contains(['*', '?', '[']));
    let last = parts.last().copied().unwrap_or("");
    let ext = extension(last);
    let literal: String = match glob_at {
        Some(k) => parts[..k].join("/"),
        None => w.clone(),
    };
    let literal = if literal.is_empty() && w.starts_with(['/', '\\']) { "/".to_string() } else { literal };
    let path = clean(&if literal.is_empty() {
        base.to_path_buf()
    } else {
        let p = PathBuf::from(&literal);
        if p.is_absolute() || p.has_root() { p } else { base.join(p) }
    });
    if glob_at.is_some() {
        return Some(Target { folder: path, ext, plain_file: false });
    }
    if path.is_dir() {
        return Some(Target { folder: path, ext: None, plain_file: false });
    }
    if ext.is_none() && path.is_file() {
        return Some(Target { folder: path.parent().map(Path::to_path_buf).unwrap_or_default(), ext: None, plain_file: true });
    }
    match ext {
        Some(ext) => Some(Target {
            folder: path.parent().map(Path::to_path_buf).unwrap_or_default(),
            ext: Some(ext),
            plain_file: false,
        }),
        // Neither a folder nor a file here: read as a folder (it may be created later, or be a typo).
        None => Some(Target { folder: path, ext: None, plain_file: false }),
    }
}

/// `.` dropped and `..` taken back, without touching the filesystem, so a folder reads the same however it was
/// spelled (`src/.`, `./src`, `src/../src`).
fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The extension of a file name or name glob, lower case: `*.rs` and `main.rs` give `rs`; `.env`, `*` and `x.*`
/// give none.
fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// `*.{rs,toml}` as `*.rs` and `*.toml`; one brace group, the first.
fn expand_braces(word: &str) -> Vec<String> {
    if let Some(open) = word.find('{')
        && let Some(len) = word[open..].find('}')
    {
        let inner = &word[open + 1..open + len];
        if inner.contains(',') {
            return inner
                .split(',')
                .map(|alt| format!("{}{alt}{}", &word[..open], &word[open + len + 1..]))
                .collect();
        }
    }
    vec![word.to_string()]
}

/// Whether a language name (`rg -t`, `ack --type`, `ag --rust`) is code (`Some(true)`), docs or data
/// (`Some(false)`), or not known here (`None`).
fn type_kind(name: &str) -> Option<bool> {
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "md" | "markdown" | "json" | "jsonl" | "toml" | "yaml" | "yml" | "txt" | "csv" | "log" | "xml" | "html"
        | "css" | "config" | "ini" | "rst" | "tex" => Some(false),
        "rust" | "python" | "js" | "javascript" | "ts" | "typescript" | "go" | "golang" | "java" | "kotlin"
        | "c" | "cpp" | "csharp" | "ruby" | "php" | "swift" | "scala" | "elixir" | "julia" | "lua" | "zig"
        | "powershell" | "ps" | "shell" | "sh" | "bash" | "vue" | "svelte" | "dart" | "sql" | "objc"
        | "groovy" | "pascal" | "verilog" | "fortran" | "astro" | "perl" => Some(true),
        _ if automap::is_code_ext(&n) => Some(true),
        _ => None,
    }
}

/// Code (`Some(true)`), not code (`Some(false)`), or unknown (`None`) for a file-name glob filter.
fn filter_kind(glob: &str) -> Option<bool> {
    extension(glob.rsplit(['/', '\\']).next().unwrap_or(glob)).map(|e| automap::is_code_ext(&e))
}

fn classify(spec: Spec, base: &Path, home: Option<&Path>) -> Option<CodeSearch> {
    let kind = spec.kind?;
    // Filters restrict the whole search: one admitting only docs, config or data means nothing else is read.
    let filter_kinds: Vec<bool> = spec
        .filters
        .iter()
        .flat_map(|f| expand_braces(f))
        .filter_map(|f| filter_kind(&f))
        .chain(spec.types.iter().filter_map(|t| type_kind(t)))
        .collect();
    let code_filter = filter_kinds.contains(&true);
    if !filter_kinds.is_empty() && !code_filter {
        return None;
    }

    let mut code_folders: Vec<PathBuf> = Vec::new();
    let mut folders: Vec<PathBuf> = Vec::new();
    for word in &spec.targets {
        for word in expand_braces(word) {
            let Some(t) = resolve(&word, base, home) else {
                continue;
            };
            match t.ext.as_deref().map(automap::is_code_ext) {
                Some(true) => push_unique(&mut code_folders, t.folder),
                // A doc, config or data file, or a file with no extension (Makefile): not source.
                Some(false) => {}
                None if t.plain_file => {}
                None => push_unique(&mut folders, t.folder),
            }
        }
    }
    if spec.targets.is_empty() && spec.default_cwd {
        folders.push(clean(base));
    }
    // Every target named was a doc, config or data file (or a variable): nothing searched is code.
    if code_folders.is_empty() && folders.is_empty() {
        return None;
    }
    let names_code = code_filter || !code_folders.is_empty();
    // A folder named bare is a code search there only through a code filter; otherwise the hint checks it for
    // source files before it counts.
    let mut all: Vec<Searched> = code_folders.into_iter().map(|folder| Searched { folder, holds_code: true }).collect();
    for folder in folders {
        if !all.iter().any(|s| s.folder == folder) {
            all.push(Searched { folder, holds_code: code_filter });
        }
    }
    let name = spec.patterns.iter().filter_map(|p| query_name(p)).fold(None::<String>, |best, t| match best {
        Some(b) if b.len() >= t.len() => Some(b),
        _ => Some(t),
    });
    Some(CodeSearch { kind, name, folders: all, names_code })
}

fn push_unique(v: &mut Vec<PathBuf>, p: PathBuf) {
    if !v.contains(&p) {
        v.push(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(cmd: &str, shell: Shell) -> Vec<Vec<String>> {
        split(cmd, shell).into_iter().map(|s| s.words).collect()
    }

    #[test]
    fn split_follows_quotes_pipes_and_redirections() {
        assert_eq!(
            words(r#"grep -n -B2 -A3 -E '^paths|Documents' /c/x/domains.toml | head -40"#, Shell::Bash),
            vec![
                vec!["grep", "-n", "-B2", "-A3", "-E", "^paths|Documents", "/c/x/domains.toml"],
                vec!["head", "-40"],
            ]
        );
        let s = split("base relay sessions 2>&1 | grep -i lynx", Shell::Bash);
        assert_eq!(s[0].words, vec!["base", "relay", "sessions"]);
        assert!(!s[0].piped_in && s[1].piped_in);
        assert_eq!(words("grep x a.rs 2>/dev/null; echo done", Shell::Bash), vec![vec!["grep", "x", "a.rs"], vec!["echo", "done"]]);
        assert_eq!(words(r#"grep "fn select\(" src/"#, Shell::Bash), vec![vec!["grep", r"fn select\(", "src/"]]);
    }

    #[test]
    fn split_skips_heredoc_bodies_and_comments() {
        let cmd = "python - <<'EOF'\nimport os\ngrep -rn select src/\nEOF\necho after # grep -rn x src/";
        assert_eq!(words(cmd, Shell::Bash), vec![vec!["python", "-"], vec!["echo", "after"]]);
    }

    #[test]
    fn split_keeps_powershell_backslashes() {
        assert_eq!(
            words(r#"Select-String -Path C:\work\app\*.rs -Pattern 'fn main' | Select-Object -First 3"#, Shell::PowerShell),
            vec![
                vec!["Select-String", "-Path", r"C:\work\app\*.rs", "-Pattern", "fn main"],
                vec!["Select-Object", "-First", "3"],
            ]
        );
    }

    #[test]
    fn query_name_pins_the_rule() {
        for (pattern, name) in [
            ("fn select(", Some("select")),
            ("user_prompt_submit::handle", Some("user_prompt_submit")),
            ("TODO", Some("TODO")),
            (r"\[budget\]", Some("budget")),
            ("^paths|Documents", Some("Documents")),
            (r"^\s*\$scrub\s*=", Some("scrub")),
            (r"tabArgs|wt\.exe|Start-Process", Some("tabArgs")),
            (r"\bselect\b", Some("select")),
            ("[[:alpha:]]+_id", Some("_id")),
            ("(?i)handler", Some("handler")),
            ("$p", None),
            ("${pattern}", None),
            ("rc", None),
            ("2026", None),
            ("^$", None),
        ] {
            assert_eq!(query_name(pattern).as_deref(), name, "{pattern:?}");
        }
    }

    #[test]
    fn extension_reads_names_and_globs() {
        assert_eq!(extension("*.rs").as_deref(), Some("rs"));
        assert_eq!(extension("main.RS").as_deref(), Some("rs"));
        assert_eq!(extension(".env"), None);
        assert_eq!(extension("*"), None);
        assert_eq!(extension("x.*"), None);
        assert_eq!(expand_braces("*.{rs,toml}"), vec!["*.rs", "*.toml"]);
    }

    #[test]
    fn grep_options_are_parsed() {
        let s = grep(&["-rn".into(), "--include=*.rs".into(), "-e".into(), "foo".into(), "src".into()]);
        assert_eq!(s.patterns, vec!["foo"]);
        assert_eq!(s.targets, vec!["src"]);
        assert_eq!(s.filters, vec!["*.rs"]);
        assert!(s.default_cwd);
        let s = grep(&["-n".into(), "-B2".into(), "x".into()]);
        assert_eq!(s.patterns, vec!["x"]);
        assert!(s.targets.is_empty() && !s.default_cwd, "no file, not recursive: reads stdin");
    }
}
