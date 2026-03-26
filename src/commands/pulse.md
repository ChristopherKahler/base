---
name: base:pulse
description: Daily workspace health briefing
allowed-tools: [Read, Glob, Grep, Bash]
---

<objective>
Workspace health briefing — drift score, stale areas, overdue grooming, quick status.

**When to use:** Session start, "what's the state of my workspace", daily check-in.
</objective>

<execution_context>
@~/.claude/base-framework/tasks/pulse.md
@~/.claude/base-framework/context/base-principles.md
</execution_context>

<context>
$ARGUMENTS

@.base/workspace.json
@.base/data/state.json
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/pulse.md
</process>

<success_criteria>
- [ ] Drift score calculated and displayed
- [ ] Stale areas identified
- [ ] Groom cadence checked
</success_criteria>
