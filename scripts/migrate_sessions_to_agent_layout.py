#!/usr/bin/env python3
import argparse
import json
import shutil
import sys
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Migrate Neo session transcripts to the agent record layout."
    )
    parser.add_argument("--neo-home", default="~/.neo", help="Neo home directory")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--apply", action="store_true", help="write migration changes")
    mode.add_argument("--dry-run", action="store_true", help="print planned migrations only")
    parser.add_argument(
        "--no-backup",
        action="store_true",
        help="do not create a .pre-agent-layout-backup copy before applying",
    )
    return parser.parse_args()


def state_json() -> str:
    state = {
        "schema_version": 1,
        "agents": {
            "main": {
                "kind": "main",
                "record_dir": "agents/main",
                "parent_agent_id": None,
            }
        },
    }
    return json.dumps(state, indent=2) + "\n"


def iter_sessions(sessions_root: Path) -> list[Path]:
    if not sessions_root.is_dir():
        return []

    sessions: list[Path] = []
    for bucket_dir in sorted(sessions_root.iterdir()):
        if not bucket_dir.is_dir():
            continue
        for session_dir in sorted(bucket_dir.glob("session_*")):
            if session_dir.is_dir() and ".pre-agent-layout-backup" not in session_dir.name:
                sessions.append(session_dir)
    return sessions


def backup_path(session_dir: Path) -> Path:
    base = session_dir.with_name(f"{session_dir.name}.pre-agent-layout-backup")
    if not base.exists():
        return base

    index = 1
    while True:
        candidate = session_dir.with_name(
            f"{session_dir.name}.pre-agent-layout-backup.{index}"
        )
        if not candidate.exists():
            return candidate
        index += 1


def backup_session(session_dir: Path) -> None:
    shutil.copytree(session_dir, backup_path(session_dir))


def copy_transcript(transcript_path: Path, wire_path: Path) -> None:
    wire_path.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(transcript_path, wire_path)
    if transcript_path.read_bytes() != wire_path.read_bytes():
        raise RuntimeError("copied transcript does not match agent wire")


def migrate_session(session_dir: Path, apply: bool, backup: bool) -> None:
    transcript_path = session_dir / "transcript.jsonl"
    agent_dir = session_dir / "agents" / "main"
    wire_path = agent_dir / "wire.jsonl"
    state_path = session_dir / "state.json"

    if wire_path.is_file() and state_path.is_file() and not transcript_path.exists():
        print(f"skipped:new-layout\t{session_dir}")
        return
    if wire_path.is_file() and not state_path.is_file() and not transcript_path.is_file():
        raise RuntimeError("agent wire exists without state.json or legacy transcript")
    if not transcript_path.is_file():
        print(f"skipped:no-transcript\t{session_dir}")
        return
    if not apply:
        print(f"would-migrate\t{session_dir}")
        return

    if backup:
        backup_session(session_dir)

    copy_transcript(transcript_path, wire_path)
    state_path.write_text(state_json(), encoding="utf-8")
    transcript_path.unlink()
    print(f"migrated\t{session_dir}")


def main() -> int:
    args = parse_args()
    neo_home = Path(args.neo_home).expanduser()
    sessions_root = neo_home / "sessions"
    sessions = iter_sessions(sessions_root)
    if not sessions:
        print(f"skipped:no-sessions\t{sessions_root}")
        return 0

    failed = False
    for session_dir in sessions:
        try:
            migrate_session(session_dir, args.apply, not args.no_backup)
        except Exception as error:  # noqa: BLE001 - top-level migration keeps going.
            failed = True
            print(f"failed\t{session_dir}\t{error}", file=sys.stderr)

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
