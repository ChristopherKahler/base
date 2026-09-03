//! Every app gets a code map without anyone asking for one.
//!
//! Chris, 2026-09-01: "anytime a dev project is started, it auto creates the
//! AST map ... users should never have to ask and no app should ever go
//! without one." This module is the one place that decides WHICH folder is
//! the app and WHETHER to build or refresh its map. The hooks and the CLI only
//! call in; none of them carries a rule of its own.
//!
//! Where a map comes from, in order of first contact:
//!   * session-start — the cwd's app root, or the cwd itself when nothing
//!     marks it yet (a folder of source files nobody has `git init`ed is still
//!     an app: adopting it creates `.base-ast/`, which marks it from then on);
//!   * pre-tool-use — the first Read / Edit / Write / Grep of a file inside an
//!     app that has no map, whatever the session's cwd — and the first Bash
//!     command that `cd`s into one or names a path inside one, because a
//!     session booted in a workspace navigates with the shell;
//!   * a Windows session reaching a WSL tree through `wsl …` hands the same
//!     check to the WSL base (`base ast ensure`), since no Windows hook can
//!     see a Linux path;
//!   * Stop — the cwd's app and every app edited this turn, refreshed;
//!   * `base scaffold` and `base project add --path` — the folder they just
//!     registered.
//!
//! What a hook never maps: the OS temp directory, the caches, `AppData`,
//! `node_modules`, `~/.claude`, `~/.base-gbl`, the home directory — this OS's
//! or, across a WSL boundary, the other's — a filesystem root, the well-known
//! user folders (Desktop, Documents, Downloads, a cloud-drive root),
//! and a folder that only HOLDS apps — a `.base` workspace whose repos sit one
//! or two levels down (`~/ops-sys/toolbox` keeps fifty under `frameworks/`,
//! `apps/`, …). Mapping one of those walks every project on the machine into
//! a single file, which is the one failure worse than a missing map. A marked
//! app that contains other marked apps maps only its own tree: the extractor
//! skips any subfolder carrying `.git`, `.paul` or `.base-ast`.
//!
//! Every build is detached and debounced; a hook never waits. A failed build
//! leaves `.base-ast/.last-error` behind so the next session start can say why
//! instead of promising a map that never lands.
//!
//! An unattended build passes `--yes`, so it never stops at the extractor's own
//! file-count prompt — which is why the size fuse decides BEFORE the spawn
//! rather than relying on it. A first build measures the tree, and a root over
//! `FUSE_SOURCE_FILES` / `FUSE_ENTRIES` is not built at all: the counts and the
//! exact by-hand command go into `.base-ast/.needs-confirm` and the next
//! session start prints them. Chris, 2026-09-03, after a hook spent thirty
//! minutes extracting `%TEMP%`: "Base is set to automatically make graphs, this
//! will blow up people's computers."

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const DEBOUNCE_SECS: u64 = 20;

/// Set by tests: decide everything, spawn nothing. A test binary that spawned
/// itself as `base sync` would run the test suite instead of the extractor.
const NO_SPAWN_ENV: &str = "BASE_AST_NO_SPAWN";

/// Directory names never walked when looking for source files. Mirrors the
/// extractor's `_NOISE_DIRS` (scripts/ast/detect.py) so "has code" here and
/// "has files to extract" there agree.
const NOISE_DIRS: &[&str] = &[
    "node_modules", ".git", ".hg", ".svn", "__pycache__", ".mypy_cache",
    ".pytest_cache", ".tox", ".eggs", "dist", "build", ".next", ".nuxt",
    ".output", "target", ".cargo", "vendor", ".bundle", "venv", ".venv",
    "env", ".env", ".direnv", "coverage", ".coverage", ".nyc_output",
    ".turbo", ".cache", ".parcel-cache", ".webpack", "tmp", "temp",
    ".terraform", ".pulumi", "zig-cache", "zig-out", ".base-ast-cache",
];

/// Extensions that make a bare folder a code project worth adopting.
/// Deliberately narrower than what the extractor parses: a folder whose only
/// "source" is a config.json is a folder, not an app.
const CODE_EXTS: &[&str] = &[
    "rs", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "go", "java", "kt", "kts",
    "scala", "groovy", "c", "h", "cpp", "cc", "cxx", "hpp", "cs", "rb", "php",
    "swift", "dart", "lua", "zig", "ex", "exs", "jl", "sql", "ps1", "psm1", "sh",
    "bash", "zsh", "vue", "svelte", "astro", "r", "f90", "pas", "m", "mm", "v", "sv",
];

/// Folders directly under home that are a cloud drive's root, not a project.
const CLOUD_ROOTS: &[&str] = &[
    "OneDrive", "Dropbox", "Google Drive", "GoogleDrive", "iCloud Drive", "iCloudDrive", "Nextcloud", "Box",
];

/// Path components that make a folder unmappable wherever one appears, matched
/// case-insensitively on every platform. This half of the rule is what works
/// across the WSL boundary: a Windows `%TEMP%` reaches a WSL hook as
/// `/mnt/c/Users/<name>/AppData/Local/Temp`, where not one of the environment
/// variables in [`temp_roots`] is set.
const NEVER_MAP_SEGMENTS: &[(&str, &str)] = &[
    ("node_modules", "a node_modules tree"),
    (".cache", "a cache directory"),
    (".claude", "the Claude Code state directory"),
    (".base-gbl", "base's own global tier"),
];

/// `tmp` / `temp` as a path component. Held apart from [`NEVER_MAP_SEGMENTS`]
/// because every test fixture workspace is built under the system temp dir —
/// see [`never_map_reason`].
const TEMP_SEGMENTS: &[&str] = &["tmp", "temp"];

/// The children of `AppData` that are never mapped. Matched as an ordered pair
/// so a project folder merely called `Local` is not caught.
const APPDATA_CHILDREN: &[&str] = &["local", "locallow", "roaming"];

/// A hook builds no tree larger than this without being asked first. Measured
/// 2026-09-03 against the largest real app on the machine this fork came from
/// (`~/ops-sys/toolbox/frameworks/05-exp-cadre`: 209 source files, 10,853
/// entries), so both fuses sit several times above anything real here.
/// Tripping one costs a prompt, never a map, so erring low is cheap.
const FUSE_SOURCE_FILES: usize = 5_000;
const FUSE_ENTRIES: usize = 50_000;

/// How long a tripped fuse is believed before the tree is measured again.
const NEEDS_CONFIRM_SECS: u64 = 24 * 60 * 60;

/// How deep and how far the "does this folder hold code" probe looks.
const PROBE_DEPTH: usize = 3;
const PROBE_ENTRIES: usize = 2000;

/// What a hook does about an app root's code map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapPlan {
    /// The root is the operator's home directory: never mapped by a hook.
    SkipHome,
    /// The root is a workspace that only holds other apps: never mapped.
    SkipHub,
    /// The root is, or sits under, a location a hook never maps — the OS temp
    /// directory, a cache, `AppData`, `node_modules`, the other OS's home.
    /// Never built AND never refreshed: a single bad adoption plants
    /// `.base-ast/`, and without this the marker would make itself permanent.
    SkipNeverMap(&'static str),
    /// A sync ran within the debounce window; the caller may requeue.
    Debounced,
    /// The tree is too large to extract unattended. Nothing was spawned;
    /// `.base-ast/.needs-confirm` holds the counts and the exact command to
    /// run by hand, and the session-start notice prints it.
    NeedsConfirm,
    /// No `.base-ast/ast.ttl` yet: build the first map, registered.
    Build,
    /// A map exists: refresh it in the background, unregistered.
    Refresh,
}

/// The build/refresh decision, with every input explicit so it can be tested
/// without a filesystem: `home` is the operator's home, `never` the reason
/// this root is off limits (see [`never_map_reason`]), `mapped` whether
/// `ast.ttl` exists, `debounced` whether `.last-sync` is fresher than the window.
pub fn plan_map(
    root: &Path,
    home: Option<&Path>,
    never: Option<&'static str>,
    mapped: bool,
    debounced: bool,
) -> MapPlan {
    if home.is_some_and(|h| h == root) {
        return MapPlan::SkipHome;
    }
    // Before `mapped` and before `debounced`, deliberately: an `ast.ttl` that
    // already sits inside a never-mapped root must not license a refresh.
    if let Some(why) = never {
        return MapPlan::SkipNeverMap(why);
    }
    if debounced {
        return MapPlan::Debounced;
    }
    if mapped { MapPlan::Refresh } else { MapPlan::Build }
}

/// Make sure `root` has a code map and that it is fresh: build it on first
/// contact, refresh it afterwards, debounced, never for the home directory or
/// a workspace hub. Returns what was decided; `Build` and `Refresh` mean a
/// detached sync is now running.
pub fn ensure_app_map(root: &Path) -> MapPlan {
    // First, and before any filesystem probing: a never-mapped root must not
    // even be walked, and must never have `.base-ast/` written into it.
    let never = never_map_reason(root);
    if never.is_some() {
        return plan_map(root, crate::home::real_home().as_deref(), never, false, false);
    }
    if is_workspace_hub(root) {
        return MapPlan::SkipHub;
    }
    let base_ast = root.join(".base-ast");
    let mapped = base_ast.join("ast.ttl").is_file();
    let marker = base_ast.join(".last-sync");
    let home = crate::home::real_home();
    // One build at a time per app: a 13,000-file tree takes minutes, and
    // session-start, first contact and Stop all arrive inside that window.
    // `.building` is written before the spawn and removed by `base sync`
    // when it finishes, success or failure; a lock older than BUILD_LOCK_SECS
    // is a crashed build and no longer counts.
    let building = recently(&base_ast.join(".building"), BUILD_LOCK_SECS);
    let plan = plan_map(root, home.as_deref(), never, mapped, building || recently_synced(&marker));
    // The fuse, on a FIRST build only. An existing map means the extractor
    // already got through this tree once; re-measuring up to 50,000 entries on
    // every Stop-hook refresh would cost more than the bug it guards against.
    if plan == MapPlan::Build && fuse_blocks(root, &base_ast) {
        return MapPlan::NeedsConfirm;
    }
    match plan {
        MapPlan::Build | MapPlan::Refresh => {
            let _ = std::fs::create_dir_all(&base_ast);
            let _ = std::fs::write(&marker, b"");
            let _ = std::fs::write(base_ast.join(".building"), b"");
            spawn_sync(root, plan == MapPlan::Build);
        }
        MapPlan::SkipHome
        | MapPlan::SkipHub
        | MapPlan::SkipNeverMap(_)
        | MapPlan::NeedsConfirm
        | MapPlan::Debounced => {}
    }
    plan
}

/// First contact only: build a map when `root` has none, never refresh one.
/// `None` means a map is already there — the cheap answer, taken on every
/// tool call that names a file.
pub fn ensure_first_map(root: &Path) -> Option<MapPlan> {
    if root.join(".base-ast").join("ast.ttl").is_file() {
        return None;
    }
    Some(ensure_app_map(root))
}

/// Which folder a session (or a Stop, or a registration) maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootPlan {
    /// An app root marked by `.git` / `.paul` / `.base-ast`, or a `.base`
    /// workspace that is itself the app.
    Marked(PathBuf),
    /// Nothing marks it and nothing above it (below home) does either: the
    /// folder itself is the app. Building creates `.base-ast/`, the marker.
    Adopt(PathBuf),
    /// The home directory: never.
    Home,
    /// A user folder, a filesystem root, or a folder that holds other apps.
    Hub,
    /// The cwd is, or sits under, a location a hook never maps.
    NeverMap(&'static str),
    /// No source files under it yet. The Stop hook looks again every turn.
    Empty,
}

/// Everything [`plan_root`] needs, gathered once so the decision is pure.
#[derive(Debug, Clone, Copy)]
pub struct RootFacts<'a> {
    pub cwd: &'a Path,
    pub home: Option<&'a Path>,
    /// Nearest `.git` / `.paul` / `.base-ast` root at or above cwd, below home.
    pub strong: Option<&'a Path>,
    /// Nearest `.base`-only root at or above cwd, below home (only consulted
    /// when `strong` is `None`).
    pub weak: Option<&'a Path>,
    /// That `.base` root directly contains other marked folders — a workspace
    /// of apps rather than an app.
    pub weak_is_hub: bool,
    /// Why the cwd is off limits entirely (temp, a cache, `AppData`,
    /// `node_modules`, the other OS's home), or `None`.
    pub never: Option<&'static str>,
    /// cwd is home, a filesystem root, Desktop / Documents / Downloads /
    /// Pictures / Music / Videos / Public, or a cloud drive's root.
    pub user_folder: bool,
    /// cwd directly contains a marked folder.
    pub child_apps: bool,
    /// At least one code file under cwd (bounded probe).
    pub sources: bool,
}

/// The one rule. Read top to bottom.
pub fn plan_root(f: &RootFacts<'_>) -> RootPlan {
    if f.home.is_some_and(|h| h == f.cwd) {
        return RootPlan::Home;
    }
    // Before the markers: a `.git` inside the temp dir is still the temp dir.
    if let Some(why) = f.never {
        return RootPlan::NeverMap(why);
    }
    if let Some(s) = f.strong {
        return RootPlan::Marked(s.to_path_buf());
    }
    if let Some(w) = f.weak
        && !f.weak_is_hub
    {
        return RootPlan::Marked(w.to_path_buf());
    }
    // Unmarked, or inside a workspace that only holds apps: the folder itself
    // is the candidate, unless it is a place that holds many projects.
    if f.user_folder || f.child_apps {
        return RootPlan::Hub;
    }
    if !f.sources {
        return RootPlan::Empty;
    }
    RootPlan::Adopt(f.cwd.to_path_buf())
}

/// Gather the facts for `cwd` and decide.
pub fn session_root(cwd: &Path) -> RootPlan {
    let home = crate::home::home_root();
    let home = home.as_deref();
    let strong = nearest_below_home(cwd, home, has_strong_marker);
    let weak = if strong.is_none() {
        nearest_below_home(cwd, home, |d| d.join(".base").is_dir())
    } else {
        None
    };
    let facts = RootFacts {
        cwd,
        home,
        strong: strong.as_deref(),
        weak: weak.as_deref(),
        weak_is_hub: weak.as_deref().is_some_and(has_child_apps),
        never: never_map_reason(cwd),
        user_folder: is_user_folder(cwd, home),
        child_apps: has_child_apps(cwd),
        sources: has_code_files(cwd),
    };
    plan_root(&facts)
}

/// Session start: make sure the cwd's app has a map, and say so when this is
/// the first one — or when the automatic build has been failing, in which
/// case the reason is worth more than another promise.
pub fn session_start_notice(cwd: &Path) -> Option<String> {
    let (root, adopted) = match session_root(cwd) {
        RootPlan::Marked(r) => (r, false),
        RootPlan::Adopt(r) => (r, true),
        RootPlan::NeverMap(why) => {
            return Some(format!(
                "[AST] {} is never mapped ({why}) — no code map is built here. \
                 Run `base sync --ast --target <path>` by hand if you really want one.",
                cwd.display()
            ));
        }
        RootPlan::Home | RootPlan::Hub | RootPlan::Empty => return None,
    };
    let outcome = ensure_app_map(&root);
    let base_ast = root.join(".base-ast");
    if !base_ast.join("ast.ttl").is_file()
        && let Some(why) = last_error(&base_ast)
    {
        return Some(format!(
            "[AST] the automatic code-map build for {} has been failing: {why} \
             (full output in {}). It retries every turn until it succeeds.",
            root.display(),
            base_ast.join(".last-error").display()
        ));
    }
    match (outcome, adopted) {
        (MapPlan::Build, true) => Some(format!(
            "[AST] {} has no .git yet — mapping it anyway: its code map is building in the \
             background under {}/ (git init any time; the map follows).",
            root.display(),
            base_ast.display()
        )),
        (MapPlan::NeedsConfirm, _) => Some(format!(
            "[AST] no code map for {}: {}",
            root.display(),
            std::fs::read_to_string(base_ast.join(".needs-confirm"))
                .unwrap_or_default()
                .trim()
                .to_string()
        )),
        (MapPlan::Build, false) => Some(format!(
            "[AST] no code map for {} yet — building one now in the background \
             (base sync --ast); `base ast query` and the file-map injection answer once it lands.",
            root.display()
        )),
        _ => None,
    }
}

/// A `.base`-only workspace whose direct children are apps: a hub, not an app.
pub fn is_workspace_hub(dir: &Path) -> bool {
    !has_strong_marker(dir) && has_child_apps(dir)
}

/// `.git` (a dir, or a submodule's gitlink file), `.paul`, or a `.base-ast`
/// map — the markers that make a folder an app on their own.
pub fn has_strong_marker(dir: &Path) -> bool {
    dir.join(".git").exists() || dir.join(".paul").is_dir() || dir.join(".base-ast").is_dir()
}

fn is_marked(dir: &Path) -> bool {
    has_strong_marker(dir) || dir.join(".base").is_dir()
}

/// A marked folder sits within two levels below `dir`. Two, not one: a
/// workspace usually groups its repos (`frameworks/x`, `apps/y`), and a probe
/// that only saw direct children called `~/ops-sys/toolbox` an app the moment
/// its one directly-marked child went away. Bounded like the code probe.
pub fn has_child_apps(dir: &Path) -> bool {
    holds_apps(dir, HUB_DEPTH)
}

const HUB_DEPTH: usize = 2;

fn holds_apps(dir: &Path, depth: usize) -> bool {
    let mut seen = 0usize;
    let mut stack: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
    while let Some((d, lvl)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > PROBE_ENTRIES {
                return false;
            }
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            if is_marked(&path) {
                return true;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if lvl + 1 < depth && !name.starts_with('.') && !is_noise_dir(&name) {
                stack.push((path, lvl + 1));
            }
        }
    }
    false
}

/// First contact through any path: the app that contains `path` gets a map
/// if it has none. `None` when the path is in no app or the app is mapped.
pub fn first_contact(path: &Path) -> Option<MapPlan> {
    let root = crate::config::ast_app_root(path)?;
    ensure_first_map(&root)
}

/// First contact from a path a Bash command merely NAMED. Same decisions as
/// [`first_contact`], except the resolved root must already be marked: a
/// command's `cd` is transient, a session's cwd is a choice. Booting a session
/// in an unmarked folder of source still adopts it (Chris, 2026-09-01) — but
/// passing through one on the way to somewhere else does not.
///
/// `config::ast_app_root` happens to require a marker today, so the check here
/// is belt and braces. That is deliberate: the rule now lives somewhere it can
/// be tested, instead of being an accident of resolution that a later change to
/// `ast_app_root` could quietly undo.
pub fn bash_first_contact(path: &Path) -> Option<MapPlan> {
    let root = crate::config::ast_app_root(path)?;
    if !is_marked(&root) {
        return None;
    }
    ensure_first_map(&root)
}

/// First contact that builds in the FOREGROUND and returns when the map has
/// landed. For a caller whose process must outlive the build: a `wsl -e sh`
/// invocation from Windows ends the moment its shell exits and takes every
/// detached child with it, so a delegated build has to run inside it.
/// Same decisions as [`first_contact`]; only the spawn differs.
pub fn first_contact_wait(path: &Path) -> Option<MapPlan> {
    let root = crate::config::ast_app_root(path)?;
    if let Some(why) = never_map_reason(&root) {
        return Some(MapPlan::SkipNeverMap(why));
    }
    let base_ast = root.join(".base-ast");
    if base_ast.join("ast.ttl").is_file() {
        return None;
    }
    if is_workspace_hub(&root) {
        return Some(MapPlan::SkipHub);
    }
    let home = crate::home::real_home();
    if recently(&base_ast.join(".building"), BUILD_LOCK_SECS) {
        return Some(MapPlan::Debounced);
    }
    if fuse_blocks(&root, &base_ast) {
        return Some(MapPlan::NeedsConfirm);
    }
    let plan = plan_map(&root, home.as_deref(), None, false, false);
    if plan != MapPlan::Build {
        return Some(plan);
    }
    let _ = std::fs::create_dir_all(&base_ast);
    let _ = std::fs::write(base_ast.join(".last-sync"), b"");
    let _ = std::fs::write(base_ast.join(".building"), b"");
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe)
            .arg("sync").arg("--ast").arg("--yes").arg("--target").arg(&root)
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Some(MapPlan::Build)
}

/// The paths a Bash command touches, as far as text can tell: `cd` targets
/// (which also re-base what follows them), and every token that looks like a
/// path. Pure — the caller keeps the ones that exist. `~` is `home`; a Git
/// Bash `/c/…` becomes `C:/…` on Windows; relative tokens resolve against the
/// last `cd` in the command, else `cwd`.
pub fn bash_paths(command: &str, cwd: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    const MAX_TOKENS: usize = 48;
    let mut out: Vec<PathBuf> = Vec::new();
    let mut base = cwd.to_path_buf();
    let mut seen = 0usize;
    for segment in command.split([';', '\n', '|', '&']) {
        let mut tokens = segment.split_whitespace().map(|t| t.trim_matches(['"', '\'', '(', ')']));
        let mut first = tokens.next();
        // skip leading env assignments: FOO=bar cmd …
        while first.is_some_and(|t| t.contains('=') && !t.starts_with('-') && !t.contains('/')) {
            first = tokens.next();
        }
        let Some(first) = first else { continue };
        let rest: Vec<&str> = tokens.collect();
        if first == "cd" || first == "pushd" {
            if let Some(target) = rest.first().and_then(|t| resolve_token(t, &base, home)) {
                base = target.clone();
                push_unique(&mut out, target);
            }
            continue;
        }
        for tok in std::iter::once(first).chain(rest) {
            seen += 1;
            if seen > MAX_TOKENS {
                return out;
            }
            if let Some(p) = resolve_token(tok, &base, home) {
                push_unique(&mut out, p);
            }
        }
    }
    out
}

fn push_unique(out: &mut Vec<PathBuf>, p: PathBuf) {
    if !out.contains(&p) {
        out.push(p);
    }
}

/// A token that reads as a path, resolved; flags, globs, variables and URLs
/// are not paths.
fn resolve_token(tok: &str, base: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let t = tok.trim_end_matches([',', ':']);
    if t.is_empty() || t.starts_with('-') || t.contains("://") || t.contains(['*', '$', '{', '`']) {
        return None;
    }
    let looks_like_path = t.contains('/') || t.contains('\\') || t == "." || t == ".." || t.starts_with('~');
    if !looks_like_path {
        return None;
    }
    if t == "~" || t.starts_with("~/") {
        return home.map(|h| h.join(t.trim_start_matches('~').trim_start_matches('/')));
    }
    // Git Bash spells C:\ as /c/ — a Windows binary must see the drive.
    if cfg!(windows)
        && let Some(rest) = t.strip_prefix('/')
        && let Some((drive, tail)) = rest.split_once('/')
        && drive.len() == 1
        && drive.chars().all(|c| c.is_ascii_alphabetic())
    {
        return Some(PathBuf::from(format!("{}:/{}", drive.to_ascii_uppercase(), tail)));
    }
    let p = Path::new(t);
    Some(if p.is_absolute() { p.to_path_buf() } else { base.join(p) })
}

/// Linux-side paths named in a command that runs through `wsl`: what a
/// Windows session hands to the WSL base for its own first-contact check.
pub fn linux_paths(command: &str) -> Vec<String> {
    const MAX: usize = 8;
    let mut out: Vec<String> = Vec::new();
    if !command.split_whitespace().any(|t| t.trim_matches('"') == "wsl" || t.ends_with("wsl.exe")) {
        return out;
    }
    for raw in command.split(|c: char| c.is_whitespace() || c == ';' || c == '|' || c == '&' || c == '(' || c == ')' || c == '"' || c == '\'') {
        let t = raw.trim_end_matches([',', ':']);
        if (t.starts_with("~/") || t.starts_with("/home/") || t.starts_with("/mnt/")) && !t.contains(['*', '$', '{', '`']) {
            let s = t.to_string();
            if !out.contains(&s) {
                out.push(s);
                if out.len() == MAX {
                    break;
                }
            }
        }
    }
    out
}

/// Windows only: ask the WSL base to give each Linux path's app a map if it
/// has none. Detached, one `wsl` process per path, and a path is asked about
/// at most once per `WSL_CONTACT_SECS` (a marker under `~/.base-gbl/`), so a
/// session full of `wsl` commands costs nothing after the first.
pub fn delegate_wsl_contact(paths: &[String]) {
    if !cfg!(windows) || paths.is_empty() || std::env::var_os(NO_SPAWN_ENV).is_some() {
        return;
    }
    let Some(dir) = crate::home::home_root().map(|h| h.join(".base-gbl").join(".wsl-contact")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    for p in paths {
        // A Linux path the WSL base would refuse anyway: do not pay for a
        // `wsl` process to be told no.
        if never_map_reason(Path::new(p)).is_some() {
            continue;
        }
        let marker = dir.join(fingerprint(p));
        if recently(&marker, WSL_CONTACT_SECS) {
            continue;
        }
        let _ = std::fs::write(&marker, p.as_bytes());
        let script = wsl_script(p);
        let _ = Command::new("wsl")
            .args(["-e", "sh", "-lc", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

const WSL_CONTACT_SECS: u64 = 600;

/// The one-liner the WSL shell runs for a Linux path. `~/x` must reach the
/// shell as `"$HOME/x"` — inside single quotes a tilde is a tilde, and the
/// WSL base would be asked about a folder that does not exist.
pub fn wsl_script(path: &str) -> String {
    let quoted = match path.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME/{}\"", rest.replace(['"', '$', '`'], "")),
        None => format!("'{}'", path.replace('\u{27}', "")),
    };
    format!("b=\"$HOME/.local/bin/base\"; [ -x \"$b\" ] || b=base; \"$b\" ast ensure --wait {quoted} >/dev/null 2>&1")
}

/// A `.building` lock older than this is a crashed build, not a running one.
const BUILD_LOCK_SECS: u64 = 30 * 60;

fn fingerprint(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn recently(marker: &Path, secs: u64) -> bool {
    std::fs::metadata(marker)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|e| e < Duration::from_secs(secs))
        .unwrap_or(false)
}

/// The deepest folder at or above `start`, stopping BEFORE home, for which
/// `hit` is true. A marker on home itself (a dotfiles repo, the usual
/// `.base`) never makes home an app.
fn nearest_below_home(start: &Path, home: Option<&Path>, hit: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if home.is_some_and(|h| h == dir) {
            return None;
        }
        if hit(&dir) {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Home, a filesystem root, a well-known user folder, or a cloud drive root.
fn is_user_folder(cwd: &Path, home: Option<&Path>) -> bool {
    if home.is_some_and(|h| h == cwd) || cwd.parent().is_none() {
        return true;
    }
    let known = [
        dirs::desktop_dir(),
        dirs::document_dir(),
        dirs::download_dir(),
        dirs::picture_dir(),
        dirs::audio_dir(),
        dirs::video_dir(),
        dirs::public_dir(),
        dirs::template_dir(),
    ];
    if known.iter().flatten().any(|k| same_dir(k, cwd)) {
        return true;
    }
    // A cloud drive's root directly under home ("OneDrive", "OneDrive - Acme").
    if let (Some(h), Some(parent), Some(name)) = (home, cwd.parent(), cwd.file_name().and_then(|n| n.to_str()))
        && same_dir(parent, h)
    {
        return CLOUD_ROOTS.iter().any(|r| name == *r || name.starts_with(&format!("{r} ")));
    }
    false
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        let s = s.trim_end_matches('/').to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

/// A directory name that is never walked. Case-insensitive: Windows spells
/// the same folder `Temp` and `temp`, `Build` and `build`, and "has code" here
/// has to agree with "has files to extract" in `scripts/ast/detect.py`.
pub fn is_noise_dir(name: &str) -> bool {
    NOISE_DIRS.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// Why a hook never maps `root`, or `None` when it may.
///
/// Chris, 2026-09-03, after a hook spent thirty minutes extracting `%TEMP%`:
/// "just make it not ast parse shit it obviously shouldn't." A root that IS or
/// sits UNDER one of these is never built and never refreshed — an `ast.ttl`
/// already inside one buys nothing, because a single bad adoption plants
/// `.base-ast/` and would otherwise make itself permanent.
///
/// Deliberately NOT wired into [`crate::config::ast_app_root`]: resolution
/// stays honest about which folder owns a path, and only the decision to build
/// changes. An explicit `base sync --ast --target <path>` from a human still
/// runs, with the extractor's own prompt.
pub fn never_map_reason(root: &Path) -> Option<&'static str> {
    never_map_reason_with(root, crate::home::isolation_active())
}

/// [`never_map_reason`] with the sandbox exemption as an explicit input, so
/// both halves are testable from a test binary — which is itself isolated, and
/// could otherwise never observe the temp rule it depends on.
pub fn never_map_reason_with(root: &Path, isolated: bool) -> Option<&'static str> {
    let parts = lower_components(root);

    // Always armed, in tests too.
    for (i, name) in parts.iter().enumerate() {
        if let Some(&(_, why)) = NEVER_MAP_SEGMENTS.iter().find(|(n, _)| n == name) {
            return Some(why);
        }
        if i > 0 && parts[i - 1] == "appdata" && APPDATA_CHILDREN.contains(&name.as_str()) {
            return Some("a Windows AppData directory");
        }
    }
    if is_foreign_home(&parts) {
        return Some("the other OS's home directory");
    }
    if let Some(why) = under_any(root, &state_roots()) {
        return Some(why);
    }

    // The system temp directory is also where every test builds its fixture
    // workspaces, and `home::within_sandbox` already defines it as this
    // process's sandbox. Disarm this half — and only this half — while
    // isolated. `isolation_active` is off for every `cargo build` and
    // `cargo run`, so production keeps the guard.
    if isolated {
        return None;
    }
    if parts.iter().any(|p| TEMP_SEGMENTS.contains(&p.as_str())) {
        return Some("a temp directory");
    }
    under_any(root, &temp_roots())
}

/// `/mnt/c/Users/<name>` — a Windows home seen from a WSL hook, which its own
/// `real_home()` (`/home/<user>`) cannot recognise, so `SkipHome` never fires.
/// Matched EXACTLY, never as a prefix: real projects live under that home and
/// must keep mapping normally.
fn is_foreign_home(parts: &[String]) -> bool {
    matches!(
        parts,
        [mnt, drive, users, _name]
            if mnt == "mnt"
                && users == "users"
                && drive.len() == 1
                && drive.chars().all(|c| c.is_ascii_alphabetic())
    )
}

/// Caches and per-user state that hold no project of anyone's: never mapped,
/// isolated process or not.
fn state_roots() -> Vec<(PathBuf, &'static str)> {
    let mut v: Vec<(PathBuf, &'static str)> = Vec::new();
    if let Some(p) = dirs::cache_dir() {
        v.push((p, "a cache directory"));
    }
    // Only on Windows: `dirs` maps these to `~/.config` and `~/.local/share`
    // on Linux, which are not what `%APPDATA%` means and not on the list.
    if cfg!(windows) {
        for p in [dirs::config_dir(), dirs::data_dir(), dirs::data_local_dir()].into_iter().flatten() {
            v.push((p, "a Windows AppData directory"));
        }
        for key in ["LOCALAPPDATA", "APPDATA"] {
            if let Some(val) = std::env::var_os(key)
                && !val.is_empty()
            {
                v.push((PathBuf::from(val), "a Windows AppData directory"));
            }
        }
    }
    if let Some(h) = crate::home::real_home() {
        v.push((h.join(".cache"), "a cache directory"));
        v.push((h.join(".claude"), "the Claude Code state directory"));
        v.push((h.join(".base-gbl"), "base's own global tier"));
    }
    v
}

/// Every spelling of "the temp directory" this process can see.
fn temp_roots() -> Vec<(PathBuf, &'static str)> {
    let mut v: Vec<(PathBuf, &'static str)> = vec![(std::env::temp_dir(), "the OS temp directory")];
    for key in ["TMPDIR", "TEMP", "TMP"] {
        if let Some(val) = std::env::var_os(key)
            && !val.is_empty()
        {
            v.push((PathBuf::from(val), "the OS temp directory"));
        }
    }
    v.push((PathBuf::from("/tmp"), "the OS temp directory"));
    v.push((PathBuf::from("/var/tmp"), "the OS temp directory"));
    v
}

fn under_any(root: &Path, roots: &[(PathBuf, &'static str)]) -> Option<&'static str> {
    roots.iter().find(|(anc, _)| is_under(root, anc)).map(|(_, why)| *why)
}

/// `root` is `anc` or sits inside it, compared at a component boundary so
/// `/tmpfoo` is not under `/tmp`. Both sides are canonicalised when that
/// succeeds — which is what makes `%TEMP%`'s 8.3 short form
/// `C:\Users\CHRIS~1\AppData\Local\Temp` match the long spelling — and
/// compared raw when it does not, since a candidate root need not exist yet.
fn is_under(root: &Path, anc: &Path) -> bool {
    let norm = |p: &Path| {
        let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let s = real.to_string_lossy().replace('\\', "/");
        let s = s.trim_end_matches('/').to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    let (r, a) = (norm(root), norm(anc));
    if a.is_empty() {
        return false;
    }
    r == a || r.starts_with(&format!("{a}/"))
}

/// A path's normal components, lowercased. Case folding is unconditional: every
/// name matched against them is either a canonical lowercase dot-directory or a
/// Windows name, and no project is legitimately called `node_modules`.
fn lower_components(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// At least one code file under `dir`, looking `PROBE_DEPTH` deep and at most
/// `PROBE_ENTRIES` entries, skipping the noise directories. Bounded so a huge
/// folder costs a few milliseconds, not a walk.
pub fn has_code_files(dir: &Path) -> bool {
    let mut seen = 0usize;
    let mut stack: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > PROBE_ENTRIES {
                return false;
            }
            let path = entry.path();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if depth + 1 < PROBE_DEPTH && !name.starts_with('.') && !is_noise_dir(&name) {
                    stack.push((path, depth + 1));
                }
            } else if ft.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| CODE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
            {
                return true;
            }
        }
    }
    false
}

/// What a bounded walk of a candidate root found. Counting stops the instant a
/// fuse trips, so both numbers are lower bounds once [`TreeSize::over_fuse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeSize {
    pub sources: usize,
    pub entries: usize,
}

impl TreeSize {
    pub fn over_fuse(&self) -> bool {
        self.sources > FUSE_SOURCE_FILES || self.entries > FUSE_ENTRIES
    }
}

/// Measure `root` the way the extractor will read it: noise directories and
/// dot-directories skipped, nested marked apps left to their own maps.
///
/// Unlike the `PROBE_ENTRIES` probes this walk is not capped at 2,000 — it has
/// to be able to count as far as [`FUSE_ENTRIES`] — but it stops the moment a
/// fuse trips, so the worst case is ~50,000 stats, and it is paid once, on a
/// first build, never on a refresh.
pub fn measure_tree(root: &Path) -> TreeSize {
    let mut size = TreeSize { sources: 0, entries: 0 };
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            size.entries += 1;
            if size.over_fuse() {
                return size;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if ft.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || is_noise_dir(&name) || has_strong_marker(&path) {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| CODE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
            {
                size.sources += 1;
                if size.over_fuse() {
                    return size;
                }
            }
        }
    }
    size
}

/// True when `root` is too large to extract unattended. Records the counts and
/// the exact command in `.base-ast/.needs-confirm` on the way out, and believes
/// a fresh one rather than re-measuring the same tree every turn.
///
/// `base sync --ast --yes` from a hook cannot get past this: the fuse decides
/// before anything is spawned, so `--yes` never reaches the extractor's own
/// file-count prompt. A human running `base sync --ast` by hand is unaffected
/// and still gets that prompt.
fn fuse_blocks(root: &Path, base_ast: &Path) -> bool {
    if recently(&base_ast.join(".needs-confirm"), NEEDS_CONFIRM_SECS) {
        return true;
    }
    let size = measure_tree(root);
    if !size.over_fuse() {
        return false;
    }
    let _ = std::fs::create_dir_all(base_ast);
    let _ = std::fs::write(base_ast.join(".needs-confirm"), needs_confirm_text(root, &size));
    true
}

/// The one line `.needs-confirm` holds, and the session-start notice reads back.
pub fn needs_confirm_text(root: &Path, size: &TreeSize) -> String {
    format!(
        "at least {} source files across {} entries, over the {}/{} a hook will build unattended. \
         Run `base sync --ast --target {}` to build it by hand.",
        size.sources,
        size.entries,
        FUSE_SOURCE_FILES,
        FUSE_ENTRIES,
        root.display()
    )
}

/// The last line the failed extractor wrote, for a one-line notice.
fn last_error(base_ast: &Path) -> Option<String> {
    let text = std::fs::read_to_string(base_ast.join(".last-error")).ok()?;
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?.trim();
    let mut s: String = line.chars().take(200).collect();
    if s.len() < line.len() {
        s.push_str("...");
    }
    Some(s)
}

/// The tail of a failed extractor's stderr, what `.last-error` keeps.
pub fn stderr_tail(bytes: &[u8]) -> String {
    const KEEP: usize = 4000;
    let start = bytes.len().saturating_sub(KEEP);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// True if a refresh ran within the debounce window — skip this one.
fn recently_synced(marker: &Path) -> bool {
    std::fs::metadata(marker)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|e| e < Duration::from_secs(DEBOUNCE_SECS))
        .unwrap_or(false)
}

/// Spawn a detached, backgrounded per-app AST sync. Never waited on.
/// `register` is true for a FIRST map, so it lands in `base ast list`; a
/// background refresh skips the workspace-graph registration write so
/// frequent turns don't churn graph.nq. `--yes` because nobody is there to
/// answer the extractor's file-count prompt.
fn spawn_sync(app_root: &Path, register: bool) {
    if std::env::var_os(NO_SPAWN_ENV).is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.arg("sync").arg("--ast").arg("--yes").arg("--target").arg(app_root);
    if !register {
        cmd.env("BASE_AST_SKIP_REGISTER", "1");
    }
    let _ = cmd
        .current_dir(app_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
