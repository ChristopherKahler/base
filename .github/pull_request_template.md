<!-- Title in conventional form: feat(scope): ..., fix(scope): ..., chore: ... -->

## What and why

<!-- One paragraph. What changes for a user of base, and why now. -->

Closes #

## Checklist

- [ ] `cargo test` is green locally (it includes the base-help coach gate)
- [ ] The CLI changed? Then `BASE_REGEN_DOCS=1 cargo test --bin base help_docs` was run and a new command has its line in `commands.md` and its pair in `qa.md`
- [ ] A bug fix names its issue above so the close and the Known issues page follow the merge
