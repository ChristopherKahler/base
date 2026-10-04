#!/usr/bin/env bash
# Run every `changelog.py` command line the release pipeline assembles, with the
# argv it actually assembles, against a scratch copy of CHANGELOG.md.
#
# This exists because 0.13.16 nearly died on one: `release.sh` ends its line with
# `--date "$(date +%F)"`, and `--date` was declared on the parent parser only, so
# argparse rejected it after the subcommand. Every part of the generator had been
# proven -- the section content, `--prepend`, byte-for-byte reproducibility --
# except the one thing the release would actually type. Under `set -e` that
# aborts a release after the version bump and before the tag.
#
# The lines are read out of `scripts/release.sh` and `.github/workflows/release.yml`
# rather than copied here, so editing either one is covered by this.
#
#   ./scripts/test-release-invocations.sh
#
# Exit 0 = every invocation the release makes runs. Exit 1 = one of them does not.

set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

TMP="$(mktemp -d)"
cp CHANGELOG.md "$TMP/CHANGELOG.orig"
# Restore on any exit: these invocations write to the real CHANGELOG.md, because
# writing to the real path is the point of the test.
trap 'cp "$TMP/CHANGELOG.orig" CHANGELOG.md; rm -rf "$TMP"' EXIT

fail=0
say() { printf '%s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; fail=$((fail + 1)); }
ok()  { printf '  ok    %s\n' "$*"; }

# The values release.sh has in scope when it runs its line. OLD is Cargo.toml's
# version, whose tag exists once that version is released. A release commit is
# tested before its tag exists: the release PR and its merge commit run this as a
# required check, and `release.sh --tag` refuses a merge commit whose checks are
# not green (BO-22). There the nearest version tag reachable from HEAD stands in
# for it: the argv is the same, and the range only needs a tag that exists. Only
# there: an untagged version with no `chore(release)` commit for it in that range
# was changed by hand, and fails.
CARGO_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
OLD="$CARGO_VERSION"
if ! git rev-parse -q --verify "refs/tags/v$OLD" >/dev/null; then
  OLD="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null | sed 's/^v//')"
fi
NEW="99.99.99"
export OLD NEW

say "Release invocations, against base $CARGO_VERSION"
stop() {
  bad "$1"
  say ""
  say "FAIL -- nothing ran: $2"
  exit 1
}
if [ -z "$OLD" ]; then
  stop "v$CARGO_VERSION is not tagged and no version tag is reachable from HEAD, so release.sh's range has no start" \
       "a clone without tags cannot test the release's command lines."
elif [ "$OLD" = "$CARGO_VERSION" ]; then
  say "  range: v$OLD..HEAD, from Cargo.toml's version"
else
  subjects="$(git log --format=%s "v$OLD..HEAD")"
  awk -v want="chore(release): $CARGO_VERSION" '$0 == want || index($0, want " ") == 1 { found = 1 } END { exit !found }' <<<"$subjects" ||
    stop "Cargo.toml says $CARGO_VERSION, which is not tagged, and no 'chore(release): $CARGO_VERSION' commit is in v$OLD..HEAD: was the version changed by hand?" \
         "only a release commit may carry a version that has no tag yet."
  say "  range: v$OLD..HEAD, from the nearest version tag: v$CARGO_VERSION is not tagged yet, so this is a release commit"
fi
say ""

# ── scripts/release.sh ──────────────────────────────────────────────────────
say "scripts/release.sh"
found=0
while IFS= read -r line; do
  case "$(printf '%s' "$line" | sed 's/^[[:space:]]*//')" in \#*) continue ;; esac
  found=$((found + 1))
  say "  \$ $line"
  if eval "$line" >/dev/null 2>"$TMP/err"; then
    ok "ran"
  else
    bad "$(head -3 "$TMP/err")"
  fi
done < <(grep 'scripts/changelog\.py' scripts/release.sh)
[ "$found" -gt 0 ] || bad "no changelog.py invocation found in scripts/release.sh -- did it move?"

# The point of release.sh's call is that the new version's section lands.
if grep -q "^## $NEW " CHANGELOG.md; then
  ok "the $NEW section was written into CHANGELOG.md"
else
  bad "no '## $NEW' section in CHANGELOG.md after release.sh's line ran"
fi
cp "$TMP/CHANGELOG.orig" CHANGELOG.md
say ""

# ── .github/workflows/release.yml ───────────────────────────────────────────
# The release body is extracted from CHANGELOG.md by tag name; GITHUB_REF_NAME
# is what the workflow has, so that is what this gives it: the tag this
# Cargo.toml's version is, or will be once `release.sh --tag` pushes it.
say ".github/workflows/release.yml"
GITHUB_REF_NAME="v$CARGO_VERSION"
export GITHUB_REF_NAME
found=0
while IFS= read -r line; do
  line="$(printf '%s' "$line" | sed 's/^[[:space:]]*//; s/ > release-notes\.md$//')"
  case "$line" in \#*) continue ;; esac
  found=$((found + 1))
  say "  \$ $line"
  if eval "$line" >/dev/null 2>"$TMP/err"; then
    ok "ran"
  else
    bad "$(head -3 "$TMP/err")"
  fi
done < <(grep 'scripts/changelog\.py' .github/workflows/release.yml)
[ "$found" -gt 0 ] || bad "no changelog.py invocation found in release.yml -- did it move?"

# ── scripts/label-fixed-in.py ───────────────────────────────────────────────
# The `fixed-in:<version>` window and the labelable filter (#57), proven without
# the network: the doctests pin the arithmetic release.sh runs after the tag push.
say "scripts/label-fixed-in.py"
if python3 -m doctest scripts/label-fixed-in.py >/dev/null 2>"$TMP/err"; then
  ok "doctest: the fixed-in window and the labelable filter"
else
  bad "$(head -3 "$TMP/err")"
fi

say ""
if [ "$fail" -eq 0 ]; then
  say "PASS -- every command line the release assembles runs."
  exit 0
fi
say "FAIL -- $fail invocation(s) above. A release would abort on these."
exit 1
