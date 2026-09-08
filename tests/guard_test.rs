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

/// #91: the installed binary's name must come from one seam, never a literal.
///
/// Red on every platform, not only Windows: it greps the source rather than
/// asking what this build resolves to, so Linux CI catches a drifting site the
/// same way a Windows run would. That is the whole reason it is a grep and not
/// a behavioural test - `EXE_SUFFIX` is `""` on Linux, so a behavioural test
/// passes on CI while the Windows install stays broken.
#[test]
fn no_literal_base_binary_path_outside_the_seam() {
    const SANCTIONED: &str = "home.rs";
    let mut files = Vec::new();
    rust_files(Path::new("src"), &mut files);

    let mut offenders = Vec::new();
    for f in &files {
        if f.file_name().is_some_and(|n| n == SANCTIONED) {
            continue;
        }
        // Scoped exemptions: only where the literal IS the thing under test.
        // Named line by line, never a file-level or cfg(test)-level allow, so
        // any other fixture spelling the name by hand still fails — a fixture
        // carrying the old spelling is where the next drift starts.
        const EXEMPT: &[(&str, &str, &str)] = &[
            (
                "update/mod.rs",
                "let plain = tmp.path().join(\"base\");",
                "refresh_sibling's test seeds the extensionless sibling on purpose; \
                 the literal is the case being asserted",
            ),
            (
                "manifest.rs",
                "path = \"~/.local/bin/base\"",
                "a TOML fixture parsed as input - it must keep the spelling real \
                 manifests on disk already carry",
            ),
        ];

        let f_name = f.to_string_lossy().replace('\\', "/");
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue;
            }
            if EXEMPT
                .iter()
                .any(|(file, text, _why)| f_name.ends_with(file) && line.trim() == *text)
            {
                continue;
            }
            let literal_string = line.contains("\"~/.local/bin/base\"");
            let literal_join = line.contains(".join(\"base\")");
            // The name's OTHER spelling. A `cfg!(windows)` arm choosing between
            // the two literals is `base_binary_name()` rewritten by hand, and
            // the two checks above cannot see it: neither literal appears in the
            // form they look for. That blind spot is where the drift restarted
            // after this guard's first pass - five live sites in update/mod.rs,
            // found by reading, not by the guard.
            //
            // Matched on ONE line, which is the ternary form. A multi-line
            // `if cfg!(windows) {` block is left alone on purpose: those guard
            // genuine platform behaviour rather than construct a name, and
            // `install_dest_is_platform_correct` asserts the seam's output
            // against the literal, which is the case under test.
            let cfg_name_arm = line.contains("cfg!(windows)")
                && line.contains("\"base.exe\"")
                && line.contains("\"base\"");
            if literal_string || literal_join || cfg_name_arm {
                offenders.push(format!("{}:{}", f.display(), i + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "the installed binary must be named through crate::home::base_binary_name() \
         (or base_binary_path()/base_binary_display()), never a literal and never a \
         hand-written `if cfg!(windows) {{ \"base.exe\" }} else {{ \"base\" }}` arm - the \
         literal drops the Windows .exe suffix and produces a file Windows cannot run \
         by name, and the cfg arm is the same seam rewritten where this guard used to \
         be unable to see it (#91). Offending sites:\n  {}",
        offenders.join("\n  ")
    );
}
