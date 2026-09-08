#!/usr/bin/env python3
"""A heading is not a callable, prose is not a const, a key is not a function (#105).

`ttl_serializer.py`'s type fallback was the literal `"function"`, and ZERO of
the 5048 nodes on base's own tree carried the `type` key it reads first. So
every node no edge could classify was declared `ops:Function`, and
`ast query --contains` answered with it:

    CHANGELOG.md:1                  fn Changelog
    dashboard/package-lock.json:19  fn node_modules/@esbuild/aix-ppc64
    scripts/ast/cache.py:43         const Strip YAML frontmatter from Markdown ...

591 markdown headings, 214 fenced code blocks and 55 lockfile keys on base's
own docs. The fix is in two halves and BOTH are needed: the extractor that
parsed the thing says what it is, and the serializer stops inventing
`function` for everything else.

The direction of the failure is the reason this file exists: every assertion
here is "X is not declared a callable", and all of them pass over a map that
contains nothing at all. The law-24 control at the bottom is what makes the
rest of the file mean anything.

Run: python3 scripts/ast/test_node_kinds.py   (or: pytest scripts/ast/)
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

Run = namedtuple("Run", "ttl notices")

FIXTURE = {
    "README.md": (
        "# Top Heading\n\n"
        "Some prose.\n\n"
        "## Need an official Svelte framework?\n\n"
        "```bash\ncp -r claude/skills/base-help ~/.claude/skills/\n```\n"
    ),
    "package-lock.json": (
        '{\n  "name": "fixture",\n  "packages": {\n'
        '    "node_modules/base64-js": { "version": "1.5.1" }\n  },\n'
        '  "dependencies": { "base64-js": "^1.5.1" }\n}\n'
    ),
    "mod.py": (
        '"""Strip YAML frontmatter from Markdown content, returning only the body."""\n'
        "def strip_it():\n"
        '    """Normalize path for consistent cache keys."""\n'
        "    return 1\n"
    ),
}


def _extract(tree: dict[str, str]) -> Run:
    """As `test_file_membership_walk.py`: a real repo, the real entry point.

    NOT under /tmp -- file discovery drops any path with a `tmp` component, so
    a fixture there extracts to nothing and every leg passes on an empty map.
    """
    with tempfile.TemporaryDirectory(prefix="base-ast-k105-", dir=Path.home()) as td:
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
    assert "a ops:" in run.ttl, "extraction produced no entities; the fixture never parsed"
    assert "# Extracting" in run.notices, (
        f"the notice stream is empty. stderr was:\n{run.notices!r}"
    )
    return run


def _blocks(run: Run) -> list[tuple[str, str, str]]:
    """(iri, ops_type, label) per subject block.

    Read from each subject's OWN block, never by nearest-preceding-line across
    the file (law 33: proximity is not attribution).
    """
    out = []
    for block in run.ttl.split("\n\n"):
        m = re.search(r"^(code:\S+) a ops:(\w+) ;", block, re.M)
        if not m:
            continue
        lab = re.search(r'^    rdfs:label "(.*)" ;$', block, re.M)
        out.append((m.group(1), m.group(2), lab.group(1) if lab else ""))
    return out


def _type_of(run: Run, label: str) -> str:
    for _, ops_type, lab in _blocks(run):
        if lab == label:
            return ops_type
    raise AssertionError(
        f"no node in the map is labelled {label!r}, so nothing can be asserted "
        f"about its type. Labels present: "
        f"{[l for _, _, l in _blocks(run)][:12]}"
    )


CALLABLE = ("Function", "Method")


def test_a_markdown_heading_is_not_a_callable():
    run = _extract(FIXTURE)
    for heading in ("Top Heading", "Need an official Svelte framework?"):
        got = _type_of(run, heading)
        assert got not in CALLABLE, f"heading {heading!r} is declared ops:{got}"
        assert got == "Heading", f"heading {heading!r} is declared ops:{got}"


def test_the_heading_is_still_emitted_at_all():
    """Law 25: the leg the fix could have MASKED.

    "No heading is declared a callable" is satisfied perfectly by an extractor
    that stopped emitting headings. The count must stay, and only the class may
    change -- `src/hook/automap.rs` counts these entities for its adoption
    fuse, so dropping them would move a number in an unrelated feature.
    """
    run = _extract(FIXTURE)
    headings = [l for _, t, l in _blocks(run) if t == "Heading"]
    assert len(headings) >= 2, (
        f"the fixture has two headings and the map has {len(headings)}; a fix "
        f"that stops emitting them passes every other leg in this file"
    )


def test_a_fenced_code_block_is_not_a_callable():
    run = _extract(FIXTURE)
    blocks = [(t, l) for _, t, l in _blocks(run) if l.startswith("code:")]
    assert blocks, "the fixture's fenced block produced no node at all"
    for ops_type, label in blocks:
        assert ops_type == "CodeBlock", f"{label!r} is declared ops:{ops_type}"


def test_a_lockfile_key_is_not_a_callable():
    """`fn node_modules/base64-js` -- a directory path typed as a function."""
    run = _extract(FIXTURE)
    got = _type_of(run, "node_modules/base64-js")
    assert got not in CALLABLE, f"a lockfile key is declared ops:{got}"
    assert got == "Property", f"a JSON key is a property, got ops:{got}"


def test_docstring_prose_is_not_a_const():
    """A docstring is prose. It was rendering as `const` in the query.

    The class stays `ops:Rationale` -- the graph legitimately holds these, and
    they carry `rationale_for` edges. What changed is that the query no longer
    calls it a constant, and #563 had already established in-tree that
    rationale labels are not identifiers.
    """
    run = _extract(FIXTURE)
    prose = [(t, l) for _, t, l in _blocks(run)
             if l.startswith("Strip YAML frontmatter")]
    assert prose, "the module docstring produced no node"
    for ops_type, label in prose:
        assert ops_type == "Rationale", f"{label[:40]!r} is declared ops:{ops_type}"
        assert ops_type not in CALLABLE


def test_a_file_node_is_still_a_module():
    """The JSON extractor types its keys, and its FILE node must not follow.

    A file's identity comes from `file_map`, which is stronger evidence than
    any per-node hint, so it wins over an explicit type. Without that, typing
    JSON keys would have retyped every `.json` file node along with them.
    """
    run = _extract(FIXTURE)
    for name in ("package-lock.json", "README.md", "mod.py"):
        got = _type_of(run, name)
        assert got == "Module", f"file node {name!r} is declared ops:{got}"


def test_the_serializer_reports_what_it_could_not_type():
    """"I do not know what this is" and "this is a function" are different claims.

    Only one of them was being made. A non-code node that reaches the
    serializer with no kind is now `ops:Entity` AND counted, because a
    fallback nobody counts is how 860 non-symbols came to be callable without
    anyone noticing.
    """
    from ttl_serializer import TYPE_MAP, serialize

    ttl = serialize(
        {"nodes": [{"id": "x", "label": "mystery", "file_type": "document"}],
         "edges": []},
        "t", "t.md", "markdown", file_map={}, stats=(stats := {}),
    )
    assert "a ops:Entity ;" in ttl, (
        f"an untyped non-code node must be declared ops:Entity, got:\n{ttl}"
    )
    assert "a ops:Function ;" not in ttl
    assert stats.get("untyped_non_code_entities") == 1, (
        f"the fallback must be counted, got {stats!r}"
    )
    assert "entity" in TYPE_MAP


def test_an_unknown_node_type_fails_loudly_instead_of_defaulting():
    """#107's ruling, applied to node types: raise where it used to default.

    A silent default is what made every unclassifiable node a function. A type
    the map has no class for is a programming error in the seam, and it now
    names the file to edit rather than shipping a wrong class.
    """
    from ttl_serializer import serialize

    try:
        serialize(
            {"nodes": [{"id": "x", "label": "y", "type": "no_such_kind",
                        "file_type": "code"}],
             "edges": []},
            "t", "t.py", "python", file_map={},
        )
    except KeyError as exc:
        assert "TYPE_MAP" in str(exc), f"the raise must name the fix: {exc}"
        return
    # An unknown explicit type currently falls through to the code default,
    # which is a silent reclassification rather than a loud stop.
    raise AssertionError(
        "an unknown node type was serialized without a word; a type with no "
        "ops: class must stop the run and name TYPE_MAP"
    )


def test_an_empty_extraction_cannot_read_as_a_clean_one():
    """Law 24. Every other leg here passes over a map containing nothing."""
    try:
        _extract({"notes.txt": "not a parsed language\n"})
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
        "an extraction with no entities was accepted; 'no callable is a "
        "heading' is then true of a map with no headings and no callables"
    )


if __name__ == "__main__":
    # CI runs this as plain `python <file>` with no pytest installed, so the
    # driver is not optional: bare `def test_` functions would be defined,
    # called by nothing, and the job would go green having executed zero.
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
    print(f"ran {executed} of {len(tests)} legs; rc={rc}")
    if executed == 0:
        print("VOID: zero legs executed, so this file proved nothing")
        sys.exit(2)
    sys.exit(rc)
