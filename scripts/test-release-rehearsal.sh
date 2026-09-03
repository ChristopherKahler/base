#!/usr/bin/env bash
# Cut a whole release, for real, in a throwaway clone -- then throw it away.
#
# `test-release-invocations.sh` runs the command lines the release assembles.
# This runs `release.sh` itself, in the order it runs them, and that distinction
# is not academic. 0.13.16's first attempt died with every individual step
# working: the coach regeneration ran before the changelog was written, and
# `changelog_has_a_section_for_this_version` is one of the help_docs tests that
# regeneration deliberately cannot write its way out of. Under `set -e` the
# release aborted after the version bump and before the tag, leaving a bumped
# working tree and no release. Nothing tested the sequence, only the steps.
#
#   ./scripts/test-release-rehearsal.sh
#
# Nothing here touches this repository or the remote: the rehearsal clones the
# current commit into a temporary directory, runs `release.sh` there without
# `--push`, at a version no real release will ever use, and deletes it.
#
# Exit 0 = a release cut from this commit reaches its commit and its tag.
# Exit 1 = it does not, and the message says which step it died on.

set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

VERSION="99.99.99"
TAG="v$VERSION"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
CLONE="$TMP/base"

fail=0
say() { printf '%s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; fail=$((fail + 1)); }
ok()  { printf '  ok    %s\n' "$*"; }

HEAD_SHA="$(git rev-parse HEAD)"
OLD="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

say "Release rehearsal: $OLD -> $VERSION, from $(git rev-parse --short HEAD)"
say ""

# --no-hardlinks so a rehearsal that somehow writes into .git cannot reach the
# real object store. Tags come along, which `release.sh` needs for `v$OLD..HEAD`.
if ! git clone --quiet --no-hardlinks "$ROOT" "$CLONE" 2>"$TMP/err"; then
  bad "could not clone the repository: $(head -3 "$TMP/err")"
  say ""
  say "FAIL -- the rehearsal could not start."
  exit 1
fi

# `release.sh` refuses to run off main, and CI checks out a detached HEAD.
git -C "$CLONE" checkout -q -B main "$HEAD_SHA"

# One compile, not two: the rehearsal shares whatever target directory the
# caller is already using, so in CI it reuses the suite's and locally it reuses
# this checkout's.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

# --quick gates on the help_docs tests rather than the full suite. The step that
# broke -- the regeneration -- runs either way, and the suite has already run by
# the time anything calls this.
say "  \$ ./scripts/release.sh $VERSION --quick"
if (cd "$CLONE" && ./scripts/release.sh "$VERSION" --quick) >"$TMP/out" 2>&1; then
  ok "ran to completion"
else
  bad "release.sh exited $?; its last lines:"
  sed 's/^/        /' "$TMP/out" | tail -20
fi

# Reaching the tag is the claim. A release that bumps the version and dies is
# worse than one that never started, because the tree is left half-released.
if [ -n "$(git -C "$CLONE" tag -l "$TAG")" ]; then
  ok "tagged $TAG"
else
  bad "no $TAG tag -- the release did not reach its last step"
fi

subject="$(git -C "$CLONE" log -1 --format=%s 2>/dev/null)"
if [ "$subject" = "chore(release): $VERSION" ]; then
  ok "committed \"$subject\""
else
  bad "top commit is \"$subject\", expected \"chore(release): $VERSION\""
fi

if head -20 "$CLONE/CHANGELOG.md" 2>/dev/null | grep -q "^## $VERSION "; then
  ok "CHANGELOG.md opens on the $VERSION section"
else
  bad "no '## $VERSION' section at the top of CHANGELOG.md"
fi

# `release.sh` names the paths it stages. Anything it modifies and does not name
# is left behind uncommitted, and ships in whatever the next commit sweeps up.
dirty="$(git -C "$CLONE" status --porcelain)"
if [ -z "$dirty" ]; then
  ok "the release committed everything it touched"
else
  bad "files modified by the release but not staged by it:"
  printf '%s\n' "$dirty" | sed 's/^/        /'
fi

say ""
if [ "$fail" -eq 0 ]; then
  say "PASS -- a release cut from this commit reaches its commit and its tag."
  exit 0
fi
say "FAIL -- $fail check(s) above. Cutting a real release would break the same way."
exit 1
