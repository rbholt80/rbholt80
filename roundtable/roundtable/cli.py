"""Terminal front end."""

from __future__ import annotations

import argparse
import sys
import threading
import time

from . import __version__, config
from .config import Participant
from .engine import Roundtable

RESET = "\033[0m"
DIM = "\033[2m"
BOLD = "\033[1m"
HOST_COLOR = "\033[1;37m"

HELP = """\
  Enter            let the next model speak
  <text>           join in; @Name hands the floor to that seat
  /auto [n]        models keep talking (n turns; defaults to one round)
  /next <Name>     put a specific model up next
  /who             who is at the table
  /save            write a markdown transcript now
  /quit            exit
"""


def _print_roster(seats: list[Participant]) -> None:
    for p in seats:
        detail = p.model or p.kind
        if p.kind == "cli":
            detail += " (CLI)"
        note = f"  {DIM}{p.note}{RESET}" if p.note else ""
        print(f"  {p.color}●{RESET} {p.name:<12} {DIM}{detail}{RESET}{note}")


def _render(table: Roundtable, events, color: str) -> None:
    """Paint one streamed turn."""
    for event in events:
        if event["type"] == "start":
            sys.stdout.write(f"\n{color}{event['speaker']}:{RESET} ")
            sys.stdout.flush()
        elif event["type"] == "chunk":
            sys.stdout.write(event["text"])
            sys.stdout.flush()
        elif event["type"] == "end":
            print()


def cmd_doctor(args: argparse.Namespace) -> int:
    """Report what this machine can seat, without starting anything."""
    print(f"{BOLD}Roundtable {__version__} — what I can see here{RESET}\n")
    everything = config.discover(include_cli=True, dedupe=False)
    seats = config.paint(config.discover(include_cli=True))
    if not seats:
        print("  Nothing found.\n")
        print("  Set one or more of these and try again:")
        print("    ANTHROPIC_API_KEY, OPENAI_API_KEY, XAI_API_KEY, GEMINI_API_KEY,")
        print("    DEEPSEEK_API_KEY, GROQ_API_KEY, MISTRAL_API_KEY, OPENROUTER_API_KEY")
        print("  ...or run `ollama serve`, or install the claude/codex/gemini CLIs.")
        return 1

    for source, label in (("api", "Hosted APIs"), ("local", "Local servers"),
                          ("cli", "Installed CLIs")):
        group = [p for p in seats if p.source == source]
        if group:
            print(f"{BOLD}{label}{RESET}")
            _print_roster(group)
            print()

    seated = {p.name for p in seats}
    shadowed = sorted(p.name for p in everything if p.name not in seated)
    if shadowed:
        print(f"{DIM}Installed, but not auto-seated — the API seat covers the same")
        print(f"model without the tool access: {', '.join(shadowed)}{RESET}")
        print(f"{DIM}Seat one anyway with --only, or in roundtable.toml.{RESET}\n")

    if (blocked := config.blocked()):
        print(f"{BOLD}Nearly there{RESET}")
        for name, remedy in blocked:
            print(f"  {DIM}○{RESET} {name:<12} {DIM}{remedy}{RESET}")
        print()

    print(f"{len(seats)} seat(s) ready.")
    return 0


def _build(args: argparse.Namespace) -> tuple[Roundtable, dict]:
    seats, settings = config.resolve(
        config_path=args.config,
        only=args.only.split(",") if args.only else None,
        include_cli=not args.no_cli,
    )
    if not seats:
        raise SystemExit(
            "No models available. Run `roundtable doctor` to see what's missing.")

    topic = " ".join(args.topic).strip() or settings.get("topic", "")
    if not topic:
        topic = input("Topic: ").strip()
    if not topic:
        raise SystemExit("A roundtable needs a topic.")

    table = Roundtable(
        topic=topic,
        participants=seats,
        transcript_dir=args.transcripts or settings.get("transcripts", "."),
        policy=args.policy or settings.get("policy", "round_robin"),
        context_turns=args.context_turns or settings.get("context_turns", 40),
    )
    return table, settings


def cmd_talk(args: argparse.Namespace) -> int:
    table, _ = _build(args)

    print(f"\n{BOLD}Roundtable:{RESET} {table.topic}")
    _print_roster(table.participants)
    print(f"\n{DIM}{HELP}{RESET}")

    auto_remaining = args.auto or 0
    try:
        while True:
            if auto_remaining == 0:
                try:
                    line = input(f"{HOST_COLOR}> {RESET}").strip()
                except (EOFError, KeyboardInterrupt):
                    print()
                    break

                if line in ("/quit", "/q", "/exit"):
                    break
                if line == "/who":
                    _print_roster(table.participants)
                    continue
                if line == "/help":
                    print(HELP)
                    continue
                if line == "/save":
                    print(f"{DIM}wrote {table.export_markdown()}{RESET}")
                    continue
                if line.startswith("/next"):
                    _, _, who = line.partition(" ")
                    if not table.force_next(who.strip()):
                        print(f"{DIM}no seat called {who.strip()!r}{RESET}")
                        continue
                elif line.startswith("/auto"):
                    _, _, count = line.partition(" ")
                    auto_remaining = int(count) if count.strip().isdigit() else len(table.participants)
                    if auto_remaining <= 0:
                        continue
                    print(f"{DIM}(auto — Ctrl+C to take the wheel back){RESET}")
                elif line.startswith("/"):
                    print(f"{DIM}unknown command; /help for the list{RESET}")
                    continue
                elif line:
                    table.add_host_message(line)

            speaker = table.next_speaker()
            try:
                _render(table, table.run_turn(speaker), speaker.color)
            except KeyboardInterrupt:
                auto_remaining = 0
                print(f"\n{DIM}(paused){RESET}")
                continue

            if auto_remaining > 0:
                auto_remaining -= 1
            if auto_remaining != 0:
                try:
                    # A readable beat between turns, interruptible.
                    threading.Event().wait(args.pause)
                except KeyboardInterrupt:
                    auto_remaining = 0
                    print(f"\n{DIM}(paused){RESET}")
    finally:
        if table.history:
            md = table.export_markdown()
            print(f"\n{DIM}Transcript: {md}")
            print(f"Raw log:    {table.transcript_path}{RESET}")
    return 0


def cmd_web(args: argparse.Namespace) -> int:
    from .web import serve
    table, _ = _build(args)
    return serve(table, host=args.host, port=args.port, open_browser=not args.no_open)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="roundtable",
        description="Get every AI you have talking to each other.",
    )
    parser.add_argument("--version", action="version", version=__version__)
    sub = parser.add_subparsers(dest="command")

    def common(sp: argparse.ArgumentParser) -> None:
        sp.add_argument("topic", nargs="*", help="what they should discuss")
        sp.add_argument("-c", "--config", help="path to roundtable.toml")
        sp.add_argument("--only", help="comma-separated seats, e.g. Claude,Grok")
        sp.add_argument("--no-cli", action="store_true",
                        help="skip locally installed CLIs, APIs only")
        sp.add_argument("--policy", choices=["round_robin", "random"],
                        help="who speaks next (default round_robin)")
        sp.add_argument("--context-turns", type=int,
                        help="transcript turns each model sees (default 40)")
        sp.add_argument("--transcripts", help="directory for transcripts")

    talk = sub.add_parser("talk", help="terminal conversation (default)")
    common(talk)
    talk.add_argument("--auto", type=int, default=0,
                      help="start in auto mode for N turns")
    talk.add_argument("--pause", type=float, default=1.2,
                      help="seconds between turns in auto mode")
    talk.set_defaults(func=cmd_talk)

    web = sub.add_parser("web", help="browser conversation")
    common(web)
    web.add_argument("--host", default="127.0.0.1")
    web.add_argument("--port", type=int, default=8765)
    web.add_argument("--no-open", action="store_true",
                     help="don't open a browser window")
    web.set_defaults(func=cmd_web)

    doctor = sub.add_parser("doctor", help="list the models this machine can seat")
    doctor.set_defaults(func=cmd_doctor)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    argv = list(sys.argv[1:] if argv is None else argv)
    # `roundtable "some topic"` should just work, without typing `talk`.
    if argv and argv[0] not in {"talk", "web", "doctor", "-h", "--help", "--version"}:
        argv.insert(0, "talk")
    args = parser.parse_args(argv)
    if not getattr(args, "func", None):
        parser.print_help()
        return 0
    return args.func(args)
