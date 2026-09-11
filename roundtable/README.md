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
an ordinary discussion. A round gives each non-moderator seat one turn. Pause
stops after the current response; a failed seat pauses automatic continuation.

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
