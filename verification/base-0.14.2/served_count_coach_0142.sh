#!/usr/bin/env bash
# merlin — 0.14.2 acceptance harness for #62 #65 #70 (fork base-served-count-and-coach). No cargo inside.
#
# Drives ONE binary (BASE_BIN) on fake homes under /tmp: BASE_HOME is set, the cwd sits inside the fake root, HOME is
# never touched (I8: BASE_HOME isolates the home tier only, the workspace tier still resolves from cwd). For the
# byte-identity and F29 legs a second binary (BASELINE, the 0.14.1 asset) runs the same commands. Never touches a
# live store: both live graphs are md5'd before and after and a moved md5 fails the run whatever else passed.
#
#   BASE_BIN=<branch binary copied aside> BASELINE=<0.14.1 asset> FIXTURE=~/.cache/auk/f29-real-1303 \
#     bash verification/base-0.14.2/served_count_coach_0142.sh
#
# EXPECT=new (default) asserts the fixed behaviour; EXPECT=old asserts the 0.14.1 behaviour, so the same instrument
# is the red-first side when pointed at the asset (`BASE_BIN=<asset> EXPECT=old`).
# Exit code = number of FAILs (0 = green). Every leg prints its real n; a leg that cannot run says SKIP and why.
set -u
B=${BASE_BIN:?BASE_BIN: the binary under test, copied aside (never a path under a cargo target dir)}
OLD=${BASELINE:-}
FIX=${FIXTURE:-$HOME/.cache/auk/f29-real-1303}
EXPECT=${EXPECT:-new}
R=${R:-/tmp/merlin-0142-accept}
export BASE_NO_AUTO_UPDATE=1
fail=0; skipped=0
ok()   { printf '  ok    %s\n' "$*"; }
bad()  { printf '  FAIL  %s\n' "$*"; fail=$((fail + 1)); }
skip() { printf '  SKIP  %s\n' "$*"; skipped=$((skipped + 1)); }
say()  { echo; echo "── $* ──"; }
case "$B" in */target/*) echo "ABORT: BASE_BIN=$B is under a cargo target dir; copy it aside (cargo test rewrites it with the isolation-guard feature)"; exit 2;; esac
[ -x "$B" ] || { echo "ABORT: BASE_BIN=$B is not executable"; exit 2; }

# ── provenance (law 15 / 17e): profile, md5, both branch discriminators, this harness's own md5 ────────────
ident() { # ident <label> <bin>
  local dbg guard sym str
  dbg=$(readelf -S "$2" 2>/dev/null | grep -c '\.debug_'); guard=$(nm -C "$2" 2>/dev/null | grep -c write_back_seamed)
  sym=$(nm -C "$2" 2>/dev/null | grep -c 'SyncStats::summary'); str=$(strings "$2" | grep -c 'imported from domains.toml')
  printf '%-9s %s  %s  md5 %s  debug-sections=%s guard-symbol=%s  SyncStats::summary=%s  "imported from domains.toml"=%s\n' \
    "$1" "$2" "$("$2" --version 2>&1)" "$(md5sum "$2" | cut -c1-32)" "$dbg" "$guard" "$sym" "$str"
  [ "$dbg" -eq 0 ] || bad "$1 carries debug sections (not a release build)"
  [ "$guard" -eq 0 ] || bad "$1 carries the isolation-guard symbol (cargo test rewrote it; copy the plain build aside)"
  echo "$sym $str"
}
echo "harness $0 md5 $(md5sum "$0" | cut -c1-8)  started $(date -Is)  EXPECT=$EXPECT"
read -r NSYM NSTR < <(ident "under-test" "$B" | tail -n 1)
ident "under-test" "$B" | head -n 1
if [ "$EXPECT" = new ]; then
  { [ "$NSYM" -gt 0 ] && [ "$NSTR" -gt 0 ]; } && ok "provenance: branch discriminators present (symbol $NSYM, string $NSTR)" || bad "provenance: EXPECT=new but the binary lacks the branch discriminators (symbol $NSYM, string $NSTR): wrong head or stale build (law 15)"
else
  { [ "$NSYM" -eq 0 ] && [ "$NSTR" -eq 0 ]; } && ok "provenance: no branch discriminator (a pre-fix binary, as EXPECT=old requires)" || bad "provenance: EXPECT=old but the binary carries the branch discriminators"
fi
if [ -n "$OLD" ]; then
  [ -x "$OLD" ] || { echo "ABORT: BASELINE=$OLD is not executable"; exit 2; }
  read -r OSYM OSTR < <(ident "baseline" "$OLD" | tail -n 1); ident "baseline" "$OLD" | head -n 1
  [ "$(md5sum "$B" | cut -c1-32)" != "$(md5sum "$OLD" | cut -c1-32)" ] && ok "provenance: baseline md5 differs from the binary under test" || bad "provenance: BASELINE and BASE_BIN are the same bytes (law 15)"
  { [ "$OSYM" -eq 0 ] && [ "$OSTR" -eq 0 ]; } && ok "provenance: baseline carries no branch discriminator" || bad "provenance: BASELINE carries a branch discriminator; it is not a 0.14.1 build"
fi
LIVE_WS=$HOME/.base/graph.nq; LIVE_GBL=$HOME/.base-gbl/.base/graph.nq
live_before=$(md5sum "$LIVE_WS" "$LIVE_GBL" 2>/dev/null | cut -c1-32 | tr '\n' ' ')
rm -rf "$R"; mkdir -p "$R"

# ── fake-home builders ───────────────────────────────────────────────────────────────────────────────
# fresh <root> [domains.toml body]: home == workspace root (the injection_scope_test shape).
fresh() {
  local fh=$1; local doms=${2:-}
  rm -rf "$fh"; mkdir -p "$fh/.base-gbl/.base" "$fh/.base" "$fh/genai/vp-operators"
  printf '%s\n' "${doms:-$'[[domain]]\nname = "alpha"\nmode = "triggered"\nprompt_keywords = ["alpha"]\nrules = []'}" > "$fh/.base-gbl/domains.toml"
  printf '[namespace]\nprefix = "ops"\nuri = "http://ops-sys.local/ontology#"\n' > "$fh/.base/base.toml"
  printf '[update]\nauto = false\n[devmode]\nenabled = true\n' > "$fh/.base-gbl/base.toml"
}
run()  { ( cd "$1" && BASE_HOME=$1 "$2" "${@:3}" 2>&1 ); }                       # run <root> <bin> <args...>
fire() { printf '{"session_id":"%s","cwd":"%s","prompt":"%s"}' "$3" "$1" "$4" | ( cd "$1" && BASE_HOME=$1 "$2" hook user-prompt-submit 2>&1 ); }  # fire <root> <bin> <sid> <prompt>
third() { fire "$@" >/dev/null; fire "$@" >/dev/null; fire "$@"; }                 # prompts 1-2 lean, 3 full
touchrow() { printf '{"ts":"2026-09-07T12:00:00-05:00","hook":"pre-tool-use","success":true,"session_id":"%s","tool_name":"Read","file_path":"%s"}\n' "$2" "$3" >> "$1/.base/hook-events.jsonl"; }  # touchrow <root> <sid> <file>
count() { grep -c -F -- "$2" <<<"$1"; }                                              # count <text> <needle>: lines carrying needle

# ── #62 ───────────────────────────────────────────────────────────────────────────────────────────────
say "#62-a  base domain sync says what it counted; base domain get still counts CLI rules (#38)"
FH=$R/62a; fresh "$FH"
for i in 1 2 3 4 5; do run "$FH" "$B" rule add --domain alpha --text "cli rule $i" >/dev/null || bad "rule add $i failed"; done
out=$(run "$FH" "$B" domain sync); echo "    $out"
if [ "$EXPECT" = new ]; then
  [ "$out" = "Domain sync complete: 1 domains, 0 rules imported from domains.toml" ] && ok "sync line names the source" || bad "sync line: $out"
else
  [ "$out" = "Domain sync complete: 1 domains, 0 rules, 0 decisions" ] && ok "0.14.1 sync line (the ambiguous one)" || bad "sync line: $out"
fi
got=$(run "$FH" "$B" domain get alpha | grep -E '^Rules'); echo "    $got"
[ "$got" = "Rules (5):" ] && ok "domain get counts the five CLI rules (#38 in force)" || bad "domain get: $got"

say "#62-b  --carl: carl.json rules count too, and the line says so"
FH=$R/62b; fresh "$FH"
cat > "$FH/carl.json" <<'EOF'
{ "domains": [ { "name": "alpha", "rules": [ {"text": "carl rule one"}, {"text": "carl rule two"} ],
                 "decisions": [ {"decision": "Pick x over y", "rationale": "cheaper"} ] } ] }
EOF
out=$(run "$FH" "$B" domain sync --carl "$FH/carl.json"); echo "    $out"
if [ "$EXPECT" = new ]; then
  [ "$out" = "Domain sync complete: 1 domains, 2 rules imported from domains.toml and carl.json, 1 decisions imported from carl.json" ] && ok "carl line names both sources" || bad "carl line: $out"
else
  [ "$out" = "Domain sync complete: 1 domains, 2 rules, 1 decisions" ] && ok "0.14.1 carl line" || bad "carl line: $out"
fi

say "#62-c  the scaffold and install prints read alike"
FH=$R/62c; fresh "$FH"; mkdir -p "$FH/ws"
out=$(run "$FH" "$B" scaffold "$FH/ws" | grep -E 'Sync domains'); echo "    $out"
if [ "$EXPECT" = new ]; then grep -q 'rules imported from domains.toml)' <<<"$out" && ok "scaffold print" || bad "scaffold print: $out"
else grep -qE 'rules\)$' <<<"$out" && ok "0.14.1 scaffold print" || bad "scaffold print: $out"; fi
FH=$R/62d; fresh "$FH"; mkdir -p "$FH/.claude"
cat > "$FH/.base-gbl/carl.json" <<'EOF'
{ "domains": [ { "name": "alpha", "rules": [ {"text": "carl rule one"} ], "decisions": [ {"decision": "Pick x over y", "rationale": "cheaper"} ] } ] }
EOF
out=$(run "$FH" "$B" install --skip-hooks --no-starter-commands | grep -E 'CARL'); echo "    $out"
if [ "$EXPECT" = new ]; then grep -q 'imported from domains.toml and carl.json, 1 decisions imported from carl.json)' <<<"$out" && ok "install print" || bad "install print: $out"
else grep -qE 'rules, 1 decisions\)' <<<"$out" && ok "0.14.1 install print" || bad "install print: $out"; fi

# ── #65 ───────────────────────────────────────────────────────────────────────────────────────────────
say "#65-a  a decision the domain block listed is not listed again by the walk (prompt names the project)"
FH=$R/65a; fresh "$FH"
run "$FH" "$B" project add --name vp-operators --path "$FH/genai/vp-operators" >/dev/null
run "$FH" "$B" decision log --domain vp-operators --decision "Use Seedance for b-roll" --rationale "cheapest per clip" >/dev/null
touchrow "$FH" s65a "$FH/genai/vp-operators/x.md"
out=$(third "$FH" "$B" s65a "how is \`vp-operators\` going")
echo "$out" | grep -E 'CONTEXT\]|^  - |base-context|^  (decision|project|domain|workspace) |walk:' | sed 's/^/    /'
n=$(count "$out" "Use Seedance for b-roll"); rows=$(grep -cE '^  - Decision: Use Seedance|^  decision  Use Seedance' <<<"$out")
grep -q '^\[vp-operators CONTEXT\]' <<<"$out" || bad "the domain block did not appear on prompt 3 (fixture broke, not the fix)"
if [ "$EXPECT" = new ]; then [ "$rows" -eq 1 ] && ok "decision served once (1 row; $n line(s) carry the name)" || bad "decision rows=$rows (want 1)"
else [ "$rows" -eq 2 ] && ok "0.14.1: decision served twice (block row + walk record)" || bad "decision rows=$rows (0.14.1 wants 2)"; fi

say "#65-b  prompt names the decision itself"
FH=$R/65b; fresh "$FH"
run "$FH" "$B" project add --name vp-operators --path "$FH/genai/vp-operators" >/dev/null
run "$FH" "$B" decision log --domain vp-operators --decision "Use Seedance for b-roll" --rationale "cheapest per clip" >/dev/null
touchrow "$FH" s65b "$FH/genai/vp-operators/x.md"
out=$(third "$FH" "$B" s65b "why did we pick \`Use Seedance for b-roll\`")
echo "$out" | grep -E 'CONTEXT\]|^  - |base-context|^  (decision|project|domain|workspace) |walk:' | sed 's/^/    /'
hdr=$(grep -c 'base-context name="Use Seedance for b-roll"' <<<"$out")
if [ "$EXPECT" = new ]; then [ "$hdr" -eq 0 ] && ok "no second serving under a walk header" || bad "walk header for the served decision present ($hdr)"
else [ "$hdr" -eq 1 ] && ok "0.14.1: the served decision comes back as a walk header" || bad "walk header count=$hdr (0.14.1 wants 1)"; fi

say "#65-c  real-store copy ($FIX): base-config's decisions, prompt 3 names the domain"
if [ ! -f "$FIX/ws/.base/graph.nq" ]; then skip "fixture $FIX absent"; else
  FH=$R/65c; rm -rf "$FH"; mkdir -p "$FH/chris/.base" "$FH/home/.base-gbl/.base"
  cp "$FIX/ws/.base/graph.nq" "$FIX/ws/.base/domains.toml" "$FH/chris/.base/"
  cp "$FIX/gbl/.base-gbl/.base/graph.nq" "$FH/home/.base-gbl/.base/"; cp "$FIX/gbl/.base-gbl/base.toml" "$FIX/gbl/.base-gbl/domains.toml" "$FH/home/.base-gbl/"
  chmod -R u+w "$FH"; printf '\n[update]\nauto = false\n[devmode]\nenabled = true\n' >> "$FH/home/.base-gbl/base.toml"
  echo "    fixture md5: ws graph $(md5sum "$FIX/ws/.base/graph.nq" | cut -c1-8), gbl graph $(md5sum "$FIX/gbl/.base-gbl/.base/graph.nq" | cut -c1-8), ws domains $(md5sum "$FIX/ws/.base/domains.toml" | cut -c1-8), gbl domains $(md5sum "$FIX/gbl/.base-gbl/domains.toml" | cut -c1-8)"
  rfire() { printf '{"session_id":"%s","cwd":"%s","prompt":"%s"}' "$1" "$FH/chris" "$2" | ( cd "$FH/chris" && BASE_HOME=$FH/home "$B" hook user-prompt-submit 2>&1 ); }
  sid=r65-$RANDOM; rfire $sid "hello" >/dev/null; rfire $sid "hello again" >/dev/null; out=$(rfire $sid "how does the base command \`base-config\` work")
  echo "    served list (block rows, then walk records):"; grep -E '^\[base-config CONTEXT\]|^  - Decision|^  decision |base-context name=' <<<"$out" | head -n 40 | cut -c1-140 | sed 's/^/      /'
  dup=0; while read -r name; do [ -z "$name" ] && continue; c=$(grep -cF -- "$name" <<<"$out"); [ "$c" -gt 1 ] && dup=$((dup+1)); done < <(grep -E '^  - Decision: ' <<<"$out" | sed 's/^  - Decision: //' | cut -c1-60)
  blk=$(grep -cE '^  - Decision: ' <<<"$out"); echo "    block decisions=$blk, names served more than once=$dup"
  grep -q '^\[base-config CONTEXT\]' <<<"$out" || bad "base-config block absent on prompt 3 (keyword 'base command' did not fire?)"
  if [ "$EXPECT" = new ]; then [ "$dup" -eq 0 ] && ok "no decision served twice on the real-store copy" || bad "$dup decision(s) served twice"
  else [ "$dup" -gt 0 ] && ok "0.14.1: $dup decision(s) served twice on the real-store copy" || bad "expected duplicates on 0.14.1, saw none (walk budget? see the served list)"; fi
fi

# ── #70: the five coach claims, run on the binary (verified: source+binary) ────────────────────────────
say "#70-1  auto_inject = false keeps a domain out of the three automatic surfaces and inside base context"
FH=$R/70a; fresh "$FH" $'[[domain]]\nname = "secret"\nmode = "always"\nauto_inject = false\nrules = ["Never say a floor out loud"]\n\n[[domain]]\nname = "alpha"\nmode = "triggered"\nprompt_keywords = ["alpha"]\nrules = ["alpha rule"]'
out=$(third "$FH" "$B" s70a "hello")
[ "$(count "$out" "Never say a floor")" -eq 0 ] && ok "prompt hook: flagged domain absent" || bad "prompt hook served the flagged domain"
out=$(printf '{"session_id":"s70a","cwd":"%s","tool_name":"Read","tool_input":{"file_path":"%s/x.md"}}' "$FH" "$FH" | ( cd "$FH" && BASE_HOME=$FH "$B" hook pre-tool-use 2>&1 ))
[ "$(count "$out" "Never say a floor")" -eq 0 ] && ok "tool hook: absent" || bad "tool hook served the flagged domain"
out=$(printf '{"session_id":"s70a2","cwd":"%s"}' "$FH" | ( cd "$FH" && BASE_HOME=$FH "$B" hook session-start 2>&1 ))
[ "$(count "$out" "Never say a floor")" -eq 0 ] && ok "session start: absent" || bad "session start served the flagged domain"
out=$(run "$FH" "$B" context "secret"); [ "$(count "$out" "Never say a floor")" -ge 1 ] && ok "base context still sees it" || bad "base context does not show the flagged domain: $(head -c 200 <<<"$out")"

say "#70-2  a path trigger fires on a file this session touched, not on the store's history"
FH=$R/70b; fresh "$FH" $'[[domain]]\nname = "docs"\nmode = "triggered"\npaths = ["genai"]\nrules = ["the docs rule"]'
run "$FH" "$B" project add --name vp-operators --path "$FH/genai/vp-operators" >/dev/null
touchrow "$FH" touched "$FH/genai/vp-operators/x.md"
out=$(third "$FH" "$B" touched "hello there"); [ "$(count "$out" "the docs rule")" -ge 1 ] && ok "touched session fires docs" || bad "touched session did not fire docs"
out=$(third "$FH" "$B" untouched "hello there"); [ "$(count "$out" "the docs rule")" -eq 0 ] && ok "untouched session does not" || bad "untouched session fired docs"

say "#70-3  a broadcast trigger is inert; doctor names it under 'path triggers'"
FH=$R/70c; fresh "$FH" $'[[domain]]\nname = "wide"\nmode = "triggered"\npaths = ["genai"]\nrules = ["the wide rule"]'
run "$FH" "$B" project add --name p1 --path "$FH/genai/p1" >/dev/null; run "$FH" "$B" project add --name p2 --path "$FH/genai/p2" >/dev/null
touchrow "$FH" bcast "$FH/genai/p1/x.md"
out=$(third "$FH" "$B" bcast "hello there"); [ "$(count "$out" "the wide rule")" -eq 0 ] && ok "broadcast did not fire" || bad "broadcast fired"
[ "$(count "$out" "inert:")" -ge 1 ] && ok "devmode prints an inert: line" || bad "no inert: line in devmode"
doc=$(run "$FH" "$B" doctor); rc=$?
grep -q 'path triggers' <<<"$doc" && grep -q 'covers 2 registered projects' <<<"$doc" && ok "doctor: path triggers section names the broadcast" || bad "doctor section missing: $(grep -c . <<<"$doc") lines"
grep -A3 'path triggers' <<<"$doc" | sed 's/^/    /' | head -n 4

say "#70-4  add-trigger refuses a broadcast (exit 1, file unchanged); project add under it creates no domain"
before=$(md5sum "$FH/.base/domains.toml" 2>/dev/null | cut -c1-8)
out=$(run "$FH" "$B" domain add-trigger --domain wide --path "$FH/genai"); rc=$?
after=$(md5sum "$FH/.base/domains.toml" 2>/dev/null | cut -c1-8); echo "    $out (exit $rc; domains.toml $before -> $after)"
[ "$rc" -ne 0 ] && grep -q 'covers 2 registered projects' <<<"$out" && [ "$before" = "$after" ] && ok "refused, non-zero, nothing written" || bad "add-trigger: rc=$rc md5 $before->$after"
out=$(run "$FH" "$B" project add --name p3 --path "$FH/genai"); echo "    $out" | head -n 3
grep -q 'no domain was created' <<<"$out" && ok "project add says why no domain was created" || bad "project add: $out"
grep -q 'name = "p3"' "$FH/.base/domains.toml" 2>/dev/null && bad "a domain p3 was created" || ok "no domain p3 in domains.toml"

say "#70-5  exclude vetoes an always-on domain"
FH=$R/70e; fresh "$FH" $'[[domain]]\nname = "quiet"\nmode = "always"\nexclude = ["haiku"]\nrules = ["The quiet rule"]'
out=$(third "$FH" "$B" q1 "write a haiku about tea"); [ "$(count "$out" "The quiet rule")" -eq 0 ] && ok "excluded word vetoes the always-on block" || bad "always-on block served despite exclude"
out=$(third "$FH" "$B" q2 "hello"); [ "$(count "$out" "The quiet rule")" -ge 1 ] && ok "and it serves otherwise" || bad "always-on block absent without the excluded word"

# ── byte identity vs the baseline (the ledger's line 5 for this fork) ─────────────────────────────────
say "byte identity vs BASELINE: base context, base recall, the prompt hook on the F29 fixture"
if [ -z "$OLD" ]; then skip "BASELINE unset"; elif [ ! -f "$FIX/ws/.base/graph.nq" ]; then skip "fixture absent"; else
  FH=$R/bi; rm -rf "$FH"; mkdir -p "$FH/chris/.base" "$FH/home/.base-gbl/.base"
  cp "$FIX/ws/.base/graph.nq" "$FIX/ws/.base/domains.toml" "$FH/chris/.base/"; cp "$FIX/gbl/.base-gbl/.base/graph.nq" "$FH/home/.base-gbl/.base/"; cp "$FIX/gbl/.base-gbl/base.toml" "$FIX/gbl/.base-gbl/domains.toml" "$FH/home/.base-gbl/"
  chmod -R u+w "$FH"; printf '\n[update]\nauto = false\n[devmode]\nenabled = true\n' >> "$FH/home/.base-gbl/base.toml"
  n=0; diffs=0
  both() { # both <tag> <args...>: stdout+stderr of each binary, byte-diffed
    local tag=$1; shift
    ( cd "$FH/chris" && BASE_HOME=$FH/home "$B"   "$@" ) > "$R/bi.new.$tag" 2>&1
    ( cd "$FH/chris" && BASE_HOME=$FH/home "$OLD" "$@" ) > "$R/bi.old.$tag" 2>&1
    n=$((n+1)); if ! cmp -s "$R/bi.old.$tag" "$R/bi.new.$tag"; then diffs=$((diffs+1)); printf '  FAIL  %s moved:\n' "$tag"; diff -u "$R/bi.old.$tag" "$R/bi.new.$tag" | sed -n '1,12p' | sed 's/^/    /'; fi
  }
  for kw in "base command" "commands.toml" "skyrim" "basemode" "renda" "weather"; do both "context-$(tr ' .' '--' <<<"$kw")" context "$kw"; done
  both "context-list" context --list
  for kw in base seedance caddy skyrim relay; do both "recall-$kw" recall --keyword "$kw"; done
  for d in base-config basemode skyrim-companion; do both "recall-dom-$d" recall --domain "$d"; done
  hookboth() { # hookboth <tag> <prompt>: prompt 3 of a fresh session on each binary
    local tag=$1 p=$2 s1="bi-$RANDOM" s2="bi-$RANDOM"
    for i in 1 2 3; do printf '{"session_id":"%s","cwd":"%s","prompt":"%s"}' "$s1" "$FH/chris" "$p" | ( cd "$FH/chris" && BASE_HOME=$FH/home "$B"   hook user-prompt-submit ) > "$R/bi.new.$tag" 2>&1; done
    for i in 1 2 3; do printf '{"session_id":"%s","cwd":"%s","prompt":"%s"}' "$s2" "$FH/chris" "$p" | ( cd "$FH/chris" && BASE_HOME=$FH/home "$OLD" hook user-prompt-submit ) > "$R/bi.old.$tag" 2>&1; done
    n=$((n+1)); if ! cmp -s "$R/bi.old.$tag" "$R/bi.new.$tag"; then diffs=$((diffs+1)); printf '  FAIL  %s moved:\n' "$tag"; diff -u "$R/bi.old.$tag" "$R/bi.new.$tag" | sed -n '1,12p' | sed 's/^/    /'; fi
  }
  hookboth unrelated-1 "what is the weather like today"; hookboth unrelated-2 "please summarise this pdf for my mother"; hookboth unrelated-3 "write a haiku about tea"
  if [ "$diffs" -eq 0 ]; then ok "$n command(s) byte-identical (stdout+stderr) vs the baseline"; else bad "$diffs of $n command(s) moved vs the baseline"; fi
  # The one surface this fork CHANGES on purpose, printed and attributed, never counted above:
  s1="bi-rel-$RANDOM"; s2="bi-rel-$RANDOM"; p="how does the base command \`base-config\` work"
  for i in 1 2 3; do printf '{"session_id":"%s","cwd":"%s","prompt":"%s"}' "$s1" "$FH/chris" "$p" | ( cd "$FH/chris" && BASE_HOME=$FH/home "$B"   hook user-prompt-submit ) > "$R/bi.new.related" 2>&1; done
  for i in 1 2 3; do printf '{"session_id":"%s","cwd":"%s","prompt":"%s"}' "$s2" "$FH/chris" "$p" | ( cd "$FH/chris" && BASE_HOME=$FH/home "$OLD" hook user-prompt-submit ) > "$R/bi.old.related" 2>&1; done
  echo "    related prompt (#65 surface, attributed): $(diff "$R/bi.old.related" "$R/bi.new.related" | grep -c '^<') line(s) only in 0.14.1, $(diff "$R/bi.old.related" "$R/bi.new.related" | grep -c '^>') only in the branch"
  diff "$R/bi.old.related" "$R/bi.new.related" | grep -E '^[<>]' | head -n 12 | cut -c1-140 | sed 's/^/      /'
fi

# ── F29 regression gate on the fixture (#65 touches the same function) ───────────────────────────────
say "F29 gate on $FIX: three unrelated prompts → only GLOBAL, 0 private-term hits"
if [ ! -f "$FIX/ws/.base/graph.nq" ]; then skip "fixture absent"; else
  FH=$R/f29; rm -rf "$FH"; mkdir -p "$FH/chris/.base" "$FH/home/.base-gbl/.base"
  cp "$FIX/ws/.base/graph.nq" "$FIX/ws/.base/domains.toml" "$FH/chris/.base/"; cp "$FIX/gbl/.base-gbl/.base/graph.nq" "$FH/home/.base-gbl/.base/"; cp "$FIX/gbl/.base-gbl/base.toml" "$FIX/gbl/.base-gbl/domains.toml" "$FH/home/.base-gbl/"
  chmod -R u+w "$FH"; printf '\n[update]\nauto = false\n[devmode]\nenabled = true\n' >> "$FH/home/.base-gbl/base.toml"
  TERMS='vintrix|meet-caddy|tucker|birdsong|12k|equity'
  for p in "what is the weather like today" "please summarise this pdf for my mother" "write a haiku about tea"; do
    sid="f29-$RANDOM"; for i in 1 2; do printf '{"session_id":"%s","cwd":"%s","prompt":"%s"}' "$sid" "$FH/chris" "$p" | ( cd "$FH/chris" && BASE_HOME=$FH/home "$B" hook user-prompt-submit ) >/dev/null 2>&1; done
    out=$(printf '{"session_id":"%s","cwd":"%s","prompt":"%s"}' "$sid" "$FH/chris" "$p" | ( cd "$FH/chris" && BASE_HOME=$FH/home "$B" hook user-prompt-submit 2>/dev/null ))
    doms=$(grep -o '^\[DOMAIN: [^]]*\]' <<<"$out" | tr '\n' ' '); hits=$(grep -ciE "$TERMS" <<<"$out")
    echo "    '$p': domains=[$doms] term-hits=$hits"
    [ "$(grep -o '^\[DOMAIN: [^]]*\]' <<<"$out" | grep -vc 'DOMAIN: GLOBAL')" -eq 0 ] && [ "$hits" -eq 0 ] && ok "only GLOBAL, no private term" || bad "non-GLOBAL block or private term on an unrelated prompt"
  done
fi

# ── tripwire ─────────────────────────────────────────────────────────────────────────────────────────
live_after=$(md5sum "$LIVE_WS" "$LIVE_GBL" 2>/dev/null | cut -c1-32 | tr '\n' ' ')
[ "$live_before" = "$live_after" ] && ok "tripwire: live stores untouched" || bad "TRIPWIRE: a live store moved (before=$live_before after=$live_after)"
echo; echo "RESULT: $fail FAIL, $skipped SKIP  ($(date -Is))"
exit "$fail"
