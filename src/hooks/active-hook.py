#!/usr/bin/env python3
"""
BASE Hook v2: active-hook-v2.py
Source: .base/data/projects.json (APEX unified project management)
Output: <active-awareness> compact summary grouped by priority
Filters: items with status NOT in [backlog, archived]

Drop-in replacement for active-hook.py. Swap in settings.json when ready.
Legacy active-hook.py reads from .base/data/active.json (unchanged).
"""

import sys
import json
from pathlib import Path
from datetime import date, datetime

SURFACE_NAME = "active"

HOOK_DIR = Path(__file__).resolve().parent
WORKSPACE_ROOT = HOOK_DIR.parent.parent
DATA_FILE = WORKSPACE_ROOT / ".base" / "data" / "projects.json"

BEHAVIOR_DIRECTIVE = f"""BEHAVIOR: This context is PASSIVE AWARENESS ONLY.
Do NOT proactively mention these items unless:
  - User explicitly asks (e.g., "what should I work on?", "what's next?")
  - A deadline is within 24 hours AND user hasn't acknowledged it this session
For details on any item, use apex_get_project(id)."""

PRIORITY_ORDER = ["urgent", "high", "medium", "low", "ongoing", "deferred"]

# Staleness thresholds (days since last update)
STALE_THRESHOLDS = {
    "urgent": 3,
    "high": 5,
    "medium": 7,
    "low": 14,
    "ongoing": 14,
    "deferred": 30,
}

# Statuses that are NOT active (excluded from active view)
EXCLUDED_STATUSES = {"backlog", "archived", "completed"}

# Types to exclude from active awareness (checked via MCP during grooms)
EXCLUDED_TYPES = {"initiative"}


def days_since_update(item):
    """Calculate days since last update. Uses updated_at (ISO datetime)."""
    ts = item.get("updated_at") or item.get("created_at")
    if not ts:
        return None
    try:
        d = date.fromisoformat(ts[:10])
        return (date.today() - d).days
    except (ValueError, TypeError):
        return None


def main():
    try:
        input_data = json.loads(sys.stdin.read())
    except (json.JSONDecodeError, OSError):
        pass

    if not DATA_FILE.exists():
        sys.exit(0)

    try:
        data = json.loads(DATA_FILE.read_text())
    except (json.JSONDecodeError, OSError):
        sys.exit(0)

    items = data.get("items", [])
    if not items:
        sys.exit(0)

    # Filter: only active items (not backlog, archived, completed), exclude initiatives
    active_items = [i for i in items if i.get("status") not in EXCLUDED_STATUSES and i.get("type") not in EXCLUDED_TYPES]
    if not active_items:
        sys.exit(0)

    # Group by priority
    groups = {}
    for item in active_items:
        p = item.get("priority", "medium")
        groups.setdefault(p, []).append(item)

    # Workload balance header
    blocked_count = sum(1 for i in active_items if i.get("blocked_by"))
    ongoing_count = sum(1 for i in active_items if i.get("priority") == "ongoing")
    deferred_count = sum(1 for i in active_items if i.get("status") == "deferred")
    working_count = len(active_items) - ongoing_count - deferred_count
    lines = [f"Load: {working_count} active | {blocked_count} blocked | {ongoing_count} ongoing | {deferred_count} deferred"]

    for priority in PRIORITY_ORDER:
        group = groups.get(priority, [])
        if not group:
            continue
        lines.append(f"[{priority.upper()}]")
        for item in group:
            item_id = item.get("id", "?")
            title = item.get("title", "untitled")
            status = item.get("status", "")
            category = item.get("category", "")
            cat_tag = f"({category}) " if category else ""
            parts = [f"- [{item_id}] {cat_tag}{title}"]
            if status:
                parts[0] += f" ({status})"
            # PAUL signal (phase, loop, plan age, handoff) — only if paul data has real values
            paul_info = item.get("paul")
            if paul_info and paul_info.get("is_paul_project") and paul_info.get("phase"):
                paul_parts = []
                p_phase = paul_info.get("phase", "?")
                p_completed = paul_info.get("completed_phases", "?")
                p_total = paul_info.get("total_phases", "?")
                p_loop = paul_info.get("loop_position", "?")
                paul_parts.append(f"Phase {p_completed}/{p_total} ({p_phase})")
                paul_parts.append(str(p_loop))
                # Plan age
                last_plan = paul_info.get("last_plan_completed_at") or paul_info.get("last_update")
                if last_plan:
                    try:
                        lp = last_plan.replace("Z", "+00:00")
                        if "T" in lp:
                            lp_date = datetime.fromisoformat(lp).date() if hasattr(datetime, 'fromisoformat') else date.fromisoformat(lp[:10])
                        else:
                            lp_date = date.fromisoformat(lp)
                        age = (date.today() - lp_date).days
                        paul_parts.append(f"plan {age}d ago")
                    except (ValueError, TypeError):
                        pass
                # Handoff flag
                p_handoff = paul_info.get("handoff")
                if isinstance(p_handoff, dict) and p_handoff.get("present"):
                    paul_parts.append("HANDOFF")
                elif isinstance(p_handoff, bool) and p_handoff:
                    paul_parts.append("HANDOFF")
                parts.append(f"  PAUL: {' | '.join(paul_parts)}")

            # Revenue signal
            rev = item.get("revenue")
            if rev and rev.get("amount"):
                rev_type = rev.get("type", "")
                parts.append(f"  REV: {rev['amount']} ({rev_type})")

            blocked = item.get("blocked_by")
            if blocked:
                parts.append(f"  BLOCKED: {blocked}")
            next_action = item.get("next")
            if next_action and priority != "ongoing":
                parts.append(f"  NEXT: {next_action}")
            deadline = item.get("due_date")
            if deadline:
                parts.append(f"  DUE: {deadline}")
            days = days_since_update(item)
            threshold = STALE_THRESHOLDS.get(priority, 7)
            if days is not None:
                if days >= threshold:
                    parts.append(f"  STALE: {days}d since update (threshold: {threshold}d)")
                else:
                    parts.append(f"  updated: {days}d ago")
            lines.append("\n".join(parts))

    if lines:
        count = len(active_items)
        summary = "\n".join(lines)
        print(f"""<{SURFACE_NAME}-awareness items="{count}">
{summary}

{BEHAVIOR_DIRECTIVE}
</{SURFACE_NAME}-awareness>""")

    sys.exit(0)


if __name__ == "__main__":
    main()
