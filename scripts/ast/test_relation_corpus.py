#!/usr/bin/env python3
"""#107 leg D: a corpus in which every relation the extractor can emit appears.

`tests/ast_extension_registry.rs` proves the vocabulary is COMPLETE (every name
the extractor can emit has a predicate) and that every mapped relation CAN reach
a triple, both from synthetic edge dicts and with no grammar installed. Neither
proves the extractor still produces the name a table claims it produces. This
does: real source, real tree-sitter parses, real serialization, and an assertion
that all 27 predicates appear in the emitted TTL.

It needs the tree-sitter grammars, so it does not ride `cargo test` today; #85
(the CI job that installs them) is the other lane's. Run it directly:

    python3 scripts/ast/test_relation_corpus.py

Exit codes are assigned, never inherited (law 27):
  0  every relation in the vocabulary reached the TTL
  1  one or more did not, named
  3  the corpus produced no edges at all -- proves nothing, never a pass
  4  could not run (missing grammar, import failure)
"""
from __future__ import annotations

import collections
import re
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

#: Relations no fixture can exercise, with the reason and the issue that tracks
#: it. An entry here is a KNOWN blind spot, not a waiver: the run fails if an
#: entry starts being emitted (the exclusion is stale and must go) exactly as it
#: fails when a relation outside this dict is missing. Law 31's corollary —
#: narrowing a claim documents the blind spot as accepted and pre-excuses the
#: next drift, so the narrowing has to be able to expire.
KNOWN_UNEXERCISABLE: dict[str, str] = {
    "instantiates": (
        "#120 — extract_verilog reads `module_declaration.child_by_field_name('name')`, "
        "which is None in tree-sitter-verilog 1.0.3 (the identifier is "
        "module_header > simple_identifier), so module_nid is never set and the "
        "`module_instantiation` branch that emits this relation is unreachable. "
        "A .v file yields only its file node. Remove this entry when #120 lands."
    ),
}

FIXTURES: dict[str, str] = {
    # ── _extract_generic, PHP: contains, method, calls, extends, implements,
    #    instantiates, bound_to, binds_method, listened_by, references_constant,
    #    uses_static_prop, uses_config ────────────────────────────────────────
    "app/Contracts.php": """<?php
namespace App;
interface Speaks { public function speak(); }
abstract class Animal {
    const NOISE = 'generic';
    public static $count = 0;
    public function noise() { return self::NOISE; }
}
class Dog extends Animal implements Speaks {
    public function speak() { return $this->noise(); }
    public function sniff() { return Animal::NOISE . Animal::$count; }
    public function make() { return new Animal(); }
    public function named() { return config('Animal.name'); }
}
""",
    "app/Provider.php": """<?php
namespace App;
class OrderPlaced { public function payload() { return 1; } }
class SendEmail { public function handle() { return 2; } }
class Contract { public function run() { return 3; } }
class Handler { public function run() { return 4; } }
class Provider {
    protected $listen = [
        OrderPlaced::class => [SendEmail::class],
    ];
    public function register() {
        $this->app->bind(Contract::class, Handler::class);
        $this->app->singleton(Contract::class, Handler::class);
    }
}
""",
    # ── extract_java, through _extract_generic and its one forwarder:
    #    extends and implements. Java-only spellings — every other language's
    #    base class arrives as `inherits`, and `implements` is passed through
    #    `_emit_java_parent`, so this fixture is the only thing in the corpus
    #    that can exercise the forwarder shape end to end. ───────────────────
    "src/Shapes.java": """package demo;

interface Drawable { void draw(); }
interface Sizable { int size(); }

abstract class Shape {
    void draw() { }
}

class Square extends Shape implements Drawable, Sizable {
    public void draw() { }
    public int size() { return 1; }
}
""",
    # ── extract_blade: includes, uses_component, binds_method ────────────────
    "resources/views/page.blade.php": """@include('partials.header')
<livewire:alert.box />
<button wire:click="save">Save</button>
<div>{{ $body }}</div>
@include('partials.footer')
""",
    "resources/views/partials/header.blade.php": "<header>h</header>\n",
    "resources/views/partials/footer.blade.php": "<footer>f</footer>\n",
    # ── python: contains, calls, method, imports, imports_from, inherits,
    #    rationale_for, and `uses` via cross-file import resolution ───────────
    "pkg/config.py": '''"""Config module."""


class AppConfig:
    """Holds settings."""

    def load(self):
        return 1


def helper():
    return 2
''',
    "pkg/service.py": '''"""Service module."""
import os
from pkg.config import AppConfig


class Service(AppConfig):
    def start(self):
        # WHY: the loader is called before anything else so a bad config fails
        # fast rather than half-way through startup.
        cfg = AppConfig()
        return cfg.load() + len(os.sep)
''',
    # ── javascript: imports, imports_from, re_exports, calls ─────────────────
    "web/lib.js": """export function make() { return 1; }
export const VALUE = 2;
""",
    "web/index.js": """import { make } from './lib.js';
export { VALUE } from './lib.js';
const other = require('./lib.js');
export function run() { return make() + other.VALUE; }
""",
    # ── svelte: dynamic_import ───────────────────────────────────────────────
    "web/App.svelte": """<script>
  export let name;
  function load() { return import('./lib.js'); }
</script>
<h1>{name}</h1>
""",
    # ── sql: reads_from, triggers, references, contains ──────────────────────
    "db/schema.sql": """CREATE TABLE customers (id INT PRIMARY KEY, name TEXT);
CREATE TABLE orders (
    id INT PRIMARY KEY,
    customer_id INT REFERENCES customers(id)
);
CREATE VIEW recent_orders AS SELECT id FROM orders;
CREATE TRIGGER touch_customer AFTER INSERT ON orders
    FOR EACH ROW EXECUTE FUNCTION bump_customer();
""",
    # ── verilog: instantiates, defines, contains ─────────────────────────────
    "hw/top.v": """`define WIDTH 8
module leaf(input wire clk, output wire q);
endmodule

module top(input wire clk, output wire q);
  wire mid;
  leaf u_leaf(.clk(clk), .q(mid));
endmodule
""",
    # ── bash: defines, calls, contains ───────────────────────────────────────
    "bin/run.sh": """#!/usr/bin/env bash
inner() { echo inner; }
outer() { inner; }
outer
""",
    # ── markdown with an ontology frontmatter: relatedTo, supersedes ─────────
    "docs/spec.md": """---
ontology: true
type: doc
related:
  - docs/other.md
supersedes: docs/old.md
---

# Spec

## Detail
""",
    "docs/other.md": "# Other\n",
    "docs/old.md": "# Old\n",
}


def build_corpus(root: Path) -> None:
    for rel, body in FIXTURES.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")


def main() -> int:
    try:
        from extractor import extract, collect_files, _make_id, _file_stem
        import relations
        import ttl_serializer
    except Exception as exc:  # law 24: absent tool, not a failing assertion
        print(f"ABORT[4]: cannot import the extractor: {exc!r}")
        return 4

    with tempfile.TemporaryDirectory(prefix="relation-corpus-") as tmp:
        root = Path(tmp)
        build_corpus(root)

        # Law 24: an absent grammar is an ABSENT TOOL, not a failing assertion.
        # `_extract_generic` RETURNS {"error": "tree_sitter_php not installed"}
        # with zero edges rather than raising, so without this the PHP fixture
        # contributes nothing, `uses_config` reports as "not emitted by the
        # corpus", and the blocking job added by #122 prints a VOCABULARY defect
        # for an ENVIRONMENT fault. That is the void FAIL, inside the instrument
        # whose entire subject is relations that disappear without saying so.
        #
        # The languages are DERIVED: every distinct suffix among FIXTURES is
        # dispatched through the extractor's own `_DISPATCH` and asked for its
        # own error string. A second hand-written list of grammar names would
        # drift from the fixtures exactly the way RELATION_MAP drifted from the
        # extractor.
        from extractor import _DISPATCH

        unusable: dict[str, str] = {}
        seen: set[str] = set()
        for rel in sorted(FIXTURES):
            suffix = Path(rel).suffix.lower()
            if suffix in seen:
                continue
            seen.add(suffix)
            handler = _DISPATCH.get(suffix)
            if handler is None:
                continue
            try:
                probe = handler(root / rel)
            except Exception as exc:
                unusable[suffix] = repr(exc)
                continue
            if isinstance(probe, dict) and probe.get("error"):
                unusable[suffix] = str(probe["error"])
        if unusable:
            print("ABORT[4]: the corpus cannot parse every language it fixtures.")
            for suffix, why in sorted(unusable.items()):
                print(f"  {suffix}  {why}")
            print("  This is an ABSENT GRAMMAR, not a missing relation. No row")
            print("  below would mean what it says. Install the grammars first:")
            print("  python -m pip install -r scripts/ast/requirements.txt")
            return 4
        print(f"grammars ok for {len(seen)} fixture suffixes")

        try:
            files = collect_files(root)
            result = extract(files, cache_root=root)
        except Exception as exc:
            print(f"ABORT[4]: extraction failed: {exc!r}")
            return 4

        edges = result.get("edges", [])
        print(f"corpus files {len(files)}  nodes {len(result.get('nodes', []))}  edges {len(edges)}")
        if not edges:
            print("ABORT[3]: the corpus produced no edges -- this proved NOTHING")
            return 3

        file_map = {}
        for f in files:
            try:
                rel = str(f.relative_to(root))
            except ValueError:
                rel = f.name
            file_map[_make_id(rel)] = rel
            file_map[_make_id(str(f))] = rel
            file_map[_make_id(f"{_file_stem(f)}{f.suffix}")] = rel
            file_map.setdefault(f.name, rel)
        try:
            ttl = ttl_serializer.serialize(result, "corpus", str(root), "multi",
                                           file_map=file_map)
        except Exception as exc:
            print(f"ABORT[4]: serialize failed: {exc!r}")
            return 4

    extracted = collections.Counter(e.get("relation", "calls") for e in edges)
    emitted = collections.Counter(
        m.group(1)
        for m in re.finditer(r"^code:\S+ ops:(\w+) code:\S+ \.$", ttl, re.M)
    )

    print(f"\n{'relation':24s} {'extracted':>9s} {'ops:predicate':24s} {'in ttl':>7s}")
    missing_from_corpus: list[str] = []
    missing_from_ttl: list[str] = []
    stale_exclusions: list[str] = []
    for name, pred in sorted(relations.RELATIONS.items()):
        got, out = extracted.get(name, 0), emitted.get(pred, 0)
        note = "  (known unexercisable)" if name in KNOWN_UNEXERCISABLE else ""
        print(f"{name:24s} {got:9d} ops:{pred:<20s} {out:7d}{note}")
        if got == 0:
            if name not in KNOWN_UNEXERCISABLE:
                missing_from_corpus.append(name)
        else:
            if name in KNOWN_UNEXERCISABLE:
                stale_exclusions.append(name)
            if out == 0:
                missing_from_ttl.append(name)

    unknown = sorted(set(extracted) - set(relations.RELATIONS))
    print(f"\nchecked {len(relations.RELATIONS)} relations over {len(edges)} edges")
    print(f"exercised by the corpus: {len(relations.RELATIONS) - len(missing_from_corpus) - len(KNOWN_UNEXERCISABLE)}")
    print(f"known unexercisable: {len(KNOWN_UNEXERCISABLE)} {sorted(KNOWN_UNEXERCISABLE)}")
    print(f"not emitted by the corpus: {len(missing_from_corpus)} {missing_from_corpus}")
    print(f"emitted but absent from the ttl: {len(missing_from_ttl)} {missing_from_ttl}")
    print(f"extracted but not in the vocabulary: {len(unknown)} {unknown}")
    print(f"stale exclusions: {len(stale_exclusions)} {stale_exclusions}")

    ok = True
    if unknown:
        print("\nFAIL: the extractor emitted a relation the vocabulary does not carry.")
        ok = False
    if missing_from_ttl:
        print("\nFAIL: relations reached the extraction and were dropped before the TTL.")
        ok = False
    if missing_from_corpus:
        print("\nFAIL: the corpus does not exercise every relation, so this run cannot")
        print("      tell a dropped relation from an unexercised one. Extend FIXTURES,")
        print("      or add a KNOWN_UNEXERCISABLE entry naming the defect that blocks it.")
        ok = False
    if stale_exclusions:
        print("\nFAIL: a KNOWN_UNEXERCISABLE relation is now being emitted. The blind")
        print("      spot closed; delete the entry so the corpus gates it again:")
        for name in stale_exclusions:
            print(f"        {name}: {KNOWN_UNEXERCISABLE[name]}")
        ok = False
    print("\nRESULT: " + ("PASS" if ok else "FAIL"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
