---
type: build-spec
status: complete
tags: [base-v2, windows, cross-platform, fork]
relatedTo: [base-v2, _ops, INSTALL-REPORT-windows]
---

# Fork: base Windows-compat fixes

> **Resolved 2026-06-26** on branch `fork/windows-compat`.
> - **P0** — `rewrite_framework_dir` now emits paths via `toml_basic_string()` (TOML-spec escaping); fixes all three install paths (`ext add`/bundle/linked) since they share that helper. Added a write→re-parse round-trip test for a `C:\Users\…` path.
> - **P1** — Prebuilt `base-windows-x86_64.zip` already ships from `release.yml` (the highest-leverage fix). Added `scripts/build-base-windows.ps1` (imports vcvars, resolves libclang, `cargo install`) for from-source builds + a README Windows section.
> - **P2** — `base sync --ast` resolves the interpreter via `multimodal::python_bin()` (`python` first on Windows, `python3` on Unix). Shipped `scripts/ast/requirements.txt`; `base install` best-effort `pip install -r`s it. Release packaging now bundles `requirements.txt`.
> - **P3** — `install_scripts` reports ✓ when the extractor is already present at `~/.base-gbl/scripts/ast` instead of warning "not found near binary".
>
> Verified: `cargo check`, `cargo clippy` (no new warnings), full lib suite (219 passed), PowerShell parse-check.

Source: `_ops` first-run field report on Windows 11 (LAPTOP-DPF051SL), 2026-06-26.
Full report lives in the `ops` repo → `INSTALL-REPORT-windows-2026-06-26.md`.
The machine got fully working, but on two hand-fixes that revert if base isn't fixed.

## P0 — `base ext add` writes invalid TOML on Windows
Serializes a Windows path into a TOML **basic string**:
`framework_dir = "C:\Users\..."` → `\U` (in `\Users`) parsed as a unicode escape →
invalid TOML → the extension **silently fails to load**. Recurs on every Windows `base ext add`.
**Fix:** when writing a path into TOML, use forward slashes, escape backslashes, or emit a
TOML **literal string** (`'...'`). Add a write→re-parse round-trip test.

## P1 — Windows build of `base` is undocumented + heavy
cargo build needs: MSVC C++ build tools + Windows SDK, LLVM/libclang (bindgen/RocksDB),
and must run **inside** the MSVC dev env (`vcvars64` → `INCLUDE`/`LIB` set). None in PREFLIGHT.
**Fix (pick one):**
- Ship a prebuilt `base-windows-x86_64` release asset — skips the native RocksDB build entirely (highest leverage), **or**
- `scripts/build-base-windows.ps1`: import vcvars, set `LIBCLANG_PATH`, prepend `~/.cargo/bin`, run the cargo install.

## P2 — `base sync --ast` on Windows
- base invokes `python3`; Windows ships only `python` (bare `python3` hits the Store stub).
  **Fix:** probe `python` then `python3`, or make it configurable in `base.toml` (`ast.python`).
- Extractor needs `tree-sitter` + grammars. **Fix:** ship `scripts/ast/requirements.txt`; installer `pip install -r`.

## P3 — `base install` AST script location
Warns `scripts/ast/ not found near binary` though they ship in `~/.base-gbl/scripts/ast`.
**Fix:** resolve from `~/.base-gbl/scripts/ast` and report ✓.

## Acceptance
A fresh Windows box runs `ops install --all` → `base install` → `base sync --ast`
one-shot with zero manual steps (or one documented `build-base-windows.ps1`).
