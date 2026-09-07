#!/usr/bin/env bash
# base 0.14.2 #80 — acceptance harness: release assets carry the license files.
#
#   verification/base-0.14.2/release_license_assets_0142.sh <branch-worktree> [base-sha]
#
# The in-repo test (`scripts/test-release-packaging.sh`) can only ever see one
# tree. This is the RED/GREEN across two generations of the workflow, plus the
# rows the ledger wants: the real published v0.14.1 assets, a byte-identity row
# over the archive payload, and a mutation row that proves the instrument
# detects the specific half-fix this bug invites.
#
# Every row prints its real n. The harness ABORTS rather than score from inputs
# it cannot tell apart, and a row it cannot run is a SKIP with a reason, never a
# silent pass.

set -uo pipefail

WT="${1:-}"
BASE_SHA="${2:-f7c4f43}"

if [ -z "$WT" ] || [ ! -d "$WT/.git" ] && [ ! -f "$WT/.git" ]; then
  echo "usage: $0 <branch-worktree> [base-sha]" >&2
  exit 2
fi
WT="$(cd "$WT" && pwd)"

WORK="${WORK:-$HOME/.cache/plover-80/accept}"
ASSETS="${ASSETS:-$HOME/.cache/plover-80/assets}"
BIN_PAYLOAD="${BIN_PAYLOAD:-$HOME/.cache/plover-bins/base-branch-7a9b0a6}"

# The sha256 GitHub's own API reports for the two v0.14.1 assets (release
# 384286377). A local copy is read only after it matches; an unverified copy is
# not evidence about what users received.
SHA_LINUX="cecc309585968d8b47e28e33001b73ab4a3b9e565387e4a3589f4b7f448fa6f7"
SHA_WIN="2083bd61397fa1ddb51b049a913cb722542607560c3ca5453dea9e711f45b0ca"

LICENSES="LICENSE.md LICENSING.md LICENSE-APACHE-2.0 LICENSE-PolyForm-Noncommercial-1.0.0.md"
M_TARGET="x86_64-unknown-linux-gnu"; M_BINARY="base";     M_ASSET="base-linux-x86_64.tar.gz"
W_TARGET="x86_64-pc-windows-msvc";   W_BINARY="base.exe"; W_ASSET="base-windows-x86_64.zip"

pass=0; failn=0; skipn=0
say()  { printf '%s\n' "$*"; }
head_(){ printf '\n── %s ──────────────────────────────────────────\n' "$*"; }
ok()   { printf '  PASS  %s\n' "$*"; pass=$((pass+1)); }
bad()  { printf '  FAIL  %s\n' "$*"; failn=$((failn+1)); }
skip() { printf '  SKIP  %s\n' "$*"; skipn=$((skipn+1)); }
die()  { printf '\nABORT — %s\n' "$*"; exit 3; }

rm -rf "$WORK"; mkdir -p "$WORK"

BRANCH_SHA="$(git -C "$WT" rev-parse --short HEAD)"

# ── R0 provenance ───────────────────────────────────────────────────────────
head_ "R0 provenance"
git -C "$WT" show "$BASE_SHA:.github/workflows/release.yml" > "$WORK/release.base.yml" \
  || die "cannot read release.yml at $BASE_SHA"
cp "$WT/.github/workflows/release.yml" "$WORK/release.branch.yml" \
  || die "cannot read the branch's release.yml"

MD5_BASE="$(md5sum "$WORK/release.base.yml" | cut -d' ' -f1)"
MD5_BRANCH="$(md5sum "$WORK/release.branch.yml" | cut -d' ' -f1)"
MD5_SELF="$(md5sum "$0" | cut -d' ' -f1)"
say "  harness            $MD5_SELF  $0"
say "  release.yml @$BASE_SHA   $MD5_BASE"
say "  release.yml @$BRANCH_SHA   $MD5_BRANCH"
[ "$MD5_BASE" != "$MD5_BRANCH" ] \
  || die "the two release.yml generations are byte-identical — there is nothing to compare"
ok "the two workflow generations differ"

if [ -f "$BIN_PAYLOAD" ]; then
  say "  payload            $(md5sum "$BIN_PAYLOAD" | cut -d' ' -f1)  $BIN_PAYLOAD ($(stat -c%s "$BIN_PAYLOAD") B)"
  PAYLOAD_REAL=1
else
  say "  payload            (a real base binary was not found at $BIN_PAYLOAD)"
  PAYLOAD_REAL=0
fi

# ── extraction ──────────────────────────────────────────────────────────────
extract_step() {
  awk -v NAME="$1" '
    BEGIN { state = 0; runindent = 0 }
    state == 0 { if (index($0, "- name: " NAME) > 0) state = 1; next }
    state == 1 {
      if ($0 ~ /^[ ]*run: \|[ ]*$/) { match($0, /^[ ]*/); runindent = RLENGTH; state = 2 }
      else if ($0 ~ /^[ ]*- name: /) { exit 1 }
      next
    }
    state == 2 {
      if ($0 ~ /^[[:space:]]*$/) { print ""; next }
      match($0, /^[ ]*/)
      if (RLENGTH <= runindent) { exit 0 }
      print substr($0, runindent + 3)
    }
    END { if (state != 2) exit 1 }
  ' "$2"
}

expand() { sed -e "s|\${{ matrix.target }}|$1|g" -e "s|\${{ matrix.binary }}|$2|g" -e "s|\${{ matrix.asset }}|$3|g"; }

# A checkout-shaped tree. Identical inputs for every generation, so any archive
# difference is the workflow's and nothing else's.
tree_at() {
  local dir="$1" triple="$2" binary="$3"
  mkdir -p "$dir/target/$triple/release" "$dir/scripts/ast"
  if [ "$PAYLOAD_REAL" = 1 ]; then
    cp "$BIN_PAYLOAD" "$dir/target/$triple/release/$binary"
  else
    printf 'placeholder binary\n' > "$dir/target/$triple/release/$binary"
  fi
  cp "$WT"/scripts/ast/*.py "$WT"/scripts/ast/requirements.txt "$dir/scripts/ast/"
  for f in $LICENSES; do cp "$WT/$f" "$dir/$f"; done
}

# Run one packing leg from one workflow generation; echo the archive path.
pack_unix() {
  local gen="$1" out="$WORK/$2"
  mkdir -p "$out"
  extract_step "Package (Unix)" "$gen" | expand "$M_TARGET" "$M_BINARY" "$M_ASSET" > "$out/step.sh" \
    || return 1
  [ -s "$out/step.sh" ] || return 1
  tree_at "$out/tree" "$M_TARGET" "$M_BINARY"
  (cd "$out/tree" && bash "$out/step.sh") > "$out/step.log" 2>&1 || return 1
  [ -f "$out/tree/$M_ASSET" ] || return 1
  tar -tzf "$out/tree/$M_ASSET" > "$out/members" 2>/dev/null
}

pack_win() {
  local gen="$1" out="$WORK/$2"
  mkdir -p "$out"
  extract_step "Package (Windows)" "$gen" | expand "$W_TARGET" "$W_BINARY" "$W_ASSET" > "$out/step.ps1" \
    || return 1
  [ -s "$out/step.ps1" ] || return 1
  tree_at "$out/tree" "$W_TARGET" "$W_BINARY"
  (cd "$out/tree" && pwsh -NoProfile -NonInteractive -File "$out/step.ps1") > "$out/step.log" 2>&1 || return 1
  [ -f "$out/tree/$W_ASSET" ] || return 1
  python3 -c "import sys,zipfile; print('\n'.join(zipfile.ZipFile(sys.argv[1]).namelist()))" \
    "$out/tree/$W_ASSET" > "$out/members" 2>/dev/null
}

count_licenses() { local f=0; for l in $LICENSES; do grep -qx "$l" "$1" && f=$((f+1)); done; echo "$f"; }

# ── R1 / R2 unix ────────────────────────────────────────────────────────────
head_ "R1 RED — Package (Unix) at $BASE_SHA"
if pack_unix "$WORK/release.base.yml" u_red; then
  n=$(wc -l < "$WORK/u_red/members"); l=$(count_licenses "$WORK/u_red/members")
  say "  $M_ASSET: $n members, $l/4 license files"
  [ "$l" -eq 0 ] && ok "RED: no license file in the tarball" || bad "expected 0/4, got $l/4"
else
  bad "the $BASE_SHA Unix packing step did not produce an archive"
fi

head_ "R2 GREEN — Package (Unix) at $BRANCH_SHA"
if pack_unix "$WORK/release.branch.yml" u_green; then
  n=$(wc -l < "$WORK/u_green/members"); l=$(count_licenses "$WORK/u_green/members")
  say "  $M_ASSET: $n members, $l/4 license files"
  [ "$l" -eq 4 ] && ok "GREEN: all four license files in the tarball" || bad "expected 4/4, got $l/4"
  for f in $LICENSES; do
    grep -qx "$f" "$WORK/u_green/members" || bad "$f is not a top-level member"
  done
  grep -qx "$M_BINARY" "$WORK/u_green/members" || bad "the binary is gone from the tarball"
else
  bad "the branch Unix packing step did not produce an archive"
fi

# ── R3 / R4 windows ─────────────────────────────────────────────────────────
if ! command -v pwsh >/dev/null 2>&1; then
  head_ "R3/R4 windows"
  skip "pwsh not on PATH — the Compress-Archive legs were NOT run on this machine"
else
  head_ "R3 RED — Package (Windows) at $BASE_SHA"
  if pack_win "$WORK/release.base.yml" w_red; then
    n=$(wc -l < "$WORK/w_red/members"); l=$(count_licenses "$WORK/w_red/members")
    say "  $W_ASSET: $n members, $l/4 license files"
    [ "$l" -eq 0 ] && ok "RED: no license file in the zip" || bad "expected 0/4, got $l/4"
  else
    bad "the $BASE_SHA Windows packing step did not produce an archive"
  fi

  head_ "R4 GREEN — Package (Windows) at $BRANCH_SHA"
  if pack_win "$WORK/release.branch.yml" w_green; then
    n=$(wc -l < "$WORK/w_green/members"); l=$(count_licenses "$WORK/w_green/members")
    say "  $W_ASSET: $n members, $l/4 license files"
    [ "$l" -eq 4 ] && ok "GREEN: all four license files in the zip" || bad "expected 4/4, got $l/4"
    for f in $LICENSES; do
      grep -qx "$f" "$WORK/w_green/members" || bad "$f is not a top-level member of the zip"
    done
    grep -qx "$W_BINARY" "$WORK/w_green/members" || bad "the binary is gone from the zip"
  else
    bad "the branch Windows packing step did not produce an archive"
  fi
fi

# ── R5 byte-identity ────────────────────────────────────────────────────────
head_ "R5 byte-identity — the change adds members and alters none"
if [ -f "$WORK/u_red/tree/$M_ASSET" ] && [ -f "$WORK/u_green/tree/$M_ASSET" ]; then
  mkdir -p "$WORK/x_red" "$WORK/x_green"
  tar -xzf "$WORK/u_red/tree/$M_ASSET"   -C "$WORK/x_red"
  tar -xzf "$WORK/u_green/tree/$M_ASSET" -C "$WORK/x_green"
  # Every member of the red archive, byte-for-byte, in the green one.
  same=0; diffn=0
  while IFS= read -r m; do
    [ -f "$WORK/x_red/$m" ] || continue
    if cmp -s "$WORK/x_red/$m" "$WORK/x_green/$m"; then same=$((same+1)); else
      diffn=$((diffn+1)); bad "member differs between the two archives: $m"
    fi
  done < <(cd "$WORK/x_red" && find . -type f | sed 's|^\./||')
  say "  $same file members byte-identical, $diffn differing"
  [ "$diffn" -eq 0 ] && [ "$same" -gt 0 ] \
    && ok "every pre-existing member is byte-identical across the two generations" \
    || bad "byte-identity does not hold over $same compared members"
  # And the member-set delta is exactly the four names.
  delta="$(comm -13 <(sort "$WORK/u_red/members") <(sort "$WORK/u_green/members") | tr '\n' ' ')"
  expected="$(printf '%s\n' $LICENSES | sort | tr '\n' ' ')"
  say "  added members: $delta"
  [ "$delta" = "$expected" ] && ok "the member-set delta is exactly the four license files" \
    || bad "member-set delta is '$delta', expected '$expected'"
else
  skip "R5 needs both Unix archives; one of them was not produced"
fi

# ── R6 the assets users actually received ───────────────────────────────────
head_ "R6 published v0.14.1 assets (release 384286377)"
check_asset() {
  local path="$1" want="$2" kind="$3" label="$4"
  if [ ! -f "$path" ]; then skip "$label not on disk at $path"; return; fi
  local got; got="$(sha256sum "$path" | cut -d' ' -f1)"
  if [ "$got" != "$want" ]; then
    bad "$label sha256 $got does not match the GitHub API's $want — not the published asset"
    return
  fi
  local members lic
  if [ "$kind" = tar ]; then members="$(tar -tzf "$path")"; else
    members="$(python3 -c "import sys,zipfile; print('\n'.join(zipfile.ZipFile(sys.argv[1]).namelist()))" "$path")"
  fi
  lic="$(printf '%s\n' "$members" | grep -ci licen)"
  say "  $label: sha256 verified, $(printf '%s\n' "$members" | wc -l) members, $lic license members"
  [ "$lic" -eq 0 ] && ok "RED on the real artefact: $label shipped with no license text" \
    || bad "$label already carries $lic license member(s)?"
}
check_asset "$ASSETS/$M_ASSET" "$SHA_LINUX" tar "$M_ASSET"
check_asset "$ASSETS/$W_ASSET" "$SHA_WIN"   zip "$W_ASSET"

# ── R7 mutation — cp without the tar member list ────────────────────────────
head_ "R7 mutation — the half-fix this bug invites"
sed 's| scripts/ LICENSE.md LICENSING.md LICENSE-APACHE-2.0 LICENSE-PolyForm-Noncommercial-1.0.0.md| scripts/|' \
  "$WORK/release.branch.yml" > "$WORK/release.mutant.yml"
if cmp -s "$WORK/release.branch.yml" "$WORK/release.mutant.yml"; then
  bad "the mutation changed nothing — the tar line is not shaped as expected"
elif pack_unix "$WORK/release.mutant.yml" u_mut; then
  n=$(wc -l < "$WORK/u_mut/members"); l=$(count_licenses "$WORK/u_mut/members")
  say "  cp line kept, tar member list reverted → $n members, $l/4 license files"
  [ "$l" -eq 0 ] \
    && ok "the instrument catches a copy that never reaches the archive (lock L2)" \
    || bad "expected the mutant to lose all four, it kept $l"
else
  bad "the mutant Unix packing step did not produce an archive"
fi

# ── R8 scope ────────────────────────────────────────────────────────────────
head_ "R8 scope — no src/ change"
srcdiff="$(git -C "$WT" diff --stat "$BASE_SHA..HEAD" -- src/ tests/ | tail -1)"
if [ -z "$srcdiff" ]; then ok "git diff $BASE_SHA..HEAD -- src/ tests/ is empty"; else
  bad "src/ or tests/ moved: $srcdiff"; fi
say "  whole delta:"
git -C "$WT" diff --stat "$BASE_SHA..HEAD" | sed 's/^/    /'

# ── R9 the in-repo test, both ways ──────────────────────────────────────────
head_ "R9 scripts/test-release-packaging.sh"
(cd "$WT" && ./scripts/test-release-packaging.sh) > "$WORK/repo_green.log" 2>&1
rc=$?
say "  on the branch: exit $rc, $(grep -c '^  ok ' "$WORK/repo_green.log") ok / $(grep -c '^  FAIL' "$WORK/repo_green.log") FAIL"
[ "$rc" -eq 0 ] && ok "green on the branch" || bad "expected exit 0, got $rc"

# Same script, workflow reverted to the base sha: it must go red.
cp "$WT/.github/workflows/release.yml" "$WORK/release.restore.yml"
cp "$WORK/release.base.yml" "$WT/.github/workflows/release.yml"
(cd "$WT" && ./scripts/test-release-packaging.sh) > "$WORK/repo_red.log" 2>&1
rc=$?
cp "$WORK/release.restore.yml" "$WT/.github/workflows/release.yml"
if ! cmp -s "$WT/.github/workflows/release.yml" "$WORK/release.branch.yml"; then
  die "failed to restore the branch's release.yml after the red run"
fi
say "  with $BASE_SHA's workflow: exit $rc, $(grep -c '^  FAIL' "$WORK/repo_red.log") FAIL lines"
[ "$rc" -eq 1 ] && ok "red on the pre-fix workflow, and the tree was restored" \
  || bad "expected exit 1, got $rc"

# ── R10 changelog ───────────────────────────────────────────────────────────
head_ "R10 changelog note"
NOTE="$WT/scripts/changelog-notes/0.14.2.md"
if grep -q '^\*\*The release download carries the license\.\*\*' "$NOTE"; then
  ok "the 0.14.2 note carries the paragraph ($(wc -l < "$NOTE") lines, $(grep -c '^\*\*' "$NOTE") ledes)"
else
  bad "the paragraph is not in $NOTE"
fi
if grep -qi 'consent' "$NOTE"; then
  bad "the note mentions consent — auk ruled (b) OMIT"
else
  ok "no consent line in the note (auk's ruling (b))"
fi

# ── verdict ─────────────────────────────────────────────────────────────────
say ""
say "════ $pass PASS / $failn FAIL / $skipn SKIP ════"
[ "$failn" -eq 0 ] && exit 0 || exit 1
