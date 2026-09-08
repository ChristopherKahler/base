#!/usr/bin/env bash
# RED-FIRST RIG for verification/base-0.14.2/silent_no_ops_0142.sh — the four instrument fixes of #109.
#
# Every case runs the REAL harness twice, the PRE-FIX revision and the revision in this tree, against
# identical FIXTURES: stub `base` binaries and a stub `strings` on PATH. Neither copy is edited, so a
# mutant cannot die of a parse error. The point of each case is a DELTA between the two revisions on
# the same fixture: a fix that only ever shows green has not been shown to fix anything.
#
# Where the two revisions come from:
#   OLD  the pre-fix file, pinned by GIT BLOB ID (its content hash), fetched from this repository's
#        object store with `git cat-file`, then re-hashed so the bytes compared are provably the pinned
#        ones. Pinning the blob rather than a ref means the comparison cannot drift as main moves.
#        OLD_SH=<path> overrides the fetch with a copy kept on disk.
#   NEW  the sibling file in this tree — the revision being verified. NEW_SH=<path> overrides.
#
# Exit codes, all loud, none green by accident:
#   0  every case passed. NOTEs, if any, are printed and counted — a NOTE is never a pass.
#   1  a control leg or a case failed.
#   7  REFUSED: NEW is byte-identical to the pinned OLD, so there is no fix in this tree to measure and
#      every delta case would go red for a reason unrelated to the fix. Merge #109 first.
#
# Run:   bash verification/base-0.14.2/rig_silent_no_ops_0142.sh
# and read its exit code from OUTSIDE — a script cannot read its own. The sandbox is a private mktemp
# dir, removed on a clean run; on any FAIL or NOTE it is kept and its path printed (KEEP_SANDBOX=1
# keeps it always).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
# verification/base-0.14.2/silent_no_ops_0142.sh as of main 002b064843e72b07e30b10460038534a3fadb6c0, before #109
OLD_BLOB=226edeffeb24a80018dd33c8221fb836c9e43241
NEW_SH=${NEW_SH:-$HERE/silent_no_ops_0142.sh}
rig_pass=0; rig_fail=0; rig_note=0
rp(){ rig_pass=$((rig_pass+1)); printf '  [RIG PASS] %s\n' "$1"; }
rf(){ rig_fail=$((rig_fail+1)); printf '  [RIG FAIL] %s\n' "$1"; }
rn(){ rig_note=$((rig_note+1)); printf '  [RIG NOTE] %s\n' "$1"; }

ROOT=$(mktemp -d "${TMPDIR:-/tmp}/rig-silent-no-ops.XXXXXX") || { rf "mktemp -d failed — no sandbox, nothing can run"; exit 1; }
SB=$ROOT/sandbox
cleanup(){
  if [ "$rig_fail" -eq 0 ] && [ "$rig_note" -eq 0 ] && [ -z "${KEEP_SANDBOX:-}" ]; then rm -rf "$ROOT"
  else echo "  sandbox kept at $ROOT (sandbox/out.txt is the last harness run)"; fi
}
trap cleanup EXIT   # set ONCE and never replaced — case (d) below is about exactly that defect

# ── a fake `base`, faithful enough that every row can reach a verdict ────────
# Needed because case (c)'s whole claim is "a run with NO failures but SOME skips must not exit 0",
# and that state is unreachable unless the rows can actually pass. Two modes: `new` implements the
# fixed behaviour, `old` the 0.14.1 behaviour each row expects to find RED.
write_fake_base() { # write_fake_base <path> <mode: old|new>
  cat > "$1" <<FAKEEOF
#!/usr/bin/env bash
MODE=$2
set -u
case "\${1:-}" in
  --version) echo "base 0.14.x (fake-\$MODE)"; exit 0;;
esac
inbox_dir(){ echo "\${BASE_HOME:-\$HOME}/.base-gbl/.base/relay-inbox"; }
if [ "\${1:-}" = relay ]; then
  case "\${2:-}" in
    register) exit 0;;
    ping)
      to=""; shift 2
      while [ \$# -gt 0 ]; do case "\$1" in --to) to=\$2; shift 2;; *) shift;; esac; done
      d=\$(inbox_dir)/\$to; mkdir -p "\$d"
      if [ "\$MODE" = new ]; then
        # unique per process AND per call: the #86 fix
        f="\$d/ping-\$(date +%s%N)-\$\$-\$RANDOM.json"
      else
        # 0.14.1: millisecond-only slug, so concurrent pings overwrite each other
        f="\$d/ping-\$(date +%s%3N).json"
      fi
      printf '{"from":"probe","summary":"x"}' > "\$f"; exit 0;;
  esac
  exit 0
fi
if [ "\${1:-}" = hook ]; then
  ev=\${2:-}; payload=\$(cat)
  pcwd=\$(printf '%s' "\$payload" | sed -n 's/.*"cwd":"\([^"]*\)".*/\1/p')
  case "\$ev" in
    post-tool-use)
      if [ "\$MODE" = new ]; then
        tier="\$pcwd"                       # the payload's cwd chooses the tier (#77 fixed)
        src=payload
      else
        tier="\$PWD"                        # 0.14.1 used the process cwd
        src=process
      fi
      mkdir -p "\$tier/.base"
      printf '{"cwd_source":"%s","nudged":false,"standards_injected":false}\n' "\$src" \\
        >> "\$tier/.base/hook-events.jsonl"
      case "\$payload" in
        *mv\ * )
          if [ "\$MODE" = new ]; then
            printf '{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"path-drift detected"}}'
          else
            printf 'path-drift detected'    # 0.14.1: bare stdout, no envelope
          fi;;
      esac
      exit 0;;
    stop)
      sid=\$(printf '%s' "\$payload" | sed -n 's/.*"session_id":"\([^"]*\)".*/\1/p')
      n=\$(ls -1 \$(inbox_dir)/*/*.json 2>/dev/null | wc -l)
      if [ "\$n" -gt 0 ]; then
        if [ "\$MODE" = new ]; then
          printf '{"systemMessage":"you have %s open task(s)"}' "\$n"
        else
          printf '<relay>open task</relay>'   # 0.14.1: the bare block, #76
        fi
      fi
      exit 0;;
  esac
  exit 0
fi
exit 0
FAKEEOF
  chmod +x "$1"
}

build_sandbox() {
  rm -rf "$SB"; mkdir -p "$SB/bin" "$SB/tmp"
  write_fake_base "$SB/newbin" new
  write_fake_base "$SB/oldbin" old
  # stub `strings`: the branch carries the symbol, the control does not.
  cat > "$SB/bin/strings" <<'EOF'
#!/usr/bin/env bash
[ "${STRINGS_FAIL_CONTROL:-0}" = 1 ] && case "$1" in *oldbin) exit 7;; esac
[ "${STRINGS_FAIL_BRANCH:-0}"  = 1 ] && case "$1" in *newbin) exit 7;; esac
[ "${STRINGS_EMPTY_CONTROL:-0}" = 1 ] && case "$1" in *oldbin) exit 0;; esac
case "$1" in *newbin) echo "ping_slug"; echo "filler";; *) echo "filler";; esac
EOF
  chmod +x "$SB/bin/strings"
}

# TMPDIR is pointed into the sandbox so the pre-fix revision's leaked tempfiles (case (d)) land where
# the trap above removes them, not in the host's /tmp.
run() { # run <script> [env assignments...]  -> echoes rc, output in $SB/out.txt
  local sh=$1; shift
  ( cd "$SB" && PATH="$SB/bin:$PATH" TMPDIR="$SB/tmp" env "$@" BASE_BIN="$SB/newbin" OLD_BIN="$SB/oldbin" \
      bash "$sh" ) > "$SB/out.txt" 2>&1
  echo $?
}
counts(){ grep -oE '^  [0-9]+ PASS / [0-9]+ FAIL / [0-9]+ SKIP' "$SB/out.txt" | tail -1 | sed 's/^  //'; }

echo "############ RED-FIRST RIG — silent_no_ops_0142.sh, four fixes (#109) ############"
echo
echo "=== control leg 0: this rig must itself parse, in BOTH bash builds where two exist ==="
RIGSELF="$HERE/$(basename "$0")"
bash -n "$RIGSELF" && rp "rig parses under $(bash --version | head -1 | cut -d' ' -f1-4)" || { rf "rig does NOT parse"; exit 1; }
case "$(uname -o 2>/dev/null)" in
  Msys|Cygwin)
    if command -v wsl.exe >/dev/null 2>&1; then
      # MSYS_NO_PATHCONV=1 is REQUIRED, not stylistic: without it Git Bash rewrites the /mnt/c/... argument
      # into C:/Program Files/Git/mnt/c/... before wsl.exe sees it, and bash -n reports on a file it never opened.
      if MSYS_NO_PATHCONV=1 wsl.exe -e bash -n "/mnt$RIGSELF" 2>"$ROOT/wsl-parse.err"; then
        rp "rig parses under WSL bash too — the second build agrees"
      else
        rf "rig does NOT parse under WSL bash: $(head -1 "$ROOT/wsl-parse.err" | tr -d '\r')"
      fi
    else
      rn "Windows host without wsl.exe — the second-build parse check could NOT run; UNPROVEN-HERE, not a pass"
    fi;;
  *)
    rn "one bash build on this host ($(uname -o 2>/dev/null)) — the second-build parse check is a Windows+WSL leg and did not run here; not a pass for that leg";;
esac

echo
echo "=== control leg 1: BOTH revisions of the harness must exist and parse ==="
if [ -n "${OLD_SH:-}" ]; then
  [ -f "$OLD_SH" ] || { rf "OLD_SH override names a file that does not exist: $OLD_SH"; exit 1; }
  rp "OLD taken from the OLD_SH override at $OLD_SH (the pinned-blob fetch was bypassed)"
else
  OLD_SH=$ROOT/old.sh
  TOP=$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null) \
    || { rf "this file is not inside a git checkout, so the pinned pre-fix blob cannot be fetched; set OLD_SH=<path> to a copy"; exit 1; }
  if ! git -C "$TOP" cat-file blob "$OLD_BLOB" > "$OLD_SH" 2>"$ROOT/cat-file.err"; then
    rf "git cat-file could not fetch the pinned pre-fix blob $OLD_BLOB from $TOP — $(head -1 "$ROOT/cat-file.err" | tr -d '\r'). A shallow clone that never fetched pre-#109 main lacks the object: git fetch --depth=1 origin main (the file is unchanged there) or the pinned commit 002b064843e72b07e30b10460038534a3fadb6c0. Nothing below can compare revisions."
    exit 1
  fi
  got=$(git hash-object --no-filters "$OLD_SH")
  if [ -s "$OLD_SH" ] && [ "$got" = "$OLD_BLOB" ]; then
    rp "OLD fetched from this repo's object store and re-hashed: blob $got"
  else
    rf "the fetched OLD re-hashes to '$got', not the pinned $OLD_BLOB — refusing to compare against bytes that are not the pinned revision"; exit 1
  fi
fi
for pair in "old:$OLD_SH" "new:$NEW_SH"; do
  lbl=${pair%%:*}; f=${pair#*:}
  [ -f "$f" ] || { rf "$lbl copy missing at $f — nothing below can compare revisions"; exit 1; }
  bash -n "$f" && rp "$lbl copy parses ($(wc -l < "$f") lines, md5 $(md5sum "$f" | cut -d' ' -f1))" \
                || { rf "$lbl copy does NOT parse — nothing below can run it"; exit 1; }
done

echo
echo "=== control leg 2: NEW must differ from the pinned OLD, or there is no fix here to measure ==="
# CRs are stripped before hashing so an un-renormalised Windows checkout (see .gitattributes) is still
# recognised as the same revision rather than measured as a "delta" of line endings.
new_id=$(tr -d '\r' < "$NEW_SH" | git hash-object --stdin)
old_id=$(tr -d '\r' < "$OLD_SH" | git hash-object --stdin)
if [ "$new_id" = "$old_id" ]; then
  rf "NEW ($NEW_SH) is byte-identical to OLD (blob $old_id) — the fix under test is NOT in this tree (is #109 merged?). Every delta case below would go red for a reason unrelated to the fix. REFUSING, rc 7."
  exit 7
fi
rp "NEW blob $new_id differs from OLD blob $old_id — there is a delta to measure"

echo
echo "=== (a) THE ONE-SIDED SWALLOW: strings fails on the CONTROL only ==="
# The headline. An empty control dump makes the control's symbol check find nothing, which READS AS
# SUCCESS - and the old script then printed "both binaries are what they claim". A control that was
# never read passed the control check.
build_sandbox
o_rc=$(run "$OLD_SH" STRINGS_FAIL_CONTROL=1); o_claim=$(grep -c "both binaries are what they claim" "$SB/out.txt")
build_sandbox
n_rc=$(run "$NEW_SH" STRINGS_FAIL_CONTROL=1); n_msg=$(grep -o "ABORT: strings failed on the control binary" "$SB/out.txt" | head -1)
if [ "$o_claim" -ge 1 ] && [ "$n_rc" = 3 ] && [ -n "$n_msg" ]; then
  rp "(a) old: control dump EMPTY yet it printed 'both binaries are what they claim' (rc=$o_rc) -> new: rc=3, '$n_msg'"
else
  rf "(a) old rc=$o_rc claim-lines=$o_claim ; new rc=$n_rc msg='$n_msg' (wanted old to claim, new rc=3)"
fi

echo
echo "=== (a2) the branch side: strings fails on the BRANCH ==="
# This side already failed CLOSED, but with a WRONG diagnosis - it blamed the binary. Both revisions
# abort; the fix is that the new one names the TOOL rather than accusing the build.
build_sandbox
o_rc=$(run "$OLD_SH" STRINGS_FAIL_BRANCH=1); o_txt=$(grep -o "it is not this branch's build" "$SB/out.txt" | head -1)
build_sandbox
n_rc=$(run "$NEW_SH" STRINGS_FAIL_BRANCH=1); n_txt=$(grep -o "strings failed on the branch binary" "$SB/out.txt" | head -1)
if [ "$o_rc" = 2 ] && [ -n "$o_txt" ] && [ "$n_rc" = 3 ] && [ -n "$n_txt" ]; then
  rp "(a2) old rc=2 blamed the BINARY (\"$o_txt\") -> new rc=3 names the TOOL (\"$n_txt\")"
else
  rf "(a2) old rc=$o_rc '$o_txt' ; new rc=$n_rc '$n_txt'"
fi

echo
echo "=== (a3) an EMPTY control dump with strings exiting 0 ==="
# The nastier shape: the tool succeeds and produces nothing. `|| true` never even fires.
build_sandbox
o_rc=$(run "$OLD_SH" STRINGS_EMPTY_CONTROL=1); o_claim=$(grep -c "both binaries are what they claim" "$SB/out.txt")
build_sandbox
n_rc=$(run "$NEW_SH" STRINGS_EMPTY_CONTROL=1); n_txt=$(grep -o "EMPTY dump for the control binary" "$SB/out.txt" | head -1)
if [ "$o_claim" -ge 1 ] && [ "$n_rc" = 3 ] && [ -n "$n_txt" ]; then
  rp "(a3) old: rc=0 dump, empty, still claimed both binaries verified -> new: rc=3, '$n_txt'"
else
  rf "(a3) old rc=$o_rc claim=$o_claim ; new rc=$n_rc txt='$n_txt'"
fi

echo
echo "=== (b) strings ABSENT from PATH entirely ==="
# The harness must name the missing TOOL, not blame the binary. This case needs a PATH that carries
# everything the harness uses and no `strings` at all — and it must PROVE that before it runs, because
# on a host with binutils installed a "restricted" PATH that still resolves strings measures nothing.
case_b_path() { # case_b_path <tool-to-omit>  -> a PATH carrying everything the harness uses except $1
  case "$(uname -o 2>/dev/null)" in
    Msys|Cygwin) printf '%s' "$SB/bin:/usr/bin:/bin";;    # Git Bash ships neither binutils nor python in /usr/bin
    *) # every entry of every PATH dir, symlinked into one toolbox dir, minus the named tool
       rm -rf "$ROOT/tb"; mkdir -p "$ROOT/tb"
       local IFS=: d
       for d in $PATH; do [ -d "$d" ] && cp -ns "$d"/* "$ROOT/tb/" 2>/dev/null; done
       rm -f "$ROOT/tb/$1"
       printf '%s' "$SB/bin:$ROOT/tb";;
  esac
}
build_sandbox; rm -f "$SB/bin/strings"
CASE_PATH=$(case_b_path strings)
if ( export PATH="$CASE_PATH"; command -v strings >/dev/null 2>&1 ); then
  rn "(b) UNPROVEN-HERE: 'strings' still resolves under the case PATH at $( export PATH="$CASE_PATH"; command -v strings ) — the absent-tool leg could not be set up on this host. NOT a pass."
elif ! ( export PATH="$CASE_PATH"; command -v bash >/dev/null 2>&1 && command -v md5sum >/dev/null 2>&1 ); then
  rn "(b) UNPROVEN-HERE: the case PATH lost bash or md5sum, so the harness could not even start. NOT a pass."
else
  o_rc=$( ( cd "$SB" && PATH="$CASE_PATH" TMPDIR="$SB/tmp" BASE_BIN="$SB/newbin" OLD_BIN="$SB/oldbin" bash "$OLD_SH" ) > "$SB/out.txt" 2>&1; echo $? )
  o_txt=$(grep -o "it is not this branch's build" "$SB/out.txt" | head -1)
  build_sandbox; rm -f "$SB/bin/strings"
  n_rc=$( ( cd "$SB" && PATH="$CASE_PATH" TMPDIR="$SB/tmp" BASE_BIN="$SB/newbin" OLD_BIN="$SB/oldbin" bash "$NEW_SH" ) > "$SB/out.txt" 2>&1; echo $? )
  n_txt=$(grep -o "'strings' is not on PATH" "$SB/out.txt" | head -1)
  if [ -n "$n_txt" ] && [ "$n_rc" = 3 ]; then
    rp "(b) strings absent -> new rc=3 names the TOOL (\"$n_txt\"); old rc=$o_rc said \"${o_txt:-<nothing about the tool>}\""
  else
    rf "(b) new rc=$n_rc txt='$n_txt' (wanted rc=3 naming the missing tool)"
    grep -E 'ABORT' "$SB/out.txt" | head -3 | sed 's/^/       /'
  fi
fi

echo
echo "=== (b2) python3 ABSENT from PATH: the same misdiagnosis two rows further down (shrike's #109 review) ==="
# The #75 and #76 rows pipe into `python3 -c … 2>/dev/null` inside an elif. With python3 missing the
# condition is silently false, the control falls through, and the row reports FAIL against the PRODUCT —
# (a2)'s tool-absence misdiagnosis, left standing below the strings guard. Pre-fix: rc 1 blaming the
# binary's stdout. Fixed: `need_tool python3` aborts rc 3 naming the tool, before any row runs.
build_sandbox
CASE_PATH=$(case_b_path python3)
if ( export PATH="$CASE_PATH"; command -v python3 >/dev/null 2>&1 ); then
  rn "(b2) UNPROVEN-HERE: 'python3' still resolves under the case PATH at $( export PATH="$CASE_PATH"; command -v python3 ) — the absent-tool leg could not be set up on this host. NOT a pass."
elif ! ( export PATH="$CASE_PATH"; command -v bash >/dev/null 2>&1 && command -v strings >/dev/null 2>&1 && command -v md5sum >/dev/null 2>&1 ); then
  rn "(b2) UNPROVEN-HERE: the case PATH lost bash, the strings stub or md5sum, so the harness could not reach the python3 rows. NOT a pass."
else
  o_rc=$( ( cd "$SB" && PATH="$CASE_PATH" TMPDIR="$SB/tmp" BASE_BIN="$SB/newbin" OLD_BIN="$SB/oldbin" bash "$OLD_SH" ) > "$SB/out.txt" 2>&1; echo $? )
  o_txt=$(grep -o "FAIL  new: stdout is not the envelope" "$SB/out.txt" | head -1)
  build_sandbox
  n_rc=$( ( cd "$SB" && PATH="$CASE_PATH" TMPDIR="$SB/tmp" BASE_BIN="$SB/newbin" OLD_BIN="$SB/oldbin" bash "$NEW_SH" ) > "$SB/out.txt" 2>&1; echo $? )
  n_txt=$(grep -o "'python3' is not on PATH" "$SB/out.txt" | head -1)
  if [ "$o_rc" = 1 ] && [ -n "$o_txt" ] && [ "$n_rc" = 3 ] && [ -n "$n_txt" ]; then
    rp "(b2) python3 absent -> old rc=1 blamed the PRODUCT (\"$o_txt\"); new rc=3 names the TOOL (\"$n_txt\") before any row ran"
  else
    rf "(b2) old rc=$o_rc '$o_txt' ; new rc=$n_rc '$n_txt' (wanted old rc=1 blaming stdout, new rc=3 naming python3)"
    grep -E 'ABORT|FAIL' "$SB/out.txt" | head -4 | sed 's/^/       /'
  fi
fi

echo
echo "=== (c) A RUN WITH ZERO FAILURES AND SOME SKIPS MUST NOT EXIT 0 ==="
# The fix that changes the default outcome, so it gets a real demonstration rather than an argument.
# REPO_DIR is unset, so the #95 row skips; the fake base makes every other row reach a verdict.
build_sandbox
o_rc=$(run "$OLD_SH"); o_cnt=$(counts)
build_sandbox
n_rc=$(run "$NEW_SH"); n_cnt=$(counts)
o_fail=$(echo "$o_cnt" | sed -n 's/.*\/ \([0-9]*\) FAIL.*/\1/p'); o_skip=$(echo "$o_cnt" | sed -n 's/.*\/ \([0-9]*\) SKIP/\1/p')
if [ "${o_fail:-1}" != 0 ]; then
  rn "(c) UNPROVEN-HERE: the fake base could not produce a zero-FAIL run (old counted '$o_cnt'), so the SKIP-only state was not reached. This is NOT a pass — see the FAIL rows in $SB/out.txt."
elif [ "$o_rc" = 0 ] && [ "$n_rc" = 5 ]; then
  rp "(c) identical fixtures, $o_cnt: OLD exits 0 (skips invisible) -> NEW exits 5 INCOMPLETE. A SKIP now reaches the exit code."
else
  rf "(c) old rc=$o_rc '$o_cnt' ; new rc=$n_rc '$n_cnt' (wanted old 0, new 5 with 0 FAIL and >0 SKIP)"
  grep -E '^  (FAIL|SKIP)' "$SB/out.txt" | head -8 | sed 's/^/       /'
fi

echo
echo "=== (d) THE TRAP LEAK: two 'trap ... EXIT' do not accumulate ==="
# The second trap REPLACED the first, so the two mktemp symbol dumps were never removed.
#
# TWO THINGS THIS CASE GOT WRONG ON ITS FIRST RUN, both fixed here, because they are the difference
# between measuring the leak and measuring nothing:
#   1. It used an ABORT path (STRINGS_FAIL_BRANCH). That returns at the symbol check, BEFORE the
#      line where the second trap replaces the first — so the original trap is still installed and
#      does clean up. The old revision leaked nothing and the case reported UNPROVEN-HERE. The leak
#      only exists once execution passes the replacing trap, so this must be a FULL run.
#   2. It counted a SHARED tempdir, where every other process's files are noise. `mktemp` honours
#      TMPDIR, so each run now gets a PRIVATE one and the count is exact rather than approximate.
leak_count() { # leak_count <script>  -> files left behind in a private TMPDIR after a full run
  build_sandbox
  local td="$SB/tmpdir"; rm -rf "$td"; mkdir -p "$td"
  ( cd "$SB" && TMPDIR="$td" PATH="$SB/bin:$PATH" BASE_BIN="$SB/newbin" OLD_BIN="$SB/oldbin" \
      bash "$1" ) > "$SB/out.txt" 2>&1
  ls -1A "$td" 2>/dev/null | wc -l
}
o_leak=$(leak_count "$OLD_SH")
n_leak=$(leak_count "$NEW_SH")
if [ "$o_leak" -gt 0 ] && [ "$n_leak" -le 0 ]; then
  rp "(d) after a FULL run in a private TMPDIR: old left $o_leak file(s) behind, new left $n_leak — one trap, set once, cleans everything it owns"
elif [ "$o_leak" -le 0 ]; then
  rn "(d) UNPROVEN-HERE: the old revision leaked $o_leak files this run, so the leak was not reproduced and there is nothing to have fixed. NOT a pass."
else
  rf "(d) old leaked $o_leak, new leaked $n_leak — the new revision still leaks"
fi

echo
echo "=== RESTORE — the sandbox rebuilds clean ==="
build_sandbox
r_rc=$(run "$NEW_SH" STRINGS_FAIL_CONTROL=1)
[ "$r_rc" = 3 ] && rp "restored: the (a) case still reproduces at rc=3" || rf "restored badly: rc=$r_rc"

echo
echo "############ RIG RESULT: $rig_pass PASS / $rig_fail FAIL / $rig_note NOTE ############"
[ "$rig_fail" -eq 0 ] || exit 1
exit 0
