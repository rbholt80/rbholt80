# Roundtable

Put every AI you have in one room and let them argue.

Claude, ChatGPT, Grok, Gemini, DeepSeek, a local Llama, even the CLIs already
installed on your machine — each one sees the full labelled transcript, knows
who else is at the table, and replies to what was actually just said. You watch
it stream, and you can cut in whenever you want.

Two front ends over one engine: a terminal and a local web page.

```
roundtable doctor                              # what can this machine seat?
roundtable "Is it worth learning to code in 2026?"
roundtable web "Should we ship on Friday?"     # same thing, in a browser
```

---

## Install

```bash
cd roundtable
./setup.sh
```

That builds a virtual environment and installs into it. Debian, Ubuntu and
Fedora mark the system Python as externally managed (PEP 668), so a plain
`pip install` there fails with `externally-managed-environment` — that is the
OS protecting itself, not a problem with this project. If `setup.sh` reports
that it cannot create the environment, install `python3-venv` (`sudo apt
install python3-venv`) and run it again.

By hand, if you prefer, or to install only some of the seats:

```bash
python3 -m venv .venv && source .venv/bin/activate
pip install -e ".[all]"        # or: .[anthropic] / .[openai] / .[gemini]
```

The `roundtable` command lives inside that environment, so it is on your PATH
only while it is activated. To reach it from any shell, link it once:

```bash
mkdir -p ~/.local/bin && ln -sf "$PWD/.venv/bin/roundtable" ~/.local/bin/roundtable
```

`openai` is the workhorse dependency — it covers OpenAI, xAI, Groq, DeepSeek,
Mistral, Together, OpenRouter, Perplexity, Ollama and LM Studio, because they
all speak the same dialect. Install only the extras you want.

Then export whichever keys you have. Roundtable seats whoever shows up:

```bash
export ANTHROPIC_API_KEY=...      # Claude
export OPENAI_API_KEY=...         # ChatGPT
export XAI_API_KEY=...            # Grok
export GEMINI_API_KEY=...         # Gemini
# ...and DEEPSEEK_API_KEY, GROQ_API_KEY, MISTRAL_API_KEY, TOGETHER_API_KEY,
#    OPENROUTER_API_KEY, PERPLEXITY_API_KEY
```

`roundtable doctor` tells you exactly what it found, what it *nearly* found
(key set but SDK missing, and the one-line fix), and what it skipped.

Being on `PATH` is not the same as being signed in, and an exported key is not
the same as a working one. `roundtable doctor --probe` calls every seat once
with a trivial prompt and reports what actually came back — a signed-out CLI
shows up as a failure with its own error text, not as a ready seat.

---

## How the table fills itself

There is no hardcoded list of models. On every run Roundtable looks at the
machine it is on:

| Source | What it looks for |
|---|---|
| **Hosted APIs** | the key env vars above, plus the matching SDK |
| **Local servers** | Ollama on `:11434` (every chat-capable model gets its own seat), LM Studio on `:1234` — no key needed |
| **Installed CLIs** | `claude`, `codex`, `gemini`, `llm` on your `PATH` |

If a vendor offers both an API seat and a CLI seat, the API seat wins by
default: it is faster, cheaper, and it doesn't run an agent with filesystem
access just to have an opinion. `doctor` still lists the CLI, and `--only` or
the config file will seat it anyway.

### About the CLI seats

`Claude-CLI` and `Codex-CLI` are not chat endpoints. They are coding agents,
and they run **in your current working directory with whatever tool access you
have granted them** — they can read files, and depending on your settings,
write them. That is occasionally what you want (a participant that can actually
go look at the repo you are arguing about) and usually not. They are off by
default when the API seat exists. Enable them deliberately, and mind the
directory you start from.

---

## Driving it

**Terminal**

```
Enter            let the next model speak
<text>           join in; @Grok hands the floor to that seat
/auto [n]        models keep talking (n turns, or until Ctrl+C)
/next <Name>     put a specific model up next
/who             who is at the table
/save            write the markdown transcript now
/quit            exit
```

**Browser** — `roundtable web "topic"` prints a localhost URL with a one-run
token in the fragment. Click a name in the header to hand that model the floor,
**Next** to advance, **Auto** to let them run, type to cut in. Cmd/Ctrl+Enter
advances without sending. The page is theme-aware and works at phone width, so
you can leave it open on a second screen.

Both front ends drive the same engine over the same events, so they behave
identically — and you can point a browser at a session you started from the
terminal's `web` subcommand and watch the same conversation.

---

## Transcripts

Every turn is appended to `roundtable-<timestamp>.jsonl` **the moment it
finishes**, so a crash or a Ctrl+C costs you nothing. On exit (or `/save`) you
also get a readable `.md` alongside it. Filenames are timestamped, so runs never
overwrite each other.

---

## Configuration

Optional. Drop a `roundtable.toml` in the working directory or
`~/.config/roundtable/` to pin the table, change models, or give each model an
angle. See `roundtable.example.toml` — copy it and delete what you don't have.

The one setting worth knowing about is `persona`:

```toml
[[participant]]
name = "Grok"
kind = "openai"
model = "grok-4"
base_url = "https://api.x.ai/v1"
api_key_env = "XAI_API_KEY"
persona = "the contrarian; find the strongest objection nobody has made yet"
```

Three assistants with no personas tend to violently agree. Give them different
jobs and you get an actual discussion.

### Roles: not every seat should carry equal weight

A 1B local model given the same brief as a frontier model produces four hedged
sentences that restate the topic. That is not a bug in the model — it is the
wrong job for it. Each seat has a `role`:

| Role | Brief | Floor time |
|---|---|---|
| `principal` | argue, disagree, concede, ask real questions | full |
| `panel` | exactly one concrete objection or piece of evidence, 1–2 sentences, no summarising and no praise — and an explicit licence to say only what it would need to know instead of padding | reduced (`weight`) |
| `moderator` | does not argue; names the disagreement, what is settled, and what would resolve it | on a cadence, not in rotation |

Local models are assigned a role automatically from their parameter count
(under 4B → `panel`), which Ollama reports. Override any of it in the config.

Turn policy defaults to `auto`: plain round-robin when every seat has the same
weight, weighted rotation once they don't. `--policy` forces a specific one.

A moderator speaks every `moderate_every` turns (0 = never), or on `/moderate`.
Deliberately *not* per-turn: asking a model who should speak next before every
reply doubles your calls to buy an ordering the transcript already implies.

### Cost

Every turn resends the transcript, so an unattended `/auto` session is a
spending loop. Two things bound it:

- `context_tokens` on a seat caps the transcript by what that model can
  actually hold, rather than by a turn count that means something different
  for every seat. Local models are given 8192 automatically (Ollama's
  default); a seat with a large window still sees everything. Without this, a
  long session silently overruns the small models — and an overrun drops the
  *start* of the conversation, which is where the question was asked.
- `context_turns` (default 40) caps how much transcript each model sees. Models
  are told plainly that earlier turns were omitted rather than being left to
  assume the conversation started mid-argument.
- `max_tokens` (default 1024) caps each reply, and the system prompt asks for a
  few sentences.

`/cost` at any time (and automatically on exit) prints tokens, seconds and
turns per seat, so you can see which seat is actually expensive. Tokens are
the default unit because only some providers report them and only you know
your rates; set `price_in` / `price_out` on a seat (dollars per million
tokens) and its row gains a cost column. Where a provider reports no token
count — most local servers, every CLI — the output figure is estimated from
character count and the table says so rather than quietly mixing measured and
guessed numbers.

For Claude seats, `effort = "low"` keeps conversational turns quick and cheap;
raise it if you want the table thinking harder. `temperature` is not sent to
Claude — current models removed sampling parameters — but it works on
OpenAI-compatible and Gemini seats.

### Testing without spending anything

```toml
[[participant]]
name = "Mock"
kind = "mock"
```

A `mock` seat streams a canned reply instantly and costs nothing. Useful for
checking the UI, the transcript and your config before pointing it at anything
billable.

---

## Layout

```
roundtable/
  config.py      participants, discovery, TOML
  providers.py   streaming adapters: anthropic | openai | gemini | cli | mock
  engine.py      conversation state, turn taking, transcripts
  cli.py         terminal front end
  web.py         localhost server (SSE)
  static/        the web page
```

Adding a provider means one function in `providers.py` and one line in
`_ADAPTERS`.

---

## Failure behaviour

One seat going down never ends the conversation. A model that errors, times
out, or isn't configured gets an in-line `[Name unavailable: ...]` note, is
marked as an errored turn in the transcript, and the table moves on. That is
deliberate: a roundtable that dies because one API had a bad minute is useless
for the long unattended sessions this is built for.

Failure is raised, never returned as text. Adapters raise `ProviderError`;
whatever streamed before the failure is kept, and the engine appends the note
and sets the errored flag. A reply that got three sentences out before the
connection dropped is still worth three sentences — and the flag comes from the
exception rather than from pattern-matching the reply's punctuation.
