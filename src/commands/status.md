---
name: base:status
description: Quick workspace health check
allowed-tools: [Read, Glob, Bash]
---

<objective>
One-liner workspace health status — drift score and area summary.

**When to use:** Quick check, "workspace status", "how's my workspace".
</objective>

<execution_context>
@~/.claude/base-framework/tasks/status.md
</execution_context>

<context>
@.base/workspace.json
@.base/data/state.json
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/status.md
</process>

<success_criteria>
- [ ] Health status displayed
</success_criteria>
