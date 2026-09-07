#!/usr/bin/env bash
# merlin — fake-home repros for #41, #40, #22, #20, #19 on the 0.14.0 binary. No cargo, no live store.
set -u
B=${B:-$HOME/.cache/auk/base-traversal-b110810}
R=/tmp/merlin-0141; rm -rf "$R"; mkdir -p "$R/home/.claude" "$R/ws"
export BASE_HOME=$R/home BASE_NO_AUTO_UPDATE=1
say() { echo; echo "----- $* -----"; }

# Any dashboard a leg starts is killed on ANY exit path, not just the happy one.
# A run that ended before its kill line left a 0.14.0 dashboard on a fixed port
# for four hours, and every later run POSTed to that process and reported its
# behaviour as the behaviour of whatever binary it had been handed.
DASH_PIDS=()
cleanup() {
  for p in "${DASH_PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
  # a missed pid must not outlive the run and answer a later run's POST
  [ -n "${PORT:-}" ] && fuser -k -n tcp "$PORT" 2>/dev/null
  return 0
}
trap cleanup EXIT INT TERM
( cd "$R/home" && "$B" install --skip-hooks --no-starter-commands ) >/dev/null 2>&1
"$B" scaffold "$R/ws" >/dev/null 2>&1
cd "$R/ws"; "$B" learn --text "seed note before dashboard" --domain base >/dev/null 2>&1
W=$R/ws/.base/graph.nq

say "#19: outside any workspace (cwd /tmp/merlin-0141/nowhere)"
mkdir -p "$R/nowhere"; cd "$R/nowhere"
echo "[recall]"; "$B" recall --keyword seed; echo "   rc=$?"
echo "[project list]"; "$B" project list; echo "   rc=$?"
echo "[doctor tiers]"; "$B" doctor 2>&1 | grep -E "tier —|tier -" ; echo "[commands list]"; "$B" commands list 2>&1 | head -3; echo "   rc=$?"
cd "$R/ws"

say "#20: a broken hook is silent: corrupt the workspace graph, run the prompt hook"
cp "$W" "$W.good"; printf 'this is not an nquad line\n' >> "$W"
out=$(printf '{"session_id":"s20","cwd":"%s","prompt":"hello base"}' "$R/ws" | "$B" hook user-prompt-submit 2>"$R/hook20.err"); rc=$?
echo "   rc=$rc stdout_bytes=${#out} stderr: $(head -c 200 "$R/hook20.err" | tr '\n' ' ')"
echo "   last hook-events line: $(tail -n 1 "$R/ws/.base/hook-events.jsonl" 2>/dev/null | cut -c1-160)"
echo "   session-start says:"; printf '{"session_id":"s20b","cwd":"%s"}' "$R/ws" | "$B" hook session-start 2>&1 | grep -i -E "unhealthy|corrupt|hook|error" | head -4 | sed 's/^/     /'
cp "$W.good" "$W"

say "#22: the hook writer never trims: seed an 11 MB hook-events.jsonl, run one hook"
python3 - "$R/ws/.base/hook-events.jsonl" <<'PY'
import sys; p=sys.argv[1]
line='{"ts":"2026-09-07T00:00:00-05:00","hook":"user-prompt-submit","success":true,"cwd":"x","domains_matched":[],"rules_injected":0}\n'
with open(p,'w') as f:
    for _ in range(11*1024*1024//len(line)+1): f.write(line)
PY
b0=$(stat -c %s "$R/ws/.base/hook-events.jsonl")
printf '{"session_id":"s22","cwd":"%s","prompt":"hello"}' "$R/ws" | "$B" hook user-prompt-submit >/dev/null 2>&1
b1=$(stat -c %s "$R/ws/.base/hook-events.jsonl")
echo "   before $b0 bytes, after one hook $b1 bytes (cap is 10 MiB = 10485760; trimmed only by the dashboard at start)"

say "#41: dashboard snapshot overwrites a CLI write"
: > "$R/ws/.base/hook-events.jsonl"
ROUTE=$(git -C ~/.cache/base-release-clone show v0.14.0:src/dashboard/server.rs | grep 'add_rule' | grep -o '"/api/[^"]*"' | head -1 | tr -d '"')
echo "   add_rule route: ${ROUTE:-NOT FOUND}"
PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
( cd "$R/ws" && exec "$B" dashboard --port "$PORT" > "$R/dash.log" 2>&1 ) & echo $! > "$R/dash.pid"
DASH_PIDS+=("$(cat "$R/dash.pid")")
sleep 4
# Refuse to measure a server this run did not start (a stale dashboard on a fixed
# port answered these POSTs for hours and reported 0.14.0 behaviour for every binary).
if grep -qi "address already in use" "$R/dash.log"; then
  echo "   ABORT: port $PORT was already bound; this leg would have measured another process"
  sed 's/^/     /' "$R/dash.log" | head -3
  kill "$(cat "$R/dash.pid")" 2>/dev/null
  exit 3
fi
if ! kill -0 "$(cat "$R/dash.pid")" 2>/dev/null; then
  echo "   ABORT: the dashboard this run started is not alive"
  sed 's/^/     /' "$R/dash.log" | head -5
  exit 3
fi
echo "   dashboard pid $(cat "$R/dash.pid") on port $PORT, binary $(md5sum "$B" | cut -c1-12)"
"$B" learn --text "cli write after dashboard start" --domain base >/dev/null 2>&1; echo "   cli learn rc=$?  on disk now: $(grep -c 'cli write after dashboard start' "$W")"
code=$(curl -s -o "$R/curl.out" -w '%{http_code}' -X POST "http://127.0.0.1:$PORT$ROUTE" -H 'content-type: application/json' -d '{"domain":"base","text":"dashboard rule after cli write"}')
echo "   dashboard POST $ROUTE -> $code $(head -c 120 "$R/curl.out")"
sleep 1
echo "   cli note still on disk: $(grep -c 'cli write after dashboard start' "$W")   dashboard rule on disk: $(grep -c 'dashboard rule after cli write' "$W")"
kill "$(cat "$R/dash.pid")" 2>/dev/null; sleep 1

say "#40: an existing map over the fuse is refreshed anyway by the Stop hook"
A=$R/home/bigapp; mkdir -p "$A/.git" "$A/.base-ast" "$A/docs"; for i in $(seq 1 5200); do : > "$A/docs/f$i.md"; done
printf '@prefix ops: <http://ops-sys.local/ontology#> .\n' > "$A/.base-ast/ast.ttl"
cd "$A"; printf '{"session_id":"s40","cwd":"%s"}' "$A" | "$B" hook stop > "$R/stop40.out" 2>"$R/stop40.err"; echo "   stop rc=$? stdout: $(head -c 100 "$R/stop40.out")"
sleep 1; echo "   .base-ast after stop: $(ls -a "$A/.base-ast" | tr '\n' ' ')"; echo "   spawned sync: $(pgrep -af 'sync --ast' | grep -c bigapp)"
pkill -f "sync --ast --yes --target $A" 2>/dev/null; sleep 1
rm -f "$A/.base-ast/ast.ttl" "$A/.base-ast/.last-sync" "$A/.base-ast/.building"
printf '{"session_id":"s40b","cwd":"%s"}' "$A" | "$B" hook stop > "$R/stop40b.out" 2>&1; sleep 1
echo "   FIRST build on the same tree: .base-ast = $(ls -a "$A/.base-ast" | tr '\n' ' ')  spawned: $(pgrep -af 'sync --ast' | grep -c bigapp)"
pkill -f "sync --ast" 2>/dev/null
echo; echo "done"
