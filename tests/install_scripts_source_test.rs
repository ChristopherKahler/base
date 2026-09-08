//! Issue #92: `base install` finds `scripts/ast` only via CWD when run from an
//! unpacked release archive.
//!
//! A release archive stages `base` and `scripts/ast/` as SIBLINGS
//! (`.github/workflows/release.yml:65-77` for the tar legs, `:83-90` for the
//! zip), so the unpacked layout is `<dir>/base` + `<dir>/scripts/ast/`. Before
//! this test existed, `install_scripts` built three candidates and none of them
//! was `<dir>/scripts/ast` — only `cwd/scripts/ast` could match, and only when
//! the process happened to be started inside the unpack directory. Run the
//! unpacked binary from anywhere else and AST extraction was silently absent
//! while the install still reported success.
//!
//! `cwd` is a parameter of the seam rather than read from the process, because
//! `std::env::set_current_dir` is process-global: a test that set it would race
//! every other test in this binary.

use base::install::ast_scripts_source;
use std::path::{Path, PathBuf};

/// Write a file, creating its parents. Returns the path, so a caller can assert
/// on the exact bytes the seam is supposed to have found.
fn touch(path: &Path) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"").unwrap();
    path.to_path_buf()
}

/// The marker `install_scripts` actually probes for. Named once so a change to
/// the probe cannot leave these tests passing against a file nothing reads.
const MARKER: &str = "onto_ast.py";

// ─── R1: the red ────────────────────────────────────────────

#[test]
fn an_unpacked_release_archive_is_found_from_any_working_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    // Exactly what `tar xzf base-linux-x86_64.tar.gz -C /tmp/x` leaves behind.
    let binary = touch(&archive.join("base"));
    let scripts = archive.join("scripts").join("ast");
    touch(&scripts.join(MARKER));

    assert_eq!(
        ast_scripts_source(&binary, &elsewhere).as_deref(),
        Some(scripts.as_path()),
        "the binary's own directory is the layout every release archive ships"
    );
}

// ─── C1-C3: the dev-build candidates, unchanged ─────────────
//
// These three must be green BEFORE and AFTER the archive candidate is added.
// That is what proves the new candidate shadows nothing — an assertion the fix
// itself cannot make.

#[test]
fn a_cargo_target_build_still_resolves_to_the_source_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let binary = touch(&repo.join("target").join("release").join("base"));
    let scripts = repo.join("scripts").join("ast");
    touch(&scripts.join(MARKER));

    assert_eq!(
        ast_scripts_source(&binary, &elsewhere).as_deref(),
        Some(scripts.as_path()),
        "target/<profile>/base resolves three levels up"
    );
}

#[test]
fn a_binary_one_level_under_the_repo_still_resolves_to_the_source_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let binary = touch(&repo.join("target").join("base"));
    let scripts = repo.join("scripts").join("ast");
    touch(&scripts.join(MARKER));

    assert_eq!(
        ast_scripts_source(&binary, &elsewhere).as_deref(),
        Some(scripts.as_path()),
        "two levels up is the other dev layout"
    );
}

#[test]
fn the_working_directory_is_still_the_last_resort() {
    let tmp = tempfile::tempdir().unwrap();
    // An installed binary: nothing named scripts/ast anywhere above it.
    let binary = touch(&tmp.path().join("home").join(".local").join("bin").join("base"));
    let cwd = tmp.path().join("checkout");
    let scripts = cwd.join("scripts").join("ast");
    touch(&scripts.join(MARKER));

    assert_eq!(
        ast_scripts_source(&binary, &cwd).as_deref(),
        Some(scripts.as_path()),
        "cwd still answers when no layout near the binary does"
    );
}

// ─── C4: the must-fail control ──────────────────────────────
//
// Without this every row above could pass on a seam that never probes for the
// marker at all and just returns the first path it can build. Law 24: a gate
// whose failing and non-running states are indistinguishable is not a gate.

#[test]
fn nothing_anywhere_is_none_and_not_a_path_that_does_not_exist() {
    let tmp = tempfile::tempdir().unwrap();
    let binary = touch(&tmp.path().join("lonely").join("base"));
    let cwd = tmp.path().join("empty");
    std::fs::create_dir_all(&cwd).unwrap();

    assert_eq!(
        ast_scripts_source(&binary, &cwd),
        None,
        "no candidate carries the marker, so there is no source dir"
    );
}

#[test]
fn a_directory_without_the_marker_is_not_a_source_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let binary = touch(&archive.join("base"));
    // scripts/ast exists but is empty — the shape a half-finished copy leaves.
    std::fs::create_dir_all(archive.join("scripts").join("ast")).unwrap();
    let cwd = tmp.path().join("empty");
    std::fs::create_dir_all(&cwd).unwrap();

    assert_eq!(
        ast_scripts_source(&binary, &cwd),
        None,
        "the marker is the probe, not the directory"
    );
}

// ─── C5: precedence ─────────────────────────────────────────

#[test]
fn the_archive_beside_the_binary_wins_over_the_working_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let binary = touch(&archive.join("base"));
    let beside = archive.join("scripts").join("ast");
    touch(&beside.join(MARKER));

    // A different, equally valid-looking scripts/ast in the working directory.
    let cwd = tmp.path().join("some-checkout");
    touch(&cwd.join("scripts").join("ast").join(MARKER));

    assert_eq!(
        ast_scripts_source(&binary, &cwd).as_deref(),
        Some(beside.as_path()),
        "the binary's own layout is trusted over wherever the shell happened to be"
    );
}
