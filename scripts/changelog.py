#!/usr/bin/env python3
"""Write CHANGELOG.md sections from the commits between two release tags.

One generator, two callers. `scripts/release.sh` runs `section` on every release
and prepends the result; the historical 0.13.3-0.13.15 backfill was produced by
`file` over the same code. That is deliberate: if the backfill had been typed by
hand, the first generated entry would have looked nothing like the entries above
it and nobody would have noticed until it shipped.

    scripts/changelog.py section v0.13.15 HEAD 0.13.16 [--date D] [--prepend F]
    scripts/changelog.py file v0.13.2 v0.13.3 ... v0.13.15 > CHANGELOG.md
    scripts/changelog.py extract 0.13.16 > release-notes.md

Classification of each commit subject, which is why the repo writes conventional
subjects:

    feat(scope): ...        Added
    fix(scope): ...         Fixed, with every issue its body closes linked
    Revert "<subject>"      dropped, together with the commit it names
    chore(release): ...     dropped
    test(...): ...          dropped
    Merge ...               dropped
    anything else           Changed

A revert only cancels its target when both are being rendered together. When the
reverted feature shipped in a release that is already published, the revert is
reported as a Changed line instead, because that release's entry is history a
reader has already seen.

Release notes written by hand for a specific version live in `scripts/changelog-notes/<version>.md`
and are emitted as that section's opening paragraph. Everything else in a section
is derived from git.
"""

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

REPO_URL = "https://github.com/ChristopherKahler/base"

HEADER = """# Changelog

Every released version of base, newest first. Each entry is generated from the commits
between that release's tag and the one before it, by `scripts/changelog.py`.
`scripts/release.sh` writes the top section when it cuts a release, and `cargo test`
fails when the version in `Cargo.toml` has no entry here.

Releases before 0.13.3 are tagged in the repository but are not written up.
"""

SECTIONS = ("Added", "Fixed", "Changed")

# `Closes #11, closes #12, closes #13.` and `Closes #11, #12, #13.` both yield
# 11, 12, 13. A line like `Also closes a latent bug: ...` carries no `#N` and so
# contributes nothing, which is the point of reading numbers rather than verbs.
CLOSES_LINE = re.compile(r"\b(?:close[sd]?|fixe[sd]?|resolve[sd]?)\b", re.I)
ISSUE = re.compile(r"#(\d+)")
CONVENTIONAL = re.compile(r"^(?P<type>[a-z]+)(?:\((?P<scope>[^)]*)\))?!?:\s*(?P<rest>.+)$")
REVERT = re.compile(r'^Revert "(?P<subject>.+)"\s*$')


def git(*args):
    """Run git with an explicit environment: inheriting the caller's leaves the
    output at the mercy of whatever locale or pager the parent happened to set,
    and this script parses that output."""
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": os.environ.get("HOME", ""),
        "GIT_DIR": os.environ["GIT_DIR"] if "GIT_DIR" in os.environ else "",
        "LC_ALL": "C",
        "GIT_PAGER": "cat",
        "TZ": "UTC",
    }
    env = {k: v for k, v in env.items() if v}
    out = subprocess.run(
        ["git", *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        timeout=120,
    )
    if out.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed:\n{out.stderr.strip()}")
    return out.stdout


def commits(from_ref, to_ref):
    """Every commit in (from_ref, to_ref], oldest last, as dicts."""
    raw = git("log", "--format=%H%x1f%s%x1f%b%x1e", f"{from_ref}..{to_ref}")
    out = []
    for record in raw.split("\x1e"):
        record = record.strip("\n")
        if not record:
            continue
        sha, subject, body = record.split("\x1f")
        out.append({"sha": sha, "subject": subject.strip(), "body": body})
    return out


def closed_issues(body):
    """Issue numbers this commit says it closes, in the order written, deduplicated."""
    found = []
    for line in body.splitlines():
        if not CLOSES_LINE.search(line):
            continue
        for n in ISSUE.findall(line):
            if n not in found:
                found.append(n)
    return found


def classify(subject):
    """(bucket, text) for one subject, or (None, None) when it is not written up."""
    m = CONVENTIONAL.match(subject)
    if m:
        kind, scope, rest = m.group("type"), m.group("scope"), m.group("rest")
        if kind == "test":
            return None, None
        if kind == "chore" and scope == "release":
            return None, None
        text = f"**{scope}**: {rest}" if scope else rest
        if kind == "feat":
            return "Added", text
        if kind == "fix":
            return "Fixed", text
        return "Changed", text
    if subject.startswith("Merge "):
        return None, None
    return "Changed", subject


def entry(commit, cancelled_subjects):
    """One rendered bullet, or None when this commit is not written up."""
    subject = commit["subject"]

    revert = REVERT.match(subject)
    if revert:
        target = revert.group("subject")
        if target in cancelled_subjects:
            return None, None
        bucket, text = classify(target)
        if bucket is None:
            return None, None
        return "Changed", f"reverted: {text}"

    # The other half of a cancelled pair. A feature that was reverted inside the
    # set being rendered never reaches a reader, so writing it up as shipped and
    # then not mentioning its removal would be worse than saying nothing.
    if subject in cancelled_subjects:
        return None, None

    bucket, text = classify(subject)
    if bucket is None or text is None:
        return None, None

    # An issue the subject already names becomes a link in place; the rest are
    # appended. Either way every issue number in the entry is clickable, and no
    # number is printed twice.
    named = set(ISSUE.findall(text))
    text = ISSUE.sub(lambda m: f"[#{m.group(1)}]({REPO_URL}/issues/{m.group(1)})", text)
    links = [
        f"[#{n}]({REPO_URL}/issues/{n})" for n in closed_issues(commit["body"]) if n not in named
    ]
    if links:
        text = f"{text} ({', '.join(links)})"
    return bucket, text


def reverted_subjects(all_commits):
    """Subjects named by a `Revert "..."` commit anywhere in the rendered set."""
    named = set()
    present = {c["subject"] for c in all_commits}
    for c in all_commits:
        m = REVERT.match(c["subject"])
        if m and m.group("subject") in present:
            named.add(m.group("subject"))
    return named


def tag_date(ref):
    out = git("for-each-ref", "--format=%(creatordate:short)", f"refs/tags/{ref}").strip()
    return out or git("log", "-1", "--format=%cd", "--date=short", ref).strip()


def notes_for(version, notes_dir):
    if not notes_dir:
        return ""
    path = Path(notes_dir) / f"{version}.md"
    if not path.exists():
        return ""
    return path.read_text(encoding="utf-8").strip()


def render(version, date, commit_list, cancelled, notes):
    buckets = {name: [] for name in SECTIONS}
    for c in commit_list:
        bucket, text = entry(c, cancelled)
        if bucket:
            buckets[bucket].append(text)

    out = [f"## {version} ({date})", ""]
    if notes:
        out += [notes, ""]
    wrote = False
    for name in SECTIONS:
        if not buckets[name]:
            continue
        wrote = True
        out.append(f"### {name}")
        out.append("")
        out += [f"- {t}" for t in buckets[name]]
        out.append("")
    if not wrote and not notes:
        out += ["*No user-visible changes.*", ""]
    return "\n".join(out)


def cmd_section(args):
    commit_list = commits(args.from_ref, args.to_ref)
    date = args.date or tag_date(args.to_ref)
    block = render(
        args.version,
        date,
        commit_list,
        reverted_subjects(commit_list),
        notes_for(args.version, args.notes_dir),
    )
    if not args.prepend:
        print(block)
        return
    path = Path(args.prepend)
    existing = path.read_text(encoding="utf-8") if path.exists() else HEADER
    head, sep, rest = existing.partition("\n## ")
    body = f"{head.rstrip()}\n\n{block.rstrip()}\n{sep}{rest}" if sep else f"{head.rstrip()}\n\n{block.rstrip()}"
    path.write_text(body.rstrip() + "\n", encoding="utf-8")
    print(f"{path}: wrote the {args.version} section", file=sys.stderr)


def cmd_file(args):
    tags = args.tags
    if len(tags) < 2:
        sys.exit("file needs at least two tags: the one before the first entry, then each release")

    ranges = list(zip(tags, tags[1:]))
    every = [c for a, b in ranges for c in commits(a, b)]
    cancelled = reverted_subjects(every)

    blocks = []
    for from_ref, to_ref in ranges:
        version = to_ref.lstrip("v")
        blocks.append(
            render(
                version,
                args.date or tag_date(to_ref),
                commits(from_ref, to_ref),
                cancelled,
                notes_for(version, args.notes_dir),
            )
        )
    print(HEADER.rstrip() + "\n\n" + "\n".join(reversed(blocks)).rstrip() + "\n", end="")


def cmd_extract(args):
    """The one section for a version, verbatim, for the GitHub release body."""
    text = Path(args.file).read_text(encoding="utf-8")
    want = f"## {args.version} "
    lines = text.splitlines()
    start = next((i for i, l in enumerate(lines) if l.startswith(want)), None)
    if start is None:
        sys.exit(
            f"{args.file} has no section for {args.version}. "
            "`cargo test` gates this on every release, so a tag that reaches here without one "
            "was not cut by scripts/release.sh."
        )
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")), len(lines))
    print("\n".join(lines[start:end]).rstrip())


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--notes-dir", default=str(Path(__file__).parent / "changelog-notes"))
    p.add_argument("--date", help="override the release date (default: the tag's own date)")
    sub = p.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("section", help="one release's block")
    s.add_argument("from_ref")
    s.add_argument("to_ref")
    s.add_argument("version")
    s.add_argument("--prepend", metavar="FILE", help="insert into FILE under its header")
    s.set_defaults(func=cmd_section)

    f = sub.add_parser("file", help="the whole file, from a list of consecutive tags")
    f.add_argument("tags", nargs="+")
    f.set_defaults(func=cmd_file)

    e = sub.add_parser("extract", help="print one version's existing section")
    e.add_argument("version")
    e.add_argument("--file", default="CHANGELOG.md")
    e.set_defaults(func=cmd_extract)

    args = p.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
