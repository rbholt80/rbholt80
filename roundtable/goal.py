#!/usr/bin/env python3
"""goal.py -- a minimal command-line driver for Roundtable's goal mode.

Not the app's own UI (that wiring into cli.py/web.py is still in progress on
this same branch) -- a thin, standalone script so goal mode can actually be
driven today instead of only through raw Python calls.

    python3 goal.py create "add a --verbose flag" --project ~/some/repo
    python3 goal.py create "what do people complain about with X" --mode research
    python3 goal.py list
    python3 goal.py show <id>
    python3 goal.py resume <id> --feedback "also handle the empty case"
    python3 goal.py outcome <id> "sold for $40"
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from roundtable import config
from roundtable.work import WorkManager

ROOT = Path.home() / ".roundtable-goals"


def make_manager(quiet: bool = False) -> WorkManager:
    seats, _ = config.resolve(include_cli=True)

    def notify(state: dict) -> None:
        if quiet:
            return
        current = state.get("current")
        if not current:
            return
        goal = next((g for g in state["goals"] if g["id"] == current), None)
        if goal:
            print(f"  [{goal['worker'] or '...'}] {goal['message']}")

    return WorkManager(ROOT, seats, notify=notify)


def _watch(manager: WorkManager, identifier: str) -> None:
    while manager.busy and manager.current == identifier:
        time.sleep(0.3)
    goal = manager.get(identifier)
    print(f"\nStatus: {goal['status']}")
    print(goal["message"])
    if goal["status"] == "ready":
        print(f"\nSummary: {goal['summary']}")
        patch = manager.root / identifier / "changes.patch"
        if patch.is_file() and patch.stat().st_size:
            print(f"Patch:   {patch}")
        deliverable = manager.root / identifier / "workspace" / "deliverable.md"
        if deliverable.is_file():
            print(f"Report:  {deliverable}")
    print(f"\n(full detail: python3 goal.py show {identifier})")


def cmd_create(args: argparse.Namespace) -> None:
    manager = make_manager()
    identifier = manager.create(args.task, mode=args.mode, project=args.project or "",
                                max_steps=args.steps)
    print(f"Created goal {identifier}")
    if args.no_start:
        print(f"Not started. Run: python3 goal.py resume {identifier}")
        return
    manager.start(identifier)
    _watch(manager, identifier)


def cmd_resume(args: argparse.Namespace) -> None:
    manager = make_manager()
    manager.start(args.id, feedback=args.feedback or "", extra_steps=args.steps)
    _watch(manager, args.id)


def cmd_list(args: argparse.Namespace) -> None:
    manager = make_manager(quiet=True)
    state = manager.snapshot()
    if not state["goals"]:
        print("No goals yet. Create one with: python3 goal.py create \"...\"")
        return
    for g in state["goals"]:
        print(f"{g['id']}  [{g['mode']:8s}] [{g['status']:11s}] {g['task'][:60]}")


def cmd_show(args: argparse.Namespace) -> None:
    manager = make_manager(quiet=True)
    goal = manager.get(args.id)
    print(json.dumps(goal, indent=2, ensure_ascii=False)[:8000])


def cmd_outcome(args: argparse.Namespace) -> None:
    manager = make_manager(quiet=True)
    manager.outcome(args.id, {"note": args.note})
    print("Recorded.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    create = sub.add_parser("create", help="create a goal and start it immediately")
    create.add_argument("task")
    create.add_argument("--mode", choices=["coding", "research", "ideas", "money"],
                        default="coding")
    create.add_argument("--project", default="",
                        help="path to an existing project (coding mode only)")
    create.add_argument("--steps", type=int, default=20)
    create.add_argument("--no-start", action="store_true", help="create without starting")
    create.set_defaults(func=cmd_create)

    resume = sub.add_parser("resume", help="continue a paused/needs-input goal")
    resume.add_argument("id")
    resume.add_argument("--feedback", default="")
    resume.add_argument("--steps", type=int, default=0, help="additional step budget")
    resume.set_defaults(func=cmd_resume)

    listp = sub.add_parser("list", help="list all goals")
    listp.set_defaults(func=cmd_list)

    show = sub.add_parser("show", help="full JSON detail for one goal")
    show.add_argument("id")
    show.set_defaults(func=cmd_show)

    outcome = sub.add_parser("outcome",
                             help="record a real-world result (payment, time spent)")
    outcome.add_argument("id")
    outcome.add_argument("note")
    outcome.set_defaults(func=cmd_outcome)

    args = parser.parse_args()
    try:
        args.func(args)
    except ValueError as exc:
        sys.exit(f"error: {exc}")


if __name__ == "__main__":
    main()
