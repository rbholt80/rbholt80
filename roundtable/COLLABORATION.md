# Two agents, one repo

Roundtable is being built by two AI agents at once — Claude (in a cloud
container) and Codex (on Robert's laptop). This file is how they stay out of
each other's way. Both should read it before starting work and update it when
ownership changes.

## The channel is git. There is no other one.

Neither agent can message the other. Claude cannot see the laptop; Codex
cannot see the container. Everything shared has to be a commit on a pushed
branch. **Work that exists only in a working directory does not exist.**

This has already cost a full cycle: both agents independently found and fixed
the same three defects (embedding model seated as a speaker, signed-out CLI
counted as ready, Codex running with inherited tool settings) because neither
could see the other's copy.

Branches:

| Branch | Owner |
|---|---|
| `claude/new-session-xif2fv` | Claude |
| `codex/roundtable-local-integration` | Codex |

Push early, push unfinished, push often. A branch that is a day behind is
worse than one with a rough commit on it.

## Who owns what, and why

The split follows a real capability difference, not a coin toss.

**Codex owns anything that needs the actual machine.** It is the only one of
the two that can see seven local models, a signed-in CLI, a real browser
session, and how any of it behaves under load. Claude can only ever simulate
those, and a simulation agreeing with itself proves nothing.

- Launchers, packaging, desktop integration
- Anything verified against real local models
- Integration bugs that only appear on the host
- Assessing the `Joey` and `discernment-engine` modules it found

**Claude owns self-contained modules provable in isolation.** It can run a
browser, stand up fake providers for any dialect, and test failure paths that
are hard to produce deliberately on a real machine.

- The engine: turn taking, roles, context windowing
- Provider adapters and their failure behaviour
- Browser-driven verification of the web front end
- Cost and token accounting

**Shared, and therefore requiring a heads-up in the commit message before
touching:** `engine.py`, `providers.py`, `config.py`.

## Rules that have earned their place

1. **Confirm a reported defect in the source before fixing it.** Both agents
   have reported things about the other's code that were true, and things
   that were about a stale copy. Cite the line.
2. **Adopt the better design, say so, move on.** The stream-failure contract
   in `providers.py` is Codex's; the round-fairness fix is Claude's. Neither
   was worth a second implementation.
3. **Don't guess at an API surface.** Flags, model ids, parameter names: check
   against `--help`, an SDK signature, or a live call. Several of this
   project's real bugs were confident recollections.
4. **A fix isn't done until something ran.** Not "should work" — output.
