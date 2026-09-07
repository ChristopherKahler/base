#!/usr/bin/env bash
# The three design-to-code rows a unit test cannot reach, plus the two partial
# gaps beside them. All five are the same shape: a claim about what the process
# DID NOT DO, which is invisible in the output.
#
#   test 5   one parse per prompt        -> store::GRAPH_LOADS, asserted at 1
#   kite F9  no AST sidecar at prompt    -> graph_query::AST_LOADS, asserted at 0
#   gap      no stderr on a resolver tie -> the hook's stderr, asserted empty
#   test 11  `base graph *` did not move -> byte diff vs a baseline binary
#   test 5b  hook timing, before/after   -> same, with the baseline binary
#
# NEVER touches a live store. The run root is built under /tmp from a FROZEN
# copy, and the AST sidecar is COPIED in, so cwd is never inside a real
# workspace. Every path and md5 is printed, because a harness that silently ran
# against the wrong store is worse than one that did not run.
#
# Usage:
#   bash tests/prove_walk_cost.sh
#   BASELINE=~/.cache/auk/base-main-dd04888 bash tests/prove_walk_cost.sh
#
# Legs 4 and 5 need a binary built from the branch's MERGE BASE (main dd048888),
# not 0.13.19: PR #50 changed which nodes load, so a 0.13.19 diff would show
# fork 4's work and call it this fork's. Without BASELINE they SKIP, loudly, and
# the harness still exits non-zero if anything it DID run failed.
set -uo pipefail

TARGET=${CARGO_TARGET_DIR:-/home/chriskahler/ops-sys/toolbox/frameworks/00-kit-base/target}
B=${BASE_BIN:-$TARGET/debug/base}
BASELINE=${BASELINE:-}
FROZEN=${FROZEN:-$HOME/.cache/kite-frozen}
ASTSRC=${ASTSRC:-$HOME/chris-ai-systems/.base-ast/ast.ttl}
R=/tmp/plover-cost

fail=0
skipped=0
ok()   { printf '  ok    %s\n' "$*"; }
bad()  { printf '  FAIL  %s\n' "$*"; fail=$((fail + 1)); }
skip() { printf '  SKIP  %s\n' "$*"; skipped=$((skipped + 1)); }

[ -x "$B" ] || { echo "no binary at $B — run cargo build --bin base first"; exit 2; }

# ── the run root ────────────────────────────────────────────────────────────
# HOME and cwd both under /tmp. The frozen store is COPIED, not linked: base
# writes session state, and a symlink would put those writes back on the source.
[ -f "$FROZEN/gbl/.base-gbl/.base/graph.nq" ] || { echo "no frozen global store at $FROZEN"; exit 2; }
[ -f "$FROZEN/ws/.base/graph.nq" ]            || { echo "no frozen workspace store at $FROZEN"; exit 2; }

rm -rf "$R"; mkdir -p "$R/home" "$R/ws/.base" "$R/ws/.base-ast"
mkdir -p "$R/home/.base-gbl/.base" "$R/home/.claude"
cp "$FROZEN/gbl/.base-gbl/.base/graph.nq" "$R/home/.base-gbl/.base/graph.nq"
[ -f "$FROZEN/gbl/.base-gbl/domains.toml" ] && cp "$FROZEN/gbl/.base-gbl/domains.toml" "$R/home/.base-gbl/domains.toml"
cp "$FROZEN/ws/.base/graph.nq" "$R/ws/.base/graph.nq"
[ -f "$FROZEN/ws/.base/domains.toml" ] && cp "$FROZEN/ws/.base/domains.toml" "$R/ws/.base/domains.toml"

# devmode on: the parses line rides in the devmode block and nowhere else.
printf '[devmode]\nenabled = true\n' > "$R/home/.base-gbl/base.toml"

AST_NOTE="absent"
if [ -f "$ASTSRC" ]; then
  cp "$ASTSRC" "$R/ws/.base-ast/ast.ttl"
  AST_NOTE="$(du -h "$R/ws/.base-ast/ast.ttl" | cut -f1) copied from $ASTSRC"
fi

echo "base $("$B" --version | awk '{print $NF}') — walk cost harness"
echo "  run root   $R   (HOME=$R/home, cwd=$R/ws)"
echo "  frozen     $FROZEN"
echo "  global md5 $(md5sum "$R/home/.base-gbl/.base/graph.nq" | cut -d' ' -f1)  $(du -h "$R/home/.base-gbl/.base/graph.nq" | cut -f1)"
echo "  ws     md5 $(md5sum "$R/ws/.base/graph.nq" | cut -d' ' -f1)  $(du -h "$R/ws/.base/graph.nq" | cut -f1)"
echo "  ast map    $AST_NOTE"
echo "  baseline   ${BASELINE:-<unset — legs 4 and 5 will SKIP>}"
echo

# Fire the hook from the run root's workspace. stdout and stderr kept APART:
# leg 3 is entirely about which one the tie note came out on.
fire() { # fire <prompt> <session> <outfile> <errfile>
  printf '{"session_id":"%s","prompt":"%s"}' "$2" "$1" \
    | ( cd "$R/ws" && HOME="$R/home" BASE_NO_AUTO_UPDATE=1 "$B" hook user-prompt-submit ) \
        > "$3" 2> "$4"
}

parses_line() { grep -o 'parses: graph=[0-9]* ast=[0-9]*' "$1" | tail -1; }

# ── leg 1: one parse on a prompt that RESOLVES something ────────────────────
echo "── test 5: the walk adds no second parse ──"
# Prompt 1 and 2 are lean mode; the walk only runs from 3 on. Same session id.
fire 'hello' s1 "$R/1a.out" "$R/1a.err"
fire 'hello' s1 "$R/1b.out" "$R/1b.err"
fire 'where are we on `basemode`' s1 "$R/1c.out" "$R/1c.err"
p=$(parses_line "$R/1c.out")
[ -n "$p" ] || bad "no parses line — is devmode on? ($R/1c.out)"
case "$p" in
  "parses: graph=1 ast=0") ok "resolving prompt: $p" ;;
  "parses: graph=1"*)      bad "graph parsed once but the AST sidecar loaded: $p" ;;
  "")                      : ;;
  *)                       bad "expected graph=1 ast=0, got: $p" ;;
esac

# ── leg 2: one parse on a prompt that resolves NOTHING ──────────────────────
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

# ── leg 3: kite F9, the sidecar is present and still not parsed ─────────────
echo "── kite F9: include_ast=false, with a real sidecar at cwd ──"
if [ ! -f "$R/ws/.base-ast/ast.ttl" ]; then
  skip "no AST sidecar available at $ASTSRC — ast=0 proves nothing without one"
else
  sz=$(stat -c%s "$R/ws/.base-ast/ast.ttl")
  case "$p" in
    *"ast=0") ok "a ${sz}-byte sidecar sat at cwd and was never parsed" ;;
    *)        bad "sidecar at cwd was parsed on the prompt path: $p" ;;
  esac
fi
echo

# ── leg 4: the resolver's tie note never reaches stderr ─────────────────────
# `pick` prints "note: N ... records are named X" on stderr. Right for a CLI,
# wrong for a hook, where it is operator-visible noise on every prompt. The
# count goes to the devmode line instead, so stderr must be clean.
echo "── the resolver tie is reported in devmode, never on stderr ──"
for f in "$R"/1c.err "$R"/2c.err; do
  if grep -q '^note: ' "$f"; then
    bad "resolver tie note reached stderr ($f)"
    sed -n '1,3p' "$f"
  fi
done
grep -q '^note: ' "$R"/1c.err "$R"/2c.err || ok "stderr carries no resolver note on either prompt"
if grep -q 'same-kind ties' "$R/1c.out"; then
  ok "the tie count is in the devmode block, where it belongs"
else
  printf '  note  no tie exercised by this store — stderr silence is the only assertion here\n'
fi
echo

# ── leg 5: test 11, `base graph *` byte-identical to the merge base ─────────
echo "── test 11: base graph get-node / neighbors / path vs the merge base ──"
if [ -z "$BASELINE" ]; then
  skip "BASELINE unset — set it to a binary built from main dd048888 (NOT 0.13.19: PR #50 changed which nodes load)"
elif [ ! -x "$BASELINE" ]; then
  skip "BASELINE=$BASELINE is not executable"
else
  echo "  baseline $("$BASELINE" --version | awk '{print $NF}')  md5 $(md5sum "$BASELINE" | cut -d' ' -f1)"
  # Names taken FROM the store, so this does not rot against a hard-coded slug.
  # A few of EACH kind, not the first N alphabetically: the resolver's total
  # order is kind-first, so a sample that is all decisions would exercise one
  # branch of `kind_rank` and call the whole order proven.
  SLUG_N=${SLUG_N:-3}
  SLUGS=()
  for k in project domain decision doc note rule; do
    while read -r s; do
      [ -n "$s" ] && SLUGS+=( "$s" )
    done < <(grep -oE "ontology#$k/[A-Za-z0-9_-]+" "$R/ws/.base/graph.nq" \
               | sed 's#.*/##' | sort -u | head -"$SLUG_N")
  done
  if [ "${#SLUGS[@]}" -lt 2 ]; then
    skip "frozen workspace store yielded ${#SLUGS[@]} slugs — too few to diff"
  else
    diffs=0
    run_both() { # run_both <tag> <args...>
      local tag="$1"; shift
      ( cd "$R/ws" && HOME="$R/home" "$B"        "$@" ) > "$R/new.$tag" 2> "$R/new.$tag.err"
      ( cd "$R/ws" && HOME="$R/home" "$BASELINE" "$@" ) > "$R/old.$tag" 2> "$R/old.$tag.err"
      if ! diff -u "$R/old.$tag" "$R/new.$tag" > "$R/d.$tag"; then
        diffs=$((diffs + 1))
        printf '  FAIL  %s moved:\n' "$tag"
        sed -n '1,12p' "$R/d.$tag"
      fi
      # stderr too: the tie note is stderr, and `pick` was touched by this fork.
      if ! diff -u "$R/old.$tag.err" "$R/new.$tag.err" > "$R/de.$tag"; then
        diffs=$((diffs + 1))
        printf '  FAIL  %s stderr moved:\n' "$tag"
        sed -n '1,8p' "$R/de.$tag"
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
      ok "$n command(s) over ${#SLUGS[@]} node(s): stdout and stderr byte-identical"
    else
      bad "$diffs of $n command(s) moved against the merge base"
    fi
  fi
fi
echo

# ── leg 6: test 5b, hook timing before and after ────────────────────────────
echo "── test 5b: hook wall time, this branch vs the merge base ──"
if [ -z "$BASELINE" ] || [ ! -x "$BASELINE" ]; then
  skip "BASELINE unset — timing needs a before as well as an after"
else
  time_hook() { # time_hook <binary> <runs> -> median ms
    local bin="$1" runs="$2" i t0 t1 ms
    local -a all=()
    for i in $(seq 1 "$runs"); do
      t0=$(date +%s%N)
      printf '{"session_id":"t%s","prompt":"where are we on `basemode`"}' "$i" \
        | ( cd "$R/ws" && HOME="$R/home" BASE_NO_AUTO_UPDATE=1 "$bin" hook user-prompt-submit ) \
            > /dev/null 2>&1
      t1=$(date +%s%N)
      all+=( $(( (t1 - t0) / 1000000 )) )
    done
    printf '%s\n' "${all[@]}" | sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}'
  }
  # Warm both: the first run pays page cache for an 11 MB store either way.
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

# Session state written by this harness stays in the run root and nowhere else.
echo "artefacts in $R (nothing was written outside it)"
if [ "$fail" -eq 0 ]; then
  [ "$skipped" -eq 0 ] && { echo "PASS — every leg ran and passed."; exit 0; }
  echo "PASS with $skipped SKIP(s) — the legs that ran passed; the skipped ones proved nothing."
  exit 0
fi
echo "FAIL — $fail problem(s), $skipped skipped."
exit 1
