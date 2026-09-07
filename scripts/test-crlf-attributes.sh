#!/usr/bin/env bash
# #95: every script this repo executes must check out with LF endings on every platform.
#
# Runs as a CI step, and CI runs on ubuntu where the files are LF regardless — so a test
# that only looked at the working tree would pass on the runner and prove nothing. What is
# portable is the ATTRIBUTE: `git check-attr` answers the same on every platform, because
# it reads .gitattributes rather than the checkout. That is the thing that decides what a
# Windows clone materialises, so that is what this asserts.
#
# The second half runs only where a CRLF checkout is possible, and says so when it skips.
set -uo pipefail
cd "$(dirname "$0")/.."

pass=0; fail=0; skip=0
ok()   { pass=$((pass+1)); printf '  PASS  %s\n' "$1"; }
bad()  { fail=$((fail+1)); printf '  FAIL  %s\n' "$1"; }
skipped() { skip=$((skip+1)); printf '  SKIP  %s\n' "$1"; }

echo "=== .gitattributes exists ==="
if [ -f .gitattributes ]; then ok ".gitattributes at the repo root"
else bad ".gitattributes is missing — every .sh will check out CRLF on Windows"; fi

echo
echo "=== every executed script is pinned to lf ==="
n_sh=0; n_py=0
while IFS= read -r f; do
    n_sh=$((n_sh+1))
    eol=$(git check-attr eol -- "$f" | sed 's/.*: eol: //')
    [ "$eol" = "lf" ] && ok "$f -> eol=lf" || bad "$f -> eol=$eol (want lf)"
done < <(git ls-files '*.sh')
while IFS= read -r f; do
    n_py=$((n_py+1))
    eol=$(git check-attr eol -- "$f" | sed 's/.*: eol: //')
    [ "$eol" = "lf" ] && ok "$f -> eol=lf" || bad "$f -> eol=$eol (want lf)"
done < <(git ls-files '*.py')
echo "  ($n_sh .sh, $n_py .py inspected)"

echo
echo "=== .ps1 is pinned too, so it has one identity rather than two ==="
while IFS= read -r f; do
    eol=$(git check-attr eol -- "$f" | sed 's/.*: eol: //')
    [ "$eol" = "crlf" ] && ok "$f -> eol=crlf" || bad "$f -> eol=$eol (want crlf)"
done < <(git ls-files '*.ps1')

echo
echo "=== no CR bytes in the working tree, and every shebang is clean ==="
# On a Linux runner this is true whether or not .gitattributes exists, so it is reported
# as corroboration and never as the proof. The attribute rows above are the proof.
crlf_seen=0
while IFS= read -r f; do
    if grep -qU $'\r' "$f" 2>/dev/null; then
        crlf_seen=$((crlf_seen+1))
        bad "$f contains CR bytes in the working tree"
    fi
    first=$(head -c 200 "$f" | head -1)
    case "$first" in
        \#\!*$'\r') bad "$f has a CR-terminated shebang: env would refuse it with rc 126" ;;
    esac
done < <(git ls-files '*.sh')
[ "$crlf_seen" -eq 0 ] && ok "no CR bytes in any tracked .sh on this checkout"

echo
echo "=== a CRLF shebang really is fatal (the mechanism, on this kernel) ==="
if command -v env >/dev/null 2>&1; then
    t=$(mktemp -d)
    printf '#!/usr/bin/env bash\r\necho BODY-RAN\n' > "$t/crlf.sh"; chmod +x "$t/crlf.sh"
    printf '#!/usr/bin/env bash\necho BODY-RAN\n'   > "$t/lf.sh";   chmod +x "$t/lf.sh"
    out_crlf=$("$t/crlf.sh" 2>&1); rc_crlf=$?
    out_lf=$("$t/lf.sh" 2>&1); rc_lf=$?
    rm -rf "$t"
    if [ "$rc_lf" -eq 0 ] && [ "$rc_crlf" -ne 0 ]; then
        ok "CRLF shebang rc=$rc_crlf ($out_crlf); LF control rc=$rc_lf"
    else
        skipped "this kernel tolerates a CRLF shebang (crlf rc=$rc_crlf, lf rc=$rc_lf) — the attribute rows still stand"
    fi
else
    skipped "no env(1) to test the mechanism with"
fi

echo
echo "───────────────────────────────────────────"
echo "  $pass PASS / $fail FAIL / $skip SKIP"
[ "$fail" -eq 0 ] || exit 1
