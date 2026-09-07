<#
    base installer for Windows — downloads a release binary and hands off to `base install`.

        irm https://raw.githubusercontent.com/ChristopherKahler/base/main/install.ps1 | iex

    No Rust toolchain, no MSVC, no LLVM.

    Environment:
      BASE_VERSION        pin a tag, e.g. v0.14.1 (default: latest release)
      BASE_INSTALL_ARGS   passed through to `base install`

    Everything runs inside one scriptblock. Under `irm | iex` the script body would
    otherwise execute in the caller's own session, leaving $ErrorActionPreference,
    the helper functions and every temp variable behind after the install, and an
    `exit` would close the user's shell. The scriptblock scopes all of it, and
    failures throw rather than exit.
#>

& {
    $ErrorActionPreference = 'Stop'

    $repo = 'ChristopherKahler/base'

    function Die($msg) { throw $msg }
    function Say($msg) { Write-Host $msg }

    try {
        # ── which build ───────────────────────────────────────────────────────
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

        # BASE_HOME is base's own override and the only one that isolates on
        # Windows: src/home.rs notes that `dirs` never consults $HOME here, so a
        # USERPROFILE-based fake home silently targets the real tier. Honour it so
        # this script's own checks read the same home the binary writes to.
        $binHome = if ($env:BASE_HOME) { $env:BASE_HOME } else { $HOME }
        $binDir  = Join-Path $binHome '.local\bin'
        $plain   = Join-Path $binDir 'base'
        $withExe = Join-Path $binDir 'base.exe'

        function HashOf($p) {
            if (Test-Path $p) { (Get-FileHash $p -Algorithm SHA256).Hash } else { $null }
        }
        $exeBefore = HashOf $withExe

        # ── download, unpack, hand off ────────────────────────────────────────
        $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("base-install-" + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $tmp -Force | Out-Null

        try {
            Say "base: fetching $asset ($label)"
            $zip = Join-Path $tmp $asset
            try {
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

            # Run from inside the unpacked archive: `base install` locates
            # scripts\ast by trying two paths relative to the binary and then the
            # working directory, and a release archive satisfies only the third.
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

        # ── the .exe Windows needs, refreshed on every run ────────────────────
        # `base install` writes ~\.local\bin\base with no extension, and PATHEXT
        # means Windows cannot run that by name. Copy unconditionally with -Force:
        # copying only when base.exe is ABSENT leaves an existing base.exe stale
        # on every upgrade, so the user keeps running the old binary while the
        # installer reports success — a silent no-op update, which is worse than
        # the missing-file bug this copy exists to fix.
        if (Test-Path $plain) {
            Copy-Item $plain $withExe -Force
            $exeAfter = HashOf $withExe
            if ($exeBefore -and $exeBefore -eq $exeAfter) {
                Say 'base: base.exe already current'
            } elseif ($exeBefore) {
                Say 'base: refreshed base.exe (Windows needs the extension to run it by name)'
            } else {
                Say 'base: wrote base.exe alongside base (Windows needs the extension to run it by name)'
            }
        }

        # ── PATH check, after the fact so it never blocks the install ─────────
        $onPath = ($env:PATH -split ';') -contains $binDir
        if (-not $onPath) {
            Say ''
            Say "base: $binDir is not on your PATH. Add it for your user:"
            Say ''
            Say "    `$p = [Environment]::GetEnvironmentVariable('PATH','User')"
            Say "    [Environment]::SetEnvironmentVariable('PATH', '$binDir;' + `$p, 'User')"
            Say ''
            Say '  then reopen your terminal.'
        }

        # Hooks are only written into an existing ~\.claude. Installing base
        # before Claude Code leaves it inert with no way to self-heal, because the
        # repair runs from the session-start hook that was never wired. Test
        # settings.json, not the directory: `base install` creates ~\.claude
        # itself for the bundled skill, so the directory always exists afterwards.
        if (-not (Test-Path (Join-Path $binHome '.claude\settings.json'))) {
            Say ''
            Say 'base: no ~\.claude directory, so the hooks were not wired and base will'
            Say '  not do anything yet. Install Claude Code, then run:'
            Say ''
            Say '    base install'
            Say ''
        }
    }
    catch {
        Write-Host ''
        Write-Host "base install failed: $($_.Exception.Message)"
        Write-Host 'Nothing was changed on your PATH. Report at https://github.com/ChristopherKahler/base/issues'
    }
}
