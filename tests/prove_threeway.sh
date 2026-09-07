#!/usr/bin/env bash
# The three-way byte diff the build record once claimed and did not have.
#
# install, scaffold, and a first session start on a home that saw neither must
# print the SAME first-run text. "Identical on every install path" is a diff.
#
# Also proves the two exclusivity rules the fix rests on:
#   - a home already welcomed is not welcomed twice
#   - a home with a swap in update.log gets the update notice, not the welcome
set -uo pipefail
B=${BASE_BIN:-}
if [ -z "$B" ]; then
  cat >&2 <<'EOF'
BASE_BIN is required. Point it at a binary you copied aside, not at a path
inside CARGO_TARGET_DIR: `cargo test` rewrites that tree underneath a running
harness, and a debug build measures the optimiser rather than the change.

  cargo build --release --bin base
  cp "$CARGO_TARGET_DIR/release/base" /tmp/base-branch
  BASE_BIN=/tmp/base-branch bash /tmp/prove_threeway.sh

EOF
  exit 2
fi
[ -x "$B" ] || { echo "BASE_BIN=$B is not executable" >&2; exit 2; }
case "$B" in
  */target/*)
    echo "BASE_BIN=$B is inside a cargo target dir. cargo test rewrites it mid-run; copy it aside." >&2
    exit 2 ;;
esac
VER=$("$B" --version | awk '{print $NF}')
OUT=/tmp/threeway; rm -rf "$OUT"; mkdir -p "$OUT"

# BASE_HOME isolates the HOME half. The WORKSPACE half still resolves from
# cwd: `find_workspace_base` walks up and finds the real /home/<user>/.base,
# so `hook session-start` run from anywhere inside the real tree tries to
# write that graph while isolated, and base's guard panics on it -- correctly.
# Isolating one half and not the other isolates nothing.
cd "$OUT" || exit 1

fail=0
ok()  { printf '  ok    %s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; fail=$((fail + 1)); }
block() { sed -n '/^base is installed\.$/,/^base — Built by Chris Kahler$/p' "$1"; }

# 1. install
H1="$OUT/h1"; mkdir -p "$H1/.claude"
BASE_HOME="$H1" "$B" install --skip-hooks --no-starter-commands > "$OUT/install.raw" 2>&1

# 2. scaffold, on a home that has NOT been welcomed
H2="$OUT/h2"; mkdir -p "$H2/.claude"
BASE_HOME="$H2" "$B" scaffold "$OUT/ws" > "$OUT/scaffold.raw" 2>&1

# 3. session start on a home with neither stamp nor update.log — the path that
#    did not exist before this fix.
H3="$OUT/h3"; mkdir -p "$H3/.base-gbl" "$H3/.claude"
BASE_HOME="$H3" BASE_NO_AUTO_UPDATE=1 "$B" hook session-start > "$OUT/session.raw" 2>&1

block "$OUT/install.raw"  > "$OUT/1.fr"
block "$OUT/scaffold.raw" > "$OUT/2.fr"
block "$OUT/session.raw"  > "$OUT/3.fr"

echo "base $VER — three-way first-run diff"
for f in 1 2 3; do
  [ -s "$OUT/$f.fr" ] || bad "path $f printed no first-run block"
done
[ -s "$OUT/3.fr" ] && ok "session start prints the message (the path that did not exist)"

diff "$OUT/1.fr" "$OUT/2.fr" > "$OUT/d12" 2>&1 \
  && ok "install == scaffold ($(wc -l < "$OUT/1.fr") lines)" \
  || { bad "install != scaffold"; cat "$OUT/d12"; }
diff "$OUT/1.fr" "$OUT/3.fr" > "$OUT/d13" 2>&1 \
  && ok "install == session start" \
  || { bad "install != session start"; cat "$OUT/d13"; }
echo

echo "── a welcomed home is not welcomed twice ──"
BASE_HOME="$H3" BASE_NO_AUTO_UPDATE=1 "$B" hook session-start > "$OUT/session2.raw" 2>&1
grep -q "^base is installed\.$" "$OUT/session2.raw" \
  && bad "welcomed the same home twice" || ok "second session is silent"
echo

echo "── an updated home gets the notice, never the welcome ──"
H4="$OUT/h4"; mkdir -p "$H4/.base-gbl" "$H4/.claude"
printf '%s updated 0.13.17 -> %s (background)\n' "$(date -Iseconds)" "$VER" > "$H4/.base-gbl/update.log"
BASE_HOME="$H4" BASE_NO_AUTO_UPDATE=1 "$B" hook session-start > "$OUT/upd.raw" 2>&1
grep -q "^base is installed\.$" "$OUT/upd.raw" \
  && bad "welcomed a home that was mid-upgrade" || ok "no welcome on an upgrade"
grep -q "base updated to $VER" "$OUT/upd.raw" \
  && ok "the update notice printed instead" || bad "neither message printed"
echo

[ "$fail" -eq 0 ] && { echo "PASS — three paths, one message."; exit 0; }
echo "FAIL — $fail problem(s)."; exit 1
