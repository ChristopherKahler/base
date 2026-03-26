---
name: base:groom
description: Weekly workspace maintenance cycle
allowed-tools: [Read, Write, Edit, Glob, Grep, Bash, AskUserQuestion]
---

<objective>
Structured weekly maintenance — review each workspace area, update statuses, archive stale items, reduce drift.

**When to use:** Weekly maintenance, "groom my workspace", "run grooming".
</objective>

<execution_context>
@~/.claude/base-framework/tasks/groom.md
@~/.claude/base-framework/context/base-principles.md
@~/.claude/base-framework/frameworks/audit-strategies.md
</execution_context>

<context>
$ARGUMENTS

@.base/workspace.json
@.base/data/state.json
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/groom.md
</process>

<success_criteria>
- [ ] All workspace areas reviewed
- [ ] Stale items addressed
- [ ] state.json updated with groom results
- [ ] Drift score recalculated
</success_criteria>
