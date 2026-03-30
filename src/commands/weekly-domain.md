---
name: base:weekly-domain
description: Create a custom domain phase for the weekly ritual
allowed-tools: [Read, Write, Edit, Glob, Grep, Bash, AskUserQuestion]
---

<objective>
Guided creation of a custom domain phase for /base:weekly. Walks the user through defining what to check, what tools to use, what questions to ask, and what output to produce.

**When to use:** "Add a domain to my weekly", "create a weekly domain phase", "I want to track X in my weekly".
</objective>

<execution_context>
@~/.claude/base-framework/tasks/weekly-domain-create.md
</execution_context>

<context>
$ARGUMENTS

@.base/weekly.json
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/weekly-domain-create.md
</process>

<success_criteria>
- [ ] User described the domain area
- [ ] Data sources identified (with tool discovery if needed)
- [ ] Weekly questions defined
- [ ] Position in weekly flow chosen
- [ ] Output type specified
- [ ] Domain phase config written to weekly.json
</success_criteria>
