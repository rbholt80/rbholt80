"""Streaming adapters. Each one turns a prompt into an iterator of text chunks.

Adapters never raise for a remote failure -- a model that is down should drop
out of the conversation, not end it. They raise only for programmer error
(a missing SDK, a malformed participant).
"""

from __future__ import annotations

import codecs
import subprocess
import threading
from typing import Iterator

from dataclasses import replace

from .config import Participant


class ProviderError(RuntimeError):
    """Setup problem the user has to fix (missing SDK, missing key)."""


# --- Anthropic --------------------------------------------------------------

def _stream_anthropic(p: Participant, system: str, prompt: str) -> Iterator[str]:
    try:
        import anthropic
    except ImportError as exc:  # pragma: no cover
        raise ProviderError("pip install anthropic") from exc

    client = anthropic.Anthropic(api_key=p.api_key)
    kwargs: dict = {
        "model": p.model,
        "max_tokens": p.max_tokens,
        "system": system,
        "messages": [{"role": "user", "content": prompt}],
    }
    if p.effort:
        kwargs["output_config"] = {"effort": p.effort}
    # `temperature` is deliberately not forwarded: current Claude models
    # removed sampling parameters and reject them with a 400. Use `effort`
    # to trade depth against speed instead.

    try:
        with client.messages.stream(**kwargs) as stream:
            yield from stream.text_stream
    except TypeError:
        # Older SDK that doesn't know output_config -- retry without it.
        kwargs.pop("output_config", None)
        with client.messages.stream(**kwargs) as stream:
            yield from stream.text_stream


# --- OpenAI-compatible (OpenAI, xAI, Groq, Ollama, LM Studio, ...) ----------

def _stream_openai(p: Participant, system: str, prompt: str) -> Iterator[str]:
    try:
        import openai
    except ImportError as exc:  # pragma: no cover
        raise ProviderError("pip install openai") from exc

    client = openai.OpenAI(
        # Local servers want a non-empty placeholder rather than None.
        api_key=p.api_key or "not-needed",
        base_url=p.base_url,
        timeout=p.timeout,
    )
    messages = [
        {"role": "system", "content": system},
        {"role": "user", "content": prompt},
    ]
    base: dict = {"model": p.model, "messages": messages, "stream": True}
    if p.temperature is not None:
        base["temperature"] = p.temperature

    # Reasoning models on OpenAI reject `max_tokens` and want
    # `max_completion_tokens`; xAI and most local servers want the reverse.
    # Try the newer name, fall back on the parameter error rather than
    # maintaining a list of which vendor is on which side of the rename.
    try:
        stream = client.chat.completions.create(
            **base, max_completion_tokens=p.max_tokens)
    except openai.BadRequestError as exc:
        if "max_completion_tokens" not in str(exc):
            raise
        stream = client.chat.completions.create(**base, max_tokens=p.max_tokens)

    for chunk in stream:
        if not chunk.choices:
            continue
        delta = chunk.choices[0].delta
        text = getattr(delta, "content", None)
        if text:
            yield text


# --- Gemini -----------------------------------------------------------------

def _stream_gemini(p: Participant, system: str, prompt: str) -> Iterator[str]:
    try:
        from google import genai
        from google.genai import types
    except ImportError as exc:  # pragma: no cover
        raise ProviderError("pip install google-genai") from exc

    client = genai.Client(api_key=p.api_key)
    config = types.GenerateContentConfig(
        system_instruction=system,
        max_output_tokens=p.max_tokens,
        **({"temperature": p.temperature} if p.temperature is not None else {}),
    )
    for chunk in client.models.generate_content_stream(
        model=p.model, contents=prompt, config=config
    ):
        if chunk.text:
            yield chunk.text


# --- installed CLIs ---------------------------------------------------------

def _stream_cli(p: Participant, system: str, prompt: str) -> Iterator[str]:
    """Drive an installed CLI as a conversational participant.

    The whole prompt goes in on stdin: argv has a length ceiling a long
    transcript will hit, and quoting a transcript into a shell argument is a
    bug farm. Output is decoded incrementally so partial UTF-8 at a chunk
    boundary doesn't corrupt the stream.
    """
    argv = list(p.argv)
    if p.model == "claude":
        argv += ["--append-system-prompt", system]
        payload = prompt
    else:
        # Other CLIs have no system-prompt flag; fold it into the text.
        payload = f"{system}\n\n---\n\n{prompt}"

    try:
        proc = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as exc:
        raise ProviderError(f"could not run {argv[0]}: {exc}") from exc

    # Write on a thread: a large prompt can fill the pipe buffer and deadlock
    # against our own read loop.
    def _feed() -> None:
        try:
            assert proc.stdin is not None
            proc.stdin.write(payload.encode())
            proc.stdin.close()
        except (BrokenPipeError, OSError):
            pass

    writer = threading.Thread(target=_feed, daemon=True)
    writer.start()

    timed_out = threading.Event()

    def _kill() -> None:
        timed_out.set()
        proc.kill()

    killer = threading.Timer(p.timeout, _kill)
    killer.start()
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    try:
        assert proc.stdout is not None
        while True:
            block = proc.stdout.read(64)
            if not block:
                break
            text = decoder.decode(block)
            if text:
                yield text
        yield decoder.decode(b"", final=True)
    finally:
        killer.cancel()
        writer.join(timeout=1.0)
        proc.wait()

    if timed_out.is_set():
        yield f"\n[{p.name} timed out after {p.timeout:.0f}s]"
    elif proc.returncode not in (0, None):
        err = (proc.stderr.read().decode(errors="replace").strip()
               if proc.stderr else "")
        yield f"\n[{p.name} exited {proc.returncode}: {err[:300]}]"


# --- mock -------------------------------------------------------------------

def _stream_mock(p: Participant, system: str, prompt: str) -> Iterator[str]:
    """A seat that costs nothing, for checking the harness end to end.

    Useful before you point this at paid APIs: it exercises streaming, turn
    taking, the transcript and both front ends without spending a token.
    """
    import itertools
    import time as _time

    last = prompt.rstrip().rsplit("\n\n", 1)[-1][:120]
    reply = (
        f"({p.name}, standing in) I hear \u201c{last}\u201d. "
        f"My position on \u201c{p.persona or 'the topic'}\u201d is unchanged, "
        "and I would push back on the last point before we move on."
    )
    for word in itertools.chain.from_iterable((w, " ") for w in reply.split(" ")):
        _time.sleep(0.01)
        yield word


_ADAPTERS = {
    "anthropic": _stream_anthropic,
    "openai": _stream_openai,
    "gemini": _stream_gemini,
    "cli": _stream_cli,
    "mock": _stream_mock,
}


def stream(p: Participant, system: str, prompt: str) -> Iterator[str]:
    """Stream one reply. Remote failures arrive as an in-band `[...]` note."""
    adapter = _ADAPTERS.get(p.kind)
    if adapter is None:
        raise ProviderError(f"{p.name}: unknown kind {p.kind!r}")
    try:
        yield from adapter(p, system, prompt)
    except ProviderError:
        raise
    except Exception as exc:  # noqa: BLE001 - one seat failing must not end the table
        yield f"[{p.name} unavailable: {type(exc).__name__}: {exc}]"


def probe(p: Participant, timeout: float = 60.0) -> tuple[bool, str]:
    """Actually invoke a seat once, cheaply, and see whether it answers.

    Discovery can only see that a binary exists or a key is exported --
    neither of which means the seat will talk. A signed-out CLI looks
    identical to a signed-in one until you ask it something.
    """
    trial = replace(p, timeout=timeout, max_tokens=32, effort=None)
    try:
        text = "".join(stream(trial, "Reply with one word: ok",
                              "Say ok and nothing else.")).strip()
    except ProviderError as exc:
        return False, str(exc)
    if not text:
        return False, "no output"
    if text.startswith("[") and text.endswith("]"):
        return False, text.strip("[]")
    return True, text.replace("\n", " ")[:60]
