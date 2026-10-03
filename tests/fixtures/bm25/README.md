# BO-18 fixtures

`off-path-golden.json` is what the prompt hook printed, and the blocks file it wrote, at `07277bc` (main before BO-18),
for five prompts of the replay corpus (`tests/fixtures/replay/prompts.txt` content lines 0, 12, 28, 30 and 31; line 31
goes over the 10,000-byte budget). `tests/bm25_test.rs::bm25_off_and_no_index_print_what_07277bc_printed` holds the
prompt hook to it with `[match] bm25 = false`, and with BM25 on and no index built yet (lynx's G0 verdict on BO-18,
question 1, condition 2).

How it was made, 2026-10-03: a detached worktree at `07277bc`, a test there (never committed) that seeds
`seed::write(root, &seed::TINY, <replay base.toml>)` with the replay `domains.toml`, runs every corpus prompt as the first
prompt of its own session (`bm25-off-NN`), and keeps each one's stdout and `prompt-blocks.json` without `written_at`, the
seed's root replaced by `<root>`: the same steps as `corpus_outputs` in `tests/bm25_test.rs`. `tests/seed/mod.rs` and
`tests/fixtures/replay/` were identical at `07277bc` and on the BO-18 branch when it was made. The debug binary that ran
it had md5 `03d43c4b6850e5570d82003866085175` and no `bm25-index.json` string in it.

A later change to the prompt hook's output with BM25 off is a change to this file: regenerate it the same way from the
commit before that change, and say why in the PR body.
