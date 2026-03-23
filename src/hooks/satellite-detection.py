#!/usr/bin/env python3
"""
Hook: satellite-detection.py
Purpose: Scans the workspace recursively for .paul/paul.json files,
         auto-registers new satellites, and syncs paul.json state to
         workspace.json and projects.json.
Triggers: SessionStart — runs once when Claude Code starts a session.
Output: <base-satellites> block if new satellites registered, silent otherwise.

Sync flow (paul.json → workspace.json → projects.json):
  1. Discover paul.json files across workspace
  2. Register new satellites (existing behavior)
  3. Sync paul.json state to workspace.json satellite entries
  4. Cross-check projects.json: update paul field on matching projects
  Respects satellite.sync: false as opt-out for steps 3-4.
"""

import sys
import json
from datetime import datetime
from pathlib import Path

# Workspace root — find .base/ relative to this hook's location
HOOK_DIR = Path(__file__).resolve().parent
WORKSPACE_ROOT = HOOK_DIR.parent.parent  # hooks/ -> .base/ -> workspace
BASE_DIR = WORKSPACE_ROOT / ".base"
MANIFEST_FILE = BASE_DIR / "workspace.json"
PROJECTS_FILE = BASE_DIR / "data" / "projects.json"


def has_hidden_component(path: Path, workspace_root: Path) -> bool:
    """
    Return True if any component of path (relative to workspace_root) starts with '.',
    excluding '.paul' itself (which is the expected target directory).
    """
    try:
        rel = path.relative_to(workspace_root)
    except ValueError:
        return True  # Can't relativize — skip it
    return any(part.startswith(".") and part != ".paul" for part in rel.parts)


def find_paul_json_files(workspace_root: Path) -> list[Path]:
    """
    Recursively scan workspace_root for .paul/paul.json files.
    Skips any path that has a hidden directory component (starts with '.').
    """
    results = []
    try:
        for paul_json in workspace_root.rglob(".paul/paul.json"):
            if not has_hidden_component(paul_json, workspace_root):
                results.append(paul_json)
    except (OSError, PermissionError):
        pass
    return results


def should_sync(paul_data: dict) -> bool:
    """Check if this satellite opts into sync. Default: True."""
    satellite = paul_data.get("satellite", {})
    return satellite.get("sync", True)


def sync_to_workspace(satellites: dict, paul_data: dict, name: str) -> bool:
    """Sync paul.json state to workspace.json satellite entry. Returns True if changed."""
    if name not in satellites:
        return False

    sat = satellites[name]
    changed = False

    phase = paul_data.get("phase", {})
    loop = paul_data.get("loop", {})
    handoff = paul_data.get("handoff", {})

    updates = {
        "phase_name": phase.get("name"),
        "phase_number": phase.get("number"),
        "phase_status": phase.get("status"),
        "loop_position": loop.get("position"),
        "handoff": handoff.get("present", False),
        "last_plan_completed_at": paul_data.get("last_plan_completed_at"),
        "next_action": paul_data.get("next_action"),
    }

    for key, value in updates.items():
        if sat.get(key) != value:
            sat[key] = value
            changed = True

    return changed


def sync_to_projects(paul_data: dict, name: str, projects_data: dict) -> bool:
    """Sync paul.json state to matching project in projects.json. Returns True if changed."""
    for item in projects_data.get("items", []):
        paul_field = item.get("paul")
        if not paul_field or paul_field.get("satellite_name") != name:
            continue

        # Found matching project — update paul field
        phase = paul_data.get("phase", {})
        loop = paul_data.get("loop", {})
        handoff = paul_data.get("handoff", {})
        timestamps = paul_data.get("timestamps", {})

        paul_field["phase"] = phase.get("name")
        paul_field["loop_position"] = loop.get("position")
        paul_field["last_update"] = timestamps.get("updated_at")

        # Calculate completed phases
        if phase.get("status") == "complete":
            paul_field["completed_phases"] = phase.get("number")
        else:
            paul_field["completed_phases"] = max(0, (phase.get("number", 1) or 1) - 1)

        # Copy enriched fields
        paul_field["last_plan_completed_at"] = paul_data.get("last_plan_completed_at")
        paul_field["handoff"] = handoff.get("present", False)

        item["updated_at"] = datetime.now().isoformat()
        return True

    return False


def main():
    # Skip if BASE is not installed
    if not BASE_DIR.exists() or not MANIFEST_FILE.exists():
        sys.exit(0)

    try:
        with open(MANIFEST_FILE, "r") as f:
            manifest = json.load(f)
    except (json.JSONDecodeError, OSError):
        sys.exit(0)

    satellites = manifest.get("satellites", {})
    new_registrations = []
    workspace_changed = False
    projects_changed = False

    # Load projects.json for cross-check (if it exists)
    projects_data = None
    if PROJECTS_FILE.exists():
        try:
            with open(PROJECTS_FILE, "r") as f:
                projects_data = json.load(f)
        except (json.JSONDecodeError, OSError):
            projects_data = None

    paul_files = find_paul_json_files(WORKSPACE_ROOT)

    # Collect paul data for sync pass
    paul_registry = {}  # name → paul_data

    for paul_json_path in paul_files:
        try:
            with open(paul_json_path, "r") as f:
                paul_data = json.load(f)
        except (json.JSONDecodeError, OSError):
            continue  # Malformed or unreadable — skip silently

        name = paul_data.get("name")
        if not name:
            continue  # No name field — skip

        paul_registry[name] = paul_data

        # Read last_activity from paul.json timestamps (if present)
        last_activity = paul_data.get("timestamps", {}).get("updated_at")

        if name in satellites:
            # Already registered — refresh last_activity if available
            if last_activity and satellites[name].get("last_activity") != last_activity:
                satellites[name]["last_activity"] = last_activity
                workspace_changed = True
            continue

        # New satellite — derive relative path
        project_dir = paul_json_path.parent.parent
        try:
            rel_path = str(project_dir.relative_to(WORKSPACE_ROOT))
        except ValueError:
            continue  # Can't relativize — skip

        # Build registration entry
        entry = {
            "path": rel_path,
            "engine": "paul",
            "state": f"{rel_path}/.paul/STATE.md",
            "registered": datetime.now().strftime("%Y-%m-%d"),
            "groom_check": True,
        }
        if last_activity:
            entry["last_activity"] = last_activity

        satellites[name] = entry
        new_registrations.append(name)
        workspace_changed = True

    # --- Sync pass: paul.json → workspace.json + projects.json ---
    for name, paul_data in paul_registry.items():
        if not should_sync(paul_data):
            continue  # Opt-out — skip sync

        # Sync to workspace.json
        if sync_to_workspace(satellites, paul_data, name):
            workspace_changed = True

        # Sync to projects.json
        if projects_data and sync_to_projects(paul_data, name, projects_data):
            projects_changed = True

    # Write workspace.json if changed
    if workspace_changed:
        try:
            manifest["satellites"] = satellites
            with open(MANIFEST_FILE, "w") as f:
                json.dump(manifest, f, indent=2)
                f.write("\n")
        except OSError:
            pass  # Write failed — silent

    # Write projects.json if changed
    if projects_changed and projects_data:
        try:
            projects_data["last_modified"] = datetime.now().isoformat()
            with open(PROJECTS_FILE, "w") as f:
                json.dump(projects_data, f, indent=2)
                f.write("\n")
        except OSError:
            pass  # Write failed — silent

    # Output only for new registrations
    if new_registrations:
        names_str = ", ".join(new_registrations)
        n = len(new_registrations)
        print(f"<base-satellites>\nAuto-registered {n} new satellite(s): {names_str}\n</base-satellites>")

    sys.exit(0)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        sys.exit(0)
