#!/usr/bin/env python3
"""
APEX Insights — On-demand workspace analytics
Computes velocity, stall detection, blocking analysis, workload, and dependency chains.
Invoked by /apex:insights slash command via !command injection.
"""

import json
import sys
from datetime import datetime, date
from pathlib import Path
from collections import defaultdict

WORKSPACE = Path(__file__).resolve().parent.parent.parent
PROJECTS_FILE = WORKSPACE / ".base" / "data" / "projects.json"
WORKSPACE_JSON = WORKSPACE / ".base" / "workspace.json"


def load_json(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (json.JSONDecodeError, OSError):
        return None


def days_ago(iso_str):
    if not iso_str:
        return None
    try:
        s = iso_str.replace("Z", "+00:00")
        if "T" in s:
            d = datetime.fromisoformat(s).date()
        else:
            d = date.fromisoformat(s[:10])
        return (date.today() - d).days
    except (ValueError, TypeError):
        return None


def main():
    projects = load_json(PROJECTS_FILE)
    workspace = load_json(WORKSPACE_JSON)

    if not projects:
        print("ERROR: Cannot read projects.json")
        sys.exit(0)

    items = projects.get("items", [])
    satellites = (workspace or {}).get("satellites", {})

    # --- VELOCITY ---
    print("## VELOCITY (PAUL Projects)")
    paul_projects = []
    for item in items:
        paul = item.get("paul")
        if paul and paul.get("is_paul_project") and paul.get("phase"):
            lp_age = days_ago(paul.get("last_plan_completed_at") or paul.get("last_update"))
            paul_projects.append({
                "id": item["id"],
                "title": item["title"][:35],
                "phase": f"{paul.get('completed_phases', '?')}/{paul.get('total_phases', '?')}",
                "loop": paul.get("loop_position", "?"),
                "last_plan_age": lp_age,
                "handoff": paul.get("handoff", False),
                "status": item.get("status"),
            })

    if paul_projects:
        for p in sorted(paul_projects, key=lambda x: (x["last_plan_age"] or 0), reverse=True):
            age_str = f"{p['last_plan_age']}d ago" if p["last_plan_age"] is not None else "never"
            hf = " [HANDOFF]" if (isinstance(p["handoff"], dict) and p["handoff"].get("present")) or p["handoff"] is True else ""
            print(f"  {p['id']} {p['title']:35s} Phase {p['phase']:8s} {p['loop']:5s} plan: {age_str}{hf}")
    else:
        print("  No PAUL projects found")
    print()

    # --- STALLS (active projects with plan age > 14d) ---
    print("## STALLS (plan age > 14 days, not completed/deferred)")
    stalls = [p for p in paul_projects
              if p["last_plan_age"] is not None
              and p["last_plan_age"] > 14
              and p["status"] not in ("completed", "deferred", "archived")]
    if stalls:
        for s in sorted(stalls, key=lambda x: x["last_plan_age"], reverse=True):
            print(f"  {s['id']} {s['title']:35s} STALLED {s['last_plan_age']}d")
    else:
        print("  No stalls detected")
    print()

    # --- BLOCKING ANALYSIS ---
    print("## BLOCKING ANALYSIS")
    blocked = [i for i in items if i.get("blocked_by") and i.get("status") not in ("completed", "archived")]
    if blocked:
        # Group by blocker
        blockers = defaultdict(list)
        for item in blocked:
            blockers[item["blocked_by"]].append(item)

        for blocker, items_blocked in blockers.items():
            rev_items = [i for i in items_blocked if i.get("revenue")]
            rev_str = ""
            if rev_items:
                rev_str = f" | Revenue at risk: {', '.join(i['revenue']['amount'] for i in rev_items)}"
            print(f"  Blocker: {blocker}")
            for i in items_blocked:
                print(f"    {i['id']} {i['title'][:40]}")
            if rev_str:
                print(f"    {rev_str}")
            print()
    else:
        print("  No blocked projects")
    print()

    # --- DEPENDENCIES ---
    print("## CROSS-PROJECT DEPENDENCIES")
    has_deps = [i for i in items if i.get("dependencies")]
    if has_deps:
        for item in has_deps:
            for dep in item["dependencies"]:
                dep_project = next((i for i in items if i["id"] == dep["project_id"]), None)
                dep_title = dep_project["title"][:30] if dep_project else dep["project_id"]
                print(f"  {item['id']} {item['title'][:30]} --{dep['type']}--> {dep_title}")
                if dep.get("notes"):
                    print(f"    Note: {dep['notes']}")
    else:
        print("  No cross-project dependencies defined")
    print()

    # --- WORKLOAD BY CATEGORY ---
    print("## WORKLOAD BY CATEGORY")
    active = [i for i in items if i.get("status") not in ("backlog", "archived", "completed") and i.get("type") != "initiative"]
    cats = defaultdict(int)
    for item in active:
        cats[item.get("category", "uncategorized")] += 1
    for cat, count in sorted(cats.items(), key=lambda x: -x[1]):
        print(f"  {cat}: {count} projects")
    print()

    # --- REVENUE SUMMARY ---
    print("## REVENUE EXPOSURE")
    rev_projects = [i for i in items if i.get("revenue") and i.get("status") not in ("completed", "archived")]
    if rev_projects:
        for item in rev_projects:
            rev = item["revenue"]
            status = item.get("status", "?")
            blocked_flag = " [BLOCKED]" if item.get("blocked_by") else ""
            print(f"  {item['id']} {item['title'][:35]} | {rev['amount']} ({rev['type']}){blocked_flag}")
    else:
        print("  No revenue projects active")
    print()

    # --- HANDOFFS ---
    print("## PENDING HANDOFFS")
    handoff_sats = [(name, sat) for name, sat in satellites.items() if sat.get("handoff")]
    if handoff_sats:
        for name, sat in handoff_sats:
            phase = sat.get("phase_name", "?")
            print(f"  {name}: Phase {phase} — has HANDOFF waiting")
    else:
        print("  No pending handoffs")


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(f"ERROR: {e}")
        sys.exit(0)
