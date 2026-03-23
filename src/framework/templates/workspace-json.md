# Workspace Manifest Template

Output file: `.base/workspace.json`

```template
{
  "workspace": "{workspace-name}",
  "created": "{YYYY-MM-DD}",
  "groom_cadence": "{weekly|bi-weekly|monthly}",
  "groom_day": "{day-of-week}",
  "areas": {
    "{area-name}": {
      "type": "{working-memory|directory|config-cross-ref|system-layer|custom}",
      "description": "[Human-readable purpose of this area]",
      "paths": ["{file-or-directory-paths}"],
      "groom": "{weekly|bi-weekly|monthly}",
      "audit": {
        "strategy": "{staleness|classify|cross-reference|dead-code|pipeline-status}",
        "config": {}
      }
    }
  },
  "carl_hygiene": {
    "proactive": true,
    "cadence": "monthly",
    "staleness_threshold_days": 60,
    "max_rules_per_domain": 15,
    "last_run": null
  },
  "surfaces": {
    "{surface-name}": {
      "file": "data/{name}.json",
      "description": "[What this surface tracks]",
      "hook": true,
      "silent": true,
      "schema": {
        "id_prefix": "{PREFIX}",
        "required_fields": ["{field1}", "{field2}"],
        "priority_levels": ["{level1}", "{level2}"],
        "status_values": ["{status1}", "{status2}"]
      }
    }
  },
  "satellites": {
    "{project-name}": {
      "path": "{relative-path-to-project}",
      "engine": "{paul|custom|none}",
      "state": "{path-to-state-file}",
      "registered": "{YYYY-MM-DD}",
      "groom_check": true,
      "last_activity": null,
      "phase_name": null,
      "phase_number": null,
      "phase_status": null,
      "loop_position": "IDLE",
      "handoff": false,
      "last_plan_completed_at": null
    }
  }
}
```

## Field Documentation

| Field | Type | Description |
|-------|------|------------|
| workspace | string | Name of this workspace (typically the directory name) |
| created | date | When BASE was initialized in this workspace |
| groom_cadence | enum | Default grooming frequency for the workspace |
| groom_day | string | Preferred day for weekly grooming |
| areas | object | Map of tracked workspace areas |
| areas.*.type | enum | Classification of the area for audit strategy selection |
| areas.*.paths | array | Files or directories this area tracks |
| areas.*.groom | enum | Grooming frequency for this specific area (overrides default) |
| areas.*.audit.strategy | enum | Which audit strategy to apply (see audit-strategies.md) |
| areas.*.audit.config | object | Strategy-specific configuration |
| carl_hygiene | object | CARL rule lifecycle management config (optional — only if CARL is installed) |
| carl_hygiene.proactive | boolean | Auto-surface stale rules during groom |
| carl_hygiene.cadence | enum | How often to run CARL hygiene |
| carl_hygiene.staleness_threshold_days | number | Days before a rule is flagged as stale |
| carl_hygiene.max_rules_per_domain | number | Soft cap per domain (warn, not enforce) |
| surfaces | object | Registered data surfaces with schemas |
| surfaces.*.file | string | Path to JSON file relative to .base/ |
| surfaces.*.hook | boolean | Whether a hook auto-injects this surface |
| surfaces.*.silent | boolean | Whether hook output is passive (no proactive mentions) |
| surfaces.*.schema | object | Validation schema for surface items |
| surfaces.*.schema.id_prefix | string | Auto-generated ID prefix (e.g., "ACT", "BL") |
| surfaces.*.schema.required_fields | array | Fields required on every item |
| satellites | object | External projects tracked by BASE but managed by their own engines |
| satellites.*.engine | enum | What orchestration tool manages this project |
| satellites.*.state | string | Path to the project's state file for health checks |
| satellites.*.groom_check | boolean | Whether BASE checks this project's health during groom (default: true) |
| satellites.*.last_activity | string | ISO timestamp of last project activity (synced from paul.json) |
| satellites.*.phase_name | string | Current phase name (synced from paul.json) |
| satellites.*.loop_position | string | PAUL loop state: IDLE, PLAN, APPLY, UNIFY |
| satellites.*.handoff | boolean | Whether a handoff file exists for this project |
