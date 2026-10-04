#!/usr/bin/env python3
"""The CI guards: checks that fail a run whose green would not mean what it says.

Called from `.github/workflows/ci.yml`. Standard library only, so it runs on every runner
before anything is installed. Each guard closes a way a false green reached a report in the
0.16.0 round (BO-00, 2026-10-01):

  ignores              A failing test was marked `#[ignore]` and the suite stayed green (FS1).
                       Every ignore in src/ and tests/ must be on .github/ignored-tests.txt with
                       its reason, and every entry there must still be an ignored test.
  no-allowed-failure   A required job was once allowed to fail. No workflow file may contain
                       the key at all, comments included, so it cannot be put back quietly.
  no-test-filters      A hanging test was left out of the run and the total quoted anyway. ci.yml's
                       `cargo test` commands may carry no `--skip`, `--exclude`, test name or
                       target filter. Arguments are allowed by name; anything else fails.
  timeouts             A hang ran for hours. Every job in every workflow file has `timeout-minutes`;
                       until BO-22 this read ci.yml alone, and release.yml's four jobs had none.
  test-count           Tests can vanish without failing (a target that stops compiling on one
                       platform, a test deleted). The `test result: ok. N passed` lines of a
                       `cargo test` log must sum to the platform's number in
                       .github/test-counts.txt, in either direction.

`--selftest` runs first in CI and is the control: every arm of every guard is run on a
fixture where it must pass and one where it must fail, and the failing ones must name what
they caught. A guard that cannot fail is not a guard.

Exit codes: 0 pass, 1 the guard found something, 2 the guard could not run (bad arguments,
missing file). A check that reads nothing fails: zero files, zero commands or zero result
lines never reads as a pass.
"""

from __future__ import annotations

import argparse
import os
import re
import shlex
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ALLOWLIST = Path(".github/ignored-tests.txt")
COUNTS = Path(".github/test-counts.txt")
WORKFLOWS = Path(".github/workflows")
CI_WORKFLOW = WORKFLOWS / "ci.yml"
FORBIDDEN_KEY = "continue-on-error"
SEP = " · "


def fail(lines: list[str]) -> int:
    for line in lines:
        print(f"FAIL: {line}")
    return 1


# ── ignores ──────────────────────────────────────────────────────────────────


def strip_rust(text: str) -> str:
    """`text` with comments and string and char literal contents blanked to spaces.

    Newlines are kept, so offsets and line numbers still match the file. Without this a doc
    comment that MENTIONS `#[ignore]` (tests/deferral_test.rs has two) reads as an attribute,
    and an attribute inside a raw string in a test fixture reads as one too.
    """
    out = list(text)
    i, n = 0, len(text)

    def blank(a: int, b: int) -> None:
        for k in range(a, min(b, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end == -1 else end
            blank(i, end)
            i = end
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c == "r" and re.match(r'r(#*)"', text[i:]) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            hashes = re.match(r'r(#*)"', text[i:]).group(1)
            start = i + 2 + len(hashes)
            end = text.find('"' + hashes, start)
            end = n if end == -1 else end + 1 + len(hashes)
            blank(start, end - 1 - len(hashes))
            i = end
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            blank(i + 1, j)
            i = j + 1
        elif c == "'":
            # A char literal ('x', '\n', '\u{1F600}'), or a lifetime ('a), which is left alone.
            m = re.match(r"'(\\u\{[0-9a-fA-F]+\}|\\.|[^\\'])'", text[i:])
            if m:
                blank(i + 1, i + len(m.group(0)) - 1)
                i += len(m.group(0))
            else:
                i += 1
        else:
            i += 1
    return "".join(out)


ATTR = re.compile(r"#\s*\[\s*(?:ignore\b|cfg_attr\s*\((?:[^()]|\([^()]*\))*?\bignore\b)")
FN = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")


def find_ignored(root: Path) -> dict[str, int]:
    """`<file>:<test fn>` for every ignore attribute in src/ and tests/, with its line number."""
    found: dict[str, int] = {}
    for top in ("src", "tests"):
        base = root / top
        if not base.is_dir():
            continue
        for path in sorted(base.rglob("*.rs")):
            text = path.read_text(encoding="utf-8", errors="replace")
            code = strip_rust(text)
            rel = path.relative_to(root).as_posix()
            for m in ATTR.finditer(code):
                f = FN.search(code, m.end())
                name = f.group(1) if f else "<no fn after the attribute>"
                found.setdefault(f"{rel}:{name}", code.count("\n", 0, m.start()) + 1)
    return found


def read_allowlist(path: Path) -> tuple[dict[str, str], list[str]]:
    entries: dict[str, str] = {}
    errors: list[str] = []
    for no, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        key, sep, reason = line.partition(SEP)
        if not sep or not reason.strip() or ":" not in key:
            errors.append(f"{path.as_posix()}:{no} is not '<file>:<test name>{SEP}<reason>': {raw}")
            continue
        entries[key.strip()] = reason.strip()
    return entries, errors


def check_ignores(root: Path) -> int:
    allow_path = root / ALLOWLIST
    if not allow_path.is_file():
        print(f"FAIL: {ALLOWLIST.as_posix()} does not exist")
        return 2
    found = find_ignored(root)
    listed, errors = read_allowlist(allow_path)
    problems = list(errors)
    for key, line in sorted(found.items()):
        if key not in listed:
            file = key.split(":", 1)[0]
            problems.append(f"{key} is ignored but not on {ALLOWLIST.as_posix()} (attribute at {file}:{line})")
    for key in sorted(listed):
        if key not in found:
            problems.append(f"{key} is on {ALLOWLIST.as_posix()} but is not an ignored test in src/ or tests/")
    if problems:
        return fail(problems)
    print(f"ignored tests: {len(found)} found, {len(listed)} listed, every one matches")
    for key in sorted(found):
        print(f"  {key}{SEP}{listed[key]}")
    return 0


# ── workflow files ───────────────────────────────────────────────────────────


def workflow_files(root: Path) -> list[Path]:
    d = root / WORKFLOWS
    return sorted(p for p in d.glob("*") if p.suffix in (".yml", ".yaml")) if d.is_dir() else []


def check_no_continue_on_error(root: Path) -> int:
    files = workflow_files(root)
    if not files:
        return fail([f"no workflow files under {WORKFLOWS.as_posix()}; this check read nothing"])
    hits = []
    for path in files:
        for no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if FORBIDDEN_KEY in line.lower():
                hits.append(f"{path.relative_to(root).as_posix()}:{no} contains {FORBIDDEN_KEY}: {line.strip()}")
    if hits:
        return fail(hits)
    print(f"{FORBIDDEN_KEY}: absent from {len(files)} workflow file(s)")
    return 0


CARGO_TEST = re.compile(r"\bcargo\s+(?:\+\S+\s+)?test\b")
YAML_KEY = re.compile(r"^-?\s*([A-Za-z0-9_-]+)\s*:")
SHELL_STOP = re.compile(r"\s(?:\d?>|<|\||;|&&|\|\||#)|\)$")
# Arguments that change how tests run, never which run. Anything not here fails.
CARGO_FLAGS = {"--no-fail-fast", "--locked", "--frozen", "--offline", "--workspace", "--all-features",
               "--release", "-v", "--verbose", "-q", "--quiet"}
CARGO_VALUED = {"--color", "-j", "--jobs"}
HARNESS_FLAGS = {"--nocapture", "--show-output"}
HARNESS_VALUED = {"--test-threads", "--color"}


def cargo_test_commands(path: Path) -> list[tuple[int, str]]:
    """Each `cargo test` in a workflow's shell, with its line, continuation lines joined."""
    lines = path.read_text(encoding="utf-8").splitlines()
    joined: list[tuple[int, str]] = []
    i = 0
    while i < len(lines):
        start, text = i + 1, lines[i]
        while text.rstrip().endswith("\\") and i + 1 < len(lines):
            i += 1
            text = text.rstrip()[:-1] + " " + lines[i].strip()
        joined.append((start, text))
        i += 1
    out = []
    for no, text in joined:
        stripped = text.strip()
        if stripped.startswith("#"):
            continue
        m = CARGO_TEST.search(text)
        if not m:
            continue
        key = YAML_KEY.match(stripped)
        if key and key.group(1) != "run":
            continue  # a step's name or another key mentioning the command, not the command
        rest = text[m.end():]
        stop = SHELL_STOP.search(rest)
        out.append((no, rest[: stop.start()] if stop else rest))
    return out


def filter_problems(args: str) -> list[str]:
    try:
        tokens = shlex.split(args)
    except ValueError as e:
        return [f"cannot parse the arguments ({e})"]
    problems, harness, i = [], False, 0
    while i < len(tokens):
        t = tokens[i]
        if t == "--" and not harness:
            harness = True
        elif t in ("--skip", "--exclude") or t.startswith(("--skip=", "--exclude=")):
            problems.append(f"{t.split('=')[0]} leaves tests out")
            i += 1 if "=" not in t else 0
        else:
            flags, valued = (HARNESS_FLAGS, HARNESS_VALUED) if harness else (CARGO_FLAGS, CARGO_VALUED)
            name = t.split("=", 1)[0]
            if t in flags:
                pass
            elif name in valued:
                i += 0 if "=" in t else 1
            elif not t.startswith("-"):
                problems.append(f"'{t}' is a test name filter")
            else:
                problems.append(f"'{t}' is not an argument this guard knows to be safe")
        i += 1
    return problems


def check_no_test_filters(root: Path) -> int:
    # ci.yml only: it runs the suite whose result lines `test-count` sums. release.yml's docs gate runs
    # `cargo test --bin base help_docs` on purpose and is not the suite.
    path = root / CI_WORKFLOW
    commands = [(path, no, args) for no, args in cargo_test_commands(path)] if path.is_file() else []
    if not commands:
        return fail([f"no `cargo test` command in {CI_WORKFLOW.as_posix()}; this check read nothing"])
    problems = []
    for path, no, args in commands:
        for p in filter_problems(args):
            problems.append(f"{path.relative_to(root).as_posix()}:{no} cargo test{args}: {p}")
    if problems:
        return fail(problems)
    for path, no, args in commands:
        print(f"{path.relative_to(root).as_posix()}:{no} cargo test{args.rstrip()}: no filter")
    return 0


JOB = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")


def workflow_jobs(path: Path) -> dict[str, bool]:
    """Each job in a workflow file, and whether it sets a job-level `timeout-minutes`.

    A job that calls a reusable workflow (`uses:` at job level) counts as set: GitHub refuses the
    key there, and the called file is a workflow file whose own jobs this guard reads.
    """
    jobs: dict[str, bool] = {}
    in_jobs, current = False, None
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.lstrip().startswith("#"):
            continue  # a comment at column 0 does not end the jobs block
        if re.match(r"^\S", line):
            in_jobs, current = line.startswith("jobs:"), None
            continue
        if not in_jobs:
            continue
        m = JOB.match(line)
        if m:
            current = m.group(1)
            jobs[current] = False
        elif current and re.match(r"^    (timeout-minutes:\s*\d+|uses:\s*\S+)\s*(#.*)?$", line):
            jobs[current] = True
    return jobs


def check_timeouts(root: Path) -> int:
    files = workflow_files(root)
    if not files:
        return fail([f"no workflow files under {WORKFLOWS.as_posix()}; this check read nothing"])
    problems, read = [], []
    for path in files:
        rel = path.relative_to(root).as_posix()
        jobs = workflow_jobs(path)
        if not jobs:
            problems.append(f"no jobs found in {rel}; this check read nothing there")
            continue
        problems += [f"{rel}: job '{j}' has no timeout-minutes, so a hang runs for hours" for j, ok in jobs.items() if not ok]
        read.append((rel, list(jobs)))
    if problems:
        return fail(problems)
    print(f"timeout-minutes: set on all {sum(len(j) for _, j in read)} job(s) in {len(read)} workflow file(s)")
    for rel, jobs in read:
        print(f"  {rel}: {', '.join(jobs)}")
    return 0


# ── test-count ───────────────────────────────────────────────────────────────

RESULT = re.compile(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored")
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def check_test_count(root: Path, platform: str, log: Path) -> int:
    platform = platform.lower()
    counts_path = root / COUNTS
    if not counts_path.is_file():
        print(f"FAIL: {COUNTS.as_posix()} does not exist")
        return 2
    if not log.is_file():
        print(f"FAIL: no cargo test log at {log}")
        return 2
    want = None
    for raw in counts_path.read_text(encoding="utf-8").splitlines():
        parts = raw.split("#", 1)[0].split()
        if len(parts) == 2 and parts[0] == platform and parts[1].isdigit():
            want = int(parts[1])
    if want is None:
        return fail([f"{COUNTS.as_posix()} has no '{platform} <n>' line"])
    lines = ok = passed = failed_lines = ignored = 0
    for raw in log.read_text(encoding="utf-8", errors="replace").splitlines():
        m = RESULT.match(ANSI.sub("", raw).strip())
        if not m:
            continue
        lines += 1
        ignored += int(m.group(4))
        if m.group(1) == "ok":
            ok += 1
            passed += int(m.group(2))
        else:
            failed_lines += 1
    if lines == 0:
        return fail([f"{platform}: no 'test result:' lines in {log}; the suite did not run, so nothing was counted"])
    print(f"{platform}: {lines} result lines ({ok} ok, {failed_lines} FAILED), {passed:,} passed in ok lines, {ignored} ignored")
    if failed_lines:
        # The test step fails on its own, unless something swallowed its exit code (`|| true`, a lost
        # pipefail). This makes the count step fail either way.
        return fail([f"{platform}: {failed_lines} test target(s) reported FAILED; a failing run is never counted as a pass"])
    if passed != want:
        hint = ("a PR that adds tests updates the number in the same diff" if passed > want
                else "a PR that removes tests updates the number and names the removed tests in its body")
        return fail([f"{platform}: {passed:,} tests passed, {COUNTS.as_posix()} says {want} ({hint})"])
    print(f"{platform}: matches {COUNTS.as_posix()}")
    return 0


# ── selftest ─────────────────────────────────────────────────────────────────


def selftest() -> int:
    import contextlib
    import io

    results: list[tuple[str, bool, str]] = []

    def run(fn, *args) -> tuple[int, str]:
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = fn(*args)
        return rc, buf.getvalue()

    def arm(name: str, fn, args: tuple, want_rc: int, must_say: str = "", must_not_say: str = "") -> None:
        rc, out = run(fn, *args)
        ok = rc == want_rc and must_say in out and (not must_not_say or must_not_say not in out)
        results.append((name, ok, f"rc={rc} want={want_rc}" + ("" if ok else f"\n{out}")))

    def tree(files: dict[str, str]) -> Path:
        root = Path(tempfile.mkdtemp(prefix="ci-guards-"))
        for rel, text in files.items():
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text, encoding="utf-8")
        return root

    good_rs = (
        '#[test]\n#[ignore = "benchmark"]\nfn bench_a() {}\n'
        "/// this test once sat under `#[ignore]`, a mention and not an attribute\n#[test]\nfn mentioned() {}\n"
        '#[test]\nfn quoted() { let s = "#[ignore]"; let r = r#"#[ignore]"#; let c = \'#\'; }\n'
        "/* #[ignore] in a block comment */\n#[test]\nfn blocked<'a>() {}\n"
    )
    allow = f"# header comment\ntests/a_test.rs:bench_a{SEP}benchmark, run by hand\n"
    root = tree({"tests/a_test.rs": good_rs, ".github/ignored-tests.txt": allow})
    arm("ignores: every ignore listed, mentions in comments and strings not counted", check_ignores, (root,), 0,
        "1 found, 1 listed")
    root = tree({"tests/a_test.rs": good_rs + "#[test]\n#[ignore]\nfn first_screen_holds() {}\n",
                 ".github/ignored-tests.txt": allow})
    arm("ignores: an unlisted #[ignore] fails and is named", check_ignores, (root,), 1,
        "tests/a_test.rs:first_screen_holds is ignored but not on .github/ignored-tests.txt")
    root = tree({"src/x.rs": '#[cfg_attr(windows, ignore = "hangs")]\n#[test]\nfn on_windows() {}\n',
                 ".github/ignored-tests.txt": "# none\n"})
    arm("ignores: a cfg_attr ignore counts", check_ignores, (root,), 1, "src/x.rs:on_windows is ignored but not on")
    root = tree({"tests/a_test.rs": good_rs,
                 ".github/ignored-tests.txt": allow + f"tests/a_test.rs:gone{SEP}removed long ago\n"})
    arm("ignores: a listed test that no longer exists fails", check_ignores, (root,), 1,
        "tests/a_test.rs:gone is on .github/ignored-tests.txt but is not an ignored test")
    root = tree({"tests/a_test.rs": good_rs, ".github/ignored-tests.txt": "tests/a_test.rs:bench_a\n"})
    arm("ignores: an entry with no reason fails", check_ignores, (root,), 1, "is not '<file>:<test name>")

    wf_ok = (
        "jobs:\n  test:\n    runs-on: ubuntu-latest\n    timeout-minutes: 30\n    steps:\n"
        "      - name: cargo test\n        run: cargo test --no-fail-fast 2>&1 | tee cargo-test.log\n"
        "      # cargo test --skip in a comment is not a command\n"
        "  clippy:\n    runs-on: ubuntu-latest\n    timeout-minutes: 15\n    steps:\n"
        "      - run: cargo clippy --all-targets -- -D warnings\n"
    )
    root = tree({".github/workflows/ci.yml": wf_ok})
    arm("no-allowed-failure: absent passes", check_no_continue_on_error, (root,), 0, "absent from 1")
    root = tree({".github/workflows/ci.yml": wf_ok, ".github/workflows/other.yaml":
                 "jobs:\n  x:\n    # it was continue-on-error: true once\n    runs-on: x\n"})
    arm("no-allowed-failure: present anywhere, a comment included, fails", check_no_continue_on_error, (root,), 1,
        ".github/workflows/other.yaml:3 contains continue-on-error")
    root = tree({"README.md": "no workflows\n"})
    arm("no-allowed-failure: no workflow files fails", check_no_continue_on_error, (root,), 1, "read nothing")

    arm("no-test-filters: the plain command passes, a step name and a comment are not commands",
        check_no_test_filters, (tree({".github/workflows/ci.yml": wf_ok}),), 0, "cargo test --no-fail-fast: no filter")
    for label, cmd, says in [
        ("--skip after --", "cargo test --no-fail-fast -- --skip relay_wake", "--skip leaves tests out"),
        ("--exclude", "cargo test --workspace --exclude base", "--exclude leaves tests out"),
        ("a test name", "cargo test --no-fail-fast relay_wake", "'relay_wake' is a test name filter"),
        ("a name after --", "cargo test -- relay_wake --nocapture", "'relay_wake' is a test name filter"),
        ("a target filter", "cargo test --test deferral_test", "'--test' is not an argument"),
        ("a continued line", "cargo test \\\n          --no-fail-fast relay", "'relay' is a test name filter"),
    ]:
        root = tree({".github/workflows/ci.yml": wf_ok.replace(
            "cargo test --no-fail-fast 2>&1 | tee cargo-test.log", cmd + " 2>&1 | tee cargo-test.log")})
        arm(f"no-test-filters: {label} fails", check_no_test_filters, (root,), 1, says)
    root = tree({".github/workflows/ci.yml": "jobs:\n  a:\n    timeout-minutes: 5\n    steps:\n      - run: cargo build\n"})
    arm("no-test-filters: no cargo test at all fails", check_no_test_filters, (root,), 1, "read nothing")

    arm("timeouts: every job has one", check_timeouts, (tree({".github/workflows/ci.yml": wf_ok}),), 0,
        "all 2 job(s) in 1 workflow file(s)")
    root = tree({".github/workflows/ci.yml": wf_ok.replace("    timeout-minutes: 15\n", "")})
    arm("timeouts: a job without one fails and is named", check_timeouts, (root,), 1,
        ".github/workflows/ci.yml: job 'clippy' has no timeout-minutes")
    # BO-22: release.yml's jobs had none and nothing noticed, because only ci.yml was read.
    release_ok = (
        "on:\n  push:\n    tags:\n      - 'v*'\n\njobs:\n"
        "  docs-gate:\n    runs-on: ubuntu-latest\n    timeout-minutes: 15\n    steps:\n      - run: cargo test --bin base help_docs\n"
        "# a comment at column 0 between two jobs\n"
        "  build:\n    needs: docs-gate\n    runs-on: ${{ matrix.os }}\n    timeout-minutes: 30\n    steps:\n"
        "      - run: cargo build --release\n"
    )
    root = tree({".github/workflows/ci.yml": wf_ok, ".github/workflows/release.yml": release_ok})
    arm("timeouts: every job in every workflow file has one", check_timeouts, (root,), 0,
        "all 4 job(s) in 2 workflow file(s)")
    root = tree({".github/workflows/ci.yml": wf_ok,
                 ".github/workflows/release.yml": release_ok.replace("    timeout-minutes: 30\n", "")})
    arm("timeouts: a job without one in a second workflow file, after a column-0 comment, fails naming file and job", check_timeouts, (root,), 1,
        ".github/workflows/release.yml: job 'build' has no timeout-minutes", must_not_say="ci.yml: job")
    root = tree({".github/workflows/ci.yml": wf_ok, ".github/workflows/other.yaml": "on: push\n"})
    arm("timeouts: a workflow file with no jobs read fails", check_timeouts, (root,), 1,
        "no jobs found in .github/workflows/other.yaml")
    root = tree({".github/workflows/ci.yml": wf_ok, ".github/workflows/call.yml":
                 "jobs:\n  call:\n    uses: ./.github/workflows/ci.yml\n  own:\n    runs-on: x\n    steps: []\n"})
    arm("timeouts: a job calling a reusable workflow needs none, its neighbour still does", check_timeouts, (root,), 1,
        ".github/workflows/call.yml: job 'own' has no timeout-minutes", must_not_say="job 'call'")
    arm("timeouts: no workflow files fails", check_timeouts, (tree({"README.md": "no workflows\n"}),), 1, "read nothing")

    counts = "# comment\nlinux 12\nwindows 10\n"
    log = ("     Running unittests src/lib.rs\n\ntest result: ok. 7 passed; 0 failed; 1 ignored; 0 measured\n"
           "\x1b[32mtest result: ok. 5 passed; 0 failed; 0 ignored; 0 measured\x1b[0m\n")
    root = tree({".github/test-counts.txt": counts, "log.txt": log})
    arm("test-count: a matching sum passes, colored lines included", check_test_count, (root, "Linux", root / "log.txt"), 0,
        "12 passed in ok lines")
    arm("test-count: fewer than the file fails", check_test_count, (root, "windows", root / "log.txt"), 1,
        "windows: 12 tests passed, .github/test-counts.txt says 10")
    root = tree({".github/test-counts.txt": "linux 13\nwindows 10\n", "log.txt": log})
    arm("test-count: a dropped test fails", check_test_count, (root, "linux", root / "log.txt"), 1,
        "linux: 12 tests passed, .github/test-counts.txt says 13 (a PR that removes tests")
    root = tree({".github/test-counts.txt": counts, "log.txt": "error: could not compile\n"})
    arm("test-count: no result lines fails", check_test_count, (root, "linux", root / "log.txt"), 1, "nothing was counted")
    root = tree({".github/test-counts.txt": "linux 12\n", "log.txt": log})
    arm("test-count: a platform missing from the file fails", check_test_count, (root, "windows", root / "log.txt"), 1,
        "has no 'windows <n>' line")
    root = tree({".github/test-counts.txt": counts, "log.txt": log + "test result: FAILED. 3 passed; 1 failed; 0 ignored\n"})
    arm("test-count: a FAILED target fails even when the ok lines sum to the file", check_test_count,
        (root, "linux", root / "log.txt"), 1, "1 test target(s) reported FAILED")

    bad = [r for r in results if not r[1]]
    for name, ok, detail in results:
        print(f"{'ok  ' if ok else 'FAIL'} {name} ({detail.splitlines()[0]})")
        if not ok:
            print("\n".join("     " + l for l in detail.splitlines()[1:]))
    print(f"selftest: {len(results) - len(bad)} of {len(results)} arms landed where they must")
    return 1 if bad else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("check", choices=["ignores", "no-allowed-failure", "no-test-filters", "timeouts", "test-count", "selftest"])
    ap.add_argument("--root", type=Path, default=REPO)
    ap.add_argument("--platform", help="test-count: linux or windows (runner.os is accepted)")
    ap.add_argument("--log", type=Path, help="test-count: the cargo test output, stdout and stderr")
    a = ap.parse_args()
    if a.check == "selftest":
        return selftest()
    if a.check == "test-count":
        if not a.platform or not a.log:
            ap.error("test-count needs --platform and --log")
        return check_test_count(a.root, a.platform, a.log)
    return {
        "ignores": check_ignores,
        "no-allowed-failure": check_no_continue_on_error,
        "no-test-filters": check_no_test_filters,
        "timeouts": check_timeouts,
    }[a.check](a.root)


if __name__ == "__main__":
    sys.exit(main())
