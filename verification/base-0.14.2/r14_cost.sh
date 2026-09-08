#!/usr/bin/env bash
# R14 — what the lock costs. Two syscalls on a write that already takes ~1s.
#
# Ten runs each, medians reported. Both binaries are identified by md5 and by the
# branch-only marker before any number is printed: a timing run against the wrong
# binary is the failure mode law 15 exists for.
set -u
N=10
ROOT=/mnt/c/base-g0/r14
SRC=/mnt/c/Users/Chris/.base-gbl/.base/graph.nq   # copied, never written

CTRL=/home/chriskahler/.cache/shrike-controls/base-0141-linux
BR=/home/chriskahler/.cache/shrike-target/release/base

ident () {
  local b="$1"
  printf '%s  md5=%s  lockmark=%s  %s\n' \
    "$(basename "$b")" \
    "$(md5sum "$b" | awk '{print $1}')" \
    "$(grep -ac 'waiting for the graph lock' "$b" 2>/dev/null || echo 0)" \
    "$("$b" --version 2>&1 | head -1)"
}
median () { sort -n | awk '{a[NR]=$1} END {printf "%d", (NR%2) ? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2}'; }

say () { printf '%s\n' "$*"; }
say "=== binaries under test ==="
say "control: $(ident "$CTRL")"
say "branch : $(ident "$BR")"
[ "$(grep -ac 'waiting for the graph lock' "$CTRL" || echo 0)" = "0" ] || { say "ABORT: control carries the lock"; exit 2; }
[ "$(grep -ac 'waiting for the graph lock' "$BR" || echo 0)" -ge 1 ] || { say "ABORT: branch lacks the lock"; exit 2; }
say ""
say "source graph: $SRC  ($(wc -c < "$SRC") bytes, copied per run, never written)"
say ""

# one timed op on a fresh copy of the real graph
timed () {                       # timed <bin> <op>  -> milliseconds
  local bin="$1" op="$2" d="$ROOT/w"
  rm -rf "$d"; mkdir -p "$d/home" "$d/ws/.base" "$d/docs"
  cp "$SRC" "$d/ws/.base/graph.nq"
  printf '#\n' > "$d/docs/TIMED.md"
  export BASE_HOME="$d/home"
  if [ "$op" = "archive" ]; then
    ( cd "$d/ws" && "$bin" fork create --project t14 --doc "$d/docs/TIMED.md" ) >/dev/null 2>&1
  fi
  local s e
  s=$(date +%s%N)
  case "$op" in
    create)  ( cd "$d/ws" && "$bin" fork create --project t14 --doc "$d/docs/TIMED.md" ) >/dev/null 2>&1 ;;
    archive) ( cd "$d/ws" && "$bin" fork archive TIMED ) >/dev/null 2>&1 ;;
  esac
  e=$(date +%s%N)
  printf '%d' $(( (e - s) / 1000000 ))
}

for op in create archive; do
  for tag in control branch; do
    [ "$tag" = control ] && bin="$CTRL" || bin="$BR"
    : > "$ROOT/$tag-$op.txt" 2>/dev/null || { mkdir -p "$ROOT"; : > "$ROOT/$tag-$op.txt"; }
    for _ in $(seq 1 $N); do timed "$bin" "$op" >> "$ROOT/$tag-$op.txt"; printf '\n' >> "$ROOT/$tag-$op.txt"; done
    printf 'fork %-8s %-8s median %s ms   (n=%s: %s)\n' \
      "$op" "$tag" "$(median < "$ROOT/$tag-$op.txt")" "$N" \
      "$(tr '\n' ' ' < "$ROOT/$tag-$op.txt")"
  done
done
