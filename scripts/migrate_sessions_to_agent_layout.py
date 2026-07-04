#!/usr/bin/env python3
"""Migrate legacy session layouts to the agent-based layout.

A legacy session directory looks like::

    <neo-home>/sessions/<workspace>/<session-id>/
        transcript.jsonl

After migration it looks like::

    <neo-home>/sessions/<workspace>/<session-id>/
        agents/main/wire.jsonl   # moved from transcript.jsonl
        state.json               # describes the agent layout

Usage::

    python3 migrate_sessions_to_agent_layout.py --neo-home <dir> \
        [--apply|--dry-run] [--no-backup]
"""

import argparse
import json
import os
import shutil
import sys

LEGACY_TRANSCRIPT = "transcript.jsonl"
WIRE_REL = os.path.join("agents", "main", "wire.jsonl")
STATE_NAME = "state.json"
BACKUP_SUFFIX = ".pre-agent-layout-backup"


def session_state_json():
    return {
        "schema_version": 1,
        "agents": {
            "main": {"kind": "main", "record_dir": "agents/main"},
        },
    }


def write_state(session_dir):
    state = session_state_json()
    state_path = os.path.join(session_dir, STATE_NAME)
    with open(state_path, "w", encoding="utf-8") as handle:
        json.dump(state, handle, sort_keys=True)


def next_backup_dir(session_dir):
    """Return a backup directory path that does not yet exist.

    The base backup lives next to the session directory and is named
    ``<session-id>.pre-agent-layout-backup``. If that already exists, a numeric
    suffix (``.1``, ``.2``, ...) is appended while preserving any prior backup.
    """
    session_name = os.path.basename(session_dir.rstrip(os.sep))
    parent = os.path.dirname(session_dir.rstrip(os.sep))
    base = os.path.join(parent, session_name + BACKUP_SUFFIX)
    if not os.path.exists(base):
        return base
    index = 1
    while True:
        candidate = "{}.{}".format(base, index)
        if not os.path.exists(candidate):
            return candidate
        index += 1


def backup_session(session_dir):
    backup_dir = next_backup_dir(session_dir)
    shutil.copytree(session_dir, backup_dir)


def discover_session_dirs(sessions_dir):
    """Find candidate session directories under ``sessions/``.

    A session directory is any directory that directly contains a legacy
    ``transcript.jsonl`` or an ``agents/main/wire.jsonl``. The list is computed
    up front so that backups created during processing are not re-processed.
    """
    found = []
    if not os.path.isdir(sessions_dir):
        return found
    for root, dirs, _files in os.walk(sessions_dir):
        has_transcript = os.path.isfile(os.path.join(root, LEGACY_TRANSCRIPT))
        has_wire = os.path.isfile(os.path.join(root, WIRE_REL))
        if has_transcript or has_wire:
            found.append(root)
    return found


def process(session_dir, apply, no_backup):
    """Process a single session directory.

    Returns True on success or if nothing needed doing. Returns False to signal
    an unrecoverable failure (caller exits non-zero).
    """
    transcript = os.path.join(session_dir, LEGACY_TRANSCRIPT)
    wire = os.path.join(session_dir, WIRE_REL)
    state_path = os.path.join(session_dir, STATE_NAME)

    transcript_exists = os.path.isfile(transcript)
    wire_exists = os.path.isfile(wire)
    state_exists = os.path.isfile(state_path)

    if transcript_exists:
        # Migration (or repair if wire already present).
        if not apply:
            print("would-migrate: {}".format(session_dir))
            return True

        if not no_backup:
            backup_session(session_dir)

        if wire_exists:
            # Repair: wire.jsonl already there, just finalize state + drop transcript.
            write_state(session_dir)
            os.remove(transcript)
            print("repaired: {}".format(session_dir))
            return True

        wire_dir = os.path.dirname(wire)
        os.makedirs(wire_dir, exist_ok=True)
        shutil.move(transcript, wire)
        write_state(session_dir)
        print("migrated: {}".format(session_dir))
        return True

    # No transcript remaining.
    if wire_exists and not state_exists:
        # Half-migrated with no way to finalize: unrecoverable.
        print(
            "failed: session {} has wire.jsonl but no transcript.jsonl "
            "and no state.json".format(session_dir),
            file=sys.stderr,
        )
        return False

    # Already migrated (wire + state) or empty: nothing to do.
    return True


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Migrate legacy session layouts to the agent-based layout."
    )
    parser.add_argument("--neo-home", required=True, help="Path to the neo home directory.")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--apply", action="store_true", help="Perform the migration.")
    mode.add_argument("--dry-run", action="store_true", help="Only report what would happen.")
    parser.add_argument("--no-backup", action="store_true", help="Skip creating a backup copy.")
    args = parser.parse_args(argv)

    apply = args.apply or not args.dry_run
    # Default to dry-run when neither flag is given.
    if not args.apply and not args.dry_run:
        apply = False

    sessions_dir = os.path.join(args.neo_home, "sessions")
    session_dirs = discover_session_dirs(sessions_dir)

    if not session_dirs:
        print("no sessions to migrate under {}".format(sessions_dir))
        return 0

    exit_code = 0
    for session_dir in session_dirs:
        if not process(session_dir, apply=apply, no_backup=args.no_backup):
            exit_code = 1
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
