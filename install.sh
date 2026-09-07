#!/bin/sh
# base installer — downloads a release binary and hands off to `base install`.
#
#   curl -fsSL https://raw.githubusercontent.com/ChristopherKahler/base/main/install.sh | sh
#
# No Rust toolchain, no compiler. Everything after the download is `base install`
# doing what it already does: copy the binary to ~/.local/bin/base, write
# ~/.base-gbl/, and wire the hooks into ~/.claude/settings.json.
#
# Environment:
#   BASE_VERSION   pin a tag, e.g. v0.14.1 (default: latest release)
#   BASE_INSTALL_ARGS   passed through to `base install`, e.g. --no-starter-commands
#
# Any argument given to this script is also passed through to `base install`.

set -eu

REPO="ChristopherKahler/base"
INSTALL_ARGS="${BASE_INSTALL_ARGS:-}${*:+ $*}"

say()  { printf '%s\n' "$*"; }
die()  { printf 'install: %s\n' "$*" >&2; exit 1; }

# ── the fetcher ──────────────────────────────────────────────────────────────
if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
else
  die "need curl or wget on PATH"
fi

# ── which build ──────────────────────────────────────────────────────────────
os=$(uname -s)
arch=$(uname -m)

case "$os" in
  Linux)  os_tag=linux ;;
  Darwin) os_tag=darwin ;;
  *)
    die "unsupported OS: $os. Windows uses install.ps1; everything else builds from source (see the README)."
    ;;
esac

case "$arch" in
  x86_64|amd64)  arch_tag=x86_64 ;;
  arm64|aarch64) arch_tag=aarch64 ;;
  *) die "unsupported architecture: $arch" ;;
esac

# Releases carry linux-x86_64, darwin-x86_64 and darwin-aarch64. There is no
# linux-aarch64 build, so say that plainly rather than 404 on the download.
if [ "$os_tag" = linux ] && [ "$arch_tag" = aarch64 ]; then
  die "no linux-aarch64 release is published yet. Build from source: https://github.com/$REPO#build-from-source"
fi

asset="base-${os_tag}-${arch_tag}.tar.gz"

if [ -n "${BASE_VERSION:-}" ]; then
  url="https://github.com/$REPO/releases/download/${BASE_VERSION}/${asset}"
  label="$BASE_VERSION"
else
  url="https://github.com/$REPO/releases/latest/download/${asset}"
  label="latest"
fi

# ── download, unpack, hand off ───────────────────────────────────────────────
tmp=$(mktemp -d 2>/dev/null || mktemp -d -t base-install)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "base: fetching $asset ($label)"
fetch "$url" "$tmp/$asset" || die "download failed: $url"

# A GitHub 404 page is served as HTML with a 200 on some proxies; a truncated
# or wrong-content download would otherwise reach `tar` as a confusing error.
tar tzf "$tmp/$asset" >/dev/null 2>&1 || die "downloaded file is not a valid archive — check that $label exists"

tar xzf "$tmp/$asset" -C "$tmp"
[ -x "$tmp/base" ] || chmod +x "$tmp/base" 2>/dev/null || true
[ -f "$tmp/base" ] || die "archive did not contain the base binary"

say "base: installing"
# Run from inside the unpacked archive. `base install` locates scripts/ast by
# trying, in order, two paths relative to the binary and then the working
# directory; a release archive only satisfies the third, so the cd is what makes
# the AST extractor land instead of printing "not found near binary".
# shellcheck disable=SC2086
(cd "$tmp" && ./base install $INSTALL_ARGS)

# ── after the fact, so neither check can block the install ───────────────────
# Follow BASE_HOME when it is set, the way install.ps1 does: base writes to the
# root it resolves, and these checks have to look where the binary actually
# landed or they report on the wrong home.
root="${BASE_HOME:-$HOME}"
bindir="$root/.local/bin"
case ":${PATH}:" in
  *":$bindir:"*) ;;
  *)
    say ""
    say "base: $bindir is not on your PATH. Add it:"
    say ""
    say "    export PATH=\"\$HOME/.local/bin:\$PATH\""
    say ""
    say "  then reopen your shell, or run it now for this session."
    ;;
esac

# Hooks are what make base do anything, and they are only written into an
# existing ~/.claude. Installing base before Claude Code leaves it inert with no
# way to self-heal, because the repair runs from the session-start hook that was
# never wired. Test settings.json rather than the directory: `base install`
# creates ~/.claude itself to hold the bundled skill, so the directory always
# exists afterwards and cannot tell us anything.
if [ ! -f "$root/.claude/settings.json" ]; then
  say ""
  say "base: no ~/.claude directory, so the hooks were not wired and base will"
  say "  not do anything yet. Install Claude Code, then run:"
  say ""
  say "    base install"
  say ""
fi
