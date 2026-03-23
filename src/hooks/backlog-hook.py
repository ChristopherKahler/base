#!/usr/bin/env python3
"""
BASE Hook v2: backlog-hook-v2.py
Source: .base/data/projects.json (APEX unified project management)
Output: <backlog-awareness> compact summary grouped by priority
Filters: only items with status "backlog"

Drop-in replacement for backlog-hook.py. Swap in settings.json when ready.
Legacy backlog-hook.py reads from .base/data/backlog.json (unchanged).
"""

import sys
import json
from pathlib import Path
from datetime import date

SURFACE_NAME = "backlog"

HOOK_DIR = Path(__file__).resolve().parent
WORKSPACE_ROOT = HOOK_DIR.parent.parent
DATA_FILE = WORKSPACE_ROOT / ".base" / "data" / "projects.json"

BEHAVIOR_DIRECTIVE = f"""BEHAVIOR: This context is PASSIVE AWARENESS ONLY.
Do NOT proactively mention these items unless:
  - User explicitly asks (e.g., "what's in the backlog?", "what's queued?")
  - A review_by date has passed AND user hasn't acknowledged it this session
For details on any item, use apex_get_project(id)."""

PRIORITY_ORDER = ["high", "medium", "low"]

# Staleness thresholds (days since last update)
STALE_THRESHOLDS = {
    "high": 7,
    "medium": 14,
    "low": 30,
}


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

    # Filter: only backlog items
    backlog_items = [i for i in items if i.get("status") == "backlog"]
    if not backlog_items:
        sys.exit(0)

    # Group by priority
    groups = {}
    for item in backlog_items:
        p = item.get("priority", "medium")
        groups.setdefault(p, []).append(item)

    lines = []
    for priority in PRIORITY_ORDER:
        group = groups.get(priority, [])
        if not group:
            continue
        lines.append(f"[{priority.upper()}]")
        for item in group:
            item_id = item.get("id", "?")
            title = item.get("title", "untitled")
            review_by = item.get("review_by")
            entry = f"- [{item_id}] {title}"
            if review_by:
                entry += f" [review by: {review_by}]"
            days = days_since_update(item)
            threshold = STALE_THRESHOLDS.get(priority, 14)
            if days is not None:
                if days >= threshold:
                    entry += f" STALE: {days}d"
                else:
                    entry += f" ({days}d ago)"
            lines.append(entry)

    if lines:
        count = len(backlog_items)
        summary = "\n".join(lines)
        print(f"""<{SURFACE_NAME}-awareness items="{count}">
{summary}

{BEHAVIOR_DIRECTIVE}
</{SURFACE_NAME}-awareness>""")

    sys.exit(0)


if __name__ == "__main__":
    main()
