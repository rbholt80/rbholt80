# Roundtable

A local browser and terminal app where your connected AI models discuss one
shared topic. You choose the topic, control the pace, and can interrupt. It
uses the same engine for both interfaces.

## Start the browser

From this directory:

```sh
./roundtable.sh
```

The launcher opens the existing table or starts one. Choose **New topic**, then
**Independent round** for separate first assessments or **Run one round** for
an ordinary discussion. A round gives each non-moderator seat one turn. **Auto
solve** keeps drafting and requesting a separate review without further turn
clicks. It prefers configured principal seats, tries other connections on
failure, and waits before retrying unavailable seats. Pause stops after the
current response.

Auto stops generating when another seat accepts a concrete answer, or identifies
essential missing input. It labels the result **Proposed answer**, not verified
truth. New Host messages restart Auto without another click while Auto remains
enabled. It can keep running indefinitely if reviewers cannot resolve the
question; usage continues to accumulate until an answer, missing-input pause,
or manual Pause. No provider restrictions, billing limits, or tool permissions
are bypassed.

The latest actual Host message stays in context even if older turns are trimmed.
Forged numbered speaker records are flagged on completion, kept out of later
prompts, and preserved in an expandable audit block and the local transcript.
Streaming text is provisional until that check completes. Plain addresses such
as "Host:" remain model text inside JSON-framed bodies. The format check
allows quotations and code examples; it cannot detect all fabricated claims.

No Python packages are needed for Ollama or the installed CLI connections.
Python 3.11 or newer is required. For hosted API adapters, run `./setup.sh` once;
it installs optional SDKs in `.venv`, leaving system Python alone. The launcher
uses that environment automatically when present.

```sh
./roundtable.sh --terminal "What should we discuss?"
./roundtable.sh --stop
python3 create_shortcuts.py
```

The last command creates two Linux desktop shortcuts in this directory. It
does not install system menu entries. CLI installation users can also run
`.venv/bin/roundtable web "Topic"` or activate `.venv` to use `roundtable` directly.

## Have a useful discussion

**Independent round** freezes the starting transcript. Each model gives its
assessment from that shared baseline, within its own context budget, without
seeing other answers produced in that round. New Host
messages still reach subsequent speakers. This reduces first-speaker anchoring;
it does not guarantee that model judgments are independent or correct.

**Challenge** attaches a Host question to an exact quote in a completed model
reply. The source turn number and quote are saved. Ask for evidence, missing
assumptions, or a test that would resolve it. The model can agree with the claim;
it is not instructed to manufacture disagreement. A transcript reference proves
where a statement was made, not whether it is true.

The default context window is 40 turns. Seats with `context_tokens` also trim
old whole turns to an estimated token budget, reserving the actual system prompt,
reply allowance, and framing space. Estimates use UTF-8 bytes and are not exact
model token counts. If the newest message or topic alone cannot fit, that seat
records a visible error before making a provider call; shorten it or use a
larger-context seat. Models see a notice when older turns
are omitted. Replies keep stable IDs and record which context they received,
including when the Host interjects during a reply. Transcripts are written to
JSONL after each completed turn and refreshed in Markdown after Host/model turns.
The launcher keeps them in `transcripts/`; ordinary CLI runs honor `--transcripts`.

Terminal commands:

| Command | Effect |
| --- | --- |
| Enter | Next model replies |
| Your text | Join as Host; `@Name` requests that seat |
| `/round` | One turn per non-moderator seat |
| `/opening` | Independent round from the same starting transcript |
| `/challenge 0 \| exact quote \| your question` | Question a claim in turn #0 |
| `/next Name` | Choose the next speaker |
| `/auto N` | Run N turns |
| `/auto` | Keep drafting and reviewing until a proposed answer or essential missing input; Ctrl+C pauses |
| `/cost` | Show usage and time per seat |
| `/moderate` | Ask the configured moderator to summarize |
| `/save` | Export Markdown |
| `/quit` | Save and exit |

Use finite rounds first. Ctrl+C during a terminal reply pauses it. The browser's
Pause button lets the current reply finish.

## Connections and roles

`roundtable doctor` discovers installed connections and checks CLI sign-in.
`roundtable doctor --probe` sends a short real request to each discovered seat
and therefore uses its account allowance. Discovery alone does not prove a model
will answer successfully.

- **Ollama:** discovers completion-capable installed models and uses native
  streaming without an SDK. Embedding models are excluded. Models below 4B
  parameters default to brief panel roles; configuration can override that.
  Native context defaults to 2,048 tokens to limit memory on small machines.
  Set `context_tokens` on a seat, `local_context` in `[roundtable]`, or use
  `--local-context N` for longer local inputs if memory permits. Hosted seats
  default to the turn window unless their own context budget is configured.
- **Claude CLI / Codex CLI:** reuse their existing logins. Automatic invocations
  restrict tools and inherited configuration, and run in temporary directories.
  Custom CLI commands in TOML are trusted operator configuration; review them
  before enabling. Hosted CLI services still consume their account allowances.
- **Hosted APIs:** enabled by their key environment variable and installed SDK.
  Provider/model examples in configuration are defaults, not a live model catalog.
  Override them for models available to your account. Do not export placeholder
  keys such as `...`; an apparent API connection can shadow a working CLI seat.
- **LM Studio:** uses the optional OpenAI-compatible adapter.
- **Grok:** needs configured xAI API access for automatic replies. A browser chat
  subscription is not automatically connected. Manual replies can be pasted as
  Host messages with their origin identified.

The original verified machine has seven Ollama chat models and two signed-in
CLI seats. `doctor` reports what is available on the current machine.

A `principal` gets a normal discussion brief. A `panel` seat offers one concrete
objection, example, or uncertainty. A `moderator` is outside the normal rotation
and speaks on `/moderate` or a configured cadence. `policy = "auto"` uses weighted
selection when weights differ. Explicit rounds retain the one-turn-per-seat
rule regardless of weights.

## Usage accounting

`/cost` and the browser show turns, elapsed seconds, and tokens per seat. Hosted
providers and native Ollama can report token counts. Where counts are missing,
output is estimated from character count and labeled as estimated. Estimates
are not billing records. Configure `price_in` and `price_out` in dollars per
million tokens to calculate usage at your own rates; otherwise no price is implied.

The OpenAI-compatible adapter negotiates supported token/usage parameters and
caches the accepted parameter names per endpoint/model. A probe's small response
budget does not cap later replies. Missing SDKs, connection failures, and
mid-stream failures remain visible; already received text is kept and the turn
is marked as failed.

## Goal mode

Beyond open-ended discussion, connected CLI and Ollama seats can also be set
loose on a bounded, autonomous goal -- writing code against a real project,
researching a question, or scoping a money-making idea -- via a native
desktop app or a command-line driver, both over the same `WorkManager`:

```sh
./goal-gui.sh                                   # native window: create, watch, resume
```

```sh
python3 goal.py create "add a --verbose flag" --project ~/some/repo
python3 goal.py create "what do people complain about with X" --mode research
python3 goal.py list
python3 goal.py show <id>
python3 goal.py resume <id> --feedback "also handle the empty case" --steps 10
python3 goal.py outcome <id> "sold for $40"
```

The GUI (`goal_gui.py`) is Tkinter, so it needs your system's Tk bindings --
usually already present, otherwise `sudo apt install python3-tk` on
Debian/Ubuntu. `python3 create_shortcuts.py` also generates a desktop
launcher for it alongside the discussion-app shortcuts.

A goal runs against a **sandboxed copy** of the target project (bubblewrap
filesystem/network isolation where available, `unshare --net` as a weaker
fallback), never the original directory. Only `cli`/`ollama`/`mock` seats
work goals; hosted API seats are excluded. Each step is one JSON tool call
(`plan`, `read_file`, `write_file`, `run`, `check_python`, `fetch_url`,
`diff`, `finish`, `needs_input`, ...) recorded as durable evidence on disk.

A `finish` proposal is never accepted by its own author: a second seat must
independently inspect the actual files or evidence (not just agree) before
the goal reaches `ready` status and a `changes.patch` is written. Small
local models often try to accept without inspecting first -- that is
rejected and the same seat is asked again rather than treated as a fatal
error, but it still costs step budget, so give a goal enough steps (`create
--steps N` or `resume --steps N`) to get through both the work and the
review.

This is not yet wired into the browser/terminal discussion UI above
(`cli.py`/`web.py`) -- `goal_gui.py` and `goal.py` are separate front ends
over the same `WorkManager`, not an extension of the discussion app.

## Configuration and boundaries

Copy `roundtable.example.toml` to `roundtable.toml` to select models, roles,
weights, timeouts, output limits, and rates. The default app sends the context
to whichever seat is speaking. A mixed session containing hosted seats is not
local-only. Choose only Ollama seats when the transcript must remain local.

Other speakers' text is quoted discussion data. The app does not refuse ordinary
discussion because it contains an injection phrase. CLI restrictions are applied
separately; regex filtering is not treated as a sandbox.

The localhost browser uses a per-session token, kept in the URL fragment and
sessionStorage, with an HttpOnly session-cookie fallback for browsers that clear
storage on reload. Cookie-only writes also require a custom request header.
Treat the token like a local session
credential. A new topic creates a separate transcript and refreshes discovery.

## Development and collaboration

```sh
python3 -m unittest discover -s tests -v
```

See `COLLABORATION.md` for the ownership proposal and `CODEX_STATUS.md` for verified
branch refs, integration work, validation results, and the next handoff. Fetch
remote refs before drawing conclusions from a local tracking branch.
