---
name: base:audit-claude-md
description: Audit CLAUDE.md and generate recommended version
allowed-tools: [Read, Write, Edit, Glob, Grep, Bash]
---

<objective>
Audit the project's CLAUDE.md for completeness, accuracy, and alignment with workspace state. Generate a recommended version.

**When to use:** "audit claude md", "check my claude.md", after major workspace changes.
</objective>

<execution_context>
@~/.claude/base-framework/tasks/audit-claude-md.md
</execution_context>

<context>
$ARGUMENTS

@CLAUDE.md
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/audit-claude-md.md
</process>

<success_criteria>
- [ ] Current CLAUDE.md analyzed
- [ ] Gaps and outdated sections identified
- [ ] Recommended version generated or changes suggested
</success_criteria>
