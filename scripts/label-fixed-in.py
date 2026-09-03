#!/usr/bin/env python3
"""Label every issue closed since the previous release `fixed-in:<version>`.

That label, not the close, is what flips an entry on the docs site's Known
issues page from "Still live" to "Fixed in <version>". A close can mean the
report was a duplicate, or wrong, or went stale; the label means one thing.

Run by `scripts/release.sh` after the tag is pushed.

    scripts/label-fixed-in.py 0.13.16
    scripts/label-fixed-in.py 0.13.16 --dry-run
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("version")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    label = f"fixed-in:{args.version}"

    releases = gh(f"/repos/{REPO}/releases?per_page=100")
    published = sorted(r["published_at"] for r in releases if r.get("published_at"))
    if not published:
        print("no published releases to measure against; nothing to label")
        return 0
    since = published[-1]

    closed = gh(f"/repos/{REPO}/issues?state=closed&since={since}&per_page=100")
    targets = [
        i
        for i in closed
        if "pull_request" not in i
        and i.get("closed_at")
        and i["closed_at"] > since
        and not any(l["name"].startswith("fixed-in:") for l in i["labels"])
    ]

    print(f"{label}: {len(targets)} issue(s) closed since {since}")
    for i in targets:
        print(f"  #{i['number']} {i['title'][:60]}")
    if not targets:
        return 0
    if args.dry_run:
        print("--dry-run: nothing labelled")
        return 0

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
