#!/usr/bin/env bash
# K2e (BO-14): `base rule test` over the fixture rule set in tests/fixtures/rule-tests/, in a throwaway home, must exit
# 0. The fixture carries a prompt that must serve each rule and one that must not, chosen so each part of prompt
# matching has a case that fails if it breaks (whole-word and multi-word keywords, case, exclude, auto_inject = false,
# an always-on domain, a rule scored on its own words). A matcher change that breaks matching fails the build.
#
# Then the CONTROL: the same set with one rule whose fires_on cannot match must exit 1. Without it a green step could
# mean the tests held or that nothing ran; the control shows this step can fail.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cargo build --quiet --manifest-path "$root/Cargo.toml"
target="${CARGO_TARGET_DIR:-$root/target}"
bin="$target/debug/base"
[ -f "$bin" ] || bin="$bin.exe"
[ -f "$bin" ] || { echo "FAIL: no base binary at $target/debug"; exit 1; }

# The throwaway home must sit where the binary's own write tripwire (src/home.rs, armed whenever BASE_HOME is set)
# accepts it: inside the system temp folder as the BINARY spells it, or outside the real home. On a GitHub Windows
# runner `mktemp` spells %TEMP% one way and the binary's temp_dir() another, so the tripwire read the fixture's graph
# write as a write into the real home (PR #191, run 37113466070). RUNNER_TEMP (D:\a\_temp there) is outside the
# profile, so on a Windows runner the home goes there. Both spellings are printed, so a log shows which applied.
if command -v cygpath >/dev/null 2>&1; then
  echo "temp folder: bash TMP=${TMP:-} · native $(powershell.exe -NoProfile -Command '[IO.Path]::GetTempPath()' 2>/dev/null | tr -d '\r')"
fi
if command -v cygpath >/dev/null 2>&1 && [ -n "${RUNNER_TEMP:-}" ]; then
  tmp="$(mktemp -d -p "$(cygpath -u "$RUNNER_TEMP")")"
else
  tmp="$(mktemp -d)"
fi
if command -v cygpath >/dev/null 2>&1; then echo "throwaway home under: $(cygpath -w "$tmp")"; fi
trap 'rm -rf "$tmp"' EXIT

# run <name> <domains.toml>: `base rule test` in a fresh home whose workspace holds that domains.toml.
run() {
  local home="$tmp/$1/home" ws="$tmp/$1/ws"
  mkdir -p "$home/.base-gbl/.base" "$ws/.base"
  cp "$2" "$ws/.base/domains.toml"
  # A native Windows binary needs a Windows path in BASE_HOME; Git Bash converts the working directory itself.
  if command -v cygpath >/dev/null 2>&1; then home="$(cygpath -w "$home")"; fi
  (cd "$ws" && BASE_HOME="$home" BASE_NO_AUTO_UPDATE=1 BASE_AST_NO_SPAWN=1 "$bin" rule test)
}

fixture="$root/tests/fixtures/rule-tests/domains.toml"
echo "== base rule test on the fixture rule set (must exit 0)"
run fixture "$fixture"

echo "== control: one more rule whose fires_on cannot match (must exit 1)"
cp "$fixture" "$tmp/control.toml"
printf '\n[[domain]]\nname = "control"\nprompt_keywords = ["zzz-never"]\nrules = [{ text = "control", fires_on = ["nothing matches this"] }]\n' >> "$tmp/control.toml"
rc=0
run control "$tmp/control.toml" || rc=$?
if [ "$rc" -ne 1 ]; then
  echo "FAIL: the control exited $rc, not 1, so this step could not have failed"
  exit 1
fi
echo "ok: the fixture's rule tests pass, and the control fails as it must"
