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
    seq: int = -1        # position in the transcript; identity for the UI

    def as_event(self) -> dict:
        return {"type": "turn", **asdict(self)}


_ROLE_BRIEF: dict[str, list[str]] = {
    "principal": [
        "Reply to what was actually just said, and name whoever you are "
        "answering. Disagree when you disagree, and say why -- a roundtable "
        "where everyone agrees is a waste of everyone's tokens. Ask the "
        "others real questions. Concede a point when someone makes a good "
        "one.",
        "Keep it to a few sentences. This is talk, not an essay, and no "
        "stage directions about yourself.",
    ],
    "panel": [
        "You are on the panel, not leading it. Contribute exactly one "
        "concrete thing: a specific objection, a piece of evidence, a case "
        "the others have not considered. One or two sentences.",
        "Do not summarise what was said. Do not restate the topic. Do not "
        "open with praise for the previous speaker. If you have nothing "
        "specific to add, say only what you would need to know in order to "
        "have an opinion -- that is a useful answer and padding is not.",
    ],
    "moderator": [
        "You are moderating, not arguing, and you do not take a side.",
        "In at most three sentences: name the actual point of disagreement, "
        "say what now looks settled, and name the single thing that would "
        "resolve what is still open.",
    ],
}

def _system_prompt(me: Participant, others: list[str], topic: str) -> str:
    roster = ", ".join(others) if others else "no one else yet"
    lines = [
        f"You are {me.name}, one voice in a live roundtable with {roster}, "
        f"plus a human host who may interject at any point.",
        f"The topic on the table: {topic}",
        "",
        "The transcript is labelled by speaker. Reply as yourself, in first "
        "person, to what was actually just said. Do not prefix your reply "
        "with your own name.",
    ]
    lines += _ROLE_BRIEF[me.role if me.role in _ROLE_BRIEF else "principal"]
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
        policy: str = "auto",
        context_turns: int = 40,
        moderate_every: int = 0,
    ) -> None:
        if not participants:
            raise ValueError("a roundtable needs at least one participant")
        self.topic = topic
        self.participants = participants
        # Equal floor time is only fair when the seats are comparable. Once
        # some are panel seats, rotate by weight instead.
        if policy == "auto":
            policy = ("weighted"
                      if len({p.weight for p in participants}) > 1
                      else "round_robin")
        self.policy = policy
        self.context_turns = context_turns
        self.moderate_every = moderate_every
        self._since_moderation = 0
        self.history: list[Turn] = []
        self._index = 0
        self._forced: str | None = None
        self._pending: list[str] = []
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

    @property
    def moderators(self) -> list[Participant]:
        return [p for p in self.participants if p.role == "moderator"]

    @property
    def regulars(self) -> list[Participant]:
        """Everyone in the normal rotation. A moderator is not a debater."""
        return [p for p in self.participants if p.role != "moderator"] or self.participants

    def queue_round(self) -> list[str]:
        """Everyone speaks once, in rotation order, before anyone speaks twice.

        A round is a fairness guarantee, so it queues named seats rather than
        crediting N turns to the ordinary selector -- a weighted selector
        given eight credits produces eight weighted picks, not a round.
        """
        pool = self.regulars
        start = self._index % len(pool)
        self._pending = [p.name for p in pool[start:] + pool[:start]]
        return list(self._pending)

    def next_speaker(self) -> Participant:
        if self._forced and (p := self.by_name(self._forced)):
            self._forced = None
            # A host interruption jumps the queue but does not cancel it.
            return p
        while self._pending:
            if (p := self.by_name(self._pending.pop(0))) is not None:
                return p

        # A moderator earns the floor on a cadence, not every turn: asking a
        # model who should speak next before every single reply doubles the
        # calls to buy an ordering the transcript mostly implies anyway.
        if (self.moderate_every and self.moderators
                and self._since_moderation >= self.moderate_every):
            return self.moderators[0]

        pool = self.regulars
        if len(pool) > 1:
            others = [p for p in pool if p.name != self._last_speaker()] or pool
            if self.policy == "random":
                return random.choice(others)
            if self.policy == "weighted":
                weights = [max(p.weight, 0.01) for p in others]
                return random.choices(others, weights=weights, k=1)[0]
        return pool[self._index % len(pool)]

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
        turn = Turn(speaker=HOST, text=text, seq=len(self.history))
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

        yield {"type": "start", "speaker": p.name, "hex": p.hex,
               "seq": len(self.history)}
        parts: list[str] = []
        errored = False
        try:
            for chunk in stream(p, system, prompt):
                parts.append(chunk)
                yield {"type": "chunk", "speaker": p.name, "text": chunk}
        except ProviderError as exc:
            # Anything already streamed stays; only the failure is appended.
            errored = True
            note = f"\n[{p.name} unavailable: {exc}]"
            parts.append(note)
            yield {"type": "chunk", "speaker": p.name, "text": note}

        text = "".join(parts).strip()
        # A model that says nothing at all still has to occupy its turn,
        # or the rotation silently skips it forever.
        if not text:
            text = f"[{p.name} returned nothing]"
            errored = True
        turn = Turn(speaker=p.name, text=text, error=errored,
                    seq=len(self.history))
        # Resume round-robin from whoever actually spoke, so a forced turn or
        # an @mention reorders the table instead of double-seating someone.
        pool = self.regulars
        self._index = (pool.index(p) + 1) if p in pool else self._index
        if p.role == "moderator":
            self._since_moderation = 0
        else:
            self._since_moderation += 1
        self.history.append(turn)
        self._append(turn.as_event())
        yield {"type": "end", "speaker": p.name, "text": text,
               "error": errored, "seq": turn.seq, "hex": p.hex}
