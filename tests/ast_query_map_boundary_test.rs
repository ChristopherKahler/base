//! A read never crosses out of the app it started in.
//!
//! `tests/ast_map_resolution_test.rs` pins the home rule as a pure rule. These
//! legs pin the BOUNDARY on a real filesystem: an app with no map of its own
//! must not answer from the map of the app above it.
//!
//! ## Why every fixture is built entirely under `env::temp_dir()`
//!
//! `walk_up` carries a `cfg(feature = "isolation-guard")` early return on
//! `within_sandbox` (`config.rs:76-77`), and that feature is ON in every test
//! binary and OFF in the shipped one (`Cargo.toml:33,36`). So a fixture whose
//! parent sat OUTSIDE the sandbox would give `None` because the walk left the
//! sandbox — and that `None` would read as proof the app boundary works while
//! the shipped binary, which has no such line, kept walking and found the map.
//! A green test and a broken product would be indistinguishable.
//!
//! Putting the parent map inside the sandbox removes the confound by
//! construction: `within_sandbox` is TRUE for every directory these legs
//! traverse, so it cannot be what stops the walk. Each leg asserts that, and
//! prints the chain, rather than leaving it to be re-derived by a reader.

use std::path::{Path, PathBuf};

use base::config::find_ast_ttl;

/// Assert the sandbox guard cannot be the thing producing a `None`, and show
/// the reader the chain it was asserted over. A leg that visited nothing has
/// proved nothing (law 23), so a zero-length chain is a failure.
fn assert_chain_inside_sandbox(from: &Path, upto: &Path) {
    let mut visited = 0;
    let mut dir = from.to_path_buf();
    loop {
        visited += 1;
        let inside = base::home::within_sandbox(&dir);
        let has_map = dir.join(".base-ast").join("ast.ttl").is_file();
        println!("  [{visited}] {} map:{has_map} within_sandbox:{inside}", dir.display());
        assert!(
            inside,
            "{} is outside the sandbox, so a None from this fixture could be the \
             isolation-guard ceiling rather than the app boundary — the fixture is \
             the wrong shape, not the product",
            dir.display()
        );
        if dir == upto {
            break;
        }
        assert!(dir.pop(), "walked past the filesystem root without reaching {}", upto.display());
    }
    assert!(visited > 0, "visited 0 directories — this leg proved NOTHING");
    println!("  directories visited: {visited}");
}

fn write_map(dir: &Path) -> PathBuf {
    let base_ast = dir.join(".base-ast");
    std::fs::create_dir_all(&base_ast).unwrap();
    let map = base_ast.join("ast.ttl");
    // Enough Turtle to be a real map rather than an empty file, so "the map
    // exists" is a claim about content and not about a zero-byte placeholder.
    std::fs::write(
        &map,
        "@prefix ops: <https://ops.example/> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         <urn:e1> a ops:Function ; rdfs:label \"marker_entity\" .\n",
    )
    .unwrap();
    map
}

fn mark_app_root(dir: &Path) {
    // `.git` as a plain directory: `ast_app_root` tests for existence only.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
}

/// I1 — the boundary. An app with no map does not borrow the parent's.
///
/// The ancestry assertions are the point: without them a `None` here is
/// indistinguishable from a fixture whose parent map was never created. condor
/// measured `C:/Windows/Temp` — unmapped with no mapped ancestor — already
/// returning the named error on the UNFIXED binary, so fixture ancestry, not
/// the fix, is what decides red from green.
#[test]
fn an_unmapped_app_does_not_borrow_its_parents_map() {
    let tmp = tempfile::tempdir().unwrap();
    let suite = tmp.path();
    let app = suite.join("app");
    std::fs::create_dir_all(&app).unwrap();
    mark_app_root(&app);

    let parent_map = write_map(suite);

    // C-3: the ancestor exists, by path and with real content.
    assert!(parent_map.is_file(), "parent map absent — a None below would prove nothing");
    let bytes = std::fs::metadata(&parent_map).unwrap().len();
    assert!(bytes > 0, "parent map is empty — a None below would prove nothing");
    println!("parent map: {} ({bytes} bytes)", parent_map.display());

    // C-5: and it is reachable, so the sandbox ceiling is not the cause.
    assert_chain_inside_sandbox(&app, suite);

    assert_eq!(
        find_ast_ttl(&app),
        None,
        "an app with no map of its own must not answer from {}",
        parent_map.display()
    );
}

/// I2 — the same tree, one file different: the app's own map wins.
///
/// This is what separates "the boundary works" from "every query now fails".
#[test]
fn an_apps_own_map_answers_and_is_preferred_over_the_parents() {
    let tmp = tempfile::tempdir().unwrap();
    let suite = tmp.path();
    let app = suite.join("app");
    std::fs::create_dir_all(&app).unwrap();
    mark_app_root(&app);

    let parent_map = write_map(suite);
    let own_map = write_map(&app);
    assert!(parent_map.is_file() && own_map.is_file());

    assert_eq!(
        find_ast_ttl(&app).as_deref(),
        Some(own_map.as_path()),
        "the app's own map must answer, not the parent's"
    );
}

/// I3 — walking up WITHIN an app is kept. A query from a source subdirectory
/// still answers from the repo's map; the fix bounds the walk, it does not
/// remove it.
#[test]
fn a_subdirectory_still_walks_up_to_its_own_app_root() {
    let tmp = tempfile::tempdir().unwrap();
    let suite = tmp.path();
    let app = suite.join("app");
    let deep = app.join("src").join("crud");
    std::fs::create_dir_all(&deep).unwrap();
    mark_app_root(&app);

    write_map(suite);
    let own_map = write_map(&app);

    assert_eq!(
        find_ast_ttl(&deep).as_deref(),
        Some(own_map.as_path()),
        "a read from src/crud/ must reach the app's map at the repo root"
    );
}

/// I4 — the legacy `<root>/.base/ast.ttl` still resolves inside the app.
#[test]
fn the_legacy_workspace_map_still_resolves_within_the_app() {
    let tmp = tempfile::tempdir().unwrap();
    let suite = tmp.path();
    let app = suite.join("app");
    std::fs::create_dir_all(&app).unwrap();
    mark_app_root(&app);

    write_map(suite);
    let legacy_dir = app.join(".base");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    let legacy = legacy_dir.join("ast.ttl");
    std::fs::write(&legacy, "@prefix ops: <https://ops.example/> .\n").unwrap();

    assert_eq!(
        find_ast_ttl(&app).as_deref(),
        Some(legacy.as_path()),
        "the pre-sidecar map must keep working for anyone who has not re-synced"
    );
}

/// I5 — a directory that is not an app at all, with a map above it, still
/// resolves upward. The boundary is the APP root; it is not "never walk up".
#[test]
fn a_plain_directory_with_no_app_marker_still_resolves_upward() {
    let tmp = tempfile::tempdir().unwrap();
    let suite = tmp.path();
    mark_app_root(suite);
    let map = write_map(suite);

    let plain = suite.join("notes").join("drafts");
    std::fs::create_dir_all(&plain).unwrap();

    assert_eq!(
        find_ast_ttl(&plain).as_deref(),
        Some(map.as_path()),
        "an ordinary subdirectory of a mapped app answers from that app"
    );
}
