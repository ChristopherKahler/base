#!/usr/bin/env bash
# base 0.14.2 — acceptance harness for #83, #84, #89.
#
#   verification/base-0.14.2/ast_followups_0142.sh <branch-worktree> [base-sha]
#
# The rows that need the 28 tree-sitter grammars, which `cargo test` cannot have
# until #85 (deferred). The grammar-free halves ride `cargo test` in
# tests/ast_extension_registry.rs and tests/ast_notices_test.rs.
#
# Law 24 / auk's A3: every orphan number is produced by an instrument that
# ABORTS unless the count it derives equals the number the extractor itself
# printed, and the abort prints both counts. Three parsers disagreed with the
# code before one agreed with it, and the disagreement turned out to be a real
# defect rather than a parser bug -- so a shape number from an unreconciled
# parser is not evidence.
#
# Law 17(e): sourced from its canonical verification dir, md5 printed beside the
# scripts it drives. A row it cannot run is a SKIP with its reason.

set -uo pipefail

WT="${1:-}"
BASE_SHA="${2:-de301d5}"
[ -n "$WT" ] && [ -e "$WT/.git" ] || { echo "usage: $0 <branch-worktree> [base-sha]" >&2; exit 2; }
WT="$(cd "$WT" && pwd)"

# NOT under ~/.cache: the extractor treats it as a noise directory and walks
# zero files, so every fixture row here would score an empty map.
WORK="${WORK:-$HOME/plover-accept/followups}"
PY="${PY:-python3}"
rm -rf "$WORK"; mkdir -p "$WORK"

pass=0; failn=0; skipn=0
say()  { printf '%s\n' "$*"; }
head_(){ printf '\n== %s ==\n' "$*"; }
ok()   { printf '  PASS  %s\n' "$*"; pass=$((pass+1)); }
bad()  { printf '  FAIL  %s\n' "$*"; failn=$((failn+1)); }
skip() { printf '  SKIP  %s\n' "$*"; skipn=$((skipn+1)); }
die()  { printf '\nABORT -- %s\n' "$*"; exit 3; }

BRANCH_SHA="$(git -C "$WT" rev-parse --short HEAD)"

# ── R0 provenance ───────────────────────────────────────────────────────────
head_ "R0 provenance"
say "  harness       $(md5sum "$0" | cut -d' ' -f1)  $0"
for f in extractor.py onto_ast.py ttl_serializer.py detect.py; do
  b=$(git -C "$WT" show "$BASE_SHA:scripts/ast/$f" 2>/dev/null | md5sum | cut -d' ' -f1)
  n=$(md5sum "$WT/scripts/ast/$f" | cut -d' ' -f1)
  mark=$([ "$b" = "$n" ] && echo "same" || echo "CHANGED")
  say "  $f  $BASE_SHA=$b  $BRANCH_SHA=$n  $mark"
done
git -C "$WT" show "$BASE_SHA:scripts/ast/extractor.py" > "$WORK/extractor.base.py" || die "no base extractor.py"
cmp -s "$WORK/extractor.base.py" "$WT/scripts/ast/extractor.py" \
  && die "extractor.py is unchanged from $BASE_SHA -- there is nothing to compare"
ok "the branch's extractor differs from $BASE_SHA"

$PY - <<'PY' || { echo "  (grammars missing)"; }
import importlib
missing = [m for m in ("tree_sitter", "tree_sitter_javascript", "tree_sitter_python")
           if not importlib.util.find_spec(m)]
print("  grammars:", "present" if not missing else f"MISSING {missing}")
PY

have_grammars() { $PY -c "import importlib.util,sys; sys.exit(0 if importlib.util.find_spec('tree_sitter_javascript') else 1)" 2>/dev/null; }

# ── #83 ─────────────────────────────────────────────────────────────────────
head_ "#83 LANG_MAP derives from _DISPATCH"
r83="$($PY - "$WT" <<'PY'
import sys, subprocess, json
wt = sys.argv[1]
code = '''
import sys; sys.path.insert(0, ".")
import extractor, onto_ast as o, json
print(json.dumps({
  "dispatch": len(extractor._DISPATCH),
  "langmap": len(o.LANG_MAP),
  "gap": sorted(set(extractor._DISPATCH) - set(o.LANG_MAP)),
  "extra": sorted(set(o.LANG_MAP) - set(extractor._DISPATCH)),
  "unknown": sorted(e for e, l in o.LANG_MAP.items() if l == "unknown"),
  "psm1": o.LANG_MAP.get(".psm1"), "zsh": o.LANG_MAP.get(".zsh"),
  "vue": o.LANG_MAP.get(".vue"), "cjs": o.LANG_MAP.get(".cjs"),
  "psm1_parsed": ".psm1" in extractor._DISPATCH, "zsh_parsed": ".zsh" in extractor._DISPATCH,
}))
'''
out = subprocess.run([sys.executable, "-c", code], cwd=f"{wt}/scripts/ast",
                     capture_output=True, text=True)
print(out.stdout.strip() or out.stderr.strip())
PY
)"
say "  $r83"
case "$r83" in
  *'"gap": []'*)   ok "every parsed extension has a language (gap 0)";;
  *)               bad "parsed extensions with no language remain";;
esac
case "$r83" in
  *'"extra": [".blade.php"]'*) ok "the only mapped-but-unparsed entry is .blade.php, matched by filename at extractor.py:7421 -- which is why LANG_MAP is one longer than _DISPATCH";;
  *)                           bad "an unexpected mapped-but-unparsed extension";;
esac
case "$r83" in *'"unknown": []'*) ok 'no extension maps to the literal "unknown"';; *) bad 'an extension maps to "unknown"';; esac
case "$r83" in *'"psm1_parsed": true'*) ok ".psm1 is parsed, not merely claimed";; *) bad ".psm1 still claims a language nothing parses";; esac
case "$r83" in *'"zsh_parsed": true'*)  ok ".zsh is parsed, not merely claimed";;  *) bad ".zsh still claims a language nothing parses";; esac

# RED on the base sha, from the same probe.
red83="$(git -C "$WT" show "$BASE_SHA:scripts/ast/onto_ast.py" > "$WORK/onto.base.py" && \
  mkdir -p "$WORK/base_ast" && \
  git -C "$WT" archive "$BASE_SHA" scripts/ast | tar -x -C "$WORK" && \
  (cd "$WORK/scripts/ast" && $PY -c '
import sys; sys.path.insert(0, ".")
import extractor, onto_ast as o
print("gap", len(set(extractor._DISPATCH) - set(o.LANG_MAP)))') 2>&1)"
say "  on $BASE_SHA: $red83"
case "$red83" in *"gap 22"*) ok "RED on $BASE_SHA: 22 parsed extensions had no language";; *) bad "expected 'gap 22' on the base sha, got: $red83";; esac

# ── #84 ─────────────────────────────────────────────────────────────────────
head_ "#84 single-file components"
if ! have_grammars; then
  skip "tree_sitter_javascript is not installed -- the SFC extraction rows were NOT run"
else
  FIX="$WORK/sfc"; mkdir -p "$FIX"
  printf '<template><div/></template>\n<script>\nexport default { methods: { aObjMethod() { return 1; } } }\n</script>\n' > "$FIX/a_obj.vue"
  printf '<template><div/></template>\n<script>\nexport let bName;\nfunction bFn() { return 2; }\n</script>\n' > "$FIX/b_fn.vue"
  printf '<script>\nexport default { methods: { cObjMethod() { return 3; } } }\n</script>\n<div/>\n' > "$FIX/c_obj.svelte"
  printf '<script>\nexport let dName;\nfunction dFn() { return 4; }\n</script>\n<div/>\n' > "$FIX/d_fn.svelte"
  printf '<template><div/></template>\n<script>\nfunction eFn() { return 5; }\n</script>\n' > "$FIX/e_fnonly.vue"
  printf 'export function plainFn() { return 6; }\n' > "$FIX/plain.vue"

  run_fixture() {  # <scripts-dir> <label>
    local sdir="$1" label="$2" out="$WORK/sfc.$2.ttl"
    BASE_AST_OUT="$WORK" $PY "$sdir/onto_ast.py" --full --confirm --out "$out" "$FIX" \
      > "$WORK/sfc.$2.log" 2>&1 || return 1
    $PY - "$out" <<'PY'
import sys, re
from collections import defaultdict
raw = open(sys.argv[1], encoding="utf-8", errors="replace").read().splitlines()
cur=None; lab=None; by=defaultdict(list); typ={}
for l in raw:
    if l.startswith("code:") and " a ops:" in l:
        cur=l.split()[0]; typ[cur]=l.split("a ops:")[1].rstrip(" ;")
    m=re.search(r'rdfs:label\s+"([^"]*)"', l)
    if m and cur: lab=m.group(1)
    m=re.search(r'ops:sourceFile\s+"([^"]*)"', l)
    if m and cur: by[m.group(1)].append((lab, typ.get(cur,"?")))
for f in sorted(by):
    if f.startswith("/"): continue
    fns=sorted(l for l,t in by[f] if t != "Module")
    print(f"{f}={','.join(fns) if fns else 'NONE'}")
PY
  }

  git -C "$WT" archive "$BASE_SHA" scripts/ast | tar -x -C "$WORK" 2>/dev/null
  base_out="$(run_fixture "$WORK/scripts/ast" base)" || bad "the $BASE_SHA extractor failed on the fixture"
  new_out="$(run_fixture "$WT/scripts/ast" branch)" || bad "the branch extractor failed on the fixture"
  say "  --- $BASE_SHA ---"; printf '%s\n' "$base_out" | sed 's/^/    /'
  say "  --- $BRANCH_SHA ---"; printf '%s\n' "$new_out" | sed 's/^/    /'

  for row in "d_fn.svelte=NONE" "e_fnonly.vue=NONE"; do
    printf '%s\n' "$base_out" | grep -qx "$row" \
      && ok "RED on $BASE_SHA: ${row%=*} yielded no entities" \
      || bad "expected '$row' on the base sha"
  done
  for row in "a_obj.vue=aObjMethod()" "b_fn.vue=bFn()" "c_obj.svelte=cObjMethod()" \
             "d_fn.svelte=dFn()" "e_fnonly.vue=eFn()" "plain.vue=plainFn()"; do
    printf '%s\n' "$new_out" | grep -qx "$row" \
      && ok "GREEN: $row" \
      || bad "expected '$row' on the branch, got: $(printf '%s\n' "$new_out" | grep "^${row%%=*}=" || echo missing)"
  done
fi

# ── A1: byte-identity, split in two ─────────────────────────────────────────
head_ "A1a must-NOT-move: a tree with no .vue and no .svelte"
if ! have_grammars; then
  skip "grammars missing -- the ast query sweep was NOT run"
else
  NOSFC="$WORK/nosfc"; mkdir -p "$NOSFC"
  printf 'def alpha():\n    return 1\n\nclass Beta:\n    def gamma(self):\n        return 2\n' > "$NOSFC/a.py"
  printf 'export function delta() { return 3; }\n' > "$NOSFC/b.js"
  printf '# Title\n\nSome prose.\n\n## Section\n' > "$NOSFC/c.md"
  git -C "$WT" archive "$BASE_SHA" scripts/ast | tar -x -C "$WORK" 2>/dev/null
  BASE_AST_OUT="$WORK" $PY "$WORK/scripts/ast/onto_ast.py" --full --confirm \
    --out "$WORK/nosfc.base.ttl" "$NOSFC" >/dev/null 2>&1
  BASE_AST_OUT="$WORK" $PY "$WT/scripts/ast/onto_ast.py" --full --confirm \
    --out "$WORK/nosfc.branch.ttl" "$NOSFC" >/dev/null 2>&1
  if cmp -s "$WORK/nosfc.base.ttl" "$WORK/nosfc.branch.ttl"; then
    ok "the map is byte-identical on a tree with no SFC ($(wc -l < "$WORK/nosfc.branch.ttl") lines)"
  else
    bad "the map moved on a tree the change should not touch:"
    diff "$WORK/nosfc.base.ttl" "$WORK/nosfc.branch.ttl" | head -20 | sed 's/^/        /'
  fi
fi

head_ "A1b must-move: the delta on an SFC tree, enumerated"
if ! have_grammars; then
  skip "grammars missing"
elif [ -f "$WORK/sfc.base.ttl" ] && [ -f "$WORK/sfc.branch.ttl" ]; then
  d="$(diff <(grep -c . "$WORK/sfc.base.ttl") <(grep -c . "$WORK/sfc.branch.ttl") >/dev/null && echo same || echo differs)"
  say "  sfc map: $(grep -c . "$WORK/sfc.base.ttl") -> $(grep -c . "$WORK/sfc.branch.ttl") non-empty lines ($d)"
  added="$(diff "$WORK/sfc.base.ttl" "$WORK/sfc.branch.ttl" | grep '^>' | grep -o 'rdfs:label "[^"]*"' | sort -u | sed 's/rdfs:label //' | tr '\n' ' ')"
  say "  labels only in the branch map: $added"
  case "$added" in
    *'"dFn()"'*) ok "dFn() appears where it did not before";;
    *) bad "dFn() is not in the added labels";;
  esac
  case "$added" in
    *'"eFn()"'*) ok "eFn() appears where it did not before";;
    *) bad "eFn() is not in the added labels";;
  esac
  case "$added" in
    *'"aObjMethod()"'*|*'"bFn()"'*|*'"cObjMethod()"'*|*'"plainFn()"'*)
      bad "a symbol that already extracted is listed as added -- its id or line moved";;
    *) ok "no already-extracting symbol changed (the delta is only what was missing)";;
  esac
else
  skip "the SFC maps were not produced"
fi

# ── #89 ─────────────────────────────────────────────────────────────────────
head_ "#89 notices persist through a build nobody watched"
BIN="${BASE_BIN:-$HOME/.cache/plover-target/release/base}"
if [ ! -x "$BIN" ]; then
  skip "no branch binary at $BIN -- the end-to-end notice rows were NOT run"
else
  say "  binary  $(md5sum "$BIN" | cut -d' ' -f1)  $BIN"
  # `grep -c`, not `grep -q`: under `set -o pipefail` a `-q` exits at the
  # first match, `strings` takes SIGPIPE, and the pipeline reports failure ON
  # A MATCH. This aborted on a binary that did carry the symbol.
  hits="$(strings "$BIN" 2>/dev/null | grep -c "last-notices" || true)"
  if [ "${hits:-0}" -lt 1 ]; then
    die "the binary at $BIN does not carry '.last-notices' ($hits hits) -- not this branch's build"
  fi
  ok "the binary carries the branch-only '.last-notices' string ($hits occurrences)"

  T="$WORK/e2e"; mkdir -p "$T"
  git -C "$T" init -q 2>/dev/null
  printf 'def alpha():\n    return 1\n' > "$T/a.py"
  printf '<script>\nexport let x;\nfunction f() {}\n</script>\n' > "$T/b.svelte"
  # A file the extractor cannot read at all, so there IS something to report.
  printf '\x00\x01\x02 not source\n' > "$T/broken.py"

  (cd "$T" && BASE_AST_SKIP_REGISTER=1 "$BIN" sync --ast --yes --target . ) \
    > "$WORK/e2e.log" 2>&1
  rc=$?
  say "  sync exit $rc"
  if [ -f "$T/.base-ast/.last-notices" ]; then
    ok "a foreground --yes sync left .last-notices ($(wc -l < "$T/.base-ast/.last-notices") line(s))"
    sed 's/^/        /' "$T/.base-ast/.last-notices"
    grep -q "Extracting" "$T/.base-ast/.last-notices" \
      && bad "the routine per-run line was persisted; it would become a banner" \
      || ok "the routine 'Extracting N files' line is not persisted"
  else
    skip "no .last-notices -- this fixture produced nothing worth reporting"
  fi
fi

say ""
say "==== $pass PASS / $failn FAIL / $skipn SKIP ===="
[ "$failn" -eq 0 ] && exit 0 || exit 1
