#!/usr/bin/env python3
"""Label every issue closed between the previous release and this one `fixed-in:<version>`.

That label, not the close, is what flips an entry on the docs site's Known
issues page from "Still live" to "Fixed in <version>". A close can mean the
report was a duplicate, or wrong, or went stale; the label means one thing.

Run by `scripts/release.sh` after the tag is pushed. Safe to re-run for a version
that is already published: the window is then bounded by that release's own
publish time, so a later re-run labels the same issues and nothing newer (#57).

    scripts/label-fixed-in.py 0.13.16
    scripts/label-fixed-in.py 0.13.16 --dry-run
    python3 -m doctest scripts/label-fixed-in.py     # the window arithmetic
"""

import argparse
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request

REPO = os.environ.get("GITHUB_REPOSITORY", "ChristopherKahler/base")


def token():
    if os.environ.get("GH_TOKEN"):
        return os.environ["GH_TOKEN"]
    out = subprocess.run(
        ["git", "credential", "fill"],
        input="protocol=https\nhost=github.com\n\n",
        capture_output=True,
        text=True,
        env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": os.environ.get("HOME", "")},
        timeout=30,
    ).stdout
    for line in out.splitlines():
        if line.startswith("password="):
            return line[len("password=") :]
    sys.exit("no GitHub credential: set GH_TOKEN, or configure a git credential helper")


def gh(path, method="GET", payload=None):
    req = urllib.request.Request(
        "https://api.github.com" + path,
        method=method,
        data=json.dumps(payload).encode() if payload is not None else None,
        headers={
            "Authorization": f"Bearer {token()}",
            "Accept": "application/vnd.github+json",
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        if method == "POST" and e.code == 422:
            return {}  # the label already exists
        sys.exit(f"github said {e.code} for {method} {path}: {e.read().decode()[:300]}")


def window(releases, version):
    """The (since, until) publish-time window that `fixed-in:<version>` covers.

    `since` is the newest publish time before this version's own release; `until`
    is this version's publish time when it is already published, else None (now).
    Taking the newest release of all as `since` was the defect in #57: once the
    Release workflow has published v<version>, that is the newest release, the
    window starts at the release being labelled, and every issue it fixed falls
    before it.

    Release publish times are ISO-8601 UTC strings, so string order is time order.

    >>> rel = [{"tag_name": "v1.1", "published_at": "2026-09-05T13:33:54Z"},
    ...        {"tag_name": "v1.0", "published_at": "2026-09-04T16:13:49Z"}]
    >>> window(rel, "1.2")                       # not yet published: since = newest
    ('2026-09-05T13:33:54Z', None)
    >>> window(rel + [{"tag_name": "v1.2", "published_at": "2026-09-05T16:20:43Z"}], "1.2")
    ('2026-09-05T13:33:54Z', '2026-09-05T16:20:43Z')
    >>> window(rel, "1.1")                       # re-run for the newest release: previous, not itself
    ('2026-09-04T16:13:49Z', '2026-09-05T13:33:54Z')
    >>> window([{"tag_name": "v1.0", "published_at": "2026-09-04T16:13:49Z"}], "1.0")
    (None, '2026-09-04T16:13:49Z')
    >>> window([{"tag_name": "v1.0", "published_at": None}], "1.1")
    (None, None)
    """
    tag = f"v{version}"
    dated = sorted(
        ((r["published_at"], r.get("tag_name")) for r in releases if r.get("published_at")),
        key=lambda x: x[0],
    )
    this = [p for p, t in dated if t == tag]
    until = this[0] if this else None
    before = [p for p, t in dated if t != tag and (until is None or p < until)]
    since = before[-1] if before else None
    return since, until


def in_window(closed_at, since, until):
    """
    >>> in_window("2026-09-05T13:56:02Z", "2026-09-05T13:33:54Z", "2026-09-05T16:20:43Z")
    True
    >>> in_window("2026-09-05T13:07:23Z", "2026-09-05T13:33:54Z", "2026-09-05T16:20:43Z")
    False
    >>> in_window("2026-09-07T13:31:23Z", "2026-09-05T13:33:54Z", "2026-09-05T16:20:43Z")
    False
    >>> in_window("2026-09-07T13:31:23Z", "2026-09-05T16:20:43Z", None)
    True
    >>> in_window("2026-09-04T09:00:00Z", None, "2026-09-04T16:13:49Z")
    True
    """
    return (since is None or closed_at > since) and (until is None or closed_at <= until)


NOT_A_FIX = {"question", "duplicate", "wontfix", "invalid", "not-a-bug"}


def labelable(issue):
    """A closed issue in the window that a release can honestly call fixed.

    Pull requests are not issues; an issue already carrying a `fixed-in:` label
    keeps the release that first shipped it; and a close that the tracker itself
    says was not a fix (a question answered, a duplicate, a report ruled out) is
    not one either. The docstring at the top has said the last part since the
    script was written; the code did not check it until #57.

    >>> labelable({"closed_at": "x", "labels": [{"name": "bug"}]})
    True
    >>> labelable({"closed_at": "x", "labels": [{"name": "bug"}], "pull_request": {}})
    False
    >>> labelable({"closed_at": "x", "labels": [{"name": "fixed-in:0.13.17"}]})
    False
    >>> labelable({"closed_at": "x", "labels": [{"name": "question"}]})
    False
    >>> labelable({"closed_at": None, "labels": []})
    False
    """
    if "pull_request" in issue or not issue.get("closed_at"):
        return False
    names = {l["name"] for l in issue.get("labels", [])}
    if any(n.startswith("fixed-in:") for n in names):
        return False
    return not (names & NOT_A_FIX)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("version")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    label = f"fixed-in:{args.version}"

    releases = gh(f"/repos/{REPO}/releases?per_page=100")
    since, until = window(releases, args.version)
    if since is None and until is None:
        print("no published releases to measure against; nothing to label")
        return 0

    query = f"/repos/{REPO}/issues?state=closed&per_page=100" + (f"&since={since}" if since else "")
    closed = gh(query)
    targets = [i for i in closed if labelable(i) and in_window(i["closed_at"], since, until)]

    print(f"{label}: {len(targets)} issue(s) closed after {since or 'the beginning'} and up to {until or 'now'}")
    for i in targets:
        print(f"  #{i['number']} {i['title'][:60]}")
    if args.dry_run:
        print("--dry-run: nothing labelled")
        return 0

    # The label exists after every run, targets or not: an empty label on the repo
    # says "this release closed nothing", where no label at all said nothing (#57).
    gh(
        f"/repos/{REPO}/labels",
        "POST",
        {"name": label, "color": "0E8A16", "description": f"Closed and shipped in {args.version}"},
    )
    for i in targets:
        gh(f"/repos/{REPO}/issues/{i['number']}/labels", "POST", {"labels": [label]})
    print(f"labelled {len(targets)} issue(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
