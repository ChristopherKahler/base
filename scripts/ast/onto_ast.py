#!/usr/bin/env python3
"""AST-to-ontology extraction — parse source files via tree-sitter, emit TTL triples.

Uses tree-sitter-based extraction (extractor.py) for 35+ language support,
then serializes to Turtle via ttl_serializer.py for loading into the BASE graph.
"""

import argparse
import sys
from pathlib import Path

from extractor import extract, _get_extractor, _safe_extract, collect_files
from ttl_serializer import serialize

LANG_MAP = {
    ".py": "python", ".js": "javascript", ".ts": "typescript", ".tsx": "typescript",
    ".jsx": "javascript", ".svelte": "svelte", ".astro": "astro",
    ".java": "java", ".groovy": "groovy", ".kt": "kotlin", ".scala": "scala",
    ".c": "c", ".h": "c", ".cpp": "cpp", ".cc": "cpp", ".cxx": "cpp", ".hpp": "cpp",
    ".rb": "ruby", ".cs": "csharp", ".php": "php", ".blade.php": "php",
    ".dart": "dart", ".v": "verilog", ".sv": "verilog",
    ".sql": "sql", ".lua": "lua", ".swift": "swift", ".jl": "julia",
    ".f90": "fortran", ".f95": "fortran", ".f03": "fortran", ".f08": "fortran",
    ".go": "go", ".rs": "rust", ".zig": "zig",
    ".ps1": "powershell", ".psm1": "powershell",
    ".m": "objective-c", ".mm": "objective-c",
    ".ex": "elixir", ".exs": "elixir",
    ".md": "markdown", ".pas": "pascal", ".pp": "pascal",
    ".sh": "bash", ".bash": "bash", ".zsh": "bash",
    ".json": "json",
}


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
        # Build file map: bare filename → relative path (for sourceFile resolution)
        file_map = {}
        for f in files:
            try:
                rel = str(f.relative_to(target))
            except ValueError:
                rel = f.name
            file_map[f.name] = rel
        return serialize(result, project, str(target), "multi", file_map=file_map)

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
