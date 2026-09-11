#!/usr/bin/env bash
# The three design-to-code rows that need a real hook rather than a unit test:
# lean-mode skip, the devmode line, and the global tier.
#
# Each drives `base hook user-prompt-submit` on a fake home with a seeded graph,
# because what is being checked is what the HOOK does with the walk, not what
# the walk returns.
set -uo pipefail
B=${BASE_BIN:-}
if [ -z "$B" ]; then
  cat >&2 <<'EOF'
BASE_BIN is required. Point it at a binary you copied aside, not at a path
inside CARGO_TARGET_DIR: `cargo test` rewrites that tree underneath a running
harness, and a debug build measures the optimiser rather than the change.

  cargo build --release --bin base
  cp "$CARGO_TARGET_DIR/release/base" /tmp/base-branch
  BASE_BIN=/tmp/base-branch bash tests/prove_walk_hook.sh

EOF
  exit 2
fi
[ -x "$B" ] || { echo "BASE_BIN=$B is not executable" >&2; exit 2; }
case "$B" in
  */target/*)
    echo "BASE_BIN=$B is inside a cargo target dir. cargo test rewrites it mid-run; copy it aside." >&2
    exit 2 ;;
esac

# The mirror of the baseline check, and its absence is I7. `cargo build` in a
# target dir another worktree warmed can report "Finished in 0.28s" and leave
# someone else's binary in place: cargo freshness is mtime-based, and outputs
# newer than sources read as fresh. That binary then walks through every leg
# below, because nothing here asks whether it is the thing under test.
#
# BASE_BIN must HAVE exactly what the baseline must LACK.
branch_binary_has_fork() {
  local bin="$1" a b
  a=$(nm -C "$bin" 2>/dev/null | grep -c maps_from_store)
  b=$(nm -C "$bin" 2>/dev/null | grep -c resolve_strict)
  printf '  binary identity:   maps_from_store=%s resolve_strict=%s (both want >0)\n' "$a" "$b"
  [ "$a" -gt 0 ] && [ "$b" -gt 0 ]
}
if ! branch_binary_has_fork "$B"; then
  echo "BASE_BIN=$B does not contain this fork. A stale target dir handed you another build." >&2
  echo "Run: cargo clean -p base --release && cargo build --release --bin base" >&2
  exit 2
fi
if [ -n "${BASELINE:-}" ] && [ -x "${BASELINE:-}" ] \
   && [ "$(md5sum "$B" | cut -d' ' -f1)" = "$(md5sum "$BASELINE" | cut -d' ' -f1)" ]; then
  echo "BASE_BIN and BASELINE are the same file. Nothing below could ever fail." >&2
  exit 2
fi
OUT=/tmp/walkhook; rm -rf "$OUT"; mkdir -p "$OUT"

fail=0
ok()  { printf '  ok    %s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; fail=$((fail + 1)); }

# A home whose GLOBAL tier carries a project and a decision hanging off it.
# Global on purpose: the walk must read the merged store, not the workspace one.
seed_home() {
  local h="$1"
  mkdir -p "$h/.base-gbl/.base" "$h/.claude"
  cat > "$h/.base-gbl/.base/graph.nq" <<'NQ'
<http://ops-sys.local/ontology#project/aurora> <http://ops-sys.local/ontology#name> "aurora" <http://ops-sys.local/ontology#graph/global> .
<http://ops-sys.local/ontology#decision/pick-postgres> <http://ops-sys.local/ontology#name> "pick postgres over mysql" <http://ops-sys.local/ontology#graph/global> .
<http://ops-sys.local/ontology#decision/pick-postgres> <http://ops-sys.local/ontology#belongsTo> <http://ops-sys.local/ontology#project/aurora> <http://ops-sys.local/ontology#graph/global> .
<http://ops-sys.local/ontology#decision/pick-postgres> <http://ops-sys.local/ontology#updatedAt> "2026-09-01T00:00:00Z" <http://ops-sys.local/ontology#graph/global> .
<http://ops-sys.local/ontology#project/aurora-borealis> <http://ops-sys.local/ontology#name> "Aurora Borealis" <http://ops-sys.local/ontology#graph/global> .
<http://ops-sys.local/ontology#decision/ship-friday> <http://ops-sys.local/ontology#name> "ship on friday" <http://ops-sys.local/ontology#graph/global> .
<http://ops-sys.local/ontology#decision/ship-friday> <http://ops-sys.local/ontology#belongsTo> <http://ops-sys.local/ontology#project/aurora-borealis> <http://ops-sys.local/ontology#graph/global> .
<http://ops-sys.local/ontology#project/base> <http://ops-sys.local/ontology#name> "base" <http://ops-sys.local/ontology#graph/global> .
<http://ops-sys.local/ontology#decision/keep-it-small> <http://ops-sys.local/ontology#name> "keep it small" <http://ops-sys.local/ontology#graph/global> .
<http://ops-sys.local/ontology#decision/keep-it-small> <http://ops-sys.local/ontology#belongsTo> <http://ops-sys.local/ontology#project/base> <http://ops-sys.local/ontology#graph/global> .
NQ
  printf '[[domain]]\nname = "GLOBAL"\nmode = "always"\nprompt_keywords = []\n' > "$h/.base-gbl/domains.toml"
}

# Fire the hook with a prompt, at a given prompt count, and print what it emits.
fire() {
  local h="$1" prompt="$2" sid="$3"
  printf '{"session_id":"%s","prompt":"%s"}' "$sid" "$prompt" \
    | BASE_HOME="$h" BASE_NO_AUTO_UPDATE=1 "$B" hook user-prompt-submit 2>&1
}

echo "── row: the walk reads the GLOBAL tier, not the workspace only ──"
H="$OUT/global"; seed_home "$H"
# Run from a directory that is NOT a workspace: load_graph would have errored
# here, which is exactly the divergence maps_from_store was split to avoid.
cd /tmp || exit 1
# ONE FIRE, AT PROMPT 1. This row used to fire three times on one session and
# read the third, because prompts 1 and 2 were lean and the walk did not run in
# either. Now it does, and the warmups became actively harmful: they resolved
# `aurora` at prompt 1 and MARKED it, so the third fire was deduped and this row
# read a blank. Worse, the two absence-control rows below would have gone GREEN
# over a regression, because a rule that started resolving would have been
# marked at prompt 1 and deduped out of the third fire. Every row reads prompt 1.
out=$(fire "$H" 'where are we on `aurora`' s1)
grep -q "pick postgres over mysql" <<<"$out" \
  && ok "a decision in the global tier reached the prompt" \
  || { bad "global-tier record never arrived"; sed -n '1,15p' <<<"$out"; }
echo

echo "── row: prompt 1 of a FRESH session DOES serve the walk ──"
# INVERTED, not deleted. This row used to assert the defect.
#
# `lean_mode` was introduced by fb1cd48 (2026-06-01) for the NEIGHBOURHOOD --
# `git show fb1cd48:src/hook/user_prompt_submit.rs` contains no walk at all,
# because the walk did not exist for another three months. 29ff99a (2026-09-07)
# then hung the walk on that same flag, so the gate this row protected was never
# reasoned about for the walk.
#
# Chris ruled it off: skipping the injection does not save context, it moves the
# cost and makes it bigger, because the session then spends more than the block's
# bytes finding the same thing by hand. Prompt 1 is usually where someone starts
# work, which is where that signal is worth most.
#
# TWO ARMS, and the second is what makes the first mean anything. An assertion
# that a block APPEARS passes just as happily on a build that emits one
# unconditionally. So a prompt naming nothing in the graph is fired at the same
# position and must stay silent. Both arms were measured on the pre-change and
# post-change binaries before this row was written: the control reads 0 blocks
# on both, so it can genuinely see a zero.
H2="$OUT/lean"; seed_home "$H2"
first=$(fire "$H2" 'where are we on `aurora`' s-lean)
grep -q "<base-context" <<<"$first" \
  && ok "prompt 1 (FRESH) serves a base-context block" \
  || { bad "prompt 1 served no walk block — the walk is gated again"; sed -n '1,15p' <<<"$first"; }

H2b="$OUT/lean-control"; seed_home "$H2b"
none=$(fire "$H2b" 'a name the graph does not have' s-lean-none)
grep -q "<base-context" <<<"$none" \
  && { bad "prompt 1 served a block for a prompt naming nothing — this row cannot see a zero"; sed -n '1,15p' <<<"$none"; } \
  || ok "control: prompt 1 naming nothing still serves no block"

# Prompt 2 is the other position the old gate covered. Asserted separately rather
# than assumed from prompt 1: `lean_mode` was `prompt_num <= 2`, so a partial
# revert that restored the gate for only one of the two positions would pass a
# row that tested prompt 1 alone.
H2c="$OUT/lean-p2"; seed_home "$H2c"
_=$(fire "$H2c" 'an opening prompt that names nothing' s-lean-p2)
second=$(fire "$H2c" 'where are we on `aurora`' s-lean-p2)
grep -q "<base-context" <<<"$second" \
  && ok "prompt 2 (FRESH) serves a base-context block" \
  || { bad "prompt 2 served no walk block — the walk is gated again"; sed -n '1,15p' <<<"$second"; }
echo

echo "── row: the devmode line names what was resolved ──"
H3="$OUT/dev"; seed_home "$H3"
printf '[devmode]\nenabled = true\n' > "$H3/.base-gbl/base.toml"
# Prompt 1, for the reason on the GLOBAL-tier row above.
d=$(fire "$H3" 'where are we on `aurora`' s-dev)
grep -q "walk: aurora" <<<"$d" \
  && ok "devmode names the resolved node" \
  || { bad "no devmode walk line"; grep -n "DEVMODE" -A6 <<<"$d" | head -10; }
echo

echo "── the same name does not re-inject on the next prompt ──"
again=$(fire "$H3" 'and `aurora` again' s-dev)
grep -q "already injected this session" <<<"$again" \
  && ok "second mention deduped by session state" \
  || bad "a name re-injected within one session"
echo

echo "── F19: a name the budget squeezed out is NOT suppressed for the session ──"
H4="$OUT/budget"; seed_home "$H4"
# A budget of 1 byte cannot fit any record, so the walk renders nothing at all.
printf '[injection]\nwalk_budget = 1\n' > "$H4/.base-gbl/base.toml"
# Prompt 1. This row was sound either way -- a 1-byte budget renders nothing at
# any position, so no warmup could have marked anything -- but its trailing
# position comment would have been the only surviving claim that lean mode still
# gates the walk, and a stale comment is how the next reader inherits a false
# premise.
squeezed=$(fire "$H4" 'where are we on `aurora`' s-bud)
grep -q "<base-context" <<<"$squeezed" \
  && bad "a 1-byte budget still rendered a block" \
  || ok "the budget rendered nothing, as set up"

# Now give it room. The name must come back: it was never served, so it was
# never injected, so nothing should have marked it as such.
printf '[injection]\nwalk_budget = 4000\n' > "$H4/.base-gbl/base.toml"
roomy=$(fire "$H4" 'where are we on `aurora`' s-bud)
grep -q "pick postgres over mysql" <<<"$roomy" \
  && ok "the name came back once the budget allowed it" \
  || bad "the budget permanently suppressed a name that was never served"
echo

echo "── a bare lowercase SLUG resolves; a bare lowercase WORD still does not ──"
# `aurora-borealis` is what `base domain list` and `base project list` print and what a
# user types back. Before this row existed the single-word rule dropped it before known()
# was consulted, so it never resolved -- while `Aurora-Borealis` did, on nothing but the
# case of the first letter.
H5="$OUT/slug"; seed_home "$H5"
# Prompt 1, for the reason on the GLOBAL-tier row above.
slug=$(fire "$H5" 'aurora-borealis' s-slug)
grep -q "ship on friday" <<<"$slug" \
  && ok "a bare lowercase slug resolved" \
  || { bad "a bare lowercase slug did not resolve"; sed -n '1,12p' <<<"$slug"; }

# The control that keeps the exemption honest, and it needs a node named `base` in the
# graph or it reads 0 for the WRONG REASON: known() would miss, the single-word rule would
# never be the thing under test, and no regression of that rule could redden this row.
# Prompt 1, and here it is load-bearing rather than tidy: warming this row would
# mark `base` at prompt 1 if the single-word rule ever regressed, and the read
# below would then be a deduped blank -- printing ok over the regression.
word=$(fire "$H5" 'base mid-sentence here' s-word)
grep -q "keep it small" <<<"$word" \
  && bad "a bare lowercase WORD resolved; the single-word rule is gone" \
  || ok "a bare lowercase word is still a word"

# And no fabrication: a slug-shaped English word the graph does not have stays a word.
# Prompt 1, load-bearing for the same reason as the row above.
fab=$(fire "$H5" 'a well-known thing entirely' s-fab)
grep -q "<base-context" <<<"$fab" \
  && bad "a slug-shaped word with no record behind it fabricated a block" \
  || ok "no fabrication from a slug-shaped English word"
echo

[ "$fail" -eq 0 ] && { echo "PASS — the hook rows, the budget-suppression leg and the slug rows."; exit 0; }
echo "FAIL — $fail problem(s)."; exit 1
