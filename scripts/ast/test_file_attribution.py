#!/usr/bin/env python3
"""Same-named files must keep their own identity in the AST map.

Regression guard for the collision that made `base ast query --file` lie: the
serializer resolved a node's sourceFile through a map keyed by BARE FILENAME,
so every `mod.rs` / `index.ts` / `__init__.py` in a tree collapsed onto
whichever one was extracted last. `--file src/relay/mod.rs` then found nothing
while its entities sat under `src/update/mod.rs`, and `--calls` reported the
wrong file for perfectly ordinary functions.

Run: python3 scripts/ast/test_file_attribution.py   (or: pytest scripts/ast/)
"""

import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent

# Three same-named files in different directories, each with a unique symbol.
TREE = {
    "src/alpha/mod.rs": "pub fn alpha_only() {}\n",
    "src/beta/mod.rs": "pub fn beta_only() {}\n",
    "src/gamma/mod.rs": "pub fn gamma_only() {}\n",
    "src/solo.rs": "pub fn solo_only() {}\n",
}


def _extract(root: Path) -> str:
    proc = subprocess.run(
        [sys.executable, str(HERE / "onto_ast.py"), str(root), "--project", "t", "--full"],
        capture_output=True,
        text=True,
        cwd=str(HERE),
    )
    assert proc.returncode == 0, f"extraction failed:\n{proc.stderr}"
    return proc.stdout


def test_same_named_files_get_distinct_source_files():
    # NOT under /tmp: file discovery drops any path with a `tmp` component, so
    # a fixture there extracts to nothing and the test would pass vacuously.
    with tempfile.TemporaryDirectory(prefix="base-ast-fixture-", dir=Path.home()) as td:
        root = Path(td)
        for rel, body in TREE.items():
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(body)
        # File discovery walks a repo, so the fixture has to look like one.
        subprocess.run(["git", "init", "-q"], cwd=str(root), check=True)

        ttl = _extract(root)

        for rel in TREE:
            assert f'ops:sourceFile "{rel}"' in ttl, (
                f"{rel} has no entities attributed to it — same-named files "
                f"collapsed onto one path again"
            )

        # Each unique symbol must be attributed to its OWN file, not a neighbour's.
        # An entity is a run of lines; sourceFile follows the label two lines down.
        lines = ttl.splitlines()
        for rel, body in TREE.items():
            symbol = body.split("fn ")[1].split("(")[0]
            idx = [i for i, ln in enumerate(lines) if f'rdfs:label "{symbol}(' in ln]
            assert idx, f"symbol {symbol} missing entirely"
            window = "\n".join(lines[idx[0] : idx[0] + 4])
            assert f'ops:sourceFile "{rel}"' in window, (
                f"{symbol} should be attributed to {rel}, got:\n{window}"
            )


if __name__ == "__main__":
    test_same_named_files_get_distinct_source_files()
    print("ok — same-named files keep distinct sourceFile attribution")
