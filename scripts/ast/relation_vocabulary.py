"""The set of relation names `extractor.py` can emit, derived from its own source.

This is the instrument behind the #107 contract test. It exists because the answer
cannot be grepped: a relation name reaches an edge through five different syntactic
shapes, and the two that are not string literals in an `add_edge(...)` call are
exactly the ones a literal scan reports as absent.

The shapes, enumerated FROM `extractor.py` rather than from this module's own
assumptions (law 31 — a guard is only proven against the codebase):

  1. a literal in a positional or keyword argument of `add_edge` / `_add_edge`
     — two helper names, 21 nested definitions between them; a scan for
       `add_edge(` alone misses `reads_from` and `triggers` in `extract_sql`.
  2. a literal under a `"relation"` key in an edge dict built inline
  3. a literal passed to a FORWARDER — a function that takes a relation as a
     parameter and hands it to an edge helper. `_emit_java_parent:1517` is the
     only one, and `implements` is spelled at NO other site in the file, so a
     census that skips this shape reports 25 relations where there are 26.
  4. a conditional expression with constant arms, e.g.
     `"method" if container != parent_nid else "contains"`
  5. a local name bound to constants — including an attribute read off a
     dataclass whose constructor sites all pass a constant
     (`use_fact.relation`, always `"calls"`)
  6. an f-string over a name the enclosing code has already constrained to a
     closed frozenset: `relation = f"uses_{callee_name}"` at `:1956`, reachable
     only under `if callee_name in config.helper_fn_names`, and
     `helper_fn_names` is declared once as `frozenset({"config"})` at `:1208`.
     The one relation this yields, `uses_config`, is spelled as a literal
     NOWHERE in the file — every literal census run against this issue,
     including the one in #107 itself, reported 25 or 26 relations because of
     this single site. Resolving it means reading the guard, so that widening
     `helper_fn_names` widens the vocabulary and fails this test until the
     table is updated, rather than silently dropping a new relation.

Anything the resolver cannot reduce to constants is returned in `unresolved`, and
the caller is expected to FAIL on a non-empty list rather than report a smaller
vocabulary (law 23: a check that visited fewer sites than it thinks proved less
than it claims).
"""
from __future__ import annotations

import ast
from dataclasses import dataclass, field
from pathlib import Path

EDGE_FNS = frozenset({"add_edge", "_add_edge"})


@dataclass
class _Scope:
    """One top-level function, with what its own body constrains."""

    start: int
    end: int
    node: ast.AST
    #: names bound only to string constants inside this function
    bindings: dict[str, set[str]]


@dataclass
class Vocabulary:
    """What the resolver found in one source file."""

    relations: set[str] = field(default_factory=set)
    #: `file:line  <expression>` for every relation slot that did not reduce to
    #: constants. A non-empty list means the census is incomplete.
    unresolved: list[str] = field(default_factory=list)
    #: how many of each shape were visited, so a shape that silently stops
    #: matching shows up as a zero instead of as a smaller vocabulary.
    shape_counts: dict[str, int] = field(default_factory=dict)


def _const_str(node: ast.AST) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return None


def _relation_arg(call: ast.Call, index: int) -> ast.AST | None:
    """The expression in a call's relation slot: keyword first, then position."""
    for kw in call.keywords:
        if kw.arg == "relation":
            return kw.value
    if len(call.args) > index:
        return call.args[index]
    return None


def _forwarders(tree: ast.AST) -> dict[str, int]:
    """Functions that pass one of their own parameters into an edge helper's
    relation slot, mapped to that parameter's index (shape 3)."""
    found: dict[str, int] = {}
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if node.name in EDGE_FNS:
            continue
        params = [a.arg for a in node.args.args]
        for call in ast.walk(node):
            if not (isinstance(call, ast.Call) and isinstance(call.func, ast.Name)):
                continue
            if call.func.id not in EDGE_FNS:
                continue
            slot = _relation_arg(call, 2)
            if isinstance(slot, ast.Name) and slot.id in params:
                found[node.name] = params.index(slot.id)
    return found


def _constant_bindings(scope: ast.AST) -> dict[str, set[str]]:
    """Names in ONE function scope that are only ever bound to string constants,
    or to conditionals over them (shape 5, first half).

    Scoped per function deliberately. File-wide, `relation = f"uses_{callee}"`
    in `_extract_generic` poisons the name `relation` for `extract_pascal`,
    which binds it only to constants — reporting an unresolved site in a
    function that has none.
    """
    bindings: dict[str, set[str]] = {}
    poisoned: set[str] = set()
    for node in ast.walk(scope):
        targets: list[ast.AST] = []
        value: ast.AST | None = None
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target = node.targets[0]
            if isinstance(target, ast.Tuple) and isinstance(node.value, ast.Tuple) \
                    and len(target.elts) == len(node.value.elts):
                # `container, relation = module_nid, "contains"`
                for elt, val in zip(target.elts, node.value.elts):
                    if isinstance(elt, ast.Name):
                        consts = _constants_of(val)
                        if consts is None:
                            poisoned.add(elt.id)
                        else:
                            bindings.setdefault(elt.id, set()).update(consts)
                continue
            targets = [target]
            value = node.value
        if not targets or not isinstance(targets[0], ast.Name):
            continue
        values = _constants_of(value)
        if values is None:
            poisoned.add(targets[0].id)
        else:
            bindings.setdefault(targets[0].id, set()).update(values)
    for name in poisoned:
        bindings.pop(name, None)
    return bindings


def _closed_set_fields(tree: ast.AST) -> dict[str, set[str]]:
    """Every `<field>=frozenset({...})` keyword in the file, by field name, when
    every one of its members is a string constant (shape 6's other half)."""
    out: dict[str, set[str]] = {}
    blocked: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        for kw in node.keywords:
            if kw.arg is None:
                continue
            call = kw.value
            if not (isinstance(call, ast.Call) and isinstance(call.func, ast.Name)
                    and call.func.id == "frozenset" and len(call.args) == 1):
                continue
            members = call.args[0]
            if not isinstance(members, (ast.Set, ast.List, ast.Tuple)):
                continue
            values = {_const_str(e) for e in members.elts}
            if None in values:
                blocked.add(kw.arg)
                continue
            out.setdefault(kw.arg, set()).update(v for v in values if v)
    for name in blocked:
        out.pop(name, None)
    return out


def _dominating_guards(scope: ast.AST, target: ast.AST) -> dict[str, set[str]]:
    """Names constrained by `<name> in <obj>.<field>` in an `if` that ENCLOSES
    `target`, mapped to the field names those tests reference.

    Dominance, not scope membership. `_extract_generic` tests the same
    `callee_name` against `config.helper_fn_names` at :1935 and against
    `config.container_bind_methods` at :1975; a scope-wide scrape picks up both
    and expands `f"uses_{callee_name}"` into four relations the code cannot
    emit. Only the branch the assignment actually sits inside may constrain it.
    """
    parents: dict[int, ast.AST] = {}
    for node in ast.walk(scope):
        for child in ast.iter_child_nodes(node):
            parents[id(child)] = node

    out: dict[str, set[str]] = {}
    node: ast.AST | None = target
    seen: set[int] = set()
    while node is not None and id(node) not in seen:
        seen.add(id(node))
        parent = parents.get(id(node))
        if isinstance(parent, ast.If) and any(node is stmt for stmt in parent.body):
            for test in ast.walk(parent.test):
                if not (isinstance(test, ast.Compare) and len(test.ops) == 1):
                    continue
                if not isinstance(test.ops[0], ast.In):
                    continue
                left, right = test.left, test.comparators[0]
                if isinstance(left, ast.Name) and isinstance(right, ast.Attribute):
                    out.setdefault(left.id, set()).add(right.attr)
        node = parent
    return out


def _dataclass_field_constants(tree: ast.AST, attr: str) -> dict[str, set[str]]:
    """For `obj.<attr>`, the constants every constructor of a class carrying that
    field passes into it (shape 5, second half).

    Keyed by class name. A class whose field is fed anything non-constant, or
    which is constructed with keywords this reader does not model, is omitted —
    the caller then reports the site as unresolved rather than guessing.
    """
    field_index: dict[str, int] = {}
    for node in ast.walk(tree):
        if not isinstance(node, ast.ClassDef):
            continue
        names = [
            stmt.target.id
            for stmt in node.body
            if isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name)
        ]
        if attr in names:
            field_index[node.name] = names.index(attr)

    out: dict[str, set[str]] = {}
    for cls, index in field_index.items():
        values: set[str] = set()
        ok = True
        seen = 0
        for node in ast.walk(tree):
            if not (isinstance(node, ast.Call) and isinstance(node.func, ast.Name)):
                continue
            if node.func.id != cls:
                continue
            seen += 1
            slot = None
            for kw in node.keywords:
                if kw.arg == attr:
                    slot = kw.value
            if slot is None and len(node.args) > index:
                slot = node.args[index]
            consts = _constants_of(slot) if slot is not None else None
            if not consts:
                ok = False
                break
            values.update(consts)
        if ok and seen and values:
            out[cls] = values
    return out


def _constants_of(node: ast.AST | None) -> set[str] | None:
    """Every string constant an expression can evaluate to, or None if it can
    evaluate to something this reader cannot see."""
    if node is None:
        return None
    literal = _const_str(node)
    if literal is not None:
        return {literal}
    if isinstance(node, ast.IfExp):  # shape 4
        body = _constants_of(node.body)
        orelse = _constants_of(node.orelse)
        if body is None or orelse is None:
            return None
        return body | orelse
    return None


def extractor_relations(source_path: Path | str) -> Vocabulary:
    """Every relation name `extractor.py` can put on an edge."""
    path = Path(source_path)
    tree = ast.parse(path.read_text(encoding="utf-8"))
    vocab = Vocabulary()
    counts = vocab.shape_counts

    forwarders = _forwarders(tree)
    closed_sets = _closed_set_fields(tree)
    # One binding table and one guard table per top-level function, so a name
    # bound to an f-string in one extractor cannot poison the same name in
    # another (see `_constant_bindings`).
    scopes: list[_Scope] = [
        _Scope(top.lineno, getattr(top, "end_lineno", top.lineno), top,
               _constant_bindings(top))
        for top in tree.body
        if isinstance(top, (ast.FunctionDef, ast.AsyncFunctionDef))
    ]

    def scope_of(lineno: int) -> _Scope | None:
        for scope in scopes:
            if scope.start <= lineno <= scope.end:
                return scope
        return None

    # An edge helper's own body reads its `relation` PARAMETER, and a forwarder's
    # body passes its own on. Neither is a relation name; both are answered by the
    # call sites outside. Skipping them here rather than filtering afterwards keeps
    # the shape counts honest — a count that includes a site whose value came from
    # somewhere else is law 23's problem wearing a smaller hat.
    helper_ranges = [
        (n.lineno, getattr(n, "end_lineno", n.lineno))
        for n in ast.walk(tree)
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))
        and (n.name in EDGE_FNS or n.name in forwarders)
    ]

    def inside_helper(lineno: int) -> bool:
        return any(start <= lineno <= end for start, end in helper_ranges)

    counts["edge_helper_defs"] = sum(
        1
        for n in ast.walk(tree)
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.name in EDGE_FNS
    )
    counts["forwarders"] = len(forwarders)

    def record(shape: str, node: ast.AST, slot: ast.AST | None) -> None:
        if inside_helper(getattr(node, "lineno", 0)):
            counts["skipped_helper_bodies"] = counts.get("skipped_helper_bodies", 0) + 1
            return
        counts[shape] = counts.get(shape, 0) + 1
        scope = scope_of(getattr(node, "lineno", 0))
        bindings = scope.bindings if scope else {}
        values = _constants_of(slot)
        if values is not None:
            vocab.relations.update(values)
            return
        if isinstance(slot, ast.Name) and slot.id in bindings:
            vocab.relations.update(bindings[slot.id])
            counts["via_binding"] = counts.get("via_binding", 0) + 1
            return
        if isinstance(slot, ast.Name) and scope is not None:
            expanded = _expand_fstring_binding(slot.id, scope, closed_sets)
            if expanded:
                vocab.relations.update(expanded)
                counts["via_closed_set"] = counts.get("via_closed_set", 0) + 1
                return
        if isinstance(slot, ast.Attribute):
            per_class = _dataclass_field_constants(tree, slot.attr)
            if len(per_class) == 1:
                vocab.relations.update(next(iter(per_class.values())))
                counts["via_dataclass"] = counts.get("via_dataclass", 0) + 1
                return
        expr = ast.dump(slot)[:70] if slot is not None else "(no relation argument)"
        vocab.unresolved.append(f"{path.name}:{getattr(node, 'lineno', 0)}  {expr}")

    for node in ast.walk(tree):
        # shapes 1 and 3
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
            if node.func.id in EDGE_FNS:
                record("edge_helper_calls", node, _relation_arg(node, 2))
            elif node.func.id in forwarders:
                index = forwarders[node.func.id]
                slot = node.args[index] if len(node.args) > index else None
                record("forwarder_calls", node, slot)
        # shape 2
        elif isinstance(node, ast.Dict):
            for key, value in zip(node.keys, node.values):
                if isinstance(key, ast.Constant) and key.value == "relation":
                    record("edge_dicts", node, value)

    return vocab


def _expand_fstring_binding(
    name: str,
    scope: "_Scope",
    closed_sets: dict[str, set[str]],
) -> set[str]:
    """Shape 6: a name bound to an f-string over other names the same scope has
    constrained to a closed frozenset.

    Returns the full cross-product, or an empty set when any part of the chain
    is open — an empty return means "unresolved", never "no relations".
    """
    out: set[str] = set()
    for node in ast.walk(scope.node):
        if not (isinstance(node, ast.Assign) and len(node.targets) == 1):
            continue
        target = node.targets[0]
        if not (isinstance(target, ast.Name) and target.id == name):
            continue
        if not isinstance(node.value, ast.JoinedStr):
            continue
        pieces: list[set[str]] = []
        for part in node.value.values:
            literal = _const_str(part)
            if literal is not None:
                pieces.append({literal})
                continue
            if not (isinstance(part, ast.FormattedValue)
                    and isinstance(part.value, ast.Name)):
                return set()
            guards = _dominating_guards(scope.node, node)
            fields = guards.get(part.value.id) or set()
            if len(fields) != 1:
                # No dominating guard, or more than one — the name is not
                # provably closed at this site, so the whole binding is
                # unresolved rather than partially guessed.
                return set()
            members = closed_sets.get(next(iter(fields)))
            if not members:
                return set()
            pieces.append(set(members))
        combos = {""}
        for piece in pieces:
            combos = {prefix + suffix for prefix in combos for suffix in piece}
        out.update(combos)
    return out
