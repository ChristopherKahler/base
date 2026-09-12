#!/usr/bin/env bash
# b3_hermetic.sh — acceptance leg B3, on a FROZEN input. This is the gate.
# Becomes tests/prove_context_walk.sh (granted by auk 2026-09-10).
#
# THE CLAIM: `base context <text>` and the prompt hook, same binary, store, budget and
# text, emit the SAME SET of walk record lines. That is the help string's "same engine as
# hook injection" promise turned into something that can fail.
#
# WHY FROZEN AND NOT LIVE. The first version ran on the operator's live store and refused
# once, reading 4 records from the command and 0 from the hook. It did not reproduce: three
# consecutive re-runs read 4 v 4, and control, gapA and gapB binaries all read 4 on that
# text. Four sessions write that graph during a round. A two-call comparison over an input
# that moves between the two calls cannot be a bar, however good its controls are. The live
# run stays useful as a REPORTED FIGURE; this one is the gate.
#
# CONTROLS:
#   C1 neither side empty, both counts printed every run. Two empty sets are not a pass.
#   C2 both arms must have matched the SAME domains. The two paths use different matchers
#      (match_domains_auto with session paths vs match_domains with an empty list), so a
#      difference there means different served sets and the diff cannot judge the walk.
#      Reported as NOT COMPARABLE, never as a code defect.
#   C3 must-fail canary: perturb one line, the comparator must go red.
#   C4 zero parse errors, so no invocation is silently skipped.
set -uo pipefail
B="${BASE_BIN:?BASE_BIN required — a binary copied ASIDE from the target dir}"
[ -x "$B" ] || { echo "BASE_BIN not executable: $B"; exit 2; }
case "$B" in */target/*) echo "BASE_BIN is inside a cargo target dir; copy it aside."; exit 2;; esac
fail=0

echo "== provenance =="
echo "  binary  = $B"
echo "  md5     = $(md5sum "$B" | cut -d' ' -f1)"
echo "  version = $("$B" --version 2>&1 | head -1)"
printf '  symbol %-16s count=%s\n' walk_from_text "$(nm -C "$B" 2>/dev/null | grep -c walk_from_text)"

NS="http://ops-sys.local/ontology#"; G="${NS}graph/global"
ROOT=$(mktemp -d /tmp/b3h-XXXXXX)
mkdir -p "$ROOT/.base-gbl/.base" "$ROOT/nowhere"
# The domain node carries hasDecision, so the DOMAIN BLOCK serves records of its own. That
# is what makes the served-set half of this leg meaningful: without it the walk would have
# nothing to dedup against and the comparison would be trivially equal.
cat > "$ROOT/.base-gbl/.base/graph.nq" <<NQ
<${NS}domain/global> <${NS}name> "GLOBAL" <$G> .
<${NS}domain/global> <${NS}hasDecision> <${NS}decision/served-one> <$G> .
<${NS}decision/served-one> <${NS}name> "a decision the domain block serves" <$G> .
<${NS}project/aurora-borealis> <${NS}name> "Aurora Borealis" <$G> .
<${NS}project/aurora-borealis> <${NS}hasDomain> <${NS}domain/global> <$G> .
<${NS}decision/pick-postgres> <${NS}name> "pick postgres over mysql" <$G> .
<${NS}decision/pick-postgres> <${NS}belongsTo> <${NS}project/aurora-borealis> <$G> .
<${NS}decision/pick-postgres> <${NS}updatedAt> "2026-09-01T00:00:00Z" <$G> .
<${NS}decision/ship-friday> <${NS}name> "ship on friday" <$G> .
<${NS}decision/ship-friday> <${NS}belongsTo> <${NS}project/aurora-borealis> <$G> .
<${NS}decision/ship-friday> <${NS}updatedAt> "2026-09-02T00:00:00Z" <$G> .
NQ
printf '[[domain]]\nname = "GLOBAL"\nmode = "always"\nprompt_keywords = []\nrules = ["a standing rule"]\n' \
  > "$ROOT/.base-gbl/domains.toml"
# Relay off: with it on the hook prints a wake-contract block naming a RANDOM codename, so
# two runs of one binary differ and nothing here is reproducible.
printf '[relay]\nenabled = false\n' > "$ROOT/.base-gbl/base.toml"
echo "  frozen store = $ROOT ($(wc -l < "$ROOT/.base-gbl/.base/graph.nq") quads)"

pairs() {
  awk '
    /^<base-context / { n=$0; sub(/^<base-context name="/,"",n); sub(/".*$/,"",n); blk=n; next }
    /^<\/base-context>/ { blk=""; next }
    blk!="" && / — / { lab=$0; sub(/^  [a-z]+  */,"",lab); sub(/ — .*$/,"",lab);
                       gsub(/^[[:space:]]+|[[:space:]]+$/,"",lab); print blk "\t" lab }
  ' | sort -u
}
domains_of() { grep -o '^\[DOMAIN: [^]]*\]\|^\[[A-Za-z ]* CONTEXT\]' | sort -u; }

leg() {
  local text="$1" tag="$2"
  echo
  echo "──────── B3: $text ────────"
  rm -f "$ROOT/.base-gbl/.base/.session"
  (cd "$ROOT/nowhere" && BASE_HOME="$ROOT" BASE_NO_AUTO_UPDATE=1 "$B" context "$text") > "$ROOT/$tag.ctx" 2>&1
  rm -f "$ROOT/.base-gbl/.base/.session"
  local s="b3h-$tag-$$" i
  : > "$ROOT/$tag.hook"
  for i in 1 2; do
    python3 -c 'import json,sys; print(json.dumps({"session_id":sys.argv[1],"prompt":"hello"}))' "$s" \
      | (cd "$ROOT/nowhere" && BASE_HOME="$ROOT" BASE_NO_AUTO_UPDATE=1 "$B" hook user-prompt-submit) >/dev/null 2>&1
  done
  python3 -c 'import json,sys; print(json.dumps({"session_id":sys.argv[1],"prompt":sys.argv[2]}))' "$s" "$text" \
    | (cd "$ROOT/nowhere" && BASE_HOME="$ROOT" BASE_NO_AUTO_UPDATE=1 "$B" hook user-prompt-submit) > "$ROOT/$tag.hook" 2>&1

  pairs < "$ROOT/$tag.ctx"  > "$ROOT/$tag.ctx.p"
  pairs < "$ROOT/$tag.hook" > "$ROOT/$tag.hook.p"
  local nc nh
  nc=$(wc -l < "$ROOT/$tag.ctx.p"); nh=$(wc -l < "$ROOT/$tag.hook.p")
  echo "  context walk records = $nc"
  echo "  hook    walk records = $nh"

  # C4
  if grep -qE 'expected `,`|expected `\}`|hook user-prompt-submit:' "$ROOT/$tag.hook"; then
    echo "  REFUSE (C4): an invocation failed to parse its event and never ran."; fail=$((fail+1)); return
  fi
  # C1
  if [ "$nc" -eq 0 ] || [ "$nh" -eq 0 ]; then
    echo "  REFUSE (C1): a side is empty — $nc vs $nh. A diff of two empty sets is not a pass."
    fail=$((fail+1)); return
  fi
  # C2
  domains_of < "$ROOT/$tag.ctx" > "$ROOT/$tag.ctx.d"; domains_of < "$ROOT/$tag.hook" > "$ROOT/$tag.hook.d"
  if ! diff -q "$ROOT/$tag.ctx.d" "$ROOT/$tag.hook.d" >/dev/null; then
    echo "  NOT COMPARABLE (C2): the arms matched different domains, so their served sets"
    echo "  differ and this diff cannot judge the walk."
    diff "$ROOT/$tag.ctx.d" "$ROOT/$tag.hook.d" | sed 's/^/    /'; fail=$((fail+1)); return
  fi
  echo "  C2 ok: both arms matched the same domains"
  if diff -q "$ROOT/$tag.ctx.p" "$ROOT/$tag.hook.p" >/dev/null; then
    echo "  PASS: identical sets of (block, record) pairs."
  else
    echo "  FAIL: the two paths disagree —"; diff "$ROOT/$tag.ctx.p" "$ROOT/$tag.hook.p" | sed 's/^/    /' | head -12
    fail=$((fail+1))
  fi
  # C3
  sed '1s/$/ CANARY/' "$ROOT/$tag.ctx.p" > "$ROOT/$tag.can"
  if diff -q "$ROOT/$tag.can" "$ROOT/$tag.hook.p" >/dev/null; then
    echo "  CANARY FAILED (C3): a perturbed set read as equal. This leg cannot fail."; fail=$((fail+1))
  else
    echo "  C3 ok: a one-line change made the comparator go red."
  fi
}

leg 'where are we on `Aurora Borealis`' backticked
leg 'aurora-borealis' slug
leg 'status of aurora-borealis today' insentence

echo
echo "legs=3 problems=$fail  frozen store kept at $ROOT"
exit "$fail"
