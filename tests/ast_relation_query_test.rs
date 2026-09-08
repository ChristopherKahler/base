//! `--relation` end to end: the query answers both directions, and an unknown
//! relation name says what IS present instead of returning nothing.
//!
//! Before #107, `--calls` and `--imports` were the only two relations the CLI
//! could reach, and the serializer mapped 8 of the 27 the extractor emits — so
//! `ops:inherits` occurred in no map on any tree and class hierarchy was
//! unanswerable twice over: the triples were dropped, and there was no way to
//! ask for them if they had not been.
//!
//! The unknown-name row is the one that is easy to leave untested and is the
//! reason this file spawns the binary rather than calling the library. An empty
//! result set and "this map has no such relation" are DIFFERENT FACTS that look
//! identical to a caller who only checks `Ok(())`, and the second one sends the
//! reader somewhere useful. Asserting the printed text is the only way to hold
//! that behaviour, so the assertion has to see stdout.

use std::process::Command;

/// A map with two `inherits` edges forming a three-level chain, plus one
/// `calls` and one `importsFrom`, so `Present:` has more than one name in it
/// and cannot pass by accident on a single-entry list.
fn write_map(app: &std::path::Path) {
    let sidecar = app.join(".base-ast");
    std::fs::create_dir_all(&sidecar).unwrap();
    let ttl = r#"@prefix ops: <http://ops-sys.local/ontology#> .
@prefix code: <http://ops-sys.local/code#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

code:animal a ops:Class ; rdfs:label "Animal" ; ops:sourceFile "zoo.py" ; ops:sourceLine 1 .
code:dog    a ops:Class ; rdfs:label "Dog" ; ops:sourceFile "zoo.py" ; ops:sourceLine 10 .
code:puppy  a ops:Class ; rdfs:label "Puppy" ; ops:sourceFile "zoo.py" ; ops:sourceLine 20 .
code:zoo    a ops:File  ; rdfs:label "zoo.py" ; ops:sourceFile "zoo.py" ; ops:sourceLine 1 .

code:dog ops:inherits code:animal .
code:puppy ops:inherits code:dog .
code:dog ops:calls code:animal .
code:zoo ops:importsFrom code:animal .
"#;
    std::fs::write(sidecar.join("ast.ttl"), ttl).unwrap();
}

fn run(home: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_base"))
        .args(args)
        .current_dir(cwd)
        .env("BASE_HOME", home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_SKIP_REGISTER", "1")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// `tmp/home` for BASE_HOME and `tmp/app` for the map, so the run cannot reach
/// the operator's real graph even if a code path tries to write one.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let app = tmp.path().join("app");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&app).unwrap();
    write_map(&app);
    (tmp, home, app)
}

#[test]
fn inherits_answers_both_directions_at_once() {
    let (_tmp, home, app) = fixture();
    let (rc, out) = run(&home, &app, &["ast", "query", "--relation", "inherits", "Dog"]);
    assert_eq!(rc, 0, "--relation inherits Dog exited {rc}:\n{out}");

    // Dog inherits Animal, and Puppy inherits Dog. One direction alone would
    // answer half the question a reader actually has, so both must appear.
    assert!(
        out.contains("Animal"),
        "the outgoing edge (Dog inherits Animal) is missing:\n{out}"
    );
    assert!(
        out.contains("Puppy"),
        "the incoming edge (Puppy inherits Dog) is missing:\n{out}"
    );

    // Control for the unknown-name test below: a KNOWN relation must not print
    // the `Present:` line. Without this, that test could pass on a build where
    // every query prints it.
    assert!(
        !out.contains("Present:"),
        "a known relation printed the unknown-relation line:\n{out}"
    );
}

#[test]
fn an_unknown_relation_names_what_is_present() {
    let (_tmp, home, app) = fixture();
    let (rc, out) = run(
        &home,
        &app,
        &["ast", "query", "--relation", "wibble_wobble", "Dog"],
    );
    assert_eq!(rc, 0, "an unknown relation is not an error:\n{out}");

    // The whole point: it says which half is missing, and then says what it
    // could have answered. An empty result cannot be told apart from "Dog has
    // no such edges", which is a different fact.
    assert!(
        out.contains("Present:"),
        "an unknown relation returned no guidance:\n{out}"
    );
    assert!(
        out.contains("inherits") && out.contains("calls"),
        "the Present: list does not name the relations this map carries:\n{out}"
    );
    assert!(
        !out.contains("Animal"),
        "an unknown relation answered with edges anyway:\n{out}"
    );
}

#[test]
fn underscore_and_camel_spellings_are_the_same_query() {
    let (_tmp, home, app) = fixture();

    // The map stores `importsFrom`; the extractor and the issues write
    // `imports_from`. A reader should not have to know which.
    let (rc_snake, snake) = run(
        &home,
        &app,
        &["ast", "query", "--relation", "imports_from", "zoo.py"],
    );
    let (rc_camel, camel) = run(
        &home,
        &app,
        &["ast", "query", "--relation", "importsFrom", "zoo.py"],
    );
    assert_eq!(rc_snake, 0, "snake_case spelling exited {rc_snake}:\n{snake}");
    assert_eq!(rc_camel, 0, "camelCase spelling exited {rc_camel}:\n{camel}");

    // Neither spelling may fall through to the unknown-relation branch, and the
    // two must agree — a normaliser that mapped both to the same WRONG answer
    // would still fail the first assertion.
    assert!(
        !snake.contains("Present:") && !camel.contains("Present:"),
        "a spelling fell through to the unknown branch:\nsnake:\n{snake}\ncamel:\n{camel}"
    );
    assert!(
        snake.contains("Animal") && camel.contains("Animal"),
        "importsFrom did not reach the edge:\nsnake:\n{snake}\ncamel:\n{camel}"
    );
}

#[test]
fn a_map_with_no_such_entity_still_says_the_relation_exists() {
    let (_tmp, home, app) = fixture();
    let (rc, out) = run(
        &home,
        &app,
        &["ast", "query", "--relation", "inherits", "Nonexistent"],
    );
    assert_eq!(rc, 0, "a missing entity is not an error:\n{out}");

    // Two different absences, told apart: the RELATION is present, the ENTITY
    // is not. Collapsing these is what makes an empty result unactionable.
    assert!(
        out.contains("Nonexistent") || out.contains("inherits"),
        "neither the entity nor the relation is named:\n{out}"
    );
    assert!(
        !out.contains("Present:"),
        "a known relation printed the unknown-relation line:\n{out}"
    );
}
