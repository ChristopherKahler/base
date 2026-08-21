"""Convert extraction dicts ({nodes, edges}) to Turtle strings for ops:code graph."""

from pathlib import Path

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

RELATION_MAP = {
    "calls": "calls",
    "imports": "imports",
    "imports_from": "importsFrom",
    "contains": "contains",
    "method": "hasMethod",
    "rationale_for": "rationaleFor",
    "relatedTo": "relatedTo",
    "supersedes": "supersedes",
}

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


_FILE_EXTS = frozenset({
    ".rs", ".py", ".js", ".ts", ".tsx", ".jsx", ".go", ".java", ".rb",
    ".cpp", ".c", ".h", ".cs", ".php", ".swift", ".kt", ".scala", ".ex",
    ".exs", ".lua", ".jl", ".zig", ".sh", ".sql", ".dart", ".svelte", ".astro",
})

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

    for nid, label in node_labels.items():
        if any(label.endswith(ext) for ext in _FILE_EXTS):
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


def _build_file_membership(edges: list[dict], file_nodes: set[str]) -> dict[str, str]:
    """Map each node to its containing file by walking edges from file-level nodes.

    Returns dict mapping node_id → file node_id.
    """
    membership: dict[str, str] = {}

    # Direct containment: file → entity
    for edge in edges:
        if edge.get("relation") == "contains" and edge["source"] in file_nodes:
            membership[edge["target"]] = edge["source"]

    # Method containment: class → method (walk up to file via class)
    for edge in edges:
        if edge.get("relation") == "method":
            cls_id = edge["source"]
            method_id = edge["target"]
            if cls_id in membership and method_id not in membership:
                membership[method_id] = membership[cls_id]

    # Rationale containment: rationale → target (inherit file from rationaleFor target)
    for edge in edges:
        if edge.get("relation") == "rationale_for":
            rationale_id = edge["source"]
            target_id = edge["target"]
            if rationale_id not in membership:
                if target_id in membership:
                    membership[rationale_id] = membership[target_id]
                elif target_id in file_nodes:
                    membership[rationale_id] = target_id

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

    # Identify file-level nodes
    file_node_ids: set[str] = set()
    for nid, label in node_labels.items():
        if any(label.endswith(ext) for ext in _FILE_EXTS):
            file_node_ids.add(nid)

    # Build file membership (must come before role_map for Gap 2)
    file_membership = _build_file_membership(edges, file_node_ids)

    # Infer types (uses file_membership for struct vs class distinction)
    role_map = _build_role_map(edges, nodes, file_membership, node_labels)

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
        ops_rel = RELATION_MAP.get(relation)
        if not ops_rel:
            continue
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
