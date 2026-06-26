---
type: spec
status: delivered
tags: [base-v2, plugins, extensions, distribution, cross-platform]
relatedTo: [meta-cli, highlevel-cli, nano-banana]
---

# Cross-Platform Plugin Distribution — base-v2 Spec

> **Status (2026-06-25): DELIVERED.** P1–P4 all shipped. `base ext add` (fetch/verify/
> unpack/source-fallback) + Windows `.exe` resolution + `base ext scaffold` are live in
> base; all 3 CLI plugins (nano-banana, meta, highlevel) have Bun cross-platform releases
> (linux-x86_64 / darwin-aarch64 / windows-x86_64 + sha256). Verified live on linux; the
> macOS/Windows consumer path is code-complete and its assets build, pending a real
> Mac/Windows operator to execute.

## Problem

base itself ships cross-platform (`.github/workflows/release.yml`: linux-gnu / windows-msvc /
darwin-aarch64). But base's **extension system only wires LOCAL handlers** — `base ext install
[--bundle]` copies local files and points `framework_dir` at a repo or a bundled dir. There is
**no mechanism to distribute a compiled plugin's binary to operators per-OS.**

Interpreted handlers (nano-banana's `.mjs`) sidestep this — source runs anywhere a runtime
exists. **Compiled handlers do not**: a Linux `bin/meta` (ELF) will not `exec` under a macOS or
Windows `base`. base being native to the host doesn't help — base `exec`s the handler as a
*separate* child process (`plugin::exec` → `Command::new(handler)`); the handler must itself be
native. As more compiled CLI plugins ship (meta-cli, highlevel-cli, …), each independently hits
this wall.

## Goal

A **single standard** so every base CLI plugin works on all three target OSes **at install
time**, with **no operator toolchain**, solved **once**. Reuse base's proven release matrix and
manifest contract. **Strictly additive — no existing extension breaks.**

## Principles

- **Reuse:** base's `release.yml` matrix + asset-naming scheme, verbatim.
- **Additive:** `[dist]` is optional; absent → today's local/bundle behavior, unchanged.
- **No toolchain:** prebuilt binaries; source-build is a graceful *fallback* only.
- **Trust:** verify downloads (sha256); pin versions.

## 1. Manifest — optional `[dist]` block (additive)

New optional field on `ExtensionFile` (`#[serde(default)] pub dist: Option<DistSpec>`):

```toml
[dist]
repo    = "ChristopherKahler/meta-cli"   # GitHub owner/repo holding releases
version = "0.1.0"                          # pinned release tag (resolved as v<version>)
binary  = "meta"                            # bin name (→ meta.exe on Windows)
# Asset naming follows base's existing scheme: <binary>-<os>-<arch>.<ext>
#   linux   → <binary>-linux-x86_64.tar.gz
#   darwin  → <binary>-darwin-aarch64.tar.gz
#   windows → <binary>-windows-x86_64.zip
```

Absent `[dist]` ⇒ current behavior (local handler / `--bundle`). Every existing manifest
(nano-banana, design-humanizer, outpost) parses and behaves identically.

## 2. Build — standard `release.yml` (reused from base, parameterized)

Each plugin repo ships base's matrix with the binary name swapped:

- **3 targets** (identical to base): `x86_64-unknown-linux-gnu` / `x86_64-pc-windows-msvc` /
  `aarch64-apple-darwin`.
- **Asset names** (identical scheme to base): `<binary>-{linux|darwin|windows}-{x86_64|aarch64}.{tar.gz|zip}`.
- **Package contents:** the binary + `commands/` + `base-extension.toml` (+ `docs/` if present),
  plus a `.sha256` sidecar per asset.
- Fires on `v*` tags → GitHub Release.

Reference implementation: `apps/meta-cli/.github/workflows/release.yml`.

## 3. Install — platform-aware `base ext add` (the new base capability)

Extend the installer. When `[dist]` is present and the local handler isn't available (or `--fetch`):

1. **Detect host:** `std::env::consts::OS` → {linux, macos→darwin, windows}; `ARCH` → {x86_64, aarch64}.
2. **Resolve** the asset name from the `[dist]` scheme + host.
3. **Download** `https://github.com/{repo}/releases/download/v{version}/{asset}` (+ its `.sha256`).
4. **Verify** the checksum — abort loud on mismatch (supply-chain guard).
5. **Unpack** into `~/.base-gbl/plugins/<name>/`; `chmod +x` the binary (unix); repoint the
   manifest's `framework_dir` → that dir; install to `~/.base-gbl/extensions/<name>.toml`.
6. **Fallback:** no asset for the host **and** `cargo` present → build from source via
   `prepare.sh` (today's bundled-source path). Else: a clear error naming the missing target.

The existing `base ext install <toml> [--bundle]` **local-copy path is unchanged** — the fetch
is a separate branch, gated on `[dist]` + absence of a local handler (or explicit `--fetch`).

## 4. Handler resolution — Windows `.exe` (additive fix)

`plugin::resolve_handler` returns the handler path as-is. On Windows, if `handler` has no
extension and the file doesn't exist, also try `handler.exe`. Additive; unix path untouched.

## 5. Scaffold — `base ext scaffold <name>` (makes it part of the flow)

`src/plugin/scaffold.rs` stamps a complete, buildable **Bun** plugin matching the shipped
meta/highlevel/nano-banana layout: a runnable `index.ts` skeleton (JSON envelope + exit-code
contract + `--help`/`--version`), `package.json`, `tsconfig.json`, `prepare.sh`,
`base-extension.toml` (with `[dist]`), the standard Bun cross-compile `release.yml`,
`.gitignore`, `commands/`. Flags: `--into <dir>` (exact target folder), `--build`, `--git`,
`--create-repo` (private GitHub repo + push, via gh-create + SSH push), and `--bootstrap`
(all of it — the one-command kickoff). New tools are *born* cross-platform-ready and
conformant — the standard becomes the default, not per-tool effort.

## Backward-compatibility guarantees

- `[dist]` optional → all current extensions parse + behave identically.
- `base ext install --bundle` (local) path unchanged; fetch is a new, separately-gated branch.
- Windows `.exe` resolution is additive (unix unaffected).
- No change to handler exec, hooks, domains, or any existing command.

## Phased implementation (base-v2 PAUL)

- **P1** — `[dist]` struct + parse (additive) + reference `release.yml` in meta-cli. *(delivered.)*
- **P2** — platform-aware fetch / verify / unpack in `base ext add` (+ source fallback). *(delivered — auth-aware for private repos; verified live on linux-x86_64.)*
- **P3** — Windows `.exe` handler resolution. *(delivered.)*
- **P4** — `base ext scaffold` (+ `--into`/`--build`/`--git`/`--create-repo`/`--bootstrap`). *(delivered.)*

Each phase is independently shippable; none depends on changing existing behavior.

## Adopters

meta-cli (reference), highlevel-cli, every future compiled CLI plugin.
