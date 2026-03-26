---
name: base:carl-hygiene
description: CARL domain maintenance and rule review
allowed-tools: [Read, Write, Edit, Glob, Grep, Bash, AskUserQuestion]
---

<objective>
CARL rule lifecycle management — review staleness, dedup, staging pipeline, domain health.

**When to use:** "carl hygiene", "review carl rules", "clean up carl".
</objective>

<execution_context>
@~/.claude/base-framework/tasks/carl-hygiene.md
</execution_context>

<context>
$ARGUMENTS

@.carl/manifest
@.base/workspace.json
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/carl-hygiene.md
</process>

<success_criteria>
- [ ] All domains reviewed for staleness
- [ ] Duplicate/conflicting rules identified
- [ ] Staging pipeline processed
- [ ] carl_hygiene.last_run updated in workspace.json
</success_criteria>
