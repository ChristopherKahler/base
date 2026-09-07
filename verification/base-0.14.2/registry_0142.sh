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
LOCKED=$(grep -ac 'waiting for the graph lock' "$BIN" 2>/dev/null || true)
LOCKED=${LOCKED:-0}
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
count_in () {                    # count_in <file> <pattern>  -> a number, always
  # grep -c prints the count AND exits 1 when it is zero. Taking the `||` branch
  # on a real zero appended a second "0" and every later arithmetic broke on it.
  local n
  n=$(grep -c "$2" "$1" 2>/dev/null) || true
  printf '%s' "${n:-0}"
}
rows_typed () {                  # rows_typed <graph> <slug-prefix>
  count_in "$1" "handoff/$2.*22-rdf-syntax-ns#type"
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

# ── R16: a LIVE holder past LOCK_STALE must NOT be reaped ────
# R15 proves a DEAD pid is reaped. This proves the other half, which is the half
# that loses data if it is wrong: `holder_is_alive` is all that stands between a
# slow `graph compact` (legitimately holding the lock past LOCK_STALE) and two
# writers on one graph. Its Windows arm shells out to `tasklist`, its Linux arm
# stats /proc, and the two have never been compared. Same lock file, same old
# mtime, reaped only after the holder actually dies.
if [ "$LOCKED" -ge 1 ]; then
  say "=== R16  a live holder past LOCK_STALE is not reaped ==="
  d=$(fresh r16); export BASE_HOME="$d/home"
  printf '#\n' > "$d/docs/LK2.md"
  ( cd "$d/ws" && "$BIN" fork create --project lk2 --doc "$d/docs/LK2.md" ) >/dev/null 2>&1
  G="$d/ws/.base/graph.nq"

  # The helper reports its OWN os-level pid. A shell job's $! is the msys pid
  # under Git Bash and `tasklist` has never heard of it, so asking the process
  # is the only portable way to get a pid the binary under test can resolve.
  rm -f "$OUT/r16.pid"
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
      ( powershell -NoProfile -Command '$PID; Start-Sleep -Seconds 120' > "$OUT/r16.pid" 2>/dev/null ) &
      ;;
    *)
      ( sh -c 'echo $$; exec sleep 120' > "$OUT/r16.pid" 2>/dev/null ) &
      ;;
  esac
  helper_job=$!
  helper_pid=""
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    helper_pid=$(tr -d ' \r\n' < "$OUT/r16.pid" 2>/dev/null || true)
    case "$helper_pid" in ''|*[!0-9]*) helper_pid="" ;; *) break ;; esac
    sleep 1
  done

  if [ -z "$helper_pid" ]; then
    # Never a silent skip: the row that cannot run says why.
    bad "R16 could not obtain a live os-level pid on $(uname -s) — leg DID NOT RUN"
  else
    say "  live holder pid=$helper_pid"
    printf '%s\n' "$helper_pid" > "$G.lock"
    touch -d '2 hours ago' "$G.lock"
    ( cd "$d/ws" && "$BIN" fork archive LK2 >"$OUT/r16-live.out" 2>&1 )
    rc=$?
    say "  live-holder archive rc=$rc"
    if [ "$rc" != "0" ] && grep -q "graph lock" "$OUT/r16-live.out"; then
      ok "R16 a live holder past LOCK_STALE is refused, not reaped"
    else
      bad "R16 the lock of a LIVE pid $helper_pid was reaped or ignored (rc=$rc)"
    fi
    grep -q "reaping stale graph lock" "$OUT/r16-live.out" \
      && bad "R16 the reap fired on a live holder" \
      || ok "R16 no reap message while the holder was alive"
    grep -q "handoff/LK2> <http://ops-sys.local/ontology#status> \"open\"" "$G" \
      && ok "R16 nothing was written behind the live holder" \
      || bad "R16 the row changed while a live holder held the lock"

    # Kill the holder and rerun: the SAME lock file, the SAME old mtime, now
    # reaped. Only holder_is_alive changed its answer, which is the discrimination.
    case "$(uname -s)" in
      MINGW*|MSYS*|CYGWIN*) taskkill //PID "$helper_pid" //F >/dev/null 2>&1 ;;
      *)                    kill -9 "$helper_pid" >/dev/null 2>&1 ;;
    esac
    wait "$helper_job" 2>/dev/null || true
    sleep 2
    ( cd "$d/ws" && "$BIN" fork archive LK2 >"$OUT/r16-dead.out" 2>&1 )
    rc=$?
    say "  dead-holder archive rc=$rc"
    if [ "$rc" = "0" ] && grep -q "handoff/LK2> <http://ops-sys.local/ontology#status> \"archived\"" "$G"; then
      ok "R16 the same lock is reaped once the holder dies"
    else
      bad "R16 the lock was not reaped after the holder died (rc=$rc)"
    fi
  fi
else
  say "=== R16  skipped: control binary has no lock ==="
fi

# ── R3: the help string promises the tier-scoped behaviour ───
say "=== R3  'handoff create --help' names the tier ==="
"$BIN" handoff create --help > "$OUT/r3.out" 2>&1
if grep -q "archives any prior open handoff for the project in this tier" "$OUT/r3.out"; then
  ok "R3 help says 'in this tier'"
else
  bad "R3 help still promises a project-wide archive: $(grep -m1 'Register a handoff' "$OUT/r3.out")"
fi

# ── R1: create must name the handoff it archived ─────────────
say "=== R1  second create in one tier reports what it closed ==="
d=$(fresh r1); export BASE_HOME="$d/home"
printf '#\n' > "$d/docs/HANDOFF-A.md"; printf '#\n' > "$d/docs/HANDOFF-B.md"
( cd "$d/ws" && "$BIN" handoff create --project pj --doc "$d/docs/HANDOFF-A.md" ) >/dev/null 2>&1
( cd "$d/ws" && "$BIN" handoff create --project pj --doc "$d/docs/HANDOFF-B.md" ) >"$OUT/r1.out" 2>&1
say "  output: $(tr '\n' '|' < "$OUT/r1.out")"
grep -q "handoff/HANDOFF-A> <http://ops-sys.local/ontology#status> \"archived\"" "$d/ws/.base/graph.nq" \
  && ok "R1 A was archived" || bad "R1 A was not archived"
if grep -q "archived prior open handoff: HANDOFF-A (workspace tier)" "$OUT/r1.out"; then
  ok "R1 create names what it archived"
else
  bad "R1 create archived HANDOFF-A silently"
fi

# ── R2: the OTHER tier is named, never touched ───────────────
say "=== R2  an open handoff in the other tier is named, not archived ==="
d=$(fresh r2); export BASE_HOME="$d/home"
printf '#\n' > "$d/docs/HANDOFF-G.md"; printf '#\n' > "$d/docs/HANDOFF-W.md"
( cd "$d/ws" && "$BIN" handoff -g create --project pj --doc "$d/docs/HANDOFF-G.md" ) >/dev/null 2>&1
( cd "$d/ws" && "$BIN" handoff create --project pj --doc "$d/docs/HANDOFF-W.md" ) >"$OUT/r2.out" 2>&1
say "  output: $(tr '\n' '|' < "$OUT/r2.out")"
G="$d/home/.base-gbl/.base/graph.nq"
grep -q "handoff/HANDOFF-G> <http://ops-sys.local/ontology#status> \"open\"" "$G" \
  && ok "R2 the global handoff was left open" || bad "R2 the global handoff was mutated"
if grep -q "global tier also holds an open handoff for 'pj': HANDOFF-G" "$OUT/r2.out" \
   && grep -q "base handoff -g archive HANDOFF-G" "$OUT/r2.out"; then
  ok "R2 output names it and the command that would archive it"
else
  bad "R2 the other tier's open handoff went unmentioned"
fi

# ── R4: cross-tier archive by slug (regression pin, green both sides) ──
say "=== R4  a global-tier fork archives from a workspace cwd, no -g ==="
d=$(fresh r4); export BASE_HOME="$d/home"
printf '#\n' > "$d/docs/XT.md"
( cd "$d/ws" && "$BIN" fork -g create --project xt --doc "$d/docs/XT.md" ) >/dev/null 2>&1
( cd "$d/ws" && "$BIN" fork archive XT ) >"$OUT/r4.out" 2>&1
rc=$?
G="$d/home/.base-gbl/.base/graph.nq"
if [ "$rc" = "0" ] && grep -q "handoff/XT> <http://ops-sys.local/ontology#status> \"archived\"" "$G"; then
  ok "R4 archived across tiers by slug (rc=$rc)"
else
  bad "R4 cross-tier archive failed (rc=$rc)"
fi
if [ "$LOCKED" -ge 1 ]; then
  grep -q "archived (global tier)" "$OUT/r4.out" \
    && ok "R4 names the tier it changed" \
    || bad "R4 did not name the tier: $(cat "$OUT/r4.out")"
fi

# ── R5/R6: a no-op must never print success ──────────────────
say "=== R5/R6  archive and snooze of a slug no tier holds ==="
d=$(fresh r5); export BASE_HOME="$d/home"
printf '#\n' > "$d/docs/REAL.md"
( cd "$d/ws" && "$BIN" fork create --project np --doc "$d/docs/REAL.md" ) >/dev/null 2>&1
for cmd in "fork archive" "fork snooze" "handoff archive" "handoff snooze"; do
  tag=$(printf '%s' "$cmd" | tr ' ' '-')
  case "$cmd" in
    *snooze) ( cd "$d/ws" && "$BIN" $cmd shrike-no-such-slug-xyz 3 ) >"$OUT/r5-$tag.out" 2>&1 ;;
    *)       ( cd "$d/ws" && "$BIN" $cmd shrike-no-such-slug-xyz )   >"$OUT/r5-$tag.out" 2>&1 ;;
  esac
  rc=$?
  say "  '$cmd <missing>' rc=$rc  -> $(head -1 "$OUT/r5-$tag.out")"
  if [ "$rc" != "0" ] && grep -q "in either tier" "$OUT/r5-$tag.out"; then
    ok "R5/R6 '$cmd' fails loudly on a no-op"
  else
    bad "R5/R6 '$cmd' reported success for a slug no tier holds (rc=$rc)"
  fi
done

# ── R13: the production shape — registry writes vs relay pings ──
# On 2026-09-07 the global graph took five writes between 15:07:14 and 15:07:15:
# an ops:Ping INSERT and four archive UPDATEs. The interleaving writer was
# `relay ping`, not a registry command, which is why a registry-only lock would
# have left the reported incident standing. This row is that shape.
say "=== R13  8 concurrent 'fork -g archive' interleaved with 8 'relay ping' ==="
d=$(fresh r13); export BASE_HOME="$d/home"
export CLAUDE_CODE_SESSION_ID="00000000-0000-0000-0000-00000000r13a"
( cd "$d/ws" && "$BIN" relay register --as r13target ) >/dev/null 2>&1
for i in $(seq 1 8); do
  printf '#\n' > "$d/docs/g-$i.md"
  ( cd "$d/ws" && "$BIN" fork -g create --project r13 --doc "$d/docs/g-$i.md" ) >/dev/null 2>&1
done
G="$d/home/.base-gbl/.base/graph.nq"
seeded=$(count_in "$G" "handoff/g-.*22-rdf-syntax-ns#type")
pids=""
for i in $(seq 1 8); do
  ( cd "$d/ws" && "$BIN" fork -g archive "g-$i" >"$OUT/r13-a$i.out" 2>&1 ) & pids="$pids $!"
  ( cd "$d/ws" && "$BIN" relay ping --to r13target --from r13send --msg "p$i" \
      >"$OUT/r13-p$i.out" 2>&1 ) & pids="$pids $!"
done
for p in $pids; do wait "$p"; done
arch=0
for i in $(seq 1 8); do
  grep -q "handoff/g-$i> <http://ops-sys.local/ontology#status> \"archived\"" "$G" && arch=$((arch+1))
done
# The ping half is asserted as an INVARIANT, not a count of 8.
# A ping's slug is `ping-<epoch millis>`, so eight pings fired inside two
# milliseconds collapse onto two slugs by construction — measured on the branch:
# 8 commands, 8 changelog INSERTs, 2 inbox files, 2 graph rows, the two slugs one
# millisecond apart. That is the slug scheme, not a lost write, and it is its own
# defect (reported separately). What the lock owes is that every ping which got
# an inbox alert also kept its graph row: inbox files are one-file-per-ping and
# never whole-file rewritten, so they are the ground truth for how many distinct
# pings existed.
inbox=$(ls "$d/home/.base-gbl/.base/relay-inbox/r13target/" 2>/dev/null | grep -c json) || true
graph_pings=$(grep -o "ontology#ping/[a-z0-9-]*" "$G" 2>/dev/null | sort -u | wc -l)
say "  seeded=$seeded archived=$arch of 8   distinct pings: graph=$graph_pings inbox=${inbox:-0}"
[ "$arch" = "8" ] && ok "R13 all 8 archives survived the ping traffic" \
                  || bad "R13 only $arch of 8 archived alongside relay pings"
if [ "${inbox:-0}" -gt 0 ] && [ "$graph_pings" = "${inbox:-0}" ]; then
  ok "R13 every ping that alerted also kept its graph row ($graph_pings)"
else
  bad "R13 graph holds $graph_pings ping rows for ${inbox:-0} alerted pings"
fi
unset CLAUDE_CODE_SESSION_ID

# ── Isolation tripwire: the real graphs must be untouched ────
# The operator's graphs live at a different path depending on which shell is
# driving: /mnt/c/... under WSL, /c/... under Git Bash on Windows. Listing only
# the WSL spellings made every Windows run take the `[ -f ] || continue` branch
# on both entries and print PASS having opened nothing — a silent false green in
# the one row whose entire job is to prove nothing leaked. Both spellings are
# listed now, and a run that inspected ZERO graphs is a FAIL, not a pass.
say "=== isolation ==="
leak=0
checked=0
for g in /mnt/c/Users/Chris/.base/graph.nq /mnt/c/Users/Chris/.base-gbl/.base/graph.nq \
         /c/Users/Chris/.base/graph.nq /c/Users/Chris/.base-gbl/.base/graph.nq; do
  [ -f "$g" ] || continue
  checked=$((checked+1))
  if grep -q "handoff/s-1>\|handoff/a-1>\|handoff/t1>\|handoff/n-1>\|handoff/HANDOFF-A>\|handoff/HANDOFF-G>\|handoff/XT>\|handoff/REAL>\|handoff/g-1>\|handoff/LK2>" "$g"; then
    leak=1; say "  LEAK into $g"
  fi
done
say "  operator graphs inspected: $checked"
if [ "$checked" = "0" ]; then
  bad "isolation proved NOTHING — no operator graph was found at any known path"
elif [ "$leak" = "0" ]; then
  ok "no harness slug reached the operator's graphs ($checked graph(s) inspected)"
else
  bad "harness leaked into a real graph"
fi

say ""
say "registry_0142: $PASS pass, $FAIL fail  (binary $BIN_VER md5 $BIN_MD5 lockmark $LOCKED)"
[ "$FAIL" = "0" ]
