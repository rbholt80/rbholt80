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
  /round           every seat speaks once, in order
  /auto [n]        models keep talking (n turns, or until Ctrl+C)
  /next <Name>     put a specific model up next
  /who             who is at the table
  /cost            tokens and seconds spent so far, per seat
  /moderate        ask the moderator seat to sum up where things stand
  /save            write a markdown transcript now
  /quit            exit
"""


def _print_roster(seats: list[Participant]) -> None:
    for p in seats:
        detail = p.model or p.kind
        if p.kind == "cli":
            detail += " (CLI)"
        if p.role != "principal":
            detail += f" · {p.role}"
        note = f"  {DIM}{p.note}{RESET}" if p.note else ""
        print(f"  {p.color}●{RESET} {p.name:<14} {DIM}{detail}{RESET}{note}")


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


def _print_ledger(table) -> None:
    ledger = table.ledger()
    if not ledger["seats"]:
        print(f"{DIM}nothing spent yet{RESET}")
        return
    total = ledger["total"]
    money = any(row["dollars"] for row in ledger["seats"].values())
    head = f"  {'seat':<14}{'turns':>6}{'in':>10}{'out':>9}{'sec':>8}"
    print(BOLD + head + (f"{'cost':>10}" if money else "") + RESET)
    for name, row in sorted(ledger["seats"].items(),
                            key=lambda kv: -kv[1]["output_tokens"]):
        line = (f"  {name:<14}{row['turns']:>6}{row['prompt_tokens']:>10,}"
                f"{row['output_tokens']:>9,}{row['seconds']:>8.1f}")
        if money:
            line += f"{'$' + format(row['dollars'], '.4f'):>10}"
        print(line)
    line = (f"  {'total':<14}{total['turns']:>6}{total['prompt_tokens']:>10,}"
            f"{total['output_tokens']:>9,}{total['seconds']:>8.1f}")
    if money:
        line += f"{'$' + format(total['dollars'], '.4f'):>10}"
    print(DIM + line + RESET)
    if total["estimated"]:
        print(f"{DIM}  output tokens estimated where the provider reported none"
              f"{RESET}")


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

    if not getattr(args, "probe", False):
        print(f"{len(seats)} seat(s) found. `doctor --probe` checks they answer.")
        return 0

    from .providers import probe
    print(f"{BOLD}Live check{RESET}")
    working = 0
    for seat in seats:
        print(f"  {seat.color}●{RESET} {seat.name:<12} ", end="", flush=True)
        ok, detail = probe(seat)
        working += ok
        print(("✓ " if ok else "✗ ") + f"{DIM}{detail}{RESET}")
    print(f"\n{working}/{len(seats)} seat(s) answered.")
    return 0 if working else 1


def _build(args: argparse.Namespace) -> tuple[Roundtable, dict]:
    seats, settings = config.resolve(
        config_path=args.config,
        only=args.only.split(",") if args.only else None,
        include_cli=not args.no_cli,
        local_context=args.local_context,
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
        policy=args.policy or settings.get("policy", "auto"),
        context_turns=args.context_turns or settings.get("context_turns", 40),
        moderate_every=(args.moderate_every
                        if args.moderate_every is not None
                        else settings.get("moderate_every", 0)),
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
            if auto_remaining <= 0:
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
                if line == "/cost":
                    _print_ledger(table)
                    continue
                if line == "/moderate":
                    mods = table.moderators
                    if not mods:
                        print(f"{DIM}no moderator seat; set role = \"moderator\" "
                              f"on one in roundtable.toml{RESET}")
                        continue
                    table.force_next(mods[0].name)
                elif line == "/save":
                    print(f"{DIM}wrote {table.export_markdown()}{RESET}")
                    continue
                if line.startswith("/next"):
                    _, _, who = line.partition(" ")
                    if not table.force_next(who.strip()):
                        print(f"{DIM}no seat called {who.strip()!r}{RESET}")
                        continue
                elif line == "/round":
                    auto_remaining = len(table.queue_round())
                    print(f"{DIM}(one turn each — Ctrl+C to stop){RESET}")
                elif line.startswith("/auto"):
                    _, _, count = line.partition(" ")
                    auto_remaining = int(count) if count.strip().isdigit() else -1
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
            print()
            _print_ledger(table)
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
        sp.add_argument("--policy",
                        choices=["auto", "round_robin", "random", "weighted"],
                        help="who speaks next (default auto: weighted when "
                             "seats have different weights)")
        sp.add_argument("--moderate-every", type=int, default=None,
                        metavar="N",
                        help="let a moderator seat sum up every N turns")
        sp.add_argument("--context-turns", type=int,
                        help="transcript turns each model sees (default 40)")
        sp.add_argument("--transcripts", help="directory for transcripts")
        sp.add_argument("--local-context", type=int, default=None,
                        metavar="N",
                        help="context tokens per local model (default "
                             f"{config.LOCAL_CONTEXT_TOKENS}); memory cost "
                             "multiplies by the number of seats")

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
    doctor.add_argument("--probe", action="store_true",
                        help="actually call each seat once to confirm it answers")
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
