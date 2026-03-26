<purpose>
Audit an existing CLAUDE.md and generate a recommended alternative (CLAUDE.base.md) that follows BASE workspace conventions. Never overwrites the user's existing file.
</purpose>

<user-story>
As an AI builder setting up BASE, I want my CLAUDE.md audited against best practices, so that I get a recommended version I can adopt without losing my current config.
</user-story>

<when-to-use>
- During /base:scaffold (optional step)
- When user says "audit my claude.md", "improve my claude.md"
- Entry point routes here via /base:audit-claude-md
</when-to-use>

<steps>

<step name="read_existing" priority="first">
Read the user's existing CLAUDE.md.

1. Read `CLAUDE.md` from workspace root
2. If no CLAUDE.md exists, skip audit and generate fresh from template
3. Parse sections: identify what's covered (identity, structure, conventions, tool config, etc.)
4. Note what's missing vs BASE best practices
</step>

<step name="assess_and_recommend">
Compare against BASE workspace conventions.

Check for:
- [ ] Working memory references (@.base/data/active.json, @.base/data/backlog.json, @.base/data/state.json)
- [ ] Workspace structure tree (accurate to actual filesystem?)
- [ ] Git strategy documentation
- [ ] Tool priority declarations (LSP, etc.)
- [ ] Business/identity context (who, what, why)
- [ ] CARL/BASE integration references
- [ ] Key locations table
- [ ] Documentation system description
- [ ] Banned words / style preferences

For each missing or outdated section, prepare a recommendation.
</step>

<step name="generate_alternative">
Generate CLAUDE.base.md as a recommended version.

1. Build a complete CLAUDE.md following BASE conventions
2. Preserve all existing content that's still accurate
3. Add missing sections from BASE template
4. Update workspace structure tree to match actual filesystem
5. Write to `CLAUDE.base.md` (NOT CLAUDE.md)
6. Present diff summary: "Here's what changed vs your current CLAUDE.md"

Tell user: "Review CLAUDE.base.md. If you want to adopt it, rename it to CLAUDE.md. Your original is untouched."
</step>

</steps>

<output>
CLAUDE.base.md file in workspace root — a recommended CLAUDE.md following BASE conventions. Original CLAUDE.md is never modified.
</output>

<acceptance-criteria>
- [ ] Existing CLAUDE.md read and analyzed
- [ ] All BASE best-practice sections checked
- [ ] CLAUDE.base.md generated with recommendations
- [ ] Original CLAUDE.md untouched
- [ ] User informed of differences and how to adopt
</acceptance-criteria>
