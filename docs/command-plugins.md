---
type: doc
status: active
tags: [base-v2, v0.6, command-plugins, extensions, cli]
relatedTo: [base-v2, extension-contract]
---

# Command Plugins (v0.6)

Drop-in `base <foo>` CLI commands contributed by extensions — a 4th extensibility
layer on top of star-commands (prompt modes), extensions (hooks/domains/ingest),
and the compiled core. A framework ships a specialized command as a script or
binary; an operator drops one TOML into `~/.base-gbl/extensions/`; `base <name>`
just works. No fork of the binary, no recompile, no separate tool to install.

## How it works

base's core CLI is compiled (clap). Unrecognized subcommands fall through clap's
`external_subcommand` seam (git's `git-foo` model) into the plugin dispatcher,
which:

1. builds a registry from the `[[commands]]` sections of every installed
   extension (`~/.base-gbl/extensions/*.toml`),
2. resolves the command name → handler,
3. execs the handler with the remaining args forwarded verbatim and an env
   contract injected, inheriting stdio.

**Core commands always resolve first** — a plugin can never shadow a built-in
(reserved-name guard). On a name collision between two extensions, the first
(extensions load sorted by name) wins.

## Declaring a command

Add to your extension manifest:

```toml
[extension]
name = "nano-banana"
version = "0.1.0"
description = "Image generation via the Gemini image API"
framework_dir = "~/chris-ai-systems/apps/nano-banana-studio/nanobanana-cli"

[[commands]]
name = "nano-banana"
handler = "bin/nano-banana.mjs"          # executable; relative to framework_dir
description = "Generate/edit images (Gemini)"
usage = "base nano-banana generate --prompt \"...\" [--ratio 16:9] [--json]"
```

- **`handler` must be directly executable** — a shebang'd script
  (`#!/usr/bin/env node`, then `chmod +x`) or a compiled binary. Any language.
- **Path resolution:** tilde-expanded; absolute used as-is; relative resolved
  against `framework_dir`, else the manifest's own directory.
- One extension may declare multiple `[[commands]]`.

## Install modes: linked vs packaged

A plugin can be installed two ways — same registry (`~/.base-gbl/extensions/`), different execution source:

| | **Linked** (default) | **Packaged** (`--bundle`) |
|---|---|---|
| Install | `base ext install <manifest>` | `base ext install --bundle <manifest>` |
| Handler runs from | the source repo (`framework_dir` points there) | `~/.base-gbl/plugins/<name>/` (a copy) |
| Edit → live | instant, no reinstall | re-bundle to pick up changes |
| Survives repo move/delete | ✗ | ✓ |
| Use when | developing / iterating | shipping, or stabilized on your own machine |

**`base ext install --bundle <manifest>`** copies the manifest's `framework_dir` into `~/.base-gbl/plugins/<name>/` (preserving executable bits; vendoring `node_modules` if present, else running `npm install`), then rewrites `framework_dir` to that copy. The result is repo-independent — the artifact a community member receives. `~/.base-gbl/plugins/` is an install *destination*, not a distribution source: you ship the repo/release; `--bundle` lands it there on each machine.

Re-running `--bundle` cleanly replaces the previous copy.

## Cross-platform distribution: `base ext add`

`--bundle` lands the **build host's** binary — a Linux ELF won't `exec` on macOS/Windows. For compiled handlers, give the manifest a `[dist]` block and ship per-OS release assets instead:

```toml
[dist]
repo    = "owner/my-tool-cli"   # GitHub repo holding the releases
version = "0.1.0"               # pinned tag (resolved as v<version>)
binary  = "my-tool"            # → my-tool.exe on Windows
```

`base ext add <manifest>` then detects the host OS/arch, downloads the matching asset (`<binary>-<os>-<arch>.<ext>` — `linux-x86_64.tar.gz` / `darwin-aarch64.tar.gz` / `windows-x86_64.zip`) from the GitHub release, **verifies its sha256**, unpacks into `~/.base-gbl/plugins/<name>/`, and repoints the manifest — no operator toolchain. Private repos are supported (it uses `GITHUB_TOKEN` / `~/.base-gbl/.env` / `gh`). If no asset exists for the host but a local source dir with `prepare.sh` is present, it falls back to a source build. `[dist]` is optional — absent, nothing changes.

## Scaffold a conformant plugin: `base ext scaffold`

`base ext scaffold <name>` stamps a complete, buildable Bun plugin (runnable `index.ts` skeleton with the envelope + exit-code contract, `package.json`, `prepare.sh`, `base-extension.toml` with `[dist]`, the standard cross-compile `release.yml`, `commands/`). Flags: `--into <dir>` (exact target folder), `--build`, `--git`, `--create-repo`, and **`--bootstrap`** — the one-command kickoff that writes the files, builds the binary, `git init`s + commits, and creates + pushes a private GitHub repo. New tools are born cross-platform-ready.

## The env contract

Every plugin process receives:

| Var | Value |
|---|---|
| `BASE_WORKSPACE` | workspace root (the dir containing `.base/`), else cwd |
| `BASE_GRAPH_PATH` | the workspace graph file (`.base/graph.nq`) |
| `BASE_GLOBAL_DIR` | `~/.base-gbl` |
| `BASE_BIN` | path to the running `base` binary |
| _plus_ | every `KEY=VALUE` in `~/.base-gbl/.env` |

### Secrets: `~/.base-gbl/.env`

This is the base-framework secret store. Put API keys there:

```
GEMINI_API_KEY=AIza...
```

base injects every line into the plugin's environment (dotenv semantics — an
already-exported var wins, so `.env` never clobbers an explicit export). Your
handler just reads `process.env.GEMINI_API_KEY` (or the equivalent). Git-ignored,
operator-owned.

### The sole-writer rule

Plugins **never** touch `graph.nq` directly. To read or write base state, a
plugin calls back through base:

```bash
"$BASE_BIN" learn --text "generated asset X" --domain content
"$BASE_BIN" recall --keyword "asset"
```

base stays the only graph writer — zero new API surface, and the graph format
has no second consumer to keep in sync.

## The output seam

base inherits the handler's stdio. Whatever the handler writes to **stdout**
flows straight to the terminal/Claude. The convention for machine-readable
results is a single JSON line on stdout (e.g. `{"ok":true,"images":[…]}`); there
is no special return channel.

## Invoking

```bash
base nano-banana generate --prompt "…" --json   # flat form (fallthrough)
base ext run nano-banana generate --prompt "…"  # explicit, collision-proof
```

`base ext run <name>` bypasses the fallthrough — use it when a name is ambiguous
or for stable scripting.

## Discovery

```bash
base ext list      # extensions + their contributed plugin commands
base --help        # points to `base ext list`
```

Unknown commands fail loud (exit 127) with a did-you-mean suggestion when a near
match exists.

## Exit codes

base propagates the handler's exit code unchanged. base-level failures:
`127` handler not found / unknown command · `126` handler not executable.

## Authoring checklist

1. Write the handler as an executable (shebang + `chmod +x`), reading secrets
   from env (`~/.base-gbl/.env`) and state via `$BASE_BIN`.
2. Emit a single JSON line to stdout under a `--json` flag.
3. Add a `[[commands]]` entry to your extension manifest.
4. `base ext install <manifest>.toml` (or drop it in `~/.base-gbl/extensions/`).
5. `base ext list` to confirm; `base <name> …` to run.
