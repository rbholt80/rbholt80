# Handoff

Read this first if you're an AI (or a human) picking up this project without
the conversation that built it. Last updated 2026-09-13.

## What this is

Roundtable connects every AI Robert has — installed CLIs (Claude Code, Codex),
local Ollama models, hosted APIs — into either (a) one shared discussion, or
(b) a bounded, autonomous "goal mode" that works a coding/research/money task
end to end with sandboxing and independent review.

## Where things actually stand

Everything described below is merged into `main` ([PR #2](https://github.com/rbholt80/rbholt80/pull/2)).
There is no longer a scattered set of feature branches to reconcile — `main`
is the current truth.

- **Discussion engine**: working, exercised extensively on Robert's real
  machine (9 real seats: 7 Ollama models, Claude CLI, Codex CLI).
- **Goal mode**: as of this merge, verified working **end to end for the
  first time** — a worker executed a real sandboxed command, submitted
  `finish`, and a *second* seat independently inspected the evidence (not
  just agreed) and accepted it, producing a written `changes.patch`. Every
  individual piece (sandbox isolation, evidence-grounded finish, self-review
  prevention) had been verified before; this was the first time the whole
  chain completed. Has both a native Tkinter GUI (`goal_gui.py`) and a
  terminal driver (`goal.py`) — not merged into the discussion app's own
  browser/terminal UI, which is a separate front end.

## Run it

```sh
./roundtable.sh                                  # browser discussion app
./roundtable.sh --terminal "topic"                # terminal version
./goal-gui.sh                                    # native goal-mode window
python3 goal.py create "task" --project ~/repo    # autonomous goal mode
python3 goal.py resume <id> --steps 8
```
Full usage detail (including the terminal command table and goal-mode
constraints) is in `README.md`. Run `roundtable doctor` to see what this
machine can actually seat.

## Architecture map

| Files | What |
|---|---|
| `engine.py`, `providers.py`, `config.py` | Turn-taking, streaming, per-seat context budgeting. Shared — see COLLABORATION.md before editing. |
| `cli.py`, `web.py`, `static/index.html` | Terminal and browser front ends. |
| `safety.py` | Fences suspicious (instruction-shaped) text for *other* seats only; never hides or edits what the host sees. |
| `autopilot.py` | Continuous draft/independent-review loop for discussion mode (`/auto`). |
| `local.py` | CLI subprocess + native Ollama transport, no SDK required. |
| `work.py`, `worktools.py` | Goal-mode loop and its sandbox (bubblewrap/unshare, resource limits, path confinement). |
| `goal_gui.py`, `goal.py` | Native Tkinter GUI and terminal driver for goal mode (repo root), both thin front ends over `WorkManager`. |
| `tests/` | 69 passed, 1 skipped as of this merge (goal_gui.py has no automated tests — GUI construction was verified manually under Xvfb, not added to the suite; see note below). |

## Real bugs found and fixed via live testing (most recent first)

- **`{"tool":"diff"}` was completely unimplemented** despite being documented
  and reviewer-whitelisted — any seat that tried the obvious inspection step
  hit `unknown tool: 'diff'`. Fixed in `worktools.py`.
- **A reviewer that accepted without inspecting was treated as a hard error**,
  rotating the whole reviewer pool through one failed attempt each with no
  seat ever getting a second try. Now it's a nudge that keeps the same seat
  up next, bounded by step budget. Fixed in `work.py`, covered by
  `tests/test_work_review.py`.
- **A worker's JSON response with trailing commentary** (`json.loads` demands
  an exact match) failed the whole parse for small local models that add a
  sentence after their JSON. Switched to `json.JSONDecoder().raw_decode()`.
- **Challenge-budget lockout**: one large challenge could permanently pin a
  "latest Host message" too big for a small seat's context, failing it
  identically forever. Fixed at creation time in `engine.py`.
- **Seat-backoff introduced a stale round-robin index bug** that made one
  seat repeat 20 times in a row once cooldown filtering shrank the pool.
- **SSE duplicate-turn race**: subscribing and snapshotting weren't atomic.
  Fixed with a per-subscriber watermark.
- Earlier history (dialect-cache poisoning, OOM from an 8192-token local
  default, fabricated speaker turns) is in `CODEX_STATUS.md` and
  `MERGE_NOTES.md` — those predate this merge and describe the
  pre-integration branches; treat them as historical record, not current
  state, if anything here conflicts with them.

## Known limitations / open items

- Goal mode has a native GUI (`goal_gui.py`) and a terminal driver
  (`goal.py`) now, but neither is wired into the discussion app's own
  `cli.py`/`web.py` — they're separate front ends, by design, not a gap
  to close later.
- `goal_gui.py` was verified by construction and a scripted run under
  Xvfb (window builds, a goal creates/starts/runs/updates the log pane
  live, both dialogs open cleanly) — not by a human looking at it. It is
  not in `tests/`, deliberately: adding it to the standard suite would
  make `python3 -m unittest discover` fail on any machine without Tk
  installed, which the rest of this project doesn't require. Actual
  visual/UX verification on a real desktop is still outstanding.
- Small local models (sub-4B) frequently can't complete the two-step
  review protocol (inspect, then verdict) even with the retry fix above —
  it took Gemma2 five attempts across two `resume` calls to get there once.
  If this keeps being the bottleneck, the next move is probably either a
  stronger dedicated reviewer seat or a simplified one-shot review prompt
  for weak models, not another retry-logic patch.
- Phase 2 items from the original handoff are still deferred, not dropped:
  MCP server, knowledge base, skills library, OpenRouter integration, the
  "money loop" (scout/debate/build/ship/learn), voice interface.
- Hard rules still in force from that handoff: closed-source/paid-only
  license posture, human approval required before any money/publishing
  action, self-improvement only via branch+PR never touching main directly,
  vet-before-seating for new models, no dark-web integration.

## Collaboration model

See `COLLABORATION.md` for the ownership split between the two agents that
built this (Claude: engine/providers/browser-testable code; Codex:
anything needing the real machine). Both branches are merged now, but the
rule about flagging `engine.py`/`providers.py`/`config.py` changes in the
commit message still applies to whoever touches them next.

## If you're an AI picking this up cold

1. Read this file, then `README.md` for usage, then `COLLABORATION.md`
   before changing shared files.
2. Robert runs everything from `~/rbholt80-merge/roundtable` on a real
   Ubuntu desktop. Nothing here — sandbox behavior, live model responses,
   whether a fix actually works — can be verified from description alone;
   ask him to run it and paste output.
3. Goal-mode evidence for any run lives at
   `~/.roundtable-goals/<goal-id>/` on that machine, not in this repo.
4. Session that produced this file:
   https://claude.ai/code/session_013aSrQtDjXkcwWMe9NYjNWd
