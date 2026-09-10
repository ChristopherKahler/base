#!/usr/bin/env python3
"""`config('app.name')` must resolve to `config/app.php` in ANOTHER file, as `uses_config`.

THE DEFECT THESE LEGS PIN (N-HELPER-NEVER-RESOLVES). The config branch resolved its key
against `label_to_nid`, which holds only the CURRENT FILE's nodes. `config/app.php` is a
different file, so the lookup missed and the reference was dropped with no edge and no
report. Measured on a real 68-file Laravel tree at `main 96df6829`: **26 keys read, ZERO
edges emitted**, and zero of those 26 resolved in-file -- so the whole feature was silent
on a real app.

THE BLOCKER `test_relation_is_uses_config` EXISTS FOR. Handing the miss to the cross-file
promotion in `extract()` is the obvious fix and is wrong on its own: that promotion
HARD-CODED `"relation": "calls"`. Routed in unchanged it asserts that a controller CALLS
`config/app.php` -- fabricated relationship data, and WORSE than the silence it replaced,
because a wrong edge is believed. `test_cross_file_resolves` passes for such an
implementation. Only the relation leg fails it.

`test_stale_cache_defaults` IS NOT OPTIONAL. `raw_calls` is PERSISTED to the per-file
cache (`cache.py:217` writes the whole result dict; a cached entry's top-level keys are
`edges, nodes, raw_calls`). A cache written before the `relation` field existed is read
back after it, so the promotion reads `rc.get("relation", "calls")` and must NEVER require
the key. An implementation using `rc["relation"]` raises on a stale cache; one with a wrong
default silently mis-relates. **Both look like "works" to every other leg here.**

`test_collision_reports` asserts the REPORT, not just the zero. A colliding file-shaped
target reads zero either way, so a count-only leg is identical before and after the fix and
proves nothing. Measured on the real tree: 68 files, 66 distinct basenames, `auth.php`
twice, and one of the two is among the 11 `config/` files -- so a Laravel app with
`config/auth.php` and any other `auth.php` loses that key. Reporting rather than preferring
a `config/` directory match is deliberate: framework-specific logic inside a pass built for
every language is a defect waiting for its second framework.

NO PYTEST. `ci.yml` runs each of these files as plain `python <file>`, and pytest is not
installed in that job, so bare `def test_` functions would be defined, called by nothing,
and the job would go green having executed zero legs. The `__main__` driver below is what
makes them run, and `check_test_reachability.py` is what keeps that true.
"""
import contextlib
import io
import json
import sys
import tempfile
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import cache as cache_mod
from extractor import extract, extract_php

# A Laravel config file: a bare `return [...]`, no class. Its ONLY node is the file node,
# whose label is `<name>.php` -- which is exactly the key the fix looks up. Measured: the
# BARE segment (`app`) is not a key in the global label index at all, and only `app.php`
# resolves, which is the form the config branch already computes.
CONFIG_FILE = "<?php\nreturn ['b' => 1, 'x' => 2];\n"


def _write(root, files):
    written = []
    for rel, text in files.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf8")
        written.append(p)
    return sorted(written)


def _run(root, paths):
    """stderr captured in-process, which parallel=False makes possible."""
    buf = io.StringIO()
    with contextlib.redirect_stderr(buf):
        result = extract(paths, cache_root=root, parallel=False)
    return result, buf.getvalue()


def _labels(result):
    return {n["id"]: n.get("label") for n in result.get("nodes", [])}


def _uses(result):
    return [e for e in result.get("edges", [])
            if str(e.get("relation", "")).startswith("uses_")]


@contextlib.contextmanager
def _box(name):
    """A unique, cache-free directory per leg. `.php` is not in
    `_JS_CACHE_BYPASS_SUFFIXES`, so a shared root would let one leg serve another's
    answer."""
    with tempfile.TemporaryDirectory(prefix="cfgxf_%s_" % name) as d:
        root = Path(d) / uuid.uuid4().hex[:8]
        root.mkdir(parents=True)
        assert not (root / ".base-ast-cache").exists(), "cache pre-existing in %s" % root
        yield root


def test_ordinary_cross_file_call_still_resolves():
    """POSITIVE CONTROL, and it must be read FIRST.

    If the cross-file promotion cannot emit an ordinary `calls` edge at all, then every
    "0 edges" leg below reads 0 for a reason that has nothing to do with what it claims
    to test, and the file goes green having proved nothing. #118 lost thirteen arms to
    exactly this shape.
    """
    with _box("ordinary") as root:
        paths = _write(root, {
            "caller.php": "<?php\nfunction caller() {\n    g();\n}\n",
            "other.php": "<?php\nfunction g() { return 1; }\n",
        })
        result, _err = _run(root, paths)
        labels = _labels(result)
        calls = [e for e in result.get("edges", [])
                 if e.get("relation") == "calls"
                 and labels.get(e.get("target")) in ("g", "g()")]
        assert len(calls) == 1, (
            "POSITIVE CONTROL FAILED: the cross-file promotion emitted %d ordinary "
            "calls edges, so every zero-edge leg in this file is vacuous" % len(calls))
        assert calls[0].get("context") == "call", (
            "an ordinary call edge lost its context field: %r" % calls[0].get("context"))


def test_cross_file_resolves():
    """THE FINDING: a config key naming another file must produce exactly one edge."""
    with _box("crossfile") as root:
        paths = _write(root, {
            "caller.php": "<?php\nfunction caller() {\n    config('a.b');\n}\n",
            "a.php": CONFIG_FILE,
        })
        result, _err = _run(root, paths)
        labels = _labels(result)
        uses = _uses(result)
        targets = sorted(labels.get(e["target"], "?") for e in uses)
        assert len(uses) == 1, "expected 1 uses_ edge, got %d" % len(uses)
        assert targets == ["a.php"], "expected the a.php file node, got %r" % targets


def test_relation_is_uses_config():
    """THE BLOCKER. A config key is a REFERENCE, not a call.

    An implementation that routes the miss into `raw_calls` without carrying its own
    relation passes `test_cross_file_resolves` and asserts here that a controller CALLS
    `config/app.php`.
    """
    with _box("relation") as root:
        paths = _write(root, {
            "caller.php": "<?php\nfunction caller() {\n    config('a.b');\n}\n",
            "a.php": CONFIG_FILE,
        })
        result, _err = _run(root, paths)
        rels = sorted({e["relation"] for e in _uses(result)})
        assert rels == ["uses_config"], (
            "expected ['uses_config'], got %r -- a 'calls' edge here asserts that a "
            "caller CALLS a config file, which is fabricated relationship data" % rels)
        # The in-file uses_* emission carries no "context", so the two lanes must agree
        # on the shape they produce.
        for e in _uses(result):
            assert "context" not in e, (
                "a uses_ edge carried context=%r; the in-file lane sets none"
                % e.get("context"))


def test_in_file_resolution_unchanged():
    """What #118 shipped. A fix that moves resolution to the global pass breaks this."""
    with _box("infile") as root:
        paths = _write(root, {
            "caller.php": "<?php\nclass a { public function m() { return 1; } }\n"
                          "function caller() {\n    config('a.b');\n}\n",
        })
        result, _err = _run(root, paths)
        uses = _uses(result)
        rels = sorted({e["relation"] for e in uses})
        assert len(uses) == 1, "in-file resolution emitted %d edges" % len(uses)
        assert rels == ["uses_config"], "in-file relation changed to %r" % rels


def test_missing_target_stays_silent():
    """NEGATIVE CONTROL. A missing TARGET is not an unknown VALUE.

    The key `zzz.b` reads perfectly and names nothing, so silence is correct. A report
    here would mean the fix reports on every non-emitting call, which is the opposite
    defect and the one `_report_no_literal_key` was deliberately not widened to cover.
    """
    with _box("silent") as root:
        paths = _write(root, {
            "caller.php": "<?php\nfunction caller() {\n    config('zzz.b');\n}\n",
            "a.php": CONFIG_FILE,
        })
        result, err = _run(root, paths)
        assert len(_uses(result)) == 0, "emitted an edge for an unresolvable key"
        assert "ambiguous" not in err, (
            "reported a key that simply has no target: %r" % err[:300])


def test_collision_reports():
    """Zero edges AND one report naming the file, the target and the candidate count."""
    with _box("collision") as root:
        paths = _write(root, {
            "caller.php": "<?php\nfunction caller() {\n    config('auth.x');\n}\n",
            "config/auth.php": CONFIG_FILE,
            "modules/auth.php": CONFIG_FILE,
        })
        result, err = _run(root, paths)
        assert len(_uses(result)) == 0, "emitted an edge for an ambiguous target"
        # A targeted contract, never "stderr is non-empty" -- the extractor prints other
        # warnings and an empty-vs-nonempty test would read those as a pass.
        hits = [ln for ln in err.splitlines()
                if "caller.php" in ln
                and ":3 " in ln
                and "'auth.php'" in ln
                and "matches 2 nodes" in ln
                and "ambiguous, no uses_config edge emitted" in ln]
        assert len(hits) == 1, (
            "expected exactly one conforming report line, got %d from stderr %r"
            % (len(hits), err[:400]))


def test_ordinary_collision_stays_silent():
    """The scope of the collision report, asserted rather than assumed.

    An ordinary cross-file call to a name that resolves to several nodes is the case the
    uniqueness gate exists for -- the comment there names `log`, `execute` and `find` --
    and its silence is documented behaviour, not a defect. Widening the report to every
    entry would print a line per such call on any large repo. This leg is what stops the
    report from being scoped by accident: without it, dropping the `relation != "calls"`
    condition changes real behaviour and no leg notices.
    """
    with _box("ordcollide") as root:
        paths = _write(root, {
            "caller.php": "<?php\nfunction caller() {\n    log_it();\n}\n",
            "one.php": "<?php\nfunction log_it() { return 1; }\n",
            "two.php": "<?php\nclass log_it { public function m() { return 2; } }\n",
        })
        result, err = _run(root, paths)
        labels = _labels(result)
        cands = [nid for nid, lab in labels.items()
                 if str(lab).strip("()").lower() == "log_it"]
        assert len(cands) > 1, (
            "FIXTURE CONTROL FAILED: log_it resolves to %d nodes, so this leg is not "
            "testing a collision at all" % len(cands))
        calls = [e for e in result.get("edges", [])
                 if e.get("relation") == "calls"
                 and str(labels.get(e.get("target"), "")).strip("()").lower() == "log_it"]
        assert len(calls) == 0, "an ambiguous ordinary call was promoted: %d" % len(calls)
        assert "ambiguous" not in err, (
            "the collision report widened to ordinary calls, which would print a line "
            "per colliding common name on any large repo: %r" % err[:300])


def test_stale_cache_defaults():
    """A cache written BEFORE the `relation` field existed must still read.

    Built by extracting the fixture, DELETING `relation` from every raw_calls entry, and
    planting the result as the cache. The cache must be proven CONSUMED or this leg is
    vacuous: an ignored cache would be re-extracted and read `uses_config`, which is a
    different failure wearing the same face. A sentinel node does that positively.
    """
    with _box("stalecache") as root:
        paths = _write(root, {
            "caller.php": "<?php\nfunction caller() {\n    config('a.b');\n}\n",
            "a.php": CONFIG_FILE,
        })
        caller = [p for p in paths if p.name == "caller.php"][0]
        per_file = extract_php(caller)
        raw = per_file.get("raw_calls", [])
        assert any("relation" in rc for rc in raw), (
            "the fixture produced no raw_calls entry carrying a relation, so stripping "
            "the key strips nothing and this leg would prove nothing")
        old_shape = [{k: rc[k] for k in ("caller_nid", "callee", "is_member_call",
                                         "source_file", "source_location") if k in rc}
                     for rc in raw]

        sentinel = "cache_sentinel_%s" % uuid.uuid4().hex[:8]
        entry = {
            "nodes": per_file.get("nodes", []) + [{
                "id": sentinel, "label": sentinel, "source_file": str(caller),
                "file_type": "code",
            }],
            "edges": per_file.get("edges", []),
            "raw_calls": old_shape,
        }
        cdir = cache_mod.cache_dir(root, "ast")
        h = cache_mod.file_hash(caller, root)
        (cdir / ("%s.json" % h)).write_text(json.dumps(entry), encoding="utf8")

        result, _err = _run(root, paths)   # must not raise
        labels = _labels(result)
        assert sentinel in labels, (
            "the planted cache was NOT consumed, so this leg never exercised the stale "
            "shape it exists for")
        cross = [e for e in result.get("edges", [])
                 if labels.get(e.get("target")) == "a.php"]
        rels = sorted({e["relation"] for e in cross})
        assert rels == ["calls"], (
            "a relation-less cached entry must default to 'calls', got %r" % rels)


def test_interpolated_key_never_reaches_cross_file():
    """A non-literal must not be handed to the resolver, which would invent a target
    one layer further out than #118's rejection.

    Observed at the PER-FILE layer, the only layer that can see `raw_calls` at all --
    `extract()` does not return it.
    """
    with _box("interp") as root:
        paths = _write(root, {
            "caller.php": '<?php\nfunction caller() {\n    config("a.$key");\n}\n',
            "a.php": CONFIG_FILE,
        })
        caller = [p for p in paths if p.name == "caller.php"][0]
        leaked = [rc for rc in extract_php(caller).get("raw_calls", [])
                  if str(rc.get("relation", "")).startswith("uses_")
                  or str(rc.get("callee", "")).endswith(".php")]
        assert leaked == [], "an interpolated key reached raw_calls: %r" % leaked
        result, err = _run(root, paths)
        assert len(_uses(result)) == 0, "an interpolated key produced an edge"
        assert "interpolates" in err, (
            "the #118 interpolation report stopped firing: %r" % err[:300])


def test_member_call_still_skipped():
    """The `is_member_call` guard, in case it is dropped while touching the loop."""
    with _box("member") as root:
        paths = _write(root, {
            "caller.php": "<?php\nfunction caller() {\n    $o->g();\n}\n",
            "other.php": "<?php\nfunction g() { return 1; }\n",
        })
        result, _err = _run(root, paths)
        labels = _labels(result)
        calls = [e for e in result.get("edges", [])
                 if e.get("relation") == "calls"
                 and labels.get(e.get("target")) in ("g", "g()")]
        assert len(calls) == 0, (
            "a member call was promoted across files: %d edges" % len(calls))


if __name__ == "__main__":
    # The positive control runs FIRST and by name. If the promotion cannot fire, every
    # zero-edge leg below is vacuous and this file must say so rather than go green.
    ordered = [test_ordinary_cross_file_call_still_resolves]
    ordered += [v for k, v in sorted(globals().items())
                if k.startswith("test_") and callable(v) and v not in ordered]
    rc = 0
    executed = 0
    for t in ordered:
        executed += 1
        try:
            t()
            print("ok   %s" % t.__name__)
        except Exception as exc:                      # noqa: BLE001
            rc = 1
            first = str(exc).splitlines()[0] if str(exc) else type(exc).__name__
            print("FAIL %s: %s" % (t.__name__, first))
            if t is ordered[0]:
                print("VOID: the positive control failed, so no count below is evidence")
                sys.exit(2)
    print("ran %d of %d legs; rc=%d" % (executed, len(ordered), rc))
    if executed == 0:
        print("VOID: zero legs executed, so this file proved nothing")
        sys.exit(2)
    sys.exit(rc)
