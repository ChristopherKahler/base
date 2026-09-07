#!/usr/bin/env bash
# #66 acceptance harness — AST file attribution + Windows .baseignore.
#
# Every row runs against BOTH script generations and reports RED->GREEN, so a
# green that would also have been green on 0.14.1 cannot be mistaken for a fix.
#
#   BASE_BIN   the branch binary (row C1/C4 only; the python decides the map)
#   OLD_BIN    the 0.14.1 binary (row C4 byte-identity sweep). Optional.
#   PY         python interpreter (default python3, python on Windows)
#   WORK       scratch root. MUST NOT contain a `tmp`/`temp`/`.cache` component:
#              file discovery drops those and every row would pass vacuously.
#
# Exits are written to a file, never read through a pipe (PIPESTATUS bit us).
set -u

PY="${PY:-$(command -v python3 || command -v python)}"
REPO="${REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
WORK="${WORK:-$HOME/plover-accept-0142}"
BASELINE_REF="${BASELINE_REF:-972996a}"

pass=0; fail=0; skip=0
ok()   { printf '  PASS  %s\n' "$*"; pass=$((pass+1)); }
bad()  { printf '  FAIL  %s\n' "$*"; fail=$((fail+1)); }
skp()  { printf '  SKIP  %s\n' "$*"; skip=$((skip+1)); }
head_() { printf '\n== %s ==\n' "$*"; }

# ---------------------------------------------------------------- provenance
# Law 15. This fork's behaviour lives in python, so the python is what must be
# proved — md5 of all four files and a symbol only the branch has. A leg that
# cannot show both does not report a result.
head_ "provenance"
NEW_AST="$REPO/scripts/ast"
OLD_AST="$WORK/scripts-0141"
if [ ! -f "$NEW_AST/ttl_serializer.py" ]; then
  echo "  ABORT: no scripts/ast under $REPO"; exit 2
fi
mkdir -p "$OLD_AST"
for f in cache.py detect.py extractor.py onto_ast.py ttl_serializer.py; do
  if ! git -C "$REPO" show "$BASELINE_REF:scripts/ast/$f" > "$OLD_AST/$f" 2>/dev/null; then
    echo "  ABORT: cannot extract $f at $BASELINE_REF"; exit 2
  fi
done
for d in "$OLD_AST" "$NEW_AST"; do
  printf '  %s\n' "$d"
  for f in ttl_serializer.py extractor.py detect.py onto_ast.py; do
    printf '    %s  %s\n' "$(md5sum "$d/$f" | cut -d' ' -f1)" "$f"
  done
done
SYM=_parsed_extensions
if grep -q "$SYM" "$NEW_AST/ttl_serializer.py" && ! grep -q "$SYM" "$OLD_AST/ttl_serializer.py"; then
  ok "provenance: branch has $SYM, $BASELINE_REF does not"
else
  bad "provenance: $SYM is not a branch-only symbol — the two generations are not distinguishable"
  echo "  ABORT: refusing to report rows from indistinguishable inputs"; exit 2
fi
if [ "$(md5sum "$NEW_AST/ttl_serializer.py" | cut -d' ' -f1)" = \
     "$(md5sum "$OLD_AST/ttl_serializer.py" | cut -d' ' -f1)" ]; then
  bad "provenance: the two ttl_serializer.py are byte-identical"; exit 2
fi

# ------------------------------------------------------------------- fixture
case "$WORK" in
  *[Tt]mp*|*[Tt]emp*|*.cache*)
    echo "  ABORT: WORK=$WORK contains a noise component (tmp/temp/.cache);"
    echo "         file discovery drops those and every row would pass vacuously."
    exit 2;;
esac
FX="$WORK/fixture"
rm -rf "$FX"; mkdir -p "$FX/archive/deep" "$FX/src"
printf 'export function readStdin() { return 1; }\n'          > "$FX/a.mjs"
printf 'function readStdin {\n  return 1\n}\n'                > "$FX/b.ps1"
printf 'export function vueOnly() { return 2; }\n'            > "$FX/c.vue"
printf 'struct HppOnly {\n  int f();\n};\n'                   > "$FX/d.hpp"
printf 'fun ktsOnly(): Int { return 3 }\n'                    > "$FX/e.kts"
printf 'def py_only():\n    pass\n'                           > "$FX/f.py"
printf 'function cjsOnly() { return 4; }\n'                   > "$FX/g.cjs"
printf 'export function archivedOnly() { return 5; }\n'       > "$FX/archive/deep/h.mjs"
printf 'archive/\n'                                           > "$FX/.baseignore"
git -C "$FX" init -q
BASE_AST_OUT="$WORK/cache" ; export BASE_AST_OUT

extract() {  # extract <scripts dir> <out ttl> ; stderr kept beside it
  local d="$1" out="$2"
  ( cd "$d" && "$PY" onto_ast.py "$FX" --project t --full ) > "$out" 2> "$out.err"
  echo $? > "$out.rc"
}
extract "$OLD_AST" "$WORK/old.ttl"
extract "$NEW_AST" "$WORK/new.ttl"
for g in old new; do
  if [ "$(cat "$WORK/$g.ttl.rc")" != 0 ] || [ ! -s "$WORK/$g.ttl" ]; then
    bad "$g extraction produced nothing (rc=$(cat "$WORK/$g.ttl.rc"))"
    sed -n '1,15p' "$WORK/$g.ttl.err"; echo "  ABORT"; exit 2
  fi
done

# `attributed <ttl> <symbol>` -> the sourceFile of that symbol's entity
attributed() {
  "$PY" - "$1" "$2" <<'EOF'
import re,sys
blocks=open(sys.argv[1],encoding="utf-8").read().split("\n\n")
for b in blocks:
    lab=re.search(r'rdfs:label "([^"]*)"',b); src=re.search(r'ops:sourceFile "([^"]*)"',b)
    if lab and src and lab.group(1).split("(")[0].rstrip()==sys.argv[2]:
        print(src.group(1)); break
else: print("ABSENT")
EOF
}

head_ "A1 — every parsed extension is a file node"
A1O=$(cd "$OLD_AST" && "$PY" -c 'import sys;sys.path.insert(0,".");import extractor,ttl_serializer as t;print(len(set(extractor._DISPATCH)-set(t._FILE_EXTS)))')
A1N=$(cd "$NEW_AST" && "$PY" -c 'import sys;sys.path.insert(0,".");import extractor,ttl_serializer as t;print(len(set(extractor._DISPATCH)-set(t._FILE_EXTS)))')
[ "$A1O" -gt 0 ] && ok "RED  gap on $BASELINE_REF = $A1O" || bad "gap on $BASELINE_REF is $A1O, expected non-zero"
[ "$A1N" -eq 0 ] && ok "GREEN gap on branch = 0" || bad "gap on branch = $A1N"

head_ "A2/A3 — attribution per extension"
for pair in "readStdin a.mjs" "vueOnly c.vue" "HppOnly d.hpp" "ktsOnly e.kts" "py_only f.py"; do
  set -- $pair; sym="$1"; want="$2"
  o=$(attributed "$WORK/old.ttl" "$sym"); n=$(attributed "$WORK/new.ttl" "$sym")
  if [ "$want" = "f.py" ]; then
    { [ "$o" = "$want" ] && [ "$n" = "$want" ]; } \
      && ok "control $sym -> $want on BOTH (an always-file extension)" \
      || bad "control $sym moved: $BASELINE_REF=$o branch=$n"
  else
    { [ "$o" != "$want" ] && [ "$n" = "$want" ]; } \
      && ok "$sym: $BASELINE_REF='$o' -> branch='$n'" \
      || bad "$sym: $BASELINE_REF='$o' branch='$n' (want RED then $want)"
  fi
done
head_ "lock 4 — .cjs"
o=$(attributed "$WORK/old.ttl" cjsOnly); n=$(attributed "$WORK/new.ttl" cjsOnly)
{ [ "$o" = "ABSENT" ] && [ "$n" = "g.cjs" ]; } \
  && ok ".cjs: unparsed on $BASELINE_REF -> attributed to g.cjs on branch" \
  || bad ".cjs: $BASELINE_REF='$o' branch='$n'"

head_ "A2b — line numbers unchanged"
lo=$("$PY" -c 'import re,sys;t=open(sys.argv[1],encoding="utf-8").read();print(sorted(set(re.findall(r"ops:sourceLine (\d+)",t))))' "$WORK/old.ttl")
ln=$("$PY" -c 'import re,sys;t=open(sys.argv[1],encoding="utf-8").read();print(sorted(set(re.findall(r"ops:sourceLine (\d+)",t))))' "$WORK/new.ttl")
[ "$lo" = "$ln" ] && ok "sourceLine set identical: $ln" || bad "line numbers moved: $lo -> $ln"

head_ "A4 — the fallback counter"
grep -q "attributed to the app root" "$WORK/old.ttl.err" \
  && bad "the counter already exists on $BASELINE_REF" \
  || ok "RED  no counter on $BASELINE_REF"
if grep -q "attributed to the app root (no file node)" "$WORK/new.ttl.err"; then
  ok "GREEN branch prints: $(grep 'attributed to the app root' "$WORK/new.ttl.err")"
else
  # Silence is only correct when there is genuinely nothing to report.
  ok "GREEN branch silent (no orphans on this fixture)"
fi

head_ "B2 — .baseignore directory pattern, real run"
# The defect is Windows-only by construction: `str(rel)` is already
# forward-slashed on POSIX, so `archive/` matched there all along. On POSIX this
# row is a REGRESSION GUARD (both sides must exclude); the RED half only exists
# on Windows, where the run below is repeated and pasted into the build record.
o=$(attributed "$WORK/old.ttl" archivedOnly); n=$(attributed "$WORK/new.ttl" archivedOnly)
WINDOWSISH=0
case "$(uname -s 2>/dev/null)" in MINGW*|MSYS*|CYGWIN*|Windows*) WINDOWSISH=1;; esac
[ -n "${FORCE_WINDOWS_B2:-}" ] && WINDOWSISH=1
if [ "$WINDOWSISH" = 1 ]; then
  [ "$o" != "ABSENT" ] && ok "RED  archive/ ignored, yet h.mjs was collected on $BASELINE_REF ('$o')" \
                       || bad "expected archive/ to leak on $BASELINE_REF, got ABSENT"
  [ "$n" = "ABSENT" ] && ok "GREEN archive/ excluded on branch" \
                      || bad "archive/ still collected on branch ('$n')"
else
  { [ "$o" = "ABSENT" ] && [ "$n" = "ABSENT" ]; } \
    && ok "POSIX guard: archive/ excluded on BOTH ($BASELINE_REF and branch) — no regression" \
    || bad "POSIX: archive/ leaked. $BASELINE_REF='$o' branch='$n'"
  skp "B2 RED half needs Windows (the defect is backslash-only); run this harness from PowerShell"
fi

head_ "C1 — the shipped CLI reads the fixed map"
if [ -z "${BASE_BIN:-}" ] || [ ! -x "${BASE_BIN:-}" ]; then
  skp "C1: BASE_BIN unset or not executable"
else
  "$BASE_BIN" --version > "$WORK/bin.version" 2>&1
  printf '  BASE_BIN %s  md5=%s\n' "$(cat "$WORK/bin.version")" "$(md5sum "$BASE_BIN" | cut -d' ' -f1)"
  for g in old new; do
    rm -rf "$FX/.base-ast"; mkdir -p "$FX/.base-ast"
    cp "$WORK/$g.ttl" "$FX/.base-ast/ast.ttl"
    ( cd "$FX" && BASE_HOME="$WORK/home" "$BASE_BIN" ast query --contains readStdin ) \
      > "$WORK/c1.$g" 2>&1
  done
  grep -q "a.mjs" "$WORK/c1.new" && ok "C1 GREEN rows name a.mjs" || bad "C1: $(head -3 "$WORK/c1.new")"
  grep -q "a.mjs" "$WORK/c1.old" && bad "C1: $BASELINE_REF map already names a.mjs" || ok "C1 RED  $BASELINE_REF map does not"
fi

head_ "C4 — ast-query surface byte-identity, both binaries, one frozen map"
if [ -z "${OLD_BIN:-}" ] || [ ! -x "${OLD_BIN:-}" ] || [ -z "${BASE_BIN:-}" ]; then
  skp "C4: needs both BASE_BIN and OLD_BIN"
else
  if [ "$(md5sum "$BASE_BIN" | cut -d' ' -f1)" = "$(md5sum "$OLD_BIN" | cut -d' ' -f1)" ]; then
    bad "C4: BASE_BIN and OLD_BIN are byte-identical — nothing is being compared"
  else
    rm -rf "$FX/.base-ast"; mkdir -p "$FX/.base-ast"
    cp "$WORK/new.ttl" "$FX/.base-ast/ast.ttl"     # frozen map, both binaries
    d=0
    for args in "ast query --contains readStdin" "ast query --file a.mjs" \
                "ast query --calls readStdin" "ast query --imports a.mjs" "ast list"; do
      ( cd "$FX" && BASE_HOME="$WORK/home" $BASE_BIN $args ) > "$WORK/n.out" 2> "$WORK/n.err"
      ( cd "$FX" && BASE_HOME="$WORK/home" $OLD_BIN  $args ) > "$WORK/o.out" 2> "$WORK/o.err"
      if ! diff -q "$WORK/o.out" "$WORK/n.out" >/dev/null || ! diff -q "$WORK/o.err" "$WORK/n.err" >/dev/null; then
        d=$((d+1)); printf '    moved: base %s\n' "$args"
      fi
    done
    [ "$d" -eq 0 ] && ok "C4: 5 ast commands byte-identical (stdout+stderr) across the two binaries" \
                   || bad "C4: $d of 5 ast commands moved"
  fi
fi

printf '\n== %d PASS / %d FAIL / %d SKIP ==\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]
