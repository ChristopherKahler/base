---
name: base:audit-claude-md
description: Audit CLAUDE.md against the CLAUDE.md Strategy and generate a compliant version
allowed-tools: [Read, Write, Edit, Glob, Grep, Bash]
---

<objective>
Audit the project's CLAUDE.md for strategy compliance, interactively rewrite it section by section, and route operational rules to CARL or an artifact.

**When to use:** "audit claude md", "check my claude.md", "rewrite my claude.md", after major workspace changes.
</objective>

<execution_context>
@~/.claude/base-framework/frameworks/claudemd-strategy.md
@~/.claude/base-framework/templates/claudemd-template.md
@~/.claude/base-framework/tasks/audit-claude-md.md
</execution_context>

<context>
$ARGUMENTS

@CLAUDE.md
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/audit-claude-md.md

Key gates (do NOT skip):
1. Load strategy + template BEFORE reading user's CLAUDE.md
2. Present full audit classification — wait for user approval
3. Detect CARL — wait for user decision on rule routing
4. Propose each section individually — wait for approval per section
5. Write to CLAUDE.base.md (never overwrite CLAUDE.md)
</process>

<success_criteria>
- [ ] Strategy framework loaded first
- [ ] Every line classified (KEEP/REMOVE/RESTRUCTURE/CARL_CANDIDATE)
- [ ] User approved audit before rewriting began
- [ ] CARL detection completed, rule routing decided
- [ ] Each section approved individually
- [ ] Final CLAUDE.base.md under 100 lines
- [ ] Original CLAUDE.md untouched
</success_criteria>
