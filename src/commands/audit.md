---
name: base:audit
description: Deep workspace optimization
allowed-tools: [Read, Write, Edit, Glob, Grep, Bash, Agent, AskUserQuestion]
---

<objective>
Deep workspace audit — comprehensive optimization across all areas with actionable recommendations.

**When to use:** "audit my workspace", "deep clean", monthly optimization.
</objective>

<execution_context>
@~/.claude/base-framework/tasks/audit.md
@~/.claude/base-framework/context/base-principles.md
@~/.claude/base-framework/frameworks/audit-strategies.md
</execution_context>

<context>
$ARGUMENTS

@.base/workspace.json
</context>

<process>
Follow task: @~/.claude/base-framework/tasks/audit.md
</process>

<success_criteria>
- [ ] All areas audited with strategy-specific checks
- [ ] Findings categorized and prioritized
- [ ] Actionable recommendations presented
</success_criteria>
