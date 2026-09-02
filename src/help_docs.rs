//! The base-help coach must describe the binary it ships with.
//!
//! `claude/skills/base-help/` is what `/base-help` answers from. Its reference
//! files are stamped to a base version, and until this module nothing held that
//! stamp to `Cargo.toml`: the gate written for v0.12.3 lived on an unmerged
//! branch, so twelve releases shipped a coach that still said v0.13.2 while
//! `base doctor` on every machine reported the drift and nobody could act on it.
//!
//! Enforced on every `cargo test`:
//!
//! 1. `references/cli.md` is exactly what this binary's clap tree renders: the
//!    verbatim `--help` of every visible subcommand.
//! 2. The version stamps in `commands.md` and `qa.md`, and the pair counts in
//!    `SKILL.md` and `README.md`, match `CARGO_PKG_VERSION`.
//! 3. Every shipped subcommand appears in `qa.md` or `commands.md`.
//! 4. Every `base ...` invocation the skill shows a reader resolves against
//!    this binary: the subcommand path exists (aliases included) and every flag
//!    it names is real and sits on the right command.
//!
//! 1 and 2 are generated: `BASE_REGEN_DOCS=1 cargo test --bin base help_docs`
//! rewrites them, and `scripts/release.sh` does that on every release. 3 and 4
//! are curation, so their failure messages name the exact command or flag.
//!
//! This walks `clap::Command` directly instead of spawning `base --help`, so
//! it is hermetic (no HOME, no graph, no subprocess) and runs in milliseconds.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Once;

use clap::{Arg, Command, CommandFactory};

use crate::cli::Cli;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REGEN_ENV: &str = "BASE_REGEN_DOCS";

/// The generated block inside a curated file sits between these two markers.
const STAMP_BEGIN: &str = "<!-- stamp:begin";
const STAMP_END: &str = "<!-- stamp:end -->";
/// Invocations the bank shows precisely because they do NOT work
/// ("`base rule l` is an error") are declared in one of these comments.
const ALLOW_BEGIN: &str = "<!-- invalid-by-design:";

const SKILL_MD: &str = "SKILL.md";
const README_MD: &str = "README.md";
const QA_MD: &str = "references/qa.md";
const COMMANDS_MD: &str = "references/commands.md";
const CLI_MD: &str = "references/cli.md";

const COMMANDS_TITLE: &str = "# base command reference (v";
const SKILL_COUNT_PHRASE: &str = " verified Q&A pairs";
const README_COUNT_PHRASE: &str = " question/answer pairs";

fn skill_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("claude")
        .join("skills")
        .join("base-help")
}

fn read(rel: &str) -> String {
    let path = skill_dir().join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

/// Write through a sibling temp file so a reader never sees a torn file.
fn write(rel: &str, text: &str) {
    let path = skill_dir().join(rel);
    let tmp = path.with_extension("md.regen-tmp");
    std::fs::write(&tmp, text).unwrap_or_else(|e| panic!("cannot write {}: {e}", tmp.display()));
    std::fs::rename(&tmp, &path)
        .unwrap_or_else(|e| panic!("cannot replace {}: {e}", path.display()));
}

fn regen_requested() -> bool {
    std::env::var_os(REGEN_ENV).is_some_and(|v| !v.is_empty() && v != "0")
}

/// This binary's command tree, built, with bin names resolved so usage lines
/// read `base rule add` rather than `base.exe rule add` or a bare `rule add`.
fn command_tree() -> Command {
    let mut cmd = Cli::command();
    cmd.set_bin_name("base");
    cmd.build();
    cmd
}

/// Hidden commands are internal by declaration; `help` is clap's own.
fn is_documented_command(cmd: &Command) -> bool {
    !cmd.is_hide_set() && cmd.get_name() != "help"
}

fn bin_path(cmd: &Command) -> String {
    cmd.get_bin_name().unwrap_or(cmd.get_name()).to_string()
}

// ─── Command paths ────────────────────────────────────────────────────────

/// One level of a subcommand path: the canonical name plus every alias clap
/// accepts for it, hidden ones included, because a reader can type those too.
#[derive(Clone, Debug)]
struct Seg {
    name: String,
    aliases: Vec<String>,
}

impl Seg {
    fn of(cmd: &Command) -> Self {
        Seg {
            name: cmd.get_name().to_string(),
            aliases: cmd.get_all_aliases().map(str::to_string).collect(),
        }
    }

    fn spellings(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.name.as_str()).chain(self.aliases.iter().map(String::as_str))
    }
}

/// Every visible subcommand path, depth-first in declaration order.
fn visible_paths(cmd: &Command, prefix: &[Seg], out: &mut Vec<Vec<Seg>>) {
    for sub in cmd.get_subcommands().filter(|s| is_documented_command(s)) {
        let mut path = prefix.to_vec();
        path.push(Seg::of(sub));
        out.push(path.clone());
        visible_paths(sub, &path, out);
    }
}

fn canonical(path: &[Seg]) -> String {
    path.iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every accepted spelling of a full path, each prefixed with `base `.
fn spellings(path: &[Seg]) -> Vec<String> {
    let mut acc = vec![String::from("base")];
    for seg in path {
        let mut next = Vec::new();
        for prefix in &acc {
            for spelling in seg.spellings() {
                next.push(format!("{prefix} {spelling}"));
            }
        }
        acc = next;
    }
    acc
}

// ─── cli.md: the verbatim --help tree ─────────────────────────────────────

fn render_cli_reference(root: &mut Command, version: &str) -> String {
    let mut sections: Vec<(String, Vec<String>, String)> = Vec::new();
    collect_help(root, &[], &mut sections);

    let mut out = String::new();
    out.push_str(&format!("# base CLI reference (v{version})\n\n"));
    out.push_str(
        "Generated from this release's own command tree: the verbatim `--help` of every \
         subcommand, exactly as the binary prints it. `src/help_docs.rs` in the base repo \
         regenerates this file on every release and fails `cargo test` when it is behind the \
         code, so when the version above matches `base --version`, every flag here is real. \
         Do not edit by hand; regenerate with `BASE_REGEN_DOCS=1 cargo test --bin base help_docs`.\n\n\
         Use it for exact syntax. `commands.md` groups the same surface by what is safe to run, \
         and `qa.md` explains the mechanics.\n\n",
    );
    out.push_str("## Index\n\n");
    for (path, aliases, _) in &sections {
        let alias = if aliases.is_empty() {
            String::new()
        } else {
            format!(" (alias: {})", aliases.join(", "))
        };
        out.push_str(&format!("- `{path}`{alias}\n"));
    }
    for (path, _, help) in &sections {
        out.push_str(&format!(
            "\n## {path}\n\n```text\n{}\n```\n",
            help.trim_end()
        ));
    }
    out
}

fn collect_help(
    cmd: &mut Command,
    prefix: &[String],
    out: &mut Vec<(String, Vec<String>, String)>,
) {
    let mut path = prefix.to_vec();
    path.push(cmd.get_name().to_string());
    let display = path.join(" ");
    let aliases: Vec<String> = cmd.get_visible_aliases().map(str::to_string).collect();
    let help = cmd.render_long_help().to_string();
    out.push((display, aliases, help));
    for sub in cmd.get_subcommands_mut() {
        if is_documented_command(sub) {
            collect_help(sub, &path, out);
        }
    }
}

// ─── Stamps and counts in the curated files ───────────────────────────────

/// The generated block for one curated file, without the markers. The qa.md
/// block must open with `base v<semver>`: `base doctor` reads the first such
/// token in the file to decide whether the installed coach matches the binary.
fn stamp_body(file: &str, version: &str, date: &str) -> String {
    match file {
        COMMANDS_MD => format!(
            "Mechanically checked against base v{version} on {date}: every `base ...` \
             invocation in this file resolves to a live subcommand, every flag it names exists \
             on that command, and every shipped subcommand appears in this file or in `qa.md`. \
             `src/help_docs.rs` in the base repo enforces all three on every `cargo test` and \
             restamps this block at each release, so a release cannot ship with this file behind \
             the binary. The read-only / mutating / destructive grouping and the gotcha notes are \
             curated by hand. For the verbatim `--help` of any command, see `cli.md`."
        ),
        QA_MD => format!(
            "**Stamped for base v{version} on {date}.** This is a mechanical stamp: every \
             `base ...` invocation in this file resolves to a live subcommand with real flags, \
             and every shipped subcommand appears here or in `commands.md`, both enforced by \
             `src/help_docs.rs` on every `cargo test` and restamped at each release. It says the \
             syntax is current. The `<!-- vX.Y.Z | verified: ... -->` tag under each pair says \
             which release a person last checked that pair's mechanism claims against; the \
             generator never moves those."
        ),
        other => panic!("no stamp block is defined for {other}"),
    }
}

fn replace_stamp(text: &str, body: &str) -> Result<String, String> {
    let begin = text
        .find(STAMP_BEGIN)
        .ok_or("no `<!-- stamp:begin` marker")?;
    let begin_end = text[begin..]
        .find("-->")
        .map(|i| begin + i + 3)
        .ok_or("the stamp:begin marker never closes")?;
    let end = text[begin_end..]
        .find(STAMP_END)
        .map(|i| begin_end + i)
        .ok_or("no `<!-- stamp:end -->` marker")?;
    Ok(format!("{}\n{body}\n{}", &text[..begin_end], &text[end..]))
}

/// The `base vX.Y.Z` inside the stamp block, if the block exists.
fn stamped_version(text: &str) -> Option<String> {
    let begin = text.find(STAMP_BEGIN)?;
    let end = text[begin..].find(STAMP_END).map(|i| begin + i)?;
    semver_after(&text[begin..end], "base v")
}

fn semver_after(text: &str, marker: &str) -> Option<String> {
    let i = text.find(marker)?;
    let v: String = text[i + marker.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let v = v.trim_end_matches('.').to_string();
    (v.split('.').count() == 3).then_some(v)
}

fn retitle_commands(text: &str, version: &str) -> Result<String, String> {
    let mut lines = text.splitn(2, '\n');
    let first = lines.next().unwrap_or("");
    let rest = lines.next().unwrap_or("");
    if !first.starts_with(COMMANDS_TITLE) {
        return Err(format!("the first line must start with `{COMMANDS_TITLE}`"));
    }
    Ok(format!("{COMMANDS_TITLE}{version})\n{rest}"))
}

fn pair_count(qa: &str) -> usize {
    qa.lines().filter(|l| l.starts_with("### Q:")).count()
}

/// The integer immediately before `phrase`, e.g. `176` in `176 verified Q&A pairs`.
fn count_before(text: &str, phrase: &str) -> Option<usize> {
    let at = text.find(phrase)?;
    let digits = text[..at]
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .len();
    text[digits..at].parse().ok()
}

fn replace_count(text: &str, phrase: &str, n: usize) -> Result<String, String> {
    let at = text
        .find(phrase)
        .ok_or_else(|| format!("no `{phrase}` phrase to carry the count"))?;
    let digits = text[..at]
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .len();
    if digits == at {
        return Err(format!("no number precedes `{phrase}`"));
    }
    Ok(format!("{}{n}{}", &text[..digits], &text[at..]))
}

// ─── Invocations: every `base ...` the skill shows a reader ───────────────

/// `(line, invocation)` pairs. Fenced blocks contribute every line that starts
/// with `base ` (trailing ` # comment` stripped, deeper-indented continuation
/// lines joined on). Prose contributes every backtick span that starts with
/// `base `. A span like `base.toml` or `base` alone is not an invocation.
fn invocations(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut in_fence = false;
    let mut continuing: Option<usize> = None;

    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        if raw.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continuing = None;
            continue;
        }
        if in_fence {
            let code = raw.split(" #").next().unwrap_or("").trim_end();
            let trimmed = code.trim_start();
            let body = trimmed.strip_prefix("$ ").unwrap_or(trimmed);
            if body.starts_with("base ") {
                out.push((line, body.to_string()));
                continuing = Some(out.len() - 1);
            } else if trimmed.is_empty() || trimmed.starts_with('#') {
                // comment-only or blank: keep the continuation open
            } else if raw.starts_with("        ") {
                if let Some(k) = continuing {
                    out[k].1.push(' ');
                    out[k].1.push_str(trimmed);
                }
            } else {
                continuing = None;
            }
            continue;
        }
        let mut rest = raw;
        while let Some(open) = rest.find('`') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('`') else { break };
            let span = &after[..close];
            if span.starts_with("base ") {
                out.push((line, span.to_string()));
            }
            rest = &after[close + 1..];
        }
    }
    out
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Invocations declared invalid on purpose, anywhere in the file:
/// `<!-- invalid-by-design: `base rule l` `base workspace list` -->`.
fn allowlist(text: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find(ALLOW_BEGIN) {
        let after = &rest[i + ALLOW_BEGIN.len()..];
        let Some(end) = after.find("-->") else { break };
        let mut block = &after[..end];
        while let Some(open) = block.find('`') {
            let a = &block[open + 1..];
            let Some(close) = a.find('`') else { break };
            set.insert(normalize(&a[..close]));
            block = &a[close + 1..];
        }
        rest = &after[end..];
    }
    set
}

/// Shell-ish split: quoted strings stay whole; `[` and `]` (optional groups),
/// a bare `|` (alternatives) and `...` are notation, not arguments.
fn tokenize(inv: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in inv.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '[' | ']' => {}
                c if c.is_whitespace() => {
                    if !cur.is_empty() {
                        tokens.push(std::mem::take(&mut cur));
                    }
                }
                _ => cur.push(c),
            },
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
        .into_iter()
        .filter(|t| t != "|" && t != "...")
        .collect()
}

/// `--long`, `--long=value`, `-s`. Combined shorts (`-ab`) are not modelled.
fn find_flag<'a>(cmd: &'a Command, token: &str) -> Option<&'a Arg> {
    let bare = token.split('=').next().unwrap_or(token);
    if let Some(long) = bare.strip_prefix("--") {
        cmd.get_arguments().find(|a| {
            a.get_long() == Some(long)
                || a.get_all_aliases()
                    .into_iter()
                    .flatten()
                    .any(|al| al == long)
        })
    } else if let Some(short) = bare.strip_prefix('-') {
        let mut chars = short.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return None;
        };
        cmd.get_arguments().find(|a| {
            a.get_short() == Some(c)
                || a.get_all_short_aliases()
                    .into_iter()
                    .flatten()
                    .any(|s| s == c)
        })
    } else {
        None
    }
}

fn subcommand_names(cmd: &Command) -> String {
    cmd.get_subcommands()
        .filter(|s| is_documented_command(s))
        .map(|s| s.get_name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve one tokenized invocation (`tokens[0] == "base"`) against the tree.
fn resolve(root: &Command, tokens: &[String]) -> Result<(), String> {
    let mut chain: Vec<&Command> = vec![root];
    let mut descending = true;
    let mut i = 1;
    while i < tokens.len() {
        let t = tokens[i].as_str();
        let cmd = *chain.last().expect("chain starts with root");
        if t == "--" {
            break;
        }
        if t == "help" && descending {
            return Ok(()); // clap's own `base help <sub>`
        }
        if matches!(t, "-h" | "--help" | "-V" | "--version") {
            i += 1;
            continue;
        }
        let negative_number = t.len() > 1 && t[1..].starts_with(|c: char| c.is_ascii_digit());
        if t.starts_with('-') && !negative_number {
            let found = find_flag(cmd, t).or_else(|| {
                chain
                    .iter()
                    .rev()
                    .skip(1)
                    .find_map(|c| find_flag(c, t).filter(|a| a.is_global_set()))
            });
            let Some(arg) = found else {
                let owner = chain
                    .iter()
                    .rev()
                    .skip(1)
                    .find(|c| find_flag(c, t).is_some());
                return Err(match owner {
                    Some(c) => format!(
                        "`{t}` belongs to `{}` and goes before the subcommand, not after it",
                        bin_path(c)
                    ),
                    None => format!("`{t}` is not a flag of `{}`", bin_path(cmd)),
                });
            };
            i += 1;
            let takes_value = arg.get_action().takes_values() && !t.contains('=');
            if takes_value && i < tokens.len() && !tokens[i].starts_with('-') {
                i += 1; // its value
            }
            continue;
        }
        if t.starts_with('<') || t.starts_with('{') {
            descending = false; // a placeholder positional
            i += 1;
            continue;
        }
        if descending {
            if let Some(sub) = cmd.find_subcommand(t) {
                chain.push(sub);
                i += 1;
                continue;
            }
            if cmd.has_subcommands() && cmd.get_positionals().next().is_none() {
                return Err(format!(
                    "`{t}` is not a subcommand of `{}` (it has: {})",
                    bin_path(cmd),
                    subcommand_names(cmd)
                ));
            }
            descending = false;
        }
        i += 1; // a positional value
    }
    Ok(())
}

#[derive(Debug)]
struct Problem {
    file: &'static str,
    line: usize,
    invocation: String,
    reason: String,
}

fn check_file(root: &Command, file: &'static str, text: &str) -> Vec<Problem> {
    let allowed = allowlist(text);
    invocations(text)
        .into_iter()
        .filter(|(_, inv)| !allowed.contains(&normalize(inv)))
        .filter_map(|(line, inv)| {
            resolve(root, &tokenize(&inv)).err().map(|reason| Problem {
                file,
                line,
                invocation: inv,
                reason,
            })
        })
        .collect()
}

// ─── Coverage: every shipped subcommand is in the bank ────────────────────

/// Substring search that respects word boundaries, so the alias spelling
/// `base t` does not match inside `base task`.
fn mentions(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        let next_ok = match bytes.get(end) {
            None => true,
            Some(&c) => !(c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        };
        if next_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

fn undocumented(root: &Command, haystack: &str) -> Vec<(String, Vec<String>)> {
    let mut paths = Vec::new();
    visible_paths(root, &[], &mut paths);
    paths
        .iter()
        .filter_map(|p| {
            let s = spellings(p);
            (!s.iter().any(|x| mentions(haystack, x))).then(|| (canonical(p), s))
        })
        .collect()
}

// ─── Regeneration ─────────────────────────────────────────────────────────

/// With `BASE_REGEN_DOCS=1`, rewrite the generated parts once per test
/// process, before any check reads them. Without it, do nothing: the checks
/// then report drift instead of hiding it.
fn ensure_regenerated() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if !regen_requested() {
            return;
        }
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mut root = command_tree();
        write(CLI_MD, &render_cli_reference(&mut root, VERSION));

        let commands = read(COMMANDS_MD);
        let commands = retitle_commands(&commands, VERSION)
            .and_then(|t| replace_stamp(&t, &stamp_body(COMMANDS_MD, VERSION, &date)))
            .unwrap_or_else(|e| panic!("{COMMANDS_MD}: {e}"));
        write(COMMANDS_MD, &commands);

        let qa = read(QA_MD);
        let pairs = pair_count(&qa);
        let qa = replace_stamp(&qa, &stamp_body(QA_MD, VERSION, &date))
            .unwrap_or_else(|e| panic!("{QA_MD}: {e}"));
        write(QA_MD, &qa);

        let skill = replace_count(&read(SKILL_MD), SKILL_COUNT_PHRASE, pairs)
            .unwrap_or_else(|e| panic!("{SKILL_MD}: {e}"));
        write(SKILL_MD, &skill);
        let readme = replace_count(&read(README_MD), README_COUNT_PHRASE, pairs)
            .unwrap_or_else(|e| panic!("{README_MD}: {e}"));
        write(README_MD, &readme);
    });
}

const HOW_TO_REGEN: &str = "Regenerate with `BASE_REGEN_DOCS=1 cargo test --bin base help_docs` (scripts/release.sh runs this on every release).";

// ─── The gate ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ArgAction;

    #[test]
    fn cli_reference_matches_this_binary() {
        ensure_regenerated();
        let mut root = command_tree();
        let expected = render_cli_reference(&mut root, VERSION);
        let path = skill_dir().join(CLI_MD);
        let on_disk = std::fs::read_to_string(&path)
            .map(|s| s.replace("\r\n", "\n"))
            .unwrap_or_default();
        if on_disk == expected {
            return;
        }
        let first_diff = expected
            .lines()
            .zip(on_disk.lines())
            .position(|(a, b)| a != b)
            .map(|i| {
                format!(
                    "first difference at line {}:\n  generated: {}\n  on disk:   {}",
                    i + 1,
                    expected.lines().nth(i).unwrap_or(""),
                    on_disk.lines().nth(i).unwrap_or("")
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "same prefix, different length ({} vs {} lines)",
                    expected.lines().count(),
                    on_disk.lines().count()
                )
            });
        panic!(
            "\n{} is not what this binary (v{VERSION}) renders.\n\
             The CLI changed and the coach's reference was not regenerated.\n{HOW_TO_REGEN}\n\n{first_diff}\n",
            path.display()
        );
    }

    #[test]
    fn bank_stamps_match_this_binary() {
        ensure_regenerated();
        let mut wrong: Vec<String> = Vec::new();

        let commands = read(COMMANDS_MD);
        match semver_after(commands.lines().next().unwrap_or(""), COMMANDS_TITLE) {
            Some(v) if v == VERSION => {}
            other => wrong.push(format!("{COMMANDS_MD} title says {other:?}")),
        }
        for file in [COMMANDS_MD, QA_MD] {
            match stamped_version(&read(file)) {
                Some(v) if v == VERSION => {}
                other => wrong.push(format!("{file} stamp block says {other:?}")),
            }
        }

        let pairs = pair_count(&read(QA_MD));
        for (file, phrase) in [
            (SKILL_MD, SKILL_COUNT_PHRASE),
            (README_MD, README_COUNT_PHRASE),
        ] {
            match count_before(&read(file), phrase) {
                Some(n) if n == pairs => {}
                other => wrong.push(format!(
                    "{file} says {other:?} pairs but {QA_MD} has {pairs} `### Q:` pairs"
                )),
            }
        }

        assert!(
            wrong.is_empty(),
            "\nThe base-help coach is stamped for a different release than this binary (v{VERSION}):\n  {}\n\
             `base doctor` reports exactly this drift on every machine that installs the release.\n{HOW_TO_REGEN}\n",
            wrong.join("\n  ")
        );
    }

    #[test]
    fn every_shipped_subcommand_is_in_the_knowledge_bank() {
        ensure_regenerated();
        let root = command_tree();
        let haystack = normalize(&format!("{}\n{}", read(QA_MD), read(COMMANDS_MD)));
        let missing = undocumented(&root, &haystack);
        let total = {
            let mut p = Vec::new();
            visible_paths(&root, &[], &mut p);
            p.len()
        };
        assert!(
            total > 50,
            "walked only {total} subcommand paths; the clap tree walk is broken"
        );
        if missing.is_empty() {
            return;
        }
        let mut msg = format!(
            "\n{} of {total} shipped subcommand(s) appear in NEITHER {QA_MD} NOR {COMMANDS_MD}.\n\n\
             The base-help skill answers out of those two files, so an undocumented command is one\n\
             the skill will tell users does not exist. Write each one up, then re-run:\n\
             \x20 - {QA_MD}: a `### Q:` / `**A:**` pair in the matching `## ` section, closed by a\n\
             \x20   `<!-- v{VERSION} | verified: cli-help -->` comment.\n\
             \x20 - {COMMANDS_MD}: one line under read-only / mutating / destructive, whichever fits.\n\
             Genuinely internal? Mark it `#[command(hide = true)]` in src/cli.rs.\n\n\
             Undocumented:\n",
            missing.len()
        );
        for (canon, spellings) in &missing {
            msg.push_str(&format!(
                "  base {canon}\n      accepted spellings: {}\n",
                spellings.join("  |  ")
            ));
        }
        panic!("{msg}");
    }

    #[test]
    fn every_documented_invocation_resolves_against_this_binary() {
        ensure_regenerated();
        let root = command_tree();
        let mut problems: Vec<Problem> = Vec::new();
        let mut checked = 0usize;
        for file in [SKILL_MD, README_MD, QA_MD, COMMANDS_MD] {
            let text = read(file);
            checked += invocations(&text).len();
            problems.extend(check_file(&root, file, &text));
        }
        assert!(
            checked > 100,
            "found only {checked} invocations; the extractor is broken"
        );
        if problems.is_empty() {
            return;
        }
        let mut msg = format!(
            "\n{} `base ...` invocation(s) in the base-help coach do not resolve against this binary (v{VERSION}).\n\
             Each names a subcommand or flag the CLI does not have (any more), so the coach would hand a\n\
             user a command that errors. Fix the text, or, if the bank shows it BECAUSE it is invalid,\n\
             list it in an `{ALLOW_BEGIN} `...` -->` comment in that file.\n\n",
            problems.len()
        );
        for p in &problems {
            msg.push_str(&format!(
                "  {}:{}  `{}`\n      {}\n",
                p.file, p.line, p.invocation, p.reason
            ));
        }
        panic!("{msg}");
    }

    // ── Pure unit tests on a synthetic tree ──

    fn toy() -> Command {
        let mut cmd = Command::new("base")
            .subcommand(
                Command::new("rule")
                    .visible_alias("r")
                    .arg(
                        Arg::new("global")
                            .short('g')
                            .long("global")
                            .action(ArgAction::SetTrue),
                    )
                    .subcommand(
                        Command::new("add")
                            .arg(Arg::new("domain").long("domain").required(true))
                            .arg(Arg::new("text").long("text").required(true)),
                    )
                    .subcommand(Command::new("list").alias("l")),
            )
            .subcommand(Command::new("hook").arg(Arg::new("event")))
            .subcommand(Command::new("secret").hide(true))
            .subcommand(
                Command::new("graph").subcommand(
                    Command::new("purge")
                        .arg(Arg::new("stale").long("stale").action(ArgAction::SetTrue))
                        .arg(Arg::new("days").long("days"))
                        .arg(
                            Arg::new("verbose")
                                .short('v')
                                .global(true)
                                .action(ArgAction::SetTrue),
                        ),
                ),
            );
        cmd.set_bin_name("base");
        cmd.build();
        cmd
    }

    fn check(inv: &str) -> Result<(), String> {
        resolve(&toy(), &tokenize(inv))
    }

    #[test]
    fn resolves_names_aliases_and_flags_before_the_subcommand() {
        assert_eq!(check("base rule add --domain X --text \"a b\""), Ok(()));
        assert_eq!(check("base r l"), Ok(()));
        assert_eq!(check("base rule -g add --domain X --text ..."), Ok(()));
        assert_eq!(check("base rule --global list"), Ok(()));
        assert_eq!(check("base hook <event>"), Ok(()));
        assert_eq!(check("base hook user-prompt-submit"), Ok(()));
        assert_eq!(check("base help rule"), Ok(()));
        assert_eq!(check("base --version"), Ok(()));
        assert!(check("base graph purge --stale [--days N] [--apply-later]").is_err());
        assert_eq!(check("base graph purge --stale [--days N]"), Ok(()));
        assert_eq!(check("base <subcommand>"), Ok(()));
        assert_eq!(
            check("base secret"),
            Ok(()),
            "hidden commands still resolve"
        );
    }

    #[test]
    fn rejects_dead_subcommands_misplaced_flags_and_unknown_flags() {
        let e = check("base rule l --nope").unwrap_err();
        assert!(
            e.contains("`--nope` is not a flag of `base rule list`"),
            "{e}"
        );
        let e = check("base rule add -g --domain X --text T").unwrap_err();
        assert!(e.contains("belongs to `base rule`"), "{e}");
        let e = check("base rule remove --index 1").unwrap_err();
        assert!(
            e.contains("`remove` is not a subcommand of `base rule`"),
            "{e}"
        );
        let e = check("base workspace list").unwrap_err();
        assert!(
            e.contains("`workspace` is not a subcommand of `base`"),
            "{e}"
        );
    }

    #[test]
    fn tokenizer_keeps_quotes_and_drops_notation() {
        assert_eq!(
            tokenize("base t tag <slug> --add <label> | --remove <label> [--yes] \"a b\" ..."),
            [
                "base", "t", "tag", "<slug>", "--add", "<label>", "--remove", "<label>", "--yes",
                "\"a b\""
            ]
        );
    }

    #[test]
    fn extractor_reads_fences_prose_continuations_and_comments() {
        let text = "Try `base rule list --domain X` or `base.toml`.\n\
                    ```bash\n\
                    base doctor [--json]        # health\n\
                    base task update <slug> [--name ...]\n\
                    \x20                       [--due ...] [-p <project>]\n\
                    \x20                       # a trailing comment line\n\
                    $ base --version\n\
                    ```\n\
                    ### Q: Is there a `base standards add`?\n";
        let got = invocations(text);
        assert_eq!(
            got,
            vec![
                (1, "base rule list --domain X".to_string()),
                (3, "base doctor [--json]".to_string()),
                (
                    4,
                    "base task update <slug> [--name ...] [--due ...] [-p <project>]".to_string()
                ),
                (7, "base --version".to_string()),
                (9, "base standards add".to_string()),
            ]
        );
    }

    #[test]
    fn allowlist_reads_every_span_in_the_comment() {
        let text = "x\n<!-- invalid-by-design: `base rule  l` and `base workspace list` -->\ny";
        let set = allowlist(text);
        assert!(set.contains("base rule l"));
        assert!(set.contains("base workspace list"));
        assert_eq!(set.len(), 2);
        let root = toy();
        assert!(
            check_file(
                &root,
                "t.md",
                "see `base workspace list`\n<!-- invalid-by-design: `base workspace list` -->"
            )
            .is_empty()
        );
        assert_eq!(
            check_file(&root, "t.md", "see `base workspace list`").len(),
            1
        );
    }

    #[test]
    fn stamp_block_replacement_and_version_parse_round_trip() {
        let text = "# t\n\n<!-- stamp:begin (generated) -->\nold base v0.1.0 words\n<!-- stamp:end -->\nrest";
        let out = replace_stamp(text, &stamp_body(QA_MD, "9.9.9", "2026-01-02")).unwrap();
        assert!(out.starts_with(
            "# t\n\n<!-- stamp:begin (generated) -->\n**Stamped for base v9.9.9 on 2026-01-02.**"
        ));
        assert!(out.ends_with("<!-- stamp:end -->\nrest"));
        assert_eq!(stamped_version(&out).as_deref(), Some("9.9.9"));
        assert!(replace_stamp("no markers", "x").is_err());
        assert_eq!(
            retitle_commands("# base command reference (v0.1.0)\nbody", "2.0.0").unwrap(),
            "# base command reference (v2.0.0)\nbody"
        );
    }

    #[test]
    fn counts_are_read_and_rewritten_in_place() {
        let text = "the bank holds 176 verified Q&A pairs today";
        assert_eq!(count_before(text, SKILL_COUNT_PHRASE), Some(176));
        assert_eq!(
            replace_count(text, SKILL_COUNT_PHRASE, 201).unwrap(),
            "the bank holds 201 verified Q&A pairs today"
        );
        assert!(replace_count("no count verified Q&A pairs", SKILL_COUNT_PHRASE, 1).is_err());
        assert_eq!(pair_count("### Q: a\n**A:** x\n### Q: b\n"), 2);
    }

    #[test]
    fn coverage_walk_sees_aliases_and_skips_hidden() {
        let root = toy();
        let mut paths = Vec::new();
        visible_paths(&root, &[], &mut paths);
        let canon: Vec<String> = paths.iter().map(|p| canonical(p)).collect();
        assert_eq!(
            canon,
            [
                "rule",
                "rule add",
                "rule list",
                "hook",
                "graph",
                "graph purge"
            ]
        );
        assert_eq!(
            spellings(&paths[2]),
            ["base rule list", "base rule l", "base r list", "base r l"]
        );
        let missing: Vec<String> = undocumented(&root, "base r l and base hook")
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        assert_eq!(missing, ["rule add", "graph", "graph purge"]);
        assert!(!mentions("run base task list", "base t"));
        assert!(mentions("run base t list", "base t"));
    }
}
