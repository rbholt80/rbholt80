# Roundtable — Charter & Later-Phase Roadmap

This file is a companion to the repo's own `HANDOFF.md`, not a
replacement for it. As of this update, the repo's `HANDOFF.md` holds a
detailed "Next Phase Implementation Directive" written by Claude Code —
the current, authoritative technical plan for Goal Mode: evidence-based
review, dynamic team formation, model reputation, the experiment
engine, and more. That directive already instructs whoever's building
to keep `HANDOFF.md` current as the project's real state changes, so
treat whatever's in the repo's `HANDOFF.md` at any given moment as the
live source of truth for near-term technical sequencing — this file
doesn't try to duplicate or race it.

What this file holds instead: the business/safety charter that governs
everything Roundtable does no matter what phase it's in, and the
longer-horizon roadmap items the directive explicitly deferred, kept
here so the thinking isn't lost.

Renamed from `HANDOFF.md` to `CHARTER.md` so the two documents don't
fight over the same filename — rename it back if you'd rather keep
everything under one name.

## Where things stand

Goal Mode is real and has completed an actual end-to-end run: one seat
did bounded, sandboxed work; a different seat independently tried to
prove it wrong before Roundtable accepted the result as done. Current
focus, per the repo's directive, is making that loop more reliable,
visible, and trustworthy before anything else gets built — picking the
right seat for each job, forcing claims to be tested rather than taken
on faith, and learning which models are actually good at what.

## Charter — hard rules, not preferences

These override convenience every time. If a future change would violate
one of these, stop and flag it instead of proceeding.

1. **Everything this system builds ships closed-source and paid.** No public
   repos, no permissive open-source release of anything original built here.
2. **Dual licensing on every product.** Two tiers, always: a retail
   license (the working tool, no source, no modify/resell rights) and a
   separate, much higher-priced source license (buyer gets the code and a
   right to use it in their own product, seller keeps ownership and can
   resell it to others). Every ship checklist includes setting both prices
   and writing the source-license terms.
3. **License-compliance gate on every dependency.** Before anything is added
   to a product's build, check its license. GPL/AGPL and similar copyleft
   dependencies are flagged and blocked from shipping products unless
   explicitly reviewed and approved — they can force the product's own
   source open, which breaks rule 1. MIT/Apache/BSD-style dependencies are
   fine.
4. **Local-first for the system's own brain.** Roundtable's memory,
   knowledge base, and skills library live as plain files on the owner's
   machine, not in a vendor's cloud. Cloud models are hired help, called
   on demand — the system's knowledge doesn't live inside them and doesn't
   disappear if one becomes unavailable.
5. **No lock-in for buyers.** Products the system ships are pay-once and
   keep working without a subscription, a login, or phone-home telemetry.
   (This is about what customers receive — it doesn't conflict with rule 1;
   the product is closed-source but still fully functional offline once
   bought.)
6. **Money and publishing require a human yes.** The system can research,
   draft, price, and prepare a product or a listing completely, but it
   never charges a card, spends crypto, or publishes/posts publicly without
   the owner explicitly approving that specific action. (Echoed directly
   in the repo's directive — no conflict, just confirmation from both
   sides.)
7. **Self-improvement happens on branches.** The self-review loop may
   propose and implement changes to Roundtable's own code, but only on a
   branch with a PR for the owner to read and merge. It never pushes
   directly to main. (Also echoed directly in the repo's directive.)
8. **New spaces, not loopholes.** The system should actively look for
   niches, formats, and product categories that don't have established big
   players yet — that's where a solo operator wins. It should not pursue
   business models whose viability depends on evading a law or ToS that
   already applies. If a scouted idea's edge is "nobody's enforcing this
   yet," treat that as a red flag on the idea, not a green light.
9. **No dark-web AI integration, ever.** Not a technical limitation to work
   around — off the table. These services are overwhelmingly scams or
   malware, add nothing a properly-sourced open model doesn't already give,
   and the legal exposure isn't worth it.
10. **Vet before seating.** Any model added as a participant — local,
    cloud, or peer-to-peer — needs to come from a known, reputable source.
    No auto-adding unknown or unverified models the system happens to
    find, because seat output gets fed into other seats' prompts and some
    seats have filesystem access. New candidate models get proposed to
    the owner, not auto-enrolled.
11. **Full access to the owner's existing GitHub, all repos.** The team
    isn't scoped to the `roundtable` repo alone — it has read/write access
    to everything the owner has already built (see Context for
    continuity) and should actively mine it. All of these repos are
    private and stay that way (rule 1) — full access doesn't mean
    anything gets made public.
12. **Check before you build, every time.** Before writing new code for
    any task, search all of the owner's repos for something already built
    that covers it or gets close. Adapt or extend what exists before
    writing anything from scratch. Runs on every task, indefinitely —
    that code was already paid for in tokens and time once.

## Deferred until Goal Mode is robust

The repo's directive is explicit that these come later, not never. Kept
here so the thinking isn't lost when it's time to pick them back up:

- **MCP server for the system's own brain** — expose the knowledge base,
  skills library, scoreboard, and repo reuse-check as one MCP server so
  any MCP-capable agent (Claude Code, Grok Build, Codex, or whatever
  comes next) plugs in without custom integration work per vendor.
- **Knowledge base** — plain markdown notes Roundtable writes to itself
  after substantive discussions, re-read at the start of future sessions.
- **Skills library** — reusable how-to files for procedures the system
  works out once and shouldn't have to re-derive.
- **OpenRouter expansion** — one API key reaching hundreds of models
  beyond Claude/GPT/Grok, for whenever the seat pool needs to widen past
  what Goal Mode currently needs.
- **The money loop** — scouting income ideas (new builds, plus mining the
  owner's existing repos for anything sellable or licensable), and the
  product-outcome scoreboard, which is separate from the model-reputation
  tracking the repo's directive already covers.
- **Voice interface** — speech-to-text input; a commodity feature on all
  three vendors' tooling now, genuinely useful given the owner's
  eyesight, but additive and not urgent.
- **Local-model triage layer / current small-model picks** (Qwen3 1.7B
  or Phi-4-mini for the owner's 8 GB, no-GPU hardware) — the directive's
  own cost/hardware-aware routing work may end up covering this anyway;
  revisit once local seats are back in scope.

## Still active now — not covered by the repo's directive

- **Full GitHub access + check-before-you-build** (charter rules 11–12)
  apply the moment any seat does coding work, including inside Goal
  Mode. Worth confirming Goal Mode's worker seats actually search the
  owner's other repos (`DiScernment`, `discernment-engine`, `splyn`,
  `getoffur-assistance`) before writing new code, not just `roundtable`
  itself.
- **Baseline repo knowledge, not just reactive search.** Rule 12's
  search-before-you-build is triggered by a build task. That's not
  enough on its own — every seat should also carry at least a standing,
  lightweight summary of what exists in each of the owner's repos (a
  short digest of each repo's purpose and main pieces, refreshed as
  repos change) as part of its regular context, so it has some working
  knowledge of what's already been built even when it isn't actively
  searching. Doesn't need to be the full deferred vector index — a
  repo-by-repo summary file the knowledge-base area can hold once that
  exists, or a simple per-repo README digest in the meantime, is enough
  to start with.
- **Cross-seat injection safety** — the directive doesn't explicitly
  address a seat's raw output being used to smuggle instructions into
  another seat's prompt. This matters most exactly where the directive
  is adding new machinery: structured evidence and independent review
  both mean more seat-generated text flowing into other seats' context.
  Worth raising with whoever builds Phase C/D (structured evidence,
  adaptive review protocols) — sandboxing protects the filesystem, not
  the prompt, so it doesn't cover this on its own.
- **Repo strategy** (does a new idea get its own repo, or fold into an
  existing one) — still a live decision each time something's ready to
  ship, whenever the money loop resumes.

## Context for continuity

- GitHub account: `rbholt80`, all repos private — `roundtable`,
  `DiScernment`, `discernment-engine`, `splyn`, `getoffur-assistance`
  (the last two still not described to the team). Joey/tuxmentor still
  exists as local zips (`joey.zip`, v0.9 as of last check) — confirm
  whether it's been pushed to its own repo before assuming it's covered
  by repo access.
- `roundtable` now has real structure beyond the original browser app:
  `engine.py`, `providers.py`, `config.py`, `work.py`, `worktools.py`,
  `goal.py`, `autopilot.py`, `safety.py`, `local.py`, `cli.py`, `web.py`,
  a browser frontend, plus `README.md`, `COLLABORATION.md`,
  `CODEX_STATUS.md`, and `MERGE_NOTES.md` for history. `HANDOFF.md` is
  the live technical directive — see the top of this file.
- Related prior work still worth mining if the money loop resumes: Four
  Horsemen (competing-AI debate format, ancestor of Roundtable), the
  consensus/confidence-map app, and the accuracy-linter concept.

## Open questions — deferred alongside the money loop

- Storefront choice for shipped products, and how the dual-license
  (rule 2) shows up as one listing with tiers vs. two separate listings.
- Crypto payment path: BTCPay Server (free, self-hosted, zero fees,
  non-custodial, Bitcoin/Lightning only, needs a small always-on server)
  versus a hosted processor like NOWPayments (more coins, small fee, no
  server to maintain).
