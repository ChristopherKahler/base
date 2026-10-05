#!/usr/bin/env python3
"""A pnpm workspace whose `packages:` lists the root itself (#172).

`packages: [ "." ]` is valid pnpm for a single-package workspace. The workspace
loader handed every entry to `Path.glob`, which rejects `.` and `./` (a
`ValueError` on one Python, an `IndexError` or `AttributeError` on another), so
`base sync --ast` died before writing a map. `.`, `./` and an empty entry mean
the workspace root; any other entry `Path.glob` rejects is skipped with one
`# Skipped ...` notice, never a crash.

Run: python3 scripts/ast/test_pnpm_workspace_root.py   (or: pytest scripts/ast/)
"""

import contextlib
import io
import json
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import extractor  # noqa: E402


def _fixture_dir() -> tempfile.TemporaryDirectory:
    # NOT under /tmp: file discovery drops any path with a `tmp` component.
    # Same rule as test_gitignore_collect.py.
    return tempfile.TemporaryDirectory(prefix="base-ast-pnpm-", dir=Path.home())


def _workspace(root: Path, entries: list[str]) -> None:
    root.mkdir(parents=True)
    lines = ["packages:"] + [f"  - {e}" for e in entries]
    (root / "pnpm-workspace.yaml").write_text("\n".join(lines) + "\n")
    (root / "package.json").write_text(json.dumps({"name": "root-pkg"}))
    lib = root / "packages" / "lib"
    lib.mkdir(parents=True)
    (lib / "package.json").write_text(json.dumps({"name": "lib-pkg"}))
    (lib / "index.ts").write_text("export function lib() { return 1; }\n")


def _load(root: Path) -> tuple[dict, str]:
    extractor._WORKSPACE_PACKAGE_CACHE.clear()
    err = io.StringIO()
    with contextlib.redirect_stderr(err):
        packages = extractor._load_workspace_packages(root)
    return packages, err.getvalue()


def test_pnpm_workspace_root_entry_is_the_root():
    for entry in (".", "./", "'.'", '"./"'):
        with _fixture_dir() as d:
            root = Path(d) / "ws"
            _workspace(root, [entry, "packages/*"])
            packages, err = _load(root)
            assert packages.get("root-pkg") == root, (entry, packages)
            assert packages.get("lib-pkg") == root / "packages" / "lib", (entry, packages)
            assert "Skipped" not in err, (entry, err)


def test_an_unusable_entry_is_skipped_with_one_notice():
    with _fixture_dir() as d:
        root = Path(d) / "ws"
        # An absolute pattern: Path.glob refuses it on every supported Python.
        _workspace(root, ["/abs/*", "packages/*"])
        packages, err = _load(root)
        assert packages == {"lib-pkg": root / "packages" / "lib"}, packages
        notices = [l for l in err.splitlines() if l.startswith("# Skipped ")]
        assert len(notices) == 1, err
        assert "/abs/*" in notices[0] and "pnpm-workspace.yaml" in notices[0], notices
        # Loaded again from the cache: no second notice.
        err2 = io.StringIO()
        with contextlib.redirect_stderr(err2):
            extractor._load_workspace_packages(root)
        assert "Skipped" not in err2.getvalue(), err2.getvalue()


def test_extraction_through_a_root_entry_workspace_does_not_crash():
    # The reported path: an import of a workspace package resolves through the
    # loader during extraction (#172's traceback is from inside it).
    with _fixture_dir() as d:
        root = Path(d) / "ws"
        _workspace(root, ["."])
        app = root / "src" / "app.ts"
        app.parent.mkdir(parents=True)
        app.write_text("import { lib } from 'lib-pkg';\nexport function app() { return lib(); }\n")
        files = extractor.collect_files(root)
        assert app in files, files
        result = extractor.extract(files, cache_root=root, parallel=False)
        assert any(n.get("label") == "app()" or "app" in str(n.get("label", "")) for n in result["nodes"]), result["nodes"][:5]


if __name__ == "__main__":
    test_pnpm_workspace_root_entry_is_the_root()
    test_an_unusable_entry_is_skipped_with_one_notice()
    test_extraction_through_a_root_entry_workspace_does_not_crash()
    print("ok")
