#!/usr/bin/env python3
"""One key for `relatedTo`, on both extraction paths, inline AND block (#115).

A user who follows base's OWN published manual gets zero relationships back and
no warning. `docs/markdown-ontology-protocol.md` documents `related:` at :88 and
as a YAML block sequence at :102 and :120 -- and every documented example carries
`ontology: true`.

The two extractors disagree about the key, and the disagreement is silent:

  base sync        (Rust, the DEFAULT path)  reads `relatedto`   src/extract/frontmatter.rs:49
  base sync --ast  (Python, this file)       reads `related`     scripts/ast/extractor.py:6635

So on THIS path the documented `related:` works and `relatedTo:` -- the spelling
the Rust default path accepts, and the spelling of the emitted predicate itself --
produces nothing at all. Not an error, not a warning: an empty list and exit 0.

WHY THIS FILE CARRIES A POSITIVE CONTROL, AND WHY IT IS NOT `tags:`
-------------------------------------------------------------------
The failure mode here is ZERO EDGES WITH EXIT 0. A wrong key and a blind probe
produce a byte-identical result, so a leg that only counts `relatedTo` edges
cannot tell "the key is not read" from "my fixture never reached the extractor".
Every cell below therefore asserts a control in the SAME call.

`tags:` was the obvious control and it is the WRONG one on this path: it appears
only in `extract_markdown`'s docstring. No tag edge is ever emitted here -- the
frontmatter is stashed whole as `ontology_meta` instead. A control that reads
zero because IT is unimplemented would have made every cell unfalsifiable.

The controls actually used, both proven to fire in `test_zz_controls_have_a_red_state`:

  ontology_meta on the file node -- the frontmatter PARSED and the `ontology: true`
                                    gate at extractor.py:6552 OPENED.
  a heading `contains` edge      -- the body walker RAN over this file.

Run: python3 scripts/ast/test_frontmatter_relatedto.py   (or: pytest scripts/ast/)
"""

import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))


# ── fixtures ──────────────────────────────────────────────────────────────────

BODY = "\n# A Heading\n\nBody text.\n"

CELLS = {
    # name              key          form      frontmatter lines
    "relatedto_inline": ("relatedTo", "inline", "relatedTo: [alpha-entity, beta-entity]"),
    "relatedto_block":  ("relatedTo", "block",  "relatedTo:\n  - alpha-entity\n  - beta-entity"),
    "related_inline":   ("related",   "inline", "related: [alpha-entity, beta-entity]"),
    "related_block":    ("related",   "block",  "related:\n  - alpha-entity\n  - beta-entity"),
}


def _doc(fm_lines: str) -> str:
    return (
        "---\n"
        "ontology: true\n"
        "type: note\n"
        "tags: [alpha, beta]\n"
        f"{fm_lines}\n"
        "---\n" + BODY
    )


def _extract(text: str) -> dict:
    from extractor import extract_markdown

    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "fixture.md"
        p.write_text(text, encoding="utf-8")
        return extract_markdown(p)


def _controls(res: dict) -> tuple[bool, bool]:
    """(frontmatter parsed and gate opened, body walker ran)."""
    parsed = any("ontology_meta" in n for n in res.get("nodes", []))
    walked = any(e.get("relation") == "contains" for e in res.get("edges", []))
    return parsed, walked


def _related_targets(res: dict) -> list[str]:
    return sorted(
        e["target"] for e in res.get("edges", []) if e.get("relation") == "relatedTo"
    )


def _assert_cell(name: str) -> None:
    key, form, fm = CELLS[name]
    res = _extract(_doc(fm))
    parsed, walked = _controls(res)
    # The control is asserted BEFORE the count, so a zero below can only mean
    # absence. Without this an unreachable fixture reads exactly like the defect.
    assert parsed, (
        f"[{name}] CONTROL FAILED: no ontology_meta on any node -- the frontmatter "
        f"did not parse or the ontology gate never opened, so this leg proves NOTHING "
        f"about the {key} key"
    )
    assert walked, (
        f"[{name}] CONTROL FAILED: no 'contains' edge -- the body walker never ran "
        f"over the fixture, so this leg proves NOTHING about the {key} key"
    )
    targets = _related_targets(res)
    assert len(targets) == 2, (
        f"[{name}] {key}: {form} form produced {len(targets)} relatedTo edges, "
        f"expected 2 (controls both GREEN, so the extractor reached this file "
        f"and the frontmatter parsed -- this zero is the key, not the probe)"
    )


# ── the four cells ────────────────────────────────────────────────────────────


def test_relatedto_inline_emits_edges() -> None:
    """RED at 9f8c9b52: `relatedTo` is never read on this path (extractor.py:6635)."""
    _assert_cell("relatedto_inline")


def test_relatedto_block_emits_edges() -> None:
    """RED at 9f8c9b52: same key defect. yaml.safe_load parses the block fine."""
    _assert_cell("relatedto_block")


def test_related_inline_still_emits_edges() -> None:
    """NON-REGRESSION. Works today; a fix that only renames the key breaks it."""
    _assert_cell("related_inline")


def test_related_block_still_emits_edges() -> None:
    """NON-REGRESSION. Works today. The manual's :102 and :120 examples are this."""
    _assert_cell("related_block")


# ── canary and control red-state ──────────────────────────────────────────────


def test_unrelated_key_emits_no_related_edges() -> None:
    """MUST-FAIL CANARY: a plausible-but-wrong key must NOT be swept in.

    Without this, a fix that treats every key containing 'relat' as relatedTo
    would pass all four cells above while inventing edges from `unrelated:`.
    """
    res = _extract(_doc("unrelatedthings: [alpha-entity]"))
    parsed, walked = _controls(res)
    assert parsed and walked, (
        "CANARY CONTROL FAILED: fixture never reached the extractor, so a zero "
        "below would be blindness rather than correct silence"
    )
    targets = _related_targets(res)
    assert targets == [], (
        f"canary matched: 'unrelatedthings' produced {targets}; the key matcher "
        f"is too loose and every cell above is worthless"
    )


def test_bare_scalar_does_not_fabricate_one_edge_per_character() -> None:
    """A scalar `related:` used to emit one edge PER CHARACTER.

    Measured at 9f8c9b52: `related: alpha-entity` produced **12** edges with
    targets 'a', 'l', 'p', 'h', 'a', '' ... because the value is a str and the
    loop iterated it. That is fabricated relationship data, not a missing one,
    and it survives every count-based check that only asks whether edges exist.

    This arm is DISCLOSED SCOPE: the acceptance for #115 covers inline and block
    forms, not bare scalars. The key fix incidentally repaired this, so the arm
    exists to stop it regressing silently -- not to claim it was asked for.
    """
    res = _extract(_doc("related: alpha-entity"))
    parsed, walked = _controls(res)
    assert parsed and walked, "CONTROL FAILED: fixture never reached the extractor"
    targets = _related_targets(res)
    assert len(targets) == 1, (
        f"bare scalar produced {len(targets)} edges {targets[:6]}, expected exactly 1 "
        f"-- more than one means the string is being iterated character-wise again"
    )


def test_both_spellings_present_do_not_double_emit() -> None:
    """One relatedTo CONCEPT, so a document carrying both keys emits each once."""
    res = _extract(
        _doc("related: [alpha-entity]\nrelatedTo: [alpha-entity, beta-entity]")
    )
    parsed, walked = _controls(res)
    assert parsed and walked, "CONTROL FAILED: fixture never reached the extractor"
    targets = _related_targets(res)
    assert len(targets) == len(set(targets)), f"duplicate edges emitted: {targets}"
    assert len(targets) == 2, f"expected 2 distinct targets, got {len(targets)}: {targets}"


def test_zz_controls_have_a_red_state() -> None:
    """A control never seen red is decoration that happens to print PASS.

    Both controls are driven to FAIL here on inputs where they SHOULD fail, so a
    green control in the cells above is evidence rather than an assumption.
    """
    # No `ontology: true` -> the gate at extractor.py:6552 stays shut.
    no_gate = _extract("---\ntype: note\nrelated: [alpha-entity]\n---\n" + BODY)
    parsed, _ = _controls(no_gate)
    assert not parsed, (
        "the ontology_meta control did NOT go red on a document with no "
        "`ontology: true` -- it cannot distinguish a parsed gate from a shut one"
    )

    # No headings -> the body walker emits no `contains` edge.
    no_body = _extract("---\nontology: true\nrelated: [alpha-entity]\n---\n\nplain text\n")
    _, walked = _controls(no_body)
    assert not walked, (
        "the 'contains' control did NOT go red on a document with no headings -- "
        "it cannot distinguish a walked body from an unwalked one"
    )


if __name__ == "__main__":
    # Every leg runs even after one fails, and the exit code is the run's.
    # CI runs this file as plain `python <file>` with no pytest installed
    # (`ci.yml`, the `scripts/ast/test_*.py` step), so a bare `def test_` with
    # no driver here would be DEFINED, never CALLED, and the job would go green
    # having executed nothing.
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
