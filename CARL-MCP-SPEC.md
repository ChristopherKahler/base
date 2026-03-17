# CARL MCP — Consolidated Tool Server

**Created:** 2026-03-16
**Status:** Design Spec
**Related:** CARL-HYGIENE-SPEC.md, DECISIONS-REDESIGN.md

---

## What This Is

A single MCP server that houses all CARL extension tools. Replaces the separate decision-logger and DRL-engine MCPs. Adds PSMM and staging tools.

**Core principle:** Tools exist but are dormant. Users activate them by adding corresponding CARL rules. No rules = no usage = zero overhead.

---

## Architecture

```
tools/mcp-servers/production/carl-mcp/
├── index.js          # Main server, registers all tool groups
├── package.json
├── tools/
│   ├── domains.js    # Domain CRUD (from DRL-engine)
│   ├── decisions.js  # Decision logging/recall (from decision-logger)
│   ├── psmm.js       # Per-session meta memory
│   └── staging.js    # Rule proposal staging (for CARL hygiene)
└── data/
    ├── decisions/    # Per-domain decision JSON files
    ├── staging.json  # Staged rule proposals
    └── psmm.json     # Session-keyed meta memory
```

---

## Tool Groups

### Domains (migrated from DRL-engine)
| Tool | Purpose |
|------|---------|
| `carl_list_domains` | List all domains with state |
| `carl_get_domain_rules` | Get rules for a specific domain |
| `carl_create_domain` | Create a new domain |
| `carl_toggle_domain` | Enable/disable a domain |
| `carl_get_manifest` | Read the full manifest |

### Decisions (migrated from decision-logger)
| Tool | Purpose |
|------|---------|
| `carl_log_decision` | Log a decision to a domain's decision file |
| `carl_get_decisions` | Get decisions for a domain/category |
| `carl_search_decisions` | Search decisions by keyword |
| `carl_archive_decision` | Archive a decision |

### PSMM (new)
| Tool | Purpose |
|------|---------|
| `carl_psmm_log` | Append a meta memory entry for the current session |
| `carl_psmm_get` | Get entries for a specific session (by UUID) |
| `carl_psmm_list` | List all sessions with entry counts and timestamps |
| `carl_psmm_clean` | Remove a stale session's entries |

### Staging (new — for CARL hygiene pipeline)
| Tool | Purpose |
|------|---------|
| `carl_stage_proposal` | Stage a rule proposal from any source (PSMM, decisions, manual) |
| `carl_get_staged` | List all pending proposals |
| `carl_approve_proposal` | Move a proposal to committed CARL rule |
| `carl_kill_proposal` | Delete a proposal |
| `carl_archive_proposal` | Archive a proposal (keep for reference, don't activate) |

---

## Activation Pattern

1. User installs CARL MCP in `.mcp.json` — all tools are available
2. No CARL rules reference them → Claude never calls them → zero overhead
3. User wants decisions → adds a CARL rule like: "When a significant technical choice is made, use carl_log_decision" → Claude starts using decision tools
4. User wants PSMM → adds GLOBAL_RULE_12 → Claude starts using PSMM tools
5. User removes the rule → Claude stops using the tools

**The rules ARE the activation mechanism.** No config flags, no enable/disable toggles. CARL governs itself.

---

## Migration Plan

1. Build carl-mcp with all 4 tool groups
2. Migrate decision-logger data (DECISIONS.md → data/decisions/)
3. Migrate DRL-engine tools (already have the tool definitions)
4. Point psmm.json to carl-mcp data directory
5. Create staging.json
6. Update .mcp.json: remove decision-logger + drl-engine entries, add carl-mcp
7. Update CARL rules to reference new tool names (carl_* prefix)
8. Test all tool groups
9. Delete old MCP servers

---

## .mcp.json Entry (target state)

```json
{
  "carl-mcp": {
    "type": "stdio",
    "command": "node",
    "args": ["./tools/mcp-servers/production/carl-mcp/index.js"]
  }
}
```

No env vars needed — CARL MCP reads from workspace-relative paths.

---

## CARL Rule Updates Required

When built, update GLOBAL_RULE_12 from file read/write to:
```
Use carl_psmm_log(session_id, type, text) to record meta moments.
Use carl_psmm_get(session_id) to check current session entries.
```

Decision-related rules update similarly to reference carl_log_decision, carl_get_decisions, etc.
