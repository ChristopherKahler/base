#!/usr/bin/env python3
"""Read the bug reports nobody has ruled on yet, check them against this tree, and label them.

Applies exactly one of `confirmed`, `needs-info` or `not-a-bug`, and posts one comment saying how
it got there. It never closes an issue, and it stops immediately on an issue carrying
`human-verdict`, which is how a person freezes automation on a report.

The label is the whole point: the docs site renders its Known issues page from `bug` + `confirmed`
issues, so this is what puts a real bug in front of readers and keeps a misfiled one off the page.

    scripts/triage-issue.py --poll              every untriaged open bug report
    scripts/triage-issue.py --issue 42          one report, triaged or not
    scripts/triage-issue.py --poll --dry-run    the verdicts, posting nothing

The reader is `claude -p`, the Claude Code CLI, which authenticates as whoever is logged in on the
machine running it. No API key and no model credential in CI: a maintainer's own subscription reads
the report. That is also why this polls from a machine rather than running on a GitHub runner —
a runner has no Claude login, and giving it one means putting a billable credential in a repository
secret to answer a handful of reports a week.

GitHub credential: `GH_TOKEN`, else whatever `git credential fill` already holds for github.com.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

MODEL = os.environ.get("TRIAGE_MODEL", "opus")
REPO = os.environ.get("GITHUB_REPOSITORY", "ChristopherKahler/base")
ROOT = Path(__file__).resolve().parent.parent
BANK = ROOT / "claude" / "skills" / "base-help" / "references"

VERDICTS = ("confirmed", "needs-info", "not-a-bug")
FROZEN = "human-verdict"

SYSTEM = """You triage bug reports for base, a Rust CLI that injects context into coding-agent \
sessions from a local knowledge graph.

You are given a report and the release's own command reference, which is generated from the \
binary and is therefore exact about what commands and flags exist.

Reach one of three verdicts.

confirmed - the report describes real behaviour that base should not have. You could point at \
the surface it concerns and the described behaviour is consistent with it. A design choice that \
surprises people is still confirmed; say so in your reasoning.

needs-info - you cannot tell from what was written. A missing version, no way to reproduce, or a \
description that could be several different problems. Say exactly what would settle it.

not-a-bug - the command or flag does not exist, the behaviour is documented and intended, or the \
report is about something outside base.

Judge only what the report says against what the reference shows. You cannot run anything, and you \
must not try: no tools, no files, no commands. When the reference does not settle it, that is \
usually needs-info rather than a guess.

Write for the person who filed it. Plain sentences, no headings, no bullet lists, under 150 \
words. Say what you checked and what you concluded. Never claim to have reproduced anything.

Reply with one JSON object and nothing else, no code fence:
{"verdict": "confirmed|needs-info|not-a-bug", "reasoning": "the comment to post"}"""


def gh(path, method="GET", payload=None):
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN") or git_credential()
    if not token:
        sys.exit("no GitHub credential: set GH_TOKEN, or log git in to github.com")
    req = urllib.request.Request(
        "https://api.github.com" + path,
        method=method,
        data=json.dumps(payload).encode() if payload is not None else None,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "Content-Type": "application/json",
            "User-Agent": "base triage",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            body = r.read()
            return json.loads(body) if body else {}
    except urllib.error.HTTPError as e:
        sys.exit(f"github said {e.code} for {method} {path}: {e.read().decode()[:300]}")


def git_credential():
    """The credential git already holds for github.com, so a local run needs no setup."""
    try:
        out = subprocess.run(
            ["git", "credential", "fill"],
            input="protocol=https\nhost=github.com\n\n",
            capture_output=True,
            text=True,
            timeout=30,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return ""
    for line in out.splitlines():
        if line.startswith("password="):
            return line[len("password=") :]
    return ""


def reference():
    """The generated command reference for this tree, which is what makes a
    verdict about whether a command exists checkable rather than recalled."""
    cli = BANK / "cli.md"
    if not cli.is_file():
        sys.exit(f"no command reference at {cli}; run this from a checkout of the repository")
    version = ""
    for line in (ROOT / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("version = "):
            version = line.split('"')[1]
            break
    return version, cli.read_text(encoding="utf-8")


def read_report(prompt):
    """One `claude -p` turn, returning (verdict, reasoning).

    Every tool is denied: this is a reading task, and a triage run that edits a
    file or runs a command is a triage run that has escaped its job.
    """
    exe = shutil.which("claude")
    if not exe:
        sys.exit("no `claude` on PATH. Triage reads reports with the Claude Code CLI.")
    try:
        # The prompt goes in on stdin, never as an argument. It carries the whole
        # command reference — 62 KB at 0.13.16 — and Windows caps a command line
        # at 32767 characters, so passing it as argv fails there and nowhere else.
        proc = subprocess.run(
            [
                exe, "-p",
                "--append-system-prompt", SYSTEM,
                "--model", MODEL,
                "--output-format", "json",
                "--disallowedTools", "Bash,Read,Write,Edit,NotebookEdit,WebFetch,WebSearch,Glob,Grep,Task",
            ],
            input=prompt,
            capture_output=True,
            text=True,
            # Pinned, not inherited: Windows hands a pipe cp1252 by default and the
            # command reference has an arrow in it, which then fails to encode.
            encoding="utf-8",
            errors="replace",
            timeout=600,
        )
    except OSError as e:
        sys.exit(f"could not run {exe}: {e}")
    except subprocess.TimeoutExpired:
        sys.exit("claude did not answer within ten minutes")
    if proc.returncode != 0:
        sys.exit(f"claude exited {proc.returncode}: {(proc.stderr or proc.stdout)[:300]}")

    envelope = json.loads(proc.stdout)
    if envelope.get("is_error"):
        sys.exit(f"claude reported an error: {str(envelope.get('result'))[:300]}")
    said = envelope.get("result", "")

    match = re.search(r"\{.*\}", said, re.S)
    if not match:
        sys.exit(f"no verdict in what claude said: {said[:300]}")
    try:
        answer = json.loads(match.group(0))
    except json.JSONDecodeError as e:
        sys.exit(f"claude's verdict is not JSON ({e}): {match.group(0)[:300]}")

    verdict = answer.get("verdict")
    reasoning = (answer.get("reasoning") or "").strip()
    if verdict not in VERDICTS or not reasoning:
        sys.exit(f"claude returned {verdict!r} with {len(reasoning)} characters of reasoning")
    return verdict, reasoning


def untriaged():
    """Open `bug` reports carrying no verdict and no `human-verdict`, oldest first.

    A machine that was off for a day catches up on everything it missed, which is
    the whole reason this polls instead of firing on the issue event.
    """
    issues = gh(f"/repos/{REPO}/issues?state=open&labels=bug&per_page=100&sort=created&direction=asc")
    out = []
    for i in issues:
        if "pull_request" in i:
            continue
        labels = {l["name"] for l in i["labels"]}
        if labels & {FROZEN, *VERDICTS}:
            continue
        out.append(i)
    return out


def triage(issue, version, cli_md, dry_run):
    number = issue["number"]
    labels = {l["name"] for l in issue["labels"]}
    if FROZEN in labels:
        print(f"#{number} carries `{FROZEN}`; a person has ruled on it. Nothing to do.")
        return
    if "bug" not in labels:
        print(f"#{number} is not labelled `bug`. Nothing to do.")
        return

    prompt = (
        f"base {version}\n\n"
        f"## The report\n\n### {issue['title']}\n\n{issue.get('body') or '(no body)'}\n\n"
        f"## The command reference for this release\n\n{cli_md}\n\n"
        "Reach a verdict and reply with the JSON object."
    )
    verdict, reasoning = read_report(prompt)
    print(f"#{number}: {verdict}\n\n{reasoning}\n")
    if dry_run:
        print("--dry-run: nothing posted\n")
        return

    gh(
        f"/repos/{REPO}/issues/{number}/comments",
        "POST",
        {
            "body": f"{reasoning}\n\n<sub>Triaged automatically against base {version}. "
            f"A maintainer can overrule this by applying the `{FROZEN}` label.</sub>"
        },
    )
    # Exactly one verdict label: drop the other two before adding this one, so a
    # re-triage after an edit does not leave two contradicting labels.
    for stale in (v for v in VERDICTS if v != verdict and v in labels):
        gh(f"/repos/{REPO}/issues/{number}/labels/{stale}", "DELETE")
    gh(f"/repos/{REPO}/issues/{number}/labels", "POST", {"labels": [verdict]})
    print(f"#{number}: comment posted, `{verdict}` applied\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--issue", type=int, default=int(os.environ.get("ISSUE_NUMBER", 0)))
    ap.add_argument("--poll", action="store_true", help="every untriaged open bug report")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    if not args.issue and not args.poll:
        sys.exit("nothing to do: pass --poll, or --issue N")

    version, cli_md = reference()

    if args.issue:
        triage(gh(f"/repos/{REPO}/issues/{args.issue}"), version, cli_md, args.dry_run)
        return 0

    pending = untriaged()
    if not pending:
        print(f"{REPO}: no untriaged bug reports.")
        return 0
    print(f"{REPO}: {len(pending)} untriaged bug report(s): {[i['number'] for i in pending]}\n")
    for issue in pending:
        triage(issue, version, cli_md, args.dry_run)
    return 0


if __name__ == "__main__":
    sys.exit(main())
