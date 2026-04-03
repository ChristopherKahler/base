#!/usr/bin/env python3
"""
BASE Hook v2: base-pulse-check-v2.py
Purpose: Workspace health check on session start.
         Reads .base/data/state.json (pre-calculated drift, areas, groom config).
         Much simpler than v1 which parsed STATE.md text + computed drift from file mtimes.
Triggers: UserPromptSubmit (session context)
Output: <base-pulse> workspace health status or groom reminder

Drop-in replacement for base-pulse-check.py. Swap in settings.json when ready.
Legacy base-pulse-check.py reads STATE.md + workspace.json (unchanged).
"""

import sys
import json
from datetime import datetime, date
from pathlib import Path

HOOK_DIR = Path(__file__).resolve().parent
WORKSPACE_ROOT = HOOK_DIR.parent.parent
BASE_DIR = WORKSPACE_ROOT / ".base"
STATE_FILE = BASE_DIR / "data" / "state.json"
PROJECTS_FILE = BASE_DIR / "data" / "projects.json"
CARL_DIR = WORKSPACE_ROOT / ".carl"
CARL_JSON = CARL_DIR / "carl.json"


def recalculate_drift(state):
    """Recalculate drift indicators from live data and update state.json.

    This ensures drift score is always fresh on session start, not stale
    from the last time base_update_drift was manually called.
    """
    now = date.today()

    # Calculate indicators from projects.json
    indicators = {
        "active_age_days": 0,
        "backlog_age_days": 0,
        "backlog_past_review": 0,
        "orphaned_sessions": 0,
        "untracked_root_files": 0,
        "stale_satellites": 0,
    }

    if PROJECTS_FILE.exists():
        try:
            projects = json.loads(PROJECTS_FILE.read_text())
            items = projects.get("items", [])

            # Active staleness: max days since update for active/in_progress/blocked/in_review projects
            active_statuses = {"in_progress", "blocked", "in_review", "todo"}
            active_ages = []
            backlog_ages = []
            past_review = 0

            for item in items:
                if item.get("type") != "project":
                    continue

                updated = item.get("updated_at")
                if updated:
                    try:
                        updated_date = datetime.fromisoformat(updated).date()
                        age = (now - updated_date).days
                    except (ValueError, TypeError):
                        age = 0
                else:
                    age = 0

                status = item.get("status", "")
                if status in active_statuses:
                    active_ages.append(age)
                elif status == "backlog":
                    backlog_ages.append(age)

                # Check review_by dates
                review_by = item.get("review_by")
                if review_by:
                    try:
                        review_date = date.fromisoformat(review_by)
                        if now > review_date:
                            past_review += 1
                    except (ValueError, TypeError):
                        pass

            indicators["active_age_days"] = max(active_ages) if active_ages else 0
            indicators["backlog_age_days"] = max(backlog_ages) if backlog_ages else 0
            indicators["backlog_past_review"] = past_review

        except (json.JSONDecodeError, OSError):
            pass

    # Stale satellites: check paul.json timestamps
    satellites = state.get("satellites", {})
    stale_sats = 0
    for name, sat in satellites.items():
        sat_path = WORKSPACE_ROOT / sat.get("path", "") / ".paul" / "paul.json"
        if sat_path.exists():
            try:
                paul = json.loads(sat_path.read_text())
                ts = paul.get("timestamps", {}).get("updated_at")
                if ts:
                    updated_date = datetime.fromisoformat(ts).date()
                    if (now - updated_date).days > 14:
                        stale_sats += 1
            except (json.JSONDecodeError, OSError, ValueError):
                pass
    indicators["stale_satellites"] = stale_sats

    # Compute score as sum of indicators
    score = sum(v for v in indicators.values() if isinstance(v, (int, float)))

    # Write back to state
    if "drift" not in state:
        state["drift"] = {}
    state["drift"]["score"] = score
    state["drift"]["indicators"] = indicators

    try:
        state["last_modified"] = datetime.now().isoformat()
        STATE_FILE.write_text(json.dumps(state, indent=2))
    except OSError:
        pass

    return state


def main():
    if not STATE_FILE.exists():
        sys.exit(0)

    try:
        state = json.loads(STATE_FILE.read_text())
    except (json.JSONDecodeError, OSError):
        sys.exit(0)

    # Self-heal: recalculate drift from live data every session start
    state = recalculate_drift(state)

    output_parts = []
    now = date.today()

    # Check groom overdue
    groom = state.get("groom", {})
    next_due = groom.get("next_groom_due")
    if next_due:
        try:
            due_date = date.fromisoformat(next_due)
            if now > due_date:
                last_groom = groom.get("last_groom", "unknown")
                overdue_days = (now - due_date).days
                output_parts.append(
                    f"BASE: Workspace groom overdue by {overdue_days} days "
                    f"(last groom: {last_groom}). "
                    f"Run /base:groom to maintain workspace health."
                )
        except ValueError:
            pass

    # Drift score and stale areas
    drift = state.get("drift", {})
    drift_score = drift.get("score", 0)
    areas = state.get("areas", {})
    stale_areas = [name for name, area in areas.items() if area.get("status") in ("stale", "critical")]

    if stale_areas:
        output_parts.append(
            f"BASE drift score: {drift_score} | Stale areas: {', '.join(stale_areas)}"
        )
    elif drift_score == 0:
        last_groom = groom.get("last_groom", "unknown")
        output_parts.append(
            f"BASE: Drift 0 | Last groom: {last_groom} | All areas current"
        )

    # CARL hygiene reminder
    carl_hygiene = state.get("carl_hygiene", {})
    if carl_hygiene.get("proactive", False):
        hygiene_cadence = {"weekly": 7, "bi-weekly": 14, "monthly": 30}.get(
            carl_hygiene.get("cadence", "monthly"), 30
        )
        last_run = carl_hygiene.get("last_run")
        if last_run:
            try:
                last_run_date = date.fromisoformat(last_run)
                days_since = (now - last_run_date).days
                if days_since > hygiene_cadence:
                    output_parts.append(
                        f"CARL hygiene overdue ({days_since}d since last run). Run /base:carl-hygiene"
                    )
            except ValueError:
                output_parts.append("CARL hygiene: last_run date invalid. Run /base:carl-hygiene")
        else:
            output_parts.append("CARL hygiene never run. Run /base:carl-hygiene when ready")

        # Check staging proposals in carl.json
        if CARL_JSON.exists():
            try:
                carl_data = json.loads(CARL_JSON.read_text())
                pending = [p for p in carl_data.get("staging", []) if p.get("status") == "pending"]
                if pending:
                    output_parts[-1] += f" | {len(pending)} staged proposals pending"
            except (json.JSONDecodeError, OSError):
                pass

    if output_parts:
        print(f"""<base-pulse>
{chr(10).join(output_parts)}
</base-pulse>""")

    sys.exit(0)


if __name__ == "__main__":
    main()
