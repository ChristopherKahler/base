//! Command-plugin system (v0.6) — drop-in `base <foo>` CLI commands.
//!
//! Frameworks contribute new subcommands without forking the binary or editing
//! the core clap enum. The seam is clap's `external_subcommand`: any
//! unrecognized `base <foo> …` is captured as raw args and routed here
//! (git's `git-foo` external-subcommand model).
//!
//! Resolution path: the existing extension loader (`crate::extension`) already
//! scans `~/.base-gbl/extensions/*.toml`. A `[[commands]]` manifest section
//! (Phase 39) maps `name → handler`; this module builds a registry from the
//! loaded extensions, resolves the name, and shells out to the handler
//! (Phase 40) with an injected env contract.
//!
//! **Graph-access model (locked 2026-06-22):** plugins NEVER touch `graph.nq`
//! directly. They mutate state only by calling back through `base` itself
//! (`base learn` / `base task add` / …). base stays the only writer. The env
//! contract gives a plugin enough context to invoke base.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::extension::{self, ExtensionDef};

pub mod dist;
pub mod scaffold;

// ─── Reserved core command names ─────────────────────────────
//
// A plugin can never shadow a built-in: core subcommands always resolve first
// (clap matches them before the `external_subcommand` fallthrough ever fires),
// and the registry (Phase 39) additionally refuses to register any plugin
// command whose name collides with this set. Keep in sync with the `Commands`
// enum + visible aliases in `cli.rs`.
const RESERVED: &[&str] = &[
    // clap built-ins
    "help",
    // core subcommands
    "hook",
    "ast", "a",
    "project", "p",
    "milestone", "m",
    "task", "t",
    "decision", "d",
    "entity", "e",
    "goal", "g",
    "reminder", "r",
    "sync",
    "domain",
    "learn",
    "recall",
    "rule",
    "install",
    "activate",
    "update",
    "uninstall",
    "dashboard", "dash",
    "scaffold",
    "operator",
    "extension", "ext",
    "commands", "cmd",
    "memory",
    "config",
    "context",
    "doctor",
    "graph",
];

/// The set of names a plugin command may not claim (core subcommands + aliases).
pub fn reserved_command_names() -> HashSet<String> {
    RESERVED.iter().map(|s| s.to_string()).collect()
}

/// True if `name` is a reserved core command (case-sensitive — clap subcommands
/// are lowercase).
pub fn is_reserved(name: &str) -> bool {
    RESERVED.contains(&name)
}

// ─── Registry ────────────────────────────────────────────────

/// A resolved drop-in command: a manifest `[[commands]]` entry with its handler
/// path resolved and its source extension recorded.
#[derive(Debug, Clone)]
pub struct PluginCommand {
    pub name: String,
    /// Absolute path to the executable handler (resolved, not existence-checked).
    pub handler: PathBuf,
    pub description: String,
    pub usage: Option<String>,
    /// The extension that contributed this command.
    pub ext_name: String,
}

/// All drop-in commands contributed by installed extensions. First-declared
/// wins on name collision; reserved (core) names are refused at build time.
#[derive(Debug, Default)]
pub struct CommandRegistry {
    commands: Vec<PluginCommand>,
}

impl CommandRegistry {
    /// Resolve a command by exact name.
    pub fn resolve(&self, name: &str) -> Option<&PluginCommand> {
        self.commands.iter().find(|c| c.name == name)
    }
    pub fn list(&self) -> &[PluginCommand] {
        &self.commands
    }
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
    pub fn names(&self) -> Vec<&str> {
        self.commands.iter().map(|c| c.name.as_str()).collect()
    }
}

/// Build the registry from loaded extensions.
///
/// Guards, each emitting a stderr warning (fail-open — a bad entry is skipped,
/// never fatal):
/// - **Reserved-name guard:** a plugin command whose name matches a core
///   subcommand (or alias) is refused — built-ins always win.
/// - **First-wins dedup:** if two extensions declare the same command name, the
///   first (extensions load sorted by name) keeps it.
///
/// Handler paths are resolved but NOT checked for existence here — `exec`
/// validates at call time, so `base ext list` can still surface a mis-pathed
/// command with a clean error rather than hiding it.
pub fn build_registry(exts: &[ExtensionDef]) -> CommandRegistry {
    let reserved = reserved_command_names();
    let mut registry = CommandRegistry::default();
    let mut seen: HashSet<String> = HashSet::new();

    for ext in exts {
        for spec in &ext.commands {
            let name = spec.name.trim();
            if name.is_empty() {
                eprintln!(
                    "base: ext:{} declares a command with an empty name — skipped",
                    ext.name
                );
                continue;
            }
            if reserved.contains(name) {
                eprintln!(
                    "base: ext:{} command '{name}' collides with a core command — skipped (plugins cannot shadow built-ins)",
                    ext.name
                );
                continue;
            }
            if seen.contains(name) {
                eprintln!(
                    "base: command '{name}' already provided by another extension — ext:{} duplicate skipped (first wins)",
                    ext.name
                );
                continue;
            }
            seen.insert(name.to_string());
            registry.commands.push(PluginCommand {
                name: name.to_string(),
                handler: resolve_handler(&spec.handler, ext),
                description: spec.description.clone(),
                usage: spec.usage.clone(),
                ext_name: ext.name.clone(),
            });
        }
    }
    registry
}

/// Resolve a handler path: tilde-expand; absolute used as-is; relative resolved
/// against `framework_dir`, else the extension file's own directory, else left
/// relative (exec will fail loud if it can't be found).
fn resolve_handler(handler: &str, ext: &ExtensionDef) -> PathBuf {
    let expanded = expand_tilde(handler);
    let path = if expanded.is_absolute() {
        expanded
    } else if let Some(fw) = &ext.framework_dir {
        expand_tilde(fw).join(&expanded)
    } else if let Some(parent) = ext.source_path.as_ref().and_then(|p| p.parent()) {
        parent.join(&expanded)
    } else {
        expanded
    };
    windows_exe_fixup(path)
}

/// P3 (PLUGIN-DIST-SPEC §4): on Windows, a handler declared without an extension
/// (`bin/highlevel`) maps to the packaged `highlevel.exe`. If the bare path is
/// missing but its `.exe` sibling exists, use that. No-op on unix and whenever the
/// declared path already resolves — strictly additive.
fn windows_exe_fixup(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if path.extension().is_none() && !path.exists() {
            let exe = path.with_extension("exe");
            if exe.exists() {
                return exe;
            }
        }
    }
    path
}

/// Expand a leading `~/` to the home directory.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(path))
    } else {
        PathBuf::from(path)
    }
}

/// Resolve a manifest's `framework_dir` to an absolute path. Absolute and
/// `~/`-prefixed values are used as-is; a relative value (including `"."`, the
/// "this app's own directory" template marker) resolves against the MANIFEST's
/// own directory. This is what makes a shipped `base-extension.toml` with
/// `framework_dir = "."` install correctly from anywhere.
fn resolve_framework_dir(manifest_path: &Path, fw: &str) -> PathBuf {
    let expanded = expand_tilde(fw);
    if expanded.is_absolute() {
        return expanded;
    }
    let base = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let joined = base.join(&expanded);
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

// ─── Executor + env contract ─────────────────────────────────

/// Path to the global secret store: `~/.base-gbl/.env`.
///
/// **The base-framework convention** — all API keys / secrets for base and its
/// plugins live here. base loads it and injects the vars into every plugin's
/// environment so a handler can read e.g. `GEMINI_API_KEY` from `process.env`
/// without knowing where it came from. Git-ignored, operator-owned.
pub fn global_env_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".base-gbl").join(".env"))
}

/// Parse `KEY=VALUE` lines from a `.env` file body.
/// Skips blank lines and `#` comments; trims whitespace; strips a single layer
/// of matching single/double quotes around the value. Lines without `=` are
/// ignored. First occurrence of a key wins.
fn parse_env(content: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Tolerate a leading `export ` (common in .env files).
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_string();
        if key.is_empty() || seen.contains(&key) {
            continue;
        }
        let mut val = v.trim();
        // Strip one matching pair of surrounding quotes.
        if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            val = &val[1..val.len() - 1];
        }
        seen.insert(key.clone());
        out.push((key, val.to_string()));
    }
    out
}

/// Load + parse `~/.base-gbl/.env` (empty if absent/unreadable — fail-open).
fn load_global_env() -> Vec<(String, String)> {
    let Some(path) = global_env_path() else {
        return Vec::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => parse_env(&content),
        Err(_) => Vec::new(),
    }
}

/// The `BASE_*` env contract injected into every plugin process.
/// - `BASE_WORKSPACE`  — workspace root (the dir containing `.base/`), or cwd.
/// - `BASE_GRAPH_PATH` — the workspace graph file (`.base/graph.nq`).
/// - `BASE_GLOBAL_DIR` — `~/.base-gbl`.
/// - `BASE_BIN`        — path to the running `base` binary (so a plugin can call back).
fn base_contract(cwd: &Path) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let base_dir = crate::config::find_workspace_base(cwd);
    let workspace = base_dir
        .as_ref()
        .and_then(|b| b.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| cwd.to_path_buf());
    v.push(("BASE_WORKSPACE".into(), workspace.to_string_lossy().into_owned()));
    if let Some(b) = &base_dir {
        v.push((
            "BASE_GRAPH_PATH".into(),
            b.join("graph.nq").to_string_lossy().into_owned(),
        ));
    }
    if let Some(home) = dirs::home_dir() {
        v.push((
            "BASE_GLOBAL_DIR".into(),
            home.join(".base-gbl").to_string_lossy().into_owned(),
        ));
    }
    if let Ok(exe) = std::env::current_exe() {
        v.push(("BASE_BIN".into(), exe.to_string_lossy().into_owned()));
    }
    v
}

/// Build the child `Command` for a plugin handler — pure + testable.
///
/// Sets the `BASE_*` contract, then applies `extra_env` (the global `.env`)
/// with **dotenv semantics: never override a var already set in base's own
/// environment** (an explicitly exported var wins). stdio is inherited so the
/// handler's stdout/stderr flow straight through to the terminal/Claude — this
/// is the drop-in seam (a `--json` handler line reaches the caller unchanged).
fn build_child(handler: &Path, args: &[String], cwd: &Path, extra_env: &[(String, String)]) -> Command {
    let mut cmd = Command::new(handler);
    cmd.args(args);
    for (k, v) in base_contract(cwd) {
        cmd.env(k, v);
    }
    for (k, v) in extra_env {
        // dotenv: process env wins over .env.
        if std::env::var_os(k).is_none() {
            cmd.env(k, v);
        }
    }
    cmd
}

/// Execute a resolved plugin command: forward `args` to its handler with the
/// env contract + global `.env` injected, inherit stdio, and propagate the
/// child's exit code. Fail-loud on a missing/non-executable handler.
pub fn exec(plugin: &PluginCommand, args: &[String], cwd: &Path) -> ! {
    if !plugin.handler.exists() {
        eprintln!(
            "base: command '{}' (ext:{}) — handler not found: {}",
            plugin.name,
            plugin.ext_name,
            plugin.handler.display()
        );
        eprintln!("  Check the extension's [[commands]] handler path.");
        std::process::exit(127);
    }

    let mut cmd = build_child(&plugin.handler, args, cwd, &load_global_env());
    match cmd.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(e) => {
            eprintln!(
                "base: failed to run '{}' handler ({}): {e}",
                plugin.name,
                plugin.handler.display()
            );
            eprintln!("  Is it executable? Try: chmod +x {}", plugin.handler.display());
            std::process::exit(126);
        }
    }
}

// ─── Bundle install (linked → packaged) ─────────────────────

/// Outcome of a bundle install (for human/CLI reporting).
pub struct BundleOutcome {
    pub name: String,
    pub source: PathBuf,
    pub dest: PathBuf,
    pub files_copied: usize,
    /// true if deps are present in the bundle (vendored node_modules or npm-installed).
    pub deps_ready: bool,
    pub manifest: PathBuf,
}

/// Bundle a linked extension into `~/.base-gbl/plugins/<name>/` and repoint its
/// manifest there — turns a repo-linked plugin into a self-contained, repo-
/// independent install. This is the artifact a community member would receive.
///
/// Steps: validate manifest → copy `framework_dir` into `plugins/<name>/`
/// (vendoring `node_modules` if present; else `npm install` when a package.json
/// exists) → rewrite `framework_dir` to the bundled path → write the manifest
/// into `~/.base-gbl/extensions/<name>.toml`.
pub fn bundle_install(manifest_path: &Path) -> anyhow::Result<BundleOutcome> {
    let ext = extension::validate_extension(manifest_path)
        .map_err(|v| anyhow::anyhow!("invalid manifest: {}", v.join("; ")))?;

    let fw = ext.framework_dir.as_deref().ok_or_else(|| {
        anyhow::anyhow!("--bundle requires framework_dir in the manifest (nothing to copy)")
    })?;
    let source = resolve_framework_dir(manifest_path, fw);
    if !source.is_dir() {
        anyhow::bail!("framework_dir not found: {}", source.display());
    }

    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    let dest = home.join(".base-gbl").join("plugins").join(&ext.name);
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?; // re-bundle = clean replace
    }
    std::fs::create_dir_all(&dest)?;

    // Copy the framework tree, EXCLUDING build artifacts + VCS + generated output.
    // These are regenerable and can be huge — a Rust `target/` swept 205 MB on a real
    // install (Phase 53). node_modules is dropped too: JS deps resolve via the
    // npm-install fallback below (the portable, per-platform-safe pattern), not a
    // vendored copy.
    let exclude = [
        "target",
        "node_modules",
        ".git",
        "dist",
        "build",
        "generated_imgs",
    ];
    let files_copied = copy_dir_all(&source, &dest, &exclude)?;

    // Deps: vendored copy survived the exclude (rare), else install from package.json.
    let deps_ready = if dest.join("node_modules").is_dir() {
        true
    } else if dest.join("package.json").exists() {
        npm_install(&dest)?
    } else {
        false
    };

    // Repoint framework_dir → the bundled copy, write into extensions/.
    let original = std::fs::read_to_string(manifest_path)?;
    let rewritten = rewrite_framework_dir(&original, &dest.to_string_lossy())?;
    let ext_dir = home.join(".base-gbl").join("extensions");
    std::fs::create_dir_all(&ext_dir)?;
    let manifest = ext_dir.join(format!("{}.toml", ext.name));
    std::fs::write(&manifest, rewritten)?;

    Ok(BundleOutcome {
        name: ext.name,
        source,
        dest,
        files_copied,
        deps_ready,
        manifest,
    })
}

/// Outcome of a linked install (for CLI reporting).
pub struct LinkedOutcome {
    pub name: String,
    pub dest: PathBuf,
    pub prev_version: Option<String>,
    pub version: String,
}

/// Install an extension in LINKED mode — the handler keeps running from its
/// source repo (edits stay live). Validates, resolves a relative/"." `framework_dir`
/// to an absolute path (against the manifest's own dir), and writes the resolved
/// manifest into `~/.base-gbl/extensions/<name>.toml`. Replaces the old verbatim
/// file-copy so a shipped `base-extension.toml` (framework_dir = ".") installs
/// correctly.
pub fn install_linked(manifest_path: &Path) -> anyhow::Result<LinkedOutcome> {
    let ext = extension::validate_extension(manifest_path)
        .map_err(|v| anyhow::anyhow!("invalid manifest: {}", v.join("; ")))?;
    let original = std::fs::read_to_string(manifest_path)?;

    // Resolve a relative/"." framework_dir to absolute — the installed copy lives
    // in extensions/, not the app, so a relative path would otherwise break.
    let resolved_text = match &ext.framework_dir {
        Some(fw) if !expand_tilde(fw).is_absolute() => {
            let abs = resolve_framework_dir(manifest_path, fw);
            rewrite_framework_dir(&original, &abs.to_string_lossy())?
        }
        _ => original,
    };

    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    let ext_dir = home.join(".base-gbl").join("extensions");
    std::fs::create_dir_all(&ext_dir)?;
    let dest = ext_dir.join(format!("{}.toml", ext.name));

    let prev_version = if dest.exists() {
        std::fs::read_to_string(&dest)
            .ok()
            .and_then(|c| toml::from_str::<extension::ExtensionFile>(&c).ok())
            .map(|f| f.extension.version)
    } else {
        None
    };

    std::fs::write(&dest, resolved_text)?;
    Ok(LinkedOutcome {
        name: ext.name.clone(),
        dest,
        prev_version,
        version: ext.version,
    })
}

/// Recursively copy `src` into `dst`, skipping top-level + nested entries whose
/// file name matches `exclude`. Symlinks are dereferenced (portable bundles).
/// `std::fs::copy` carries permission bits, so executable handlers stay +x.
fn copy_dir_all(src: &Path, dst: &Path, exclude: &[&str]) -> std::io::Result<usize> {
    let mut count = 0;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if exclude.iter().any(|e| std::ffi::OsStr::new(e) == name) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let ft = entry.file_type()?;
        if ft.is_dir() {
            std::fs::create_dir_all(&to)?;
            count += copy_dir_all(&from, &to, exclude)?;
        } else if ft.is_symlink() {
            // Dereference: copy the target's content if it resolves to a file.
            if let Ok(real) = std::fs::canonicalize(&from)
                && real.is_file()
            {
                std::fs::copy(&real, &to)?;
                count += 1;
            }
        } else {
            std::fs::copy(&from, &to)?;
            count += 1;
        }
    }
    Ok(count)
}

/// Run a production-only `npm install` in `dir`. Best-effort: errors bubble up
/// so the caller can warn (a copied node_modules may already suffice).
fn npm_install(dir: &Path) -> anyhow::Result<bool> {
    // On Windows npm ships as npm.cmd, a batch script — there is no bare `npm`
    // executable, so CreateProcess cannot find it and the spawn fails with
    // "program not found" even when npm is installed and on PATH. Try the .cmd
    // form first there, and keep the bare name as a fallback for environments
    // that do expose a shim (Git Bash, MSYS, a vendored npm).
    let candidates: &[&str] = if cfg!(windows) {
        &["npm.cmd", "npm"]
    } else {
        &["npm"]
    };

    let mut last_err = None;
    for exe in candidates {
        let status = Command::new(exe)
            .current_dir(dir)
            .args(["install", "--omit=dev", "--no-audit", "--no-fund", "--silent"])
            .status();
        match status {
            Ok(s) if s.success() => return Ok(true),
            // A real exit code means npm ran — that is a build failure, not a
            // lookup failure, so do not fall through to the next candidate.
            Ok(s) => anyhow::bail!("npm install exited with code {}", s.code().unwrap_or(-1)),
            Err(e) => last_err = Some(format!("{exe}: {e}")),
        }
    }
    anyhow::bail!(
        "npm not available ({}) — vendor node_modules or install npm",
        last_err.unwrap_or_else(|| "no candidates tried".to_string())
    )
}

/// Serialize a string as a valid TOML basic string token (including the
/// surrounding quotes), escaping per the TOML spec. Critical on Windows: a path
/// like `C:\Users\…` written raw into `"…"` turns `\U` into an invalid unicode
/// escape, so the manifest silently fails to parse and the extension never loads.
/// Lossless — the parsed value round-trips to the exact input path.
fn toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Replace the `framework_dir` value in a manifest's text, preserving all other
/// lines + comments. Errors if there's no `framework_dir` line to repoint.
fn rewrite_framework_dir(original: &str, dest: &str) -> anyhow::Result<String> {
    let mut out = String::new();
    let mut replaced = false;
    for line in original.lines() {
        if line.trim_start().starts_with("framework_dir") && line.contains('=') {
            out.push_str(&format!("framework_dir = {}\n", toml_basic_string(dest)));
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        anyhow::bail!("manifest has no framework_dir line to repoint");
    }
    Ok(out)
}

/// Human-readable bundle report.
pub fn format_bundle_human(o: &BundleOutcome) -> String {
    format!(
        "✓ Bundled '{}' (linked → packaged)\n  {} files → {}\n  deps: {}\n  manifest repointed → {}\n  Now repo-independent; this is the shippable install.",
        o.name,
        o.files_copied,
        o.dest.display(),
        if o.deps_ready { "ready (vendored/installed)" } else { "none (dep-free handler)" },
        o.manifest.display(),
    )
}

// ─── Dispatch ────────────────────────────────────────────────

/// Entry point for an unrecognized subcommand captured by clap's
/// `external_subcommand`. `args[0]` is the command name; `args[1..]` are the
/// raw arguments to forward to the handler.
///
/// Phase 38: skeleton only — no registry is wired yet, so nothing resolves and
/// every unknown command fails LOUD (nonzero exit), consistent with the
/// fail-loud CLI convention (`cli::die`). Phase 39 wires the registry; Phase 40
/// executes the resolved handler.
pub fn dispatch(args: &[String], cwd: &Path) -> ! {
    let Some((name, rest)) = args.split_first() else {
        eprintln!("base: no command provided. Run `base --help` for usage.");
        std::process::exit(2);
    };

    let registry = build_registry(&extension::load_extensions());
    match registry.resolve(name) {
        Some(cmd) => exec(cmd, rest, cwd),
        None => unknown_command(name, &registry),
    }
}

/// Explicit, collision-proof invocation for `base ext run <name> [args…]`.
/// Bypasses the external-subcommand fallthrough entirely — useful when a name is
/// ambiguous or shadowed, and as a stable scripting entry point.
pub fn run_named(name: &str, args: &[String], cwd: &Path) -> ! {
    let registry = build_registry(&extension::load_extensions());
    match registry.resolve(name) {
        Some(cmd) => exec(cmd, args, cwd),
        None => unknown_command(name, &registry),
    }
}

/// Fail-loud exit for a command name that resolves to nothing, with a
/// did-you-mean suggestion when a near-match exists.
fn unknown_command(name: &str, registry: &CommandRegistry) -> ! {
    eprintln!("base: unknown command '{name}'");
    if let Some(suggestion) = did_you_mean(name, registry) {
        eprintln!("  Did you mean '{suggestion}'?");
    }
    if registry.is_empty() {
        eprintln!("  No plugin commands installed. Run `base ext list` to see extensions.");
    } else {
        eprintln!("  Available plugin commands: {}", registry.names().join(", "));
    }
    eprintln!("  Run `base --help` for core commands.");
    // 127 = "command not found" (shell convention).
    std::process::exit(127);
}

/// Closest registry command name within edit distance 2, if any.
fn did_you_mean(name: &str, registry: &CommandRegistry) -> Option<String> {
    registry
        .list()
        .iter()
        .map(|c| (levenshtein(name, &c.name), c.name.clone()))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, n)| n)
}

/// Iterative Levenshtein edit distance (two-row).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

// ─── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_contains_core_commands() {
        let r = reserved_command_names();
        for name in ["task", "recall", "learn", "graph", "doctor", "extension", "help"] {
            assert!(r.contains(name), "expected '{name}' to be reserved");
        }
    }

    #[test]
    fn reserved_contains_aliases() {
        // Visible aliases must be reserved too, else a plugin could shadow `base t`.
        for alias in ["t", "p", "m", "ext", "dash", "cmd"] {
            assert!(is_reserved(alias), "expected alias '{alias}' to be reserved");
        }
    }

    #[test]
    fn unknown_name_is_not_reserved() {
        assert!(!is_reserved("frobnicate"));
        assert!(!is_reserved("hello"));
    }

    // ─── Registry ────────────────────────────────────────────

    use crate::extension::CommandSpec;

    fn ext_with_commands(name: &str, cmds: Vec<(&str, &str)>) -> ExtensionDef {
        ExtensionDef {
            name: name.into(),
            version: "0.1.0".into(),
            description: "test".into(),
            framework_dir: Some("/opt/fw".into()),
            state_dir: None,
            hooks: None,
            commands: cmds
                .into_iter()
                .map(|(n, h)| CommandSpec {
                    name: n.into(),
                    handler: h.into(),
                    description: format!("{n} command"),
                    usage: None,
                })
                .collect(),
            dist: None,
            source_path: None,
        }
    }

    #[test]
    fn registry_registers_and_resolves() {
        let exts = vec![ext_with_commands("nano", vec![("nano-banana", "bin/nb.mjs")])];
        let reg = build_registry(&exts);
        assert_eq!(reg.list().len(), 1);
        let cmd = reg.resolve("nano-banana").expect("should resolve");
        assert_eq!(cmd.ext_name, "nano");
        // handler resolved against framework_dir
        assert_eq!(cmd.handler.to_string_lossy(), "/opt/fw/bin/nb.mjs");
        assert!(reg.resolve("nonexistent").is_none());
    }

    #[test]
    fn registry_refuses_reserved_names() {
        // A plugin trying to claim `task` (a core command) must be skipped.
        let exts = vec![ext_with_commands("evil", vec![("task", "bin/evil"), ("ok", "bin/ok")])];
        let reg = build_registry(&exts);
        assert!(reg.resolve("task").is_none(), "core command must not be shadowable");
        assert!(reg.resolve("ok").is_some(), "non-reserved sibling still registers");
        assert_eq!(reg.list().len(), 1);
    }

    #[test]
    fn registry_first_wins_on_collision() {
        // Two extensions both declare `dupe` — first one wins.
        let exts = vec![
            ext_with_commands("aaa", vec![("dupe", "bin/first")]),
            ext_with_commands("bbb", vec![("dupe", "bin/second")]),
        ];
        let reg = build_registry(&exts);
        assert_eq!(reg.list().len(), 1);
        let cmd = reg.resolve("dupe").unwrap();
        assert_eq!(cmd.ext_name, "aaa");
        assert_eq!(cmd.handler.to_string_lossy(), "/opt/fw/bin/first");
    }

    #[test]
    fn handler_absolute_path_used_as_is() {
        let exts = vec![ext_with_commands("abs", vec![("c", "/usr/local/bin/c")])];
        let reg = build_registry(&exts);
        assert_eq!(reg.resolve("c").unwrap().handler.to_string_lossy(), "/usr/local/bin/c");
    }

    #[test]
    fn handler_resolves_against_source_dir_when_no_framework_dir() {
        let mut ext = ext_with_commands("src", vec![("c", "bin/c")]);
        ext.framework_dir = None;
        ext.source_path = Some(PathBuf::from("/home/u/.base-gbl/extensions/src.toml"));
        let reg = build_registry(&[ext]);
        assert_eq!(
            reg.resolve("c").unwrap().handler.to_string_lossy(),
            "/home/u/.base-gbl/extensions/bin/c"
        );
    }

    // ─── Executor + env contract ─────────────────────────────

    #[test]
    fn parse_env_basic() {
        let body = "\
# a comment
GEMINI_API_KEY=abc123
export QUOTED=\"hello world\"
SINGLE='single quoted'

EMPTY_VALUE=
no_equals_line
DUP=first
DUP=second
";
        let env = parse_env(body);
        let get = |k: &str| env.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.as_str());
        assert_eq!(get("GEMINI_API_KEY"), Some("abc123"));
        assert_eq!(get("QUOTED"), Some("hello world"));
        assert_eq!(get("SINGLE"), Some("single quoted"));
        assert_eq!(get("EMPTY_VALUE"), Some(""));
        assert_eq!(get("DUP"), Some("first")); // first wins
        assert!(env.iter().all(|(k, _)| k != "no_equals_line"));
    }

    #[test]
    fn base_contract_sets_workspace_and_graph() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join(".base")).unwrap();
        std::fs::write(ws.path().join(".base").join("graph.nq"), "").unwrap();

        let contract = base_contract(ws.path());
        let get = |k: &str| contract.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.clone());

        // BASE_WORKSPACE is the dir containing .base/ (canonicalize for macos /private symlink etc.)
        let want_ws = std::fs::canonicalize(ws.path()).unwrap();
        let got_ws = std::fs::canonicalize(get("BASE_WORKSPACE").unwrap()).unwrap();
        assert_eq!(got_ws, want_ws);
        assert!(get("BASE_GRAPH_PATH").unwrap().ends_with("graph.nq"));
        assert!(get("BASE_GLOBAL_DIR").is_some());
    }

    #[test]
    fn build_child_sets_args_and_extra_env() {
        let cwd = tempfile::tempdir().unwrap();
        let extra = vec![("PLUGIN_TEST_VAR".to_string(), "xyz".to_string())];
        let cmd = build_child(
            Path::new("/bin/echo"),
            &["generate".into(), "--prompt".into(), "hi".into()],
            cwd.path(),
            &extra,
        );
        // Program + args
        assert_eq!(cmd.get_program(), std::ffi::OsStr::new("/bin/echo"));
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, vec!["generate", "--prompt", "hi"]);
        // Env contract + extra present
        let envs: std::collections::HashMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains_key("BASE_WORKSPACE"));
        assert!(envs.contains_key("BASE_GLOBAL_DIR"));
        assert_eq!(envs.get("PLUGIN_TEST_VAR"), Some(&Some("xyz".to_string())));
    }

    #[test]
    fn real_subprocess_round_trip() {
        // A real handler: a shebang'd shell script that echoes an injected env
        // var + its first two args, then exits with a code derived from arg 3.
        // Proves: args forwarded, env contract reaches the child, exit propagates.
        let dir = tempfile::tempdir().unwrap();
        let handler = dir.path().join("handler.sh");
        std::fs::write(
            &handler,
            "#!/usr/bin/env bash\necho \"WS=$BASE_WORKSPACE\"\necho \"KEY=$INJECTED_KEY\"\necho \"ARGS=$1 $2\"\nexit ${3:-0}\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&handler).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&handler, perms).unwrap();
        }

        let extra = vec![("INJECTED_KEY".to_string(), "secret42".to_string())];
        let out = build_child(
            &handler,
            &["alpha".into(), "beta".into(), "7".into()],
            dir.path(),
            &extra,
        )
        .output()
        .unwrap();

        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("KEY=secret42"), "env injected; got: {stdout}");
        assert!(stdout.contains("ARGS=alpha beta"), "args forwarded; got: {stdout}");
        assert!(stdout.contains("WS="), "BASE_WORKSPACE present; got: {stdout}");
        assert_eq!(out.status.code(), Some(7), "child exit code propagates");
    }

    // ─── Discovery / did-you-mean ────────────────────────────

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("nano", "nano"), 0);
        assert_eq!(levenshtein("nano", "nanp"), 1);
        assert_eq!(levenshtein("generate", "generat"), 1);
        assert_eq!(levenshtein("abc", "xyz"), 3);
    }

    #[test]
    fn did_you_mean_suggests_near_match() {
        let exts = vec![ext_with_commands("nb", vec![("nano-banana", "bin/nb")])];
        let reg = build_registry(&exts);
        assert_eq!(did_you_mean("nano-banan", &reg).as_deref(), Some("nano-banana")); // dist 1
        assert!(did_you_mean("xyzzy", &reg).is_none()); // far away → no suggestion
    }

    // ─── Bundle install ──────────────────────────────────────

    #[test]
    fn rewrite_framework_dir_replaces_preserving_comments() {
        let original = "# header comment\n[extension]\nname = \"x\"\nframework_dir = \"~/old/path\"\nversion = \"0.1.0\"\n";
        let out = rewrite_framework_dir(original, "/home/u/.base-gbl/plugins/x").unwrap();
        assert!(out.contains("framework_dir = \"/home/u/.base-gbl/plugins/x\""));
        assert!(out.contains("# header comment"), "comments preserved");
        assert!(out.contains("name = \"x\""));
        assert!(!out.contains("~/old/path"));
    }

    #[test]
    fn rewrite_framework_dir_windows_path_roundtrips() {
        // A Windows dest (`\Users`) written raw would emit an invalid `\U`
        // escape; the manifest must stay valid TOML and parse back to the
        // exact path.
        let original =
            "[extension]\nname = \"x\"\nversion = \"0.1.0\"\ndescription = \"d\"\nframework_dir = \"~/old\"\n";
        let dest = r"C:\Users\chris\.base-gbl\plugins\x";
        let out = rewrite_framework_dir(original, dest).unwrap();
        let parsed: extension::ExtensionFile =
            toml::from_str(&out).expect("repointed manifest must be valid TOML");
        assert_eq!(parsed.extension.framework_dir.as_deref(), Some(dest));
    }

    #[test]
    fn toml_basic_string_escapes_specials() {
        assert_eq!(toml_basic_string(r"C:\a"), r#""C:\\a""#);
        assert_eq!(toml_basic_string("/home/u/x"), r#""/home/u/x""#);
        assert_eq!(toml_basic_string("a\"b"), r#""a\"b""#);
    }

    #[test]
    fn rewrite_framework_dir_errors_when_absent() {
        let original = "[extension]\nname = \"x\"\nversion = \"0.1.0\"\n";
        assert!(rewrite_framework_dir(original, "/dest").is_err());
    }

    #[test]
    fn resolve_framework_dir_relative_to_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("base-extension.toml");
        std::fs::write(&manifest, "x").unwrap();
        // "." → the manifest's own directory (canonicalized)
        let got = resolve_framework_dir(&manifest, ".");
        let want = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(got, want);
        // absolute used as-is
        assert_eq!(resolve_framework_dir(&manifest, "/opt/x"), PathBuf::from("/opt/x"));
    }

    #[test]
    fn copy_dir_all_excludes_and_preserves_exec() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("bin")).unwrap();
        std::fs::create_dir_all(src.path().join(".git")).unwrap();
        std::fs::write(src.path().join(".git/config"), "x").unwrap();
        let handler = src.path().join("bin/h.mjs");
        std::fs::write(&handler, "#!/usr/bin/env node\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&handler, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let n = copy_dir_all(src.path(), dst.path(), &[".git"]).unwrap();
        assert_eq!(n, 1, "only bin/h.mjs copied; .git excluded");
        assert!(dst.path().join("bin/h.mjs").exists());
        assert!(!dst.path().join(".git").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dst.path().join("bin/h.mjs"))
                .unwrap()
                .permissions()
                .mode();
            assert!(mode & 0o111 != 0, "exec bit preserved on copy");
        }
    }

    // Phase 53: the bundle copy must drop build-artifact dirs (a Rust `target/`
    // swept 205 MB on a real install) while keeping real source.
    #[test]
    fn copy_dir_all_excludes_build_dirs() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        // Real source to keep.
        std::fs::create_dir_all(src.path().join("src")).unwrap();
        std::fs::write(src.path().join("src/main.rs"), "fn main() {}").unwrap();
        // Build/dep/VCS dirs to drop.
        for d in ["target", "node_modules", ".git", "dist", "build"] {
            std::fs::create_dir_all(src.path().join(d)).unwrap();
            std::fs::write(src.path().join(d).join("junk"), "x").unwrap();
        }

        let exclude = ["target", "node_modules", ".git", "dist", "build", "generated_imgs"];
        let n = copy_dir_all(src.path(), dst.path(), &exclude).unwrap();

        assert_eq!(n, 1, "only src/main.rs copied");
        assert!(dst.path().join("src/main.rs").exists());
        for d in ["target", "node_modules", ".git", "dist", "build"] {
            assert!(!dst.path().join(d).exists(), "{d} must be excluded");
        }
    }
}
