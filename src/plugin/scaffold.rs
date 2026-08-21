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
use std::process::Command;

use anyhow::{bail, Context, Result};

/// What to do after writing the files. `--bootstrap` turns all three on for the
/// one-command, zero-step kickoff.
#[derive(Default)]
pub struct ScaffoldOpts {
    /// Exact target folder (overrides the default `<parent>/<name>-cli`).
    pub into: Option<PathBuf>,
    /// GitHub owner/repo for [dist] + `--create-repo` (default ChristopherKahler/<name>-cli).
    pub repo: Option<String>,
    /// Run prepare.sh (bun build → bin/<name>) after writing files.
    pub build: bool,
    /// git init + first commit.
    pub git: bool,
    /// gh repo create (private) + wire remote + push. Implies git.
    pub create_repo: bool,
}

/// Outcome of a scaffold, for CLI reporting.
pub struct ScaffoldOutcome {
    pub name: String,
    pub dir: PathBuf,
    pub files: Vec<String>,
    pub built: bool,
    pub git: bool,
    /// owner/repo if a GitHub repo was created + pushed.
    pub repo_pushed: Option<String>,
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

/// Scaffold a complete, buildable Bun plugin, then optionally build + git-init +
/// create+push a private GitHub repo (the fully-bootstrapped one-command kickoff).
pub fn scaffold_plugin(name: &str, parent: &Path, opts: &ScaffoldOpts) -> Result<ScaffoldOutcome> {
    if crate::plugin::is_reserved(name) {
        bail!("'{name}' is a reserved base command — pick another plugin name");
    }
    if !valid_name(name) {
        bail!("invalid plugin name '{name}' — use lowercase letters, digits and single hyphens (e.g. my-tool)");
    }
    let repo = opts.repo.clone().unwrap_or_else(|| format!("ChristopherKahler/{name}-cli"));

    // Target: --into <dir> exactly, else <parent>/<name>-cli. Must be new or empty.
    let dir = opts.into.clone().unwrap_or_else(|| parent.join(format!("{name}-cli")));
    if dir.exists() {
        let nonempty = std::fs::read_dir(&dir).map(|mut it| it.next().is_some()).unwrap_or(true);
        if nonempty {
            bail!("{} exists and is not empty — refusing to overwrite", dir.display());
        }
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

    // ── post-write bootstrap (each step fail-loud so the operator isn't left half-done) ──
    let built = if opts.build { run_prepare(&dir)? } else { false };
    let want_git = opts.git || opts.create_repo;
    let git_done = if want_git {
        git_init_commit(&dir, name)?;
        true
    } else {
        false
    };
    let repo_pushed = if opts.create_repo {
        gh_create_repo(&dir, &repo)?;
        Some(repo.clone())
    } else {
        None
    };

    Ok(ScaffoldOutcome {
        name: name.to_string(),
        dir,
        files,
        built,
        git: git_done,
        repo_pushed,
    })
}

/// Build the freshly-scaffolded plugin via prepare.sh (best-effort: a missing Bun is
/// a warning, not a failure — the files are already written).
fn run_prepare(dir: &Path) -> Result<bool> {
    let have_bun = Command::new("bun").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !have_bun {
        eprintln!("base: bun not found — skipping build (run ./prepare.sh after installing Bun: https://bun.sh)");
        return Ok(false);
    }
    let status = Command::new("bash").arg("prepare.sh").current_dir(dir).status().context("running prepare.sh")?;
    if !status.success() {
        eprintln!("base: warning — prepare.sh did not succeed; build skipped");
        return Ok(false);
    }
    Ok(true)
}

/// git init (idempotent) + add -A + commit.
fn git_init_commit(dir: &Path, name: &str) -> Result<()> {
    let git = |args: &[&str]| -> Result<bool> {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .with_context(|| format!("running `git {}` (is git installed?)", args.join(" ")))?;
        Ok(status.success())
    };
    if !dir.join(".git").exists() && !git(&["init", "-q"])? {
        bail!("git init failed");
    }
    if !git(&["add", "-A"])? {
        bail!("git add failed");
    }
    if !git(&["commit", "-q", "-m", &format!("init {name} — base ext scaffold")])? {
        bail!("git commit failed (configure git user.name / user.email, then commit)");
    }
    Ok(())
}

/// Create a PRIVATE GitHub repo, wire `origin` (SSH), and push.
///
/// We create with `gh` but push via direct `git` over SSH rather than
/// `gh repo create --push`: gh's internal push uses HTTPS, which fails when git's
/// `git-remote-https` helper isn't on PATH (e.g. a snap-confined gh). SSH uses the
/// `ssh` transport already on PATH — the operator's normal git auth.
fn gh_create_repo(dir: &Path, repo: &str) -> Result<()> {
    let created = Command::new("gh")
        .args(["repo", "create", repo, "--private"])
        .current_dir(dir)
        .status()
        .context("running `gh repo create` (is the GitHub CLI installed + authed? `gh auth login`)")?;
    if !created.success() {
        bail!("gh repo create failed — does {repo} already exist, or is `gh` unauthed?");
    }
    let git = |args: &[&str]| -> Result<bool> {
        Ok(Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .with_context(|| format!("running `git {}`", args.join(" ")))?
            .success())
    };
    let url = format!("git@github.com:{repo}.git");
    if !git(&["remote", "add", "origin", &url])? {
        bail!("git remote add origin failed");
    }
    if !git(&["push", "-u", "origin", "HEAD"])? {
        bail!("git push failed — is SSH to GitHub working? test with `ssh -T git@github.com`");
    }
    Ok(())
}

pub fn format_scaffold_human(o: &ScaffoldOutcome) -> String {
    let mut s = format!("✓ Scaffolded '{}' → {} ({} files)", o.name, o.dir.display(), o.files.len());
    if o.built {
        s.push_str(&format!("\n  built → bin/{}", o.name));
    }
    if o.git {
        s.push_str("\n  git → initialized + committed");
    }
    if let Some(repo) = &o.repo_pushed {
        s.push_str(&format!("\n  github → created (private) + pushed → {repo}"));
    }
    s.push_str("\n\nNext:");
    s.push_str(&format!("\n  cd {}", o.dir.display()));
    if !o.built {
        s.push_str(&format!("\n  ./prepare.sh                                   # bun build → bin/{}", o.name));
    }
    s.push_str("\n  # edit index.ts — replace the `hello` example with your commands");
    s.push_str("\n  base ext install --bundle ./base-extension.toml  # local install");
    if o.repo_pushed.is_some() {
        s.push_str("\n  git tag v0.1.0 && git push origin v0.1.0          # release.yml builds linux/darwin/windows");
        s.push_str("\n  base ext add ./base-extension.toml                # then fetch the prebuilt binary cross-platform");
    } else {
        s.push_str("\n  # ship cross-platform: re-run with --bootstrap, or push to the [dist] repo + tag v0.1.0, then `base ext add`");
    }
    s
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

A [BASE](https://www.skool.com/claude-code-titans-9203) command-plugin (Bun). Scaffolded by `base ext scaffold`.

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
