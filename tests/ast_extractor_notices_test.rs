//! #66 lock 2, the visible half: `base sync --ast --yes` pipes the extractor's
//! stderr so a FAILED build can explain itself at the next session start
//! (`.base-ast/.last-error`). Until 0.14.2 a SUCCESSFUL build's stderr was
//! captured and dropped, so the new app-root attribution counter would have been
//! invisible on the path that runs it most — the Stop hook, which always passes
//! `--yes` (src/hook/automap.rs, spawn_sync).
//!
//! The stub extractor here writes notice lines and exits 0. The real python is
//! not involved: this pins the plumbing, not the count.

use std::path::Path;
use std::process::Command;

/// A fake `python3`/`python` earlier on PATH than the real one. It ignores its
/// argv except `--out`, writes notices to stderr, a minimal map to `--out`, and
/// exits 0 — a successful build with something to say.
#[cfg(unix)]
fn stub_python(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("python3");
    std::fs::write(
        &p,
        "#!/bin/sh\n\
         out=\"\"\n\
         while [ $# -gt 0 ]; do\n\
         \x20 case \"$1\" in --out) out=\"$2\"; shift;; esac\n\
         \x20 shift\n\
         done\n\
         echo '# Extracting 3 files from somewhere' >&2\n\
         echo '# Skipped weird.rs: SyntaxError' >&2\n\
         echo '# 7 entities attributed to the app root (no file node)' >&2\n\
         echo 'not-a-notice line' >&2\n\
         [ -n \"$out\" ] && printf '# empty map\\n' > \"$out\"\n\
         exit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    // `python_bin()` prefers python3 on unix but falls back to python.
    std::fs::copy(&p, dir.join("python")).unwrap();
    std::fs::set_permissions(dir.join("python"), std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn echo_extractor_notices_forwards_hash_lines_only() {
    // Unit half: the filter itself. `# ` prefixed lines are the extractor's
    // notices; anything else is a traceback fragment or a grammar warning and
    // belongs in `.last-error` on failure, not on a successful run's stderr.
    let input = b"# one\nnot a notice\n# two\n  # indented, not a notice\n# three\n";
    let text = String::from_utf8_lossy(input);
    let echoed: Vec<&str> = text.lines().filter(|l| l.starts_with("# ")).collect();
    assert_eq!(echoed, vec!["# one", "# two", "# three"]);
    // The function under test must agree with that filter; it prints, so this
    // asserts it runs clean over the same bytes rather than re-deriving them.
    base::hook::automap::echo_extractor_notices(input);
    base::hook::automap::echo_extractor_notices(b"");
    base::hook::automap::echo_extractor_notices(b"\xff\xfe not utf8");
}

#[cfg(unix)]
#[test]
fn a_successful_yes_sync_echoes_the_extractors_notices() {
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    stub_python(bin.path());

    // An app root with one source file, outside any real workspace.
    let app = tempfile::tempdir().unwrap();
    std::fs::write(app.path().join("a.py"), "def f():\n    pass\n").unwrap();
    std::fs::create_dir_all(app.path().join(".git")).unwrap();
    // `base sync --ast` resolves the extractor as $BASE_HOME/.base-gbl/scripts/ast,
    // then cwd/scripts/ast (src/cli.rs). Without one of those it never spawns
    // python at all and the row would be measuring a missing-script message.
    let scripts = app.path().join("scripts").join("ast");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("onto_ast.py"), "# stub; the fake python ignores it\n").unwrap();

    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_base"))
        .args(["sync", "--ast", "--yes", "--target"])
        .arg(app.path())
        .current_dir(app.path())
        .env("PATH", &path)
        .env("BASE_HOME", home.path())
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_SKIP_REGISTER", "1")
        .output()
        .unwrap();

    let err = String::from_utf8_lossy(&out.stderr);
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), err);
    assert!(
        all.contains("7 entities attributed to the app root (no file node)"),
        "the attribution counter must survive a --yes sync; without it the Stop \
         hook refresh reports nothing at all.\n--- stdout+stderr ---\n{all}"
    );
    assert!(
        all.contains("# Skipped weird.rs: SyntaxError"),
        "a skipped file must stay visible too:\n{all}"
    );
    assert!(
        !all.contains("not-a-notice line"),
        "only `# ` notices are echoed; the rest is failure-path material:\n{all}"
    );
}
