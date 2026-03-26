---
name: base:surface-create
description: Create a new data surface (guided)
argument-hint: "[surface-name]"
allowed-tools: [Read, Write, Edit, Glob, Bash, AskUserQuestion]
---

<objective>
Create a new data surface through guided conversation. Generates JSON data file, injection hook, workspace.json registration, and settings.json hook entry.

**When to use:** User wants to track something new as a structured data surface.
</objective>

<execution_context>
@~/.claude/base-framework/tasks/surface-create.md
@.base/hooks/_template.py
</execution_context>

<context>
$ARGUMENTS

@.base/workspace.json
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/surface-create.md
</process>

<success_criteria>
- [ ] .base/data/{name}.json created
- [ ] .base/hooks/{name}-hook.py created
- [ ] workspace.json updated with surface registration
- [ ] settings.json updated with hook entry
</success_criteria>
