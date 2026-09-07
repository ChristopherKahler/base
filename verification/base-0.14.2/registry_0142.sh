#!/usr/bin/env bash
# registry_0142.sh — #71 #72 #73 #74, the registry cluster.
#
# Driven by BASE_BIN. Red on the 0.14.1 control, green on the branch binary.
# Everything runs against a fake BASE_HOME under a real drive-lettered path
# (never /tmp: a /tmp fake root drops the drive letter and flips base's own
# case rule — F29, 2026-09-07), from a cwd with no .base/ above it, so the
# operator's graphs are never touched. The isolation is PROVEN at the end, not
# assumed.
#
# Exits go to files, never through a pipe: `cmd | tail` reports tail's status.
set -u

BIN="${BASE_BIN:?set BASE_BIN to the binary under test}"
ROOT="${HARNESS_ROOT:-/mnt/c/base-g0/run}"
OUT="$ROOT/_out"
PASS=0; FAIL=0

say () { printf '%s\n' "$*"; }
ok   () { PASS=$((PASS+1)); printf 'PASS  %s\n' "$*"; }
bad  () { FAIL=$((FAIL+1)); printf 'FAIL  %s\n' "$*"; }

# ── Provenance (law 15). No numbers from an unidentified binary. ──
say "=== provenance ==="
[ -x "$BIN" ] || { say "ABORT: BASE_BIN not executable: $BIN"; exit 2; }
BIN_MD5=$(md5sum "$BIN" | awk '{print $1}')
BIN_VER=$("$BIN" --version 2>&1 | head -1)
# The branch-only marker: a user-visible string only this change emits.
LOCKED=$(strings "$BIN" 2>/dev/null | grep -c 'waiting for the graph lock')
say "binary   : $BIN"
say "version  : $BIN_VER"
say "md5      : $BIN_MD5"
say "lock mark: $LOCKED  (0 = control/0.14.1, >=1 = branch)"
if [ -n "${EXPECT_LOCKED:-}" ] && [ "$EXPECT_LOCKED" != "$LOCKED" ]; then
  say "ABORT: EXPECT_LOCKED=$EXPECT_LOCKED but this binary reports $LOCKED."
  say "       That is the wrong binary — refusing to report numbers from it."
  exit 2
fi
say ""

fresh () {                       # fresh <tag>  -> echoes the workspace dir
  local d="$ROOT/$1"
  rm -rf "$d"
  mkdir -p "$d/home" "$d/ws/.base" "$d/docs" "$OUT"
  printf '%s' "$d"
}
rows_typed () {                  # rows_typed <graph> <slug-prefix>
  grep -c "handoff/$2.*22-rdf-syntax-ns#type" "$1" 2>/dev/null || printf '0'
}

# ── R10: concurrent create must not lose rows ────────────────
say "=== R10  8 concurrent 'fork create' on one tier ==="
d=$(fresh r10); export BASE_HOME="$d/home"
for i in $(seq 1 8); do printf '#\n' > "$d/docs/s-$i.md"; done
( cd "$d/ws" && "$BIN" fork create --project conc --doc "$d/docs/s-1.md" ) >/dev/null 2>&1
pids=""
for i in $(seq 2 8); do
  ( cd "$d/ws" && "$BIN" fork create --project conc --doc "$d/docs/s-$i.md" \
      >"$OUT/r10-$i.out" 2>&1; printf '%s' "$?" > "$OUT/r10-$i.rc" ) &
  pids="$pids $!"
done
for p in $pids; do wait "$p"; done
present=$(rows_typed "$d/ws/.base/graph.nq" "s-")
nz=0; for i in $(seq 2 8); do [ "$(cat "$OUT/r10-$i.rc")" = "0" ] || nz=$((nz+1)); done
say "  launched=8 rows_present=$present non_zero_exits=$nz"
[ "$present" = "8" ] && ok "R10 all 8 rows present" || bad "R10 only $present of 8 rows survived"

# ── R11: concurrent archive must not lose updates ────────────
say "=== R11  8 concurrent 'fork archive' ==="
d=$(fresh r11); export BASE_HOME="$d/home"
for i in $(seq 1 8); do
  printf '#\n' > "$d/docs/a-$i.md"
  ( cd "$d/ws" && "$BIN" fork create --project arc --doc "$d/docs/a-$i.md" ) >/dev/null 2>&1
done
seeded=$(rows_typed "$d/ws/.base/graph.nq" "a-")
pids=""
for i in $(seq 1 8); do
  ( cd "$d/ws" && "$BIN" fork archive "a-$i" >"$OUT/r11-$i.out" 2>&1 ) &
  pids="$pids $!"
done
for p in $pids; do wait "$p"; done
arch=0
for i in $(seq 1 8); do
  grep -q "handoff/a-$i> <http://ops-sys.local/ontology#status> \"archived\"" \
    "$d/ws/.base/graph.nq" && arch=$((arch+1))
done
say "  seeded=$seeded archived=$arch of 8"
[ "$arch" = "8" ] && ok "R11 all 8 archived" || bad "R11 only $arch of 8 archived"

# ── R12: create+archive under a concurrent writer (#73/#74) ──
say "=== R12  50x create+archive with one concurrent base writer ==="
d=$(fresh r12); export BASE_HOME="$d/home"
printf '#\n' > "$d/docs/seed.md"
( cd "$d/ws" && "$BIN" fork create --project seed --doc "$d/docs/seed.md" ) >/dev/null 2>&1
( for k in $(seq 1 400); do
    printf '#\n' > "$d/docs/n-$k.md"
    ( cd "$d/ws" && "$BIN" fork create --project noise --doc "$d/docs/n-$k.md" ) >/dev/null 2>&1
  done ) &
NOISE=$!
absent=0; unarch=0
for i in $(seq 1 50); do
  s="t$i"; printf '#\n' > "$d/docs/$s.md"
  ( cd "$d/ws" && "$BIN" fork create --project i73 --doc "$d/docs/$s.md" ) >/dev/null 2>&1
  ( cd "$d/ws" && "$BIN" fork archive "$s" ) >/dev/null 2>&1
  if ! grep -q "handoff/$s> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type>" "$d/ws/.base/graph.nq"; then
    absent=$((absent+1))
  elif ! grep -q "handoff/$s> <http://ops-sys.local/ontology#status> \"archived\"" "$d/ws/.base/graph.nq"; then
    unarch=$((unarch+1))
  fi
done
kill "$NOISE" 2>/dev/null; wait "$NOISE" 2>/dev/null
say "  50 iterations: rows_absent=$absent not_archived=$unarch"
[ "$absent" = "0" ] && ok "R12 no row vanished" || bad "R12 $absent of 50 rows vanished (#73)"
[ "$unarch" = "0" ] && ok "R12 every archive landed" || bad "R12 $unarch of 50 archives lost (#74)"

# ── R15: the lock's own failure paths (branch only) ──────────
if [ "$LOCKED" -ge 1 ]; then
  say "=== R15  lock timeout and stale reap ==="
  d=$(fresh r15); export BASE_HOME="$d/home"
  printf '#\n' > "$d/docs/L.md"
  ( cd "$d/ws" && "$BIN" fork create --project lk --doc "$d/docs/L.md" ) >/dev/null 2>&1
  G="$d/ws/.base/graph.nq"

  # A held lock must make the write fail loudly, not wait forever or write anyway.
  printf '999999\n' > "$G.lock"
  ( cd "$d/ws" && "$BIN" fork archive L >"$OUT/r15-timeout.out" 2>&1 )
  rc=$?; printf '%s' "$rc" > "$OUT/r15-timeout.rc"
  say "  held-lock archive rc=$rc"
  if [ "$rc" != "0" ] && grep -q "graph lock" "$OUT/r15-timeout.out"; then
    ok "R15 timeout is a loud non-zero error naming the lock"
  else
    bad "R15 held lock did not produce a loud non-zero error (rc=$rc)"
  fi
  grep -q "handoff/L> <http://ops-sys.local/ontology#status> \"open\"" "$G" \
    && ok "R15 nothing was written while the lock was held" \
    || bad "R15 the row changed despite the held lock"

  # A lock with an old mtime and no holder is reaped and the write proceeds.
  touch -d '2 hours ago' "$G.lock"
  ( cd "$d/ws" && "$BIN" fork archive L >"$OUT/r15-stale.out" 2>&1 )
  rc=$?
  say "  stale-lock archive rc=$rc"
  if [ "$rc" = "0" ] && grep -q "handoff/L> <http://ops-sys.local/ontology#status> \"archived\"" "$G"; then
    ok "R15 stale lock reaped, write proceeded"
  else
    bad "R15 stale lock was not reaped (rc=$rc)"
  fi
  grep -q "reaping stale graph lock" "$OUT/r15-stale.out" \
    && ok "R15 the reap says so on stderr" \
    || bad "R15 the reap was silent"
else
  say "=== R15  skipped: control binary has no lock ==="
fi

# ── Isolation tripwire: the real graphs must be untouched ────
say "=== isolation ==="
leak=0
for g in /mnt/c/Users/Chris/.base/graph.nq /mnt/c/Users/Chris/.base-gbl/.base/graph.nq; do
  [ -f "$g" ] || continue
  if grep -q "handoff/s-1>\|handoff/a-1>\|handoff/t1>\|handoff/n-1>" "$g"; then
    leak=1; say "  LEAK into $g"
  fi
done
[ "$leak" = "0" ] && ok "no harness slug reached the operator's graphs" || bad "harness leaked into a real graph"

say ""
say "registry_0142: $PASS pass, $FAIL fail  (binary $BIN_VER md5 $BIN_MD5 lockmark $LOCKED)"
[ "$FAIL" = "0" ]
