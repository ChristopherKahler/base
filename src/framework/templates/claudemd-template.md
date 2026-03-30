# CLAUDE.md Template

Reference template for generating strategy-compliant CLAUDE.md files. Placeholders use `{PLACEHOLDER}` format. Comments use `<!-- -->` and must be removed in final output.

***

```Markdown
# CLAUDE.md

## What

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## Why

<!-- 2-5 lines. Philosophy. What this workspace is for. Separation of concerns (what lives here vs elsewhere). -->

{PHILOSOPHY}

---

## Who

<!-- Business context. Not a life story — enough for Claude to make relevant suggestions. -->

**{WORKSPACE_NAME}** — {ONE_LINE_DESCRIPTION}

### {BUSINESS_1_NAME}
{BUSINESS_1_URL_IF_APPLICABLE}
- {SERVICE_OR_PRODUCT_1}
- {SERVICE_OR_PRODUCT_2}
- {SERVICE_OR_PRODUCT_3}

<!-- Add additional businesses/ventures as needed. Keep each to 3-5 bullet points max. -->

<!-- Optional: content funnel, revenue model, team context — only if it shapes how Claude should assist. -->

---

## Where

<!-- Tree diagram of workspace. Include top-level dirs + meaningful subdirectories. Comments explain purpose. -->

` ` `
{WORKSPACE_ROOT}/
├── {DIR_1}/         # {PURPOSE}
├── {DIR_2}/         # {PURPOSE}
│   ├── {SUBDIR}/    # {PURPOSE — only if meaningful}
│   └── {SUBDIR}/    # {PURPOSE}
└── {DIR_N}/         # {PURPOSE}
` ` `

---

## How

### Systems

<!-- Compact table. One row per system/framework. No inline descriptions. -->

| System | Purpose | Location |
|--------|---------|----------|
| {SYSTEM_1} | {ONE_LINE_PURPOSE} | `{LOCATION}` |
| {SYSTEM_2} | {ONE_LINE_PURPOSE} | `{LOCATION}` |

### Git Strategy

<!-- How version control works in this workspace. -->

| Directory | Approach |
|-----------|----------|
| {DIR} | {STRATEGY} |

### Rules

<!-- NEVER pattern. One per line. Wrong action first, right action as redirect. -->
<!-- Only constitutional/identity rules here. Operational rules belong in domain-specific systems. -->

NEVER {WRONG_ACTION} — {RIGHT_ACTION}
NEVER {WRONG_ACTION} — {RIGHT_ACTION}

### Quick Reference

<!-- Common actions. One per line. Action → instruction. -->

**{ACTION}?** → {INSTRUCTION}
**{ACTION}?** → {INSTRUCTION}
```

***

## Usage Notes

* Target: under 100 lines in final output
* Remove all `<!-- -->` comments before finalizing
* Remove placeholder sections that don't apply (not every workspace needs Git Strategy or Systems)
* The Where tree should reflect the ACTUAL filesystem, verified by scanning
* Rules should be workspace-identity-level, not operational. If a rule only applies during specific work (e.g., "when writing tests..."), it belongs in a domain-specific rule system, not CLAUDE.md
* `@` references point Claude to files it should read on demand — use for anything volatile or detailed

