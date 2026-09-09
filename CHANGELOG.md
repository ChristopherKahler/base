# Changelog

Every released version of base, newest first. Each entry is generated from the commits
between that release's tag and the one before it, by `scripts/changelog.py`.
`scripts/release.sh` writes the top section when it cuts a release, and `cargo test`
fails when the version in `Cargo.toml` has no entry here.

Releases before 0.13.3 are tagged in the repository but are not written up.

## 0.15.0 (2026-09-09)

### Fixed

- **store**: retry the atomic rename, and stop discarding its error ([#127](https://github.com/ChristopherKahler/base/issues/127)) ([#155](https://github.com/ChristopherKahler/base/issues/155))
- **ast**: read a PHP helper key in every plain quote style ([#118](https://github.com/ChristopherKahler/base/issues/118)) ([#154](https://github.com/ChristopherKahler/base/issues/154))
- **doctor**: report quads belonging to another workspace instead of passing the tier HEALTHY ([#142](https://github.com/ChristopherKahler/base/issues/142)) ([#153](https://github.com/ChristopherKahler/base/issues/153))
- **ast**: declare each IRI exactly once in the map ([#98](https://github.com/ChristopherKahler/base/issues/98)) ([#152](https://github.com/ChristopherKahler/base/issues/152))
- **windows**: run the CLI on an 8 MB stack so a debug base.exe reaches product code ([#151](https://github.com/ChristopherKahler/base/issues/151))
- **frontmatter**: the documented `related` key produces edges, on both paths ([#150](https://github.com/ChristopherKahler/base/issues/150))
- **ast**: ast query returns non-symbols as symbols ([#148](https://github.com/ChristopherKahler/base/issues/148))
- **ast**: a read never crosses out of the app it started in ([#147](https://github.com/ChristopherKahler/base/issues/147))
- **commands**: star commands activate only at an invocation position, and replies render the token on its own line ([#145](https://github.com/ChristopherKahler/base/issues/145)) ([#101](https://github.com/ChristopherKahler/base/issues/101))
- **ast**: file membership is transitive over containment, and the walk is seed-order deterministic ([#144](https://github.com/ChristopherKahler/base/issues/144)) ([#82](https://github.com/ChristopherKahler/base/issues/82))
- **[#87](https://github.com/ChristopherKahler/base/issues/87)**: route nine graph writers plus a tenth site through the lock seam ([#139](https://github.com/ChristopherKahler/base/issues/139))
- **install**: [#93](https://github.com/ChristopherKahler/base/issues/93) permanently-inert install, [#92](https://github.com/ChristopherKahler/base/issues/92) scripts/ast beside the binary ([#136](https://github.com/ChristopherKahler/base/issues/136))
- **rule**: `rule remove` removes what `rule list` showed, and reports the count it read back ([#112](https://github.com/ChristopherKahler/base/issues/112)) ([#140](https://github.com/ChristopherKahler/base/issues/140))
- **ast**: one relation vocabulary shared by the extractor and the serializer ([#107](https://github.com/ChristopherKahler/base/issues/107)) ([#130](https://github.com/ChristopherKahler/base/issues/130)) ([#98](https://github.com/ChristopherKahler/base/issues/98), [#105](https://github.com/ChristopherKahler/base/issues/105), [#82](https://github.com/ChristopherKahler/base/issues/82))

### Changed

- **python-ast-tests**: count TESTS, not files ([#149](https://github.com/ChristopherKahler/base/issues/149))
- drop the rocksdb default feature from oxigraph ([#117](https://github.com/ChristopherKahler/base/issues/117)) ([#137](https://github.com/ChristopherKahler/base/issues/137))
- --no-fail-fast on the trunk suite ([#135](https://github.com/ChristopherKahler/base/issues/135))
- Run the Python AST tests in CI, blocking ([#122](https://github.com/ChristopherKahler/base/issues/122))
- Remove the PolyForm license file from the repo and the release archives ([#123](https://github.com/ChristopherKahler/base/issues/123))
- The Windows lint gate: three cfg-dead items, and clippy becomes a required check ([#119](https://github.com/ChristopherKahler/base/issues/119))

## 0.14.2 (2026-09-08)

**base can be used at work.** Every release up to and including 0.14.1 shipped under the PolyForm Noncommercial License 1.0.0, which permitted noncommercial use only, so a company running base internally, the audience it is built for, sat outside the grant. From this release the engine is licensed under the Functional Source License 1.1 with an Apache 2.0 future license (FSL-1.1-ALv2). You may run base inside a company of any size, modify it, and build and sell products that use it. The one thing withheld is selling base itself, or a service that stands in for base or for basemode. Each version becomes Apache 2.0 two years after it ships; that grant is irrevocable and written into the license text. Extensions and adapters written against base's documented interfaces (the extension manifest and API, command plugins, the hook protocol, the CLI surface, the on-disk graph format) are their author's own work under an explicit exception and may be licensed any way the author likes, including commercially. Versions already released keep the terms they shipped with; nothing is withdrawn. `LICENSING.md` carries the plain-English guide and the surface-by-surface table, and `Cargo.toml` now declares the SPDX identifier.

**Every file the extractor parses is now a file in the map.** Two lists decided this and they had drifted apart: the extractor parsed 66 extensions, the serialiser recognised 27 of them as files, and the 39 in the gap — `.mjs`, `.cjs`, `.ps1`, `.vue`, `.md`, `.mdx`, `.json`, `.hpp`, `.cc`, `.cxx`, `.kts`, `.m`, `.mm`, `.groovy`, `.gradle`, `.bash`, `.luau`, every Fortran and Pascal spelling — were parsed, reached the graph, and had every one of their entities attributed to the app root instead of the file they came from. Line numbers stayed right, so `base ast query` answered with a real line in the wrong file, and `--file src/thing.mjs` found nothing at all. On the tree that reported it, 6,495 of 21,979 entities sat on the project root; on a Windows tree here, 293 of 2,807. The two lists cannot drift again: in a whole-app extraction a node is a file when it is one of the files that were walked, which needs no list, and the extension set that covers single-file mode is now derived from the parser table rather than typed out beside it. Reported by @Elevation-mindset, who measured it, found the cause, and offered the patch.

`.cjs` is parsed at all now — it was in none of the three lists — and `.hpp` gains one more thing: it was already marked as a language whose records are structs, but never as a file, so a struct in a C++ header could not be typed as one. It can be. A map built before this release keeps its old paths until it is rebuilt; `base sync --ast --target <app>` does that in seconds, because the per-file extraction cache is still valid and only the serialisation step re-runs, and the Stop hook does it on its own the next time you work in that app unless the tree is over the size fuse — in which case run the command yourself. Nothing re-indexes automatically; the 0.14.1 fuse stands.

**`.baseignore` directory patterns work on Windows.** A pattern is written with forward slashes and the path it was compared against was backslash-separated, so `archive/` never matched `archive\deep\thing.ts` and 139 files under an ignored directory were collected anyway. Both sides are normalised now, the way base already normalises paths everywhere else. Only patterns carrying a slash were affected — a bare `archive` and a glob like `*.log` matched on both platforms all along — and POSIX behaviour is unchanged.

**The extractor says when it could not place an entity.** A run that attributes entities to the app root because it found no file node for them now prints how many, every time rather than only in devmode, and a run with nothing to report prints nothing new. The number counts attributions rather than distinct nodes, and the two can differ: a symbol the extractor synthesises for something outside the tree, an inherited base class from the standard library say, is emitted once per place it is referenced and collapses to one node in the map, so a count of 89 can be 85 nodes. The count is the right tripwire either way — it is the number of times the fallback was taken. This is the tripwire that would have caught the drift above months ago. `base sync --ast --yes` captured the extractor's output so that a failed build could explain itself later, and on a successful build threw it away, including every "skipped this file" line. Those come back, on a sync you run yourself. A refresh started by a hook still prints to nobody: it is deliberately detached with its output discarded, and giving it a voice means writing these notices down for session start to read, which is not this release.

**The sync line says what it counted.** `base domain sync`, and the same line printed by `base scaffold` and `base install`, reported `N rules` for the rules it had imported from `domains.toml` (and from `carl.json` when one was named). That read as the domain's rule count, so a domain with five rules added by `base rule add` looked empty: sync never touches CLI rules, by design, and then printed a number that looked as if it had counted them, the same reading that produced #38 from `base domain get`. The line now says `M rules imported from domains.toml`, adds `and carl.json` and `K decisions imported from carl.json` only when a carl.json was named, and reads the same on all three prints; nothing about what sync counts changed, and `base domain get` still counts every rule. **A record the domain block listed is not listed again.** The `[X CONTEXT]` block served a domain's decisions and projects and the prompt-time walk then served the same records under `<base-context>` in the same prompt, because the block never marked them as served: the query did not project the record it read, and the list it kept was in a form the walk could never match. Both are fixed. A name the block listed still walks, because the block served its row and not what is attached to it, and it is never listed as its own record.

The base-help coach now answers the questions the 0.14.1 injection changes raised — keeping a domain out of automatic injection, how a path trigger matches, why a domain stopped loading and what `base doctor` says about it, why an add-trigger was refused, and `exclude` on an always-on domain — and the three pairs that described the pre-0.14.1 domain tier split as a live bug now say it was fixed.

**The release download carries the license.** Every asset up to and including 0.14.1 packed the binary and `scripts/` and nothing else, so the first copy of base anyone receives arrived with no license text in it, while FSL-1.1-ALv2 asks that copies carry the terms or a link to them. The three tarballs and the Windows zip now also carry `LICENSE.md`, `LICENSING.md`, `LICENSE-APACHE-2.0` and the retained `LICENSE-PolyForm-Noncommercial-1.0.0.md`, which is the grant those earlier releases shipped under and is kept so that a download can be compared against the terms it replaced. Nothing about an install changes: `base update` looks for the binary and for `scripts/` by name and ignores everything beside them. The packing step is now read out of the release workflow and run by CI on every pull request, which is what would have caught this: the two archives are built differently, `tar` packs the members it is named while the Windows zip globs its staging directory, so a fix applied to one of them can leave the other three assets exactly as they were.

**base installs in one line, and needs no toolchain to do it.** The front door in the README was `cargo build --release`, and on Windows that additionally meant the MSVC developer environment plus LLVM and libclang, because the vendored RocksDB tree is built through bindgen. Binaries for all four platforms were published on every release and almost nobody found them: v0.14.1's four assets were downloaded 1, 2, 4 and 6 times, and across all 59 releases 1,068 times in total. `install.sh` and `install.ps1` now detect the platform, pull the matching asset from the latest release, unpack it, and hand off to `base install`, which does exactly what it always did. The one-liner sits above the fold; the source build keeps its own section, unchanged, for anyone who wants it. `BASE_VERSION` pins a tag, arguments pass straight through to `base install`, and `linux-aarch64` — which has no published asset — says so rather than failing on a download.

Running the installer end to end on a machine that had never had base turned up three things the binary does on a clean install that it does not do on yours, filed as #91, #92 and #93. The installers work around all three for now: they run the unpacked binary from inside its own directory so the AST extractor is found, they write `base.exe` alongside `base` so Windows can run it by name, and they say to rerun `base install` when there was no `~/.claude` to wire hooks into. The fixes belong in the binary and are landing separately; the workarounds come out when they do.

**A handoff or fork you registered stays registered.** Until now two `base` processes writing the graph at the same moment silently destroyed each other's work. Every write loads the whole graph, changes it in memory, and writes the whole file back, and nothing made those writers take turns — so the one that finished second overwrote the first, and both printed success. That is what was behind rows that came back as `open` a minute after you archived them, an archive that reported `archived` and changed nothing, and a fork that vanished from `fork list` in every state right after it was created (#72, #73, #74 — one defect, not three). It was never rare: eight `fork create` commands starting together left three of eight rows on Windows and two of eight on Linux, with no error anywhere. Writers now take a lock on the graph file, one tier at a time, and readers never wait for it — `base recall` and the hooks are as fast as they were. The writers that take it are the ones you use: everything behind `base learn`, `task`, `project`, `decision`, `rule`, `note`, `milestone`, `goal`, `reminder` and `relay ping`, the handoff and fork commands, `supersede`, `graph purge`, workspace reconcile, and the hook that runs on every tool call. The dashboard's own writes are not locked and will not be: it is being deprecated. A handful of bulk operations (`base sync`, `graph compact`, `doctor --repair`, the migrations) are not locked yet either.

**`archive` and `snooze` tell you what changed, and say so when nothing did.** Archiving a slug that exists in neither tier used to print `archived` and exit 0. Now each command names the tier it changed, and a slug nothing holds is an error naming the slug and both places that were searched. A `-g` archive also stopped doing its work twice on the same file.

**`handoff create` says which handoff it closed.** Registering a handoff archives the previous open one for that project in the tier you are standing in — that has always been true, and the help text now says "in this tier" instead of promising something project-wide. What is new is that it tells you: the handoff it archived, or that there was none, and, when the other tier is holding an open handoff for the same project, its name and the exact command that would archive it. It still does not touch the other tier; a write acts on the tier you stand in.

**Every language base parses now has a name, and a component's script block is read.** Three lists decided what the extractor does with a file and only two of them were joined up in this release's first pass. `LANG_MAP`, which fills `ops:language`, was still typed out by hand: 22 extensions were parsed and reached the graph as `unknown` — `.mjs`, `.cjs`, `.vue`, `.mdx`, `.kts`, `.gradle`, `.luau`, every capitalised Fortran spelling, every Delphi and Lazarus form — while `.psm1` and `.zsh` were listed as PowerShell and shell and were never parsed at all. The map is derived from the parser table now, the way the file-node set already is: a language belongs to an extractor rather than to an extension, and an extractor with no language stops the extractor at import instead of quietly filling the graph with `unknown`. `.psm1` and `.zsh` are parsed for real rather than delisted, since the extractors for both were already there. Single-file components are the other half. A `.vue` or `.svelte` file wraps its code in `<script>` tags, the JavaScript grammar cannot read the markup around them, and what survived was decided by tree-sitter's error recovery: two files with identical script contents, one `.vue` and one `.svelte`, gave a function and nothing, and two `.vue` files differing by a single `export let` line did the same. The under-report was silent — the file appeared in the map with its functions missing. The script block is now isolated before parsing, with the markup blanked rather than removed so every line number still points where it did, and both extensions read the same way. A file that is plain JavaScript under a `.vue` name is unaffected. `.astro` has the same shape and is not covered here; it is tracked in #850.

**A code map built in the background can finally say what it could not do.** Every automatic refresh discards its child's output on purpose — the Stop hook, a first contact, the WSL delegate, and the git hook all do, because nobody is watching a detached process — so the counter above, and every "skipped this file" line, reached a person only on a sync they ran themselves. A tree could collect unreadable files for weeks and never mention it. Those notices are now written next to the map in `.base-ast/.last-notices`, exactly the way a failed build already leaves `.last-error`, and session start reads them back in one line. All four background paths run the same command, so the child records them whoever started it. A run with nothing to report deletes the file rather than leaving the last complaint standing, the same set is not repeated once it has been shown, and a changed one is shown again.

**Hooks say things the model can now actually read.** Claude Code adds a hook's plain stdout to the model's context on four events, and `PostToolUse` is not one of them — everywhere else stdout is written to the transcript and dropped. base already knew this and applied it to `PreToolUse` only, so the section AST context printed after a partial read, every extension `inject` nudge, and both directory-move blocks were written, logged as delivered, and never seen. All four now travel in the `hookSpecificOutput.additionalContext` envelope. `session-start` and `user-prompt-submit`, where the host does deliver plain stdout, are unchanged. Two fields the log had been keeping to itself, `nudged` and `standards_injected`, are now written out, because an extension nudge that fires and is recorded nowhere is invisible in both directions at once.

**The end-of-turn relay nudge is gone rather than pretending.** `Stop` has no channel that reaches the model at all, and the nudge was redundant besides: the same open task is re-announced the moment the session moves again. Printing it was a no-op that read as a clean pass. The block now goes out as `systemMessage`, which reaches the operator, and the operator is who it was useful to.

**A hook event is logged in the workspace it happened in.** The line recorded the cwd the host reported and then wrote itself to the tier of whatever directory the hook process was standing in, so an event could name workspace A and sit in workspace B — and `base doctor`'s hook trail, which reads one tier, reported on the wrong one. A failing hook in A was invisible from A. The tier now comes from the payload on both the success and the failure arm, and each line carries `cwd_source` saying which input chose it.

**Two pings in the same millisecond no longer overwrite each other.** The slug was `ping-<millis>`, and it names both the inbox alert file and the graph IRI, where the write is a delete followed by an insert at a fixed address. So the earlier ping was replaced, with nothing on either side to show it had existed, while its sender saw a success. Eight pings fired at once left three. The slug now carries a per-process counter and the process id, because neither alone is enough: a counter cannot separate two processes and a pid cannot separate two pings from one.

**Scripts check out runnable on Windows.** Git for Windows turns on `core.autocrlf` by default and the repo shipped no `.gitattributes`, so every `.sh` arrived with CRLF endings including the shebang, and running one directly failed with `/usr/bin/env: 'bash\r': Permission denied`. It stayed hidden because CI runs on Linux, where the files are unaffected, and because `bash script.sh` ignores the shebang entirely. `.sh` and `.py` are now pinned to LF and `.ps1` to CRLF — pinned rather than left alone, so no script has one identity on Windows and another on Linux. An existing Windows clone needs `git add --renormalize .` or a fresh checkout to pick this up; a `git pull` alone will leave its files exactly as they are.

**Windows installs are runnable again.** `base install` wrote the binary to `~/.local/bin/base` with no extension, so a machine that had never run base before finished the install, printed correct PATH guidance, and then could not run `base` at all — Windows resolves executables through PATHEXT, and an extensionless file is not on it. Existing installs were unaffected because the update path already handled both names, which is why this only ever bit a first install. The binary's name now comes from one seam that carries the platform suffix. Install writes every name that seam lists, uninstall removes every one of them and, when it finds none, names the directory it searched and each name it looked for; the manifest records the name it actually wrote; and a guard test fails the build if any site outside the seam ever spells the name by hand.

### Added

- one-line installer for base ([#90](https://github.com/ChristopherKahler/base/issues/90)) ([#71](https://github.com/ChristopherKahler/base/issues/71))

### Fixed

- **install**: step 1 prints on one line like steps 2-7 ([#110](https://github.com/ChristopherKahler/base/issues/110))
- **verification**: silent_no_ops_0142.sh could pass on a control it never read ([#109](https://github.com/ChristopherKahler/base/issues/109))
- **install**: write the binary under every name Windows needs ([#91](https://github.com/ChristopherKahler/base/issues/91)) ([#103](https://github.com/ChristopherKahler/base/issues/103))
- **hooks**: the silent no-ops - output channels, log tier, ping slug, CRLF ([#75](https://github.com/ChristopherKahler/base/issues/75) [#76](https://github.com/ChristopherKahler/base/issues/76) [#77](https://github.com/ChristopherKahler/base/issues/77) [#86](https://github.com/ChristopherKahler/base/issues/86) [#95](https://github.com/ChristopherKahler/base/issues/95)) ([#102](https://github.com/ChristopherKahler/base/issues/102))
- **ast**: every parsed extension has a language, SFCs yield their functions, notices reach session start ([#99](https://github.com/ChristopherKahler/base/issues/99)) ([#83](https://github.com/ChristopherKahler/base/issues/83), [#84](https://github.com/ChristopherKahler/base/issues/84), [#89](https://github.com/ChristopherKahler/base/issues/89))
- **registry**: serialize graph writes, and make archive/create say what they changed ([#88](https://github.com/ChristopherKahler/base/issues/88)) ([#72](https://github.com/ChristopherKahler/base/issues/72), [#73](https://github.com/ChristopherKahler/base/issues/73), [#74](https://github.com/ChristopherKahler/base/issues/74))
- **ast**: every parsed extension is a file node, .baseignore dirs match on Windows ([#66](https://github.com/ChristopherKahler/base/issues/66)) ([#81](https://github.com/ChristopherKahler/base/issues/81))

### Changed

- **changelog**: the 0.14.2 tripwire paragraph ships once, not three times ([#106](https://github.com/ChristopherKahler/base/issues/106))
- Ship the license files inside the release assets ([#94](https://github.com/ChristopherKahler/base/issues/94)) ([#80](https://github.com/ChristopherKahler/base/issues/80))
- Serve each record once, say what sync imported, and correct the coach ([#79](https://github.com/ChristopherKahler/base/issues/79)) ([#62](https://github.com/ChristopherKahler/base/issues/62), [#65](https://github.com/ChristopherKahler/base/issues/65), [#70](https://github.com/ChristopherKahler/base/issues/70))
- **license**: relicense engine to FSL-1.1-ALv2, add extension exception ([#78](https://github.com/ChristopherKahler/base/issues/78))

## 0.14.1 (2026-09-07)

**Corrections now retire what they correct.** Until now a correction was stored beside the record it corrected, both came back from `recall` together, and nothing said which one was live — the drift that base exists to stop, sitting in base's own store. `base learn`, `base decision log` and `base rule add` take `--supersedes <slug>`, and `base graph supersede <old> <new>` records it for two records that already exist. The write refuses a slug that matches nothing, a slug that matches more than one record (naming every candidate rather than guessing), a record superseding itself, and any link that would close a cycle; every refusal happens before anything is written, so a refused correction leaves the store exactly as it was. The new record and both edges land in one write.

Surfaces that ANSWER a question now answer with the live version: `base recall`, `base context`, the prompt-time injection, `base rule list` and the domain block a session injects all serve the current record and not the one it replaced; `base rule list --include-superseded` shows the rest, each marked. Surfaces that DESCRIBE the graph keep everything: `graph analyze`, `neighbors`, `path`, `get-node`, the dashboard and the LOGOS export still show the superseded record and the edge that retired it, because that record is the evidence the correction happened — `graph get-node` marks it `[superseded]` and names the live head. `base recall --include-superseded` shows the whole chain. `graph purge --stale` never deletes either end of one. `base doctor` reports how many records are superseded, any chain longer than three links, any cycle, any disagreement between the edge and the `status` field, and how many corrections name nothing they correct.

Nothing is superseded until you say so, and a store that never uses the feature prints and serves exactly what it did before.

**A session start no longer rewrites the store to change nothing.** The PAUL ingest refreshed every scanned project's `updatedAt` on every run and wrote the whole graph back. That moved the store's identity, which re-opened the domain backfill's delta gate, which bought a full re-plan that was then discarded because there was nothing to plan. Measured on a 14 MB store: eighteen session starts, eighteen changelog entries carrying the same four timestamp quads, and no delta work to show for any of them. A project is re-stamped now only when something about it actually changed.

The cause was a duplicate scan, and it is fixed where it happened. A `paul.toml` reachable by two paths - a junction or a symlink sitting beside the real directory - was returned twice, so the ingest ran twice for one project and the two passes overwrote each other's recorded path on every run. One project is one entry now, whichever path reaches it first, and a duplicate is reported once when devmode is on.

**Domain and rule commands act on the tier you are standing in, and say which one.** `base domain remove`, `create` and `remove-trigger` wrote to the global `domains.toml` wherever you ran them, so a domain created inside a workspace could not be removed from it, `create` reported success for a domain it had written somewhere else, and `remove-trigger` reported success while editing a same-named global domain and leaving yours in place. One resolver now decides which file every write touches, every write reports what actually changed, and a command that changed nothing names the tier it searched and the one it did not, then exits non-zero instead of printing success. `base rule list` prints both tiers by default, labelled, each numbered the way `base rule remove --index` counts them, because the prompt-time block merges the two and renumbers them into an order no command used to print; `--global` keeps the single-tier view for scripts. `base domain list` and `base domain get` count both tiers as well. A tier whose graph file does not exist yet reads as empty instead of failing, so a fresh tier can take its first rule.

**Prompt-time injection is scoped to this session and driven by the prompt.** Until now a path trigger matched against every path the store had ever seen active — every registered project, task and handoff, 923 of them on one real store, the same list on every prompt — and matched by substring, so a trigger named `Documents` fired on prompt 3 of every session whatever was typed, and seven domain blocks reached a prompt that said `running a demo!`. Two of those blocks held live negotiation terms. A path trigger now matches only the files this session has touched through the tool hooks, plus the session's cwd, and matches by path prefix: a trigger resolves against the tier it came from (home for the global file, the workspace for the workspace file; `~` and absolute paths as written), compares component by component, and on a Windows path compares case-blind, so `Documents` never covers `MyDocuments` or `Documents-old` and `tools` and `Tools` are one directory. Keywords match whole words, so `base` no longer fires on `database`. An `exclude` now vetoes an always-on domain, which it silently failed to do. A context block whose only row is the domain's own project is not emitted. With devmode on, the reason line names the file that satisfied a path trigger.

**A domain can say it is never injected automatically.** `auto_inject = false` in `domains.toml` keeps a domain out of the prompt hook, the tool hook and the session-start cheat-sheet, whatever its mode or triggers; `base context`, `base recall` and star commands still see it, because you asked. The key is ignored by 0.14.0, and the 0.14.0 `add-trigger` round-trip drops keys it does not know, so install this release on every machine that shares the file before you set the flag, not after.

**A path trigger that covers two or more registered projects is a broadcast, and a broadcast cannot fire.** `Documents` on a home where nine projects live under Documents is not a trigger for any of them. Such a trigger is inert in both hooks; `base doctor` reports it per tier with the domain, the trigger and the projects it covers, and counts it against the verdict, because an inert trigger is a domain that silently stopped loading and the fix is one line in `domains.toml`; `base domain add-trigger --path` refuses one with the same sentence, exits non-zero and writes nothing; `base project add` under such a path registers the project, creates no domain, and says why. A project registered in both tiers counts once. Devmode prints one `inert:` line per faulty trigger on every prompt. An unrooted trigger — a glob, or a relative path in a file with no tier root — is inert for the same reason and named the same way.

**The dashboard no longer overwrites what you wrote from the CLI.** It loaded the graph once at startup and serialised that snapshot back over `graph.nq` on every write, so anything `base learn`, `base decision log` or `base rule add` had written since the server started was erased by the next click. Six of seven writes were lost in the session that reported it. Every dashboard read and write now compares each graph file's identity on disk with the one it was loaded from and reloads first when they differ, so a CLI write made while the dashboard is open survives. `base graph reload` stays as the manual escape hatch.

**A map that is too big to refresh unattended is not refreshed unattended.** The size fuse only ran when a map was built for the first time, so an existing map licensed a full refresh on every Stop hook however large the tree was, and the fuse counted source files by code extension, which meant a markdown-only workspace was never protected even on first contact. A 25,166-file docs tree re-indexed for ten minutes after every turn. Files the extractor turns into entities now count toward the fuse, markdown included; the fuse applies to refreshes as well as first builds, reading the counts recorded at the last measure rather than walking the tree again; and a refusal is written down and named once at session start with the command to run by hand.

**`hook-events.jsonl` is bounded by the thing that writes it.** The only trim lived in the dashboard's startup path, so an install that never opened the dashboard grew the file forever. The writer now trims to its last 5,000 lines when the file is over 10 MiB, before it appends, and the dashboard's rotation calls the same code.

**A hook that failed leaves a trace you can find.** Hooks fail open on purpose, so a broken one is silent by design and looks exactly like a quiet one. Failure lines now carry the error text, `base doctor` reports how many hooks failed in the recent window and what the last error was, and a session whose hooks are failing prints one line saying so. A hook whose most recent run failed is failing now and makes `doctor` exit non-zero; a failure with successes after it is reported as history and does not hold `doctor` red. A healthy install prints nothing new.

**Outside a workspace, every reader says so.** `recall` printed "No results found", `project list` exited 1 with an error, and `doctor` and `commands list` quietly answered from the global tier alone: four behaviours for one situation, none of which said the workspace was missing rather than empty. All four now print one line naming the tier they searched and how to make a workspace.

### Added

- **supersede**: drift has a mechanism, ops:supersedes gets a writer and every reader honours it ([#59](https://github.com/ChristopherKahler/base/issues/59))

### Fixed

- **hooks,dashboard**: [#41](https://github.com/ChristopherKahler/base/issues/41) [#40](https://github.com/ChristopherKahler/base/issues/40) [#22](https://github.com/ChristopherKahler/base/issues/22) [#20](https://github.com/ChristopherKahler/base/issues/20) [#19](https://github.com/ChristopherKahler/base/issues/19), the dashboard keeps CLI writes, the fuse guards refreshes, the hook log caps itself, doctor names a broken hook, readers name the tier ([#69](https://github.com/ChristopherKahler/base/issues/69))
- **injection**: the prompt hook injects what this session touched and what the prompt names, never a broadcast; auto_inject = false keeps a domain out of every automatic surface (F29, [#67](https://github.com/ChristopherKahler/base/issues/67))
- **tiers**: the tier cluster, [#52](https://github.com/ChristopherKahler/base/issues/52) [#53](https://github.com/ChristopherKahler/base/issues/53) [#55](https://github.com/ChristopherKahler/base/issues/55) [#18](https://github.com/ChristopherKahler/base/issues/18) [#60](https://github.com/ChristopherKahler/base/issues/60) are one seam, writes act on the tier you stand in and say what changed ([#61](https://github.com/ChristopherKahler/base/issues/61))
- **ingest**: the paul scan dedupes a project reachable by two paths, so an unchanged project is not rewritten every session ([#63](https://github.com/ChristopherKahler/base/issues/63))

### Changed

- **release**: the 0.14.1 note carries F29 ([#68](https://github.com/ChristopherKahler/base/issues/68))
- **release**: the 0.14.1 note carries F25 and names every serving surface ([#64](https://github.com/ChristopherKahler/base/issues/64))

## 0.14.0 (2026-09-07)

Every record in the graph now carries a domain. The first session start on 0.14.0 backfills a link onto every record that lacks one, per tier, after snapshotting the store; on a 14 MB workspace store (60,000 quads) that pass took 1.1 seconds, once, and later session starts add about half a second only when the store changed since the last one. Later session starts file only what was written since, so records that `base sync` and the PAUL ingest create are filed at the next session start, and tasks, milestones and handoffs are filed the moment they are created. `base doctor` reports each tier's `schema:` line and anything still without a domain; `base graph migrate` runs the backfill by hand. Relay pings no longer appear in `graph analyze`, `recall`, prompt-time injection or the dashboard.

**Every install rewrites the BASE CLI section of `~/.claude/CLAUDE.md` once.** The section's text changed in this release, so the first session start on 0.14.0 refreshes it and prints one `[contract]` line; everything outside the section is left byte for byte as it was.

**`base activate` is retired.** It removed an attribution block that no longer exists. The command still parses and exits 0, so a script that runs it does not break. An install that had activated will see the update-available banner again when a newer release exists; `base update --snooze` quiets it for 24 hours.

Nothing base prints or writes names a company any more: the install and scaffold banners, the update banner, the generated `base.toml`, `domains.toml` and operator files, the CLAUDE.md section, `--help`, the release bodies and the docs site carry only the docs link and the author credit. `base install` and `base scaffold` print one short first-run message, and the first session start after an update prints one line saying what version is now running and where the changelog is.

**Prompt-time injection walks the graph from what you named.** Until now everything served at prompt time hung off a domain that a keyword selected, so naming a project surfaced nothing about its client or its files, and a topic with no configured keyword could never fire at all. Naming a thing now resolves it to a record and serves what hangs off it — decisions, rules, tasks in progress, notes, documents and entities — as one `<base-context>` block per named thing, each line carrying the relation that put it there. A project or a domain gets two hops; a single record gets one. Keyword triggers stay as the domain-level layer and this is the record-level layer beside them, so a domain with no keywords is now reachable through anything filed in it that you name. Quoting, backticking, capitalising, a path shape, or two or more words all read as naming something; a bare lowercase word never resolves on its own, so `base` mid-sentence stays a word. An ambiguous name resolves to the same record every time or is skipped, never guessed. Relay pings never appear. The walk reuses the store the hook has already parsed rather than reading it a second time — one graph load per prompt, and the per-app AST map is not read at all — and `injection.walk_budget` caps the block, counting what it drops rather than dropping it silently.

### Added

- **injection**: prompt-time injection walks the graph from what the prompt names ([#56](https://github.com/ChristopherKahler/base/issues/56))
- **domain**: every record carries a domain link, migrated in one idempotent pass ([#50](https://github.com/ChristopherKahler/base/issues/50))
- **install**: brand strip, one first-run message, and a session-start update notice ([#49](https://github.com/ChristopherKahler/base/issues/49))

### Fixed

- **release**: label-fixed-in windows on the previous release and always creates the label ([#58](https://github.com/ChristopherKahler/base/issues/58)) ([#57](https://github.com/ChristopherKahler/base/issues/57))

### Changed

- **release**: the 0.14.0 note and the licence notice without the company ([#51](https://github.com/ChristopherKahler/base/issues/51))

## 0.13.19 (2026-09-05)

A follow-up to 0.13.18's CLAUDE.md contract refresh. 0.13.18 marked the refresh done even when it had installed nothing (no CLAUDE.md, no `## BASE CLI` section, or two of them), so a user who fixed that later never received the current section. This release looks once more at the next session start: a current section is left alone, a missing or duplicated one is reported every session until it is resolved, and the refresh lands as soon as it can.

### Fixed

- **install**: stamp the CLAUDE.md refresh only once the text is on disk ([#48](https://github.com/ChristopherKahler/base/issues/48))

## 0.13.18 (2026-09-05)

This release makes the relational graph commands read the edges base writes for itself, makes `sync --repair` actually write the repairs it prints, and refreshes the installed CLAUDE.md contract once per version at session start, so an agent on an old install stops running an old contract.

**Windows installs older than 0.13.4 cannot receive this or any update.** That version fixed self-update on Windows, and the fix ships inside the update those installs cannot apply; `base update` on them prints "Run: base update" and cannot succeed. Bootstrap once with the release zip or `npx chrisai`, after which base updates itself in place.

### Fixed

- **update**: refresh the installed CLAUDE.md contract once per version ([#47](https://github.com/ChristopherKahler/base/issues/47)) ([#45](https://github.com/ChristopherKahler/base/issues/45))
- **sync**: --repair writes the repairs it prints, one statement at a time ([#46](https://github.com/ChristopherKahler/base/issues/46)) ([#44](https://github.com/ChristopherKahler/base/issues/44))
- **graph**: read the edges base writes for itself, and resolve names the same way every run ([#43](https://github.com/ChristopherKahler/base/issues/43)) ([#42](https://github.com/ChristopherKahler/base/issues/42))

## 0.13.17 (2026-09-04)

### Fixed

- **domain**: count the rules a domain actually injects, not just the file's ([#38](https://github.com/ChristopherKahler/base/issues/38))
- **triage**: read reports with the Claude Code CLI, polled from a machine
- **rule**: the listing counts up too, and a test that fails on the old code ([#30](https://github.com/ChristopherKahler/base/issues/30))
- **rule**: compare rule numbers as integers, not text

## 0.13.16 (2026-09-04)

### Added

- **automap**: a size fuse before any unattended build
- **triage**: vet bug reports and label the release that fixed them ([#24](https://github.com/ChristopherKahler/base/issues/24))
- **release**: write the changelog entry at release time, and refuse a release without one ([#23](https://github.com/ChristopherKahler/base/issues/23))

### Fixed

- **automap**: the sandbox exemption covers the segments the sandbox itself sits under
- **hook**: write the sync time into .domain-sync-ts so the guard closes on NTFS
- **store**: buffer the write_back dump, propagate the flush, and count the quads it wrote
- **automap**: key the temp exemption on the sandbox, never on a feature flag
- **automap**: a shell `cd` never adopts an unmarked folder
- **automap**: never map temp, caches, AppData, node_modules or the other OS's home
- **release**: give the rehearsal clone a git identity
- **release**: write the changelog before regenerating the coach ([#27](https://github.com/ChristopherKahler/base/issues/27))
- **clippy**: the 26 lints rustc 1.98.1 raises, so CI's clippy job is green again
- **changelog**: accept --date and --notes-dir after the subcommand ([#25](https://github.com/ChristopherKahler/base/issues/25))

### Changed

- **changelog**: CHANGELOG.md, generated from the tags by scripts/changelog.py ([#17](https://github.com/ChristopherKahler/base/issues/17))
- CI on every PR, a bug issue form, triage labels, and the process in CONTRIBUTING.md ([#16](https://github.com/ChristopherKahler/base/issues/16))

## 0.13.15 (2026-09-03)

### Fixed

- **base-help**: hold the coach to the binary — generated cli.md, enforced stamps, release script

### Changed

- src/help_docs.rs is a source file, not an executable

## 0.13.14 (2026-09-02)

*No user-visible changes.*

## 0.13.13 (2026-09-02)

*No user-visible changes.*

## 0.13.12 (2026-09-02)

*No user-visible changes.*

## 0.13.11 (2026-09-02)

### Added

- **automap**: workspace boots — hubs two levels deep, nested apps map themselves, Bash and wsl first contact, one build at a time

## 0.13.10 (2026-09-01)

### Added

- **automap**: no app goes without a code map — bare folders, read-only contact, big trees, missing hooks

## 0.13.9 (2026-09-01)

### Added

- **hooks**: every app gets a code map on first contact — session-start and Stop build it, never by hand

## 0.13.8 (2026-09-01)

### Fixed

- **hooks**: the Stop-hook AST refresh survives a cwd change — dirty marks get a global-tier copy

## 0.13.7 (2026-09-01)

### Fixed

- **ast**: match paths below the app root on Windows — normalise the stored sourceFile in SPARQL

## 0.13.6 (2026-09-01)

### Fixed

- **ast**: `ast query --file` is linear again — sourceFile pattern back inside the first BGP

## 0.13.5 (2026-09-01)

- **Skill backups move out of `~/.claude/skills`.** When an update replaced `base-help`, the previous copy was kept beside it as `base-help.bak-<timestamp>`, and Claude Code loaded that backup as a second, stale skill. Backups now live under `~/.base-gbl/backups/skills/`. If you already have a `*.bak-*` directory in `~/.claude/skills`, move or delete it.

Windows installs older than 0.13.4: see the v0.13.4 notes for the one-time bootstrap.

### Fixed

- **install**: park replaced skills under ~/.base-gbl/backups/skills, not inside ~/.claude/skills

## 0.13.4 (2026-09-01)

- **Self-update works on Windows.** The updater is `base.exe` itself, and Windows refuses to rename a file over a running image, so every hook-spawned update since 0.13.0 downloaded the release and failed the last step in silence. The running binary is now renamed aside (`base.exe.old`) and the new one renamed in; a side-by-side extensionless `base` (Git Bash) is refreshed too.
- **`~/.base-gbl/update.log`**: one line per swap or failure, background or manual. The silent path finally leaves a trace.

**One-time step for Windows installs older than 0.13.4:** the old binary cannot replace itself, so install this release by downloading `base-windows-x86_64.zip` below (or `npx chrisai`). From here on it updates in place at session start.

### Fixed

- **update**: swap a running Windows binary by renaming it aside, and leave a trace

## 0.13.3 (2026-09-01)

- **Relay wake contract** (#11, #13): the hook-injected block explains what the relay is, that everything it touches stays under `~/.base-gbl/.base/relay-inbox/`, where the instruction comes from and how to switch it off. It names the operator from `base operator init` (or says "the operator"). New `[relay]` section: `base config set relay.enabled false` / `base config set relay.wake_nudge false`.
- **Nudge throttle on Windows** (#13): the 180 s cooldown never engaged because a zero-byte write does not move mtime on Windows; it now sets the mtime explicitly.
- **`base scaffold` on Windows** (#13): the workspace path is written in a TOML-safe form instead of the verbatim `\\?\` path that broke `base config`.
- **Skills install** (#12): reports when `~/.claude/skills` is a symlink; `BASE_SKILLS_DIR` overrides the destination.
- **Domain sync** (#14, thanks @PulseCheckAI): `updatedAt` no longer accumulates one quad per sync.

**Windows note:** the silent self-update in this release cannot replace its own running binary; that is fixed in v0.13.4. Install this or later by downloading the zip or `npx chrisai`.

### Added

- **doctor**: record the coach's version and report when it lags
- **update**: refresh the bundled skill after a binary swap
- **sync**: ring a running app after a graph write (R7)
- **sync**: publish the hook command table as JSON (R4)
- **sync**: capture deltas at the SPARQL write sites behind a pairing gate (R1b)
- **sync**: carry the delta on the five fact-producing writes (R1a)
- **sync**: stamp origin on every changes.jsonl record
- **sync**: base graph apply-ops — apply inbound fact ops into the local graph
- **changelog**: append every successful graph write to changes.jsonl

### Fixed

- **relay,install,scaffold**: answer issues [#11](https://github.com/ChristopherKahler/base/issues/11), [#12](https://github.com/ChristopherKahler/base/issues/12) and [#13](https://github.com/ChristopherKahler/base/issues/13)
- **sync**: GC stale domain metadata before re-upsert
- **sync**: stop dying on backslashed relative paths in triple inserts
- **base-help**: import the v0.12.3 bank and correct it against 0.13.2
- **tier**: resolve --global directly instead of walking up into the workspace tier
- **sync**: let a first pull create graph.nq instead of refusing
- **plugin**: resolve manifest paths to one shape on Windows and unix
