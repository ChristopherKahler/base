#!/usr/bin/env bash
# Release base through a pull request: prepare the release on a branch, open its PR, tag the merge.
#
#   scripts/release.sh 0.16.0            on new branch release/v0.16.0: bump + changelog + coach + full suite + commit, no tag
#   scripts/release.sh 0.16.0 --push     ...then push release/v0.16.0 and open its PR to main (needs gh)
#   scripts/release.sh 0.16.0 --quick    gate on the help_docs tests only, skip the full suite
#   scripts/release.sh 0.16.0 --tag      once that PR has merged: tag its merge commit and push only the tag (needs gh)
#
# main is protected (a PR, every required check green, admins included), so nothing here pushes
# main. The tag push starts the Release workflow, which builds the binaries.
#
# Why a script: 0.13.13 shipped a Cargo.lock that cargo could not parse (a hand
# bump), twelve releases shipped a base-help coach stamped v0.13.2 because
# nothing regenerated it, and thirteen shipped with no changelog entry at all.
# Every step here is one that was skipped at least once.
set -euo pipefail

usage() { sed -n '2,10p' "$0"; exit 2; }
[ $# -ge 1 ] || usage
NEW="$1"; shift
PUSH=0; QUICK=0; TAG=0
for a in "$@"; do
  case "$a" in
    --push) PUSH=1 ;;
    --quick) QUICK=1 ;;
    --tag) TAG=1 ;;
    *) usage ;;
  esac
done
[[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not a version: $NEW"; exit 2; }
if [ "$TAG" = 1 ] && [ "$PUSH$QUICK" != 00 ]; then
  echo "--tag runs on its own, once the release PR has merged"; exit 2
fi
RELEASE_BRANCH="release/v$NEW"

cd "$(git rev-parse --show-toplevel)"

# gh opens the PR and reads it back. Preparing alone never calls it. Checked before anything is
# written, so a missing or logged-out gh costs nothing.
need_gh() {
  command -v gh >/dev/null 2>&1 || { echo "$1 needs the GitHub CLI, and gh is not on PATH"; exit 2; }
  gh auth status --hostname "${GH_HOST:-github.com}" >/dev/null 2>&1 ||
    { echo "$1 needs gh logged in to ${GH_HOST:-github.com}, and gh auth status says it is not; run: gh auth login"; exit 2; }
}

# ── --tag: publish a merged release ─────────────────────────────────────────
# The tag is the step that publishes: its push starts the Release workflow. So it goes on the
# commit the release PR merged as, read from GitHub rather than taken from origin/main's head,
# which may have moved on since; and only once that commit carries this version, its changelog
# section and a green result on every check run. No flag or variable skips any of this.
if [ "$TAG" = 1 ]; then
  need_gh --tag
  # gh's JSON is read from stdout alone; what it says on stderr is shown only when it fails.
  GH_ERR=$(mktemp)
  trap 'rm -f "$GH_ERR"' EXIT
  if ! pr=$(gh pr view "$RELEASE_BRANCH" --json state,mergeCommit,baseRefName,url 2>"$GH_ERR"); then
    echo "no PR found for $RELEASE_BRANCH; prepare and open one with: scripts/release.sh $NEW --push"
    sed 's/^/    /' "$GH_ERR"
    exit 1
  fi
  if ! fields=$(python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d.get("state") or "-", (d.get("mergeCommit") or {}).get("oid") or "-", d.get("baseRefName") or "-", d.get("url") or "-")
' <<<"$pr" 2>&1); then
    echo "could not read gh's answer about $RELEASE_BRANCH:"
    printf '    %s\n' "$pr" "$fields"
    exit 1
  fi
  read -r state sha base url <<<"$fields"
  [ "$state" = MERGED ] || { echo "PR for $RELEASE_BRANCH is $state, not MERGED; merge it first: $url"; exit 1; }
  [ "$base" = main ] || { echo "PR for $RELEASE_BRANCH merged into $base, not main: $url"; exit 1; }
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo "PR for $RELEASE_BRANCH is MERGED but gh names no merge commit: $url"; exit 1; }
  # --no-tags: the tag checks below read this clone and origin separately, and a fetch that
  # followed tags would turn origin's into this clone's before either was read.
  git fetch --quiet --no-tags origin || { echo "could not fetch origin, so merge commit $sha cannot be read"; exit 1; }
  git cat-file -e "$sha^{commit}" 2>/dev/null || { echo "merge commit $sha is not in this clone after fetching origin"; exit 1; }
  short=$(git rev-parse --short "$sha")
  git merge-base --is-ancestor "$sha" origin/main ||
    { echo "not tagging $short: GitHub says $RELEASE_BRANCH merged as $short, but origin/main does not contain it"; exit 1; }
  echo "==> $RELEASE_BRANCH merged as $short"

  at_version=$(git show "$sha:Cargo.toml" 2>/dev/null | sed -n 's/^version = "\(.*\)"/\1/p' | head -1) || at_version=""
  [ "$at_version" = "$NEW" ] || { echo "not tagging $short: its Cargo.toml says ${at_version:-no version}, not $NEW"; exit 1; }
  changelog=$(git show "$sha:CHANGELOG.md" 2>/dev/null) || changelog=""
  awk -v want="## $NEW " 'index($0, want) == 1 { found = 1 } END { exit !found }' <<<"$changelog" ||
    { echo "not tagging $short: its CHANGELOG.md has no '## $NEW' section"; exit 1; }
  # The section was written when the release was prepared. Anything merged into main while its
  # PR was open is in the merge commit too, and would ship listed in no release's notes, since the
  # next section counts from this tag. So every commit the merge commit releases is rendered the
  # way changelog.py renders it, and each line must already be in the section.
  prev=$(git describe --tags --abbrev=0 --match 'v[0-9]*' "$sha" 2>/dev/null) || prev=""
  [ -n "$prev" ] || { echo "not tagging $short: no version tag is reachable from it to count its changes from"; exit 1; }
  listed=$(awk -v want="## $NEW " 'index($0, want) == 1 { on = 1; next } on && /^## / { exit } on && /^- /' <<<"$changelog")
  left_out=$(python3 -c '
import sys
sys.path.insert(0, "scripts")
try:
    import changelog
    listed = {l[2:] for l in sys.stdin.read().splitlines() if l.startswith("- ")}
    found = changelog.commits(sys.argv[1], sys.argv[2])
    cancelled = changelog.reverted_subjects(found)
    missing = [c for c in found if changelog.entry(c, cancelled)[0] and changelog.entry(c, cancelled)[1] not in listed]
except (Exception, SystemExit) as e:
    print("could not compare: %s" % e)
    sys.exit(3)
for c in missing[:10]:
    print(c["sha"][:7], c["subject"])
if len(missing) > 10:
    print("+%d more" % (len(missing) - 10))
sys.exit(1 if missing else 0)
' "$prev" "$sha" <<<"$listed" 2>&1) && compared=0 || compared=$?
  if [ "$compared" = 1 ]; then
    echo "not tagging $short: its CHANGELOG.md $NEW section leaves out commits it releases, merged into main after the release was prepared:"
    sed 's/^/    /' <<<"$left_out"
    echo "to fix: update $RELEASE_BRANCH from main, regenerate its $NEW CHANGELOG section, push it, and merge it again"
    exit 1
  elif [ "$compared" != 0 ]; then
    echo "not tagging $short: $left_out"; exit 1
  fi

  # The check runs GitHub holds for that commit: the latest attempt of each, so a re-run that
  # passed replaces the attempt that failed. None at all is a refusal, not a vacuous pass, and so
  # is a required check of main's that has not run yet: a workflow started by some other event
  # can put a passing run on main's head before CI has queued its own.
  if ! runs=$(gh api "repos/{owner}/{repo}/commits/$sha/check-runs?per_page=100" 2>"$GH_ERR"); then
    echo "not tagging $short: could not read its check runs:"
    sed 's/^/    /' "$GH_ERR"
    exit 1
  fi
  if ! required=$(gh api "repos/{owner}/{repo}/branches/main/protection/required_status_checks" 2>"$GH_ERR"); then
    echo "not tagging $short: could not read the checks main requires:"
    sed 's/^/    /' "$GH_ERR"
    exit 1
  fi
  if ! checks=$(python3 -c '
import json, sys
d = json.load(sys.stdin)
req = json.loads(sys.argv[1])
names = [c.get("context") for c in req.get("checks") or []] or list(req.get("contexts") or [])
if not names:
    print("main requires no checks, so nothing says which runs must pass")
    sys.exit(1)
runs = d.get("check_runs") or []
if not runs:
    print("it has no check runs, so nothing says it passed")
    sys.exit(1)
total = d.get("total_count", len(runs))
if total != len(runs):
    print("it has %d check runs and only %d were read" % (total, len(runs)))
    sys.exit(1)
bad = 0
have = {r.get("name") for r in runs}
for n in names:
    if n not in have:
        bad += 1
        print("check %r is required on main and has not run on it yet" % n)
for r in runs:
    if r.get("conclusion") != "success":
        bad += 1
        state = r.get("status") or "unknown"
        if r.get("conclusion"):
            state += "/" + r["conclusion"]
        print("check %r is %s" % (r.get("name"), state))
if bad:
    sys.exit(1)
print("%d of %d success" % (len(runs), len(runs)))
' "$required" <<<"$runs" 2>&1); then
    printf '%s\n' "$checks" | sed "s/^/not tagging $short: /"
    exit 1
  fi

  if git rev-parse -q --verify "refs/tags/v$NEW" >/dev/null; then
    echo "not tagging $short: tag v$NEW already exists in this clone"; exit 1
  fi
  remote_tag=$(git ls-remote --tags origin "refs/tags/v$NEW") || { echo "could not read origin's tags"; exit 1; }
  [ -z "$remote_tag" ] || { echo "not tagging $short: tag v$NEW already exists on origin"; exit 1; }
  echo "==> Cargo.toml at $short: $NEW · CHANGELOG section: present · checks: $checks"

  git tag -a "v$NEW" -m "v$NEW" "$sha"
  if ! git push --quiet origin "refs/tags/v$NEW"; then
    git tag -d "v$NEW" >/dev/null
    echo "pushing tag v$NEW failed, so it was deleted here too and nothing is released; re-run: scripts/release.sh $NEW --tag"
    exit 1
  fi
  echo "==> tagged v$NEW on $short and pushed the tag; the Release workflow builds the binaries"
  # `fixed-in:<version>`, not the close, is what flips an entry on the docs
  # site's Known issues page. Never fatal: the release is already out, and this
  # can be re-run by hand.
  echo "==> label issues closed since the previous release"
  python3 scripts/label-fixed-in.py "$NEW" || echo "    (labelling failed; re-run: scripts/label-fixed-in.py $NEW)"
  exit 0
fi

# ── prepare: the release commit, on its own branch ──────────────────────────
[ "$PUSH" = 0 ] || need_gh --push
BRANCH=$(git rev-parse --abbrev-ref HEAD)
[ "$BRANCH" = main ] || { echo "release from main, not $BRANCH"; exit 2; }
if [ -n "$(git status --porcelain)" ]; then
  echo "working tree is not clean; commit or stash first:"
  git status --short
  exit 2
fi
# The PR merges into origin's main, and the changelog lists the commits since the last tag. A
# main behind origin's leaves commits out of that list; one ahead of it carries commits that never
# went through a PR. The fetch is the one network call preparing makes.
git fetch --quiet origin || { echo "could not fetch origin, so there is no telling whether main is current"; exit 1; }
counts=$(git rev-list --left-right --count main...origin/main) || { echo "there is no origin/main to compare main with"; exit 1; }
read -r ahead behind <<<"$counts"
if [ "$ahead" != 0 ] && [ "$behind" != 0 ]; then
  echo "main has diverged from origin/main ($ahead commit(s) ahead, $behind behind); reset it to origin/main first"; exit 2
elif [ "$behind" != 0 ]; then
  echo "main is $behind commit(s) behind origin/main; update it first: git pull --ff-only"; exit 2
elif [ "$ahead" != 0 ]; then
  echo "main is $ahead commit(s) ahead of origin/main; main moves only through PRs, so those commits need one first"; exit 2
fi
OLD=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$OLD" ] || { echo "no [package] version in Cargo.toml"; exit 1; }
[ "$OLD" != "$NEW" ] || { echo "already at $NEW"; exit 2; }
# The changelog counts from v$OLD. Between a release PR's merge and its --tag run, main carries a
# version with no tag; preparing then would die after the bump instead of here.
if ! git rev-parse -q --verify "refs/tags/v$OLD" >/dev/null; then
  echo "main is at $OLD, but tag v$OLD does not exist, even after fetching origin; tag that release first: scripts/release.sh $OLD --tag"; exit 2
fi
if git rev-parse -q --verify "refs/tags/v$NEW" >/dev/null; then
  echo "tag v$NEW already exists"; exit 2
fi
if git rev-parse -q --verify "refs/heads/$RELEASE_BRANCH" >/dev/null; then
  echo "branch $RELEASE_BRANCH already exists; delete it to start over: git branch -D $RELEASE_BRANCH"; exit 2
fi
remote_branch=$(git ls-remote --heads origin "refs/heads/$RELEASE_BRANCH") || { echo "could not read origin's branches"; exit 1; }
[ -z "$remote_branch" ] || { echo "origin already has $RELEASE_BRANCH; its PR is open or was"; exit 2; }

# Cheap, and it runs the exact line below against a scratch copy first. A broken
# invocation then stops the release before the version bump instead of after it.
echo "==> check the release's own command lines"
./scripts/test-release-invocations.sh > /dev/null || {
  echo "a command line this script assembles does not run; details:"
  ./scripts/test-release-invocations.sh
  exit 1
}

git checkout --quiet -b "$RELEASE_BRANCH"
# From here a failing step leaves the half-bumped tree on the release branch, where a re-run
# refuses ("release from main") and `git branch -D` cannot delete the branch it is on.
trap '[ $? = 0 ] || echo "stopped on $RELEASE_BRANCH; main is untouched. To start over: git checkout -f main && git branch -D $RELEASE_BRANCH"' EXIT
echo "==> $OLD -> $NEW on branch $RELEASE_BRANCH"
# The [package] version is the first `version = ` line in Cargo.toml.
sed -i "0,/^version = \"$OLD\"/s//version = \"$NEW\"/" Cargo.toml
sed -i "s/version-$OLD-/version-$NEW-/; s/alt=\"Version $OLD\"/alt=\"Version $NEW\"/" README.md
# Let cargo rewrite the lock entry for the root package. Never edit it by hand.
cargo update --workspace --offline --quiet
if ! grep -A1 '^name = "base"$' Cargo.lock | grep -q "version = \"$NEW\""; then
  echo "Cargo.lock did not pick up $NEW"; exit 1
fi

# The tag does not exist yet, so the range ends at HEAD and the date is today's
# rather than the last commit's. This has to precede the regeneration below, not
# merely the suite: `changelog_has_a_section_for_this_version` is one of the
# help_docs tests, and it is the single check BASE_REGEN_DOCS deliberately
# cannot write its way out of. Regenerating first fails on it and aborts the
# release after the version bump and before the tag, which is exactly how
# 0.13.16's first attempt died.
echo "==> write the $NEW section of CHANGELOG.md"
python3 scripts/changelog.py section "v$OLD" HEAD "$NEW" --date "$(date +%F)" --prepend CHANGELOG.md

echo "==> regenerate the base-help coach for $NEW"
BASE_REGEN_DOCS=1 cargo test --quiet --bin base help_docs

echo "==> test"
if [ "$QUICK" = 1 ]; then
  cargo test --quiet --bin base help_docs
else
  cargo test --quiet
fi

git add Cargo.toml Cargo.lock README.md CHANGELOG.md claude/skills/base-help
git commit --quiet -m "chore(release): $NEW"
trap - EXIT
echo "==> committed $(git rev-parse --short HEAD) on $RELEASE_BRANCH (no tag yet)"
if [ "$PUSH" = 1 ]; then
  # The PR's body is the section just written, the text the GitHub release will carry.
  BODY=$(python3 scripts/changelog.py extract "$NEW")
  git push --quiet -u origin "$RELEASE_BRANCH"
  if ! url=$(gh pr create --base main --head "$RELEASE_BRANCH" --title "chore(release): $NEW" --body "$BODY"); then
    echo "pushed $RELEASE_BRANCH, but gh could not open its PR; open it by hand: base main, title \"chore(release): $NEW\""
    exit 1
  fi
  echo "==> pushed $RELEASE_BRANCH; PR: $url"
  echo "after it merges: scripts/release.sh $NEW --tag"
else
  echo "to open its PR: git push -u origin $RELEASE_BRANCH, then a PR to main titled \"chore(release): $NEW\""
  echo "after it merges: scripts/release.sh $NEW --tag"
fi
