# Audit Strategies

Reusable audit strategies that can be applied to any workspace area. The workspace manifest (`workspace.json`) maps areas to strategies. The audit command reads the manifest and applies the appropriate strategy to each area.

## Strategies

### staleness
**Applies to:** Data surfaces (active.json, backlog.json, any tracked document)
**What it does:** Check file modification timestamps against configured thresholds. Flag files past their groom cadence.
**Config:**
- `threshold_days` — days after which the file is considered stale
**Output:** List of stale files with age, recommended action (update or review)

### classify
**Applies to:** Directories with lifecycle items (projects/, clients/)
**What it does:** List all items in the directory. For each, present to operator for classification: active, archive, or delete. Check for planning docs, recent activity, git history.
**Config:**
- `states` — classification options (default: ["active", "archive", "delete"])
- `archive_path` — where archived items go (default: `{path}/_archive/`)
**Output:** Classification decisions, items moved to archive, items deleted

### cross-reference
**Applies to:** Tools/servers that have a config file mapping (e.g., MCP servers vs .mcp.json)
**What it does:** Compare directory contents against a configuration file. Identify directories not referenced in config (orphaned) and config entries pointing to missing directories (broken).
**Config:**
- `config_file` — path to the configuration file to cross-reference
**Output:** Orphaned items, broken references, recommendations

### dead-code
**Applies to:** System directories (hooks, commands, skills)
**What it does:** Scan for files that appear unused — no references from other files, no recent invocations, no clear purpose. Presents findings for human decision.
**Config:**
- `reference_check` — whether to search for references in other files (default: true)
**Output:** Potentially dead files with evidence, operator decides keep/delete

### pipeline-status
**Applies to:** Content pipelines, task queues, any workflow with stages
**What it does:** Check items in each pipeline stage. Flag stuck items (in same stage too long), empty stages, bottlenecks.
**Config:**
- `stages` — ordered list of pipeline stages
- `stuck_threshold_days` — days in one stage before flagging
**Output:** Pipeline health report, stuck items, stage distribution

## Extending Strategies

Custom strategies can be added for workspace-specific needs. A strategy is defined by:
1. A name (kebab-case)
2. What it applies to (description)
3. What it checks (logic)
4. What config it needs (parameters)
5. What it outputs (findings format)

Add custom strategies to this file and reference them in `workspace.json`.
