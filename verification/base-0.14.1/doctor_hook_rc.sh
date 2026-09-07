#!/bin/bash
# auk's #20 ruling as exit codes on the real binary:
#   a hook whose LATEST event failed  -> healthy false -> doctor rc 1
#   an older failure, successes after -> warning only  -> doctor rc 0
set -u
B=${B:-/tmp/base-0141b}
R=/tmp/rc20; rm -rf "$R"; mkdir -p "$R/home/.claude" "$R/ws"
export BASE_HOME=$R/home BASE_NO_AUTO_UPDATE=1
( cd "$R/home" && "$B" install --skip-hooks --no-starter-commands ) >/dev/null 2>&1
"$B" scaffold "$R/ws" >/dev/null 2>&1
cd "$R/ws"
L=$R/ws/.base/hook-events.jsonl
echo "binary $(md5sum "$B" | cut -c1-12)"

printf '%s\n' '{"ts":"t1","hook":"user-prompt-submit","success":false,"error":"planted live fault"}' > "$L"
"$B" doctor >/dev/null 2>&1; rc=$?
[ "$rc" = "1" ] && echo "  ok    trailing failure -> rc=$rc" || echo "  FAIL  trailing failure -> rc=$rc, expected 1"

printf '%s\n%s\n' '{"ts":"t1","hook":"user-prompt-submit","success":false,"error":"old"}' '{"ts":"t2","hook":"user-prompt-submit","success":true}' > "$L"
"$B" doctor >/dev/null 2>&1; rc=$?
[ "$rc" = "0" ] && echo "  ok    old failure then success -> rc=$rc" || echo "  FAIL  history -> rc=$rc, expected 0"
"$B" doctor 2>&1 | grep -i "hooks:" | sed 's/^/        /'

printf '%s\n' '{"ts":"t1","hook":"stop","success":true}' > "$L"
"$B" doctor >/dev/null 2>&1; rc=$?
[ "$rc" = "0" ] && echo "  ok    clean trail -> rc=$rc" || echo "  FAIL  clean trail -> rc=$rc"
"$B" doctor 2>&1 | grep -ci "hooks:" | sed 's/^/        hook lines on a clean trail: /'
