#!/usr/bin/env bash
# base 0.14.2 — acceptance for the silent no-ops fork (#75 #76 #77 #86 #95).
#
# Red on the 0.14.1 asset, green on the branch binary, every row from its own command.
#
# usage: BASE_BIN=<branch binary> OLD_BIN=<0.14.1 asset> bash silent_no_ops_0142.sh
#
# PROVENANCE (law 15). A shared CARGO_TARGET_DIR can hand a builder the other worktree's
# binary at exit 0, and a debug build timed against a release one reads as a regression
# that is not there. So this script prints the md5 and a branch-only symbol of BOTH
# binaries and ABORTS if they are the same file or if the branch binary lacks the symbol.
# A row measured against the wrong binary is not evidence in either direction.
set -uo pipefail

BASE_BIN="${BASE_BIN:?set BASE_BIN to the branch binary}"
OLD_BIN="${OLD_BIN:?set OLD_BIN to the 0.14.1 asset}"
SYMBOL="${EXPECT_SYMBOL:-ping_slug}"

pass=0; fail=0; skip=0
ok()      { pass=$((pass+1)); printf '  PASS  %s\n' "$1"; }
bad()     { fail=$((fail+1)); printf '  FAIL  %s\n' "$1"; }
skipped() { skip=$((skip+1)); printf '  SKIP  %s\n' "$1"; }

# A tool that is ABSENT produces the same empty output as a binary that genuinely lacks the
# symbol. Assert the tool first, or the diagnosis below blames the binary for the toolchain.
need_tool() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "ABORT: '$1' is not on PATH. Its empty output is indistinguishable from a binary"
        echo "       that lacks the symbol, so every provenance check below would be void."
        exit 3
    }
}
need_tool strings; need_tool md5sum

# ONE trap, set ONCE, covering everything it must clean. `trap ... EXIT` does not accumulate:
# a second one REPLACES the first. This script had two — the tempfile trap here and a WORK trap
# further down — so the two symbol dumps leaked on every single run.
WORK=$(mktemp -d)
WORK_SYMS_NEW=$(mktemp); WORK_SYMS_OLD=$(mktemp)
trap 'rm -rf "$WORK"; rm -f "$WORK_SYMS_NEW" "$WORK_SYMS_OLD"' EXIT
echo "═══ provenance ═══"
for b in "$BASE_BIN" "$OLD_BIN"; do
    [ -x "$b" ] || { echo "ABORT: not executable: $b"; exit 2; }
done
MD5_NEW=$(md5sum "$BASE_BIN" | cut -d' ' -f1)
MD5_OLD=$(md5sum "$OLD_BIN" | cut -d' ' -f1)
printf '  branch   %s  md5 %s  %s\n' "$BASE_BIN" "$MD5_NEW" "$("$BASE_BIN" --version 2>&1 | head -1)"
printf '  control  %s  md5 %s  %s\n' "$OLD_BIN"  "$MD5_OLD" "$("$OLD_BIN"  --version 2>&1 | head -1)"
if [ "$MD5_NEW" = "$MD5_OLD" ]; then
    echo "ABORT: the two binaries are the same file — every row below would be void."; exit 2
fi
# NO PIPE. `strings BIN | grep -q SYM` under `set -o pipefail` reports the pipeline as
# FAILED when it MATCHES: grep -q exits at the first hit, strings takes SIGPIPE, and
# pipefail publishes that. This check therefore inverted and refused a correct binary
# whose symbol the build had just counted twice. Dump once, grep the file.
# The `|| true` that used to sit on both of these lines swallowed the failure, and the swallow
# was ONE-SIDED in effect. On the BRANCH an empty dump makes the check below find no symbol and
# ABORT — closed. On the CONTROL an empty dump makes its check find no symbol and PASS, and the
# script then prints "both binaries are what they claim". A CONTROL THAT WAS NEVER READ PASSED
# THE CONTROL CHECK. Same shape as a missing baseline reading as zero differing files.
dump_syms() { # dump_syms <binary> <outfile> <which>
    if ! strings "$1" > "$2" 2>/dev/null; then
        echo "ABORT: strings failed on the $3 binary ($1); its symbol dump is not evidence."; exit 3
    fi
    # An empty dump is not a result. Asserted on BOTH arms, because the control is the arm
    # where emptiness reads as success.
    [ -s "$2" ] || {
        echo "ABORT: strings produced an EMPTY dump for the $3 binary ($1)."
        echo "       Every symbol check below would be meaningless, and the CONTROL check"
        echo "       would PASS on it — emptiness is indistinguishable from absence there."
        exit 3
    }
}
dump_syms "$BASE_BIN" "$WORK_SYMS_NEW" branch
dump_syms "$OLD_BIN"  "$WORK_SYMS_OLD" control
if ! grep -q "$SYMBOL" "$WORK_SYMS_NEW"; then
    echo "ABORT: branch binary has no '$SYMBOL' — it is not this branch's build."; exit 2
fi
if grep -q "$SYMBOL" "$WORK_SYMS_OLD"; then
    echo "ABORT: the CONTROL carries '$SYMBOL' — it is not 0.14.1."; exit 2
fi
echo "  symbol '$SYMBOL': present in branch, absent in control — both binaries are what they claim"

# WORK and its cleanup are established with the single trap at the top of this script.
HOME_DIR="$WORK/home"; mkdir -p "$HOME_DIR/.base-gbl/.base"

hook() { # hook <bin> <event> <cwd> <payload>
    printf '%s' "$4" | BASE_HOME="$HOME_DIR" BASE_NO_AUTO_UPDATE=1 \
        env -C "$3" "$1" hook "$2" 2>/dev/null
}

# ── #75: PostToolUse context travels in the envelope ─────────────────────
echo
echo "═══ #75  PostToolUse context reaches the model ═══"
for gen in old new; do
    [ "$gen" = old ] && BIN="$OLD_BIN" || BIN="$BASE_BIN"
    WS="$WORK/ws75-$gen"; mkdir -p "$WS/.base"
    OLD_D="$WORK/alpha-old-$gen"; NEW_D="$WORK/alpha-new-$gen"; mkdir -p "$OLD_D" "$NEW_D"
    G='<http://ops-sys.local/ontology#graph/ws/test>'; P='<http://ops-sys.local/ontology#project/alpha>'
    { echo "$P <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ops-sys.local/ontology#Project> $G ."
      echo "$P <http://ops-sys.local/ontology#name> \"Alpha\" $G ."
      echo "$P <http://ops-sys.local/ontology#status> \"active\" $G ."
      echo "$P <http://ops-sys.local/ontology#path> \"$OLD_D\" $G ."; } > "$WS/.base/graph.nq"
    out=$(hook "$BIN" post-tool-use "$WS" "{\"session_id\":\"a75\",\"cwd\":\"$WS\",\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"mv $OLD_D $NEW_D\"}}")
    if [ -z "$(printf '%s' "$out" | tr -d '[:space:]')" ]; then
        skipped "$gen: the move nudge did not fire — row measured nothing"
    elif printf '%s' "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if d.get("hookSpecificOutput",{}).get("hookEventName")=="PostToolUse" and "path-drift" in d["hookSpecificOutput"].get("additionalContext","") else 1)' 2>/dev/null; then
        [ "$gen" = new ] && ok "new: envelope with hookEventName=PostToolUse carrying the path-drift block" \
                         || bad "old: 0.14.1 must NOT already emit the envelope — control is not red"
    else
        [ "$gen" = old ] && ok "old: bare stdout, no envelope (RED as expected)" \
                         || bad "new: stdout is not the envelope: $(printf '%s' "$out" | head -c 200)"
    fi
done

# ── #76: Stop never writes a bare block ─────────────────────────────────
echo
echo "═══ #76  Stop writes JSON or nothing, never a bare block ═══"
WS="$WORK/ws76"; mkdir -p "$WS/.base"
SID76=00000000-0000-0000-0000-0000000000a7

# Row 1: the empty path. Silence is correct here and proves the dead print is gone.
out=$(hook "$BASE_BIN" stop "$WS" "{\"session_id\":\"$SID76\",\"cwd\":\"$WS\",\"hook_event_name\":\"Stop\"}")
t=$(printf '%s' "$out" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')
[ -z "$t" ] && ok "new: silent with no open task" \
            || bad "new: expected silence with no open task, got: $(printf '%s' "$t" | head -c 200)"

# Row 2: the path that matters. Build a REAL open task with the product itself, then run
# stop again. Without this the section only ever exercises the branch that does nothing —
# a leg that cannot fail is vacuous, and this one gates the whole (D)+(C) claim.
( cd "$WS" && BASE_HOME="$HOME_DIR" CLAUDE_CODE_SESSION_ID="$SID76" BASE_NO_AUTO_UPDATE=1 \
    "$BASE_BIN" relay register --as t76 >/dev/null 2>&1 )
( cd "$WS" && BASE_HOME="$HOME_DIR" CLAUDE_CODE_SESSION_ID="$SID76" BASE_NO_AUTO_UPDATE=1 \
    "$BASE_BIN" relay ping --to t76 --from probe --msg "open task for the stop row" >/dev/null 2>&1 )
pending=$(ls -1 "$HOME_DIR/.base-gbl/.base/relay-inbox/t76"/*.json 2>/dev/null | wc -l)

if [ "$pending" -eq 0 ]; then
    bad "new: could not create an open task, so the systemMessage row measured NOTHING"
else
    out=$(hook "$BASE_BIN" stop "$WS" "{\"session_id\":\"$SID76\",\"cwd\":\"$WS\",\"hook_event_name\":\"Stop\"}")
    t=$(printf '%s' "$out" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')
    if [ -z "$t" ]; then
        bad "new: $pending open task(s) and Stop said NOTHING — the nudge is lost, not delivered"
    elif printf '%s' "$t" | cut -c1 | grep -q '<'; then
        bad "new: bare relay block on stdout with an open task — this is #76 exactly: $(printf '%s' "$t" | head -c 120)"
    elif printf '%s' "$t" > "$WORK/stop.json" && python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if isinstance(d.get("systemMessage"),str) and d["systemMessage"] else 1)' "$WORK/stop.json" 2>/dev/null; then
        ok "new: $pending open task(s) -> systemMessage envelope, no bare block"
    else
        bad "new: stdout with an open task is not a systemMessage envelope: $(printf '%s' "$t" | head -c 200)"
    fi
fi

# ── #77: the log lands in the payload's tier, on both arms ──────────────
echo
echo "═══ #77  the event line lands in the workspace the payload named ═══"
for gen in old new; do
    [ "$gen" = old ] && BIN="$OLD_BIN" || BIN="$BASE_BIN"
    A="$WORK/wsA-$gen"; B="$WORK/wsB-$gen"; mkdir -p "$A/.base" "$B/.base"
    hook "$BIN" post-tool-use "$B" "{\"session_id\":\"a77\",\"cwd\":\"$A\",\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"echo x\"}}" >/dev/null
    na=$(wc -l < "$A/.base/hook-events.jsonl" 2>/dev/null || echo 0)
    nb=$(wc -l < "$B/.base/hook-events.jsonl" 2>/dev/null || echo 0)
    if [ "$gen" = new ]; then
        [ "$na" -eq 1 ] && [ "$nb" -eq 0 ] && ok "new: line in A (payload cwd), none in B (process cwd)" \
                                           || bad "new: A=$na B=$nb, want A=1 B=0"
        grep -q '"cwd_source":"payload"' "$A/.base/hook-events.jsonl" 2>/dev/null \
            && ok "new: cwd_source=payload names the input that chose the tier" \
            || bad "new: cwd_source missing or not 'payload'"
        for k in nudged standards_injected; do
            grep -q "\"$k\":" "$A/.base/hook-events.jsonl" 2>/dev/null \
                && ok "new: $k reaches the log" || bad "new: $k still absent from the log"
        done
    else
        [ "$nb" -eq 1 ] && [ "$na" -eq 0 ] && ok "old: line lands in B, the process cwd (RED as expected)" \
                                           || bad "old: A=$na B=$nb — control is not red, so the new row proves less"
    fi
done

# ── #86: pings in one millisecond keep their own slugs ───────────────────
echo
echo "═══ #86  concurrent pings all survive ═══"
N=8
for gen in old new; do
    [ "$gen" = old ] && BIN="$OLD_BIN" || BIN="$BASE_BIN"
    H="$WORK/pinghome-$gen"; mkdir -p "$H"
    WS="$WORK/pingws-$gen"; mkdir -p "$WS"
    ( cd "$WS" && BASE_HOME="$H" CLAUDE_CODE_SESSION_ID=00000000-0000-0000-0000-00000000acc1 "$BIN" relay register --as acc1 >/dev/null 2>&1 )
    for i in $(seq 1 $N); do
        ( cd "$WS" && BASE_HOME="$H" CLAUDE_CODE_SESSION_ID=00000000-0000-0000-0000-00000000acc1 "$BIN" relay ping --to acc1 --from "s$i" --msg "m$i" >/dev/null 2>&1 ) &
    done
    wait
    got=$(ls -1 "$H/.base-gbl/.base/relay-inbox/acc1"/*.json 2>/dev/null | wc -l)
    if [ "$gen" = new ]; then
        [ "$got" -eq "$N" ] && ok "new: $N pings -> $got inbox files" || bad "new: $N pings -> $got files, $((N-got)) lost"
    else
        [ "$got" -lt "$N" ] && ok "old: $N pings -> $got files, $((N-got)) silently replaced (RED as expected)" \
                            || skipped "old: $N pings -> $got files — the race did not fire this run, so the control proved nothing"
    fi
done

# ── #95: line endings are pinned ────────────────────────────────────────
echo
echo "═══ #95  .gitattributes pins every executed script ═══"
if [ -n "${REPO_DIR:-}" ] && [ -x "$REPO_DIR/scripts/test-crlf-attributes.sh" ]; then
    if ( cd "$REPO_DIR" && ./scripts/test-crlf-attributes.sh >/dev/null 2>&1 ); then
        ok "scripts/test-crlf-attributes.sh exits 0 in $REPO_DIR"
    else bad "scripts/test-crlf-attributes.sh failed in $REPO_DIR"; fi
else
    skipped "set REPO_DIR to the branch worktree to run the CRLF check here (it is a CI step in its own right)"
fi

echo
echo "═══════════════════════════════════════════"
echo "  $pass PASS / $fail FAIL / $skip SKIP"
echo "  branch $MD5_NEW   control $MD5_OLD"
# The verdict used to read the FAIL counter alone, so a run in which every control arm SKIPPED
# — the move nudge not firing, the #86 race not firing, #95 with REPO_DIR unset — exited 0 and
# read as acceptance. A row that never ran is neither a pass nor a failure, and collapsing it
# into either is how an instrument reports coverage it does not have. Three exits, three states.
if [ "$fail" -ne 0 ]; then echo "  ACCEPTANCE FAIL — $fail failing row(s)"; exit 1; fi
if [ "$skip" -ne 0 ]; then
    echo "  ACCEPTANCE INCOMPLETE — $skip skipped row(s); a SKIP is not a PASS."
    echo "  (set REPO_DIR to the branch worktree to run the #95 row here)"
    exit 5
fi
echo "  ACCEPTANCE PASS"
exit 0   # explicit: the last statement of this script is never a bare test again
