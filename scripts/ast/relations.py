"""The relation vocabulary shared by the extractor and the serializer.

#107: `RELATION_MAP` lived in `ttl_serializer.py` with 8 entries, written
2026-06-01. `extractor.py` grew to 34 language extractors emitting **27**
relations, and every edge whose relation was not one of the 8 was discarded at
the last step by `RELATION_MAP.get(relation)` followed by a bare `continue` —
no warning, no counter. The edge was built, carried through every merge and
resolution pass, and dropped where nobody was looking: 19 of the 27 relations
appear in **no map on any tree**, including `inherits` and `extends`, which are
the class-hierarchy relations for every object-oriented language the extractor
supports.

This module is the contract those two files were implying and never wrote down.
Both import it; neither owns it. `ttl_serializer` already imports `_DISPATCH`
from `extractor`, so a table living in either file would run the dependency the
wrong way for one of them — and the deeper reason for a third module is that the
producer never reads predicates and the consumer never invents names, so neither
is the natural owner of the pairing.

The failure this exists to kill is the same one `_parsed_extensions()` (#66) and
`_build_lang_map()` (#83) were restructured to kill, one layer down: **a silent
degraded default**. An unknown relation now raises where it used to `continue`.

### Adding a relation

Add its **name** to `_RELATION_NAMES`. The `ops:` predicate is derived
(`snake_case` -> `lowerCamelCase`) unless the name is in `_PREDICATE_OVERRIDES`,
so a new relation needs one line, not two, and cannot be given a predicate that
disagrees with its name by accident.

`tests/ast_extension_registry.rs` fails the build if the extractor can emit a
name this list does not carry. That check reads `extractor.py`'s own AST through
`relation_vocabulary.py`, because four of the names — `implements`,
`uses_config`, and the two conditional ones — are not spelled as string literals
at any `add_edge` call site and no grep can find them.

### Why `uses_config` is enumerated and there is no `uses_*` pattern rule

`uses_config` is not written anywhere. `extractor.py:1956` builds it with
`relation = f"uses_{callee_name}"`, under a guard at `:1935` that admits only
names in `config.helper_fn_names`, declared 750 lines earlier at `:1208` as
`frozenset({"config"})`. A computed name turns the raise above into a hazard: if
the vocabulary did not carry it, `base sync --ast` would **hard-fail on the
first PHP tree containing `config('x.y')`** — the silent-to-loud regression this
change exists to prevent, shipped inside the change that prevents it.

Two ways to close that. **Enumerating the one name it can currently produce is
the choice made here**, and a `uses_*` pattern rule is rejected: a pattern would
accept `uses_` plus anything, which hands the whole family back the silent
default — a typo in a future relation name would quietly acquire a predicate
instead of failing. Enumeration is only safe because the widening is caught
before a user meets it, and it is caught three different ways:

- add `"route"` to `helper_fn_names` -> the resolver expands the guard and
  reports `uses_route` -> `every_emittable_relation_has_a_predicate` fails with
  `missing 1 ['uses_route']`, on the PR that added it.
- point `helper_fn_names` at something the resolver cannot read (a module
  constant, a computed set) -> the site resolves to nothing, lands in
  `unresolved`, and the same test fails asking for the new shape to be taught.
- delete the guard entirely -> `callee_name` is unbounded, no dominating guard
  is found, and the site is `unresolved` again.

All three are build-time failures with the file to edit named in the message.
None can reach a user. `scripts/ast/test_relation_corpus.py` carries the PHP
fixture that actually emits `uses_config`, so the enumerated name is proven by a
parse rather than by this paragraph.
"""
from __future__ import annotations

#: Every relation name `extractor.py` can put on an edge. Pinned to the
#: extractor's own source by the contract test; see the module docstring.
_RELATION_NAMES: frozenset[str] = frozenset({
    "binds_method",
    "bound_to",
    "calls",
    "contains",
    "defines",
    "dynamic_import",
    "extends",
    "implements",
    "imports",
    "imports_from",
    "includes",
    "inherits",
    "instantiates",
    "listened_by",
    "method",
    "rationale_for",
    "re_exports",
    "reads_from",
    "references",
    "references_constant",
    "relatedTo",
    "supersedes",
    "triggers",
    "uses",
    "uses_component",
    "uses_config",
    "uses_static_prop",
})

#: Predicates that are not the derived form of their relation name. Only for
#: names whose predicate is already in shipped maps and must not move: renaming
#: one rewrites every map on every machine at the next sync.
_PREDICATE_OVERRIDES: dict[str, str] = {
    "method": "hasMethod",
}


def _derive_predicate(name: str) -> str:
    """`imports_from` -> `importsFrom`. Names already in camelCase pass through."""
    head, *rest = name.split("_")
    return head + "".join(part[:1].upper() + part[1:] for part in rest)


RELATIONS: dict[str, str] = {
    name: _PREDICATE_OVERRIDES.get(name, _derive_predicate(name))
    for name in sorted(_RELATION_NAMES)
}


class UnknownRelation(RuntimeError):
    """An edge carried a relation with no `ops:` predicate.

    Raised rather than skipped. A relation the extractor spent a parse building
    is either worth a triple or worth a decision — silently dropping it is how
    `inherits` reached zero occurrences in every map ever built.
    """


def predicate(relation: str) -> str:
    """The `ops:` predicate for an extractor relation name."""
    try:
        return RELATIONS[relation]
    except KeyError:
        raise UnknownRelation(
            f"relation {relation!r} is emitted by the extractor but has no ops: "
            f"predicate. Add it to _RELATION_NAMES in scripts/ast/relations.py. "
            f"Known ({len(RELATIONS)}): {', '.join(sorted(RELATIONS))}"
        ) from None


def assert_known(relations: object, where: str = "") -> None:
    """Raise `UnknownRelation` naming every unknown relation in an iterable.

    Used at `extract()`'s merge seam so the error names the source file the edge
    came from, which the serializer cannot: by the time an edge reaches
    serialization the per-file context is gone.
    """
    unknown = sorted({r for r in relations if r not in RELATIONS})  # type: ignore[union-attr]
    if not unknown:
        return
    site = f" ({where})" if where else ""
    raise UnknownRelation(
        f"{len(unknown)} relation(s) with no ops: predicate{site}: "
        f"{', '.join(unknown)}. Add them to _RELATION_NAMES in "
        f"scripts/ast/relations.py."
    )
