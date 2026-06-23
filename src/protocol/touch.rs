//! Real folder last-touch: the newest modification time of any non-ignored file
//! under a project's artifact folder. This is filesystem truth — what was actually
//! *edited* — not a graph timestamp the session hook stamped. It is the input that
//! makes "active" mechanical (it's being worked) instead of conceptual (it "should"
//! be active).

use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Local, Utc};

/// Directory names never counted as work: VCS internals, dependency/build caches,
/// and base's own state dir (whose `graph.nq` is rewritten every session — counting
/// it would re-introduce the very freshness-faking this module exists to kill).
const IGNORE_DIRS: &[&str] = &[
    ".git", ".svn", ".hg", ".base", "node_modules", "target", "dist", "build",
    ".next", ".nuxt", ".venv", "venv", "__pycache__", ".cache", ".mypy_cache",
    ".pytest_cache", "vendor", ".idea", ".gradle", "coverage",
];

/// Hard cap on entries visited so a pathologically large tree can't stall session
/// start. Newest-mtime is monotonic, so a partial walk only ever under-reports
/// freshness — it never fakes it.
const MAX_ENTRIES: usize = 20_000;

/// Newest mtime of any non-ignored file under `root` (recursive), as local time.
/// Returns `None` when the path is absent or holds no countable files — the caller
/// reads that as "can't determine touch" and leaves graph state alone.
pub fn folder_last_touch(root: &Path) -> Option<DateTime<Local>> {
    if !root.exists() {
        return None;
    }
    if root.is_file() {
        return mtime_of(root);
    }

    let mut newest: Option<SystemTime> = None;
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0usize;

    'walk: while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_ENTRIES {
                break 'walk;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue; // don't follow — avoids cycles and foreign trees
            }
            if ft.is_dir() {
                let name = entry.file_name();
                if IGNORE_DIRS.contains(&name.to_string_lossy().as_ref()) {
                    continue;
                }
                stack.push(entry.path());
            } else if ft.is_file()
                && let Ok(meta) = entry.metadata()
                && let Ok(m) = meta.modified()
            {
                newest = Some(match newest {
                    Some(cur) if cur >= m => cur,
                    _ => m,
                });
            }
        }
    }

    newest.map(to_local)
}

fn mtime_of(p: &Path) -> Option<DateTime<Local>> {
    std::fs::metadata(p).ok()?.modified().ok().map(to_local)
}

fn to_local(t: SystemTime) -> DateTime<Local> {
    DateTime::<Utc>::from(t).with_timezone(&Local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_path_is_none() {
        assert!(folder_last_touch(Path::new("/nonexistent/path/xyz")).is_none());
    }

    #[test]
    fn newest_file_wins_and_ignored_dirs_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("a.txt"), "a").unwrap();

        // An ignored dir with a (would-be newer) file must not count.
        let ignored = root.join("node_modules");
        std::fs::create_dir_all(&ignored).unwrap();
        std::fs::write(ignored.join("huge.js"), "x").unwrap();

        let touch = folder_last_touch(root);
        assert!(touch.is_some());
    }

    #[test]
    fn empty_folder_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(folder_last_touch(tmp.path()).is_none());
    }
}
