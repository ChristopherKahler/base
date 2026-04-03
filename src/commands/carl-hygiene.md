---
name: base:carl-hygiene
description: CARL domain maintenance and rule review
allowed-tools: [Read, Write, Edit, Glob, Grep, Bash, AskUserQuestion, carl_v2_list_domains, carl_v2_get_domain, carl_v2_get_staged, carl_v2_approve_proposal, carl_v2_remove_rule, carl_v2_replace_rules, carl_v2_archive_decision]
---

<objective>
CARL rule lifecycle management — review staleness, staging pipeline, domain health.

**When to use:** "carl hygiene", "review carl rules", "clean up carl".
</objective>

<execution_context>
@~/.claude/base-framework/tasks/carl-hygiene.md
</execution_context>

<context>
$ARGUMENTS

@.carl/carl.json
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
