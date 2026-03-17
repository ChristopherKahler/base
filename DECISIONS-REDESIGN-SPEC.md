# Decisions System Redesign — CARL Extension Layer

**Created:** 2026-03-15
**Status:** Design / Captured from session

---

## Core Insight

Decisions are NOT a standalone system. They're an extension layer of CARL domains. Without CARL, decisions are useless. With CARL, they become just-in-time contextual recall that loads alongside the domain they belong to.

---

## Architecture

### Decisions as CARL Domain Extensions

Each CARL domain can have an optional decisions layer:

```
.carl/
├── manifest              # Domain definitions (existing)
├── global                # Global rules (existing)
├── development           # Development rules (existing)
├── decisions/            # NEW: Decision store per domain
│   ├── development.json  # Decisions for DEVELOPMENT domain
│   ├── casegate.json     # Decisions for a CASEGATE domain
│   ├── skool.json        # Decisions for SKOOL domain
│   └── ...
```

When CARL loads a domain (e.g., DEVELOPMENT), it checks `decisions/{domain}.json` and injects relevant decisions alongside the rules. No separate fetch. No manual recall. Just-in-time with the domain.

### Domain Granularity Solves the Noise Problem

- `DEVELOPMENT` domain loads → development decisions load (general coding patterns)
- `CASEGATE` domain loads → CaseGate architecture decisions load (project-specific)
- Working on CaseGate? Both domains load, both decision sets load.
- Working on a different app? DEVELOPMENT loads but CASEGATE doesn't. No noise.

This means project-specific domains become more important. CaseGate, Hunter Exotics, etc. should each be their own CARL domain (can be lightweight — just recall keywords + decisions, no behavioral rules needed).

### Decision Format (decisions/*.json)

```json
{
  "domain": "casegate",
  "decisions": [
    {
      "id": "casegate-001",
      "decision": "Use Cloudflare for email, not SendGrid",
      "rationale": "Cost, existing infrastructure, simpler DNS",
      "date": "2026-02-15",
      "source": "paul-phase-12",
      "recall": ["email", "cloudflare", "sendgrid"]
    }
  ]
}
```

### Who Logs Decisions

**CARL manages ALL logging and recall.** Not a command. Not a CLAUDE.md instruction. CARL rules define:
1. When to detect a decision worth logging (pattern recognition in conversation)
2. How to log it (write to `decisions/{domain}.json`)
3. When to surface it (domain loads → decisions load)

This is a CARL behavioral rule, not a user-invoked action:
```
When a significant technical or strategic choice is made during work within a loaded domain,
propose logging it: "Decision detected: [summary]. Log to {domain} decisions?"
```

### PAUL Integration

When PAUL and CARL coexist in a project:
- PAUL phases often produce architectural decisions
- PAUL's `/paul:plan` and `/paul:unify` steps should check for decisions made during the phase
- At phase completion, PAUL proposes: "This phase produced N decisions. Log them to {domain}?"
- Decisions logged during PAUL phases include `source: "paul-phase-{N}"` for traceability

PAUL needs an upgrade:
- Phase summaries should include a "Decisions" section
- `/paul:unify` (reconcile plan vs actual) should scan for unlogged decisions
- STATE.md should note whether decisions system is active

### BASE Integration

BASE handles the maintenance layer:
- During `/base:groom`, review decision counts per domain
- Flag domains with 0 decisions (might be missing logging)
- Flag decisions older than 90 days (might be outdated)
- During `/base:scaffold`, detect CARL + PAUL presence and configure decision paths
- BASE STATE.md tracks: decisions system active (yes/no), total decisions, last logged

### Detection Flow

```
CARL domain loads
  → decisions/{domain}.json loaded alongside rules
  → Claude has full context: rules + prior decisions
  → During work, Claude detects significant choice
  → Proposes logging → User confirms → Written to decisions/{domain}.json
  → Next session that loads this domain gets the decision automatically
```

---

## Migration Path

1. Keep existing DECISIONS.md and decision-logger MCP for now
2. Build decisions/ directory in .carl/
3. Migrate existing 31 decisions from DECISIONS.md into domain-specific .json files
4. Update CARL injector to load decisions alongside domain rules
5. Add decision detection to CARL behavioral rules
6. Upgrade PAUL to include decision logging in phase lifecycle
7. Add decision maintenance to BASE groom cycle
8. Deprecate DECISIONS.md and decision-logger MCP once new system is proven

---

## Open Questions

- [ ] Should decisions have an expiry/review date like backlog items?
- [ ] Should decisions be surfaced in the hook output or just silently available in context?
- [ ] How many decisions per domain before it becomes noise? Cap? Archive threshold?
- [ ] Should PAUL's decision logging be automatic or always require confirmation?
