#!/usr/bin/env python3
"""An import has three states and they are never collapsed to one (#105).

`base ast query` could not tell these apart, because nothing in the pipeline
carried the difference:

  RESOLVED     the specifier names a file in this tree, and it is there.
  FAILED       the specifier names a path INSIDE this tree that is not there.
               A broken import. Reporting it as third-party cements a lookup
               failure as a deliberate taxonomy, which is #105's second half.
  THIRD PARTY  the specifier names a package, crate or stdlib module that was
               never supposed to be in this tree.

Two mechanisms produced the defect and both are pinned here.

  * Every `_import_*` handler is handed `edges` but NOT `nodes`, so it could
    not emit the target it named. 810 import edges on base's own tree pointed
    at nothing, and no node anywhere carried an external type -- there was
    nowhere for one to come from.
  * `_resolve_js_import_target` already returned whether the specifier
    resolved, and the caller discarded it.

The FAILED state is proven on a FIXTURE, deliberately. base's own tree has
zero broken imports, so a suite that only measured base could never tell a
working classifier from one that never reports a failure at all.

Run: python3 scripts/ast/test_import_states.py   (or: pytest scripts/ast/)
"""

import os
import re
import subprocess
import sys
import tempfile
from collections import namedtuple
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

#: `# imports: N resolved, M failed to resolve, K third-party`, printed by
#: `onto_ast.py` on STDERR. Each state is read on its own: a single total
#: cannot tell a fix from a suppression, because an extractor that stopped
#: emitting scores the same zero as one that resolved everything.
STATES = re.compile(
    r"^# imports: (\d+) resolved, (\d+) failed to resolve, (\d+) third-party",
    re.M,
)

Run = namedtuple("Run", "ttl notices")


def _extract(tree: dict[str, str]) -> Run:
    """Write `tree` to a fixture repo, extract it, return both raw streams.

    NOT under /tmp: file discovery drops any path with a `tmp` component, so a
    fixture there extracts to nothing and every assertion below would pass
    against an empty map.
    """
    with tempfile.TemporaryDirectory(prefix="base-ast-i105-", dir=Path.home()) as td:
        root = Path(td)
        for rel, body in tree.items():
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(body, encoding="utf-8")
        subprocess.run(["git", "init", "-q"], cwd=str(root), check=True)
        proc = subprocess.run(
            [sys.executable, str(HERE / "onto_ast.py"), str(root),
             "--project", "t", "--full"],
            capture_output=True, text=True, cwd=str(HERE),
            env=os.environ.copy(), timeout=300,
        )
        assert proc.returncode == 0, f"extraction failed:\n{proc.stderr}"
        run = Run(proc.stdout, proc.stderr)
    # Law 24. An empty map must never read as a clean one: "no broken imports"
    # and "nothing was parsed" are the same output with different meanings.
    assert "a ops:" in run.ttl, "extraction produced no entities; the fixture never parsed"
    assert "# Extracting" in run.notices, (
        "the extractor's notice stream is empty, so every state count below "
        f"would read a false zero. stderr was:\n{run.notices!r}"
    )
    return run


def _states(run: Run) -> dict[str, int]:
    """The three counts, read from the stream whose presence `_extract` proved."""
    m = STATES.search(run.notices)
    assert m, (
        "the run printed no import-state line at all, so no claim about the "
        f"three states can rest on it. stderr was:\n{run.notices!r}"
    )
    return {"resolved": int(m.group(1)), "failed": int(m.group(2)),
            "external": int(m.group(3))}


def _typed(run: Run, ops_type: str) -> list[str]:
    """Labels of every node the map declares as `ops:<ops_type>`."""
    out = []
    for block in run.ttl.split("\n\n"):
        if f" a ops:{ops_type} ;" not in block:
            continue
        m = re.search(r'^    rdfs:label "(.*)" ;$', block, re.M)
        if m:
            out.append(m.group(1))
    return out


def _import_edges(run: Run) -> list[tuple[str, str, str]]:
    return re.findall(
        r"^(code:\S+) ops:(importsFrom|imports|dynamicImport|reExports|includes) (code:\S+) \.$",
        run.ttl, re.M,
    )


def _declared(run: Run) -> set[str]:
    return set(re.findall(r"^(code:\S+) a ops:\w+ ;", run.ttl, re.M))


# ─── the three states ────────────────────────────────────────────────────────

PY_BROKEN = {
    "pkg/__init__.py": "",
    "pkg/main.py": "from .gone import missing\nimport json\nfrom pkg.helper import thing\n",
    "pkg/helper.py": "def thing():\n    return 1\n",
}


def test_a_relative_import_that_does_not_resolve_is_reported_as_failed():
    run = _extract(PY_BROKEN)
    assert _states(run)["failed"] >= 1, (
        "`from .gone import missing` names a path inside the tree that is not "
        "there. That is a broken import and the run must say so; filing it "
        "under third-party is the defect #105 was opened for"
    )
    labels = _typed(run, "UnresolvedImport")
    assert labels, "no node carries ops:UnresolvedImport, so the failure has no target"


def test_a_third_party_import_is_external_and_not_a_failure():
    run = _extract(PY_BROKEN)
    assert "json" in [l.rsplit(".", 1)[-1] for l in _typed(run, "ExternalModule")], (
        "`import json` is stdlib and must be typed as an external module"
    )


def test_a_first_party_bare_import_is_placed_by_the_tree():
    """`from pkg.helper import thing` is spelled exactly like a third party.

    Syntax cannot tell them apart, so the TREE decides. A classifier that
    called every bare name external would report this as third-party, which is
    the same error as calling a broken relative path external.
    """
    run = _extract(PY_BROKEN)
    assert "helper" not in _typed(run, "ExternalModule"), (
        "`helper` is a file in the fixture, so this import resolved; calling "
        "it third-party would be a false claim about the tree"
    )


def test_the_three_states_are_separately_countable():
    """auk's binding condition: never one total. Each state on its own."""
    run = _extract(PY_BROKEN)
    s = _states(run)
    assert s["failed"] >= 1 and s["external"] >= 1 and s["resolved"] >= 1, (
        f"all three states must be separately non-zero on this fixture, got {s}"
    )


def test_the_fix_cannot_pass_by_dropping_the_edge():
    """Law 25: the leg that the fix could have MASKED.

    Deleting the offending edge would satisfy "no import target is a callable"
    and "nothing dangles" identically. So the broken import must still be
    PRESENT as an edge, and still be counted as a failure.
    """
    run = _extract(PY_BROKEN)
    unresolved = [t for _, _, t in _import_edges(run)
                  if t in _declared(run)
                  and any(f"{t} a ops:UnresolvedImport ;" in b
                          for b in run.ttl.split("\n\n"))]
    assert unresolved, (
        "the broken import has no edge left. A fix that drops the edge passes "
        "every count in this file and loses the finding, which is worse than "
        "the defect"
    )
    assert _states(run)["failed"] >= 1


def test_no_import_edge_dangles_after_classification():
    run = _extract(PY_BROKEN)
    declared = _declared(run)
    dangling = [(s, r, t) for s, r, t in _import_edges(run) if t not in declared]
    assert not dangling, (
        f"{len(dangling)} import edge(s) still name a target that is emitted as "
        f"no node: {dangling[:3]}"
    )


# ─── rust: the crate is the identity, not the last path segment ──────────────

RUST_TREE = {
    "src/main.rs": (
        "use oxigraph::model::NamedNode;\n"
        "use serde::Serialize;\n"
        "use anyhow::Context;\n"
        "use crate::helper::helped;\n"
        "fn main() { let _ = helped(); }\n"
        "#[cfg(test)]\nmod tests {\n    use super::*;\n"
        "    #[test]\n    fn t() { let _ = helped(); }\n}\n"
    ),
    "src/helper.rs": "pub fn helped() -> u8 { 1 }\n",
}


def test_a_rust_use_names_the_crate_not_the_last_segment():
    """auk's condition 2: assert the NAMES, not just the total.

    `use oxigraph::model::NamedNode` used to record an edge to a node called
    `model` -- a path segment nobody wrote, which existed as no node and so
    dangled. 718 of base's own 894 dangling edges were this. A rewire that
    produced 718 plausible-but-wrong ids would pass a pure count check
    identically, so the crates are named here.
    """
    run = _extract(RUST_TREE)
    externals = set(_typed(run, "ExternalModule"))
    for crate in ("oxigraph", "serde", "anyhow"):
        assert crate in externals, (
            f"{crate!r} is a crate this fixture imports and must appear as an "
            f"external module BY NAME; got {sorted(externals)}"
        )
    assert "model" not in externals, (
        "`model` is a path SEGMENT inside oxigraph, not a thing anyone imported"
    )


def test_a_crate_relative_rust_import_resolves_to_the_file():
    run = _extract(RUST_TREE)
    assert "crate::helper::helped" not in _typed(run, "UnresolvedImport"), (
        "`use crate::helper::helped` names src/helper.rs, which exists, so it "
        "resolved. Item names are not modules: resolution walks the path from "
        "the longest prefix down"
    )
    assert "helper" not in set(_typed(run, "ExternalModule")), (
        "a `crate::` import is internal by construction and must never be "
        "filed as third-party"
    )


def test_a_super_glob_inside_an_inline_mod_is_not_a_failure():
    """`use super::*` in a `#[cfg(test)] mod tests` is a working import.

    Measured while building this: a first pass reported 73 failures on base's
    own tree and 72 of them were this one line, because stripping the glob
    leaves no segments to resolve. Reporting 72 working imports as broken
    would have been a fresh false claim inside the fix.
    """
    run = _extract(RUST_TREE)
    assert _states(run)["failed"] == 0, (
        f"this fixture has no broken imports, got {_states(run)}"
    )


# ─── resolved, but to a file this extractor does not parse ───────────────────

def test_a_resolved_but_unparsed_target_is_not_a_failure():
    """`import './style.css'` works. The target is just not a mapped language.

    Calling it broken would be the same class of false claim as filing a
    broken relative path under external, so it is counted as RESOLVED and the
    target is emitted for what it is.
    """
    run = _extract({
        "app/main.js": "import './style.css';\nexport function go() { return 1; }\n",
        "app/style.css": "body { color: red }\n",
    })
    assert _states(run)["failed"] == 0, (
        f"a css import resolves on disk and must not be reported broken, "
        f"got {_states(run)}"
    )
    assert "./style.css" in _typed(run, "UnparsedFile"), (
        "the target resolved to a real file and must be emitted as one"
    )


# ─── the vocabulary cannot drift away from the classifier ────────────────────

def test_the_import_relation_set_is_pinned_to_the_vocabulary():
    from extractor import _IMPORT_RELATIONS
    from relations import _RELATION_NAMES

    unknown = _IMPORT_RELATIONS - _RELATION_NAMES
    assert not unknown, (
        f"{sorted(unknown)} is classified as an import relation but is not in "
        f"the vocabulary; one of the two is wrong"
    )
    # A sixth import relation must not be able to join the vocabulary and
    # quietly skip classification -- that is how 19 relations were silently
    # dropped in #107.
    smells = {n for n in _RELATION_NAMES
              if "import" in n or "include" in n or "export" in n}
    unclassified = smells - _IMPORT_RELATIONS
    assert not unclassified, (
        f"relation(s) {sorted(unclassified)} look like imports but are not in "
        f"_IMPORT_RELATIONS. Decide: does the relation NAME a module target? "
        f"Then add it there, so its targets get classified instead of dangling"
    )


# ─── law 24: the control that separates "clean" from "could not run" ─────────

def test_an_empty_extraction_cannot_read_as_a_clean_one():
    """The harness itself must refuse a run that proved nothing.

    Every assertion in this file is of the form "no import is mis-stated". All
    of them pass over an empty map. So the guard belongs in the harness, and
    this leg proves the guard fires rather than assuming it.
    """
    try:
        _extract({"README.txt": "not a parsed language\n"})
    except AssertionError as exc:
        # Two gates can catch it and either is a pass: the extractor exits
        # non-zero on a tree it could not parse, and the harness refuses a map
        # with no entities. What must NOT happen is the run being accepted.
        msg = str(exc)
        assert ("no entities" in msg or "extraction failed" in msg), (
            f"the harness rejected the empty run for an unrelated reason, so "
            f"it is not the guard this leg claims to prove: {msg[:200]}"
        )
        return
    raise AssertionError(
        "an extraction with no entities was accepted; every leg in this file "
        "would then pass on a map that contains nothing"
    )


if __name__ == "__main__":
    # Every leg runs even after one fails, and the exit code is the run's.
    # CI runs this file as plain `python <file>` with no pytest installed
    # (`ci.yml`, the `scripts/ast/test_*.py` step), so a bare `def test_` with
    # no driver here would be DEFINED, never CALLED, and the job would go
    # green having executed nothing.
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    rc = 0
    executed = 0
    for t in tests:
        executed += 1
        try:
            t()
            print(f"ok   {t.__name__}")
        except Exception as exc:
            rc = 1
            first = str(exc).splitlines()[0] if str(exc) else type(exc).__name__
            print(f"FAIL {t.__name__}: {first}")
    # Law 23: print what was CHECKED, and a checked count of zero is a failure
    # that says "this proved nothing" -- never a pass.
    print(f"ran {executed} of {len(tests)} legs; rc={rc}")
    if executed == 0:
        print("VOID: zero legs executed, so this file proved nothing")
        sys.exit(2)
    sys.exit(rc)
