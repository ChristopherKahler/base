//! #66 rows A1 and B1, and #83/#84's own grammar-free rows: the gates that need
//! no tree-sitter grammar, so they ride `cargo test` rather than waiting for a
//! CI job that installs 28 wheels (#85, deferred).
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

/// #83: every extension the extractor parses is an extension the map can name a
/// language for. `LANG_MAP` was a third hand-kept list beside `_DISPATCH` and
/// `_FILE_EXTS`; #66 derived the second and left this one, then widened its gap
/// by adding `.cjs` to the parser table alone. Derived now, so it cannot drift.
#[test]
fn every_parsed_extension_has_a_language() {
    let out = py(r#"
import sys; sys.path.insert(0, ".")
import extractor, onto_ast as o
gap = set(extractor._DISPATCH) - set(o.LANG_MAP)
extra = set(o.LANG_MAP) - set(extractor._DISPATCH) - set(o._EXT_LANG)
print("gap", len(gap), sorted(gap))
print("extra", len(extra), sorted(extra))
print("unknown", sorted(e for e, l in o.LANG_MAP.items() if l == "unknown"))
print("count", len(o.LANG_MAP))
"#);
    assert!(
        out.contains("gap 0 []"),
        "extensions are parsed but have no language — they reach the graph as \
         \"unknown\" (#83):\n{out}"
    );
    assert!(
        out.contains("extra 0 []"),
        "the language map claims a language for something nothing parses, and \
         nothing declares it an exception (#83):\n{out}"
    );
    assert!(out.contains("unknown []"), "an extension is mapped to the literal \"unknown\":\n{out}");
    let n: usize = out
        .lines()
        .find_map(|l| l.strip_prefix("count ")?.trim().parse().ok())
        .expect("no count line");
    assert!(n >= 60, "the language map collapsed to {n} entries:\n{out}");
}

/// #83: a new extractor with no language is a loud failure at import, not a
/// silent "unknown" in every map built afterwards — the same choice
/// `_parsed_extensions` makes, and the reason this class of drift ends here.
#[test]
fn an_extractor_with_no_language_fails_loudly() {
    let out = py(r#"
import sys; sys.path.insert(0, ".")
import extractor, onto_ast as o
def extract_nothing(path): return {}
extractor._DISPATCH[".plover"] = extract_nothing
try:
    o._build_lang_map()
    print("RESULT no-error")
except RuntimeError as e:
    print("RESULT raised", ".plover" in str(e) and "extract_nothing" in str(e))
"#);
    assert!(
        out.contains("RESULT raised True"),
        "an unmapped extractor must raise, and the message must name the \
         extension and the function (#83):\n{out}"
    );
}

/// #84: the markup around a single-file component's `<script>` block is blanked
/// before parsing, and the blanking preserves every byte offset and line break.
///
/// This is the deterministic core of the fix and it needs no grammar. What the
/// JS parser then MAKES of those bytes is measured in
/// `verification/base-0.14.2/ast_followups_0142.sh`, which has the 28 wheels.
#[test]
fn sfc_script_isolation_preserves_offsets() {
    let out = py(r#"
import sys; sys.path.insert(0, ".")
from extractor import _sfc_script_only
src = b"<template>\n  <div>{{ x }}</div>\n</template>\n<script>\nfunction f() {}\n</script>\n<style>a{}</style>\n"
got = _sfc_script_only(src)
print("same_len", len(got) == len(src))
print("same_lines", got.count(b"\n") == src.count(b"\n"))
print("newlines_aligned", [i for i,b in enumerate(src) if b == 10] == [i for i,b in enumerate(got) if b == 10])
print("kept_script", b"function f() {}" in got)
print("dropped_template", b"<div>" not in got and b"<style>" not in got)
print("offset_kept", got.index(b"function f() {}") == src.index(b"function f() {}"))
print("no_block", _sfc_script_only(b"export function plain() {}\n") is None)
"#);
    for row in [
        "same_len True",
        "same_lines True",
        "newlines_aligned True",
        "kept_script True",
        "dropped_template True",
        "offset_kept True",
        "no_block True",
    ] {
        assert!(
            out.contains(row),
            "#84: `{row}` did not hold — an SFC's line numbers or script body \
             would be wrong:\n{out}"
        );
    }
}


// ── #107: the relation vocabulary ────────────────────────────────────────────
//
// Same class as A1 one layer down. `_FILE_EXTS` drifted 39 extensions behind
// `_DISPATCH` and every entity in the gap landed on the app root; `RELATION_MAP`
// drifted 19 relations behind what `extractor.py` emits and every edge in THAT
// gap was discarded at `ttl_serializer.py`'s last step, silently. `ops:inherits`
// occurs in no map on any tree.
//
// These rows are grammar-free on purpose, so they ride `cargo test` today rather
// than waiting for the Python CI job (#85). The corpus row that needs real
// parses lives in `scripts/ast/test_relation_corpus.py`.

/// C1: every relation the extractor can emit has an `ops:` predicate.
///
/// The emittable set is read from `extractor.py`'s own AST, not grepped. Four of
/// the names reach an edge without ever appearing as a string literal at an
/// `add_edge` call site — `implements` arrives through a forwarder, `uses_config`
/// is built by f-string interpolation over a closed frozenset, and two more are
/// conditional expressions — so a literal scan reports 24, 25 or 26 depending on
/// which shapes it happens to know. It is 27.
#[test]
fn every_emittable_relation_has_a_predicate() {
    let out = py(r#"
import sys; sys.path.insert(0, ".")
import ttl_serializer as t
from relation_vocabulary import extractor_relations
v = extractor_relations("extractor.py")
missing = sorted(v.relations - set(t.RELATION_MAP))
dead    = sorted(set(t.RELATION_MAP) - v.relations)
print("emittable", len(v.relations))
print("mapped", len(t.RELATION_MAP))
print("missing", len(missing), missing)
print("dead", len(dead), dead)
print(v.report())
"#);
    // A8: the census publishes its own enumeration rule, and the FLOOR line is
    // asserted so a later edit cannot quietly drop it and leave a bare `27`
    // reading as a closed set. A2's standing ruling is that the set is OPEN.
    assert!(
        out.contains("FLOOR not CLOSURE"),
        "the relation census printed a completeness figure without the FLOOR \
         line naming the enumeration it counted over (amendment A8):\n{out}"
    );

    // Law 23: a resolver that silently skipped sites proves less than it claims,
    // so an unresolved site is a failure of THIS test, not a smaller vocabulary.
    assert!(
        out.contains("unresolved 0 []"),
        "the relation census could not reduce every relation slot to constants. \
         Teach `relation_vocabulary.py` the new shape — do not narrow the claim:\n{out}"
    );
    assert!(
        out.contains("missing 0 []"),
        "the extractor emits relations the serializer has no predicate for; every \
         edge carrying one is discarded silently (#107):\n{out}"
    );
    assert!(
        out.contains("dead 0 []"),
        "the serializer maps a relation nothing emits — a dead key invites the \
         next reader to delete a live one by symmetry:\n{out}"
    );
    // A shape that stops matching would quietly shrink the vocabulary and pass
    // every assertion above. Pin the ones with exactly one instance in the tree.
    // These two pin a shape with exactly one instance, and they are matched
    // LINE-EXACT rather than by `contains` for two separate reasons.
    //
    // Format: `relation_vocabulary.report()` emits `"path %s %d"` lines. The
    // previous spelling here asserted a Python tuple repr, `('forwarders', 1)`,
    // which was written against the pre-A8 report and became unmatchable the
    // moment that report was rewritten — so it asserted nothing that could ever
    // hold, while the counters it was guarding were all green.
    //
    // Exactness: `out.contains("path forwarders 1")` is also satisfied by
    // `path forwarders 10`, so a substring guard would keep passing while the
    // count it exists to pin drifted upward.
    assert!(
        out.lines().any(|l| l.trim() == "path forwarders 1"),
        "the forwarder shape found nothing — `implements` is spelled at no other \
         site and would vanish from the vocabulary:\n{out}"
    );
    assert!(
        out.lines().any(|l| l.trim() == "path via_closed_set 1"),
        "the f-string-over-a-closed-set shape found nothing — `uses_config` is \
         spelled at no other site and would vanish from the vocabulary:\n{out}"
    );
    let emittable: usize = out
        .lines()
        .find_map(|l| l.strip_prefix("emittable "))
        .and_then(|v| v.parse().ok())
        .expect("emittable line");
    assert!(
        emittable >= 27,
        "the census found {emittable} relations where 27 were measured on 610636e; \
         a shrinking vocabulary means the resolver stopped seeing a shape:\n{out}"
    );
}

/// C2: every relation in the vocabulary actually reaches the TTL as a triple.
///
/// C1 proves the table is complete; this proves the table is USED. A predicate
/// present in the map and never emitted would satisfy C1 and still lose edges.
#[test]
fn every_mapped_relation_reaches_the_ttl() {
    let out = py(r#"
import re, sys; sys.path.insert(0, ".")
import ttl_serializer as t
missing = []
for rel, pred in sorted(t.RELATION_MAP.items()):
    extraction = {
        "nodes": [
            {"id": "src_n", "label": "src", "source_file": "a.py", "source_location": "L1"},
            {"id": "tgt_n", "label": "tgt", "source_file": "a.py", "source_location": "L2"},
        ],
        "edges": [{"source": "src_n", "target": "tgt_n", "relation": rel}],
    }
    ttl = t.serialize(extraction, "proj", "a.py", "python")
    if not re.search(rf"^code:\S+ ops:{pred} code:\S+ \.$", ttl, re.M):
        missing.append(rel)
print("checked", len(t.RELATION_MAP))
print("missing", len(missing), missing)
"#);
    let checked: usize = out
        .lines()
        .find_map(|l| l.strip_prefix("checked "))
        .and_then(|v| v.parse().ok())
        .expect("checked line");
    // Law 23: a loop that visited nothing must never read as a pass.
    assert!(checked >= 27, "the coverage loop visited only {checked} relations:\n{out}");
    assert!(
        out.contains("missing 0 []"),
        "relations are in the map but emit no triple:\n{out}"
    );
}

/// C3: an unknown relation fails loudly instead of vanishing.
///
/// Law 25 — this is the leg that separates "repaired" from "silenced". C1 and C2
/// would both pass if the fix were "add 19 keys" and the `continue` stayed; the
/// next relation anyone adds would then be dropped exactly as `inherits` was.
#[test]
fn an_unknown_relation_fails_loudly() {
    let out = py(r#"
import sys; sys.path.insert(0, ".")
import ttl_serializer as t
extraction = {
    "nodes": [
        {"id": "src_n", "label": "src", "source_file": "a.py", "source_location": "L1"},
        {"id": "tgt_n", "label": "tgt", "source_file": "a.py", "source_location": "L2"},
    ],
    "edges": [{"source": "src_n", "target": "tgt_n", "relation": "wibble_wobble"}],
}
try:
    t.serialize(extraction, "proj", "a.py", "python")
    print("raised False")
    print("message -")
except Exception as e:
    print("raised True")
    print("message", type(e).__name__, str(e)[:200])
"#);
    assert!(
        out.contains("raised True"),
        "an unmapped relation was dropped silently — the #107 defect itself:\n{out}"
    );
    assert!(
        out.contains("wibble_wobble"),
        "the error must name the offending relation, or the author cannot act on it:\n{out}"
    );
    assert!(
        out.contains("relations.py"),
        "the error must name the file to edit — `_build_lang_map`'s message does:\n{out}"
    );
}
