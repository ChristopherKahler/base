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
# lean_mode is `bracket == Fresh && prompt_num <= 2`, so prompts 1 AND 2 are
# lean and the walk does not run in either. Two fires were not "past lean mode",
# they were lean mode; this row and two below failed for that one reason.
out=$(fire "$H" 'where are we on `aurora`' s1)
out=$(fire "$H" 'where are we on `aurora`' s1)
out=$(fire "$H" 'where are we on `aurora`' s1)  # prompt 3: the first non-lean one
grep -q "pick postgres over mysql" <<<"$out" \
  && ok "a decision in the global tier reached the prompt" \
  || { bad "global-tier record never arrived"; sed -n '1,15p' <<<"$out"; }
echo

echo "── row: lean mode skips the walk ──"
H2="$OUT/lean"; seed_home "$H2"
first=$(fire "$H2" 'where are we on `aurora`' s-lean)
grep -q "<base-context" <<<"$first" \
  && bad "the walk ran on prompt 1, which is lean mode" \
  || ok "prompt 1 (FRESH, lean) has no base-context block"
echo

echo "── row: the devmode line names what was resolved ──"
H3="$OUT/dev"; seed_home "$H3"
printf '[devmode]\nenabled = true\n' > "$H3/.base-gbl/base.toml"
_=$(fire "$H3" 'where are we on `aurora`' s-dev)
_=$(fire "$H3" 'where are we on `aurora`' s-dev)
d=$(fire "$H3" 'where are we on `aurora`' s-dev)  # prompt 3
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
_=$(fire "$H4" 'where are we on `aurora`' s-bud)
_=$(fire "$H4" 'where are we on `aurora`' s-bud)
squeezed=$(fire "$H4" 'where are we on `aurora`' s-bud)  # prompt 3
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
_=$(fire "$H5" 'aurora-borealis' s-slug)
_=$(fire "$H5" 'aurora-borealis' s-slug)
slug=$(fire "$H5" 'aurora-borealis' s-slug)   # prompt 3: the first non-lean one
grep -q "ship on friday" <<<"$slug" \
  && ok "a bare lowercase slug resolved" \
  || { bad "a bare lowercase slug did not resolve"; sed -n '1,12p' <<<"$slug"; }

# The control that keeps the exemption honest, and it needs a node named `base` in the
# graph or it reads 0 for the WRONG REASON: known() would miss, the single-word rule would
# never be the thing under test, and no regression of that rule could redden this row.
_=$(fire "$H5" 'base mid-sentence here' s-word)
_=$(fire "$H5" 'base mid-sentence here' s-word)
word=$(fire "$H5" 'base mid-sentence here' s-word)
grep -q "keep it small" <<<"$word" \
  && bad "a bare lowercase WORD resolved; the single-word rule is gone" \
  || ok "a bare lowercase word is still a word"

# And no fabrication: a slug-shaped English word the graph does not have stays a word.
_=$(fire "$H5" 'a well-known thing entirely' s-fab)
_=$(fire "$H5" 'a well-known thing entirely' s-fab)
fab=$(fire "$H5" 'a well-known thing entirely' s-fab)
grep -q "<base-context" <<<"$fab" \
  && bad "a slug-shaped word with no record behind it fabricated a block" \
  || ok "no fabrication from a slug-shaped English word"
echo

[ "$fail" -eq 0 ] && { echo "PASS — the hook rows, the budget-suppression leg and the slug rows."; exit 0; }
echo "FAIL — $fail problem(s)."; exit 1
