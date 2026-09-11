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
    crossed: int = 0     # messages that landed while this reply was being written
    metrics: dict = field(default_factory=dict)

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

#: A line that claims to be somebody's turn: "Host:", "[#7] Gemma3:".
_ATTRIBUTION = re.compile(r"^\s*(?:\[#\d+\]\s*)?([\w.\-]{1,24})\s*:\s?")


def strip_fabrications(text: str, speaker: str, roster: list[str]) -> str:
    """Remove turns a model wrote on other participants' behalf.

    Small models imitate the transcript they are shown and start producing
    other people's lines -- observed live: a 1B model invented two Host
    messages and answered them. Those inventions are appended verbatim, every
    later model reads them as things that were said, and the conversation
    proceeds from words the host never wrote.

    The model's own name at the start is just a label it was told not to add,
    so that is trimmed. Another participant's name is fabrication, and
    everything from there on is discarded rather than patched: once a model
    has started writing the transcript instead of its turn, the rest of the
    reply is about a conversation that did not happen.
    """
    known = {n.casefold() for n in roster} | {HOST.casefold()}
    kept: list[str] = []
    for i, line in enumerate(text.splitlines()):
        match = _ATTRIBUTION.match(line)
        name = match.group(1).casefold() if match else None
        if name in known:
            if name == speaker.casefold() and i == 0:
                kept.append(line[match.end():])
                continue
            break
        kept.append(line)
    return "\n".join(kept).strip()


def _system_prompt(me: Participant, others: list[str], topic: str) -> str:
    roster = ", ".join(others) if others else "no one else yet"
    lines = [
        f"You are {me.name}, one voice in a live roundtable with {roster}, "
        f"plus a human host who may interject at any point.",
        f"The topic on the table: {topic}",
        "",
        "The transcript is labelled by speaker. Reply as yourself, in first "
        "person, to what was actually just said. Write only your own words: "
        "do not prefix your reply with your own name, and never write a line "
        "in anyone else's voice — no \"Host:\", no \"[#4] Someone:\", no "
        "invented quotes. Quoting a real line back is fine; composing a turn "
        "for another participant is not.",
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
            note = ""
            if turn.crossed:
                what = ("the message" if turn.crossed == 1
                        else f"the {turn.crossed} messages")
                note = f" *(written before {what} above it)*"
            lines += [f"**{turn.speaker}:**{note} {turn.text}", ""]
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

    def ledger(self) -> dict:
        """What this conversation has cost so far, per seat and in total.

        Tokens are the honest unit: only some providers report them, and only
        you know what you are paying per million. Set price_in / price_out on
        a seat and its rows gain a dollar figure; leave them unset and the
        row is tokens and seconds, which is still enough to see which seat is
        expensive.
        """
        rows: dict[str, dict] = {}
        for turn in self.history:
            if not turn.metrics:
                continue
            seat = self.by_name(turn.speaker)
            row = rows.setdefault(turn.speaker, {
                "turns": 0, "prompt_tokens": 0, "output_tokens": 0,
                "seconds": 0.0, "dollars": 0.0, "estimated": False,
            })
            row["turns"] += 1
            prompt_tokens = turn.metrics.get("prompt_tokens", 0)
            output_tokens = turn.metrics.get("output_tokens", 0)
            row["prompt_tokens"] += prompt_tokens
            row["output_tokens"] += output_tokens
            row["seconds"] += turn.metrics.get("seconds", 0.0)
            row["estimated"] |= bool(turn.metrics.get("estimated"))
            if seat and seat.price_in is not None:
                row["dollars"] += prompt_tokens / 1e6 * seat.price_in
            if seat and seat.price_out is not None:
                row["dollars"] += output_tokens / 1e6 * seat.price_out

        total = {"turns": 0, "prompt_tokens": 0, "output_tokens": 0,
                 "seconds": 0.0, "dollars": 0.0, "estimated": False}
        for row in rows.values():
            for key in ("turns", "prompt_tokens", "output_tokens",
                        "seconds", "dollars"):
                total[key] += row[key]
            total["estimated"] |= row["estimated"]
        return {"seats": rows, "total": total}

    # --- context ------------------------------------------------------------

    @staticmethod
    def _estimate_tokens(text: str) -> int:
        """Four characters per token: the usual rule of thumb.

        Deliberately not a real tokenizer. Every seat is a different model
        with a different vocabulary, and the only exact answer costs a
        network call per turn to measure something we are about to spend
        anyway. An estimate that is generous by a few percent trims one turn
        too many; an exact count that arrives 300ms late costs more.
        """
        return max(1, len(text) // 4)

    def _context(self, speaker: Participant | None = None) -> str:
        """The transcript one seat sees, trimmed to what it can actually hold.

        A turn count is the wrong unit. The same forty turns are nothing to a
        model with a million tokens of context and an overrun for a local one
        with eight thousand -- and an overrun does not announce itself, it
        just quietly drops the start of the conversation, which is where the
        question was asked. Seats that declare a context budget get as much
        transcript as fits inside it; the turn window still applies to
        everyone as a cost ceiling.
        """
        turns = self.history
        dropped = 0

        if self.context_turns and len(turns) > self.context_turns:
            dropped = len(turns) - self.context_turns
            turns = turns[-self.context_turns:]

        budget = getattr(speaker, "context_tokens", None) if speaker else None
        if budget:
            # Leave room for the system prompt and the reply itself, or the
            # request fits and the generation does not.
            spare = budget - (speaker.max_tokens if speaker else 0) - 512
            kept: list[Turn] = []
            used = 0
            for turn in reversed(turns):
                cost = self._estimate_tokens(turn.text) + 8  # speaker label
                if used + cost > spare and kept:
                    break
                used += cost
                kept.append(turn)
            dropped += len(turns) - len(kept)
            turns = list(reversed(kept))

        if not turns:
            return "(no one has spoken yet — you open the discussion)"
        preamble = ""
        if dropped:
            preamble = (
                f"[{dropped} earlier turn(s) omitted for length; "
                "the conversation is already in progress]\n\n"
            )
        return preamble + "\n\n".join(
            f"{self._label(t)}: {t.text}" for t in turns)

    @staticmethod
    def _label(turn: Turn) -> str:
        """Speaker name, saying so when the reply predates what sits above it.

        Without this the next model reads a reply positioned under a host
        message as an answer to it, and treats the speaker as having ignored
        the question. It did not ignore anything -- it was already talking.
        """
        if not turn.crossed:
            return turn.speaker
        n = turn.crossed
        what = "the message" if n == 1 else f"the {n} messages"
        return f"{turn.speaker} (was already writing; had not seen {what} above)"

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
        prompt = self._context(p) + f"\n\n{p.name}:"
        # Everything visible at the moment the prompt was built. A reply takes
        # seconds to stream, and the host can type during those seconds.
        saw = len(self.history)

        yield {"type": "start", "speaker": p.name, "hex": p.hex,
               "seq": len(self.history)}
        parts: list[str] = []
        errored = False
        metrics: dict = {}
        try:
            for chunk in stream(p, system, prompt, metrics):
                parts.append(chunk)
                yield {"type": "chunk", "speaker": p.name, "text": chunk}
        except ProviderError as exc:
            # Anything already streamed stays; only the failure is appended.
            errored = True
            note = f"\n[{p.name} unavailable: {exc}]"
            parts.append(note)
            yield {"type": "chunk", "speaker": p.name, "text": note}

        text = strip_fabrications("".join(parts), p.name, self.names)
        # A model that says nothing at all still has to occupy its turn,
        # or the rotation silently skips it forever.
        if not text:
            text = f"[{p.name} returned nothing]"
            errored = True
        turn = Turn(speaker=p.name, text=text, error=errored,
                    seq=len(self.history), metrics=metrics,
                    crossed=max(0, len(self.history) - saw))
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
               "error": errored, "seq": turn.seq, "hex": p.hex,
               "crossed": turn.crossed,
               "metrics": metrics, "ledger": self.ledger()}
