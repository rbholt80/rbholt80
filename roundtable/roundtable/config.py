"""Participant definitions, runtime discovery, and TOML config loading.

The guiding idea: don't hardcode who is at the table. Look at the machine the
tool is actually running on -- which API keys are exported, which SDKs are
importable, which CLIs are on PATH, whether a local server is listening -- and
seat whoever shows up.
"""

from __future__ import annotations

import functools
import importlib.util
import json
import os
import shutil
import socket
import subprocess
import urllib.error
import urllib.request
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any, Iterable

try:  # py3.11+
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - py3.10
    tomllib = None  # type: ignore[assignment]


# --- palette ----------------------------------------------------------------
# 256-colour terminal codes, paired with a hex for the web UI.
PALETTE: list[tuple[str, str]] = [
    ("\033[38;5;173m", "#c98a5e"),  # warm clay
    ("\033[38;5;75m", "#4a9eda"),   # sky
    ("\033[38;5;114m", "#6fbf7f"),  # green
    ("\033[38;5;176m", "#b57edc"),  # violet
    ("\033[38;5;179m", "#d9a441"),  # amber
    ("\033[38;5;80m", "#43bec4"),   # teal
    ("\033[38;5;210m", "#e8836f"),  # coral
    ("\033[38;5;108m", "#8aab86"),  # sage
]


@dataclass
class Participant:
    """One seat at the table."""

    name: str
    kind: str                      # anthropic | openai | gemini | cli
    model: str = ""
    api_key_env: str | None = None
    base_url: str | None = None
    argv: list[str] = field(default_factory=list)   # kind == "cli"
    persona: str = ""              # extra system-prompt line, optional
    role: str = "principal"        # principal | panel | moderator
    context_tokens: int | None = None  # usable context; None = use turn count
    price_in: float | None = None  # $ per million input tokens, if you want $
    price_out: float | None = None # $ per million output tokens
    weight: float = 1.0            # relative floor time under the weighted policy
    max_tokens: int = 1024
    effort: str | None = None      # Claude only: low|medium|high|xhigh|max
    temperature: float | None = None
    timeout: float = 180.0
    enabled: bool = True
    color: str = PALETTE[0][0]
    hex: str = PALETTE[0][1]
    source: str = "config"         # where this seat came from, for `doctor`
    note: str = ""                 # human-readable caveat, for `doctor`

    @property
    def api_key(self) -> str | None:
        return os.environ.get(self.api_key_env) if self.api_key_env else None


# --- what an OpenAI-compatible vendor needs ---------------------------------
# Almost every vendor speaks the OpenAI chat-completions dialect now, so one
# adapter covers most of the table. (name, key env, base_url, default model)
OPENAI_COMPATIBLE: list[tuple[str, str, str | None, str]] = [
    ("ChatGPT",    "OPENAI_API_KEY",     None,                             "gpt-5"),
    ("Grok",       "XAI_API_KEY",        "https://api.x.ai/v1",            "grok-4"),
    ("DeepSeek",   "DEEPSEEK_API_KEY",   "https://api.deepseek.com",       "deepseek-chat"),
    ("Groq",       "GROQ_API_KEY",       "https://api.groq.com/openai/v1", "llama-3.3-70b-versatile"),
    ("Mistral",    "MISTRAL_API_KEY",    "https://api.mistral.ai/v1",      "mistral-large-latest"),
    ("Together",   "TOGETHER_API_KEY",   "https://api.together.xyz/v1",    "meta-llama/Llama-3.3-70B-Instruct-Turbo"),
    ("OpenRouter", "OPENROUTER_API_KEY", "https://openrouter.ai/api/v1",   "openrouter/auto"),
    ("Perplexity", "PERPLEXITY_API_KEY", "https://api.perplexity.ai",      "sonar-pro"),
]

# Local OpenAI-compatible servers, probed by opening a socket.
LOCAL_SERVERS: list[tuple[str, str, int, str]] = [
    ("Ollama",   "http://localhost:11434/v1", 11434, "llama3.2"),
    ("LMStudio", "http://localhost:1234/v1",  1234,  "local-model"),
]

# CLIs worth probing for on PATH. The prompt always goes in on stdin, never
# argv -- argv has length limits and quoting hazards a transcript will hit.
CLI_CANDIDATES: list[tuple[str, str, list[str], str]] = [
    # (display name, executable, args, note)
    ("Claude-CLI", "claude", ["-p"],
     "Claude Code CLI: runs with its own tool access in the working directory."),
    ("Codex-CLI",  "codex",  ["exec", "-"],
     "OpenAI Codex CLI: agentic, may touch the filesystem."),
    ("Gemini-CLI", "gemini", ["-p"],
     "Gemini CLI."),
    ("LLM-CLI",    "llm",    [],
     "simonw/llm: whichever model it defaults to."),
]


def _has_module(name: str) -> bool:
    return importlib.util.find_spec(name) is not None


def _port_open(port: int, host: str = "127.0.0.1", timeout: float = 0.25) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(timeout)
        return s.connect_ex((host, port)) == 0


# Substrings that mark an embedding model. Only consulted when the Ollama
# build is too old to report capabilities.
_EMBED_HINTS = ("embed", "bge-", "gte-", "e5-", "minilm", "nomic-", "arctic-")


def _ollama_capabilities(root: str, model: str) -> list[str] | None:
    """What a pulled model can do. None if this Ollama build won't say."""
    req = urllib.request.Request(
        root + "/api/show",
        data=json.dumps({"model": model}).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=2.0) as resp:
            return json.load(resp).get("capabilities")
    except (urllib.error.URLError, OSError, ValueError):
        return None


def _parameter_billions(raw: str | None) -> float | None:
    """'3.8B' -> 3.8, '596.05M' -> 0.596. None if Ollama didn't say."""
    if not raw:
        return None
    text = raw.strip().upper()
    try:
        if text.endswith("B"):
            return float(text[:-1])
        if text.endswith("M"):
            return float(text[:-1]) / 1000
        return float(text) / 1e9
    except ValueError:
        return None


# Below this, a model reliably restates the topic and hedges rather than
# holding a position. It is still useful for breadth -- just not as a
# full-weight voice. See Participant.role.
PANEL_THRESHOLD_B = 4.0


def _ollama_chat_models(base_url: str) -> list[tuple[str, float | None]]:
    """Every pulled model that can hold a conversation.

    An embedding model has no business at a roundtable -- it cannot produce a
    reply at all -- so ask Ollama what each model is actually capable of and
    seat only the ones that can complete text. Older Ollama builds don't
    report capabilities; fall back to the name, which is a weaker signal but
    beats seating `nomic-embed-text` and waiting for it to say something.
    """
    root = base_url.rsplit("/v1", 1)[0]
    try:
        with urllib.request.urlopen(root + "/api/tags", timeout=2.0) as resp:
            entries = json.load(resp).get("models") or []
    except (urllib.error.URLError, OSError, ValueError):
        return []

    chat: list[tuple[str, float | None]] = []
    for entry in entries:
        name = entry.get("name")
        if not name:
            continue
        size = _parameter_billions((entry.get("details") or {}).get("parameter_size"))
        caps = _ollama_capabilities(root, name)
        if caps is None:
            if not any(h in name.lower() for h in _EMBED_HINTS):
                chat.append((name, size))
        elif "completion" in caps:
            chat.append((name, size))
    return chat


def _seat_name(model: str, taken: set[str]) -> str:
    """A short, @mentionable name for a local model tag."""
    base, _, tag = model.partition(":")
    pretty = base.replace("-", " ").replace("_", " ").title().replace(" ", "")
    if pretty not in taken:
        return pretty
    # Two variants of the same family -- keep the tag to tell them apart.
    candidate = f"{pretty}-{tag}" if tag else pretty
    suffix = 2
    while candidate in taken:
        candidate, suffix = f"{pretty}-{suffix}", suffix + 1
    return candidate


@functools.lru_cache(maxsize=32)
def _cli_help(exe_path: str) -> str:
    """A CLI's own --help, so we can check a flag exists before passing it."""
    try:
        done = subprocess.run(
            [exe_path, "--help"], capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return ""
    return (done.stdout or "") + (done.stderr or "")


def _cli_supports(exe_path: str, flag: str, subcommand: str | None = None) -> bool:
    if _cli_help(exe_path) and flag in _cli_help(exe_path):
        return True
    if subcommand:
        try:
            done = subprocess.run([exe_path, subcommand, "--help"],
                                  capture_output=True, text=True, timeout=10)
        except (OSError, subprocess.SubprocessError):
            return False
        return flag in (done.stdout or "") + (done.stderr or "")
    return False


def discover(include_cli: bool = True, include_local: bool = True,
             dedupe: bool = True) -> list[Participant]:
    """Everything this machine can currently seat, best candidates first."""
    found: list[Participant] = []

    # Anthropic gets its own adapter (streaming shape differs from OpenAI's).
    if os.environ.get("ANTHROPIC_API_KEY") and _has_module("anthropic"):
        found.append(Participant(
            name="Claude", kind="anthropic", model="claude-opus-5",
            api_key_env="ANTHROPIC_API_KEY", effort="low", source="api",
        ))

    if _has_module("openai"):
        for name, key_env, base_url, model in OPENAI_COMPATIBLE:
            if os.environ.get(key_env):
                found.append(Participant(
                    name=name, kind="openai", model=model,
                    api_key_env=key_env, base_url=base_url, source="api",
                ))

    if os.environ.get("GEMINI_API_KEY") or os.environ.get("GOOGLE_API_KEY"):
        if _has_module("google.genai"):
            key_env = "GEMINI_API_KEY" if os.environ.get("GEMINI_API_KEY") else "GOOGLE_API_KEY"
            found.append(Participant(
                name="Gemini", kind="gemini", model="gemini-2.5-pro",
                api_key_env=key_env, source="api",
                note="check the model id against current Gemini releases",
            ))

    if include_local and _has_module("openai"):
        for name, base_url, port, default_model in LOCAL_SERVERS:
            if not _port_open(port):
                continue
            if name == "Ollama":
                taken = {p.name for p in found}
                for model, size in _ollama_chat_models(base_url):
                    seat = _seat_name(model, taken)
                    taken.add(seat)
                    small = size is not None and size < PANEL_THRESHOLD_B
                    found.append(Participant(
                        name=seat, kind="openai", model=model,
                        base_url=base_url, api_key_env=None, source="local",
                        context_tokens=8192,
                        role="panel" if small else "principal",
                        weight=0.35 if small else 1.0,
                        max_tokens=160 if small else 1024,
                        note=("local via Ollama"
                              + (f", {size:g}B — panel seat" if small else "")),
                    ))
                continue
            found.append(Participant(
                name=name, kind="openai", model=default_model,
                base_url=base_url, api_key_env=None, source="local",
                note=f"local server on :{port}, no API key needed",
            ))

    if include_cli:
        for name, exe, args, note in CLI_CANDIDATES:
            path = shutil.which(exe)
            if not path:
                continue
            argv = [path, *args]
            # Being on PATH says nothing about being signed in, and a CLI
            # agent inherits whatever tool access its config grants. Pin the
            # sandbox where the binary offers one -- checked against its own
            # --help rather than assumed, so an unknown build degrades to the
            # user's default instead of dying on a bad flag.
            if exe == "codex" and _cli_supports(path, "--sandbox", "exec"):
                argv = [path, "exec", "--sandbox", "read-only", "-"]
                note = "Codex CLI, pinned to a read-only sandbox."
            found.append(Participant(
                name=name, kind="cli", model=exe, argv=argv,
                source="cli",
                note=note + " Sign-in not verified — `doctor --probe`.",
            ))

    return _dedupe(found) if dedupe else found


# Vendors where an API seat and a CLI seat are the same underlying model.
_SAME_VENDOR = {"Claude": "Claude-CLI", "ChatGPT": "Codex-CLI", "Gemini": "Gemini-CLI"}


def _dedupe(found: list[Participant]) -> list[Participant]:
    """Prefer the API seat over the CLI seat for the same vendor.

    A CLI seat is an agent with tool access and startup overhead; for pure
    conversation the API is faster, cheaper and has no side effects. The CLI
    seat stays discoverable -- `doctor` lists it -- just not auto-seated.
    """
    names = {p.name for p in found}
    shadowed = {cli for api, cli in _SAME_VENDOR.items() if api in names}
    return [p for p in found if p.name not in shadowed]


def blocked() -> list[tuple[str, str]]:
    """Seats that would exist if something small were fixed. (name, remedy)"""
    out: list[tuple[str, str]] = []

    if os.environ.get("ANTHROPIC_API_KEY") and not _has_module("anthropic"):
        out.append(("Claude", "ANTHROPIC_API_KEY is set — pip install anthropic"))
    elif _has_module("anthropic") and not os.environ.get("ANTHROPIC_API_KEY"):
        out.append(("Claude", "anthropic is installed — export ANTHROPIC_API_KEY"))

    have_openai = _has_module("openai")
    for name, key_env, _base, _model in OPENAI_COMPATIBLE:
        if os.environ.get(key_env) and not have_openai:
            out.append((name, f"{key_env} is set — pip install openai"))

    gem_key = os.environ.get("GEMINI_API_KEY") or os.environ.get("GOOGLE_API_KEY")
    if gem_key and not _has_module("google.genai"):
        out.append(("Gemini", "GEMINI_API_KEY is set — pip install google-genai"))

    return out


def paint(participants: Iterable[Participant]) -> list[Participant]:
    """Assign each seat a stable colour."""
    out = []
    for i, p in enumerate(participants):
        term, hexcode = PALETTE[i % len(PALETTE)]
        out.append(replace(p, color=term, hex=hexcode))
    return out


# --- TOML -------------------------------------------------------------------

DEFAULT_CONFIG_NAMES = ("roundtable.toml", ".roundtable.toml")


def find_config(explicit: str | None = None) -> Path | None:
    if explicit:
        path = Path(explicit).expanduser()
        if not path.is_file():
            raise FileNotFoundError(f"config not found: {path}")
        return path
    for directory in (Path.cwd(), Path.home() / ".config" / "roundtable", Path.home()):
        for name in DEFAULT_CONFIG_NAMES:
            candidate = directory / name
            if candidate.is_file():
                return candidate
    return None


def load_config(path: Path) -> dict[str, Any]:
    if tomllib is None:
        raise RuntimeError("TOML config needs Python 3.11+ (or install tomli)")
    with path.open("rb") as fh:
        return tomllib.load(fh)


_PARTICIPANT_FIELDS = {
    "kind", "model", "api_key_env", "base_url", "argv", "persona",
    "max_tokens", "effort", "temperature", "timeout", "enabled",
    "role", "weight", "price_in", "price_out", "context_tokens",
}


def participants_from_config(data: dict[str, Any]) -> list[Participant]:
    """Build seats from a [[participant]] table list."""
    out: list[Participant] = []
    for entry in data.get("participant", []):
        if "name" not in entry:
            raise ValueError("every [[participant]] needs a name")
        unknown = set(entry) - _PARTICIPANT_FIELDS - {"name"}
        if unknown:
            raise ValueError(f"{entry['name']}: unknown keys {sorted(unknown)}")
        kwargs = {k: v for k, v in entry.items() if k in _PARTICIPANT_FIELDS}
        out.append(Participant(name=entry["name"], source="config", **kwargs))
    return [p for p in out if p.enabled]


def resolve(
    config_path: str | None = None,
    only: list[str] | None = None,
    include_cli: bool = True,
) -> tuple[list[Participant], dict[str, Any]]:
    """Config file if there is one, otherwise whatever the machine offers."""
    settings: dict[str, Any] = {}
    path = find_config(config_path)
    if path:
        data = load_config(path)
        settings = data.get("roundtable", {})
        seats = participants_from_config(data)
        if not seats:  # a config with no participants still means "discover"
            seats = discover(include_cli=include_cli)
    else:
        seats = discover(include_cli=include_cli)

    if only:
        wanted = {n.casefold() for n in only}
        seats = [p for p in seats if p.name.casefold() in wanted]
        missing = wanted - {p.name.casefold() for p in seats}
        if missing:
            raise ValueError(f"no such participant: {', '.join(sorted(missing))}")

    return paint(seats), settings
