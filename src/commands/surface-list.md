---
name: base:surface-list
description: Show all registered data surfaces
allowed-tools: [Read, Bash]
---

<objective>
Display all registered data surfaces with item counts and hook status.

**When to use:** User wants to see what surfaces exist.
</objective>

<execution_context>
@~/.claude/base-framework/tasks/surface-list.md
</execution_context>

<context>
@.base/workspace.json
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/surface-list.md
</process>

<success_criteria>
- [ ] All registered surfaces displayed with counts
</success_criteria>
