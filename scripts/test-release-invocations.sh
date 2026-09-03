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

# The values release.sh has in scope when it runs its line.
OLD="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
NEW="99.99.99"
export OLD NEW

say "Release invocations, against base $OLD"
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
# is what the workflow has, so that is what this gives it.
say ".github/workflows/release.yml"
GITHUB_REF_NAME="v$OLD"
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

say ""
if [ "$fail" -eq 0 ]; then
  say "PASS -- every command line the release assembles runs."
  exit 0
fi
say "FAIL -- $fail invocation(s) above. A release would abort on these."
exit 1
