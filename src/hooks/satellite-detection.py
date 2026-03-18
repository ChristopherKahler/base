#!/usr/bin/env python3
"""
Hook: satellite-detection.py
Purpose: Scans the workspace recursively for .paul/paul.json files,
         cross-references registered satellites in workspace.json, and
         auto-registers any unregistered projects it finds.
Triggers: SessionStart — runs once when Claude Code starts a session.
Output: <base-satellites> block if new satellites registered, silent otherwise.
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
    activity_refreshed = []

    paul_files = find_paul_json_files(WORKSPACE_ROOT)

    for paul_json_path in paul_files:
        try:
            with open(paul_json_path, "r") as f:
                paul_data = json.load(f)
        except (json.JSONDecodeError, OSError):
            continue  # Malformed or unreadable — skip silently

        name = paul_data.get("name")
        if not name:
            continue  # No name field — skip

        # Read last_activity from paul.json timestamps (if present)
        last_activity = paul_data.get("timestamps", {}).get("updated_at")

        if name in satellites:
            # Already registered — refresh last_activity if available
            if last_activity and satellites[name].get("last_activity") != last_activity:
                satellites[name]["last_activity"] = last_activity
                activity_refreshed.append(name)
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

    if new_registrations or activity_refreshed:
        # Write updated manifest
        try:
            manifest["satellites"] = satellites
            with open(MANIFEST_FILE, "w") as f:
                json.dump(manifest, f, indent=2)
                f.write("\n")
        except OSError:
            sys.exit(0)  # Write failed — silent exit

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
