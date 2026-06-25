//! `base ext scaffold <name>` — P4 of PLUGIN-DIST-SPEC.md.
//!
//! Stamp a new, conformant Bun command-plugin so every future tool is *born*
//! cross-platform-ready: a runnable `index.ts` skeleton (JSON envelope + exit-code
//! contract + `--help`/`--version`), `package.json`, `tsconfig.json`, `prepare.sh`
//! (`bun build --compile` → `bin/<name>`), `base-extension.toml` (with `[dist]`),
//! the standard Bun cross-compile `release.yml`, `.gitignore`, and a `commands/`
//! dir. The standard becomes the default, not per-tool effort.
//!
//! Mirrors the shipped meta-cli / highlevel-cli / nano-banana layout exactly, so a
//! scaffolded tool builds + bundles + `base ext add`s with no further wiring.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Outcome of a scaffold, for CLI reporting.
pub struct ScaffoldOutcome {
    pub name: String,
    pub dir: PathBuf,
    pub files: Vec<String>,
}

/// Validate a plugin/binary name: lowercase, alnum + single hyphens (clap/asset-safe).
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

fn render(template: &str, name: &str, repo: &str) -> String {
    template
        .replace("{{NAME}}", name)
        .replace("{{REPO}}", repo)
}

/// Scaffold `<parent>/<name>-cli/` with a complete, buildable Bun plugin.
pub fn scaffold_plugin(name: &str, parent: &Path, repo: Option<&str>) -> Result<ScaffoldOutcome> {
    if crate::plugin::is_reserved(name) {
        bail!("'{name}' is a reserved base command — pick another plugin name");
    }
    if !valid_name(name) {
        bail!("invalid plugin name '{name}' — use lowercase letters, digits and single hyphens (e.g. my-tool)");
    }
    let repo = repo.map(str::to_string).unwrap_or_else(|| format!("ChristopherKahler/{name}-cli"));

    let dir = parent.join(format!("{name}-cli"));
    if dir.exists() {
        bail!("{} already exists — refusing to overwrite", dir.display());
    }
    std::fs::create_dir_all(dir.join(".github").join("workflows"))?;
    std::fs::create_dir_all(dir.join("commands"))?;

    let mut files = Vec::new();
    let mut write = |rel: &str, body: String| -> Result<()> {
        let path = dir.join(rel);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        files.push(rel.to_string());
        Ok(())
    };

    write("base-extension.toml", render(MANIFEST, name, &repo))?;
    write("index.ts", render(INDEX_TS, name, &repo))?;
    write("package.json", render(PACKAGE_JSON, name, &repo))?;
    write("tsconfig.json", TSCONFIG.to_string())?;
    write("prepare.sh", render(PREPARE_SH, name, &repo))?;
    write(".github/workflows/release.yml", render(RELEASE_YML, name, &repo))?;
    write(".gitignore", GITIGNORE.to_string())?;
    write("README.md", render(README, name, &repo))?;
    write("commands/.gitkeep", String::new())?;

    // prepare.sh must be executable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir.join("prepare.sh"), std::fs::Permissions::from_mode(0o755));
    }

    Ok(ScaffoldOutcome {
        name: name.to_string(),
        dir,
        files,
    })
}

pub fn format_scaffold_human(o: &ScaffoldOutcome) -> String {
    format!(
        "✓ Scaffolded '{}' → {} ({} files)\n\nNext:\n  cd {}\n  ./prepare.sh                         # bun build --compile → bin/{}\n  base ext install --bundle ./base-extension.toml   # local install\n  # ship cross-platform: git init + push to the [dist] repo, tag v0.1.0 → release.yml builds all 3 OSes, then `base ext add`\n  # edit index.ts to add your commands (replace the `hello` example)",
        o.name,
        o.dir.display(),
        o.files.len(),
        o.dir.display(),
        o.name,
    )
}

// ─── Templates (mirror the shipped meta/highlevel/nano-banana layout) ────────

const MANIFEST: &str = r#"# base-extension.toml — source-of-truth manifest, shipped WITH this app.
# Compiled (Bun) single-binary handler — uniform with meta/highlevel/nano-banana.

[extension]
name = "{{NAME}}"
version = "0.1.0"
description = "{{NAME}} — a BASE command-plugin"
framework_dir = "."

# Cross-platform distribution (base plugin standard — PLUGIN-DIST-SPEC.md).
# Consumed by `base ext add`: <binary>-<os>-<arch>.<ext> assets from the release.
[dist]
repo = "{{REPO}}"
version = "0.1.0"
binary = "{{NAME}}"

[[commands]]
name = "{{NAME}}"
handler = "bin/{{NAME}}"
description = "Run {{NAME}}"
usage = "base {{NAME}} <command> [flags] [--json]"
"#;

const INDEX_TS: &str = r##"#!/usr/bin/env bun
// {{NAME}} — BASE command-plugin (Bun). Scaffolded by `base ext scaffold`.
// Conformant contract: one-line JSON envelope (--json), exit 0 ok / 1 runtime / 2 args,
// clap-style --help/--version, BTree-sorted output. Replace `hello` with your commands.
// Zero npm deps: native fetch/JSON, Bun/Node built-ins.

const CLI_VERSION = "0.1.0"; // keep in sync with package.json + base-extension.toml

type J = null | boolean | number | string | J[] | { [k: string]: J };
function sortDeep(v: J): J {
  if (Array.isArray(v)) return v.map(sortDeep);
  if (v && typeof v === "object") {
    const out: { [k: string]: J } = {};
    for (const k of Object.keys(v).sort()) out[k] = sortDeep((v as any)[k]);
    return out;
  }
  return v;
}
function stable(v: J, pretty: boolean): string {
  return JSON.stringify(sortDeep(v), null, pretty ? 2 : 0);
}

class AppError extends Error {
  code: number;
  constructor(code: number, message: string) {
    super(message);
    this.code = code;
  }
  static auth(m: string) { return new AppError(2, m); }     // bad args / config
  static runtime(m: string) { return new AppError(1, m); }  // runtime / API failure
}

function emitOk(jsonMode: boolean, tool: string, data: J): void {
  if (jsonMode) process.stdout.write(stable({ ok: true, tool, data }, false) + "\n");
  else process.stdout.write(stable(data, true) + "\n");
}
function emitErr(jsonMode: boolean, tool: string, err: AppError): void {
  if (jsonMode) process.stderr.write(stable({ ok: false, tool, error: err.message }, false) + "\n");
  else process.stderr.write(`error [${tool}]: ${err.message}\n`);
}

function flag(args: string[], long: string): string | undefined {
  const i = args.indexOf(`--${long}`);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : undefined;
}

function help(): string {
  return `{{NAME}} — a BASE command-plugin

Usage: {{NAME}} <command> [flags] [--json]

Commands:
  hello   Example command — greets --name

Options:
      --json     Emit a one-line JSON envelope for programmatic callers (BASE)
  -h, --help     Print help
  -V, --version  Print version
`;
}

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  const jsonMode = argv.includes("--json");
  try {
    if (argv.includes("--version") || argv.includes("-V")) {
      process.stdout.write(`{{NAME}} ${CLI_VERSION}\n`);
      return;
    }
    if (argv.length === 0 || argv.includes("--help") || argv.includes("-h")) {
      process.stdout.write(help());
      return;
    }
    const positionals = argv.filter((a) => !a.startsWith("-"));
    const cmd = positionals[0];
    switch (cmd) {
      case "hello": {
        const name = flag(argv, "name") ?? "world";
        emitOk(jsonMode, "hello", { message: `hello, ${name}` });
        return;
      }
      default:
        throw AppError.auth(`unknown command '${cmd ?? ""}' — run \`{{NAME}} --help\``);
    }
  } catch (e) {
    const err = e instanceof AppError ? e : AppError.runtime(String((e as any)?.message ?? e));
    emitErr(jsonMode, "cli", err);
    process.exit(err.code);
  }
}

main();
"##;

const PACKAGE_JSON: &str = r#"{
  "name": "{{NAME}}-cli",
  "version": "0.1.0",
  "description": "{{NAME}} as a BASE command-plugin (Bun)",
  "type": "module",
  "bin": { "{{NAME}}": "index.ts" },
  "scripts": {
    "build": "bun build --compile ./index.ts --outfile bin/{{NAME}}"
  }
}
"#;

const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "lib": ["ESNext"],
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "types": [],
    "skipLibCheck": true,
    "strict": true,
    "noEmit": true,
    "allowImportingTsExtensions": true
  }
}
"#;

const PREPARE_SH: &str = r#"#!/usr/bin/env bash
# prepare.sh — build hook. Compiles a single self-contained binary at bin/{{NAME}}
# (the handler path) via `bun build --compile`, so `base ext install --bundle` ships
# a working binary with no runtime needed. Idempotent; run from anywhere; exit 0 ok.
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v bun >/dev/null 2>&1; then
  echo "{{NAME}} prepare: bun not found — install Bun (https://bun.sh)" >&2
  exit 1
fi
mkdir -p bin
bun build --compile ./index.ts --outfile bin/{{NAME}}
echo "{{NAME}} prepare: built $(./bin/{{NAME}} --version) → bin/{{NAME}}"
"#;

const RELEASE_YML: &str = r#"name: Release

# base cross-platform plugin-distribution standard (PLUGIN-DIST-SPEC.md). Bun
# cross-compiles ALL targets from one host, so this is ONE job (no per-OS matrix).
# Assets follow base's scheme: <binary>-<os>-<arch>.<ext>, binary in bin/.

on:
  push:
    tags:
      - 'v*'

permissions:
  contents: write

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: oven-sh/setup-bun@v2
        with:
          bun-version: latest

      - name: Build all targets + package
        run: |
          set -euo pipefail
          build() {
            local bun_target="$1" asset="$2" binname="$3"
            rm -rf staging && mkdir -p staging/bin
            bun build --compile --target="$bun_target" ./index.ts --outfile "staging/bin/$binname"
            cp base-extension.toml staging/
            [ -d commands ] && cp -r commands staging/ || true
            [ -d docs ] && cp -r docs staging/ || true
            if [[ "$asset" == *.zip ]]; then
              ( cd staging && zip -r "../$asset" . >/dev/null )
            else
              ( cd staging && tar -czf "../$asset" * )
            fi
            shasum -a 256 "$asset" > "$asset.sha256"
          }
          build bun-linux-x64    {{NAME}}-linux-x86_64.tar.gz   {{NAME}}
          build bun-darwin-arm64 {{NAME}}-darwin-aarch64.tar.gz {{NAME}}
          build bun-windows-x64  {{NAME}}-windows-x86_64.zip    {{NAME}}.exe

      - name: Create Release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            {{NAME}}-linux-x86_64.tar.gz
            {{NAME}}-linux-x86_64.tar.gz.sha256
            {{NAME}}-darwin-aarch64.tar.gz
            {{NAME}}-darwin-aarch64.tar.gz.sha256
            {{NAME}}-windows-x86_64.zip
            {{NAME}}-windows-x86_64.zip.sha256
          body: |
            ## {{NAME}} — a BASE command-plugin (Bun)

            Install via base (`base ext add`), or download the binary for your platform below.
"#;

const GITIGNORE: &str = r#"/bin
/node_modules
.env
"#;

const README: &str = r#"# {{NAME}}

A [BASE](https://chrisai.cv/skool) command-plugin (Bun). Scaffolded by `base ext scaffold`.

## Build & install (local)

```bash
./prepare.sh                                    # bun build --compile → bin/{{NAME}}
base ext install --bundle ./base-extension.toml # local, repo-independent install
base {{NAME}} hello --name you
```

## Ship cross-platform

```bash
git init && git remote add origin git@github.com:{{REPO}}.git
git add -A && git commit -m "init" && git push -u origin master
git tag v0.1.0 && git push origin v0.1.0        # release.yml builds linux/darwin/windows
base ext add ./base-extension.toml               # fetch the prebuilt binary for THIS host
```

Edit `index.ts` to add your commands (replace the `hello` example). Keep `CLI_VERSION`
in sync with `package.json` + `base-extension.toml`.
"#;
