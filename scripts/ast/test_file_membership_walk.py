#!/usr/bin/env python3
"""File membership is transitive over containment, and ONLY over containment (#82).

`_build_file_membership` used to run three single passes: `contains` one hop from
a file node, `method` from an already-resolved class, `rationale_for`. Two things
were wrong with that shape and only the first is named in #82:

  DEPTH        a `contains` chain deeper than one hop was dropped, so every
               markdown heading below the first fell to the app root.
  RELATION     `defines` is emitted file->symbol and class->field at eight sites
               in `extractor.py`, and pass 1 matched on the literal string
               "contains". A C++ struct field is ONE HOP from a resolved struct
               and was lost anyway, purely because of the relation's name.

The fix walks to a fixed point over an explicit containment allowlist. The
allowlist is the load-bearing half: #82's text asks for a walk "rather than a
fixed set of relations", and an unbounded walk is a WORSE bug than the one being
fixed -- it would propagate membership along `calls` and `inherits` and attribute
a stdlib base class to whichever file happens to subclass it. `test_external_
symbol_stays_on_app_root` and `test_callee_keeps_its_own_file` are the guards
against exactly that, and they are green before the fix and must stay green
after it.

Not everything reaching the app root is a defect. A synthesised stand-in for a
class that lives OUTSIDE the tree (`RuntimeError`) has no file because there is
no file. It is expected to stay on the app root, and a run that drives the
count to zero has propagated membership to nodes with no legitimate file (#98).

Run: python3 scripts/ast/test_file_membership_walk.py   (or: pytest scripts/ast/)
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

COUNTER = re.compile(r"^# (\d+) entit(?:y|ies) attributed to the app root", re.M)

#: The map goes to stdout; the `# N entities attributed to the app root` notice
#: goes to STDERR (`onto_ast.py`, `file=sys.stderr`). Reading the counter off
#: stdout returns 0 for every tree ever measured -- an instrument that cannot
#: fail. Both streams are carried, separately and deliberately.
Run = namedtuple("Run", "ttl notices")


def _extract(tree: dict[str, str]) -> Run:
    """Write `tree` to a fixture repo, extract it, return the raw output.

    NOT under /tmp: file discovery drops any path with a `tmp` component, so a
    fixture there extracts to nothing and every assertion below would pass
    against an empty map.
    """
    with tempfile.TemporaryDirectory(prefix="base-ast-m82-", dir=Path.home()) as td:
        root = Path(td)
        for rel, body in tree.items():
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(body, encoding="utf-8")
        # File discovery walks a repo, so the fixture has to look like one.
        subprocess.run(["git", "init", "-q"], cwd=str(root), check=True)
        # Explicit env and a bounded wait: a child spawned by a differently
        # launched parent must not inherit a surprise, and a hung grammar must
        # fail the leg rather than the job's whole timeout.
        proc = subprocess.run(
            [sys.executable, str(HERE / "onto_ast.py"), str(root),
             "--project", "t", "--full"],
            capture_output=True, text=True, cwd=str(HERE),
            env=os.environ.copy(), timeout=300,
        )
        assert proc.returncode == 0, f"extraction failed:\n{proc.stderr}"
        run = Run(proc.stdout, proc.stderr)
    # An empty map cannot be allowed to read as a pass -- the shape that let a
    # tripwire report PASS having opened no files at all.
    assert "a ops:" in run.ttl, "extraction produced no entities; the fixture never parsed"
    # Nor can a silent stream. If the extractor said nothing on stderr the notice
    # channel is gone and every orphan assertion below would read a false zero.
    assert "# Extracting" in run.notices, (
        f"the extractor's notice stream is empty; `_orphans` cannot tell 0 from "
        f"missing. stderr was:\n{run.notices!r}"
    )
    return run


def _orphans(run: Run) -> int:
    """How many entities the extractor itself said fell to the app root.

    Read from the NOTICE stream, whose presence `_extract` has already proved.
    The line is only printed when the number is non-zero, so its absence here is
    a real zero rather than an unread stream.
    """
    m = COUNTER.search(run.notices)
    return int(m.group(1)) if m else 0


def _source_file(run: Run, label: str) -> str:
    """The sourceFile recorded for the entity carrying `label`."""
    lines = run.ttl.splitlines()
    idx = [i for i, ln in enumerate(lines) if f'rdfs:label "{label}"' in ln]
    assert idx, f"entity {label!r} is missing from the map entirely"
    window = "\n".join(lines[idx[0]: idx[0] + 5])
    m = re.search(r'ops:sourceFile "(.*?)" ;', window)
    assert m, f"entity {label!r} has no sourceFile:\n{window}"
    return m.group(1)


# --------------------------------------------------------------------------
# Depth: the live reproduction. #82's own `x.cpp` repro no longer reproduces.
# --------------------------------------------------------------------------

NESTED_HEADINGS = {
    "doc.md": "# Top Heading\n\nbody\n\n## Middle Heading\n\nbody\n\n### Deep Heading\n\nbody\n",
}


def test_nested_headings_belong_to_their_file():
    """h2 and h3 are two and three `contains` hops from the file node.

    Before the fix this failed by exactly 2: `Top Heading` resolved (one hop)
    and everything under it fell to the app root.
    """
    run = _extract(NESTED_HEADINGS)
    for label in ("Top Heading", "Middle Heading", "Deep Heading"):
        assert _source_file(run, label) == "doc.md", (
            f"{label!r} is attributed to {_source_file(run, label)!r}, not doc.md "
            f"-- the containment walk is not reaching a fixed point"
        )
    assert _orphans(run) == 0, (
        f"{_orphans(run)} entities on the app root; a tree of one markdown file "
        f"has no entity that legitimately belongs there"
    )


# --------------------------------------------------------------------------
# Relation: one hop from a resolved parent, lost on the relation's name.
# --------------------------------------------------------------------------

STRUCT_WITH_FIELD = {
    "f.cpp": "struct WithField {\n    int counter;\n    int f() { return counter; }\n};\n",
}


def test_class_field_belongs_to_its_file():
    """`counter` and `f` are siblings in one struct, reached by different relations.

    `f` arrives on `hasMethod` and always resolved. `counter` arrives on
    `defines` and fell to the app root -- same file, same parent, same depth.
    That is the relation half of #82, and it is not in the issue.
    """
    run = _extract(STRUCT_WITH_FIELD)
    assert _source_file(run, ".f()") == "f.cpp", "the method regressed"
    assert _source_file(run, "counter") == "f.cpp", (
        f"the struct field is attributed to {_source_file(run, 'counter')!r}; "
        f"`defines` is missing from the containment allowlist"
    )
    assert _orphans(run) == 0


# --------------------------------------------------------------------------
# The guards. Both are GREEN before the fix and must stay green after it.
# --------------------------------------------------------------------------

CROSS_FILE = {
    "callee.py": "def callee_fn():\n    return 1\n",
    "caller.py": (
        "from callee import callee_fn\n\n"
        "class MyErr(RuntimeError):\n    pass\n\n"
        "def caller_fn():\n    return callee_fn()\n"
    ),
}


def test_callee_keeps_its_own_file():
    """`calls` and `imports` are not containment.

    `caller.py` both imports and calls `callee_fn`. A walk that followed either
    would move `callee_fn` into the file that calls it.
    """
    run = _extract(CROSS_FILE)
    assert _source_file(run, "callee_fn()") == "callee.py", (
        f"callee_fn moved to {_source_file(run, 'callee_fn()')!r} -- membership "
        f"is propagating along `calls` or `imports`, which are cross-file"
    )


def test_external_symbol_stays_on_app_root():
    """A stand-in for a class outside the tree has no file, and must keep none.

    `RuntimeError` is synthesised because `MyErr` subclasses it; it exists in the
    map only as the target of an `inherits` edge. Attributing it to `caller.py`
    would be a false attribution that DROPS the app-root counter -- a regression
    wearing a green number. This is the #98 population, and #82 is not allowed
    to clear it.
    """
    run = _extract(CROSS_FILE)
    assert _source_file(run, "MyErr") == "caller.py"
    external = _source_file(run, "RuntimeError")
    assert external != "caller.py", (
        "RuntimeError was attributed to the file that subclasses it -- membership "
        "is propagating along `inherits`, which is not containment"
    )
    assert _orphans(run) == 1, (
        f"expected exactly 1 app-root entity (RuntimeError, no file in this tree), "
        f"got {_orphans(run)}. Zero here is a FAILURE signal, not a win."
    )


# --------------------------------------------------------------------------
# Unit legs on the seam itself. No grammar, no subprocess, no vacuous pass.
# --------------------------------------------------------------------------

def test_membership_is_independent_of_edge_order():
    """Three single passes made the result depend on the order `edges` arrived in.

    A class resolved later in the list than its own methods never propagated on
    that run. A fixed point does not care about order, and this asserts it
    rather than trusting it.
    """
    import random
    from ttl_serializer import _build_file_membership

    edges = [
        {"source": "f.md", "target": "h1", "relation": "contains"},
        {"source": "h1", "target": "h2", "relation": "contains"},
        {"source": "h2", "target": "h3", "relation": "contains"},
        {"source": "h3", "target": "cls", "relation": "contains"},
        {"source": "cls", "target": "m", "relation": "method"},
        {"source": "cls", "target": "fld", "relation": "defines"},
        {"source": "r", "target": "m", "relation": "rationale_for"},
        {"source": "other.md", "target": "x", "relation": "calls"},
    ]
    files = {"f.md", "other.md"}

    expected = {k: "f.md" for k in ("h1", "h2", "h3", "cls", "m", "fld", "r")}
    rng = random.Random(82)
    for _ in range(25):
        shuffled = edges[:]
        rng.shuffle(shuffled)
        got = _build_file_membership(shuffled, files)
        assert got == expected, (
            f"membership depends on edge order.\n  expected {expected}\n  got      {got}"
        )
    assert "x" not in _build_file_membership(edges, files), (
        "a `calls` target was given a file; the allowlist is not being enforced"
    )


def test_containment_allowlist_is_pinned_to_the_vocabulary():
    """Every name in the allowlist must be a real relation, and the complement
    is enumerated here on purpose.

    A 29th relation added to `relations._RELATION_NAMES` is silently excluded
    from membership. Silent is the right DEFAULT and the wrong DECISION, so this
    fails until someone states which side the new name belongs on -- the same
    shape as the three-list drift that #83 fixed.
    """
    import relations
    from ttl_serializer import CONTAINMENT_RELATIONS

    known = set(relations.RELATIONS)
    unknown = CONTAINMENT_RELATIONS - known
    assert not unknown, (
        f"allowlist names no relation the extractor can emit: {sorted(unknown)}. "
        f"Add them to _RELATION_NAMES in scripts/ast/relations.py or drop them."
    )

    # The complement, stated. Each of these is cross-file by construction: the
    # target lives somewhere the source merely refers to.
    NOT_CONTAINMENT = {
        "binds_method", "bound_to", "calls", "dynamic_import", "extends",
        "implements", "imports", "imports_from", "includes", "inherits",
        "instantiates", "listened_by", "re_exports", "reads_from", "references",
        "references_constant", "relatedTo", "supersedes", "triggers", "uses",
        "uses_component", "uses_config", "uses_static_prop",
    }
    unclassified = known - CONTAINMENT_RELATIONS - NOT_CONTAINMENT
    assert not unclassified, (
        f"relation(s) {sorted(unclassified)} are in the vocabulary but on neither "
        f"side of the containment split. Decide: does a {sorted(unclassified)[0]!r} "
        f"edge mean the target lives in the source's file? Then add it to "
        f"CONTAINMENT_RELATIONS in ttl_serializer.py, or to NOT_CONTAINMENT here."
    )
    overlap = CONTAINMENT_RELATIONS & NOT_CONTAINMENT
    assert not overlap, f"{sorted(overlap)} is on both sides of the split"


if __name__ == "__main__":
    # Every leg runs even after one fails, and the exit code is the run's --
    # a red table of all of them is worth more than the first traceback.
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    rc = 0
    for t in tests:
        try:
            t()
            print(f"ok   {t.__name__}")
        except Exception as exc:
            rc = 1
            first = str(exc).splitlines()[0] if str(exc) else type(exc).__name__
            print(f"FAIL {t.__name__}: {first}")
    print(f"ran {len(tests)} legs; rc={rc}")
    sys.exit(rc)
