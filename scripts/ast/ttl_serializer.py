"""Convert extraction dicts ({nodes, edges}) to Turtle strings for ops:code graph."""

from collections import deque
from pathlib import Path

# One source of truth for "is a file" (#66). Import direction:
# onto_ast imports both; extractor imports neither of us, so this closes
# no cycle. extractor's module-level imports are stdlib only — every
# tree-sitter grammar loads lazily inside its handler.
from extractor import _DISPATCH

# One source of truth for "is a relation" (#107). `relations` is a leaf: it
# imports nothing from this package, so both we and `extractor` can depend on it.
from relations import RELATIONS, predicate as _relation_predicate

_CONFIG_PATH = Path.home() / ".open-ontologies" / "config.toml"
_ns_cache: dict[str, str] | None = None


def _load_namespaces() -> dict[str, str]:
    global _ns_cache
    if _ns_cache is not None:
        return _ns_cache
    defaults = {
        "ontology": "http://ops-sys.local/ontology#",
        "code": "http://ops-sys.local/code#",
    }
    if _CONFIG_PATH.exists():
        try:
            import tomllib
        except ModuleNotFoundError:
            import tomli as tomllib  # type: ignore[no-redef]
        try:
            with open(_CONFIG_PATH, "rb") as f:
                cfg = tomllib.load(f)
            if "namespaces" in cfg:
                defaults.update(cfg["namespaces"])
        except Exception:
            pass
    _ns_cache = defaults
    return _ns_cache


NOISE_PATH_SEGMENTS = frozenset({
    "node_modules", "vendor", ".git", "__pycache__", ".next", "dist",
    ".nuxt", ".output", "build", ".cache", ".tox", ".mypy_cache",
    ".pytest_cache", "coverage", ".nyc_output", "bower_components",
    ".yarn", ".pnpm", "target/debug", "target/release",
})


def is_noise_path(path: str) -> bool:
    for segment in NOISE_PATH_SEGMENTS:
        if f"/{segment}/" in path or path.endswith(f"/{segment}"):
            return True
    return False


def _build_prefixes() -> str:
    ns = _load_namespaces()
    return (
        f'@prefix ops: <{ns["ontology"]}> .\n'
        f'@prefix code: <{ns["code"]}> .\n'
        '@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n'
        '@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n'
        '@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n'
    )

TYPE_MAP = {
    "module": "Module",
    "class": "Class",
    "struct": "Struct",
    "function": "Function",
    "method": "Method",
    "import": "Import",
    "rationale": "Rationale",
}

# #107: this was a hand-kept table of 8, written 2026-06-01, while `extractor.py`
# grew to 34 language extractors emitting 27 relations. The 19 in the gap were
# discarded below by `RELATION_MAP.get` followed by a bare `continue` — no
# warning, no counter — so `ops:inherits` and `ops:extends` occur in no map on
# any tree, and class hierarchy is unanswerable in every language.
#
# The table now lives in `relations.py`, imported by the extractor as well, for
# the same reason `_FILE_EXTS` derives from `_DISPATCH` rather than agreeing with
# it: two lists that must match are two lists that will drift. `RELATION_MAP` is
# kept as a name because it is what every reader of this module already looks
# for; it IS `relations.RELATIONS`, not a copy of it.
#
# No `try/except ImportError` guard, matching `_parsed_extensions`: a degraded
# empty map would drop EVERY edge in every map, silently — the failure this
# exists to kill, made total. Fail loudly instead.
RELATION_MAP = RELATIONS

ONTOLOGY_SKIP_KEYS = frozenset({"ontology", "type", "tags", "related", "supersedes"})


def sanitize_iri(name: str) -> str:
    return (
        name.replace("-", "_")
        .replace(" ", "_")
        .replace(".", "_")
        .replace("/", "_")
        .replace("\\", "_")
        .replace(":", "_")
        .replace("(", "")
        .replace(")", "")
        .replace(",", "")
        .replace("'", "")
        .replace('"', "")
    )


def _escape_literal(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def _extract_line(node: dict) -> int:
    if "line" in node:
        return node["line"]
    loc = node.get("source_location", "")
    if loc and loc.startswith("L"):
        try:
            return int(loc[1:])
        except ValueError:
            pass
    return 0


def _parsed_extensions() -> frozenset[str]:
    """Every extension the extractor actually parses.

    #66: this was a hand-kept list of 27 while `_DISPATCH` grew to 66. The 39
    extensions in the gap (`.mjs`, `.ps1`, `.vue`, `.md`, `.json`, `.hpp`,
    `.kts`, every Fortran and Pascal spelling, ...) were parsed, reached the
    graph, and had every one of their entities attributed to the app root,
    because no file node was recognised for them and `_resolve_source_file`
    took its fallback. Deriving the set is the only way the two cannot drift
    again. No `try/except ImportError` guard: a degraded empty set would
    attribute EVERY entity to the app root, silently — the very failure this
    exists to kill, made total. Fail loudly instead.
    """
    return frozenset(_DISPATCH)


_FILE_EXTS = _parsed_extensions()


def _identify_file_nodes(
    node_labels: dict[str, str],
    file_map: dict[str, str] | None,
) -> set[str]:
    """Which node ids are file-level nodes.

    `file_map` (built in onto_ast.py under every id form a file node can carry)
    IS the definition of a file node in `--full` mode, so membership in it is
    exact and needs no extension list at all. The extension test is the
    fallback for single-file mode, where there is no `file_map`; it is a
    heuristic over free-text labels (a markdown heading ending in `.md` reads
    as a file) and is only ever consulted when the exact answer is unavailable.
    """
    if file_map:
        return {nid for nid in node_labels if nid in file_map}
    return {
        nid
        for nid, label in node_labels.items()
        if any(label.endswith(ext) for ext in _FILE_EXTS)
    }

# Extensions whose structs/records should be ops:Struct, not ops:Class
_STRUCT_LANGUAGES = frozenset({".rs", ".go", ".c", ".cpp", ".h", ".hpp", ".zig", ".cs"})


def _file_ext(label: str) -> str:
    """Extract the file extension from a label like 'auth.rs'."""
    dot = label.rfind(".")
    return label[dot:] if dot >= 0 else ""


def _build_role_map(
    edges: list[dict],
    nodes: list[dict],
    file_membership: dict[str, str],
    node_labels: dict[str, str],
    file_nodes_in: set[str],
) -> dict[str, str]:
    """Pre-process edges to infer node types from relationships.

    Returns a dict mapping node_id → inferred type string.
    Priority: file-level nodes → class/struct (has method edges) → method → rationale → function.
    Uses file_membership + node_labels to distinguish struct (Rust/Go/C) from class (Python/JS/etc).
    """
    roles: dict[str, str] = {}

    # Build sets of node IDs by relationship role
    has_method_children: set[str] = set()
    is_method: set[str] = set()
    is_rationale: set[str] = set()
    file_nodes: set[str] = set()

    # One identification, made once by the caller: role assignment and file
    # membership must never disagree about what a file is.
    for nid in file_nodes_in:
        file_nodes.add(nid)
        roles[nid] = "module"

    for edge in edges:
        rel = edge.get("relation", "")
        src = edge.get("source", "")
        tgt = edge.get("target", "")

        if rel == "method":
            has_method_children.add(src)
            is_method.add(tgt)
        elif rel == "rationale_for":
            is_rationale.add(src)

    # Gap 2 fix: distinguish struct vs class by containing file's language
    for nid in has_method_children:
        if nid in file_nodes:
            continue
        # Check the file this node belongs to
        file_node_id = file_membership.get(nid)
        file_label = node_labels.get(file_node_id, "") if file_node_id else ""
        ext = _file_ext(file_label)
        if ext in _STRUCT_LANGUAGES:
            roles[nid] = "struct"
        else:
            roles[nid] = "class"

    for nid in is_method:
        roles[nid] = "method"
    for nid in is_rationale:
        roles[nid] = "rationale"

    return roles


#: Relations where the TARGET physically lives inside the SOURCE's file.
#: `defines` is here on the evidence of its eight `add_edge` sites in
#: `extractor.py` — four file→symbol (`:2798`, `:2812`, `:2881`, `:3310`) and
#: four scope→symbol (`:1699`, `:3329`, `:3341`, `:3355`) — every one of them
#: containment. It was absent before #82 and cost the map every C++ struct
#: field: one hop from a resolved parent, dropped on the relation's NAME.
_CONTAINS_DOWNWARD: frozenset[str] = frozenset({"contains", "method", "defines"})

#: The one relation that runs the other way: a rationale node takes the file of
#: the thing it is a rationale FOR, so membership flows target → source.
_CONTAINS_UPWARD: frozenset[str] = frozenset({"rationale_for"})

#: Membership propagates along these and NOTHING else. #82 asks for "a
#: transitive walk to a fixed point rather than a fixed set of relations", and
#: the second half of that sentence is a worse bug than the one it fixes: an
#: unbounded walk follows `calls` and `inherits` and puts a symbol in whatever
#: file happens to refer to it. `RuntimeError` would land inside the user's
#: file that subclasses it, and the app-root counter would FALL — a false
#: attribution wearing a green number (#98). Transitive in depth, bounded in
#: relation. `scripts/ast/test_file_membership_walk.py` pins this set against
#: `relations._RELATION_NAMES` so a 29th relation cannot join the vocabulary
#: without someone stating which side of the split it belongs on.
CONTAINMENT_RELATIONS: frozenset[str] = _CONTAINS_DOWNWARD | _CONTAINS_UPWARD


def _build_file_membership(edges: list[dict], file_nodes: set[str]) -> dict[str, str]:
    """Map each node to its containing file, transitively, from the file nodes out.

    Breadth-first from every file node along `CONTAINMENT_RELATIONS`, so a node
    nested any number of hops deep resolves — a markdown `### heading` three
    `contains` edges below its file used to fall to the app root, and on a
    docs-heavy tree that was the majority of the map.

    The previous shape was three single passes, which made the answer depend on
    the order `edges` happened to arrive in: a class resolved later in the list
    than its own methods never propagated on that run. A fixed point does not
    care about order.

    Returns dict mapping node_id → file node_id.
    """
    down: dict[str, list[str]] = {}
    up: dict[str, list[str]] = {}
    for edge in edges:
        rel = edge.get("relation")
        if rel in _CONTAINS_DOWNWARD:
            down.setdefault(edge["source"], []).append(edge["target"])
        elif rel in _CONTAINS_UPWARD:
            up.setdefault(edge["target"], []).append(edge["source"])

    membership: dict[str, str] = {}
    queue: deque[str] = deque(file_nodes)
    while queue:
        nid = queue.popleft()
        owner = nid if nid in file_nodes else membership[nid]
        for neighbour in (*down.get(nid, ()), *up.get(nid, ())):
            # A node already placed is never rewritten, which is what makes a
            # containment cycle terminate rather than spin.
            if neighbour not in membership and neighbour not in file_nodes:
                membership[neighbour] = owner
                queue.append(neighbour)

    return membership


def _build_import_resolver(
    nodes: list[dict],
    project_clean: str,
) -> dict[str, str]:
    """Build a lookup from import target short names to actual node IRIs.

    Gap 1 fix: the extractor's cross-file resolver creates import edges with
    shortened target IDs (e.g., 'appconfig') that don't match the actual node
    IRIs (e.g., 'code:test_sample_src_config_appconfig'). This map resolves them.
    """
    # Map: lowercase label → full IRI (first match wins)
    resolver: dict[str, str] = {}
    for node in nodes:
        label = node.get("label", "")
        if not label:
            continue
        # Clean label: strip leading dots and parens (methods show as ".new()")
        clean = label.lstrip(".").rstrip("()").lower()
        if clean:
            iri = f"code:{project_clean}_{sanitize_iri(node['id'])}"
            if clean not in resolver:
                resolver[clean] = iri
    return resolver


def _resolve_source_file(
    node_id: str,
    file_membership: dict[str, str],
    node_labels: dict[str, str],
    file_map: dict[str, str] | None,
    fallback: str,
) -> str:
    """Resolve the sourceFile for a node, using file_map for relative paths (Gap 3)."""
    file_node_id = file_membership.get(node_id)
    if not file_node_id:
        return fallback
    # The file-node id is derived from the full path, so it is unique per file.
    # Try it FIRST: the bare-name key below collapses every same-named file in
    # the tree (`mod.rs`, `index.ts`, `__init__.py`) onto a single path, which
    # silently mis-attributes their entities to one arbitrary winner.
    if file_map and file_node_id in file_map:
        return file_map[file_node_id]
    bare_name = node_labels.get(file_node_id, "")
    if not bare_name:
        return fallback
    if file_map and bare_name in file_map:
        return file_map[bare_name]
    return bare_name


def serialize(
    extraction: dict,
    project: str,
    source_file: str,
    language: str,
    file_map: dict[str, str] | None = None,
    stats: dict | None = None,
) -> str:
    """Serialize extraction to Turtle/TTL.

    file_map: optional dict mapping bare filename → relative path (e.g., {"auth.rs": "src/auth.rs"}).
    Passed from onto_ast.py in --full mode for accurate per-node sourceFile values.
    """
    if is_noise_path(source_file):
        return ""

    nodes = extraction.get("nodes", [])
    edges = extraction.get("edges", [])

    # Build node label lookup
    node_labels = {n["id"]: n.get("label", "") for n in nodes}

    # Identify file-level nodes (exact via file_map, else by extension)
    file_node_ids = _identify_file_nodes(node_labels, file_map)

    # Build file membership (must come before role_map for Gap 2)
    file_membership = _build_file_membership(edges, file_node_ids)

    # Infer types (uses file_membership for struct vs class distinction)
    role_map = _build_role_map(edges, nodes, file_membership, node_labels, file_node_ids)

    # #66: entities with no file node fall back to the app root. Silent until
    # now, which is why 39 extensions drifted for months. The caller prints it.
    if stats is not None:
        orphans = sum(
            1
            for n in nodes
            if n["id"] not in file_node_ids and n["id"] not in file_membership
        )
        stats["app_root_entities"] = orphans
        stats["total_entities"] = len(nodes)
        stats["file_nodes"] = len(file_node_ids)

    # Build import resolver (Gap 1)
    project_clean = sanitize_iri(project)
    import_resolver = _build_import_resolver(nodes, project_clean)

    # Track all known node IRIs for import target validation
    known_iris: set[str] = set()
    for node in nodes:
        known_iris.add(f"code:{project_clean}_{sanitize_iri(node['id'])}")

    lines = [_build_prefixes()]

    module_iri = f"code:{project_clean}_{sanitize_iri(source_file)}"
    lines.append(f"{module_iri} a ops:Module ;")
    lines.append(f'    rdfs:label "{_escape_literal(source_file)}" ;')
    lines.append(f'    ops:sourceFile "{_escape_literal(source_file)}" ;')
    lines.append(f'    ops:language "{language}" .')
    lines.append("")

    for node in nodes:
        meta = node.get("ontology_meta")
        if meta and node.get("file_type") == "ontology_document":
            iri = f"code:{project_clean}_{sanitize_iri(node['id'])}"
            label = _escape_literal(node.get("label", node["id"]))
            onto_type = node.get("ontology_type", "Document")
            lines.append(f"{iri} a ops:{onto_type} ;")
            lines.append(f'    rdfs:label "{label}" ;')
            lines.append(f'    ops:sourceFile "{_escape_literal(source_file)}" ;')
            lines.append(f'    ops:language "{language}" ;')
            for key, val in meta.items():
                if key in ONTOLOGY_SKIP_KEYS:
                    continue
                if isinstance(val, bool):
                    lines.append(f'    ops:{key} {str(val).lower()} ;')
                elif isinstance(val, (int, float)):
                    lines.append(f'    ops:{key} {val} ;')
                elif isinstance(val, str):
                    lines.append(f'    ops:{key} "{_escape_literal(val)}" ;')
                elif isinstance(val, list):
                    for item in val:
                        lines.append(f'    ops:{key} "{_escape_literal(str(item))}" ;')
            for tag in meta.get("tags", []) or []:
                lines.append(f'    ops:tag "{_escape_literal(str(tag))}" ;')
            lines[-1] = lines[-1].rstrip(" ;") + " ."
            lines.append("")
            continue

        # Infer type: explicit node type > role map > fallback to function
        node_type = node.get("type")
        if not node_type or node_type not in TYPE_MAP:
            node_type = role_map.get(node["id"], "function")
        ops_type = TYPE_MAP.get(node_type, "Function")
        iri = f"code:{project_clean}_{sanitize_iri(node['id'])}"
        label = _escape_literal(node.get("label", node["id"]))
        line_num = _extract_line(node)
        signature = node.get("signature", "")

        # Gap 3: resolve per-node sourceFile with relative paths
        node_source = _resolve_source_file(
            node["id"], file_membership, node_labels, file_map, source_file
        )
        # For file-level Module nodes, also use relative path as label. A module
        # node's own id IS its file-node id, so it resolves uniquely — the bare
        # label is only a fallback (see _resolve_source_file).
        if node_type == "module" and file_map:
            rel = file_map.get(node["id"]) or file_map.get(node.get("label", ""))
            if rel:
                label = _escape_literal(rel)
                node_source = rel

        lines.append(f"{iri} a ops:{ops_type} ;")
        lines.append(f'    rdfs:label "{label}" ;')
        lines.append(f'    ops:sourceFile "{_escape_literal(node_source)}" ;')
        lines.append(f"    ops:sourceLine {line_num} ;")
        lines.append(f'    ops:language "{language}" ;')
        lines.append(f"    ops:definedIn {module_iri} ;")
        if signature:
            lines.append(f'    ops:signature "{_escape_literal(signature)}" ;')
        if node_type == "method" and node.get("parent"):
            parent_iri = f"code:{project_clean}_{sanitize_iri(node['parent'])}"
            lines.append(f"    ops:definedIn {parent_iri} ;")
        lines[-1] = lines[-1].rstrip(" ;") + " ."
        lines.append("")

    for edge in edges:
        relation = edge.get("relation", "calls")
        # #107: this was `RELATION_MAP.get(relation)` then `if not ops_rel:
        # continue` — the silent drop that lost 19 of the 27 relations the
        # extractor emits. An edge the extractor spent a parse building is
        # either worth a triple or worth a decision; it is never worth a
        # `continue`. `predicate` raises `UnknownRelation` naming the relation
        # and the file to add it to.
        ops_rel = _relation_predicate(relation)
        src_iri = f"code:{project_clean}_{sanitize_iri(edge['source'])}"
        tgt_iri = f"code:{project_clean}_{sanitize_iri(edge['target'])}"
        confidence = edge.get("confidence", "extracted").lower()

        # Gap 1: resolve phantom import targets to actual node IRIs
        if tgt_iri not in known_iris and ops_rel in ("importsFrom", "imports"):
            short_name = sanitize_iri(edge["target"]).lower()
            resolved = import_resolver.get(short_name)
            if resolved:
                tgt_iri = resolved
                confidence = "resolved"

        lines.append(f"{src_iri} ops:{ops_rel} {tgt_iri} .")
        lines.append(f'<<{src_iri} ops:{ops_rel} {tgt_iri}>> ops:confidence "{confidence}" .')
        lines.append("")

    return "\n".join(lines)
