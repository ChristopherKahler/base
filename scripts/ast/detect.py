"""File detection utilities for AST extraction.

Provides ignore-pattern loading, path filtering, and noise-directory detection.
Extracted as a dependency of extractor.py's collect_files().
"""

from pathlib import Path
from typing import Optional

# Directories that are always noise — never walk into these
_NOISE_DIRS = frozenset({
    "node_modules", ".git", ".hg", ".svn", "__pycache__", ".mypy_cache",
    ".pytest_cache", ".tox", ".eggs", "dist", "build", ".next", ".nuxt",
    ".output", "target", ".cargo", "vendor", ".bundle", "venv", ".venv",
    "env", ".env", ".direnv", "coverage", ".coverage", ".nyc_output",
    ".turbo", ".cache", ".parcel-cache", ".webpack", "tmp", "temp",
    ".terraform", ".pulumi", "zig-cache", "zig-out",
    ".base-ast-cache",
})


def _is_noise_dir(name: str) -> bool:
    """Check if a directory name is noise that should be skipped."""
    return name.lower() in _NOISE_DIRS or name.startswith(".")


def _load_baseignore(root: Path) -> list[str]:
    """Load ignore patterns from .baseignore file at root.

    Returns empty list if no file exists. Patterns follow .gitignore syntax
    (simplified — glob patterns, one per line, # comments, blank lines skipped).
    """
    ignore_file = root / ".baseignore"
    if not ignore_file.exists():
        return []

    patterns = []
    for line in ignore_file.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            patterns.append(line)
    return patterns


def _is_ignored(path: Path, root: Path, patterns: list[str]) -> bool:
    """Check if a path matches any ignore pattern.

    Uses simple glob matching against the path relative to root.
    """
    if not patterns:
        return False

    try:
        rel = path.relative_to(root)
    except ValueError:
        return False

    rel_str = str(rel)

    for pattern in patterns:
        # Direct name match (e.g., "*.log")
        if path.match(pattern):
            return True
        # Check if any parent directory matches
        if "/" not in pattern and any(part == pattern for part in rel.parts):
            return True
        # Glob-style match against relative path
        if rel_str == pattern or rel_str.startswith(pattern.rstrip("/") + "/"):
            return True

    return False
