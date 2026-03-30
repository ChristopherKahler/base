# The CLAUDE.md Strategy

Composable framework for auditing and writing high-performance CLAUDE.md files. Source of truth for the `/base:audit-claude-md` workflow.

---

## The Structure: What, Why, Who, Where, How

Every CLAUDE.md follows five sections in this exact order. Each section answers one question. Together they give Claude complete operating context without bloat.

### What
What this document is and what this workspace contains.

One line. Sets the contract. Claude knows this is its instruction set.

> "This file provides guidance to Claude Code when working with code in this repository."

### Why
The philosophy. Identity context. Why this workspace exists.

This is where you separate identity from operations. CLAUDE.md answers the "who am I working with?" question. Operational details (how to run a specific project, current sprint status) live elsewhere and get referenced with `@` pointers.

### Who
Business context. Who the user is, what they do, what matters to them.

Not a life story. Enough context that Claude can make relevant suggestions. Business name, what the business does, revenue model, team, tech stack. The goal: Claude should be able to answer "what does this person's business look like?" after reading this section.

### Where
Workspace structure. The directory map.

This section serves double duty:
1. Tells Claude where to find things
2. Plants the blueprint for the workspace architecture — the Where section describes a structure that may or may not exist yet

Use a tree diagram. Include:
- Each top-level directory and its purpose
- Key subdirectories if they have meaning
- What goes where (decision guide)

### How
Ecosystem strategy. Tool strategy. Git strategy. Quick references.

This is the operational layer:
- What systems/frameworks are in use (compact table with location pointers)
- Git strategy (what gets tracked, what gets ignored)
- Rules (see NEVER pattern below)
- Quick reference table for common actions

---

## The NEVER Pattern: Rules as Anti-Patterns

**The single most important discovery from 40+ sessions of compliance testing.**

### The Pattern

```
NEVER [wrong action] — [right action]
```

Negative framing with absolute language gets near-perfect adherence. Every high-compliance rule follows this format.

### Why This Works

It's a binary check. Claude can look at its own output and ask: "Did I do the forbidden thing or not?" No judgment call. No interpretation. No sliding scale.

| Framing | Compliance | Why |
|---------|-----------|-----|
| "Try to use templates when possible" | ~40% | Ambiguous. "When possible" is a judgment call Claude resolves toward skipping. |
| "Always use templates" | ~65% | Better, but "always" gets weighed against context. Claude may decide exceptions apply. |
| "NEVER create from scratch — use templates" | ~95% | Binary. The forbidden action is unambiguous. |

### Rules for Writing Rules

1. **One rule per line.** No compound rules. If it has "and" in it, split it.
2. **The wrong action comes first.** Claude anchors on the first thing it reads. Make the forbidden action the anchor.
3. **The right action is the alternative.** Not a lecture — a redirect. "Don't do X — do Y instead."
4. **Defer complexity to reference docs.** The rule stays short. The details live in a separate file loaded on demand.

---

## The @ Reference System

CLAUDE.md stays lean by pointing to other files instead of inlining their content.

```
@LINKS.md — Personal branding URLs
@projects/dashboard-build/PLANNING.md — Active project context
```

Claude reads `@`-referenced files on demand. Your CLAUDE.md stays lean while still giving Claude access to deep context.

**What to inline vs what to reference:**
- **Inline:** Identity (who, what, why), workspace structure, rules, quick references
- **Reference:** Active project details, task lists, state files, detailed specs

---

## What Stays Out

The test: **if it changes every week, it doesn't belong in CLAUDE.md.**

CLAUDE.md is the constitution, not the daily newspaper.

| Doesn't belong | Where it goes |
|----------------|---------------|
| Task lists / current work | State files, project tracking systems |
| Project specs | Each project's PLANNING.md |
| Rules for specific domains | Domain-specific rule files loaded on demand (e.g., CARL) |
| Daily status updates | State files |
| Detailed framework docs | Separate files, `@`-referenced |

---

## Line Budget

Target: **under 100 lines.** This is a routing document, not a knowledge base.

If your CLAUDE.md is over 100 lines, content is being inlined that should be referenced or removed. Common offenders:
- Inline system/framework descriptions (replace with a compact table)
- Redundant location tables when a tree diagram already exists
- Documentation system descriptions (the tree covers this)
- Standalone sections for single rules (consolidate into Rules section)

---

## Audit Criteria

When auditing an existing CLAUDE.md, check:

### Structure
- [ ] Follows What → Why → Who → Where → How order
- [ ] Each section is correctly labeled
- [ ] No orphan sections outside the five-section model

### Content Placement
- [ ] Identity/philosophy in Why (not scattered)
- [ ] Business context in Who (not bloated)
- [ ] Directory map in Where (tree format, not just a table)
- [ ] All operational content in How (git, systems, rules, quick ref)
- [ ] No task lists, state files, or volatile data inlined

### Rules
- [ ] All rules use NEVER pattern (not "always", "try to", "prefer")
- [ ] One rule per line, no compound rules
- [ ] Wrong action first, right action as redirect
- [ ] Complex rules defer to reference docs

### Leanness
- [ ] Under 100 lines (excluding code blocks in Where tree)
- [ ] No redundant sections (e.g., key locations table + tree diagram)
- [ ] System descriptions are a compact table, not inline paragraphs
- [ ] `@` references used for anything that changes frequently

### CARL Integration (if present)
- [ ] Operational rules that belong in domain-specific contexts are flagged for CARL migration
- [ ] CLAUDE.md rules are constitutional (identity-level), not operational
- [ ] No duplication between CLAUDE.md rules and CARL domain rules
