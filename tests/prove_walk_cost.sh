#!/usr/bin/env bash
# The design-to-code rows a unit test cannot reach, plus the partial gaps beside
# them. All of the same shape: a claim about what the process DID NOT DO, which
# is invisible in the output.
#
#   test 5   one parse per prompt        -> store::GRAPH_LOADS, asserted at 1
#   kite F9  no AST sidecar at prompt    -> graph_query::AST_LOADS, asserted at 0
#   gap      no stderr on a resolver tie -> the hook's stderr, asserted empty
#   test 11  `base graph *` did not move -> byte diff vs a baseline binary
#   test 5b  hook timing, before/after   -> median of 9, both sides release
#
# NEVER touches a live store. The run root is built under /tmp from a FROZEN
# copy, isolated with BASE_HOME, and the frozen copy's md5s are checked before
# and after so "we did not write to it" is proven rather than asserted.
#
# Usage:
#   BASE_BIN=/tmp/base-branch bash tests/prove_walk_cost.sh
#   BASE_BIN=/tmp/base-branch BASELINE=~/.cache/auk/base-main-dd04888 bash ...
#
# Three instrument rules are baked in, each of which has already cost this round
# a bisect that was never needed:
#
#   PROFILE   A debug binary is 5-8x slower on every oxigraph path, which is how
#             F23 read as "+23 s per session start" against a release 0.13.19.
#             The timing leg refuses anything but a clean release build on BOTH
#             sides, and prints profile + md5 for every binary it touches.
#   IDENTITY  `cargo test` rewrites target/release/base with the isolation-guard
#             feature, so the binary must be COPIED ASIDE before any test run.
#             A path under a cargo target dir is refused outright.
#   SLUG      base names the workspace graph from the cwd directory name
#             (`crud::workspace_slug` -> graph/ws/<slug>). A store copied to a
#             dir named `ws` puts every write in graph/ws/ws, an EMPTY graph.
#             The run root's workspace is named from the store's own dominant
#             graph IRI, not from a convenient literal.
set -uo pipefail

B=${BASE_BIN:-}
BASELINE=${BASELINE:-}
FROZEN=${FROZEN:-$HOME/.cache/kite-frozen}
ASTSRC=${ASTSRC:-$HOME/chris-ai-systems/.base-ast/ast.ttl}
R=/tmp/plover-cost

fail=0
skipped=0
ok()   { printf '  ok    %s\n' "$*"; }
bad()  { printf '  FAIL  %s\n' "$*"; fail=$((fail + 1)); }
skip() { printf '  SKIP  %s\n' "$*"; skipped=$((skipped + 1)); }

if [ -z "$B" ]; then
  cat >&2 <<'EOF'
BASE_BIN is required. Point it at a binary you copied aside, not at a path
inside CARGO_TARGET_DIR: `cargo test` rewrites that tree underneath a running
harness AND rebuilds it with the isolation-guard feature.

  cargo build --release --bin base
  cp "$CARGO_TARGET_DIR/release/base" /tmp/base-branch
  BASE_BIN=/tmp/base-branch bash tests/prove_walk_cost.sh

EOF
  exit 2
fi
[ -x "$B" ] || { echo "BASE_BIN=$B is not executable" >&2; exit 2; }

# ── every binary states its profile before it is trusted ────────────────────
# Prints one line per binary and returns 1 unless it is a clean release build
# that no `cargo test` has rewritten.
profile_of() {
  local bin="$1" dbg guard rc=0
  dbg=$(readelf -S "$bin" 2>/dev/null | grep -c '\.debug_')
  guard=$(nm -C "$bin" 2>/dev/null | grep -c write_back_seamed)
  printf '  %-22s %-9s md5 %s  debug-sections=%s guard-symbol=%s\n' \
    "$(basename "$bin")" "$("$bin" --version 2>&1 | awk '{print $NF}')" \
    "$(md5sum "$bin" | cut -c1-12)" "$dbg" "$guard"
  [ "$dbg" -eq 0 ]   || rc=1
  [ "$guard" -eq 0 ] || rc=1
  return $rc
}


# The baseline must prove WHICH binary it is, and --version cannot do it: a build
# of the merge base still reports 0.13.19, because the version bump has not
# happened yet. Symbols can. Two-sided, because both errors are silent:
#   has is_transient_iri   -> PR #50 is in it, so it is not the 0.13.19 release
#   lacks maps_from_store  -> this fork is NOT in it, so it is not a build of the
#                             branch under test being diffed against itself
baseline_is_merge_base() {
  local bin="$1" has lacks
  has=$(nm -C "$bin" 2>/dev/null | grep -c is_transient_iri)
  lacks=$(nm -C "$bin" 2>/dev/null | grep -c maps_from_store)
  printf '  baseline identity: is_transient_iri=%s (want >0) maps_from_store=%s (want 0)\n' "$has" "$lacks"
  [ "$has" -gt 0 ] && [ "$lacks" -eq 0 ]
}

case "$B" in
  */target/*)
    echo "BASE_BIN=$B is inside a cargo target dir. cargo test rewrites it mid-run; copy it aside." >&2
    exit 2 ;;
esac

# ── the run root ────────────────────────────────────────────────────────────
[ -f "$FROZEN/gbl/.base-gbl/.base/graph.nq" ] || { echo "no frozen global store at $FROZEN" >&2; exit 2; }
[ -f "$FROZEN/ws/.base/graph.nq" ]            || { echo "no frozen workspace store at $FROZEN" >&2; exit 2; }

# Prove the frozen copy is what it claims BEFORE reading it.
if [ -f "$FROZEN/MD5SUMS" ]; then
  ( cd "$FROZEN" && md5sum -c MD5SUMS --quiet ) \
    || { echo "frozen copy md5 mismatch at $FROZEN — refusing to measure against it" >&2; exit 2; }
fi

# The workspace directory name IS the graph name. Take it from the store's own
# dominant graph IRI so the copy lands in the same named graph the records are
# already in; a literal `ws` here silently makes every write target an empty one.
WS_SLUG=$(grep -oE 'ontology#graph/ws/[A-Za-z0-9_-]+' "$FROZEN/ws/.base/graph.nq" \
            | sed 's#.*/##' | sort | uniq -c | sort -rn | head -1 | awk '{print $2}')
[ -n "$WS_SLUG" ] || { echo "could not read a workspace slug out of the frozen store" >&2; exit 2; }
WS=$R/$WS_SLUG

rm -rf "$R"
mkdir -p "$R/home/.base-gbl/.base" "$R/home/.claude" "$WS/.base" "$WS/.base-ast"
cp "$FROZEN/gbl/.base-gbl/.base/graph.nq" "$R/home/.base-gbl/.base/graph.nq"
[ -f "$FROZEN/gbl/.base-gbl/domains.toml" ] && cp "$FROZEN/gbl/.base-gbl/domains.toml" "$R/home/.base-gbl/domains.toml"
cp "$FROZEN/ws/.base/graph.nq" "$WS/.base/graph.nq"
[ -f "$FROZEN/ws/.base/domains.toml" ] && cp "$FROZEN/ws/.base/domains.toml" "$WS/.base/domains.toml"

# devmode on: the parses line rides in the devmode block and nowhere else.
printf '[devmode]\nenabled = true\n' > "$R/home/.base-gbl/base.toml"

AST_NOTE="absent"
if [ -f "$ASTSRC" ]; then
  cp "$ASTSRC" "$WS/.base-ast/ast.ttl"
  AST_NOTE="$(stat -c %s "$WS/.base-ast/ast.ttl") bytes, copied from $ASTSRC"
fi

echo "walk cost harness"
echo "  binaries"
profile_of "$B" || true
[ -n "$BASELINE" ] && [ -x "$BASELINE" ] && { profile_of "$BASELINE" || true; }
echo "  run root   $R   (BASE_HOME=$R/home, cwd=$WS)"
echo "  ws slug    $WS_SLUG   (from the store's own graph IRI, not a literal)"
echo "  frozen     $FROZEN   md5s verified"
echo "  global     $(md5sum "$R/home/.base-gbl/.base/graph.nq" | cut -c1-12)  $(stat -c %s "$R/home/.base-gbl/.base/graph.nq") bytes"
echo "  workspace  $(md5sum "$WS/.base/graph.nq" | cut -c1-12)  $(stat -c %s "$WS/.base/graph.nq") bytes"
echo "  ast map    $AST_NOTE"
echo "  baseline   ${BASELINE:-<unset — the diff and timing legs will SKIP>}"
echo "  load       $(cut -d' ' -f1-3 /proc/loadavg)"
echo

# stdout and stderr kept APART: one leg is entirely about which one the
# resolver's tie note came out on.
fire() { # fire <prompt> <session> <outfile> <errfile>
  printf '{"session_id":"%s","prompt":"%s"}' "$2" "$1" \
    | ( cd "$WS" && BASE_HOME="$R/home" BASE_NO_AUTO_UPDATE=1 "$B" hook user-prompt-submit ) \
        > "$3" 2> "$4"
}

parses_line() { grep -o 'parses: graph=[0-9]* ast=[0-9]*' "$1" | tail -1; }

# ── test 5: the walk adds no second parse ───────────────────────────────────
echo "── test 5: one parse per prompt ──"
# Prompts 1 and 2 are lean mode; the walk only runs from 3 on. Same session id.
fire 'hello' s1 "$R/1a.out" "$R/1a.err"
fire 'hello' s1 "$R/1b.out" "$R/1b.err"
fire 'where are we on `basemode`' s1 "$R/1c.out" "$R/1c.err"
p=$(parses_line "$R/1c.out")
case "$p" in
  "parses: graph=1 ast=0") ok "resolving prompt: $p" ;;
  "parses: graph=1"*)      bad "graph parsed once but the AST sidecar loaded: $p" ;;
  "")                      bad "no parses line — is devmode on? see $R/1c.out" ;;
  *)                       bad "expected graph=1 ast=0, got: $p" ;;
esac

# The case where an accidental second parse hides best: no block is rendered, so
# nothing in the output looks different at all.
fire 'hello' s2 "$R/2a.out" "$R/2a.err"
fire 'hello' s2 "$R/2b.out" "$R/2b.err"
fire 'a sentence naming nothing at all' s2 "$R/2c.out" "$R/2c.err"
p2=$(parses_line "$R/2c.out")
case "$p2" in
  "parses: graph=1 ast=0") ok "non-resolving prompt: $p2" ;;
  "")                      bad "no parses line on the non-resolving prompt" ;;
  *)                       bad "expected graph=1 ast=0, got: $p2" ;;
esac
echo

# ── kite F9: the sidecar is present and still not parsed ────────────────────
echo "── kite F9: include_ast=false, with a real sidecar at cwd ──"
if [ ! -f "$WS/.base-ast/ast.ttl" ]; then
  skip "no AST sidecar at $ASTSRC — ast=0 proves nothing without one to skip"
else
  sz=$(stat -c%s "$WS/.base-ast/ast.ttl")
  case "$p" in
    *"ast=0") ok "a ${sz}-byte sidecar sat at cwd and was never parsed" ;;
    *)        bad "sidecar at cwd was parsed on the prompt path: $p" ;;
  esac
fi
echo

# ── the resolver tie is reported in devmode, never on stderr ────────────────
# `pick` prints "note: N ... records are named X" on stderr. Right for a CLI,
# wrong for a hook, where it is operator-visible noise on every prompt.
echo "── the resolver tie never reaches stderr ──"
if grep -q '^note: ' "$R/1c.err" "$R/2c.err"; then
  bad "resolver tie note reached stderr"
  grep -n '^note: ' "$R/1c.err" "$R/2c.err" | head -4
else
  ok "stderr carries no resolver note on either prompt"
fi
if grep -q 'same-kind ties' "$R/1c.out"; then
  ok "the tie count is in the devmode block, where it belongs"
else
  printf '  note  no tie exercised by this store — stderr silence is the only assertion here\n'
fi
echo

# ── test 11: `base graph *` byte-identical to the merge base ────────────────
echo "── test 11: base graph get-node / neighbors / path vs the merge base ──"
if [ -z "$BASELINE" ]; then
  skip "BASELINE unset — set it to a binary built from main dd048888 (NOT 0.13.19: PR #50 changed which nodes load)"
elif [ ! -x "$BASELINE" ]; then
  skip "BASELINE=$BASELINE is not executable"
elif ! baseline_is_merge_base "$BASELINE"; then
  skip "BASELINE is not a merge-base build — see the identity line above"
else
  # Names taken FROM the store, a few of EACH kind rather than the first N
  # alphabetically: the resolver's total order is kind-first, and a sample that
  # is all decisions exercises one branch of it and calls the order proven.
  SLUG_N=${SLUG_N:-3}
  SLUGS=()
  for k in project domain decision doc note rule; do
    while read -r s; do
      [ -n "$s" ] && SLUGS+=( "$s" )
    done < <(grep -oE "ontology#$k/[A-Za-z0-9_-]+" "$WS/.base/graph.nq" \
               | sed 's#.*/##' | sort -u | head -"$SLUG_N")
  done
  if [ "${#SLUGS[@]}" -lt 2 ]; then
    skip "frozen workspace store yielded ${#SLUGS[@]} slug(s) — too few to diff"
  else
    diffs=0
    run_both() { # run_both <tag> <args...>
      local tag="$1"; shift
      ( cd "$WS" && BASE_HOME="$R/home" "$B"        "$@" ) > "$R/new.$tag" 2> "$R/new.$tag.err"
      ( cd "$WS" && BASE_HOME="$R/home" "$BASELINE" "$@" ) > "$R/old.$tag" 2> "$R/old.$tag.err"
      if ! diff -u "$R/old.$tag" "$R/new.$tag" > "$R/d.$tag"; then
        diffs=$((diffs + 1)); printf '  FAIL  %s moved:\n' "$tag"; sed -n '1,12p' "$R/d.$tag"
      fi
      # stderr too: the tie note is stderr, and `pick` was touched by this fork.
      if ! diff -u "$R/old.$tag.err" "$R/new.$tag.err" > "$R/de.$tag"; then
        diffs=$((diffs + 1)); printf '  FAIL  %s stderr moved:\n' "$tag"; sed -n '1,8p' "$R/de.$tag"
      fi
    }
    for s in "${SLUGS[@]}"; do
      run_both "getnode-$s" graph get-node "$s"
      run_both "nb1-$s"     graph neighbors "$s"
      run_both "nb2-$s"     graph neighbors "$s" --depth 2
    done
    run_both "path" graph path "${SLUGS[0]}" "${SLUGS[1]}"
    n=$(( ${#SLUGS[@]} * 3 + 1 ))
    if [ "$diffs" -eq 0 ]; then
      ok "$n command(s) over ${#SLUGS[@]} node(s) of 6 kinds: stdout and stderr byte-identical"
    else
      bad "$diffs of $n command(s) moved against the merge base"
    fi
  fi
fi
echo

# ── test 5b: hook wall time, this branch vs the merge base ──────────────────
echo "── test 5b: hook wall time vs the merge base ──"
if [ -z "$BASELINE" ] || [ ! -x "$BASELINE" ]; then
  skip "BASELINE unset — timing needs a before as well as an after"
elif ! baseline_is_merge_base "$BASELINE" >/dev/null 2>&1; then
  skip "BASELINE is not a merge-base build — timing it would compare the wrong pair"
elif ! profile_of "$B" >/dev/null 2>&1 || ! profile_of "$BASELINE" >/dev/null 2>&1; then
  # This is the F23 defect exactly: a debug binary is 5-8x slower on every
  # oxigraph path, so a debug branch against a release baseline reports the
  # optimiser as this fork's cost. Byte identity above is unaffected, since
  # opt level does not change what a command prints, so only this leg gates.
  skip "timing needs a clean release build on BOTH sides (see the profile lines above)"
elif [ "$(pgrep -c 'cargo|rustc' || true)" != "0" ]; then
  skip "cargo/rustc running — timings would be load-polluted"
else
  time_hook() { # time_hook <binary> <runs> -> median ms
    local bin="$1" runs="$2" i t0 t1
    local -a all=()
    for i in $(seq 1 "$runs"); do
      t0=$(date +%s%N)
      printf '{"session_id":"t%s","prompt":"where are we on `basemode`"}' "$i" \
        | ( cd "$WS" && BASE_HOME="$R/home" BASE_NO_AUTO_UPDATE=1 "$bin" hook user-prompt-submit ) \
            > /dev/null 2>&1
      t1=$(date +%s%N)
      all+=( $(( (t1 - t0) / 1000000 )) )
    done
    printf '%s\n' "${all[@]}" | sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}'
  }
  # Warm both: the first run of either pays page cache for an 11 MB store.
  time_hook "$BASELINE" 2 > /dev/null; time_hook "$B" 2 > /dev/null
  old_ms=$(time_hook "$BASELINE" 9)
  new_ms=$(time_hook "$B" 9)
  delta=$(( new_ms - old_ms ))
  printf '  median of 9: merge base %s ms, this branch %s ms, delta %+d ms\n' "$old_ms" "$new_ms" "$delta"
  if [ "$delta" -le 150 ]; then
    ok "within the fork's p50 + 150 ms budget"
  else
    bad "over budget: +${delta} ms against a 150 ms allowance"
  fi
fi
echo

# ── the frozen copy is still what it was ────────────────────────────────────
if [ -f "$FROZEN/MD5SUMS" ]; then
  if ( cd "$FROZEN" && md5sum -c MD5SUMS --quiet ); then
    ok "frozen copy unchanged — this harness wrote nothing outside $R"
  else
    bad "FROZEN COPY MUTATED. Something in this run wrote to $FROZEN"
  fi
fi

echo "artefacts in $R"
if [ "$fail" -eq 0 ]; then
  [ "$skipped" -eq 0 ] && { echo "PASS — every leg ran and passed."; exit 0; }
  echo "PASS with $skipped SKIP(s) — the legs that ran passed; the skipped ones proved nothing."
  exit 0
fi
echo "FAIL — $fail problem(s), $skipped skipped."
exit 1
