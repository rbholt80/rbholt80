"""Drive a `warrant` guard from Python.

Standard library only, no build step: it starts `warrant serve` and talks
line-delimited JSON to it over a pipe. That is deliberately the dullest
possible integration — a subprocess and two pipes work the same from Python,
Kotlin, Node or a shell script, and nothing has to be compiled against
anything.

    from warrant import Warrant, Refused

    w = Warrant(dir="~/.local/share/myapp")

    # Just ask. Changes nothing, records nothing.
    r = w.rule("agent:claude", "fs.delete:/home/robert/old.txt")
    print(r.decision, r.reason, r.risk)

    # Do it, with the reversal written down first. The journal records how it
    # ended whether the body returns or raises.
    with w.act("agent:claude", "fs.write:/home/robert/notes.md",
               intent="save the draft",
               undo=("restore notes.md", {"backup": "/var/b/7"})) as a:
        Path("/home/robert/notes.md").write_text(text)
        a.detail = "wrote %d bytes" % len(text)

A refused action raises `Refused` out of `act()` before the body runs. An
action that policy says needs a person raises `NeedsConfirmation`, so that
"nobody asked" can never be mistaken for "somebody said yes" — pass
`confirmed_by="robert"` once they have.
"""

from __future__ import annotations

import json
import os
import subprocess
from contextlib import contextmanager
from dataclasses import dataclass, field
from typing import Any, Dict, Iterator, List, Optional, Tuple

__all__ = [
    "Warrant",
    "Ruling",
    "Action",
    "WarrantError",
    "Refused",
    "NeedsConfirmation",
]


class WarrantError(RuntimeError):
    """The guard could not answer, or the request was malformed."""


class Refused(WarrantError):
    """Policy said no. `ruling` carries the reason and the line that decided."""

    def __init__(self, ruling: "Ruling"):
        super().__init__(ruling.explain)
        self.ruling = ruling


class NeedsConfirmation(Refused):
    """Policy said a person has to say yes first."""


@dataclass(frozen=True)
class Ruling:
    decision: str  # allow | confirm | deny
    reason: str
    risk: str  # read | write | elevated | critical
    matched: Optional[str]  # "policy.warrant:12", or None for the default deny
    absolute: bool  # decided by a `never` line; no confirmation can lift it
    explain: str
    prompt: str  # the sentence to show a person when asking

    @property
    def allowed(self) -> bool:
        return self.decision == "allow"

    @property
    def needs_confirmation(self) -> bool:
        return self.decision == "confirm"

    @classmethod
    def _from(cls, d: Dict[str, Any]) -> "Ruling":
        return cls(
            decision=d.get("decision", "deny"),
            reason=d.get("reason", ""),
            risk=d.get("risk", "critical"),
            matched=d.get("matched"),
            absolute=bool(d.get("absolute", False)),
            explain=d.get("explain", ""),
            prompt=d.get("prompt", ""),
        )


@dataclass
class Action:
    """An authorised, journalled action in progress.

    Set `detail` in the body of the `with` block to say what actually happened
    — it is written to the journal alongside the outcome.
    """

    seq: int
    risk: str
    detail: str = ""
    _finished: bool = field(default=False, repr=False)


class Warrant:
    """A guard, running as a child process."""

    def __init__(
        self,
        dir: Optional[str] = None,
        binary: str = "warrant",
        policy: Optional[str] = None,
        grades: Optional[str] = None,
        home: Optional[str] = None,
    ):
        argv: List[str] = [binary]
        if dir:
            argv += ["--dir", os.path.expanduser(dir)]
        if policy:
            argv += ["--policy", os.path.expanduser(policy)]
        if grades:
            argv += ["--grades", os.path.expanduser(grades)]
        if home:
            argv += ["--home", home]
        argv.append("serve")

        try:
            self._p = subprocess.Popen(
                argv,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                bufsize=1,
            )
        except FileNotFoundError as e:
            raise WarrantError(
                "could not start %r — is warrant on PATH? (cargo install --path .)" % binary
            ) from e

        # Fail here rather than at the first real decision. A guard that turns
        # out to be missing its policy file halfway through a run is worse than
        # one that never started.
        self._call({"op": "ping"})

    # -- lifecycle ---------------------------------------------------------

    def close(self) -> None:
        if self._p.poll() is None:
            try:
                self._p.stdin.close()
                self._p.wait(timeout=5)
            except Exception:
                self._p.kill()
                self._p.wait()
        # Closing stdin ends the guard, but its stdout and stderr pipes are ours
        # to release — a long-lived host that opens a guard per request would
        # otherwise run out of file descriptors.
        for pipe in (self._p.stdin, self._p.stdout, self._p.stderr):
            try:
                if pipe is not None:
                    pipe.close()
            except Exception:
                pass

    def __enter__(self) -> "Warrant":
        return self

    def __exit__(self, *exc: Any) -> None:
        self.close()

    def _call(self, req: Dict[str, Any]) -> Dict[str, Any]:
        if self._p.poll() is not None:
            raise WarrantError(
                "the guard exited (%s): %s"
                % (self._p.returncode, (self._p.stderr.read() or "").strip())
            )
        self._p.stdin.write(json.dumps(req) + "\n")
        self._p.stdin.flush()
        line = self._p.stdout.readline()
        if not line:
            raise WarrantError(
                "the guard stopped answering: %s" % (self._p.stderr.read() or "").strip()
            )
        reply = json.loads(line)
        if not reply.get("ok"):
            raise WarrantError(reply.get("error", "unknown error"))
        return reply

    # -- asking ------------------------------------------------------------

    def rule(self, subject: str, cap: str) -> Ruling:
        """Adjudicate one request. Changes nothing, records nothing."""
        return Ruling._from(self._call({"op": "rule", "subject": subject, "cap": cap}))

    def preflight(self, subject: str, caps: List[str]) -> List[Ruling]:
        """Adjudicate a whole plan before running any of it."""
        return [self.rule(subject, c) for c in caps]

    # -- doing -------------------------------------------------------------

    @contextmanager
    def act(
        self,
        subject: str,
        cap: str,
        intent: str = "",
        undo: Optional[Tuple[str, Dict[str, Any]]] = None,
        confirmed_by: Optional[str] = None,
    ) -> Iterator[Action]:
        """Authorise, journal the reversal, run the body, record the outcome.

        `undo` is `(note, data)`: the note is for a person, the data is
        whatever *you* need to reverse it — warrant stores it and hands it
        back, it never acts on it.

        Raises `Refused` or `NeedsConfirmation` before the body runs. If the
        body raises, the journal says `failed` with the exception text and the
        exception propagates.
        """
        r = self.rule(subject, cap)
        if not r.allowed and confirmed_by is None:
            if r.needs_confirmation:
                raise NeedsConfirmation(r)
            raise Refused(r)

        req: Dict[str, Any] = {
            "op": "begin",
            "subject": subject,
            "cap": cap,
            "intent": intent,
        }
        if undo is not None:
            req["undo"] = {"note": undo[0], "data": undo[1]}
        if confirmed_by is not None:
            req["confirmed_by"] = confirmed_by

        started = self._call(req)
        action = Action(seq=int(started["seq"]), risk=started.get("risk", ""))
        try:
            yield action
        except BaseException as e:
            # Recorded as failed, not refused: it was authorised and it ran, so
            # it may well have half-happened. That distinction is the reason
            # the undo was written before the body.
            self._end(action, "failed", action.detail or "%s: %s" % (type(e).__name__, e))
            raise
        else:
            self._end(action, "ok", action.detail)

    def _end(self, action: Action, outcome: str, detail: str) -> None:
        if action._finished:
            return
        action._finished = True
        self._call(
            {"op": "end", "seq": action.seq, "outcome": outcome, "detail": detail}
        )

    def refuse(self, subject: str, cap: str, intent: str = "") -> int:
        """Record that a request was turned down, so the refusal is in history."""
        return int(self._call(
            {"op": "refuse", "subject": subject, "cap": cap, "intent": intent}
        )["seq"])

    # -- looking back ------------------------------------------------------

    def history(self, limit: int = 20) -> List[Dict[str, Any]]:
        return self._call({"op": "history", "limit": limit})["records"]

    def unfinished(self) -> List[Dict[str, Any]]:
        """Actions begun and never reported finished — what a crash left open."""
        return self._call({"op": "unfinished"})["records"]

    def undoable(self) -> Optional[Dict[str, Any]]:
        """The newest action that can still be taken back."""
        return self._call({"op": "undoable"})["record"]

    def take_undo(self, seq: int) -> Tuple[str, Any]:
        """Claim the reversal for `seq`. Returns `(note, data)`.

        You perform it. Call `reverted()` only once it actually worked — a
        failed undo must not leave the journal claiming the action was taken
        back.
        """
        r = self._call({"op": "take_undo", "seq": seq})
        return r["note"], r["data"]

    def reverted(self, seq: int, by: int = 0) -> None:
        self._call({"op": "reverted", "seq": seq, "by": by})

    def grades(self) -> List[Dict[str, str]]:
        """Every capability this host graded, and what it costs."""
        return self._call({"op": "grades"})["grades"]

    def absolutes(self) -> List[Dict[str, str]]:
        """The `never` lines — what this system will not do under any argument."""
        return self._call({"op": "absolutes"})["absolutes"]
