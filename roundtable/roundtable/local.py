"""Subscription CLI and local Ollama connections. No hosted API key needed."""
from __future__ import annotations

import codecs
import json
import os
import signal
import subprocess
import tempfile
import threading
import urllib.request
from typing import TYPE_CHECKING, Iterator

if TYPE_CHECKING:
    from .config import Participant


def cli_ready(executable: str, path: str) -> tuple[bool, str]:
    """Inspect login status without reading or returning credentials."""
    commands = {"claude": [path, "auth", "status"], "codex": [path, "login", "status"]}
    if executable not in commands:
        return True, "Installed; login will be checked on its first turn."
    try:
        result = subprocess.run(commands[executable], capture_output=True, timeout=10)
        if executable == "claude":
            ready = bool(json.loads(result.stdout).get("loggedIn"))
        else:
            ready = result.returncode == 0
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return False, f"Could not check login; run {executable} interactively."
    return ready, ("Signed in" if ready else f"Sign in with: {executable} auth login" if executable == "claude" else "Sign in with: codex login")


def ollama_models(base_url: str) -> list[str]:
    """Only completion-capable models get seats; embeddings cannot converse."""
    with urllib.request.urlopen(base_url + "/api/tags", timeout=3) as response:
        entries = json.load(response).get("models", [])
    models = []
    for entry in entries:
        name = entry.get("name")
        if not name:
            continue
        request = urllib.request.Request(base_url + "/api/show",
            data=json.dumps({"model": name}).encode(),
            headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=3) as response:
                details = json.load(response)
            if "completion" in details.get("capabilities", []):
                models.append(name)
        except (OSError, ValueError):
            continue
    return sorted(models)


def stream_ollama(p: Participant, system: str, prompt: str,
                  metrics: dict | None = None) -> Iterator[str]:
    from .config import LOCAL_CONTEXT_TOKENS
    options = {"num_predict": p.max_tokens, "num_ctx": p.context_tokens or LOCAL_CONTEXT_TOKENS}
    if p.temperature is not None:
        options["temperature"] = p.temperature
    request = urllib.request.Request((p.base_url or "http://127.0.0.1:11434").rstrip("/") + "/api/chat",
        data=json.dumps({"model": p.model, "stream": True,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": prompt}],
            "options": options, "keep_alive": 0}).encode(),
        headers={"Content-Type": "application/json"})
    finished = False
    with urllib.request.urlopen(request, timeout=p.timeout) as response:
        for line in response:
            if not line.strip():
                continue
            event = json.loads(line)
            if event.get("error"):
                raise RuntimeError(event["error"])
            text = event.get("message", {}).get("content", "")
            if text:
                yield text
            if event.get("done"):
                if metrics is not None:
                    # Counts arrive on Ollama's terminal NDJSON event. Keep
                    # reported zero distinct from missing usage.
                    for source, target in (("prompt_eval_count", "prompt_tokens"),
                                           ("eval_count", "output_tokens")):
                        count = event.get(source)
                        if isinstance(count, int) and not isinstance(count, bool) and count >= 0:
                            metrics[target] = count
                finished = True
                break
    if not finished:
        raise RuntimeError("Ollama connection ended before the reply finished")


def stream_cli(p: Participant, system: str, prompt: str,
               metrics: dict | None = None) -> Iterator[str]:
    """Bounded subprocess lifetime, isolated working directory, drained stderr."""
    payload = f"{system}\n\n---\n\n{prompt}".encode()
    # Keep inherited coding-session markers out of child CLI sessions.
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("CLAUDECODE", "CLAUDE_CODE_", "CODEX_THREAD_"))}
    timed_out = threading.Event()
    with tempfile.TemporaryDirectory(prefix="roundtable-seat-") as cwd:
        proc = subprocess.Popen(p.argv, cwd=cwd, env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        stderr_tail = bytearray()

        def feed():
            try:
                proc.stdin.write(payload)
            except (BrokenPipeError, OSError):
                pass
            finally:
                try:
                    proc.stdin.close()
                except OSError:
                    pass

        def drain():
            while block := proc.stderr.read(4096):
                stderr_tail.extend(block)
                del stderr_tail[:-4096]

        def kill():
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

        def expire():
            timed_out.set()
            kill()

        writer = threading.Thread(target=feed, daemon=True)
        reader = threading.Thread(target=drain, daemon=True)
        writer.start()
        reader.start()
        timer = threading.Timer(p.timeout, expire)
        timer.daemon = True
        timer.start()
        decoder = codecs.getincrementaldecoder("utf-8")("replace")
        try:
            while block := proc.stdout.read1(4096):
                yield decoder.decode(block)
            yield decoder.decode(b"", final=True)
            proc.wait()
        finally:
            timer.cancel()
            kill()
            proc.wait()
            writer.join(timeout=1)
            reader.join(timeout=1)
            proc.stdout.close()
            proc.stderr.close()
        if timed_out.is_set():
            raise RuntimeError(f"timed out after {p.timeout:.0f}s")
        if proc.returncode:
            # Provider stderr may include private environment details: report a
            # status and actionable login step rather than dumping its contents.
            raise RuntimeError(f"{p.model} exited {proc.returncode}; check its login/status in a terminal")
