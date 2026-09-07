#!/usr/bin/env bash
# The tier cluster as commands: #52, #18, #55, #53, and the three unfiled ones.
#
# Every leg drives the BINARY, so it runs against 0.14.0 and goes red, then
# against the fix and goes green. Unit tests of the new seam cannot do that --
# the functions do not exist on 0.14.0 -- so the red-first evidence lives here.
#
# Isolation is BASE_HOME *and* a cwd outside the real workspace tree. A tier bug
# is precisely the class where isolating one half writes to the live store (I8).
set -uo pipefail
B=${BASE_BIN:-}
R=/tmp/plover-tiercli

fail=0; ok(){ printf '  ok    %s\n' "$*"; }; bad(){ printf '  FAIL  %s\n' "$*"; fail=$((fail+1)); }

[ -n "$B" ] && [ -x "$B" ] || { echo "BASE_BIN is required and must be executable" >&2; exit 2; }
case "$B" in */target/*) echo "BASE_BIN is inside a cargo target dir; copy it aside." >&2; exit 2;; esac

fresh() {                      # fresh <name>  -> sets W, HOMEDIR
  local n="$1"
  rm -rf "$R/$n"
  mkdir -p "$R/$n/home/.base-gbl/.base" "$R/$n/home/.claude" "$R/$n/ws/.base"
  HOMEDIR="$R/$n/home"; W="$R/$n/ws"
}
run() { ( cd "$W" && BASE_HOME="$HOMEDIR" BASE_NO_AUTO_UPDATE=1 "$B" "$@" ) 2>&1; }
rc_of() { ( cd "$W" && BASE_HOME="$HOMEDIR" BASE_NO_AUTO_UPDATE=1 "$B" "$@" ) >/dev/null 2>&1; echo $?; }

wsdomain() { printf '[[domain]]\nname = "%s"\nmode = "keyword"\nprompt_keywords = ["%s"]\npaths = []\n' "$1" "$2" > "$W/.base/domains.toml"; }
gbldomain(){ printf '[[domain]]\nname = "%s"\nmode = "keyword"\nprompt_keywords = ["%s"]\npaths = []\n' "$1" "$2" > "$HOMEDIR/.base-gbl/domains.toml"; }

echo "tier CLI harness — base $("$B" --version | awk '{print $NF}')  md5 $(md5sum "$B" | cut -c1-12)"
rm -rf "$R"; echo

# ── #52 ─────────────────────────────────────────────────────────────────────
echo "── #52: a workspace domain is removable, and create does not silently land elsewhere ──"
fresh a; wsdomain MyApp myapp
[ "$(rc_of domain remove MyApp)" = "0" ] \
  && { grep -q 'MyApp' "$W/.base/domains.toml" \
        && bad "remove exited 0 and left the domain in place" \
        || ok "remove took the workspace domain"; } \
  || bad "remove failed on a domain that exists in the workspace"

fresh b; wsdomain MyApp myapp
out=$(run domain create --name myapp --path .)
if grep -q 'myapp' "$HOMEDIR/.base-gbl/domains.toml" 2>/dev/null; then
  bad "create wrote to the GLOBAL tier while standing in a workspace: $out"
else
  ok "create did not land in the other tier"
fi

# ── #18 ─────────────────────────────────────────────────────────────────────
echo
echo "── #18: a trigger added in a workspace is removable there, and a same-named global domain is untouched ──"
fresh c; wsdomain MyApp myapp; gbldomain MyApp myapp
run domain add-trigger --domain MyApp --keyword probeword > /dev/null
grep -q probeword "$W/.base/domains.toml" || bad "add-trigger did not write the workspace"
rc=$(rc_of domain remove-trigger --domain MyApp --keyword probeword)
if grep -q probeword "$W/.base/domains.toml"; then
  bad "remove-trigger left the workspace trigger (exit $rc)"
else
  ok "remove-trigger removed the workspace trigger"
fi
grep -q 'myapp' "$HOMEDIR/.base-gbl/domains.toml" \
  && ok "the same-named global domain was not edited" \
  || bad "the global domain was edited instead"

# ── #55 ─────────────────────────────────────────────────────────────────────
echo
echo "── #55: rule remove on a tier with no such rule fails ──"
fresh d; wsdomain DEMO demo; gbldomain DEMO demo
rc=$(rc_of rule remove --domain DEMO --index 10)
[ "$rc" != "0" ] && ok "exited $rc with nothing to remove" || bad "reported success with nothing removed"

# ── fifth: a missing graph.nq is empty, not an error ─────────────────────────
echo
echo "── fifth: a fresh tier takes its first rule, and lists without erroring ──"
fresh e; wsdomain myapp myapp; gbldomain myapp myapp
rm -f "$W/.base/graph.nq" "$HOMEDIR/.base-gbl/.base/graph.nq"
out=$(run rule add --domain myapp --text "first rule")
case "$out" in
  *"Failed to open"*) bad "rule add on a fresh tier: $out" ;;
  *) ok "rule add worked on a tier with no graph file" ;;
esac
out=$(run rule --global list --domain myapp)
case "$out" in
  *"Failed to open"*) bad "rule list -g with no global graph: $out" ;;
  *) ok "rule list on a tier with no graph file reports, not errors" ;;
esac

# ── #53 + sixth ─────────────────────────────────────────────────────────────
echo
echo "── #53: one command prints both tiers; domain list counts both ──"
fresh f; wsdomain myapp myapp; gbldomain myapp myapp
: > "$W/.base/graph.nq"; : > "$HOMEDIR/.base-gbl/.base/graph.nq"
run rule add --domain myapp --text "ws one" > /dev/null
run rule add --domain myapp --text "ws two" > /dev/null
run rule --global add --domain myapp --text "gbl one" > /dev/null
run rule --global add --domain myapp --text "gbl two" > /dev/null
run rule --global add --domain myapp --text "gbl three" > /dev/null
listing=$(run rule list --domain myapp)
n_ws=$(grep -c 'ws one\|ws two' <<<"$listing")
n_gb=$(grep -c 'gbl one\|gbl two\|gbl three' <<<"$listing")
if [ "$n_ws" = "2" ] && [ "$n_gb" = "3" ]; then
  ok "rule list printed all 5 across both tiers"
else
  bad "rule list printed ws=$n_ws gbl=$n_gb of 2/3"
  sed -n '1,6p' <<<"$listing" | sed 's/^/        /'
fi
dl=$(run domain list)
grep -qE '\| *2\+3 *\|' <<<"$dl" \
  && ok "domain list counts both tiers (2+3)" \
  || { bad "domain list did not show both tiers"; grep -i myapp <<<"$dl" | sed 's/^/        /'; }

# ── seventh: `domain get` is the third command that counted one tier ──────
dg=$(run domain get myapp)
n_get=$(grep -cE 'ws one|ws two|gbl one|gbl two|gbl three' <<<"$dg")
if grep -qE 'Rules \(5\)' <<<"$dg" && [ "$n_get" = "5" ]; then
  ok "domain get counts and lists all 5 across both tiers"
else
  bad "domain get reported $(grep -oE 'Rules \([0-9]+\)' <<<"$dg" | head -1), listed $n_get of 5"
  grep -E 'Rules \(|cli-' <<<"$dg" | sed 's/^/        /'
fi

echo
[ "$fail" -eq 0 ] && { echo "PASS — all seven."; exit 0; }
echo "FAIL — $fail leg(s)."; exit 1
