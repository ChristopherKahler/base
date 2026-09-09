#!/usr/bin/env python3
"""One IRI, one declaration line (#98).

`extract()` accumulates nodes with `all_nodes.extend(...)` once per file and dedupes ids
only WITHIN a file (`seen_ids` is per-file, `extractor.py:1610`). A symbol referenced from
six files therefore arrives at the serializer as six dicts carrying one id, and every one of
them used to be emitted as its own `code:<id> a ops:<T>` block. Measured on base's own tree
at `cbb375b2`: 9 IRIs over 17 excess declarations, occurrences [6,6,2,2,2,2,2,2,2]. The map
held one node per IRI while the TTL declared it many times, so `base ast query` answered with
a single node standing for several separate sites and said nothing about it.

#98's ruling is option C: deduplicate at emission. This file pins that.

WHY THE FIRST ARM IS A POSITIVE CONTROL AND NOT A COURTESY
----------------------------------------------------------
Every other assertion here is of the form "no IRI is declared twice", and all of them pass
perfectly over a fixture where nothing ever collided -- which is what this file would become
the moment the fixture stops reproducing the shape. So the extractor layer is asserted
FIRST and in the opposite direction: `extract()` must still hand back MORE THAN ONE dict for
some id. That is the collision pressure the fix exists to absorb; option C deliberately did
not remove it. If that arm goes red, this file has stopped testing anything and says so,
rather than reporting a clean map (PROCESS laws 24, 25 and 48).

The two layers are also two independent detectors, and each fires on its own mutation
(law 39): break the dedupe and the TTL arms go red while the extractor arm stays green;
stop the extractor duplicating and the extractor arm goes red while the TTL arms stay green.

WHY THE DETERMINISM ARM IS THE SHARPEST ONE
-------------------------------------------
The obvious dedupe is keep-first, and on the measured tree it is WRONG. 6 of the 9 collisions
are a real file node meeting an import-target stub for that same file, identical but for the
source line -- and node order does not agree on which arrives first: `GraphExplorer.svelte`
had line 0 then 1, `CostAttribution.svelte` had 1 then 0. Keep-first therefore publishes
`ops:sourceLine 0` for five panels and `1` for the sixth, a coin flip per id, and no arm
counting declarations can see it. `test_the_survivor_does_not_depend_on_dict_order` feeds
`_plan_declarations` the same group in both orders and requires the same winner; it fails on
keep-first and passes on "a real line beats the 0 placeholder, then first-in-order".

Run: python3 scripts/ast/test_declaration_dedupe.py
"""

import os
import re
import subprocess
import sys
import tempfile
from collections import Counter, namedtuple
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

from extractor import extract, collect_files  # noqa: E402
from ttl_serializer import _plan_declarations, sanitize_iri, serialize  # noqa: E402

Run = namedtuple("Run", "ttl notices")

# Mirrors the shape measured on base's own `dashboard/` tree, which is where all nine
# collisions live: an App that imports two panels by relative path, panels that each
# import the same external packages, and a relative import that resolves to nothing.
# Every collision family #98 names is present:
#   `svelte` / `d3`      -> one ExternalModule dict per importing file
#   `../lib/api.js`      -> one UnresolvedImport dict per importing file
#   `./panels/*.svelte`  -> a real FILE node meeting an import stub for the same file,
#                           which is the family that carries the source-line conflict
FIXTURE = {
    "dashboard/src/App.svelte": (
        "<script>\n"
        "  import Alpha from './panels/Alpha.svelte';\n"
        "  import Beta from './panels/Beta.svelte';\n"
        "  import { onMount } from 'svelte';\n"
        "</script>\n"
        "<main><Alpha /><Beta /></main>\n"
    ),
    "dashboard/src/panels/Alpha.svelte": (
        "<script>\n"
        "  import { onMount } from 'svelte';\n"
        "  import * as d3 from 'd3';\n"
        "  import { getNodes } from '../lib/api.js';\n"
        "</script>\n"
        "<div>alpha</div>\n"
    ),
    "dashboard/src/panels/Beta.svelte": (
        "<script>\n"
        "  import { onDestroy } from 'svelte';\n"
        "  import * as d3 from 'd3';\n"
        "  import { getEdges } from '../lib/api.js';\n"
        "</script>\n"
        "<div>beta</div>\n"
    ),
}

DECL = re.compile(r"^code:(\S+)\s+a\s+ops:(\S+)", re.M)


def _fixture_root(td: str) -> Path:
    root = Path(td)
    for rel, body in FIXTURE.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(body, encoding="utf-8")
    subprocess.run(["git", "init", "-q"], cwd=str(root), check=True)
    return root


def _extract_ttl() -> Run:
    """The real entry point over a real repo.

    NOT under /tmp: file discovery drops any path with a `tmp` component, so a fixture
    there extracts to nothing and every "no duplicate declarations" leg below passes
    over an empty map. `test_node_kinds.py` carries the same note for the same reason.
    """
    with tempfile.TemporaryDirectory(prefix="base-ast-d98-", dir=Path.home()) as td:
        root = _fixture_root(td)
        proc = subprocess.run(
            [sys.executable, str(HERE / "onto_ast.py"), str(root),
             "--project", "t", "--full"],
            capture_output=True, text=True, cwd=str(HERE),
            env=os.environ.copy(), timeout=300,
        )
        assert proc.returncode == 0, f"extraction failed:\n{proc.stderr}"
        run = Run(proc.stdout, proc.stderr)
    assert "a ops:" in run.ttl, "extraction produced no entities; the fixture never parsed"
    return run


def _extract_nodes() -> list[dict]:
    """The raw node dicts, before any serializer sees them."""
    with tempfile.TemporaryDirectory(prefix="base-ast-d98n-", dir=Path.home()) as td:
        root = _fixture_root(td)
        files = collect_files(root)
        assert files, "collect_files found nothing; the fixture never reached the extractor"
        return extract(files, cache_root=root)["nodes"]


def _declarations(run: Run) -> Counter:
    """Count declarations per id, anchored at COLUMN 0.

    A TTL repeats the same id inside predicate objects (`ops:definedIn code:<id>`) and
    inside `rdfs:label` prose. Only `code:<id> a ops:<T>` at column 0 is a declaration;
    an unanchored match counts references as declarations and inflates every number here.
    """
    return Counter(m.group(1) for m in DECL.finditer(run.ttl))


def test_the_extractor_still_emits_more_than_one_dict_per_id():
    """POSITIVE CONTROL. Without this, every arm below passes over a fixture that never
    collided, and a fixture that stopped reproducing the shape would look like a fix."""
    nodes = _extract_nodes()
    per_id = Counter(n["id"] for n in nodes)
    repeats = {k: v for k, v in per_id.items() if v > 1}
    assert repeats, (
        "no id arrives from extract() more than once, so this fixture no longer "
        "reproduces #98's collision pressure and NOTHING in this file is being "
        "tested. Fix the fixture, do not delete the arm."
    )


def test_no_iri_is_declared_more_than_once():
    run = _extract_ttl()
    counts = _declarations(run)
    assert counts, "the column-0 declaration anchor matched nothing"
    dupes = {k: v for k, v in counts.items() if v > 1}
    assert not dupes, (
        f"{len(dupes)} IRI(s) declared more than once, {sum(v - 1 for v in dupes.values())} "
        f"excess declaration(s): {dict(sorted(dupes.items(), key=lambda x: -x[1]))}"
    )


def test_a_shared_external_package_is_declared_once():
    """`svelte` and `d3` are each imported by more than one fixture file."""
    run = _extract_ttl()
    counts = _declarations(run)
    for pkg in ("t_svelte", "t_d3"):
        assert counts.get(pkg, 0) == 1, (
            f"{pkg} is declared {counts.get(pkg, 0)} time(s), expected exactly 1. "
            f"0 means the fix deleted the node rather than deduplicating its "
            f"declaration, which this arm exists to separate from a repair."
        )


def test_a_file_that_is_also_an_import_target_is_declared_once():
    """The family that carries the source-line conflict: a real file node meeting an
    import-target stub for that same file."""
    run = _extract_ttl()
    counts = _declarations(run)
    panels = [k for k in counts if k.endswith("_svelte") and "panels" in k]
    assert panels, (
        "no panel file node in the map, so the file-node-meets-import-stub family "
        "is not represented and this arm proved nothing"
    )
    for p in panels:
        assert counts[p] == 1, f"{p} is declared {counts[p]} time(s), expected 1"


def test_no_iri_is_declared_with_two_different_types():
    """#98 owns declaration COUNT and must never change a node's rdf:type -- retyping was
    #105, which is merged and closed. A collapsed group that disagreed on its class and
    then had a winner picked for it would show up here as one IRI with two classes."""
    run = _extract_ttl()
    by_iri = {}
    for m in DECL.finditer(run.ttl):
        by_iri.setdefault(m.group(1), set()).add(m.group(2))
    conflicted = {k: sorted(v) for k, v in by_iri.items() if len(v) > 1}
    assert not conflicted, f"IRI(s) declared with more than one ops class: {conflicted}"


def test_the_survivor_does_not_depend_on_dict_order():
    """The keep-first regression guard, and the one arm that fails on keep-first.

    Measured on base's own tree: 6 of the 9 collisions are a file node and an import stub
    for the same file, identical but for the source line, and node order does NOT agree on
    which arrives first. Keep-first therefore publishes `ops:sourceLine 0` for some and `1`
    for others -- a coin flip that no declaration count can detect.
    """
    stub = {"id": "panels_alpha_svelte", "label": "./panels/Alpha.svelte",
            "file_type": "code", "type": "import_unresolved"}
    filenode = {"id": "panels_alpha_svelte", "label": "Alpha.svelte",
                "file_type": "code", "source_location": "L1"}

    forward, _, _, _ = _plan_declarations([stub, filenode], "t", "code:t_root")
    reverse, _, _, _ = _plan_declarations([filenode, stub], "t", "code:t_root")
    iri = f"code:t_{sanitize_iri('panels_alpha_svelte')}"

    assert forward[iri] == 1, (
        "with the stub first, the dict carrying a real source line (index 1) must win; "
        f"index {forward[iri]} won, which is keep-first and publishes sourceLine 0"
    )
    assert reverse[iri] == 0, (
        "with the file node first, that same dict (now index 0) must win; "
        f"index {reverse[iri]} won"
    )
    won_forward = [stub, filenode][forward[iri]]
    won_reverse = [filenode, stub][reverse[iri]]
    assert won_forward is won_reverse, (
        "the two orderings chose DIFFERENT dicts, so the survivor depends on the order "
        "files happened to be walked in"
    )
    assert won_forward.get("source_location") == "L1", (
        "the surviving dict has no real source line, so the declaration will carry the "
        "0 placeholder and the only line information available was thrown away"
    )


def test_the_module_iri_is_not_declared_twice():
    """The file/app-root module block is written before the node loop, so it has already
    spent its IRI's one declaration. A node sharing that IRI must not declare it again."""
    node = {"id": "root", "label": "root", "file_type": "code"}
    owner, dup_iris, dropped, _ = _plan_declarations([node], "t", "code:t_root")
    assert owner["code:t_root"] == -1, (
        "a node sharing the pre-loop module IRI was granted a declaration, so that IRI "
        "is declared twice: once by the module block and once by the node loop"
    )
    assert dup_iris == 1 and dropped == 1, (
        f"the collision with the module block must be COUNTED, not silently dropped; "
        f"got duplicate_iris={dup_iris} dropped={dropped}"
    )


def test_a_type_disagreement_stops_the_run():
    """#98's required arm, proven by MUTATION rather than by reading (law 38).

    Two dicts, one id, different explicit types. Emission must STOP naming both classes
    rather than collapse them onto a silently-picked winner -- retyping is #105's
    territory and it is closed. An arm that only asserted "no type changed" over a tree
    where none ever disagreed would pass identically with this guard deleted.
    """
    a = {"id": "shared", "label": "shared", "file_type": "code",
         "type": "import_external"}
    b = {"id": "shared", "label": "shared", "file_type": "code",
         "type": "import_unresolved"}
    try:
        serialize({"nodes": [a, b], "edges": []}, "t", "t.py", "python")
    except ValueError as exc:
        msg = str(exc)
        assert "rdf:type" in msg, f"the refusal does not say what it is protecting: {msg}"
        assert "ExternalModule" in msg and "UnresolvedImport" in msg, (
            f"the refusal must name BOTH classes so the disagreement can be found "
            f"upstream; got: {msg}"
        )
        return
    raise AssertionError(
        "a type disagreement did NOT stop the run, so the 'no rdf:type changes' arm "
        "is dead and a collapsed group would publish an arbitrary winner's class"
    )


def test_an_agreeing_group_does_not_trip_the_type_guard():
    """Law 40's other direction: the naive way to make a guard fire is to key it so
    loosely that a clean tree goes red. Identical types must collapse silently to one
    declaration, not raise."""
    a = {"id": "shared", "label": "shared", "file_type": "code",
         "type": "import_external"}
    b = {"id": "shared", "label": "shared", "file_type": "code",
         "type": "import_external"}
    ttl = serialize({"nodes": [a, b], "edges": []}, "t", "t.py", "python")
    decls = [ln for ln in ttl.splitlines() if ln.startswith("code:t_shared ")]
    assert len(decls) == 1, (
        f"an agreeing duplicate group produced {len(decls)} declarations, expected 1: "
        f"{decls}"
    )
    assert "ops:ExternalModule" in decls[0], (
        f"the surviving declaration changed class: {decls[0]}"
    )


def test_the_collapse_is_reported_not_silent():
    """A dedupe nobody counts is how the same defect walks back in unnoticed. The count
    is also the tripwire saying the upstream per-reference duplication is still there,
    which option C deliberately did not change."""
    run = _extract_ttl()
    m = re.search(r"# (\d+) repeat declaration\(s\) across (\d+) IRI", run.notices)
    assert m, (
        f"the run collapsed declarations without saying so. stderr was:\n{run.notices!r}"
    )
    collapsed, iris = int(m.group(1)), int(m.group(2))
    assert collapsed >= 1 and iris >= 1, (
        f"the notice reports {collapsed} collapsed across {iris} IRIs, but the fixture "
        f"is built to collide; a zero here means the notice is decoration"
    )


def test_counts_are_not_reported_over_an_empty_map():
    """Law 24's control. Every arm above except the first is of the form "nothing is
    duplicated", and all of them are true of a map with no declarations at all."""
    run = _extract_ttl()
    counts = _declarations(run)
    assert len(counts) >= 5, (
        f"only {len(counts)} declaration(s) in the whole map, so 'no IRI is declared "
        f"twice' is true of almost nothing and this file proved nothing"
    )
    nodes = _extract_nodes()
    assert len(nodes) > len(counts) or len(nodes) >= 5, (
        f"the extractor produced {len(nodes)} node dict(s) against {len(counts)} "
        f"declared IRIs; the fixture is too thin to test a collapse"
    )


if __name__ == "__main__":
    # CI runs this as plain `python <file>` with no pytest installed, so the driver is
    # not optional: bare `def test_` functions would be defined, called by nothing, and
    # the job would go green having executed zero legs.
    tests = [v for k, v in sorted(globals().items())
             if k.startswith("test_") and callable(v)]
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
