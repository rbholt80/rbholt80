# Codex integration status

Shared `engine.py` follow-up: the real run exposed an ordinary `Host:` address
being mistaken for a forged turn, and a reviewer ignoring its verdict format.
Reconcile ambiguous plain addresses with JSON-framed transcript bodies, and
reserve context for concise Auto output instructions after the transcript.

Live Auto inspection found a drift into a model-invented paper phone stand
experiment. Both drafting and review now receive the explicit current Host task
(the starting topic when no Host message exists). Peer examples cannot change
that stored task; only actual Host input updates it. The UI exposes Auto's
current request for inspection. All 11 focused Auto tests passed, including two
new task-drift regressions. This strengthens prompting, not a guarantee that
models will always follow it.

Claude's `a4c007b` / `9ce6018` are reconciled: crossed replies are labelled using
the existing stable IDs; possible invented speaker lines are flagged, preserved
for audit, and excluded from later context. Exact source quotes, blockquotes,
and code examples remain allowed. The latest actual Host message is pinned in
budgeted context; model bodies are JSON strings, never extra transcript records.

Robert requested continuous Auto mode. `autopilot.py` drives draft/review cycles
in both front ends, prefers principal seats, and accepts a proposal only from a
different reviewer referencing the exact candidate ID without blocking issues.
Model acceptance is labelled a proposal, not independent verification. Missing
essential information waits for the Host; a new Host message automatically
restarts an enabled Auto session and invalidates stale in-flight decisions.
Failed seats have retry delays; Pause remains available. Tool permissions and
provider limits are unchanged. There is no automatic total turn/cost cap in this
explicitly requested mode; finite rounds remain available.

Validation: 55 full-suite tests passed, then the added terminal Auto test and
all 8 other Auto tests passed. A real Claude draft and Codex review independently
computed a fictional quote comparison ($2,340 each) and produced a proposal
without intervening turn requests. A separate real Claude call followed a Host
topic change. Browser inspection verified the integrity warning and expandable
raw output; the subsequent test-page reload was blocked by approval-reviewer
usage exhaustion, not by an application error.

The integrated branch is `codex/roundtable-local-integration`. It contains both
agents' changes, including Claude's latest `8e33e73` (and its reconnect fix
`7a83b0e`). The initial integration merge is `2b790d0`; reconciliation with those
latest commits is `3b96112`. Subsequent commits finish installed-browser behavior
and documentation. Use the branch head, not an old local tracking ref.

```sh
git fetch origin refs/heads/codex/roundtable-local-integration:refs/remotes/origin/codex/roundtable-local-integration
git log -5 --oneline origin/codex/roundtable-local-integration
```

## What is integrated

- Claude's roles, weights, fair queued rounds, moderator cadence, venv setup,
  usage accounting, and OpenAI-compatible parameter negotiation.
- Codex's native Ollama transport, CLI sign-in checks and restrictions,
  subprocess timeout cleanup, partial-failure persistence, and launchers.
- Independent rounds: freeze starting context for all queued seats, while still
  accepting fresh Host input. Earlier replies in that round do not influence
  later seats' input.
- Claim challenges: exact source quote, turn ID, and Host question are validated
  and persisted. The quote remains available when its original turn is outside
  the context window. Agreement remains allowed.
- Stable turn IDs allocated when a reply begins; a Host interjection cannot
  reuse its ID. Saved replies record the context boundary they actually saw.

## Reconciled choices

The SSE server snapshots a committed event view under the same lock used to
publish and subscribe. It does not snapshot the engine's partially published
history. This closes the persistence/broadcast gap without relying on history
length as an event watermark, which would be wrong for out-of-order completion
IDs after a Host interjection. Clients also deduplicate completed and live starts.

Dialect caching stores parameter names, never a previous call's token limit.
Both agents independently fixed this; a 32-token probe no longer limits later
responses. Retries do not replay partially emitted replies.

Actual browser tests exposed two embedded-browser differences: sessionStorage
was not retained on reload, and native prompt dialogs did not open. The app now
has an authenticated HttpOnly session-cookie fallback and accessible in-page
forms. Cookie-only POSTs require a custom header; arbitrary cross-origin form
submissions are not accepted. Cookies are separated by the server port.

The earlier native Ollama 8192-token setting triggered an OOM kill on Robert's
7.4 GiB machine during the demo. The default is now 2048; `context_tokens` is
configurable per seat. All seven local models subsequently answered short probes.
Claude's per-seat budgeting is integrated for normal and independent rounds.
It reserves the actual system prompt, reply allowance, and framing overhead,
uses a conservative UTF-8 estimate, and drops old whole turns with a marker.
An oversized newest message records an explicit error before a provider call.
The estimate is not an exact tokenizer guarantee. Hosted seats retain the turn
window unless a budget is explicitly set. `--local-context` and the matching
TOML setting are preserved when New topic refreshes participants. Small local models can produce weak or inaccurate
answers; model agreement is not evidence.

## Verification

- 39 regression tests passed with the installed venv interpreter, including
  offline streaming through the actual OpenAI SDK, subprocess descendants,
  partial failures, exact rounds, independent context, claim provenance,
  concurrent Host IDs, auth, cookie reload, and configuration-preserving New topic.
- 500 subscribers raced a writer of 80 Host messages: each saw every message
  once across its snapshot plus queued events.
- `doctor --probe`: **9/9 answered** (seven installed Ollama chat models,
  Claude CLI, Codex CLI). No paid API key was added or used.
- The installed browser reloaded with its conversation intact. A real local
  Qwen reply, exact-quote challenge form, and cross-model follow-up were exercised.
  Claude answered the quote comparison; Codex corrected its mistaken setup-price
  difference and time horizon after a Host challenge. The fictional demo was
  saved locally, not published as verified client advice.

## Organization and next handoff

Robert's existing `~/rbholt80-merge/roundtable` installation now runs this branch.
The original output copy and transcripts were archived locally; its familiar
output path points to the installed tree. The command and launcher use the same
installation. Development happens in the Git checkout; runtime tokens and
transcripts are ignored and are not published.

`PAID_PILOT.md` contains a proposed $149 website-quote comparison service, a
fictional sample, outreach drafts, and explicit paid-validation gates. No buyer,
revenue, outreach, invoice, or sale is claimed.

The ownership proposal remains a useful default. Both isolated contract tests
and real-machine compatibility checks are needed. No blanket instruction-regex
refusal is planned: quoted injection discussion is legitimate, and CLI permissions
must be restricted independently of prompt contents. The complete FourHorsemen
validator is game-coupled; the inspected regex also has false positives and gaps.

Claude can review this branch and fast-forward its branch if it has made no
additional commits since `8e33e73`. Otherwise merge it normally and inspect any
new conflicts. No force push is needed.
