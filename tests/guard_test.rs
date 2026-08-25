//! The guard that keeps this fork's fix from rotting.
//!
//! `dirs::home_dir()` at a call site is what let `cargo test` write to the
//! operator's real global graph. A clippy `disallowed-methods` entry bans it
//! too, but clippy is not part of `cargo test` — this is, so it fails in the
//! same run that would otherwise reintroduce the bug.

use std::path::Path;

/// The only file allowed to name the raw OS home lookup.
const SANCTIONED: &str = "home.rs";

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_raw_home_dir_outside_the_seam() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("tests"), &mut files);

    let mut offenders = Vec::new();
    for f in files {
        if f.file_name().is_some_and(|n| n == SANCTIONED) {
            continue;
        }
        // This file necessarily spells the banned call out in its own prose.
        if f.file_name().is_some_and(|n| n == "guard_test.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if line.contains("dirs::home_dir") {
                offenders.push(format!("{}:{}", f.display(), i + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "raw dirs::home_dir() is banned outside src/{SANCTIONED} — resolve through \
         base::home::home_root() so the BASE_HOME override reaches it.\nOffenders:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_isolation_feature_is_actually_on_for_test_builds() {
    // If this fails, the self-dev-dependency in Cargo.toml stopped enabling the
    // feature, `home_root()` resolves the real home in every integration test,
    // and the tripwire is unarmed. That is the original bug, silently restored.
    assert!(
        base::home::isolation_active(),
        "the isolation-guard feature must be enabled for `cargo test`"
    );
}

#[test]
fn a_test_build_never_resolves_the_real_global_tier() {
    let root = base::home::home_root().expect("a test build always resolves a home");
    let real = base::home::real_home().expect("the OS always has a home here");
    assert_ne!(
        root.join(".base-gbl"),
        real.join(".base-gbl"),
        "integration tests must not resolve the operator's real global tier"
    );
}

/// Files allowed to name a SPARQL change variant.
///
/// `store.rs` is the seam that constructs them. `changelog.rs` owns the enum —
/// it declares the variants and matches on them to render a record, and it
/// contains no `write_back` call, so nothing built there can reach a graph file.
const SPARQL_SEAM: [&str; 2] = ["store.rs", "changelog.rs"];

/// A SPARQL write must go through `store::update_and_write` /
/// `store::mutate_and_write`, never construct its own change record.
///
/// The seam is where the delta is captured, and the capture has to happen
/// *before* the mutation — so a writer that reaches past it and hands
/// `write_back` a hand-built record cannot have a delta, and ships nothing. That
/// failure is silent: the write lands, the log line looks ordinary, and the
/// team's graph quietly never receives it. Deleting `Change::Sparql` made the
/// 26 existing sites a compile error; this keeps the 27th one from reappearing.
#[test]
fn no_hand_built_sparql_change_outside_the_seam() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);

    let mut offenders = Vec::new();
    for f in files {
        if f.file_name().is_some_and(|n| SPARQL_SEAM.iter().any(|s| n == *s)) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            // The doc-comment references in changelog.rs name the variants to
            // explain them; only a real construction takes an argument.
            if line.trim_start().starts_with("///") || line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains("Change::SparqlWithDelta(") || line.contains("Change::SparqlNoDelta(") {
                offenders.push(format!("{}:{}", f.display(), i + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a SPARQL change record may only be built in {SPARQL_SEAM:?} — call \
         store::update_and_write or store::mutate_and_write so the delta is \
         captured before the mutation.\nOffenders:\n  {}",
        offenders.join("\n  ")
    );
}
