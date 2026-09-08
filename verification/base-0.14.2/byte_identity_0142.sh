#!/usr/bin/env bash
# Merge-rule line 5 for the registry fork: byte-identity over the surface this
# fork names — the handoff/fork `list` output, and the non-registry CLI.
#
# Law 23 is the whole design here: every loop prints the REAL n of things it
# compared, the derived list is printed beside the count so the n can be audited
# rather than trusted, and a leg that cannot run is a SKIP with its reason, never
# a silent pass. plover's G0 caught the opposite shape — a quoted 46 that had
# actually run 22.
#
# The expected-to-DIFFER set is declared before anything runs. It is not an
# escape hatch: R3 pins that the registry help text CHANGED (project-wide ->
# "in this tier"), so those must move. Anything outside the declared set that
# moves is a finding.
set -u

# Env-driven, no baked-in machine paths: the same script has to run against a
# Linux pair and a Windows pair. CONTROL_MD5 is required — a control that cannot
# be identified cannot anchor a comparison.
CONTROL_SRC="${CONTROL_SRC:?set CONTROL_SRC to the 0.14.1 control binary}"
# No apostrophe in this message: an unpaired ' inside ${VAR:?word} opens a quoted
# section and the whole script fails to parse 35 lines later, nowhere near here.
CONTROL_MD5="${CONTROL_MD5:?set CONTROL_MD5 to the expected md5 of that control}"
BRANCH="${BRANCH:?set BRANCH to the branch binary under test}"
# Must be a real drive-lettered path on Windows, never /tmp (F29: a /tmp fake
# root drops the drive letter and flips base's own case rule), and outside the
# operator's profile (home.rs:61 arms the isolation guard whenever BASE_HOME is
# set, in every build, and home.rs:144 panics on a write under real_home()).
ROOT="${ROOT:?set ROOT to a scratch dir outside the operator profile}"
PASS=0; FAIL=0; SKIP=0
ok()   { PASS=$((PASS+1)); printf 'PASS  %s\n' "$*"; }
bad()  { FAIL=$((FAIL+1)); printf 'FAIL  %s\n' "$*"; }
skip() { SKIP=$((SKIP+1)); printf 'SKIP  %s\n' "$*"; }

# Commands whose output this PR is SUPPOSED to change. Declared up front, and
# kept as NARROW as the truth allows: an over-broad declared set is a weakened
# gate, because anything inside it gets waved through. The first run declared
# "handoff fork" and only `handoff` moved — so `fork` was being excused for a
# change it never made, and a real unexpected move there would have passed.
EXPECTED_DIFF="handoff"
# Nested registry surfaces, same rule. Measured: of the 8 nested registry help
# surfaces only `handoff create` moved, by exactly one line —
#   "...archives any prior open handoff for the project"
#   "...archives any prior open handoff for the project in this tier"
EXPECTED_DIFF_NESTED="handoff create"

echo "=== provenance ==="
rm -rf "$ROOT"; mkdir -p "$ROOT/bin/control" "$ROOT/bin/branch" "$ROOT/out"
# BOTH copies must be named base.exe, in separate directories. clap renders
# `Usage: <argv[0]> ...` into every --help, so naming the control copy anything
# else makes all 39 help surfaces "differ" on that one line — 37 fake findings
# from the instrument's own filename. Measured, first run of this script.
CONTROL="$ROOT/bin/control/base.exe"
BRANCH_RUN="$ROOT/bin/branch/base.exe"
cp "$CONTROL_SRC" "$CONTROL" || { echo "ABORT: cannot copy the control"; exit 2; }
cp "$BRANCH" "$BRANCH_RUN"  || { echo "ABORT: cannot copy the branch binary"; exit 2; }
GOT=$(md5sum "$CONTROL" | awk '{print $1}')
echo "  control src : $CONTROL_SRC"
echo "  control copy: $CONTROL"
echo "  control md5 : $GOT (expected $CONTROL_MD5)"
[ "$GOT" = "$CONTROL_MD5" ] || { echo "ABORT: control md5 mismatch — refusing to report"; exit 2; }
echo "  branch src  : $BRANCH"
echo "  branch copy : $BRANCH_RUN"
echo "  branch md5  : $(md5sum "$BRANCH_RUN" | awk '{print $1}')"
BRANCH="$BRANCH_RUN"           # from here on, run the identically-named copy
echo "  both copies are named base.exe (argv[0] parity — see the note above)"
echo "  control lockmark: $(grep -ac 'waiting for the graph lock' "$CONTROL" 2>/dev/null || echo 0)"
echo "  branch  lockmark: $(grep -ac 'waiting for the graph lock' "$BRANCH"  2>/dev/null || echo 0)"
echo "  NOTE: both print 'base 0.14.1' for --version; md5 and lockmark are the discriminators."
echo ""

# ── (a) handoff list / fork list on ONE seeded dataset ───────────────────────
# Seed ONCE, with the CONTROL, then hand each binary its own COPY of that home.
# Seeding with one binary and listing with the other compares two datasets and
# calls the result a match.
echo "=== (a) 'handoff list' / 'fork list' — both binaries, ONE seeded dataset ==="
SEED="$ROOT/seed"
mkdir -p "$SEED/home" "$SEED/ws/.base" "$SEED/docs"
export BASE_HOME="$SEED/home"
# Forks and handoffs are the SAME entity type, and `handoff create` archives the
# prior open one for its project. Seeding both under one project therefore left
# `fork list` printing "No forks." — 10 bytes of empty-state, compared against 10
# bytes of empty-state, reported as a pass. Separate projects so both lists carry
# real rows and the comparison has something to compare.
for i in 1 2 3; do
  printf '# fork doc %s\n' "$i" > "$SEED/docs/f-$i.md"
  printf '# handoff doc %s\n' "$i" > "$SEED/docs/h-$i.md"
  ( cd "$SEED/ws" && "$CONTROL" fork    create --project byteid-f --doc "$SEED/docs/f-$i.md" ) >/dev/null 2>&1
  ( cd "$SEED/ws" && "$CONTROL" handoff create --project byteid-h --doc "$SEED/docs/h-$i.md" ) >/dev/null 2>&1
done
# One archived row on each side, so the list exercises both status values.
( cd "$SEED/ws" && "$CONTROL" fork    archive f-1 ) >/dev/null 2>&1
( cd "$SEED/ws" && "$CONTROL" handoff archive h-1 ) >/dev/null 2>&1
seeded=$(grep -c "handoff/[fh]-" "$SEED/ws/.base/graph.nq" 2>/dev/null || echo 0)
echo "  seeded graph rows matching handoff/[fh]-: $seeded"
if [ "${seeded:-0}" -lt 1 ]; then
  skip "(a) the seed produced no rows — list comparison would compare two empty outputs"
else
  rm -rf "$ROOT/a-ctl" "$ROOT/a-brn"
  cp -r "$SEED" "$ROOT/a-ctl"; cp -r "$SEED" "$ROOT/a-brn"
  # prove the two inputs really are identical before comparing outputs
  ic=$(md5sum "$ROOT/a-ctl/ws/.base/graph.nq" | awk '{print $1}')
  ib=$(md5sum "$ROOT/a-brn/ws/.base/graph.nq" | awk '{print $1}')
  echo "  input graph md5: control-copy $ic  branch-copy $ib"
  if [ "$ic" != "$ib" ]; then
    bad "(a) the two input graphs differ — any output comparison would be meaningless"
  else
    # `handoff list` and `fork list` take NO flags (checked: `list --help` shows
    # only -h), so the -g/--all legs of the first run compared two rc=2 usage
    # errors and counted them as list-output evidence. They were CLI-surface
    # parity wearing the wrong label, and (b) already covers that properly.
    n=0
    for cmd in "handoff list" "fork list"; do
      tag=$(printf '%s' "$cmd" | tr ' -' '__')
      export BASE_HOME="$ROOT/a-ctl/home"
      ( cd "$ROOT/a-ctl/ws" && "$CONTROL" $cmd ) >"$ROOT/out/a-$tag.ctl.out" 2>"$ROOT/out/a-$tag.ctl.err"
      rc_c=$?
      export BASE_HOME="$ROOT/a-brn/home"
      ( cd "$ROOT/a-brn/ws" && "$BRANCH"  $cmd ) >"$ROOT/out/a-$tag.brn.out" 2>"$ROOT/out/a-$tag.brn.err"
      rc_b=$?
      n=$((n+1))
      so=$(cmp -s "$ROOT/out/a-$tag.ctl.out" "$ROOT/out/a-$tag.brn.out" && echo same || echo DIFF)
      se=$(cmp -s "$ROOT/out/a-$tag.ctl.err" "$ROOT/out/a-$tag.brn.err" && echo same || echo DIFF)
      bytes=$(wc -c < "$ROOT/out/a-$tag.ctl.out" | tr -d ' ')
      rows=$(grep -c '^| [a-z]' "$ROOT/out/a-$tag.ctl.out" 2>/dev/null || echo 0)
      printf '  base %-14s stdout=%-4s stderr=%-4s rc=%s/%s  (%s bytes, %s data row(s))\n' \
             "$cmd" "$so" "$se" "$rc_c" "$rc_b" "$bytes" "$rows"
      # An empty-state message compared against an empty-state message is not
      # evidence that list output is stable. The first run of this script passed
      # `fork list` on 10 bytes of "No forks." — a real comparison of nothing.
      if [ "${rows:-0}" -lt 1 ]; then
        skip "(a) '$cmd' printed no data rows ($bytes bytes) — comparing empty against empty proves nothing"
      elif [ "$so" = "same" ] && [ "$se" = "same" ] && [ "$rc_c" = "$rc_b" ]; then
        ok "(a) '$cmd' byte-identical over $rows data row(s) (stdout+stderr+rc)"
      else
        bad "(a) '$cmd' MOVED — this PR must not change list output"
        diff "$ROOT/out/a-$tag.ctl.out" "$ROOT/out/a-$tag.brn.out" | head -6 | sed 's/^/      /'
      fi
    done
    echo "  (a) real n compared: $n list invocation(s)"
  fi
fi
unset BASE_HOME
echo ""

# ── (b) the non-registry CLI surface ─────────────────────────────────────────
# The command list is DERIVED from the control binary's own help, never typed.
echo "=== (b) non-registry CLI surface — derived, not quoted ==="
"$CONTROL" --help >"$ROOT/out/top.ctl.out" 2>"$ROOT/out/top.ctl.err"
CMDS=$(awk '/^Commands:/{f=1;next} /^Options:/{f=0} f && /^[[:space:]]+[a-z]/{print $1}' "$ROOT/out/top.ctl.out" | sort -u)
total=$(printf '%s\n' $CMDS | grep -c . || echo 0)
echo "  derived from '\$CONTROL --help' -> $total subcommand(s):"
printf '%s\n' $CMDS | tr '\n' ' ' | fold -s -w 100 | sed 's/^/    /'
echo ""
if [ "${total:-0}" -lt 5 ]; then
  skip "(b) only $total subcommands derived — the help parse is wrong, refusing to report a sweep over it"
else
  same=0; moved=0; expected_moved=0; n=0
  MOVED_LIST=""
  for c in $CMDS; do
    "$CONTROL" "$c" --help >"$ROOT/out/b-$c.ctl.out" 2>"$ROOT/out/b-$c.ctl.err"
    "$BRANCH"  "$c" --help >"$ROOT/out/b-$c.brn.out" 2>"$ROOT/out/b-$c.brn.err"
    n=$((n+1))
    if cmp -s "$ROOT/out/b-$c.ctl.out" "$ROOT/out/b-$c.brn.out" && cmp -s "$ROOT/out/b-$c.ctl.err" "$ROOT/out/b-$c.brn.err"; then
      same=$((same+1))
    else
      case " $EXPECTED_DIFF " in
        *" $c "*) expected_moved=$((expected_moved+1)); echo "  EXPECTED-DIFF  $c  (R3: the registry help text changed by design)";;
        *)        moved=$((moved+1)); MOVED_LIST="$MOVED_LIST $c"; echo "  *** UNEXPECTED MOVE: $c";
                  diff "$ROOT/out/b-$c.ctl.out" "$ROOT/out/b-$c.brn.out" | head -8 | sed 's/^/      /';;
      esac
    fi
  done
  echo ""
  echo "  (b) real n compared     : $n subcommand help surfaces (stdout AND stderr each)"
  echo "  (b) byte-identical      : $same"
  echo "  (b) moved, DECLARED     : $expected_moved  ($EXPECTED_DIFF)"
  echo "  (b) moved, UNDECLARED   : $moved$MOVED_LIST"
  [ "$n" = "$total" ] || bad "(b) compared $n but derived $total — the sweep did not cover its own list"
  [ "$n" = "$total" ] && ok "(b) the sweep covered every derived subcommand ($n of $total)"

  # An instrument that flags EVERYTHING is describing itself, not the binaries.
  # First run of this script reported 37 undeclared moves; every one of them was
  # the `Usage: <argv[0]>` line differing because the control copy had been given
  # a different filename. A sweep in that state must say so instead of handing
  # over 37 findings.
  if [ "$same" = "0" ] && [ "$moved" -gt 0 ]; then
    bad "(b) INSTRUMENT SUSPECT: zero of $n surfaces matched. A change touching every"
    echo "      command is far likelier to be the harness than the binary — check argv[0],"
    echo "      cwd, env and temp paths before reading any of these as findings."
  fi

  if [ "$moved" = "0" ]; then
    ok "(b) no undeclared command's help moved ($same identical, $expected_moved declared)"
  else
    bad "(b) $moved undeclared command(s) moved:$MOVED_LIST"
  fi
  # The declared set must ACTUALLY have moved, or the row proves nothing.
  if [ "$expected_moved" -gt 0 ]; then
    ok "(b) the declared set really did move ($expected_moved) — the comparison can detect change"
  else
    bad "(b) the declared set did NOT move — this sweep cannot detect a difference and is not evidence"
  fi
fi

echo ""
# ── (c) NESTED subcommand surfaces ───────────────────────────────────────────
# (b) compares `base <cmd> --help` only. That is one level deep, and this PR's
# help change lives TWO levels down at `handoff create`, so (b) alone could have
# reported "38 of 39 identical" while a nested surface moved unnoticed. Nested
# commands are derived the same way — parsed out of each parent's own help.
echo "=== (c) nested subcommand surfaces — derived per parent, two levels deep ==="
nsame=0; nmoved=0; nexpected=0; nn=0; NMOVED_LIST=""
for c in $CMDS; do
  subs=$("$CONTROL" "$c" --help 2>/dev/null \
         | awk '/^Commands:/{f=1;next} /^Options:/{f=0} f && /^[[:space:]]+[a-z]/{print $1}' \
         | grep -v '^help$' | sort -u)
  [ -z "$subs" ] && continue
  for s in $subs; do
    "$CONTROL" "$c" "$s" --help >"$ROOT/out/c-$c-$s.ctl.out" 2>"$ROOT/out/c-$c-$s.ctl.err"
    "$BRANCH"  "$c" "$s" --help >"$ROOT/out/c-$c-$s.brn.out" 2>"$ROOT/out/c-$c-$s.brn.err"
    nn=$((nn+1))
    if cmp -s "$ROOT/out/c-$c-$s.ctl.out" "$ROOT/out/c-$c-$s.brn.out" \
    && cmp -s "$ROOT/out/c-$c-$s.ctl.err" "$ROOT/out/c-$c-$s.brn.err"; then
      nsame=$((nsame+1))
    elif [ "$c $s" = "$EXPECTED_DIFF_NESTED" ]; then
      nexpected=$((nexpected+1))
      echo "  EXPECTED-DIFF  $c $s"
      diff "$ROOT/out/c-$c-$s.ctl.out" "$ROOT/out/c-$c-$s.brn.out" | sed 's/^/      /'
    else
      nmoved=$((nmoved+1)); NMOVED_LIST="$NMOVED_LIST $c/$s"
      echo "  *** UNEXPECTED MOVE: $c $s"
      diff "$ROOT/out/c-$c-$s.ctl.out" "$ROOT/out/c-$c-$s.brn.out" | head -8 | sed 's/^/      /'
    fi
  done
done
echo ""
echo "  (c) real n compared   : $nn nested surface(s) (stdout AND stderr each)"
echo "  (c) byte-identical    : $nsame"
echo "  (c) moved, DECLARED   : $nexpected  ($EXPECTED_DIFF_NESTED)"
echo "  (c) moved, UNDECLARED : $nmoved$NMOVED_LIST"
if [ "$nn" -lt 10 ]; then
  skip "(c) only $nn nested surfaces derived — the parse is wrong, not reporting a sweep over it"
elif [ "$nmoved" = "0" ] && [ "$nexpected" -gt 0 ]; then
  ok "(c) $nsame of $nn nested surfaces byte-identical; the only move is the declared one"
elif [ "$nmoved" != "0" ]; then
  bad "(c) $nmoved undeclared nested surface(s) moved:$NMOVED_LIST"
else
  bad "(c) the declared nested surface did NOT move — this sweep cannot detect a difference"
fi

echo ""
echo "byte_identity_0142: $PASS pass, $FAIL fail, $SKIP skip"
[ "$FAIL" = "0" ]
