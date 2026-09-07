<#
    base installer for Windows — downloads a release binary and hands off to `base install`.

        irm https://raw.githubusercontent.com/ChristopherKahler/base/main/install.ps1 | iex

    No Rust toolchain, no MSVC, no LLVM. Everything after the download is
    `base install` doing what it already does: copy the binary to
    ~\.local\bin\base.exe, write ~\.base-gbl\, and wire the hooks into
    ~\.claude\settings.json.

    Environment:
      BASE_VERSION        pin a tag, e.g. v0.14.1 (default: latest release)
      BASE_INSTALL_ARGS   passed through to `base install`
#>

$ErrorActionPreference = 'Stop'

$repo = 'ChristopherKahler/base'

function Die($msg) { Write-Error "install: $msg"; exit 1 }
function Say($msg) { Write-Host $msg }

# ── which build ───────────────────────────────────────────────────────────────
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne 'AMD64') {
    Die "unsupported architecture: $arch. Only windows-x86_64 is published; build from source (see the README)."
}
$asset = 'base-windows-x86_64.zip'

if ($env:BASE_VERSION) {
    $url   = "https://github.com/$repo/releases/download/$($env:BASE_VERSION)/$asset"
    $label = $env:BASE_VERSION
} else {
    $url   = "https://github.com/$repo/releases/latest/download/$asset"
    $label = 'latest'
}

# ── download, unpack, hand off ────────────────────────────────────────────────
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("base-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

try {
    Say "base: fetching $asset ($label)"
    $zip = Join-Path $tmp $asset
    try {
        # Invoke-WebRequest is slow with the progress bar on large files.
        $prev = $ProgressPreference; $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
        $ProgressPreference = $prev
    } catch {
        Die "download failed: $url"
    }

    try {
        Expand-Archive -Path $zip -DestinationPath $tmp -Force
    } catch {
        Die "downloaded file is not a valid archive - check that $label exists"
    }

    $exe = Join-Path $tmp 'base.exe'
    if (-not (Test-Path $exe)) { Die 'archive did not contain base.exe' }

    Say 'base: installing'
    $installArgs = @('install')
    if ($env:BASE_INSTALL_ARGS) { $installArgs += ($env:BASE_INSTALL_ARGS -split '\s+') }

    # Run from inside the unpacked archive. `base install` locates scripts\ast by
    # trying two paths relative to the binary and then the working directory; a
    # release archive only satisfies the third, so this is what makes the AST
    # extractor land instead of printing "not found near binary".
    Push-Location $tmp
    try {
        & $exe @installArgs
        if ($LASTEXITCODE -ne 0) { Die "base install exited $LASTEXITCODE" }
    } finally {
        Pop-Location
    }
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# ── the .exe that Windows needs ───────────────────────────────────────────────
# `base install` copies the binary to ~\.local\bin\base with no extension
# (install.rs:1605, one path shared with Unix). The file is a valid PE image, but
# PATHEXT means cmd.exe answers "'base' is not recognized" and PowerShell runs it
# to no effect, so a fresh install reports success and then does nothing. Give
# Windows the name it can resolve.
$binHome = if ($env:BASE_HOME) { $env:BASE_HOME } else { $HOME }
$binDir  = Join-Path $binHome '.local\bin'
$plain   = Join-Path $binDir 'base'
$withExe = Join-Path $binDir 'base.exe'
if ((Test-Path $plain) -and -not (Test-Path $withExe)) {
    Copy-Item $plain $withExe -Force
    Say "base: wrote base.exe alongside base (Windows needs the extension to run it by name)"
}

# ── PATH check, after the fact so it never blocks the install ─────────────────
$bindir = Join-Path $binHome '.local\bin'
$onPath = ($env:PATH -split ';') -contains $bindir
if (-not $onPath) {
    Say ''
    Say "base: $bindir is not on your PATH. Add it for your user:"
    Say ''
    Say "    `$p = [Environment]::GetEnvironmentVariable('PATH','User')"
    Say "    [Environment]::SetEnvironmentVariable('PATH', '$bindir;' + `$p, 'User')"
    Say ''
    Say '  then reopen your terminal.'
}

# Hooks are what make base do anything, and they are only written into an
# existing ~\.claude. Installing base before Claude Code leaves it inert with no
# way to self-heal, because the repair runs from the session-start hook that was
# never wired. Test settings.json rather than the directory: `base install`
# creates ~\.claude itself to hold the bundled skill, so the directory always
# exists afterwards and cannot tell us anything.
if (-not (Test-Path (Join-Path $binHome '.claude\settings.json'))) {
    Say ''
    Say 'base: no ~\.claude directory, so the hooks were not wired and base will'
    Say '  not do anything yet. Install Claude Code, then run:'
    Say ''
    Say '    base install'
    Say ''
}
