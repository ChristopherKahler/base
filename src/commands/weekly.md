---
name: base:weekly
description: Weekly review and planning ritual
allowed-tools: [Read, Write, Edit, Glob, Grep, Bash, AskUserQuestion]
---

<objective>
Guided weekly ritual — close the week, plan the next, run maintenance, lock in priorities with calendar events.

**When to use:** Weekly planning, "run my weekly", "weekly review", "time for my weekly".
</objective>

<execution_context>
@~/.claude/base-framework/tasks/weekly.md
@~/.claude/base-framework/context/base-principles.md
</execution_context>

<context>
$ARGUMENTS

@.base/workspace.json
@.base/data/state.json
@.base/weekly.json
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/weekly.md
</process>

<success_criteria>
- [ ] Week reviewed (daily logs consumed if available)
- [ ] Calendar audited with rules applied
- [ ] Workspace groomed (drift score updated)
- [ ] Priority stack set (outcome-based, aligned to north star)
- [ ] Backlog triaged (overdue items processed)
- [ ] Domain phases executed (if configured)
- [ ] Blockers identified, follow-ups queued
- [ ] Week committed — calendar events created, weekly.json entry logged
</success_criteria>
