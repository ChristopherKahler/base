#!/usr/bin/env bash
# Acceptance harness for base 0.14.3 #107 — one AST relation vocabulary.
#
#   ./verification/base-0.14.3/relation_vocabulary_0143.sh <repo> [BASE_SHA]
#
# Every row runs TWICE: once with the BASE generation of `scripts/ast` and once
# with the BRANCH generation, so each row carries its own red-first proof rather
# than pointing at a run someone has to take on trust (law 25).
#
# The two arms are built as follows, and the asymmetry is deliberate:
#
#   base arm   = BASE_SHA's extractor.py, ttl_serializer.py, onto_ast.py,
#                cache.py, detect.py                       <- the SUBJECT
#              + the branch's relations.py, relation_vocabulary.py,
#                test_relation_corpus.py                   <- the INSTRUMENT
#   branch arm = the branch's working tree, all of it
#
# The instrument is new in this change, so an arm built purely from BASE_SHA
# could not run the rows at all — and "could not run" reported as a red is
# law 24's void FAIL. Pairing the old subject with the new instrument is the
# only construction in which a red means what it says.
#
# Exit codes are ASSIGNED, never inherited (law 27):
#   0  every row passed
#   1  one or more rows failed
#   2  the run could not be trusted: provenance, generation identity, or a
#      reconciliation refused. No row results are printed.
#   3  a row visited zero items (law 23 — checked=0 is a FAIL, never a pass)

set -uo pipefail

REPO="${1:?usage: $0 <repo> [BASE_SHA]}"
BASE_SHA="${2:-}"
WORK="${WORK:-$HOME/osprey-accept/relations}"

# Deliberately NOT under ~/.cache: detect.py treats a `.cache` path as a noise
# directory and walks zero files, so every fixture row would score an empty map
# and two of them would pass on it. (plover, base-ast-followups, instrument bug 1.)
case "$WORK" in
  *".cache"*) echo "ABORT[2]: WORK ($WORK) is under a .cache path; detect.py walks it as noise"; exit 2;;
esac

PASS=0; FAIL=0
row() { # row <name> <ok:0|1> <detail>
  if [ "$2" = "0" ]; then PASS=$((PASS+1)); printf 'PASS  %-46s %s\n' "$1" "$3"
  else FAIL=$((FAIL+1)); printf 'FAIL  %-46s %s\n' "$1" "$3"; fi
}
die() { echo "ABORT[$1]: $2"; exit "$1"; }

command -v python3 >/dev/null 2>&1 || die 2 "python3 absent — every row below needs it and a skip is indistinguishable from a pass"
python3 -c 'import tree_sitter, tree_sitter_php, tree_sitter_java' 2>/dev/null \
  || die 2 "tree-sitter grammars absent (tree_sitter / tree_sitter_php / tree_sitter_java) — the corpus rows cannot run"

cd "$REPO" || die 2 "no repo at $REPO"
HEAD_SHA=$(git rev-parse HEAD) || die 2 "not a git repo"
if [ -z "$BASE_SHA" ]; then
  BASE_SHA=$(git merge-base HEAD origin/main 2>/dev/null) || die 2 "cannot derive a merge-base; pass BASE_SHA"
fi

echo "== R0 provenance =="
echo "repo        $REPO"
echo "head        $HEAD_SHA"
echo "base        $BASE_SHA"
echo "harness md5 $(md5sum "$0" | cut -d' ' -f1)"

# #100's shape: a BASE_SHA from the wrong era passes a naive "the shas differ"
# guard and then reds loudly on rows that have nothing to do with the branch.
# Compare the scripts/ast TREE OBJECT, which is what actually decides the arms.
BASE_TREE=$(git rev-parse "$BASE_SHA:scripts/ast" 2>/dev/null) || die 2 "no scripts/ast at $BASE_SHA"
HEAD_TREE=$(git rev-parse "HEAD:scripts/ast" 2>/dev/null)      || die 2 "no scripts/ast at HEAD"
echo "scripts/ast base tree $BASE_TREE"
echo "scripts/ast head tree $HEAD_TREE"
[ "$BASE_TREE" = "$HEAD_TREE" ] && die 2 "scripts/ast is identical in both generations — this run would compare a tree with itself"

rm -rf "$WORK"; mkdir -p "$WORK/base" "$WORK/branch" || die 2 "cannot create $WORK"
for f in extractor.py ttl_serializer.py onto_ast.py cache.py detect.py; do
  git show "$BASE_SHA:scripts/ast/$f" > "$WORK/base/$f" 2>/dev/null || die 2 "missing $f at $BASE_SHA"
done
for f in relations.py relation_vocabulary.py test_relation_corpus.py; do
  [ -f "scripts/ast/$f" ] || die 2 "the branch has no scripts/ast/$f — the instrument is what makes the base arm runnable"
  cp "scripts/ast/$f" "$WORK/base/$f"
done
cp scripts/ast/*.py "$WORK/branch/"

BASE_SER_MD5=$(md5sum "$WORK/base/ttl_serializer.py" | cut -d' ' -f1)
BRANCH_SER_MD5=$(md5sum "$WORK/branch/ttl_serializer.py" | cut -d' ' -f1)
echo "ttl_serializer.py  base $BASE_SER_MD5  branch $BRANCH_SER_MD5"
[ "$BASE_SER_MD5" = "$BRANCH_SER_MD5" ] && die 2 "ttl_serializer.py is unchanged — the subject of every row below did not move"
echo "extractor.py       base $(md5sum "$WORK/base/extractor.py" | cut -d' ' -f1)  branch $(md5sum "$WORK/branch/extractor.py" | cut -d' ' -f1)"
echo

# ── A: the contract, both arms ───────────────────────────────────────────────
vocab_probe() { # vocab_probe <arm-dir>
  ( cd "$1" && python3 - <<'PY'
import sys
sys.path.insert(0, ".")
import ttl_serializer as t
from relation_vocabulary import extractor_relations
v = extractor_relations("extractor.py")
missing = sorted(v.relations - set(t.RELATION_MAP))
dead = sorted(set(t.RELATION_MAP) - v.relations)
print("emittable", len(v.relations))
print("mapped", len(t.RELATION_MAP))
print("missing", len(missing), missing)
print("dead", len(dead), dead)
print("unresolved", len(v.unresolved))
print("forwarders", v.shape_counts.get("forwarders", 0))
print("closed_set", v.shape_counts.get("via_closed_set", 0))
PY
  )
}
BASE_A=$(vocab_probe "$WORK/base"); BRANCH_A=$(vocab_probe "$WORK/branch")
get() { printf '%s\n' "$1" | awk -v k="$2" '$1==k{print $2}'; }

echo "== A1 vocabulary completeness =="
printf '%s\n' "$BASE_A"   | sed 's/^/  base   /'
printf '%s\n' "$BRANCH_A" | sed 's/^/  branch /'
B_MISS=$(get "$BASE_A" missing); H_MISS=$(get "$BRANCH_A" missing)
B_EMIT=$(get "$BASE_A" emittable); H_EMIT=$(get "$BRANCH_A" emittable)
[ -z "$B_EMIT" ] && die 3 "A1 base arm produced no emittable count — the probe did not run"
[ "$B_EMIT" -ge 27 ] || die 3 "A1 census found $B_EMIT relations, fewer than the 27 measured on 610636e — a shape stopped matching"
row "A1 red: base drops relations"        "$([ "${B_MISS:-0}" -eq 19 ] && echo 0 || echo 1)" "base missing=$B_MISS (expected 19), emittable=$B_EMIT"
row "A1 green: branch maps every relation" "$([ "${H_MISS:-1}" -eq 0 ] && echo 0 || echo 1)" "branch missing=$H_MISS, emittable=$H_EMIT"
row "A1 no dead keys on the branch"        "$([ "$(get "$BRANCH_A" dead)" = "0" ] && echo 0 || echo 1)" "dead=$(get "$BRANCH_A" dead)"
row "A1 census resolved every slot"        "$([ "$(get "$BRANCH_A" unresolved)" = "0" ] && echo 0 || echo 1)" "unresolved=$(get "$BRANCH_A" unresolved)"
row "A1 forwarder shape still matches"     "$([ "$(get "$BRANCH_A" forwarders)" = "1" ] && echo 0 || echo 1)" "forwarders=$(get "$BRANCH_A" forwarders) (implements has no other spelling)"
row "A1 closed-set shape still matches"    "$([ "$(get "$BRANCH_A" closed_set)" = "1" ] && echo 0 || echo 1)" "via_closed_set=$(get "$BRANCH_A" closed_set) (uses_config has no other spelling)"
echo

echo "== A2 every mapped relation reaches a triple =="
cover_probe() {
  ( cd "$1" && python3 - <<'PY'
import re, sys
sys.path.insert(0, ".")
import ttl_serializer as t
missing = []
for rel, pred in sorted(t.RELATION_MAP.items()):
    x = {"nodes": [{"id": "s", "label": "s", "source_file": "a.py", "source_location": "L1"},
                   {"id": "g", "label": "g", "source_file": "a.py", "source_location": "L2"}],
         "edges": [{"source": "s", "target": "g", "relation": rel}]}
    if not re.search(rf"^code:\S+ ops:{pred} code:\S+ \.$", t.serialize(x, "p", "a.py", "python"), re.M):
        missing.append(rel)
print("checked", len(t.RELATION_MAP))
print("missing", len(missing))
PY
  )
}
BASE_A2=$(cover_probe "$WORK/base"); BRANCH_A2=$(cover_probe "$WORK/branch")
printf '%s\n' "$BASE_A2"   | sed 's/^/  base   /'
printf '%s\n' "$BRANCH_A2" | sed 's/^/  branch /'
[ "$(get "$BRANCH_A2" checked)" -ge 27 ] || die 3 "A2 visited $(get "$BRANCH_A2" checked) relations — a loop over nothing proves nothing"
row "A2 branch: every mapped relation emits" "$([ "$(get "$BRANCH_A2" missing)" = "0" ] && echo 0 || echo 1)" "checked=$(get "$BRANCH_A2" checked) missing=$(get "$BRANCH_A2" missing)"
echo

echo "== A3 an unknown relation is loud, not silent =="
loud_probe() {
  ( cd "$1" && python3 - <<'PY'
import sys
sys.path.insert(0, ".")
import ttl_serializer as t
x = {"nodes": [{"id": "s", "label": "s", "source_file": "a.py", "source_location": "L1"},
               {"id": "g", "label": "g", "source_file": "a.py", "source_location": "L2"}],
     "edges": [{"source": "s", "target": "g", "relation": "wibble_wobble"}]}
try:
    t.serialize(x, "p", "a.py", "python")
    print("raised no")
except Exception as e:
    print("raised yes")
    print("names_relation", "yes" if "wibble_wobble" in str(e) else "no")
    print("names_file", "yes" if "relations.py" in str(e) else "no")
PY
  )
}
BASE_A3=$(loud_probe "$WORK/base"); BRANCH_A3=$(loud_probe "$WORK/branch")
printf '%s\n' "$BASE_A3"   | sed 's/^/  base   /'
printf '%s\n' "$BRANCH_A3" | sed 's/^/  branch /'
row "A3 red: base drops it silently"    "$([ "$(get "$BASE_A3" raised)" = "no" ] && echo 0 || echo 1)"  "base raised=$(get "$BASE_A3" raised)"
row "A3 green: branch raises"           "$([ "$(get "$BRANCH_A3" raised)" = "yes" ] && echo 0 || echo 1)" "branch raised=$(get "$BRANCH_A3" raised)"
row "A3 the error names the relation"   "$([ "$(get "$BRANCH_A3" names_relation)" = "yes" ] && echo 0 || echo 1)" "names_relation=$(get "$BRANCH_A3" names_relation)"
row "A3 the error names the file to edit" "$([ "$(get "$BRANCH_A3" names_file)" = "yes" ] && echo 0 || echo 1)" "names_file=$(get "$BRANCH_A3" names_file)"
echo

echo "== A4 corpus: every relation, real parses =="
for arm in base branch; do
  out="$WORK/$arm-corpus.txt"
  ( cd "$WORK/$arm" && python3 test_relation_corpus.py ) > "$out" 2>&1
  rc=$?
  ex=$(awk '/^exercised by the corpus:/{print $5}' "$out")
  dropped=$(awk -F'[][]' '/^emitted but absent from the ttl:/{n=$0; sub(/.*ttl: /,"",n); sub(/ .*/,"",n); print n}' "$out")
  stale=$(awk '/^stale exclusions:/{print $3}' "$out")
  echo "  $arm  rc=$rc exercised=$ex dropped=$dropped stale=$stale   ($out)"
  if [ "$arm" = base ]; then
    row "A4 red: base corpus drops 18"  "$([ "${dropped:-0}" -eq 18 ] && echo 0 || echo 1)" "dropped=$dropped rc=$rc"
  else
    [ "${ex:-0}" -ge 26 ] || die 3 "A4 branch corpus exercised only ${ex:-0} relations — it cannot tell a drop from a gap"
    row "A4 green: branch corpus drops 0" "$([ "$rc" -eq 0 ] && echo 0 || echo 1)" "rc=$rc exercised=$ex dropped=$dropped"
    row "A4 no stale exclusions"          "$([ "${stale:-1}" -eq 0 ] && echo 0 || echo 1)" "stale=$stale"
  fi
done
echo

# ── B: the maps, per real tree ───────────────────────────────────────────────
map_of() { # map_of <arm-dir> <tree> <out.ttl>
  BASE_AST_OUT="$WORK/cache-$(basename "$2")" python3 - "$1" "$2" "$3" <<'PY'
import sys
from pathlib import Path
arm, tree, out = Path(sys.argv[1]), Path(sys.argv[2]).resolve(), Path(sys.argv[3])
sys.path.insert(0, str(arm))
from extractor import extract, collect_files, _make_id, _file_stem
import ttl_serializer
files = collect_files(tree)
if not files:
    print("ZEROFILES"); raise SystemExit(3)
r = extract(files, cache_root=tree)
fm = {}
for f in files:
    try: rel = str(f.relative_to(tree))
    except ValueError: rel = f.name
    fm[_make_id(rel)] = rel; fm[_make_id(str(f))] = rel
    fm[_make_id(f"{_file_stem(f)}{f.suffix}")] = rel; fm.setdefault(f.name, rel)
out.write_text(ttl_serializer.serialize(r, tree.name, str(tree), "multi", file_map=fm),
               encoding="utf-8")
print("FILES", len(files))
PY
}

declare -A TREES=(
  [ping-chat-hub]="/mnt/c/Users/Chris/tools/ping-chat-hub:31"
  [claude-coop]="/mnt/c/Users/Chris/tools/claude-coop:109"
  [base]="$REPO:243"
)
echo "== B must-not-move / must-move, per tree =="
for name in ping-chat-hub claude-coop base; do
  spec="${TREES[$name]}"; tree="${spec%:*}"; want="${spec##*:}"
  [ -d "$tree" ] || { row "B $name" 1 "tree absent at $tree"; continue; }
  map_of "$WORK/base"   "$tree" "$WORK/$name.base.ttl"   >/dev/null 2>&1 || { row "B $name base map" 1 "base arm failed to map"; continue; }
  map_of "$WORK/branch" "$tree" "$WORK/$name.branch.ttl" >/dev/null 2>&1 || { row "B $name branch map" 1 "branch arm failed to map"; continue; }
  read -r kept lost added <<<"$(python3 - "$WORK/$name.base.ttl" "$WORK/$name.branch.ttl" <<'PY'
import sys
b = set(open(sys.argv[1], encoding="utf-8").read().splitlines())
h = set(open(sys.argv[2], encoding="utf-8").read().splitlines())
b.discard(""); h.discard("")
print(len(b & h), len(b - h), len(h - b))
PY
)"
  [ "${kept:-0}" -gt 0 ] || die 3 "B $name compared zero lines — the maps are empty"
  row "B1 $name must-not-move"  "$([ "${lost:-1}" -eq 0 ] && echo 0 || echo 1)" "kept=$kept lost=$lost"
  added_edges=$(grep -cE '^code:\S+ ops:\S+ code:\S+ \.$' "$WORK/$name.branch.ttl")
  base_edges=$(grep -cE '^code:\S+ ops:\S+ code:\S+ \.$' "$WORK/$name.base.ttl")
  delta=$((added_edges - base_edges))
  row "B2 $name must-move (+$want edges)" "$([ "$delta" -eq "$want" ] && echo 0 || echo 1)" "edges $base_edges -> $added_edges (delta $delta, expected $want)"
done
echo

echo "rows: $PASS PASS / $FAIL FAIL"
[ $((PASS + FAIL)) -gt 0 ] || die 3 "no rows ran at all"
if [ "$FAIL" -eq 0 ]; then exit 0; else exit 1; fi
