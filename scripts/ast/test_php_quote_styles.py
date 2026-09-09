#!/usr/bin/env python3
r"""A PHP helper call in ANY plain quote style produces its edge (#118).

`config('a.b')` emitted a `uses_config` edge and `config("a.b")` emitted nothing, because
the argument test was `inner.type == "string"` and tree-sitter types a double-quoted PHP
literal `encapsed_string`. Double quotes are the more common Laravel spelling, so a real
Laravel map was missing the majority of its config edges with nothing errored and nothing
warned -- a smaller map that looks complete.

THE FILED CLAIM NAMED TWO NODE TYPES. THE INSTALLED GRAMMAR PRODUCES FOUR
-------------------------------------------------------------------------
Measured against tree-sitter-php 0.24.1 rather than read off the grammar docs:

    'a.b'      -> string             string_content
    "a.b"      -> encapsed_string    string_content
    <<<EOT     -> heredoc            value: heredoc_body -> string_content
    <<<'EOT'   -> nowdoc             value: nowdoc_body  -> nowdoc_string

Two things in that table are why a plausible fix still loses edges, and both are pinned
by `test_grammar_shape_is_what_the_fix_assumes` below. Heredoc and nowdoc keep their text
ONE LEVEL DEEPER, so widening the type test while still reading the node's own children
emits nothing for them -- a silent zero at exit 0. And a nowdoc's content node is
`nowdoc_string`, NOT `string_content`, so collecting only `string_content` reads an empty
key for every nowdoc.

Backticks are excluded. `` `a.b` `` is a `shell_command_expression`: shell execution, not
a literal. It carries a `string_content` child and NO interpolation children, so a
children-only test admits it as a config key. The type set is a whitelist and the
no-compile-time-value test is applied INSIDE it.

WHY SIX ARMS ASSERT A REPORT AND THREE ASSERT ITS ABSENCE
---------------------------------------------------------
An interpolating literal has no single compile-time value, so any target emitted for it is
invented -- the same defect class as #115's 12-edges-per-character, where a bare `str` was
iterated because nobody asked what the value was. `escape_sequence` is treated the same
way on purpose: the value exists but decoding it needs a PHP escape decoder whose rules
differ between single and double quotes, while both cheap readings fabricate. Joining the
siblings yields `ab` for `"a\tb"`. Taking the first sibling -- WHAT SHIPPED BEFORE THIS
CHANGE -- yielded `a` for `'a\'b.c'`, an edge to a target the source never named.

So those six arms expect ZERO EDGES, and zero is also what the unfixed extractor produced
for five of them, for an unrelated reason: it rejected `encapsed_string` wholesale, so
nothing tested interpolation at all. An arm whose expected value is identical before and
after the fix proves nothing by itself. **Each of those arms therefore asserts 0 edges AND
its own report line naming the file and the line**, which turns a silent zero into an
observed rejection. The three arms that must stay SILENT assert 0 edges AND report ABSENT,
which catches the opposite defect: a fix that reports on everything it cannot resolve.

WHY THE FIRST ARM IS A POSITIVE CONTROL AND NOT A COURTESY
----------------------------------------------------------
Nine of these arms assert "zero edges", and every one of them passes perfectly over a
fixture where no edge could ever have been emitted. That is not hypothetical: the first
version of this fixture put the endpoint in a SECOND FILE, and every arm read 0 edges
including the single-quote baseline that the filed evidence proves emits 1. `label_to_nid`
is built from the nodes of the CURRENT FILE ONLY (`extractor.py`, per-file call-graph
pass), so no key could ever resolve. A control proving the parser reached the file cannot
tell that apart from a broken matcher -- both read 0 at exit 0. So the emission path is
asserted FIRST and in the opposite direction, and the endpoint's presence in the map is
asserted with it (laws 24, 25, 48).

Run: python3 scripts/ast/test_php_quote_styles.py
"""

import contextlib
import io
import sys
import tempfile
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

from extractor import (  # noqa: E402
    _PHP_STRING_BODY_TYPES,
    _PHP_STRING_CONTENT_TYPES,
    _PHP_STRING_TYPES,
    _php_literal_key,
    extract,
)

# `class a` is the endpoint: `config('a.b')` splits on '.' and looks the first segment up
# in the per-file label map, where `class a` normalises to 'a'. It lives in the SAME FILE
# as the call for the reason given in the module docstring.
HEADER = [
    "<?php",
    "class a {",
    "    public function m() { return 1; }",
    "}",
    "function caller() {",
]
FOOTER = ["}"]
CALL_LINE = len(HEADER) + 1
PLAIN_KEY = "a.b"
REPORT_MARKER = "no uses_config edge"

PLAIN = {
    "single": r"""config('a.b')""",
    "double": r'''config("a.b")''',
    "heredoc": "config(<<<EOT\na.b\nEOT)",
    "nowdoc": "config(<<<'EOT'\na.b\nEOT)",
}
REJECTED = {
    "interp_var": (r'''config("a.$key")''', "interpolates"),
    "interp_brace": (r'''config("a.{$key}")''', "interpolates"),
    "interp_member": (r'''config("a.$o->k")''', "interpolates"),
    "esc_single_quote": (r"""config('a\'b.c')""", "escape sequence"),
    "esc_double_tab": (r'''config("a\tb.c")''', "escape sequence"),
    "esc_hex": (r'''config("a\x2eb")''', "escape sequence"),
}
SILENT = {
    "backtick": "config(`a.b`)",
    "concat": r"""config('a' . 'b')""",
    "non_helper": r'''other("a.b")''',
}


def _fixture_text(fragment):
    return "\n".join(HEADER + ["    " + fragment + ";"] + FOOTER) + "\n"


def _run(fragment):
    """One extraction over a fresh, uniquely-named directory, stderr captured.

    A shared cache root would let one arm serve another arm's answer: `extract()`
    consults a per-path cache and `.php` is not cache-bypassed. `parallel=False` keeps
    the extraction in-process so stderr is capturable at all.
    """
    root = Path(tempfile.mkdtemp(prefix="php118_")) / uuid.uuid4().hex[:8]
    root.mkdir(parents=True)
    text = _fixture_text(fragment)
    php = root / "caller.php"
    php.write_text(text, encoding="utf8")
    buf = io.StringIO()
    with contextlib.redirect_stderr(buf):
        result = extract([php], cache_root=root, parallel=False)
    edges = [e for e in result.get("edges", [])
             if str(e.get("relation", "")).startswith("uses_")]
    labels = {n["id"]: n.get("label") for n in result.get("nodes", [])}
    return result, edges, [labels.get(e["target"]) for e in edges], buf.getvalue()


def _reports(stderr_text, reason=None):
    """A targeted match on the report contract, never 'stderr is non-empty' -- the
    extractor may print unrelated warnings and an emptiness test would read those as a
    pass. A report names the file, the call line, the marker, and a reason."""
    out = []
    for line in stderr_text.splitlines():
        if "caller.php" not in line or ":%d" % CALL_LINE not in line:
            continue
        if REPORT_MARKER not in line:
            continue
        if reason is not None and reason not in line:
            continue
        out.append(line)
    return out


def test_aa_the_emission_path_can_fire_at_all():
    """POSITIVE CONTROL, asserted before anything counts a zero.

    Single quotes were never broken, so this arm is not about #118: it is the proof that
    a `uses_config` edge is emittable over this fixture. If it goes red every
    'zero edges' arm below is vacuous and this file says so instead of going green.
    """
    result, edges, targets, _err = _run(PLAIN["single"])
    labels = {n.get("label") for n in result["nodes"]}
    assert "a" in labels, (
        f"the endpoint `class a` is not in the map (labels={sorted(x for x in labels if x)}); "
        f"no key could resolve and every zero-edge arm in this file would be vacuous"
    )
    assert len(edges) == 1, f"single quotes emitted {len(edges)} uses_ edges, expected 1"
    assert targets == ["a"], f"edge target was {targets}, expected ['a']"


def test_grammar_shape_is_what_the_fix_assumes():
    """The fix's type sets, pinned against the INSTALLED grammar.

    If a grammar upgrade renames `encapsed_string`, moves a heredoc's body, or renames
    `nowdoc_string`, the whitelist silently stops matching and every rejection arm still
    passes -- for the wrong reason. This arm is the detector for that, and it fails
    loudly with the name that changed.
    """
    import tree_sitter_php
    from tree_sitter import Language, Parser

    parser = Parser(Language(tree_sitter_php.language_php()))
    seen = {}
    for name, frag in PLAIN.items():
        tree = parser.parse(_fixture_text(frag).encode("utf8"))
        arg = _find(tree.root_node, "argument")
        assert arg is not None, f"{name}: no `argument` node; the fixture stopped parsing"
        lit = [c for c in arg.children if c.is_named][0]
        seen[name] = lit.type
    assert seen == {"single": "string", "double": "encapsed_string",
                    "heredoc": "heredoc", "nowdoc": "nowdoc"}, (
        f"the installed grammar's node types moved: {seen}"
    )
    assert set(seen.values()) <= _PHP_STRING_TYPES, (
        f"a spelling the grammar produces is not in the whitelist: "
        f"{set(seen.values()) - _PHP_STRING_TYPES}"
    )
    # And the depth claim: heredoc/nowdoc content is NOT a direct child.
    tree = parser.parse(_fixture_text(PLAIN["nowdoc"]).encode("utf8"))
    lit = [c for c in _find(tree.root_node, "argument").children if c.is_named][0]
    body = lit.child_by_field_name("value")
    assert body is not None and body.type in _PHP_STRING_BODY_TYPES, (
        f"nowdoc body is {body.type if body else None}, not in {_PHP_STRING_BODY_TYPES}"
    )
    kinds = {c.type for c in body.children if c.is_named}
    assert kinds & _PHP_STRING_CONTENT_TYPES, (
        f"nowdoc body carries {kinds}, none of which is a known content type "
        f"{_PHP_STRING_CONTENT_TYPES}; a nowdoc key would read empty"
    )


def _find(node, type_name):
    """First node of a type in DOCUMENT order."""
    if node.type == type_name:
        return node
    for c in node.children:
        got = _find(c, type_name)
        if got is not None:
            return got
    return None


def test_every_plain_spelling_emits_exactly_one_edge_to_the_same_target():
    """The defect itself, and the three spellings the filed claim did not name."""
    for name, frag in PLAIN.items():
        _result, edges, targets, err = _run(frag)
        assert len(edges) == 1, f"{name}: {len(edges)} uses_ edges, expected 1"
        assert targets == ["a"], f"{name}: target {targets}, expected ['a']"
        assert edges[0]["relation"] == "uses_config", (
            f"{name}: relation {edges[0]['relation']!r}, expected 'uses_config'")
        assert not _reports(err), (
            f"{name}: a plain literal must not be reported; got {_reports(err)}")


def test_every_plain_spelling_yields_the_exact_key():
    """LEG 5, at the value layer, because the edge cannot show it.

    The edge target is the key's FIRST SEGMENT, so `a.b` and a truncation to `a` land on
    the same node and the edge layer cannot tell them apart. Matching a wider node type is
    easy; extracting the value correctly out of four different node shapes is the actual
    work, and an edge whose key is `"a.b"` with the quotes still attached would be a new
    defect wearing a passing test.
    """
    import tree_sitter_php
    from tree_sitter import Language, Parser

    parser = Parser(Language(tree_sitter_php.language_php()))
    for name, frag in PLAIN.items():
        src = _fixture_text(frag).encode("utf8")
        lit = [c for c in _find(parser.parse(src).root_node, "argument").children
               if c.is_named][0]
        key, reason = _php_literal_key(lit, src)
        assert reason is None, f"{name}: plain literal reported {reason!r}"
        assert key == PLAIN_KEY, f"{name}: key {key!r}, expected {PLAIN_KEY!r}"


def test_no_compile_time_value_emits_nothing_and_says_so():
    """Six arms: zero edges AND a report naming file and line.

    The report is what gives these arms a way to fail. Five of them read zero on the
    unfixed extractor too, because it rejected `encapsed_string` wholesale.
    """
    for name, (frag, reason) in REJECTED.items():
        _result, edges, targets, err = _run(frag)
        assert len(edges) == 0, (
            f"{name}: emitted {len(edges)} edge(s) to {targets} for an argument with no "
            f"compile-time value; any target here is invented")
        hits = _reports(err, reason)
        assert len(hits) == 1, (
            f"{name}: expected exactly 1 report line containing {reason!r}, "
            f"got {len(hits)}: {hits or err!r}")


def test_the_pre_existing_wrong_target_is_gone():
    """The one arm whose EDGE COUNT changes downward, called out on its own.

    `config('a\\'b.c')` was admitted before this change: type `string`, and the old
    break-after-first read stopped at the `escape_sequence` sibling and returned `a`. Its
    real key is `a'b.c`, whose first segment `a'b` resolves to nothing at all. So the
    extractor published an edge to `a` that the source never named. This lane DELETES a
    live fabricated edge; it does not decline a new feature.
    """
    _result, edges, targets, err = _run(REJECTED["esc_single_quote"][0])
    assert len(edges) == 0, (
        f"the escaped single-quoted key still emits {len(edges)} edge(s) to {targets}; "
        f"the truncation-to-'a' defect is back")
    assert _reports(err, "escape sequence"), "the rejection was silent"


def test_arguments_that_are_not_literals_stay_silent():
    """Three arms: zero edges AND report ABSENT.

    The ruling does not license reporting anything unresolvable. A backtick is shell
    execution, a concatenation is an expression, and a non-helper callee is none of this
    function's business. A fix that reported on all three would pass every arm above.
    """
    for name, frag in SILENT.items():
        _result, edges, targets, err = _run(frag)
        assert len(edges) == 0, f"{name}: emitted {len(edges)} edge(s) to {targets}"
        assert not _reports(err), (
            f"{name}: must stay silent, but reported {_reports(err)}")


def test_a_resolvable_key_with_no_target_is_not_a_report():
    """NEGATIVE CONTROL. `config('zzz.b')` reads its key perfectly; nothing in the file
    is named `zzz`. That is a missing TARGET, not an unknown VALUE, so it emits no edge
    and stays silent. Without this arm, a fix that reported every non-emitting call would
    look correct."""
    _result, edges, _targets, err = _run(r"""config('zzz.b')""")
    assert len(edges) == 0, f"an unresolvable key emitted {len(edges)} edge(s)"
    assert not _reports(err), f"a compile-time literal was reported: {_reports(err)}"


def test_the_report_matcher_has_a_red_state():
    """MUST-FAIL CANARY. Every assertion above about a report rests on `_reports`, so it
    is asserted to REJECT a line missing the marker and to reject a conforming line
    bearing the wrong line number. A matcher that accepts anything makes six arms
    vacuous."""
    conforming = ("base-ast: config() argument at caller.php:%d interpolates - %s "
                  "emitted\n" % (CALL_LINE, REPORT_MARKER))
    assert _reports(conforming, "interpolates"), "rejects a conforming line"
    assert not _reports("base-ast: caller.php:%d argument interpolates\n" % CALL_LINE), (
        "accepts a line with no marker")
    assert not _reports(conforming.replace(":%d" % CALL_LINE, ":%d" % (CALL_LINE + 40))), (
        "ignores the line number")


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
