#!/usr/bin/env bash
# Run the release workflow's packing steps for real, and read what they produced.
#
# `test-release-invocations.sh` runs the `changelog.py` command lines the release
# assembles. This runs the two steps that build the asset itself, and it exists
# because nothing did: through v0.14.1 every published archive carried the binary
# and `scripts/` and no license text at all, and no test in this repository could
# have noticed. The engine ships under FSL-1.1-ALv2, whose Redistribution clause
# asks that copies carry the terms; the release asset is the first copy anyone
# receives.
#
#   ./scripts/test-release-packaging.sh
#
# The steps are read out of `.github/workflows/release.yml` by step name and
# executed, rather than re-typed here, so editing the workflow is covered by this
# and a test that passes cannot describe a workflow that no longer exists. The
# Unix leg needs both halves of its fix: `tar` packs the members it is named, so
# a file copied into `staging/` and left off the `tar` line is not in the
# archive. The Windows leg globs `staging/*` and needs no member list.
#
# What this proves is the packing step's MEMBER LIST. It packs a placeholder in
# place of a compiled binary -- this job has no Rust toolchain and does not need
# one -- so it says nothing about the binary's provenance.
#
# Exit 0 = both archives carry the license files. Exit 1 = at least one does not.

set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

WORKFLOW=".github/workflows/release.yml"

# The three files the archives must carry: the engine terms, the plain-English
# guide, and the Apache 2.0 text that every version becomes two years on.
LICENSES="LICENSE.md LICENSING.md LICENSE-APACHE-2.0"

# Matrix values for one representative target. The packing steps are written once
# and expanded per target, so proving one expansion proves the member list for
# all of them.
M_TARGET="x86_64-unknown-linux-gnu"
M_BINARY="base"
M_ASSET="base-linux-x86_64.tar.gz"
W_TARGET="x86_64-pc-windows-msvc"
W_BINARY="base.exe"
W_ASSET="base-windows-x86_64.zip"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail=0
say() { printf '%s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; fail=$((fail + 1)); }
ok()  { printf '  ok    %s\n' "$*"; }
skip(){ printf '  skip  %s\n' "$*"; }

# ── Reading a step out of the workflow ──────────────────────────────────────
# The body of a step's `run: |` block, dedented. No YAML library: a step starts
# at its `- name:` line and its block ends at the first line indented no deeper
# than the `run:` key. Prints nothing and returns 1 if the step is not there.
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
  ' "$WORKFLOW"
}

# `${{ matrix.x }}` -> its value for the target being rehearsed.
expand_matrix() {
  sed -e "s|\${{ matrix.target }}|$1|g" \
      -e "s|\${{ matrix.binary }}|$2|g" \
      -e "s|\${{ matrix.asset }}|$3|g"
}

# A tree shaped like the checkout the runner packs from, in its own directory so
# the two legs cannot see each other's staging/.
scratch_tree() {
  local dir="$1" triple="$2" binary="$3"
  mkdir -p "$dir/target/$triple/release" "$dir/scripts/ast"
  printf 'placeholder for a compiled %s; this test proves the member list, not the binary\n' \
    "$binary" > "$dir/target/$triple/release/$binary"
  cp scripts/ast/*.py scripts/ast/requirements.txt "$dir/scripts/ast/"
  # The real license files, so a rename or a deletion fails this test rather
  # than passing against a copy that no longer matches the repository.
  for f in $LICENSES; do cp "$f" "$dir/$f"; done
}

# ── The license files exist to be packed ────────────────────────────────────
say "Release packaging, from $WORKFLOW at $(git rev-parse --short HEAD)"
say ""
say "license files at the repository root"
missing=0
for f in $LICENSES; do
  if [ -f "$f" ]; then ok "$f"; else bad "$f is not in the repository"; missing=1; fi
done
if [ "$missing" -ne 0 ]; then
  say ""
  say "FAIL -- cannot rehearse packing files that are not there."
  exit 1
fi
say ""

# ── Package (Unix) ──────────────────────────────────────────────────────────
say "Package (Unix)"
if ! extract_step "Package (Unix)" > "$TMP/unix.raw" || [ ! -s "$TMP/unix.raw" ]; then
  bad "no 'Package (Unix)' step with a 'run: |' block in $WORKFLOW -- did it move?"
else
  expand_matrix "$M_TARGET" "$M_BINARY" "$M_ASSET" < "$TMP/unix.raw" > "$TMP/unix.sh"
  ok "read $(grep -cv '^[[:space:]]*$' "$TMP/unix.sh") command lines out of the workflow"
  scratch_tree "$TMP/unix" "$M_TARGET" "$M_BINARY"
  if (cd "$TMP/unix" && bash "$TMP/unix.sh") >"$TMP/unix.out" 2>&1; then
    ok "the step ran"
  else
    bad "the step exited $?; its last lines:"
    sed 's/^/        /' "$TMP/unix.out" | tail -10
  fi
  if [ -f "$TMP/unix/$M_ASSET" ]; then
    tar -tzf "$TMP/unix/$M_ASSET" > "$TMP/unix.members" 2>/dev/null
    ok "$M_ASSET has $(wc -l < "$TMP/unix.members") members"
    for f in $LICENSES; do
      if grep -qx "$f" "$TMP/unix.members"; then
        ok "$M_ASSET carries $f"
      else
        bad "$M_ASSET does not carry $f"
      fi
    done
    # The regression this was written for: the binary and scripts/ must still
    # be there. An archive of nothing but licenses would pass the loop above.
    grep -qx "$M_BINARY" "$TMP/unix.members" && ok "$M_ASSET still carries $M_BINARY" \
      || bad "$M_ASSET does not carry $M_BINARY"
    grep -q '^scripts/ast/onto_ast\.py$' "$TMP/unix.members" && ok "$M_ASSET still carries scripts/ast/" \
      || bad "$M_ASSET does not carry scripts/ast/"
  else
    bad "the step produced no $M_ASSET"
  fi
fi
say ""

# ── Package (Windows) ───────────────────────────────────────────────────────
say "Package (Windows)"
if ! extract_step "Package (Windows)" > "$TMP/win.raw" || [ ! -s "$TMP/win.raw" ]; then
  bad "no 'Package (Windows)' step with a 'run: |' block in $WORKFLOW -- did it move?"
elif ! command -v pwsh >/dev/null 2>&1; then
  # A skip, said out loud. GitHub's ubuntu-latest image ships pwsh, so this
  # branch is for a developer machine without it -- never a silent pass.
  skip "pwsh is not on PATH; the Windows leg was NOT run on this machine"
  skip "its lines, read out of the workflow:"
  sed 's/^/          /' "$TMP/win.raw"
else
  expand_matrix "$W_TARGET" "$W_BINARY" "$W_ASSET" < "$TMP/win.raw" > "$TMP/win.ps1"
  ok "read $(grep -cv '^[[:space:]]*$' "$TMP/win.ps1") command lines out of the workflow"
  scratch_tree "$TMP/win" "$W_TARGET" "$W_BINARY"
  if (cd "$TMP/win" && pwsh -NoProfile -NonInteractive -File "$TMP/win.ps1") >"$TMP/win.out" 2>&1; then
    ok "the step ran under $(pwsh -NoProfile -c '$PSVersionTable.PSVersion.ToString()' 2>/dev/null | tr -d '\r')"
  else
    bad "the step exited $?; its last lines:"
    sed 's/^/        /' "$TMP/win.out" | tail -10
  fi
  if [ -f "$TMP/win/$W_ASSET" ]; then
    if python3 -c "
import sys, zipfile
print('\n'.join(zipfile.ZipFile(sys.argv[1]).namelist()))
" "$TMP/win/$W_ASSET" > "$TMP/win.members" 2>/dev/null; then
      ok "$W_ASSET has $(wc -l < "$TMP/win.members") members"
      for f in $LICENSES; do
        if grep -qx "$f" "$TMP/win.members"; then
          ok "$W_ASSET carries $f"
        else
          bad "$W_ASSET does not carry $f"
        fi
      done
      grep -qx "$W_BINARY" "$TMP/win.members" && ok "$W_ASSET still carries $W_BINARY" \
        || bad "$W_ASSET does not carry $W_BINARY"
      grep -q '^scripts/ast/onto_ast\.py$' "$TMP/win.members" && ok "$W_ASSET still carries scripts/ast/" \
        || bad "$W_ASSET does not carry scripts/ast/"
    else
      bad "could not read $W_ASSET as a zip"
    fi
  else
    bad "the step produced no $W_ASSET"
  fi
fi

say ""
if [ "$fail" -eq 0 ]; then
  say "PASS -- both packing steps ship the license files."
  exit 0
fi
say "FAIL -- $fail check(s) above. A release cut from this commit would ship assets without them."
exit 1
