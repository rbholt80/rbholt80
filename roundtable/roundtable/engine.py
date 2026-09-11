"""Conversation state and turn-taking.

The engine is a synchronous generator of events. Both front ends consume the
same generator, so the terminal and the browser can never drift apart in
behaviour -- the only difference is how an event gets painted.
"""

from __future__ import annotations

import json
import random
import re
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Iterator

from .config import Participant
from .providers import ProviderError, stream

HOST = "Host"


@dataclass
class Turn:
    speaker: str
    text: str
    ts: float = field(default_factory=time.time)
    error: bool = False

    def as_event(self) -> dict:
        return {"type": "turn", **asdict(self)}


def _system_prompt(me: Participant, others: list[str], topic: str) -> str:
    roster = ", ".join(others) if others else "no one else yet"
    lines = [
        f"You are {me.name}, one voice in a live roundtable with {roster}, "
        f"plus a human host who may interject at any point.",
        f"The topic on the table: {topic}",
        "",
        "The transcript is labelled by speaker. Reply as yourself, in first "
        "person, to what was actually just said. Name people when you are "
        "answering them. Disagree when you disagree, and say why -- a "
        "roundtable where everyone agrees is a waste of everyone's tokens. "
        "Ask the others real questions. Concede a point when someone makes a "
        "good one.",
        "Keep it to a few sentences. This is talk, not an essay. Do not "
        "prefix your reply with your own name, and do not narrate stage "
        "directions about yourself.",
    ]
    if me.persona:
        lines += ["", f"Your angle on this: {me.persona}"]
    return "\n".join(lines)


class Roundtable:
    """Holds the conversation and decides who speaks next."""

    def __init__(
        self,
        topic: str,
        participants: list[Participant],
        transcript_dir: str | Path = ".",
        policy: str = "round_robin",
        context_turns: int = 40,
    ) -> None:
        if not participants:
            raise ValueError("a roundtable needs at least one participant")
        self.topic = topic
        self.participants = participants
        self.policy = policy
        self.context_turns = context_turns
        self.history: list[Turn] = []
        self._index = 0
        self._forced: str | None = None
        self.started = time.time()

        stamp = time.strftime("%Y%m%d-%H%M%S", time.localtime(self.started))
        self.transcript_path = Path(transcript_dir) / f"roundtable-{stamp}.jsonl"
        self.transcript_path.parent.mkdir(parents=True, exist_ok=True)
        self._append({
            "type": "meta", "topic": topic, "started": self.started,
            "participants": [p.name for p in participants],
        })

    # --- persistence --------------------------------------------------------

    def _append(self, record: dict) -> None:
        """Write through on every turn: a crash should cost one reply, not all."""
        with self.transcript_path.open("a", encoding="utf-8") as fh:
            fh.write(json.dumps(record, ensure_ascii=False) + "\n")

    def export_markdown(self, path: str | Path | None = None) -> Path:
        target = Path(path) if path else self.transcript_path.with_suffix(".md")
        lines = [f"# Roundtable: {self.topic}", ""]
        lines.append(
            "*" + time.strftime("%Y-%m-%d %H:%M", time.localtime(self.started))
            + " — " + ", ".join(p.name for p in self.participants) + "*"
        )
        lines.append("")
        for turn in self.history:
            lines += [f"**{turn.speaker}:** {turn.text}", ""]
        target.write_text("\n".join(lines), encoding="utf-8")
        return target

    # --- turn taking --------------------------------------------------------

    @property
    def names(self) -> list[str]:
        return [p.name for p in self.participants]

    def by_name(self, name: str) -> Participant | None:
        for p in self.participants:
            if p.name.casefold() == name.casefold():
                return p
        return None

    def _mentioned(self, text: str) -> str | None:
        """An @mention hands the floor directly to that seat."""
        for token in re.findall(r"@([\w.-]+)", text):
            if (p := self.by_name(token)) is not None:
                return p.name
        return None

    def _last_speaker(self) -> str | None:
        return self.history[-1].speaker if self.history else None

    def next_speaker(self) -> Participant:
        if self._forced and (p := self.by_name(self._forced)):
            self._forced = None
            return p
        if self.policy == "random" and len(self.participants) > 1:
            pool = [p for p in self.participants if p.name != self._last_speaker()]
            return random.choice(pool)
        return self.participants[self._index % len(self.participants)]

    def force_next(self, name: str) -> bool:
        if self.by_name(name) is None:
            return False
        self._forced = name
        return True

    # --- context ------------------------------------------------------------

    def _context(self) -> str:
        """The transcript each model sees, trimmed to bound cost.

        Resending everything every turn makes cost grow quadratically with
        conversation length, so keep a window and tell the model what it
        missed rather than pretending the conversation started there.
        """
        turns = self.history
        preamble = ""
        if self.context_turns and len(turns) > self.context_turns:
            dropped = len(turns) - self.context_turns
            turns = turns[-self.context_turns:]
            preamble = (
                f"[{dropped} earlier turn(s) omitted for length; "
                "the conversation is already in progress]\n\n"
            )
        if not turns:
            return "(no one has spoken yet — you open the discussion)"
        return preamble + "\n\n".join(f"{t.speaker}: {t.text}" for t in turns)

    # --- driving ------------------------------------------------------------

    def add_host_message(self, text: str) -> Turn:
        turn = Turn(speaker=HOST, text=text)
        self.history.append(turn)
        self._append(turn.as_event())
        if (target := self._mentioned(text)):
            self.force_next(target)
        return turn

    def run_turn(self, speaker: Participant | None = None) -> Iterator[dict]:
        """Stream one reply. Yields start, chunk*, end."""
        p = speaker or self.next_speaker()
        others = [n for n in self.names if n != p.name]
        system = _system_prompt(p, others, self.topic)
        prompt = self._context() + f"\n\n{p.name}:"

        yield {"type": "start", "speaker": p.name, "hex": p.hex}
        parts: list[str] = []
        try:
            for chunk in stream(p, system, prompt):
                parts.append(chunk)
                yield {"type": "chunk", "speaker": p.name, "text": chunk}
        except ProviderError as exc:
            # Misconfiguration (missing SDK, vanished binary). Loud, but the
            # rest of the table keeps talking.
            note = f"[{p.name} can't be reached: {exc}]"
            parts.append(note)
            yield {"type": "chunk", "speaker": p.name, "text": note}

        text = "".join(parts).strip()
        # A model that says nothing at all still has to occupy its turn,
        # or round-robin silently skips it forever.
        errored = not text or (text.startswith("[") and text.endswith("]"))
        if not text:
            text = f"[{p.name} returned nothing]"
        turn = Turn(speaker=p.name, text=text, error=errored)
        # Resume round-robin from whoever actually spoke, so a forced turn or
        # an @mention reorders the table instead of double-seating someone.
        self._index = self.participants.index(p) + 1
        self.history.append(turn)
        self._append(turn.as_event())
        yield {"type": "end", "speaker": p.name, "text": text, "error": errored}
