# CARL Hygiene — BASE Extension for CARL Domain Governance

**Created:** 2026-03-16
**Status:** Design / Captured from session
**Related:** BASE-SPEC.md, DECISIONS-REDESIGN.md

---

## Problem

CARL domains accumulate rules, keywords, and conventions over time. Without maintenance:
- Rules go stale (reference old patterns, tools, or workflows that changed)
- Keywords stop matching real user intent
- Domains grow bloated with rules that sounded good at the time but never proved useful
- New systems (PSMM, decisions) want to write rules directly — without governance, CARL becomes a dumping ground

---

## Design

### CARL Hygiene as a BASE Capability

BASE already manages workspace health. CARL is part of the workspace. Therefore CARL hygiene is a BASE responsibility — not a standalone system.

**Implementation:** A new BASE command + optional proactive hook behavior.

```
/base:carl-hygiene   — Structured CARL domain audit
```

**Proactive setting in workspace.json:**
```json
{
  "carl_hygiene": {
    "proactive": true,
    "cadence": "monthly",
    "last_run": "2026-03-16"
  }
}
```

When `proactive: true`, the pulse hook checks staleness and surfaces a reminder. When `proactive: false`, CARL hygiene only runs when explicitly invoked. User controls this — recommended on, but never forced.

### What CARL Hygiene Checks

1. **Domain relevance** — Is each domain still meaningful? Does it map to real work?
2. **Rule relevance** — For each rule: is this still accurate? Still useful? Has the workspace changed since this was written?
3. **Keyword accuracy** — Do recall keywords still match how the user actually talks about this work?
4. **Rule staleness** — Timestamp-based. Rules older than N days without review get flagged.
5. **Bloat detection** — Domains with too many rules (threshold configurable). Are all rules earning their keep?
6. **Cross-domain conflicts** — Rules in different domains that contradict each other.

### The Staging Layer (Critical Architecture)

**Rules are NEVER written directly to CARL from automated systems.** Not from PSMM, not from decisions, not from any in-session observation. Instead:

```
Observation (PSMM, session insight, decision, etc.)
  → Proposed Rule (staged in .carl/staging/)
  → CARL Hygiene Review (decide / approve / kill / archive)
  → Committed Rule (written to domain file)
```

**`.carl/staging/` directory:**
```json
{
  "proposals": [
    {
      "id": "prop-001",
      "proposed": "2026-03-16",
      "source": "psmm",
      "session": "workspace-optimization-session",
      "domain": "global",
      "rule_text": "When <base-pulse> drift > 15, recommend grooming before other work",
      "rationale": "Discovered during workspace optimization that drift of 50 went unnoticed for weeks",
      "status": "pending",
      "reviewed": null
    }
  ]
}
```

During `/base:carl-hygiene`, staged proposals are presented:
- "This rule was proposed on {date} from {source}. Context: {rationale}."
- User decides: **Approve** (write to domain), **Kill** (delete), **Archive** (keep for reference but don't activate), **Defer** (review next time)

### Rule Lifecycle

Every CARL rule (existing and new) carries metadata:

```
# Rule added: 2026-03-16
# Last reviewed: 2026-03-16
# Source: manual | psmm | decisions | hygiene
# Status: active | archived
GLOBAL_RULE_11=When <base-pulse> is present in context...
```

**Staleness detection:**
- Rules with `last_reviewed` older than 60 days → flagged during hygiene
- Flagged rules get "decide or kill" pressure (same pattern as backlog items)
- User reviews: still relevant? Update `last_reviewed`. Not relevant? Archive.

**Archive pattern:**
- Archived rules move to `.carl/archive/` (not deleted)
- Archive includes the original rule, when it was active, why it was archived
- Users change their minds — archived rules can be restored during hygiene
- Explicit delete only when user says "delete this, I'm sure"

### PSMM → CARL Pipeline

Per-Session Meta Memory (PSMM) captures ephemeral session insights. Some of those insights deserve to become permanent CARL rules. The pipeline:

1. **During session:** PSMM logs a meta moment (ephemeral, session-scoped)
2. **End of session (or during):** Claude asks: "This PSMM entry looks like it could be a CARL rule. Stage it?"
3. **If yes:** Written to `.carl/staging/` with source=psmm, full context
4. **During next `/base:carl-hygiene`:** Staged proposals reviewed, decided on
5. **If approved:** Written to the appropriate domain file with metadata

This prevents reactive in-session rule writing (which produces low-quality rules based on one experience) and forces a "sleep on it" review cycle.

### Decisions → CARL Pipeline

Same pattern. When a decision is logged that implies a behavioral rule:
1. Decision logged to `.carl/decisions/{domain}.json`
2. If the decision implies a rule (e.g., "We chose Cloudflare over SendGrid" → "When email provider comes up, use Cloudflare"), propose staging
3. Staged proposal reviewed during hygiene
4. If approved, becomes a domain rule

---

## The Flow

```
Session Work
  ↓
Observations (PSMM, decisions, manual)
  ↓
.carl/staging/ (proposals with context)
  ↓
/base:carl-hygiene (review cycle)
  ├── Approve → Write to domain with metadata
  ├── Kill → Delete proposal
  ├── Archive → Move to .carl/archive/
  └── Defer → Review next cycle
  ↓
Existing rules also reviewed:
  ├── Still relevant → Update last_reviewed
  ├── Stale → Decide: update, archive, or kill
  └── Conflicting → Resolve
```

---

## Integration Points

| System | Relationship to CARL Hygiene |
|--------|------------------------------|
| **BASE pulse** | When `proactive: true`, checks `carl_hygiene.last_run` and flags if overdue |
| **BASE groom** | Can include CARL hygiene as an optional step in the weekly groom |
| **BASE audit** | Deep audit always includes CARL hygiene |
| **PSMM** | Feeds proposals into `.carl/staging/` at session end |
| **Decisions** | Decision logging can trigger proposal staging |
| **PAUL** | Phase completions can trigger proposal staging for project-specific rules |

---

## Configuration

Added to `.base/workspace.json`:
```json
{
  "carl_hygiene": {
    "proactive": true,
    "cadence": "monthly",
    "staleness_threshold_days": 60,
    "max_rules_per_domain": 15,
    "auto_stage_from": ["psmm", "decisions"]
  }
}
```

---

## Open Questions

- [ ] Should the staging file be JSON or markdown? JSON is machine-readable (hooks can count proposals). Markdown is human-friendly.
- [ ] Should archived rules be queryable? ("Show me rules I archived about X")
- [ ] How aggressive should staleness detection be? 60 days might be too long or too short depending on the domain.
- [ ] Should CARL hygiene run as part of every groom or only monthly?
