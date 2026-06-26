#requires -Version 5.1
<#
.SYNOPSIS
    Build and install `base` from source on Windows, one-shot.

.DESCRIPTION
    A from-source build of base needs a native toolchain that the bare
    `cargo build` does not set up on its own:
      * MSVC C++ build tools + Windows SDK (the cl.exe / link.exe / INCLUDE / LIB env)
      * LLVM / libclang  — bindgen builds the vendored RocksDB (oxigraph) against it
      * cargo on PATH    — ~/.cargo/bin

    This script imports the Visual Studio developer environment, locates (or
    installs) libclang, then runs `cargo install --path . --locked` so `base.exe`
    lands on PATH. Run it from the repo root in a normal PowerShell window — it
    does NOT need to be launched from a Developer prompt.

    PREFERRED ALTERNATIVE: skip the build entirely and download the prebuilt
    `base-windows-x86_64.zip` from the GitHub release (or `npx chrisai`). Build
    from source only when you need a local/unreleased revision.

.EXAMPLE
    pwsh -File scripts/build-base-windows.ps1
.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-base-windows.ps1 -SkipInstall
#>
[CmdletBinding()]
param(
    # Build only (cargo build --release); do not `cargo install` onto PATH.
    [switch]$SkipInstall,
    # Skip running `base install` after a successful build.
    [switch]$NoPostInstall
)

$ErrorActionPreference = 'Stop'

function Write-Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Write-Ok($msg)   { Write-Host "  + $msg" -ForegroundColor Green }
function Write-Warn2($msg) { Write-Host "  ! $msg" -ForegroundColor Yellow }
function Fail($msg)        { Write-Host "  x $msg" -ForegroundColor Red; exit 1 }

# Repo root = parent of this script's directory.
$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
Set-Location $RepoRoot
if (-not (Test-Path (Join-Path $RepoRoot 'Cargo.toml'))) {
    Fail "Cargo.toml not found in $RepoRoot — run this from the base repo."
}

# ── 1. MSVC developer environment (vcvars64) ───────────────────────────────
Write-Step "Importing Visual Studio developer environment"
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) {
    Fail @"
vswhere.exe not found — Visual Studio Build Tools are not installed.
Install them (one of):
  winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --quiet"
  …or the full Visual Studio with the 'Desktop development with C++' workload.
"@
}

$vsPath = & $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath
if (-not $vsPath) { Fail "No VS install with the C++ (VC.Tools.x86.x64) component. Add the 'Desktop development with C++' workload." }

$vcvars = Join-Path $vsPath 'VC\Auxiliary\Build\vcvars64.bat'
if (-not (Test-Path $vcvars)) { Fail "vcvars64.bat not found under $vsPath." }

# Run vcvars64 in cmd, then import every env var it set back into this session.
$tmp = [System.IO.Path]::GetTempFileName()
cmd /c "`"$vcvars`" >NUL 2>&1 && set" > $tmp
Get-Content $tmp | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') {
        Set-Item -Path ("Env:" + $matches[1]) -Value $matches[2]
    }
}
Remove-Item $tmp -Force
if (-not $env:INCLUDE -or -not $env:LIB) { Fail "vcvars did not populate INCLUDE/LIB — MSVC env import failed." }
Write-Ok "MSVC env imported ($vsPath)"

# ── 2. LLVM / libclang (bindgen needs it for the RocksDB build) ─────────────
Write-Step "Locating libclang (LLVM)"
function Find-LibClang {
    $candidates = @(
        $env:LIBCLANG_PATH,
        (Join-Path $env:ProgramFiles 'LLVM\bin'),
        (Join-Path ${env:ProgramFiles(x86)} 'LLVM\bin')
    ) | Where-Object { $_ }
    foreach ($c in $candidates) {
        if (Test-Path (Join-Path $c 'libclang.dll')) { return $c }
    }
    # LLVM bundled inside the VS install.
    $vsClang = Get-ChildItem -Path (Join-Path $vsPath 'VC\Tools\Llvm') -Filter libclang.dll -Recurse -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($vsClang) { return $vsClang.DirectoryName }
    return $null
}

$libclang = Find-LibClang
if (-not $libclang) {
    Write-Warn2 "libclang not found — attempting 'winget install LLVM.LLVM'"
    try { winget install --id LLVM.LLVM -e --silent --accept-package-agreements --accept-source-agreements } catch {}
    $libclang = Find-LibClang
}
if (-not $libclang) {
    Fail @"
libclang.dll not found. Install LLVM and re-run:
  winget install --id LLVM.LLVM -e
  …or set LIBCLANG_PATH to a folder containing libclang.dll.
"@
}
$env:LIBCLANG_PATH = $libclang
Write-Ok "LIBCLANG_PATH = $libclang"

# ── 3. cargo on PATH ───────────────────────────────────────────────────────
Write-Step "Checking Rust toolchain"
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Fail @"
cargo not found. Install the Rust toolchain (MSVC ABI) and re-run:
  winget install --id Rustlang.Rustup -e
  rustup default stable-x86_64-pc-windows-msvc
"@
}
Write-Ok ((cargo --version) -join '')

# ── 4. Build / install ─────────────────────────────────────────────────────
if ($SkipInstall) {
    Write-Step "Building base (cargo build --release)"
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { Fail "cargo build failed (exit $LASTEXITCODE)." }
    Write-Ok "Built target\release\base.exe"
} else {
    Write-Step "Installing base (cargo install --path . --locked)"
    cargo install --path . --locked --force
    if ($LASTEXITCODE -ne 0) { Fail "cargo install failed (exit $LASTEXITCODE)." }
    Write-Ok "Installed base.exe → $cargoBin"

    if (-not $NoPostInstall -and (Get-Command base -ErrorAction SilentlyContinue)) {
        Write-Step "Running 'base install'"
        base install
    }
}

Write-Host ""
Write-Ok "Done. Next: 'base install' (if not already run) then 'base sync --ast'."
