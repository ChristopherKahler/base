#!/usr/bin/env python3
"""Read a bug report, check it against this tree, and label it.

Applies exactly one of `confirmed`, `needs-info` or `not-a-bug`, and posts one
comment saying how it got there. It never closes an issue, and it stops
immediately on an issue carrying `human-verdict`, which is how a person freezes
automation on a report.

The label is the whole point: the docs site renders its Known issues page from
`bug` + `confirmed` issues, so this is what puts a real bug in front of readers
and keeps a misfiled one off the page.

    ISSUE_NUMBER=42 GITHUB_REPOSITORY=owner/repo scripts/triage-issue.py
    scripts/triage-issue.py --issue 42 --dry-run

Credentials, in order: ANTHROPIC_API_KEY, else CLAUDE_CODE_OAUTH_TOKEN (a
subscription token, passed as ANTHROPIC_AUTH_TOKEN with the oauth beta header).
With neither, it exits nonzero and says so rather than labelling nothing.
"""

import argparse
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

import anthropic

MODEL = "claude-opus-5"
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

Judge only what the report says against what the reference shows. You cannot run anything. When \
the reference does not settle it, that is usually needs-info rather than a guess.

Write for the person who filed it. Plain sentences, no headings, no bullet lists, under 150 \
words. Say what you checked and what you concluded. Never claim to have reproduced anything."""

TOOL = {
    "name": "record_verdict",
    "description": "Record the triage verdict and the reasoning to post as a comment.",
    "strict": True,
    "input_schema": {
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": list(VERDICTS)},
            "reasoning": {"type": "string", "description": "The comment to post. Plain sentences."},
        },
        "required": ["verdict", "reasoning"],
        "additionalProperties": False,
    },
}


def gh(path, method="GET", payload=None):
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN", "")
    if not token:
        sys.exit("no GH_TOKEN in the environment")
    req = urllib.request.Request(
        "https://api.github.com" + path,
        method=method,
        data=json.dumps(payload).encode() if payload is not None else None,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        sys.exit(f"github said {e.code} for {method} {path}: {e.read().decode()[:300]}")


def client():
    if os.environ.get("ANTHROPIC_API_KEY"):
        return anthropic.Anthropic()
    oauth = os.environ.get("CLAUDE_CODE_OAUTH_TOKEN")
    if oauth:
        # A Claude Code subscription token authenticates as a bearer token and
        # needs the oauth beta header. Produced by `claude setup-token`.
        os.environ["ANTHROPIC_AUTH_TOKEN"] = oauth
        return anthropic.Anthropic(default_headers={"anthropic-beta": "oauth-2025-04-20"})
    sys.exit(
        "no Anthropic credential. Set ANTHROPIC_API_KEY, or CLAUDE_CODE_OAUTH_TOKEN from "
        "`claude setup-token`, in this repository's Actions secrets. Triage does nothing until "
        "one exists; it will not label an issue it has not read."
    )


def reference():
    """The generated command reference for this tree, which is what makes a
    verdict about whether a command exists checkable rather than recalled."""
    cli = BANK / "cli.md"
    if not cli.is_file():
        sys.exit(f"no command reference at {cli}; run this from a checkout of the repository")
    text = cli.read_text(encoding="utf-8")
    version = subprocess.run(
        ["sed", "-n", "s/^version = \"\\(.*\\)\"/\\1/p", str(ROOT / "Cargo.toml")],
        capture_output=True,
        text=True,
    ).stdout.split("\n")[0]
    return version, text


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--issue", type=int, default=int(os.environ.get("ISSUE_NUMBER", 0)))
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    if not args.issue:
        sys.exit("no issue number: pass --issue or set ISSUE_NUMBER")

    issue = gh(f"/repos/{REPO}/issues/{args.issue}")
    labels = {l["name"] for l in issue["labels"]}
    if FROZEN in labels:
        print(f"#{args.issue} carries `{FROZEN}`; a person has ruled on it. Nothing to do.")
        return 0
    if "bug" not in labels:
        print(f"#{args.issue} is not labelled `bug`. Nothing to do.")
        return 0

    version, cli_md = reference()
    prompt = (
        f"base {version}\n\n"
        f"## The report\n\n### {issue['title']}\n\n{issue.get('body') or '(no body)'}\n\n"
        f"## The command reference for this release\n\n{cli_md}\n\n"
        "Reach a verdict and record it with the record_verdict tool."
    )

    message = client().messages.create(
        model=MODEL,
        max_tokens=8000,
        thinking={"type": "adaptive"},
        system=SYSTEM,
        tools=[TOOL],
        messages=[{"role": "user", "content": prompt}],
    )

    call = next((b for b in message.content if b.type == "tool_use"), None)
    if call is None:
        said = " ".join(b.text for b in message.content if b.type == "text")
        sys.exit(f"no verdict recorded (stop_reason={message.stop_reason}). It said: {said[:400]}")
    verdict = call.input["verdict"]
    reasoning = call.input["reasoning"]

    print(f"#{args.issue}: {verdict}\n\n{reasoning}\n")
    if args.dry_run:
        print("--dry-run: nothing posted")
        return 0

    gh(
        f"/repos/{REPO}/issues/{args.issue}/comments",
        "POST",
        {"body": f"{reasoning}\n\n<sub>Triaged automatically against base {version}. "
                 f"A maintainer can overrule this by applying the `{FROZEN}` label.</sub>"},
    )
    # Exactly one verdict label: drop the other two before adding this one, so a
    # re-triage after an edit does not leave two contradicting labels.
    for stale in (v for v in VERDICTS if v != verdict and v in labels):
        gh(f"/repos/{REPO}/issues/{args.issue}/labels/{stale}", "DELETE")
    gh(f"/repos/{REPO}/issues/{args.issue}/labels", "POST", {"labels": [verdict]})
    print("comment posted, label applied")
    return 0


if __name__ == "__main__":
    sys.exit(main())
