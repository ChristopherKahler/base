#!/usr/bin/env bash
# Cut a base release: bump, regenerate the base-help coach, test, commit, tag.
#
#   scripts/release.sh 0.13.15            bump + regen + changelog + full suite + commit + tag
#   scripts/release.sh 0.13.15 --push     ...then push main and the tag (CI builds the binaries)
#   scripts/release.sh 0.13.15 --quick    gate on the help_docs tests only, skip the full suite
#
# Why a script: 0.13.13 shipped a Cargo.lock that cargo could not parse (a hand
# bump), twelve releases shipped a base-help coach stamped v0.13.2 because
# nothing regenerated it, and thirteen shipped with no changelog entry at all.
# Every step here is one that was skipped at least once.
set -euo pipefail

usage() { sed -n '2,6p' "$0"; exit 2; }
[ $# -ge 1 ] || usage
NEW="$1"; shift
PUSH=0; QUICK=0
for a in "$@"; do
  case "$a" in
    --push) PUSH=1 ;;
    --quick) QUICK=1 ;;
    *) usage ;;
  esac
done
[[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not a version: $NEW"; exit 2; }

cd "$(git rev-parse --show-toplevel)"
BRANCH=$(git rev-parse --abbrev-ref HEAD)
[ "$BRANCH" = main ] || { echo "release from main, not $BRANCH"; exit 2; }
if [ -n "$(git status --porcelain)" ]; then
  echo "working tree is not clean; commit or stash first:"
  git status --short
  exit 2
fi
OLD=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$OLD" ] || { echo "no [package] version in Cargo.toml"; exit 1; }
[ "$OLD" != "$NEW" ] || { echo "already at $NEW"; exit 2; }
if git rev-parse -q --verify "refs/tags/v$NEW" >/dev/null; then
  echo "tag v$NEW already exists"; exit 2
fi

echo "==> $OLD -> $NEW"
# The [package] version is the first `version = ` line in Cargo.toml.
sed -i "0,/^version = \"$OLD\"/s//version = \"$NEW\"/" Cargo.toml
sed -i "s/version-$OLD-/version-$NEW-/; s/alt=\"Version $OLD\"/alt=\"Version $NEW\"/" README.md
# Let cargo rewrite the lock entry for the root package. Never edit it by hand.
cargo update --workspace --offline --quiet
if ! grep -A1 '^name = "base"$' Cargo.lock | grep -q "version = \"$NEW\""; then
  echo "Cargo.lock did not pick up $NEW"; exit 1
fi

echo "==> regenerate the base-help coach for $NEW"
BASE_REGEN_DOCS=1 cargo test --quiet --bin base help_docs

# The tag does not exist yet, so the range ends at HEAD and the date is today's
# rather than the last commit's. src/help_docs.rs fails the suite below if this
# step did not run, which is the whole reason it sits above the tests.
echo "==> write the $NEW section of CHANGELOG.md"
python3 scripts/changelog.py section "v$OLD" HEAD "$NEW" --date "$(date +%F)" --prepend CHANGELOG.md

echo "==> test"
if [ "$QUICK" = 1 ]; then
  cargo test --quiet --bin base help_docs
else
  cargo test --quiet
fi

git add Cargo.toml Cargo.lock README.md CHANGELOG.md claude/skills/base-help
git commit --quiet -m "chore(release): $NEW"
git tag -a "v$NEW" -m "v$NEW"
echo "==> committed $(git rev-parse --short HEAD), tagged v$NEW"
if [ "$PUSH" = 1 ]; then
  git push origin main "v$NEW"
  echo "==> pushed; the Release workflow builds the binaries and base update picks them up"
  # `fixed-in:<version>`, not the close, is what flips an entry on the docs
  # site's Known issues page. Never fatal: the release is already out, and this
  # can be re-run by hand.
  echo "==> label issues closed since the previous release"
  python3 scripts/label-fixed-in.py "$NEW" || echo "    (labelling failed; re-run: scripts/label-fixed-in.py $NEW)"
else
  echo "push with: git push origin main v$NEW"
  echo "then label the issues it fixed: scripts/label-fixed-in.py $NEW"
fi
