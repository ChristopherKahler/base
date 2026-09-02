#!/usr/bin/env bash
# End-to-end: the auto-map guarantee, driven through the REAL hook binary in a
# scratch home. Usage: e2e-automap.sh <base.exe> <repo-root> <scratch-root>
# Every case prints PASS/FAIL; exit code is the number of failures.
set -u
BIN="$1"; REPO="$2"; ROOT="$3"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; fails=$((fails+1)); }

rm -rf "$ROOT"; mkdir -p "$ROOT/.base" "$ROOT/home/.base-gbl/scripts/ast" "$ROOT/home/.claude" "$ROOT/home/dev"
H="$ROOT/home"
cp "$REPO"/scripts/ast/*.py "$H/.base-gbl/scripts/ast/"
printf '[update]\nauto = false\n\n[relay]\nenabled = false\n' > "$H/.base-gbl/base.toml"
# Windows (Git Bash) hands the binary C:/-style paths; Linux hands them as-is.
if command -v cygpath >/dev/null 2>&1 && [ "$(uname -o 2>/dev/null)" = "Msys" ]; then
  winp() { cygpath -m "$1"; }
  PYLESS_PATH="$(cygpath -u "$(dirname "$BIN")"):/usr/bin:/bin"
else
  winp() { printf '%s' "$1"; }
  PYLESS_PATH="$(dirname "$BIN"):/nonexistent"
fi
export BASE_HOME="$(winp "$H")"

# hook <event> <cwd> [session] [extra-json]
hook() {
  local ev="$1" cwd="$2" sid="${3:-e2e-$RANDOM}" extra="${4:-}"
  printf '{"cwd":"%s","session_id":"%s"%s}' "$(winp "$cwd")" "$sid" "$extra" | "$BIN" hook "$ev" 2>>"$ROOT/hook.stderr"
}
wait_map() { # dir [seconds]
  local d="$1" n="${2:-90}"
  for _ in $(seq 1 "$n"); do [ -s "$d/.base-ast/ast.ttl" ] && return 0; sleep 1; done
  return 1
}
mkapp() { # dir -> git repo with one function
  mkdir -p "$1/src"; printf 'def alpha_%s():\n    return 1\n' "$(basename "$1")" > "$1/src/main.py"
  git -C "$1" init -q
}

echo "== binary: $("$BIN" --version)"

# A. a repo gets its first map at session start; .gitignore is honoured
A="$H/dev/repo"; mkapp "$A"
mkdir -p "$A/generated"; printf 'def gen_only():\n    return 2\n' > "$A/generated/gen.py"; printf 'generated/\n' > "$A/.gitignore"
out=$(hook session-start "$A")
echo "$out" | grep -q "\[AST\] no code map for" && pass "A1 session-start announces the first build" || fail "A1 no announcement: $out"
echo "$out" | grep -q "\[hooks\] wired base hook SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop" && pass "A0 fresh ~/.claude got a settings.json with every hook" || fail "A0 fresh install not wired: $out"
[ "$(grep -c "base hook" "$H/.claude/settings.json" 2>/dev/null)" = "5" ] && pass "A0b settings.json carries all five" || fail "A0b settings.json wrong"
wait_map "$A" && pass "A2 map landed for a repo" || fail "A2 no map for repo"
grep -q "alpha_repo" "$A/.base-ast/ast.ttl" && pass "A3 map has the function" || fail "A3 function missing"
grep -q "gen_only" "$A/.base-ast/ast.ttl" && fail "A4 .gitignore NOT honoured (gen_only in map)" || pass "A4 .gitignore honoured (generated/ excluded)"
[ -f "$A/.base-ast/.gitignore" ] && pass "A5 map self-ignores in git" || fail "A5 no .base-ast/.gitignore"
grep -q -- "--yes" "$A/.git/hooks/post-merge" 2>/dev/null && pass "A6 git hooks regenerate unattended (--yes)" || fail "A6 post-merge hook missing --yes"
out2=$(hook session-start "$A")
echo "$out2" | grep -q "\[AST\]" && fail "A7 second start announced again: $out2" || pass "A7 second start is silent"

# B. a bare folder (no .git) with code is adopted
B="$H/dev/bare"; mkdir -p "$B"; printf 'def bare_fn():\n    return 3\n' > "$B/app.py"
out=$(hook session-start "$B")
echo "$out" | grep -q "has no .git yet" && pass "B1 bare folder announced as adopted" || fail "B1 not adopted: $out"
wait_map "$B" && pass "B2 map landed for bare folder" || fail "B2 no map for bare folder"
[ -d "$B/.base-ast" ] && grep -q "bare_fn" "$B/.base-ast/ast.ttl" && pass "B3 bare map has the function" || fail "B3 bare map wrong"

# C. a session at home that only READS a file in an unmapped app maps it
C="$H/dev/readonly"; mkapp "$C"
out=$(hook session-start "$H" e2e-home)
echo "$out" | grep -q "\[AST\]" && fail "C1 home start mapped something: $out" || pass "C1 home start maps nothing"
[ -e "$H/.base-ast" ] && fail "C2 home got a .base-ast" || pass "C2 home untouched"
hook pre-tool-use "$H" e2e-home ",\"tool_name\":\"Read\",\"tool_input\":{\"file_path\":\"$(winp "$C/src/main.py")\"}" >/dev/null
wait_map "$C" && pass "C3 read-only first contact built the map" || fail "C3 no map after Read"

# D. a missing hook is wired at session start, once per version
cat > "$H/.claude/settings.json" <<'JSON'
{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"base hook session-start"}]}],
"UserPromptSubmit":[{"hooks":[{"type":"command","command":"base hook user-prompt-submit"}]}],
"PreToolUse":[{"hooks":[{"type":"command","command":"base hook pre-tool-use"}]}],
"PostToolUse":[{"hooks":[{"type":"command","command":"base hook post-tool-use"}]}]}}
JSON
rm -f "$H"/.base-gbl/.hooks-wired-*
out=$(hook session-start "$A" e2e-d1)
echo "$out" | grep -q "\[hooks\] wired base hook Stop" && pass "D1 Stop hook wired at session start" || fail "D1 not wired: $out"
grep -q "base hook stop" "$H/.claude/settings.json" && pass "D2 settings.json carries the Stop hook" || fail "D2 settings.json lacks it"
[ "$(grep -c "base hook session-start" "$H/.claude/settings.json")" = "1" ] && pass "D3 existing hooks not duplicated" || fail "D3 duplicated"
out=$(hook session-start "$A" e2e-d2)
echo "$out" | grep -q "\[hooks\]" && fail "D4 wired again: $out" || pass "D4 once per version"

# E. a 2,100-file app builds unattended (past the extractor's threshold)
E="$H/dev/big"; mkdir -p "$E/src"; git -C "$E" init -q 2>/dev/null || { mkdir -p "$E"; git -C "$E" init -q; }
for i in $(seq 1 2100); do printf 'def big_%d():\n    return %d\n' "$i" "$i" > "$E/src/f_$i.py"; done
out=$(hook session-start "$E")
wait_map "$E" 300 && pass "E1 2,100-file app mapped unattended" || fail "E1 big app did not map (threshold abort?)"
n=$(grep -c "big_2100\|big_1(" "$E/.base-ast/ast.ttl" 2>/dev/null); [ "${n:-0}" -ge 1 ] && pass "E2 big map holds the last function" || fail "E2 big map incomplete"

# F. a failed build says why at the next session start, then heals
F="$H/dev/broken"; mkapp "$F"
PATH_SAVE="$PATH"
PATH="$PYLESS_PATH" "$BIN" sync --ast --yes --target "$(winp "$F")" >/dev/null 2>&1
[ -f "$F/.base-ast/.last-error" ] && pass "F1 failed build recorded .last-error" || fail "F1 no .last-error after python-less build"
out=$(hook session-start "$F")
echo "$out" | grep -q "has been failing" && pass "F2 session start explains the failure" || fail "F2 no explanation: $out"
wait_map "$F" && pass "F3 retry landed the map" || fail "F3 retry did not land"
[ -f "$F/.base-ast/.last-error" ] && fail "F4 .last-error not cleared" || pass "F4 .last-error cleared on success"
out=$(hook session-start "$F"); echo "$out" | grep -q "\[AST\]" && fail "F5 still complaining: $out" || pass "F5 silent once healed"

# G. a workspace of apps, and the folder of apps, are never mapped
G="$H/dev/hub"; mkdir -p "$G/.base" "$G/repo1"; git -C "$G/repo1" init -q; printf 'def loose():\n    pass\n' > "$G/loose.py"
out=$(hook session-start "$G"); echo "$out" | grep -q "\[AST\]" && fail "G1 hub announced: $out" || pass "G1 hub start is silent"
hook stop "$G" >/dev/null; sleep 3
[ -e "$G/.base-ast" ] && fail "G2 hub got a .base-ast" || pass "G2 hub never mapped"
out=$(hook session-start "$H/dev"); sleep 3
[ -e "$H/dev/.base-ast" ] && fail "G3 dev/ (folder of apps) mapped" || pass "G3 folder of apps never mapped"

# H. an empty new folder: nothing at start; adopted by Stop once code exists
Hn="$H/dev/new"; mkdir -p "$Hn"
out=$(hook session-start "$Hn" e2e-h)
[ -e "$Hn/.base-ast" ] && fail "H1 empty folder mapped" || pass "H1 empty folder: nothing yet"
printf 'def newborn():\n    pass\n' > "$Hn/main.py"
hook stop "$Hn" e2e-h >/dev/null
wait_map "$Hn" && pass "H2 Stop adopted the folder once code existed" || fail "H2 Stop did not adopt"

echo "== failures: $fails"
exit "$fails"
