#!/usr/bin/env bash
# R14, hook leg — `base hook post-tool-use` is a WRITER, and it is now locked.
# It fires on every tool call, so its cost is the one that matters most.
set -u
N=10
ROOT=/mnt/c/base-g0/r14h
SRC=/mnt/c/Users/Chris/.base-gbl/.base/graph.nq
CTRL=/tmp/base-0141
BR=/tmp/base-branch

median () { sort -n | awk '{a[NR]=$1} END {printf "%d", (NR%2) ? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2}'; }
mark () { grep -ac 'waiting for the graph lock' "$1" 2>/dev/null || echo 0; }

echo "control md5=$(md5sum "$CTRL" | awk '{print $1}') lockmark=$(mark "$CTRL")"
echo "branch  md5=$(md5sum "$BR"   | awk '{print $1}') lockmark=$(mark "$BR")"
[ "$(mark "$CTRL")" = "0" ] && [ "$(mark "$BR")" = "1" ] || { echo "ABORT: provenance"; exit 2; }
echo "graph: $(wc -c < "$SRC") bytes, copied per run"
echo ""

run_hook () {                    # run_hook <bin> -> ms
  local bin="$1" d="$ROOT/w"
  rm -rf "$d"; mkdir -p "$d/home" "$d/ws/.base"
  cp "$SRC" "$d/ws/.base/graph.nq"
  export BASE_HOME="$d/home"
  local ev
  ev=$(printf '{"tool_name":"Edit","tool_input":{"file_path":"%s/ws/src/main.rs"},"cwd":"%s/ws"}' "$d" "$d")
  local s e
  s=$(date +%s%N)
  ( cd "$d/ws" && printf '%s' "$ev" | "$bin" hook post-tool-use ) >/dev/null 2>&1
  e=$(date +%s%N)
  printf '%d' $(( (e - s) / 1000000 ))
}

mkdir -p "$ROOT"
for tag in control branch; do
  [ "$tag" = control ] && bin="$CTRL" || bin="$BR"
  : > "$ROOT/$tag.txt"
  for _ in $(seq 1 $N); do run_hook "$bin" >> "$ROOT/$tag.txt"; printf '\n' >> "$ROOT/$tag.txt"; done
  printf 'hook post-tool-use  %-8s median %s ms   (n=%s: %s)\n' \
    "$tag" "$(median < "$ROOT/$tag.txt")" "$N" "$(tr '\n' ' ' < "$ROOT/$tag.txt")"
done
