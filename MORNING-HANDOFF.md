---
type: handoff
status: active
tags: [base-v2, v0.6, command-plugins, nano-banana, handoff]
relatedTo: [base-v2, nano-banana]
---

# Morning Handoff — v0.6 Command Plugins + nano-banana (2026-06-23)

Built overnight, autonomously, full PAUL loop. Everything's done and waiting on one thing: **your API key.**

## TL;DR — do this first

```bash
# 1. Add your Gemini key to the base secret store:
echo 'GEMINI_API_KEY=AIza...your-real-key...' >> ~/.base-gbl/.env

# 2. Generate a real image through base (the drop-in you wanted to test):
base nano-banana generate --prompt "emerald HUD command center, dark warm-neutral bg" --ratio 16:9 --size 2K --json

# 3. It writes a PNG to ./generated_imgs/ and prints {"ok":true,"images":[{path,…}]}
```

If that produces an image, the whole thing works. That's the only unverified step (I couldn't test a real generation without your key).

## What got built

**base v0.6 — drop-in command plugins.** A 4th extensibility layer. Any `base <foo>` that isn't a core command is routed (clap `external_subcommand`) to a handler declared by an extension's `[[commands]]` section, with args forwarded and an env contract injected. Core commands always win — a plugin can't shadow a built-in.

- 5 phases (38–42), each built + tested + recorded in `.paul/`. 14 new plugin tests, all green.
- The release binary is **already swapped into `~/.local/bin/base`** — the seam is live on PATH right now.
- Full contract: `docs/command-plugins.md`.

**nano-banana — the reference plugin** (`apps/nano-banana-studio/nanobanana-cli/`):
- `bin/nano-banana.mjs` — spec-faithful CLI (`generate`/`edit`, model/ratio/size/n/out/json/ground, input sandbox, retries, `--json`, exit 0/1/2). Built per your CLI-SPEC.md + REFACTOR-MAP.md.
- `@google/genai` installed; SDK call shape verified (no network call made).
- Extension manifest: `~/.base-gbl/extensions/nano-banana.toml`.

## The `~/.base-gbl/.env` convention (as you specified)

All base-framework API keys live in `~/.base-gbl/.env`. base loads it and injects every `KEY=VALUE` into **every plugin process's environment** (dotenv semantics — an exported var wins, `.env` never clobbers). So the nano-banana CLI just reads `GEMINI_API_KEY` from env. I created the file with a commented placeholder; add your real key.

This works for any future plugin — drop secrets in that one file.

## What I verified (without your key)

- `base ext list` shows the `nano-banana` plugin command + source extension.
- `base nano-banana --help` routes through dispatch → exec → handler.
- `base nano-banana generate --prompt x --json` → full chain → clean `{"ok":false,"error":"missing GEMINI_API_KEY"}` exit 2 (proves seam→registry→exec→.env→handler→stdout→exit-code propagation).
- `base ext run nano-banana …` (explicit, collision-proof path).
- All arg-validation paths (bad model/size, `--ground` without 3-pro, missing `--image`, missing `--prompt`).
- SDK surface: `ai.models.generateContent` is callable.

## Decisions I held for you (not done autonomously)

1. **No commit / tag.** Binary is 0.6.1 in the working tree; the milestone is uncommitted. Release gesture (version bump to a clean v0.6.x or v0.7.0, commit, tag) is your call.
2. **Verify** is your `/paul:verify` to run if you want the formal UAT gate — I verified by build+test+e2e instead since you weren't here to drive manual UAT.

## One pre-existing bug I found but did NOT fix

`store::tests::snapshot_rotates_to_keep_limit` is flaky (non-deterministic, mtime-ordering in v0.5 snapshot-rotation code — `src/store.rs`, untouched by v0.6). It passes alone, intermittently fails among siblings. I left it: I won't edit fidelity-critical durability code as an overnight side effect. Logged in STATE.md → Deferred Issues. Worth a dedicated deterministic-clock fix.

## Try another plugin command later

The mechanism is generic. To add any `base <foo>`: write an executable handler, declare it in an extension TOML's `[[commands]]`, `base ext install`. See `docs/command-plugins.md` → authoring checklist.

## Quick reference

| Thing | Path |
|---|---|
| Plugin module | `apps/base-v2/src/plugin/mod.rs` |
| Manifest schema | `apps/base-v2/src/extension/mod.rs` (`CommandSpec`) |
| Contract doc | `apps/base-v2/docs/command-plugins.md` |
| nano-banana CLI | `apps/nano-banana-studio/nanobanana-cli/bin/nano-banana.mjs` |
| nano-banana manifest | `~/.base-gbl/extensions/nano-banana.toml` |
| Secret store | `~/.base-gbl/.env` |
| PAUL record | `apps/base-v2/.paul/phases/38..42/*-SUMMARY.md` |
