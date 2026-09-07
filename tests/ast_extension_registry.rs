//! #66 rows A1 and B1: the two gates that need no tree-sitter grammar, so they
//! ride `cargo test` rather than waiting for a CI job that installs 28 wheels.
//!
//! A1 is the drift assertion that would have caught this months ago: `_FILE_EXTS`
//! (which node labels are file nodes) had drifted 39 extensions behind `_DISPATCH`
//! (which extensions are parsed), and every entity in the gap was attributed to
//! the app root instead of its own file.
//!
//! B1 is the Windows `.baseignore` half: a directory pattern is written with
//! forward slashes, the relative path is backslash-separated on Windows, and
//! `archive/` never matched `archive\x\y.ts`.
//!
//! A missing `python3` FAILS these tests. It never skips: a skip here is
//! indistinguishable from a pass, and ubuntu-latest has python3.

use std::path::PathBuf;
use std::process::Command;

fn ast_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts").join("ast")
}

/// Run a python snippet with `scripts/ast` on the path. Returns (stdout, stderr).
/// A non-zero exit is a test failure with the interpreter's own message, never a skip.
fn py(snippet: &str) -> String {
    let dir = ast_dir();
    assert!(
        dir.join("ttl_serializer.py").is_file(),
        "scripts/ast/ttl_serializer.py missing under {}",
        dir.display()
    );
    let bin = if cfg!(windows) { "python" } else { "python3" };
    let out = Command::new(bin)
        .arg("-c")
        .arg(snippet)
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "could not run `{bin}`: {e}. These rows gate the AST extractor and \
                 must not be skipped — install python3."
            )
        });
    assert!(
        out.status.success(),
        "python exited {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A1: every extension the extractor parses is an extension the serializer
/// treats as a file. Derived, so the two cannot drift apart again.
#[test]
fn every_parsed_extension_is_a_file_node() {
    let out = py(r#"
import sys; sys.path.insert(0, ".")
import extractor, ttl_serializer as t
gap = set(extractor._DISPATCH) - set(t._FILE_EXTS)
extra = set(t._FILE_EXTS) - set(extractor._DISPATCH)
print("gap", len(gap), sorted(gap))
print("extra", len(extra), sorted(extra))
print("derived", t._FILE_EXTS == frozenset(extractor._DISPATCH))
print("count", len(t._FILE_EXTS))
"#);
    assert!(
        out.contains("gap 0 []"),
        "extensions are parsed but not treated as files — their entities all land \
         on the app root (#66):\n{out}"
    );
    assert!(out.contains("extra 0 []"), "the serializer claims a file type nothing parses:\n{out}");
    assert!(
        out.contains("derived True"),
        "_FILE_EXTS must BE the dispatch keys, not merely agree with them today:\n{out}"
    );
    // A hand-written 27 is what drifted. Guard the shape, not the number.
    let n: usize = out
        .lines()
        .find_map(|l| l.strip_prefix("count "))
        .and_then(|v| v.parse().ok())
        .expect("count line");
    assert!(n > 27, "the derived set collapsed back toward the old hand list: {n}");
}

/// Lock 4: `.cjs` is parsed at all. It was in none of the three lists.
#[test]
fn cjs_is_parsed_and_attributed() {
    let out = py(r#"
import sys; sys.path.insert(0, ".")
import extractor, ttl_serializer as t
print("dispatch", ".cjs" in extractor._DISPATCH)
print("file", ".cjs" in t._FILE_EXTS)
"#);
    assert!(out.contains("dispatch True"), "{out}");
    assert!(out.contains("file True"), "{out}");
}

/// B1: `.baseignore` directory patterns on a Windows-shaped relative path.
/// `PureWindowsPath` gives backslash separators on any host, so this row runs
/// on the Linux CI box and still measures the Windows behaviour.
#[test]
fn baseignore_directory_patterns_match_on_windows_paths() {
    let out = py(r#"
import sys; sys.path.insert(0, ".")
from pathlib import PureWindowsPath
import detect
root = PureWindowsPath(r"C:\w\app")
ts   = PureWindowsPath(r"C:\w\app\archive\x\y.ts")
md   = PureWindowsPath(r"C:\w\app\docs\a.md")
deep = PureWindowsPath(r"C:\w\app\src\vendor\a.ts")
print("slash_dir",  detect._is_ignored(ts,   root, ["archive/"]))
print("nested_dir", detect._is_ignored(deep, root, ["src/vendor/"]))
print("bare_dir",   detect._is_ignored(ts,   root, ["archive"]))
print("glob",       detect._is_ignored(md,   root, ["*.md"]))
print("exact",      detect._is_ignored(ts,   root, ["archive/x/y.ts"]))
print("unrelated",  detect._is_ignored(ts,   root, ["build/"]))
print("empty",      detect._is_ignored(ts,   root, []))
"#);
    // The two that were broken: a directory pattern carrying a slash.
    assert!(out.contains("slash_dir True"), "`archive/` must ignore archive\\x\\y.ts (#66):\n{out}");
    assert!(out.contains("nested_dir True"), "`src/vendor/` must ignore src\\vendor\\a.ts:\n{out}");
    // The four that already worked and must keep working.
    assert!(out.contains("bare_dir True"), "a bare directory name regressed:\n{out}");
    assert!(out.contains("glob True"), "an extension glob regressed:\n{out}");
    assert!(out.contains("exact True"), "a whole relative path regressed:\n{out}");
    assert!(out.contains("unrelated False"), "an unrelated pattern now ignores everything:\n{out}");
    assert!(out.contains("empty False"), "no patterns must ignore nothing:\n{out}");
}

/// The forward-slash normalisation must not change POSIX behaviour, where
/// `str(rel)` and `rel.as_posix()` already agreed.
#[test]
fn baseignore_posix_behaviour_is_unchanged() {
    let out = py(r#"
import sys; sys.path.insert(0, ".")
from pathlib import PurePosixPath
import detect
root = PurePosixPath("/w/app")
ts   = PurePosixPath("/w/app/archive/x/y.ts")
print("slash_dir", detect._is_ignored(ts, root, ["archive/"]))
print("bare_dir",  detect._is_ignored(ts, root, ["archive"]))
print("unrelated", detect._is_ignored(ts, root, ["build/"]))
"#);
    assert!(out.contains("slash_dir True"), "{out}");
    assert!(out.contains("bare_dir True"), "{out}");
    assert!(out.contains("unrelated False"), "{out}");
}
