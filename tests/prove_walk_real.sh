#!/usr/bin/env bash
# The fork's acceptance lines 1-3, as commands rather than as prose.
#
#   1  a name with no keyword trigger serves its records, and the merge base
#      serves nothing from it
#   2  a client project serves its entity, documents and work, each line
#      carrying the relation that put it there
#   3  an ambiguous name resolves the same way 15 times: one sha256
#
# Line 4 (a superseded record never appears) stays blocked: the drop-superseded
# predicate is a named no-op until the drift fork lands `resolve_head`.
#
# Runs against a FRESH copy of the frozen store on every invocation, under a run
# root named from the store's own graph IRI, isolated with BASE_HOME. The live
# store is never opened.
#
# Usage:
#   BASE_BIN=/tmp/base-branch BASELINE=~/.cache/auk/base-main-dd04888 \
#     bash tests/prove_walk_real.sh
set -uo pipefail

B=${BASE_BIN:-}
BASELINE=${BASELINE:-}
FROZEN=${FROZEN:-$HOME/.cache/kite-frozen}
R=/tmp/plover-real
RUNS=${RUNS:-15}

fail=0
skipped=0
ok()   { printf '  ok    %s\n' "$*"; }
bad()  { printf '  FAIL  %s\n' "$*"; fail=$((fail + 1)); }
skip() { printf '  SKIP  %s\n' "$*"; skipped=$((skipped + 1)); }

[ -n "$B" ] || { echo "BASE_BIN is required (a release binary copied aside)" >&2; exit 2; }
[ -x "$B" ] || { echo "BASE_BIN=$B is not executable" >&2; exit 2; }
case "$B" in
  */target/*)
    echo "BASE_BIN=$B is inside a cargo target dir; copy it aside." >&2; exit 2 ;;
esac

# Same two-sided identity check the cost harness uses: a merge-base build still
# reports 0.13.19, so --version cannot tell it from the release.
baseline_is_merge_base() {
  local bin="$1" has lacks
  has=$(nm -C "$bin" 2>/dev/null | grep -c is_transient_iri)
  lacks=$(nm -C "$bin" 2>/dev/null | grep -c maps_from_store)
  printf '  baseline identity: is_transient_iri=%s (want >0) maps_from_store=%s (want 0)\n' "$has" "$lacks"
  [ "$has" -gt 0 ] && [ "$lacks" -eq 0 ]
}

[ -f "$FROZEN/ws/.base/graph.nq" ] || { echo "no frozen workspace store at $FROZEN" >&2; exit 2; }
if [ -f "$FROZEN/MD5SUMS" ]; then
  ( cd "$FROZEN" && md5sum -c MD5SUMS --quiet ) \
    || { echo "frozen copy md5 mismatch at $FROZEN" >&2; exit 2; }
fi

# The cwd directory name IS the graph name (crud::workspace_slug). Read it from
# the store rather than naming it, or the copy lands in a graph nothing is in.
WS_SLUG=$(grep -oE 'ontology#graph/ws/[A-Za-z0-9_-]+' "$FROZEN/ws/.base/graph.nq" \
            | sed 's#.*/##' | sort | uniq -c | sort -rn | head -1 | awk '{print $2}')
[ -n "$WS_SLUG" ] || { echo "could not read a workspace slug out of the frozen store" >&2; exit 2; }
WS=$R/$WS_SLUG

rm -rf "$R"
mkdir -p "$R/home/.base-gbl/.base" "$R/home/.claude" "$WS/.base"
cp "$FROZEN/gbl/.base-gbl/.base/graph.nq" "$R/home/.base-gbl/.base/graph.nq"
[ -f "$FROZEN/gbl/.base-gbl/domains.toml" ] && cp "$FROZEN/gbl/.base-gbl/domains.toml" "$R/home/.base-gbl/domains.toml"
cp "$FROZEN/ws/.base/graph.nq" "$WS/.base/graph.nq"
[ -f "$FROZEN/ws/.base/domains.toml" ] && cp "$FROZEN/ws/.base/domains.toml" "$WS/.base/domains.toml"

echo "real-store acceptance, lines 1-3"
echo "  binary     $("$B" --version | awk '{print $NF}')  md5 $(md5sum "$B" | cut -c1-12)"
echo "  run root   $R   (BASE_HOME=$R/home, cwd=$WS)"
echo "  ws slug    $WS_SLUG"
echo "  workspace  $(md5sum "$WS/.base/graph.nq" | cut -c1-12)  $(stat -c %s "$WS/.base/graph.nq") bytes"
echo "  baseline   ${BASELINE:-<unset — line 1's negative half will SKIP>}"
echo

# A fresh session id per call, so nothing is suppressed by the once-per-session
# dedup. Prompts 1 and 2 of any session are lean mode, so every call warms twice.
walk_block() { # walk_block <binary> <prompt> <session> -> the base-context block
  local bin="$1" prompt="$2" sid="$3" i
  for i in 1 2; do
    printf '{"session_id":"%s","prompt":"warm"}' "$sid" \
      | ( cd "$WS" && BASE_HOME="$R/home" BASE_NO_AUTO_UPDATE=1 "$bin" hook user-prompt-submit ) \
          > /dev/null 2>&1
  done
  printf '{"session_id":"%s","prompt":"%s"}' "$sid" "$prompt" \
    | ( cd "$WS" && BASE_HOME="$R/home" BASE_NO_AUTO_UPDATE=1 "$bin" hook user-prompt-submit ) \
        2>/dev/null \
    | sed -n '/^<base-context /,/^<\/base-context>/p'
}

# ── line 1: a name with no keyword trigger reaches the prompt ───────────────
echo "── line 1: naming \`basemode\` serves its records; the merge base serves none ──"
NEW1="$R/line1.new"
walk_block "$B" 'where are we on `basemode`' L1 > "$NEW1"
if [ ! -s "$NEW1" ]; then
  bad "the branch served no walk block at all for \`basemode\`"
else
  n=$(grep -c '^  ' "$NEW1")
  ok "branch served $n record(s):"
  sed -n '1,8p' "$NEW1" | sed 's/^/        /'
fi

if [ -z "$BASELINE" ] || [ ! -x "$BASELINE" ]; then
  skip "BASELINE unset — cannot show the merge base serves nothing from it"
elif ! baseline_is_merge_base "$BASELINE"; then
  skip "BASELINE is not a merge-base build — see the identity line above"
else
  OLD1="$R/line1.old"
  walk_block "$BASELINE" 'where are we on `basemode`' L1b > "$OLD1"
  if [ -s "$OLD1" ]; then
    bad "the merge base ALSO served a walk block — this fork is not what put it there"
    sed -n '1,6p' "$OLD1" | sed 's/^/        /'
  else
    ok "merge base served nothing: the block above is this fork's doing"
  fi
fi
echo

# ── line 2: a client project, with the relation on every line ───────────────
# The fork's wording is "the client entity, the people and the documents". This
# store has 973 entity/ nodes and ZERO person/ nodes -- there is no person kind
# in the ontology here -- so "people" is asserted as what the data can carry:
# the entity, its documents, and the work filed against it. Asserting a kind the
# store cannot produce would be a test that fails forever or gets quietly
# relaxed, and neither is worth having.
echo "── line 2: a client project serves its context, each line showing the relation ──"
NEW2="$R/line2.new"
walk_block "$B" 'where are we on `renda group`' L2 > "$NEW2"
if [ ! -s "$NEW2" ]; then
  bad "no walk block for the client project"
else
  hdr=$(head -1 "$NEW2")
  echo "        $hdr"
  sed -n '2,12p' "$NEW2" | sed 's/^/        /'
  case "$hdr" in
    *'hops="2"'*) ok "resolved as a hub, so it walked two hops" ;;
    *)            bad "expected hops=2 for a project hub, got: $hdr" ;;
  esac
  # Every record line must carry its relation: "  kind  label — relation".
  bare=$(grep -c '^  [a-z]' "$NEW2")
  withrel=$(grep -c '^  [a-z].* — ' "$NEW2")
  if [ "$bare" -gt 0 ] && [ "$bare" -eq "$withrel" ]; then
    ok "all $withrel record line(s) name the relation that put them there"
  else
    bad "$((bare - withrel)) of $bare record line(s) carry no relation"
  fi
  kinds=$(grep -oE '^  [a-z]+' "$NEW2" | tr -d ' ' | sort -u | tr '\n' ' ')
  ok "kinds served: ${kinds:-none}"
fi
echo

# ── line 3: one hash across N runs ─────────────────────────────────────────
# `basemode` answers to decision, domain, milestone, rule, project and entity in
# this store. The resolver must pick the same one every time, and the block must
# come out byte-identical, or the "surgical inputs" claim is a coin toss.
echo "── line 3: the ambiguous name resolves identically across $RUNS runs ──"
: > "$R/hashes"
for i in $(seq 1 "$RUNS"); do
  walk_block "$B" 'where are we on `basemode`' "L3-$i" | sha256sum | cut -d' ' -f1 >> "$R/hashes"
done
distinct=$(sort -u "$R/hashes" | wc -l)
h=$(head -1 "$R/hashes")
if [ "$distinct" -eq 1 ]; then
  ok "$RUNS runs, one sha256: $h"
  head -1 "$R/line1.new" | sed 's/^/        chose: /'
else
  bad "$RUNS runs produced $distinct distinct hashes — the walk is not deterministic"
  sort "$R/hashes" | uniq -c | sed 's/^/        /'
fi
echo

if [ -f "$FROZEN/MD5SUMS" ]; then
  ( cd "$FROZEN" && md5sum -c MD5SUMS --quiet ) \
    && ok "frozen copy unchanged — nothing was written outside $R" \
    || bad "FROZEN COPY MUTATED by this run"
fi

echo "artefacts in $R"
if [ "$fail" -eq 0 ]; then
  [ "$skipped" -eq 0 ] && { echo "PASS — acceptance lines 1-3 hold."; exit 0; }
  echo "PASS with $skipped SKIP(s) — the legs that ran passed; the skipped ones proved nothing."
  exit 0
fi
echo "FAIL — $fail problem(s), $skipped skipped."
exit 1
