"""Streaming adapters. Each one turns a prompt into an iterator of text chunks.

Failures raise ProviderError, preserving any chunks already yielded. The
engine records a visible error and continues with the next participant.
"""

from __future__ import annotations

import time
from typing import Iterator

from dataclasses import replace

from .config import Participant
from .local import stream_cli, stream_ollama


class ProviderError(RuntimeError):
    """A setup, transport, or streaming failure from one participant."""


# --- Anthropic --------------------------------------------------------------

def _stream_anthropic(p: Participant, system: str, prompt: str,
                      metrics: dict | None = None) -> Iterator[str]:
    try:
        import anthropic
    except ImportError as exc:  # pragma: no cover
        raise ProviderError("pip install anthropic") from exc

    client = anthropic.Anthropic(api_key=p.api_key, timeout=p.timeout)
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

    emitted = False

    def _run(call_kwargs: dict) -> Iterator[str]:
        nonlocal emitted
        with client.messages.stream(**call_kwargs) as stream:
            for text in stream.text_stream:
                emitted = True
                yield text
            usage = getattr(stream.get_final_message(), "usage", None)
            if usage is not None and metrics is not None:
                metrics["prompt_tokens"] = getattr(usage, "input_tokens", 0)
                metrics["output_tokens"] = getattr(usage, "output_tokens", 0)

    try:
        yield from _run(kwargs)
    except TypeError as exc:
        # Only negotiate an older SDK's rejected keyword before any text was
        # emitted. Replaying a failed partial reply duplicates text and calls.
        if emitted or "output_config" not in kwargs or "output_config" not in str(exc):
            raise
        kwargs.pop("output_config")
        yield from _run(kwargs)
    finally:
        client.close()


# --- OpenAI-compatible (OpenAI, xAI, Groq, Ollama, LM Studio, ...) ----------

#: Parameter names only: a probe's token budget must not leak into later turns.
_DIALECT: dict[tuple[str, str], tuple[str, bool]] = {}


def _stream_openai(p: Participant, system: str, prompt: str,
                   metrics: dict | None = None) -> Iterator[str]:
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

    # Vendors disagree about two parameters. Reasoning models on OpenAI reject
    # `max_tokens` and want `max_completion_tokens`; xAI and most local servers
    # want the reverse. `stream_options` (the only way to get token counts out
    # of a stream) is missing from several OpenAI-compatible servers entirely.
    # Rather than track which vendor is on which side, try the richest variant
    # and step down -- then remember the winner, so the cost is paid once per
    # endpoint instead of once per turn.
    variants = [
        ("max_completion_tokens", True),
        ("max_tokens", True),
        ("max_completion_tokens", False),
        ("max_tokens", False),
    ]
    key = (p.base_url or "openai", p.model)
    if (known := _DIALECT.get(key)) is not None:
        variants = [known] + [v for v in variants if v != known]

    response = None
    last: Exception | None = None
    try:
        for variant in variants:
            token_field, include_usage = variant
            kwargs = {token_field: p.max_tokens}
            if include_usage:
                kwargs["stream_options"] = {"include_usage": True}
            try:
                response = client.chat.completions.create(**base, **kwargs)
                _DIALECT[key] = variant
                break
            except openai.BadRequestError as exc:
                # An invalid model/prompt is not a parameter dialect mismatch.
                # Do not repeat those requests under all four spellings.
                if not any(name in str(exc) for name in
                           ("max_completion_tokens", "max_tokens", "stream_options")):
                    raise
                last = exc
        if response is None:
            raise last or ProviderError("no accepted parameter combination")

        for chunk in response:
            usage = getattr(chunk, "usage", None)
            if usage is not None and metrics is not None:
                metrics["prompt_tokens"] = getattr(usage, "prompt_tokens", 0) or 0
                metrics["output_tokens"] = getattr(usage, "completion_tokens", 0) or 0
            if not chunk.choices:
                continue
            delta = chunk.choices[0].delta
            text = getattr(delta, "content", None)
            if text:
                yield text
    finally:
        if response is not None:
            response.close()
        client.close()


# --- Gemini -----------------------------------------------------------------

def _stream_gemini(p: Participant, system: str, prompt: str,
                   metrics: dict | None = None) -> Iterator[str]:
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
        meta = getattr(chunk, "usage_metadata", None)
        if meta is not None and metrics is not None:
            metrics["prompt_tokens"] = getattr(meta, "prompt_token_count", 0) or 0
            metrics["output_tokens"] = getattr(meta, "candidates_token_count", 0) or 0
        if chunk.text:
            yield chunk.text


# --- mock -------------------------------------------------------------------

def _stream_mock(p: Participant, system: str, prompt: str,
                 metrics: dict | None = None) -> Iterator[str]:
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
    "cli": stream_cli,
    "ollama": stream_ollama,
    "mock": _stream_mock,
}


def stream(p: Participant, system: str, prompt: str,
           metrics: dict | None = None) -> Iterator[str]:
    """Stream one reply.

    Any failure is raised as ProviderError, including one that arrives
    mid-stream. Whatever was yielded before the failure has already reached
    the caller and is kept: a reply that got three sentences out before the
    connection died is still worth three sentences. The engine turns the
    exception into a visible note and marks the turn errored -- which is why
    failure is raised rather than returned as bracketed text that the engine
    would then have to recognise by its punctuation.
    """
    adapter = _ADAPTERS.get(p.kind)
    if adapter is None:
        raise ProviderError(f"unknown participant kind {p.kind!r}")
    started = time.monotonic()
    characters = 0
    try:
        for chunk in adapter(p, system, prompt, metrics):
            characters += len(chunk)
            yield chunk
    except ProviderError:
        raise
    except Exception as exc:  # noqa: BLE001 - one seat failing must not end the table
        raise ProviderError(f"{type(exc).__name__}: {exc}") from exc
    finally:
        if metrics is not None:
            metrics["seconds"] = time.monotonic() - started
            metrics["characters"] = characters
            # Local servers and CLIs often report nothing. Four characters per
            # token is the usual rule of thumb; it is labelled estimated so a
            # cost total never quietly mixes measured and guessed numbers.
            if "output_tokens" not in metrics and characters:
                metrics["output_tokens"] = max(1, characters // 4)
                metrics["estimated"] = True


def probe(p: Participant, timeout: float = 60.0) -> tuple[bool, str]:
    """Actually invoke a seat once, cheaply, and see whether it answers.

    Discovery checks supported CLI login status, but a real reply also tests
    transport, model access, and invocation compatibility.
    """
    trial = replace(p, timeout=timeout, max_tokens=32, effort=None)
    try:
        text = "".join(stream(trial, "Reply with one word: ok",
                              "Say ok and nothing else.")).strip()
    except ProviderError as exc:
        return False, str(exc)
    if not text:
        return False, "no output"
    return True, text.replace("\n", " ")[:60]
