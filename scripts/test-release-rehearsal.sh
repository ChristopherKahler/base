#!/usr/bin/env bash
# Run a whole release, for real, in a throwaway clone -- then throw it away.
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
# A release goes through a PR, because main is protected: `release.sh X.Y.Z`
# prepares the release commit on release/vX.Y.Z, `--push` pushes that branch and
# opens its PR, and `--tag` tags the PR's merge commit once it merged green. This
# runs all three, and every refusal `--tag` makes is its own check, because
# `--tag` is the step that publishes and one refusal passing must not hide
# another failing.
#
#   ./scripts/test-release-rehearsal.sh
#
# Nothing here touches this repository or reaches GitHub. The clone's origin is a
# bare repository in the same temporary directory, so every fetch and push is
# real and none leaves the machine. `gh` is a stub put first on PATH: it answers
# from files this script writes and records what it was asked. And
# `scripts/label-fixed-in.py`, which `--tag` runs after the tag push and which
# calls the GitHub API itself, is replaced in the clone by a recorder before any
# `--tag` run. The version is one no real release will ever use.
#
# Exit 0 = a release cut from this commit reaches its PR, and `--tag` tags only
#          a merged, green merge commit carrying the version.
# Exit 1 = it does not, and the message says which check failed.

set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

VERSION="99.99.99"
TAG="v$VERSION"
RB="release/v$VERSION"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
ORIGIN="$TMP/origin.git"
CLONE="$TMP/base"
STUB="$TMP/gh-stub"
URL="https://github.invalid/example/base/pull/1"

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
# CI checks out a detached HEAD; the bare origin's main is set to it, which is
# the main `release.sh` prepares from.
if ! git clone --quiet --bare --no-hardlinks "$ROOT" "$ORIGIN" 2>"$TMP/err" ||
   ! git -C "$ORIGIN" update-ref refs/heads/main "$HEAD_SHA" 2>>"$TMP/err" ||
   ! git -C "$ORIGIN" symbolic-ref HEAD refs/heads/main 2>>"$TMP/err"; then
  bad "could not set up the bare origin: $(head -3 "$TMP/err")"
  say ""
  say "FAIL -- the rehearsal could not start."
  exit 1
fi
# This commit can be a release commit whose tag comes after its PR merges: the
# release PR and its merge commit run this too. The tag `--tag` will push is
# stood in, in the throwaway origin only, so preparing the next release from
# here counts from it as it will once the release is out.
if ! git -C "$ORIGIN" rev-parse -q --verify "refs/tags/v$OLD" >/dev/null; then
  git -C "$ORIGIN" -c user.name="release rehearsal" -c user.email="rehearsal@invalid" tag -a "v$OLD" -m "v$OLD" "$HEAD_SHA"
  say "  (v$OLD is not tagged yet, so this is a release commit; the rehearsal's origin stands the tag in)"
fi
if ! git clone --quiet --no-hardlinks "$ORIGIN" "$CLONE" 2>"$TMP/err"; then
  bad "could not clone the bare origin: $(head -3 "$TMP/err")"
  say ""
  say "FAIL -- the rehearsal could not start."
  exit 1
fi

# `release.sh` commits and tags, and a CI runner has no git identity, so the
# commit dies with "Author identity unknown" after the version bump -- the same
# half-released shape this test exists to catch, from the environment rather
# than the script. Set here rather than globally: it reaches only this clone.
git -C "$CLONE" config user.name "release rehearsal"
git -C "$CLONE" config user.email "rehearsal@invalid"
in_origin() { git -C "$ORIGIN" -c user.name="release rehearsal" -c user.email="rehearsal@invalid" "$@"; }

# One compile per prepare, not two: the rehearsal shares whatever target
# directory the caller is already using, so in CI it reuses the suite's and
# locally it reuses this checkout's.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

# The gh stub. It knows only the calls `release.sh` makes; anything else fails,
# so an unexpected call cannot pass quietly.
mkdir -p "$STUB/bin"
cat > "$STUB/bin/gh" <<'GH'
#!/usr/bin/env bash
here="$(cd "$(dirname "$0")/.." && pwd)"
printf '%s\n' "$*" >> "$here/calls"
case "$1 ${2:-}" in
  "auth status") exit "$(cat "$here/auth-status")" ;;
  "pr create")
    shift 2
    printf '%s\0' "$@" > "$here/pr-create"
    echo "https://github.invalid/example/base/pull/1" ;;
  "pr view")
    if [ -f "$here/pr-view.json" ]; then
      cat "$here/pr-view.json"
    else
      echo "no pull requests found for branch \"$3\"" >&2; exit 1
    fi ;;
  "api "*)
    case "$2" in
      */check-runs*) cat "$here/check-runs.json" ;;
      */branches/main/protection/required_status_checks) cat "$here/required.json" ;;
      *) echo "gh stub: no answer for gh api $2" >&2; exit 1 ;;
    esac ;;
  *) echo "gh stub: unexpected call: gh $*" >&2; exit 1 ;;
esac
GH
chmod +x "$STUB/bin/gh"
echo 0 > "$STUB/auth-status"
: > "$STUB/calls"

# release ARGS...: run release.sh in the clone with the stub first on PATH. The
# output lands in $TMP/out and the exit status in $rc.
rc=0
release() {
  (cd "$CLONE" && PATH="$STUB/bin:$PATH" ./scripts/release.sh "$@") >"$TMP/out" 2>&1
  rc=$?
}
says() { grep -qF -- "$1" "$TMP/out"; }
show() { sed 's/^/        /' "$TMP/out" | tail -"${1:-12}"; }
ref() { git -C "$1" rev-parse -q --verify "$2" 2>/dev/null; }
no_tag_anywhere() { [ -z "$(ref "$CLONE" "refs/tags/$TAG")" ] && [ -z "$(ref "$ORIGIN" "refs/tags/$TAG")" ]; }
on_main_untouched() {
  [ "$(git -C "$CLONE" rev-parse --abbrev-ref HEAD)" = main ] && [ "$(ref "$CLONE" refs/heads/main)" = "$HEAD_SHA" ] &&
    [ -z "$(ref "$CLONE" "refs/heads/$RB")" ] && [ -z "$(git -C "$CLONE" status --porcelain)" ]
}
# refused CHECK-NAME TEXT: the last run exited non-zero, said TEXT, and left no tag anywhere.
refused() {
  if [ "$rc" -ne 0 ] && says "$2" && no_tag_anywhere; then
    ok "$1"
  else
    bad "$1: exit $rc, want non-zero, a line saying \"$2\" and no $TAG anywhere; its output:"
    show
  fi
}

# ── the release's command lines ─────────────────────────────────────────────
# test-release-invocations.sh is a required check on every PR, the release PR
# and its merge commit included, so where its range starts is pinned on both
# kinds of commit (BO-22): an ordinary one counts from Cargo.toml's version, a
# release commit whose tag is not pushed yet counts from the nearest version tag,
# and a clone with no tags at all, or a version changed by hand, fails in one line
# rather than passing on nothing.
say "invocations: scripts/test-release-invocations.sh"
invocations() {
  (cd "$1" && ./scripts/test-release-invocations.sh) >"$TMP/out" 2>&1
  rc=$?
}
invocations "$CLONE"
if [ "$rc" -eq 0 ] && says "  range: v$OLD..HEAD, from Cargo.toml's version"; then
  ok "on an ordinary commit it passes, counting from Cargo.toml's version: v$OLD..HEAD"
else
  bad "on an ordinary commit: exit $rc, want 0 and 'range: v$OLD..HEAD, from Cargo.toml's version'; its output:"
  show
fi
sed -i "0,/^version = \"$OLD\"/s//version = \"99.99.97\"/" "$CLONE/Cargo.toml"
git -C "$CLONE" commit --quiet -am "feat: a version changed by hand"
invocations "$CLONE"
git -C "$CLONE" reset --quiet --hard "$HEAD_SHA"
if [ "$rc" -ne 0 ] && [ "$(grep -c '^  FAIL' "$TMP/out")" = 1 ] && says "was the version changed by hand?"; then
  ok "on a commit whose untagged version no release commit made, it fails, in one line"
else
  bad "with a version changed by hand: exit $rc, want non-zero and exactly one FAIL line asking whether it was; its output:"
  show
fi
git clone --quiet --no-tags --no-hardlinks "$ORIGIN" "$TMP/no-tags" 2>"$TMP/err"
if [ -n "$(git -C "$TMP/no-tags" tag -l 2>/dev/null)" ]; then
  bad "the clone made without tags has tags, so the next check would prove nothing"
else
  invocations "$TMP/no-tags"
  if [ "$rc" -ne 0 ] && [ "$(grep -c '^  FAIL' "$TMP/out")" = 1 ] && says "no version tag is reachable from HEAD"; then
    ok "in a clone with no tags at all it fails, in one line"
  else
    bad "with no tags: exit $rc, want non-zero and exactly one FAIL line saying no version tag is reachable; its output:"
    show
  fi
fi
say ""

# ── prepare ─────────────────────────────────────────────────────────────────
say "prepare: scripts/release.sh $VERSION --quick"

# origin's main moves one commit on, which the clone has not fetched.
later="$(in_origin commit-tree "$HEAD_SHA^{tree}" -p "$HEAD_SHA" -m "a commit the clone has not fetched")"
in_origin update-ref refs/heads/main "$later"
release "$VERSION" --quick
if [ "$rc" -ne 0 ] && says "main is 1 commit(s) behind origin/main" && on_main_untouched && no_tag_anywhere; then
  ok "prepare refuses a main one commit behind origin/main, says behind, creates nothing"
else
  bad "prepare on a main behind origin/main: exit $rc, want non-zero, 'behind' and nothing created; its output:"
  show
fi
in_origin update-ref refs/heads/main "$HEAD_SHA"

git -C "$CLONE" commit --quiet --allow-empty -m "a commit that never went through a PR"
release "$VERSION" --quick
git -C "$CLONE" reset --quiet --hard "$HEAD_SHA"
if [ "$rc" -ne 0 ] && says "main is 1 commit(s) ahead of origin/main" && on_main_untouched && no_tag_anywhere; then
  ok "prepare refuses a main one commit ahead of origin/main, says ahead, creates nothing"
else
  bad "prepare on a main ahead of origin/main: exit $rc, want non-zero, 'ahead' and nothing created; its output:"
  show
fi

# The release before this one merged but was never tagged: its tag is gone here
# and on origin, and preparing must stop before the bump, naming that tag.
old_tag_origin="$(ref "$ORIGIN" "refs/tags/v$OLD")"
old_tag_here="$(ref "$CLONE" "refs/tags/v$OLD")"
git -C "$ORIGIN" update-ref -d "refs/tags/v$OLD"
git -C "$CLONE" update-ref -d "refs/tags/v$OLD"
release "$VERSION" --quick
git -C "$ORIGIN" update-ref "refs/tags/v$OLD" "$old_tag_origin"
git -C "$CLONE" update-ref "refs/tags/v$OLD" "$old_tag_here"
if [ "$rc" -ne 0 ] && says "tag v$OLD does not exist" && on_main_untouched && no_tag_anywhere; then
  ok "prepare refuses when the release before it was never tagged, names v$OLD, creates nothing"
else
  bad "prepare with v$OLD untagged: exit $rc, want non-zero, a line naming v$OLD and nothing created; its output:"
  show
fi

# A step after the branch exists fails, here cargo, stood in by one that exits 1:
# main stays untouched, and the start-over line it prints works as printed.
mkdir -p "$TMP/failing-cargo"
printf '#!/usr/bin/env bash\necho "cargo stand-in: failing on purpose" >&2\nexit 1\n' > "$TMP/failing-cargo/cargo"
chmod +x "$TMP/failing-cargo/cargo"
(cd "$CLONE" && PATH="$TMP/failing-cargo:$STUB/bin:$PATH" ./scripts/release.sh "$VERSION" --quick) >"$TMP/out" 2>&1
rc=$?
start_over="git checkout -f main && git branch -D $RB"
if [ "$rc" -ne 0 ] && says "stopped on $RB; main is untouched. To start over: $start_over" &&
   [ "$(ref "$CLONE" refs/heads/main)" = "$HEAD_SHA" ] && no_tag_anywhere; then
  (cd "$CLONE" && eval "$start_over") >/dev/null 2>&1
  if on_main_untouched; then
    ok "a prepare that fails after making the branch leaves main untouched and prints a start-over line that works"
  else
    bad "following the printed start-over line did not get back to a clean main"
  fi
else
  bad "a prepare whose cargo fails: exit $rc, want non-zero, the start-over line and main untouched; its output:"
  show
fi

: > "$STUB/calls"
release "$VERSION" --quick
if [ "$rc" -eq 0 ]; then
  ok "ran to completion"
else
  bad "release.sh exited $rc; its last lines:"
  show 20
fi
count="$(git -C "$CLONE" rev-list --count "main..$RB" 2>/dev/null)"
subject="$(git -C "$CLONE" log -1 --format=%s "$RB" 2>/dev/null)"
if [ "$(git -C "$CLONE" rev-parse --abbrev-ref HEAD)" = "$RB" ] && [ "$count" = 1 ] && [ "$subject" = "chore(release): $VERSION" ]; then
  ok "on $RB, one commit past main: \"$subject\""
else
  bad "want $RB checked out with one commit \"chore(release): $VERSION\" past main; got branch $(git -C "$CLONE" rev-parse --abbrev-ref HEAD), $count commit(s), top \"$subject\""
fi
if no_tag_anywhere; then ok "no $TAG tag: preparing tags nothing"; else bad "preparing created $TAG"; fi
if [ "$(ref "$CLONE" refs/heads/main)" = "$HEAD_SHA" ] && [ "$(ref "$ORIGIN" refs/heads/main)" = "$HEAD_SHA" ] &&
   [ -z "$(ref "$ORIGIN" "refs/heads/$RB")" ]; then
  ok "main unchanged here and on origin, and nothing pushed"
else
  bad "main moved or something was pushed: main here $(ref "$CLONE" refs/heads/main), on origin $(ref "$ORIGIN" refs/heads/main), $RB on origin '$(ref "$ORIGIN" "refs/heads/$RB")'"
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
if [ ! -s "$STUB/calls" ]; then
  ok "preparing without --push never called gh"
else
  bad "preparing without --push called gh: $(head -3 "$STUB/calls")"
fi
# The release PR runs the invocation test as a required check before v$VERSION
# exists; it must pass there, or the PR can never merge.
invocations "$CLONE"
if [ "$rc" -eq 0 ] && says "  range: v$OLD..HEAD, from the nearest version tag: v$VERSION is not tagged yet, so this is a release commit" &&
   [ -z "$(git -C "$CLONE" status --porcelain)" ]; then
  ok "on the release commit, before its tag exists, the invocation test passes, counting from the nearest version tag: v$OLD..HEAD"
else
  bad "the invocation test on the release commit: exit $rc, want 0, 'range: v$OLD..HEAD, from the nearest version tag' and a clean tree; its output:"
  show
fi
say ""

# ── --push ──────────────────────────────────────────────────────────────────
say "push: scripts/release.sh $VERSION --quick --push"

# gh off PATH: --push stops before anything is written. bash is named rather
# than found, and git reached through a one-line wrapper, so the PATH holds git
# and nothing else.
mkdir -p "$TMP/no-gh"
printf '#!%s\nexec %q "$@"\n' "$BASH" "$(command -v git)" > "$TMP/no-gh/git"
chmod +x "$TMP/no-gh/git"
git -C "$CLONE" checkout --quiet main && git -C "$CLONE" branch --quiet -D "$RB"
(cd "$CLONE" && PATH="$TMP/no-gh" "$BASH" ./scripts/release.sh "$VERSION" --quick --push) >"$TMP/out" 2>&1
rc=$?
if [ "$rc" -ne 0 ] && says "--push needs the GitHub CLI, and gh is not on PATH" && on_main_untouched && no_tag_anywhere; then
  ok "--push with no gh on PATH stops in one line, before anything is written"
else
  bad "--push with no gh: exit $rc, want non-zero, one line naming gh, nothing created; its output:"
  show
fi

: > "$STUB/calls"
rm -f "$STUB/pr-create"
release "$VERSION" --quick --push
if [ "$rc" -eq 0 ] && says "PR: $URL" && says "after it merges: scripts/release.sh $VERSION --tag"; then
  ok "ran to completion, printed the PR and the --tag line to run after the merge"
else
  bad "release.sh --push exited $rc, or did not print the PR URL and the --tag line; its last lines:"
  show 20
fi
release_sha="$(ref "$CLONE" "refs/heads/$RB")"
if [ -n "$release_sha" ] && [ "$(ref "$ORIGIN" "refs/heads/$RB")" = "$release_sha" ] &&
   [ "$(ref "$ORIGIN" refs/heads/main)" = "$HEAD_SHA" ] && no_tag_anywhere; then
  ok "origin has $RB at the release commit; origin's main unchanged; no tag"
else
  bad "want $RB on origin at ${release_sha:-<none>} and origin's main at $HEAD_SHA; got $RB '$(ref "$ORIGIN" "refs/heads/$RB")', main $(ref "$ORIGIN" refs/heads/main)"
fi
prargs=()
[ -f "$STUB/pr-create" ] && mapfile -d '' -t prargs < "$STUB/pr-create"
# arg FLAG: the value after FLAG in the recorded `gh pr create`.
arg() {
  local i
  for ((i = 0; i + 1 < ${#prargs[@]}; i++)); do
    [ "${prargs[$i]}" = "$1" ] && { printf '%s' "${prargs[$((i + 1))]}"; return 0; }
  done
  return 1
}
body="$(arg --body)"
if [ "$(arg --base)" = main ] && [ "$(arg --head)" = "$RB" ] && [ "$(arg --title)" = "chore(release): $VERSION" ] &&
   [ "${body#"## $VERSION "}" != "$body" ]; then
  ok "gh pr create --base main --head $RB --title \"chore(release): $VERSION\", body the $VERSION CHANGELOG section"
else
  bad "the recorded gh pr create was not --base main --head $RB --title \"chore(release): $VERSION\" with the section as body: ${prargs[*]:0:6}"
fi
say ""

# ── --tag ───────────────────────────────────────────────────────────────────
say "tag: scripts/release.sh $VERSION --tag"

# The recorder that stands in for scripts/label-fixed-in.py. It writes beside
# itself, so the path it writes to never crosses a shell boundary.
cat > "$CLONE/scripts/label-fixed-in.py" <<'PY'
import os, sys
here = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(here, "label-calls.txt"), "a") as f:
    f.write(" ".join(sys.argv[1:]) + "\n")
PY
LABELLED="$CLONE/scripts/label-calls.txt"
if ! grep -q "label-calls.txt" "$CLONE/scripts/label-fixed-in.py"; then
  bad "could not replace scripts/label-fixed-in.py in the clone, so --tag would reach GitHub; the --tag checks were not run"
  say ""
  say "FAIL -- $fail check(s) above."
  exit 1
fi

# The merge, made in the bare origin as GitHub would make it.
merge="$(in_origin commit-tree "$release_sha^{tree}" -p "$HEAD_SHA" -p "$release_sha" -m "Merge pull request #1 from example/$RB")"
# The same merge after eleven changes reached main while the PR was open: it
# releases commits its CHANGELOG section, written when it was prepared, leaves out.
late="$HEAD_SHA"
for i in 1 2 3 4 5 6 7 8 9 10 11; do
  late="$(in_origin commit-tree "$HEAD_SHA^{tree}" -p "$late" -m "feat: change $i, merged while the release PR was open")"
done
late_merge="$(in_origin commit-tree "$release_sha^{tree}" -p "$late" -p "$release_sha" -m "Merge pull request #1 from example/$RB")"

# Two merge commits that are each wrong in one way.
git -C "$CLONE" worktree add --quiet --detach "$TMP/arms" "$release_sha"
sed -i "0,/^version = \"$VERSION\"/s//version = \"99.99.98\"/" "$TMP/arms/Cargo.toml"
git -C "$TMP/arms" commit --quiet -am "a merge commit whose Cargo.toml says another version"
wrong_version="$(git -C "$TMP/arms" rev-parse HEAD)"
git -C "$TMP/arms" checkout --quiet --detach "$release_sha"
git -C "$TMP/arms" checkout --quiet "$HEAD_SHA" -- CHANGELOG.md
git -C "$TMP/arms" commit --quiet -m "a merge commit whose CHANGELOG.md has no section for this version"
no_section="$(git -C "$TMP/arms" rev-parse HEAD)"
git -C "$CLONE" push --quiet origin "$wrong_version:refs/heads/arm-wrong-version" "$no_section:refs/heads/arm-no-section"

# A merge GitHub would report that origin's main does not contain, on a branch so
# --tag's fetch brings it in.
off_main="$(in_origin commit-tree "$release_sha^{tree}" -p "$HEAD_SHA" -p "$release_sha" -m "Merge pull request #2 from example/$RB")"
in_origin update-ref refs/heads/arm-off-main "$off_main"

# main moves on past all the others, so each is on main, and --tag must tag the
# merge commit it is given, not main's head.
after="$(in_origin commit-tree "$release_sha^{tree}" -p "$merge" -p "$late_merge" -p "$wrong_version" -p "$no_section" -m "commits merged after the release")"
in_origin update-ref refs/heads/main "$after"

# pr_view STATE [MERGE-SHA] [BASE]: the stub's answer to `gh pr view`.
pr_view() {
  local oid="null"
  [ -n "${2:-}" ] && oid="{\"oid\": \"$2\"}"
  printf '{"state": "%s", "mergeCommit": %s, "baseRefName": "%s", "url": "%s"}\n' "$1" "$oid" "${3:-main}" "$URL" > "$STUB/pr-view.json"
}
# check_runs NAME=STATUS[/CONCLUSION]...: the stub's answer to the check-runs call.
check_runs() {
  python3 -c '
import json, sys
runs = []
for a in sys.argv[1:]:
    name, _, state = a.partition("=")
    status, _, conclusion = state.partition("/")
    runs.append({"name": name, "status": status, "conclusion": conclusion or None})
print(json.dumps({"total_count": len(runs), "check_runs": runs}))
' "$@" > "$STUB/check-runs.json"
}
GREEN=("test (ubuntu-latest)=completed/success" "test (windows-latest)=completed/success"
       "clippy (ubuntu-latest)=completed/success" "clippy (windows-latest)=completed/success"
       "guards=completed/success" "python-ast-tests=completed/success" "release-invocations=completed/success")
# What main's branch protection requires, as GitHub answers it: the seven checks above.
python3 -c '
import json, sys
print(json.dumps({"strict": True, "contexts": sys.argv[1:], "checks": [{"context": c, "app_id": 15368} for c in sys.argv[1:]]}))
' "${GREEN[@]%%=*}" > "$STUB/required.json"

# gh logged out, on the host GH_HOST names: --tag says so, naming that host.
echo 1 > "$STUB/auth-status"
: > "$STUB/calls"
pr_view MERGED "$merge"; check_runs "${GREEN[@]}"
export GH_HOST=ghe.example.invalid
release "$VERSION" --tag
unset GH_HOST
refused "--tag with gh logged out stops in one line, naming the host GH_HOST gives" "--tag needs gh logged in to ghe.example.invalid"
grep -qF "auth status --hostname ghe.example.invalid" "$STUB/calls" ||
  bad "gh auth status was not asked about GH_HOST's host; calls: $(tr '\n' ';' < "$STUB/calls")"
echo 0 > "$STUB/auth-status"

rm -f "$STUB/pr-view.json"
release "$VERSION" --tag
refused "--tag refuses when no PR exists for $RB" "no PR found for $RB"

pr_view OPEN
release "$VERSION" --tag
refused "--tag refuses an unmerged PR" "PR for $RB is OPEN, not MERGED"

pr_view MERGED "$merge" dev
release "$VERSION" --tag
refused "--tag refuses a PR merged into another branch" "merged into dev, not main"

pr_view MERGED "$off_main"
release "$VERSION" --tag
refused "--tag refuses a merge commit origin's main does not contain" "but origin/main does not contain it"

pr_view MERGED "$merge"
check_runs "${GREEN[@]:0:1}" "test (windows-latest)=completed/failure" "${GREEN[@]:2}"
release "$VERSION" --tag
refused "--tag refuses a red check and names it" "check 'test (windows-latest)' is completed/failure"

check_runs "${GREEN[@]:0:6}" "release-invocations=in_progress"
release "$VERSION" --tag
refused "--tag refuses a check still running and names it" "check 'release-invocations' is in_progress"

check_runs
release "$VERSION" --tag
refused "--tag refuses a merge commit with no check runs at all" "it has no check runs"

# A workflow started by another event (an issue labelled, say) has put a passing
# run on the merge commit before CI queued its own.
check_runs "dispatch=completed/success"
release "$VERSION" --tag
refused "--tag refuses while a check main requires has not run, though every run present passed" \
  "check 'test (ubuntu-latest)' is required on main and has not run on it yet"

check_runs "${GREEN[@]}"
pr_view MERGED "$wrong_version"
release "$VERSION" --tag
refused "--tag refuses a merge commit whose Cargo.toml says another version" "its Cargo.toml says 99.99.98, not $VERSION"

pr_view MERGED "$no_section"
release "$VERSION" --tag
refused "--tag refuses a merge commit with no $VERSION CHANGELOG section" "its CHANGELOG.md has no '## $VERSION' section"

pr_view MERGED "$late_merge"
release "$VERSION" --tag
refused "--tag refuses a merge commit releasing commits its CHANGELOG section leaves out" \
  "its CHANGELOG.md $VERSION section leaves out commits it releases"
if [ "$(grep -c 'feat: change [0-9]*, merged while the release PR was open' "$TMP/out")" = 10 ] && says "+1 more" &&
   says "${late:0:7} feat: change 11, merged while the release PR was open" &&
   says "to fix: update $RB from main, regenerate its $VERSION CHANGELOG section, push it, and merge it again"; then
  ok "it lists the first 10 of the 11 left out, short sha and subject, then +1 more, and says the fix in one line"
else
  bad "want 10 of the 11 left-out commits listed with short sha and subject, '+1 more' and the one-line fix; its output:"
  show 16
fi

if [ ! -e "$LABELLED" ]; then
  ok "no refusal ran the issue labeller"
else
  bad "the issue labeller ran on a refused --tag: $(cat "$LABELLED")"
fi

: > "$STUB/calls"
pr_view MERGED "$merge"
release "$VERSION" --tag
tagged="$(git -C "$ORIGIN" rev-parse -q --verify "refs/tags/$TAG^{commit}" 2>/dev/null)"
if [ "$rc" -eq 0 ] && [ "$tagged" = "$merge" ] && [ "$(ref "$ORIGIN" refs/heads/main)" = "$after" ]; then
  ok "tagged $TAG on the merge commit and pushed it; origin's main had moved on and is untouched"
else
  bad "want exit 0 and $TAG on origin at the merge commit $merge, not main's head $after; got exit $rc, tag at '${tagged:-<none>}'; its output:"
  show 20
fi
if [ "$(git -C "$ORIGIN" cat-file -t "refs/tags/$TAG" 2>/dev/null)" = tag ] &&
   [ "$(ref "$ORIGIN" "refs/heads/$RB")" = "$release_sha" ] && grep -qF "commits/$merge/check-runs" "$STUB/calls"; then
  ok "the tag is annotated, only the tag was pushed, and the check runs read were the merge commit's"
else
  bad "want an annotated tag, $RB on origin unmoved and the check runs of $merge read; calls: $(tr '\n' ';' < "$STUB/calls")"
fi
if [ "$(cat "$LABELLED" 2>/dev/null)" = "$VERSION" ]; then
  ok "after the tag push it ran the issue labeller for $VERSION, once"
else
  bad "want scripts/label-fixed-in.py run once with $VERSION; it recorded: '$(cat "$LABELLED" 2>/dev/null)'"
fi

release "$VERSION" --tag
if [ "$rc" -ne 0 ] && says "tag $TAG already exists in this clone" && [ "$(git -C "$ORIGIN" rev-parse -q --verify "refs/tags/$TAG^{commit}")" = "$merge" ]; then
  ok "a second --tag refuses: the tag already exists here"
else
  bad "a second --tag: exit $rc, want non-zero and 'already exists in this clone'; its output:"
  show
fi
git -C "$CLONE" tag -d "$TAG" >/dev/null
release "$VERSION" --tag
if [ "$rc" -ne 0 ] && says "tag $TAG already exists on origin" && [ "$(git -C "$ORIGIN" rev-parse -q --verify "refs/tags/$TAG^{commit}")" = "$merge" ]; then
  ok "--tag refuses a tag that exists only on origin"
else
  bad "--tag with the tag on origin only: exit $rc, want non-zero and 'already exists on origin'; its output:"
  show
fi

say ""
if [ "$fail" -eq 0 ]; then
  say "PASS -- a release cut from this commit reaches its PR, and --tag publishes only a merged, green release."
  exit 0
fi
say "FAIL -- $fail check(s) above. Cutting a real release would break the same way."
exit 1
