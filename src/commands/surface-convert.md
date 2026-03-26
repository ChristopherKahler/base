---
name: base:surface-convert
description: Convert a markdown file into a data surface
argument-hint: "<file-path>"
allowed-tools: [Read, Write, Edit, Glob, Bash, AskUserQuestion]
---

<objective>
Convert an existing @-mentioned markdown file into a structured data surface. Analyzes structure, proposes schema, migrates content.

**When to use:** User has a markdown file they want to convert to a structured surface.
</objective>

<execution_context>
@~/.claude/base-framework/tasks/surface-convert.md
@.base/hooks/_template.py
</execution_context>

<context>
$ARGUMENTS

@.base/workspace.json
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/surface-convert.md
</process>

<success_criteria>
- [ ] .base/data/{name}.json created with migrated items
- [ ] .base/hooks/{name}-hook.py created
- [ ] workspace.json updated with surface registration
- [ ] settings.json updated with hook entry
- [ ] Original markdown file preserved
</success_criteria>
