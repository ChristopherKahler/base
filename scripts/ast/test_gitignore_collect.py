#!/usr/bin/env python3
"""collect_files honours .gitignore inside a git work tree, by asking git.

Before this, only `.baseignore` and the hard-coded noise directories kept
files out of a map. A repo whose own .gitignore already named its generated
tree (build output, a vendored SDK, a checked-out dependency) still had every
one of those files walked, counted against the 2000-file threshold, and
parsed — and an automatic build that tripped the threshold died silently.
Chris, 2026-09-01: no app may go without a map, and an unattended build cannot
stop to ask about the count.

Run: python3 scripts/ast/test_gitignore_collect.py   (or: pytest scripts/ast/)
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from extractor import collect_files  # noqa: E402


def _tree(root: Path) -> None:
    (root / "src").mkdir(parents=True)
    (root / "src" / "app.py").write_text("def app():\n    pass\n")
    (root / "generated").mkdir()
    (root / "generated" / "gen.py").write_text("def gen():\n    pass\n")
    (root / "secrets.py").write_text("TOKEN = 'x'\n")
    (root / ".gitignore").write_text("generated/\nsecrets.py\n")


def _names(root: Path) -> list[str]:
    return sorted(p.relative_to(root).as_posix() for p in collect_files(root))


def _fixture_dir() -> tempfile.TemporaryDirectory:
    # NOT under /tmp: file discovery drops any path with a `tmp` component, so
    # a fixture there collects nothing and the test would fail for the wrong
    # reason (or pass vacuously). Same rule as test_file_attribution.py.
    return tempfile.TemporaryDirectory(prefix="base-ast-gitignore-", dir=Path.home())


def test_gitignored_files_stay_out_of_a_git_repo():
    if not shutil.which("git"):
        print("skip: git not on PATH")
        return
    with _fixture_dir() as d:
        root = Path(d) / "repo"
        _tree(root)
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        # Untracked-but-not-ignored is kept (a new file is still the app's);
        # ignored is not, tracked or otherwise.
        assert _names(root) == ["src/app.py"], _names(root)


def test_outside_git_the_walk_decides_alone():
    with _fixture_dir() as d:
        # Home may be someone's dotfiles repo; a ceiling keeps git discovery
        # from climbing out of the fixture.
        os.environ["GIT_CEILING_DIRECTORIES"] = d
        try:
            root = Path(d) / "plain"
            _tree(root)
            assert _names(root) == ["generated/gen.py", "secrets.py", "src/app.py"], _names(root)
        finally:
            del os.environ["GIT_CEILING_DIRECTORIES"]


if __name__ == "__main__":
    test_gitignored_files_stay_out_of_a_git_repo()
    test_outside_git_the_walk_decides_alone()
    print("ok")
