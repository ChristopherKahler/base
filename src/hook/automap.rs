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
//!     app that has no map, whatever the session's cwd;
//!   * Stop — the cwd's app and every app edited this turn, refreshed;
//!   * `base scaffold` and `base project add --path` — the folder they just
//!     registered.
//!
//! What a hook never maps: the home directory, a filesystem root, the
//! well-known user folders (Desktop, Documents, Downloads, a cloud-drive root),
//! and a folder that only HOLDS apps — a `.base` workspace whose children are
//! the repos. Mapping one of those walks every project on the machine into a
//! single file, which is the one failure worse than a missing map.
//!
//! Every build is detached and debounced; a hook never waits. An unattended
//! build never stops to ask about the file-count threshold (`base sync --ast
//! --yes`), and a failed one leaves `.base-ast/.last-error` behind so the next
//! session start can say why instead of promising a map that never lands.

use std::path::{Path, PathBuf};
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
    /// A sync ran within the debounce window; the caller may requeue.
    Debounced,
    /// No `.base-ast/ast.ttl` yet: build the first map, registered.
    Build,
    /// A map exists: refresh it in the background, unregistered.
    Refresh,
}

/// The build/refresh decision, with every input explicit so it can be tested
/// without a filesystem: `home` is the operator's home, `mapped` whether
/// `ast.ttl` exists, `debounced` whether `.last-sync` is fresher than the window.
pub fn plan_map(root: &Path, home: Option<&Path>, mapped: bool, debounced: bool) -> MapPlan {
    if home.is_some_and(|h| h == root) {
        return MapPlan::SkipHome;
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
    if is_workspace_hub(root) {
        return MapPlan::SkipHub;
    }
    let base_ast = root.join(".base-ast");
    let mapped = base_ast.join("ast.ttl").is_file();
    let marker = base_ast.join(".last-sync");
    let home = crate::home::real_home();
    let plan = plan_map(root, home.as_deref(), mapped, recently_synced(&marker));
    match plan {
        MapPlan::Build | MapPlan::Refresh => {
            let _ = std::fs::create_dir_all(&base_ast);
            let _ = std::fs::write(&marker, b"");
            spawn_sync(root, plan == MapPlan::Build);
        }
        MapPlan::SkipHome | MapPlan::SkipHub | MapPlan::Debounced => {}
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

/// A direct child of `dir` is itself a marked folder.
pub fn has_child_apps(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .any(|e| is_marked(&e.path()))
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
                if depth + 1 < PROBE_DEPTH && !name.starts_with('.') && !NOISE_DIRS.contains(&name.as_ref()) {
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
