//! Stored `ops:sourceFile` literals carry the extractor's OS-native separators
//! (`ui\deck.js` on Windows); every probe is normalised to `/`. The query side
//! has to reduce the stored side too, or nothing below the app root matches on
//! Windows — measured 2026-09-01: the PreToolUse hook injected a map for
//! root-level files only, and `--file ui/deck.js` found nothing while
//! `--file deck.js` found 72 entities.

use base::config::NamespaceConfig;

fn map_with_backslashes(tmp: &std::path::Path) {
    let sidecar = tmp.join(".base-ast");
    std::fs::create_dir_all(&sidecar).unwrap();
    // Turtle escapes a backslash as `\\`, so these literals are `ui\deck.js`.
    let ttl = r#"@prefix ops: <http://ops-sys.local/ontology#> .
@prefix code: <http://ops-sys.local/code#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

code:a a ops:Function ; rdfs:label "boot()" ; ops:sourceFile "ui\\deck.js" ; ops:sourceLine 10 .
code:b a ops:Function ; rdfs:label "paintTabs()" ; ops:sourceFile "ui\\deck.js" ; ops:sourceLine 40 .
code:c a ops:Function ; rdfs:label "other()" ; ops:sourceFile "ui\\other\\deck.js" ; ops:sourceLine 5 .
code:d a ops:Function ; rdfs:label "rootfn()" ; ops:sourceFile "Program.cs" ; ops:sourceLine 1 .
"#;
    std::fs::write(sidecar.join("ast.ttl"), ttl).unwrap();
}

#[test]
fn nested_windows_path_is_found_by_the_hook_query() {
    let tmp = tempfile::tempdir().unwrap();
    map_with_backslashes(tmp.path());
    let ns = NamespaceConfig::default();

    let abs = tmp.path().join("ui").join("deck.js");
    let abs = abs.to_str().unwrap();

    // The hook's map. Two entities in ui\deck.js; the one in ui\other\deck.js
    // and the root-level file must not leak in.
    let map = base::crud::ast_query::file_map_compact(tmp.path(), &ns, abs)
        .expect("a file below the app root gets a map");
    assert!(map.contains("boot()") && map.contains("paintTabs()"), "got: {map}");
    assert!(!map.contains("rootfn()"), "a different file leaked in: {map}");

    // The partial-read section query, same path form: lines 1..20 hold boot()
    // (line 10) and not paintTabs() (line 40).
    let section = base::crud::ast_query::section_entities(tmp.path(), &ns, abs, 1, 20)
        .expect("section entities for a nested path");
    assert!(section.contains("boot()") && !section.contains("paintTabs()"), "got: {section}");

    // The CLI form with a directory in it runs. It prints; Ok is the contract.
    base::crud::ast_query::file(tmp.path(), &ns, "ui/deck.js").expect("--file ui/deck.js");
}
