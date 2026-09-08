#!/usr/bin/env python3
"""AST-to-ontology extraction — parse source files via tree-sitter, emit TTL triples.

Uses tree-sitter-based extraction (extractor.py) for 35+ language support,
then serializes to Turtle via ttl_serializer.py for loading into the BASE graph.
"""

import argparse
import sys
from pathlib import Path

from extractor import (
    extract, _get_extractor, _safe_extract, collect_files, _make_id, _file_stem,
    _DISPATCH,
)
from ttl_serializer import serialize

# The language an extension is reported as, derived from the extractor it
# dispatches to.
#
# #83: this was a third hand-kept list beside `_DISPATCH` (what is parsed) and
# `_FILE_EXTS` (what counts as a file), and it had drifted from both -- 22
# parsed extensions with no entry here, falling back to "unknown", and two
# entries (`.psm1`, `.zsh`) claiming a language for files nothing ever parsed.
# #66 derived `_FILE_EXTS` from `_DISPATCH` for exactly this reason and left
# this list alone; it then widened the gap by one, adding `.cjs` to the parser
# table and not to this map.
#
# 32 of the 33 extractors serve exactly one language, so the language is a
# property of the extractor rather than of the extension. Only `extract_js`
# serves several, and those extensions are named in `_EXT_LANG` below. A new
# entry in `_DISPATCH` therefore gets a language for free, and one whose
# extractor is not named here fails loudly at import rather than reaching the
# graph as "unknown" -- the same choice `_parsed_extensions` makes, and for the
# same reason: a silent degraded default is the failure this exists to kill.
_EXTRACTOR_LANG = {
    "extract_astro": "astro",
    "extract_bash": "bash",
    "extract_c": "c",
    "extract_cpp": "cpp",
    "extract_csharp": "csharp",
    "extract_dart": "dart",
    "extract_delphi_form": "pascal",
    "extract_elixir": "elixir",
    "extract_fortran": "fortran",
    "extract_go": "go",
    "extract_groovy": "groovy",
    "extract_java": "java",
    "extract_js": "javascript",
    "extract_json": "json",
    "extract_julia": "julia",
    "extract_kotlin": "kotlin",
    "extract_lazarus_form": "pascal",
    "extract_lazarus_package": "pascal",
    "extract_lua": "lua",
    "extract_markdown": "markdown",
    "extract_objc": "objective-c",
    "extract_pascal": "pascal",
    "extract_php": "php",
    "extract_powershell": "powershell",
    "extract_python": "python",
    "extract_ruby": "ruby",
    "extract_rust": "rust",
    "extract_scala": "scala",
    "extract_sql": "sql",
    "extract_svelte": "svelte",
    "extract_swift": "swift",
    "extract_verilog": "verilog",
    "extract_zig": "zig",
}

# Extensions whose language is not the one their extractor implies.
# `.blade.php` is not a `_DISPATCH` key at all -- `_get_extractor` matches it on
# the full filename -- so it is carried here or it is carried nowhere.
_EXT_LANG = {
    ".ts": "typescript",
    ".tsx": "typescript",
    ".vue": "vue",
    ".blade.php": "php",
}


def _build_lang_map() -> dict[str, str]:
    """Every parsed extension, with the language it is reported as."""
    mapping = dict(_EXT_LANG)
    unknown = []
    for ext, extractor_fn in _DISPATCH.items():
        if ext in mapping:
            continue
        name = getattr(extractor_fn, "__name__", "")
        language = _EXTRACTOR_LANG.get(name)
        if language is None:
            unknown.append(f"{ext} -> {name or extractor_fn!r}")
        else:
            mapping[ext] = language
    if unknown:
        raise RuntimeError(
            "extension(s) in _DISPATCH whose extractor has no language in "
            "_EXTRACTOR_LANG: " + ", ".join(sorted(unknown))
        )
    return mapping


LANG_MAP = _build_lang_map()


def extract_single(path: Path) -> dict:
    extractor = _get_extractor(path)
    if extractor is None:
        return {"nodes": [], "edges": [], "error": f"unsupported: {path.suffix}"}
    return _safe_extract(extractor, path)


def extract_project(target: Path, project: str, full: bool = False, confirm: bool = False) -> str:
    """Extract a file or directory and return the combined TTL string."""
    if target.is_file():
        extraction = extract_single(target)
        if "error" in extraction:
            print(f"Error: {extraction['error']}", file=sys.stderr)
            sys.exit(1)
        language = LANG_MAP.get(target.suffix, "unknown")
        return serialize(extraction, project, str(target), language)

    files = collect_files(target)
    if not files:
        print(f"No extractable files found in {target}", file=sys.stderr)
        sys.exit(1)

    FILE_COUNT_WARNING = 2000
    if len(files) > FILE_COUNT_WARNING:
        print(
            f"WARNING: Found {len(files)} files (threshold: {FILE_COUNT_WARNING}).\n"
            f"This likely includes dependency code (node_modules, vendor, etc.).\n"
            f"Re-run with --confirm to proceed, or check your .gitignore.",
            file=sys.stderr,
        )
        if not confirm:
            sys.exit(1)

    print(f"# Extracting {len(files)} files from {target}", file=sys.stderr)

    if full:
        result = extract(files, cache_root=target)
        # Map every file to its relative path under every key a file node might
        # carry, most specific first:
        #
        #   _make_id(relative path) — what multi-file extraction actually emits
        #                             (`src/relay/mod.rs` -> `src_relay_mod_rs`)
        #   _make_id(full path)     — single-file extraction, which sees an
        #                             absolute path
        #   _make_id(stem + suffix) — extractors that id by `parent.stem`
        #   bare filename           — legacy fallback, still correct whenever the
        #                             name is unique in the tree
        #
        # Every id form is unique per file; the bare name is NOT. Keying on the
        # bare name alone collapsed every same-named file in the tree (every
        # `mod.rs`, `index.ts`, `__init__.py`) onto whichever one was extracted
        # last, so their entities were all attributed to one arbitrary wrong
        # file — `base ast query --file src/relay/mod.rs` found nothing while
        # its 270 entities sat under `src/update/mod.rs`.
        file_map = {}
        for f in files:
            try:
                rel = str(f.relative_to(target))
            except ValueError:
                rel = f.name
            file_map[_make_id(rel)] = rel
            file_map[_make_id(str(f))] = rel
            file_map[_make_id(f"{_file_stem(f)}{f.suffix}")] = rel
            file_map.setdefault(f.name, rel)
        stats: dict = {}
        ttl = serialize(result, project, str(target), "multi", file_map=file_map, stats=stats)
        # #66: an entity with no file node is attributed to the app root. Say so.
        # Zero prints nothing. A foreground `base sync --ast --yes` echoes every
        # `# ` line back on success (src/hook/automap.rs, echo_extractor_notices);
        # the BACKGROUND refresh cannot show it to anyone, because both the Stop
        # hook (automap.rs spawn_sync) and the git hook (ast_repo.rs) discard the
        # child's output by design. See the follow-up issue on persisting notices.
        # #105: the three import states, printed BY THE RUN. A single dangling
        # total cannot tell a fix from a suppression — an extractor that
        # stopped emitting scores the same zero as one that resolved
        # everything — so each state is reported on its own, and a failure is
        # reported AS a failure rather than filed under "external".
        states = result.get("import_states") or {}
        if states:
            print(
                "# imports: {resolved} resolved, {failed} failed to resolve, "
                "{external} third-party".format(
                    resolved=states.get("resolved", 0),
                    failed=states.get("failed", 0),
                    external=states.get("external", 0),
                ),
                file=sys.stderr,
            )
            if states.get("failed"):
                print(
                    f"# {states['failed']} import(s) name a path inside this tree "
                    f"that does not resolve — these are broken imports, not "
                    f"third-party packages",
                    file=sys.stderr,
                )
            for key, note in (
                ("resolved_unparsed", "resolved to a file this extractor does not parse"),
                ("ambiguous_multi", "bare name matched more than one file; carried as third-party"),
                ("unclassified", "no import_kind recorded by the extractor that emitted it"),
            ):
                if states.get(key):
                    print(f"#   {states[key]} {note}", file=sys.stderr)
        untyped = stats.get("untyped_non_code_entities", 0)
        if untyped:
            print(
                f"# {untyped} non-code entit{'y' if untyped == 1 else 'ies'} had no "
                f"kind and are typed ops:Entity (never ops:Function)",
                file=sys.stderr,
            )

        orphans = stats.get("app_root_entities", 0)
        if orphans:
            noun = "entity" if orphans == 1 else "entities"
            print(
                f"# {orphans} {noun} attributed to the app root (no file node)",
                file=sys.stderr,
            )
        return ttl

    chunks = []
    for file_path in files:
        extraction = extract_single(file_path)
        if "error" in extraction:
            print(f"# Skipped {file_path}: {extraction['error']}", file=sys.stderr)
            continue
        language = LANG_MAP.get(file_path.suffix, "unknown")
        chunk = serialize(extraction, project, str(file_path), language)
        if chunk:
            chunks.append(chunk)
    return "\n".join(chunks)


def main():
    parser = argparse.ArgumentParser(description="Extract AST to TTL triples")
    parser.add_argument("path", type=Path, help="Source file or directory to extract")
    parser.add_argument("--project", type=str, default=None, help="Project name (default: directory name)")
    parser.add_argument("--full", action="store_true", help="Full extraction with cross-file resolution (directory mode)")
    parser.add_argument("--confirm", action="store_true", help="Confirm extraction when file count exceeds safety threshold")
    parser.add_argument("--out", type=Path, default=None, help="Write TTL to file (default: stdout)")
    parser.add_argument("--append", action="store_true", help="Append to --out file instead of overwriting")
    args = parser.parse_args()

    target = args.path.resolve()
    if not target.exists():
        print(f"Error: {target} does not exist", file=sys.stderr)
        sys.exit(1)

    project = args.project or (target.name if target.is_dir() else target.parent.name)
    ttl = extract_project(target, project, full=args.full, confirm=args.confirm)

    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        if args.append:
            with open(args.out, "a", encoding="utf-8") as f:
                f.write(ttl)
                f.write("\n")
        else:
            # Atomic write: temp + rename, so a concurrent/background sync
            # (e.g. the Stop hook) can never observe or leave a torn ast.ttl.
            import os
            tmp = args.out.parent / (args.out.name + ".tmp")
            with open(tmp, "w", encoding="utf-8") as f:
                f.write(ttl)
                f.write("\n")
            os.replace(tmp, args.out)
        print(f"TTL written to {args.out}", file=sys.stderr)
    else:
        print(ttl)


if __name__ == "__main__":
    main()
