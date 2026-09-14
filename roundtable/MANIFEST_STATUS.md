# Manifest Status

Reconciliation of `MASTER_MANIFEST_OF_DUTY.md` against actual repository
state. Written per the manifest's own Section 0 protocol, from inspecting
the real source tree, running the real test suite, and checking real
branch/PR state — not from the manifest's own prose. Last reconciled
2026-09-14 against `main` at `bf60d89` and open PR
[#5](https://github.com/rbholt80/rbholt80/pull/5) at `54c9c30`.

## Manifest Readback

**Product mission, in my own words**: Roundtable is meant to become an
owner-controlled AI workbench where Robert's own goal — not any one AI
vendor — is the fixed point. Multiple installed, local, and hosted models
get recruited as interchangeable specialists to discuss, research, build,
and independently verify work, with every claim backed by recorded
evidence and every consequential real-world action (spending, publishing)
requiring Robert's explicit approval.

**Five most load-bearing invariants right now**:
1. Independent review beats self-certification — a worker can never
   accept its own `finish`; enforced in code (`work.py`), not just policy.
2. Evidence beats votes — a `finish` claim needs a cited, revision-matched
   test run, not agreement between seats.
3. Autonomy is bounded, visible, interruptible, auditable — step budgets,
   a durable per-step evidence trail, Pause.
4. Verified reality outranks stale docs — this very file exists to stop
   that from silently failing (see the `CHARTER.md` discrepancy below,
   which is exactly this invariant being violated once already).
5. Self-improvement only via branch → PR → owner merge, never direct to
   `main` — followed in practice across every PR to date (#2–#5).

**Current phase**: Phase 0 (Reconcile truth) is this pass. Phase 1
(Finish current GUI) is in flight and **blocked** on Robert's real-machine
visual review of PR #5 — nothing in Phase 2+ should start before that
resolves, per the manifest's own incremental-build principle.

**Working components that must not break**: discussion engine (turn-taking,
provider adapters, per-seat context budgeting), `safety.py`'s discussion-mode
fencing, the goal-mode sandbox (bubblewrap/unshare, resource limits, path
confinement), evidence-grounded `finish` + independent-review guardrail
(including this session's fix so a premature accept retries the same seat
instead of burning the whole reviewer pool), the `diff` sandbox tool
(broken until this session, now fixed), `goal.py`, and the 69-test suite.

**Next smallest safe change**: not a new capability — get PR #5 actually
looked at on a real display by Robert and either merged or revised. It is
already built, already the literal next line item in the manifest's own
Section 35 (Phase 1), and nothing else should be layered on top of an
unverified GUI.

## Known discrepancy — flagging per Section 0, not silently resolving

`CHARTER.md` (already merged, PR #4) opens by asserting that `HANDOFF.md`
already contains a "Next Phase Implementation Directive" — evidence-based
review, dynamic team formation, model reputation, an experiment engine —
authored by Claude Code as the current authoritative technical plan.
**That directive does not exist.** `HANDOFF.md`'s entire history is one
commit (the orientation doc from PR #3). This manifest pack appears to be
that missing directive, arriving separately rather than through
`HANDOFF.md` as `CHARTER.md` assumed. Recording this explicitly rather than
quietly treating `CHARTER.md`'s framing as correct.

## Section-by-section mapping

Status legend: **IMPLEMENTED** (real, working, exercised), **PARTIAL**
(a real piece exists but falls well short of the section), **PLANNED**
(deliberately not started — nothing to preserve, nothing broken),
**BLOCKED** (a known, named gap — not merely unstarted), **OBSOLETE**
(superseded by actual design).

| § | Section | Status | Note |
|---|---|---|---|
| 1 | Product Mission | PARTIAL | Discussion + Goal mode are real; repo discovery, invention, dynamic teams, Money Lab don't exist yet. |
| 2 | Owner Sovereignty / Provider Independence | PARTIAL | Seat backoff/cooldown exists (`engine.py`); no `CAPABILITY/QUOTA/RATE_LIMIT/...` outcome taxonomy — failures are plain strings. |
| 3 | Preserve Current Foundation | IMPLEMENTED | Everything listed is genuinely present; native GUI listed too, correctly caveated as unverified in HANDOFF.md. |
| 4 | First-Class Product Modes | PARTIAL | Discussion: IMPLEMENTED. Goals: IMPLEMENTED (core loop verified once end-to-end). Build/Research/Money Lab/Projects/AI Workforce as distinct modes with their own workflow: PLANNED — `research`/`ideas`/`money` are just mode strings sharing one generic worker/reviewer loop today. |
| 5 | Desktop GUI Control Center | PARTIAL/BLOCKED | Only a "Goals" pane exists (`goal_gui.py`), itself unverified on a real display. No Discussion/Build/Research/Money Lab/Projects/AI Workforce/Evidence/Activity/Settings navigation. |
| 6 | Code & Knowledge Scout | PLANNED | `worktools.py` has raw `search`/`fetch_url`; no structured HTML→findings pipeline or provenance schema. |
| 7 | Reuse Before Build | BLOCKED | Charter rule 12 states the rule; nothing enforces or records it — no Repo Registry. |
| 8 | Code Miner | PLANNED | Not started. |
| 9 | Combination & Invention Engine | PLANNED | Not started. |
| 10 | Web Research Feeds Coding | PARTIAL | `fetch_url` is callable mid-goal; no structured research-packet format. |
| 11 | Research Packets | PLANNED | Not started. |
| 12 | Model Discovery & Recruitment | PARTIAL | `config.resolve()`/`doctor` discover already-installed/signed-in seats; no discovery beyond that, no recruitment tiers. |
| 13 | Model Capability Cards & Reputation | PLANNED | Nothing exists; seat cooldown is purely operational, not reputation. |
| 14 | Dynamic Team Formation | BLOCKED | Every discovered seat is eligible every time; `WorkManager.seats()` filters by kind and sorts, doesn't select a minimal team. |
| 15 | Provider Health/Quota/Budget Routing | PARTIAL | Cooldown-on-error exists; no formal state enum, no budget caps or escalation policy. |
| 16 | Task Decomposition & Help Requests | PLANNED | Goal mode is single-worker, no dependency graph, no concurrency, no help-request mechanism. |
| 17 | Structured Evidence | PARTIAL | Durable per-step JSON evidence exists and is cited by ID; no typed schema (trust class, hash, producer) as specified. |
| 18 | Goal-Mode Trust Boundary / Prompt Injection | BLOCKED | `safety.py` fences discussion-mode text only. Goal mode's worker/reviewer evidence has **zero** equivalent — a named, real gap, not an oversight to discover later. |
| 19 | Adaptive & Adversarial Review | PARTIAL | Current protocol is closer to FULL REVIEW only; no SIMPLE REVIEW fallback, no PASS/FAIL/UNKNOWN (just accept/revise), no adversarial "try to break it" requirement. |
| 20 | Experiment Engine & Disagreement | PLANNED | Not started; discussion mode's independent-round/challenge features are a distant relative, not this. |
| 21 | Decision Ledger / Failure Memory / Run Timeline | PARTIAL | Goal mode's `remember` tool + per-goal event list is a narrow slice; no cross-goal ledger. |
| 22 | Evidence-Grounded Why | PARTIAL | `_finish_check`'s refusal and `review.reason` are steps toward this; no dedicated query view. |
| 23 | Tool Capability Manifest | PARTIAL | This is *exactly* the bug class just fixed live (`diff` documented but unimplemented). No automated test yet asserts "every advertised tool has a dispatcher case" — the fix was manual, the class of bug isn't test-guarded. |
| 24 | Robust Protocols & Context Budgets | PARTIAL | Context trimming and JSON-tolerance exist in `work.py`; no formal separation of protocol/task/review/tool failure classes — all `ValueError` strings today. |
| 25 | Caching / State Fingerprints | PARTIAL | `WorkTools.revision()` is exactly this for one purpose (stale-test detection); no broader caching layer. |
| 26 | Specialized Reviewers & Benchmarks | PLANNED | Not started. |
| 27 | Verified Completion | IMPLEMENTED | The core claim → evidence → independent review chain is real and has completed end-to-end once, live. No PASS/FAIL/UNKNOWN triage beyond binary accept/revise. |
| 28 | Self-Profiling & Loop Detection | PLANNED | No oscillation/loop detection; step budget is the only backstop. |
| 29 | Money Lab | PLANNED | `money` mode string exists; none of the scoring/validation machinery. |
| 30 | Frontier / Invention Thinking | PLANNED | Not started. |
| 31 | Provenance & License Compliance | PLANNED | `CHARTER.md` states the rule; no tracking machinery exists. |
| 32 | Self-Improvement & Human Gates | IMPLEMENTED | Every change in this project's history has gone branch → tests → PR → owner action; no direct push to `main` has occurred. |
| 33 | Capability Ledger | IMPLEMENTED (this pass) | `CAPABILITIES.md`, created alongside this file. |
| 34 | Documentation Hierarchy | PARTIAL | `HANDOFF.md`/`CHARTER.md`/`MASTER_MANIFEST_OF_DUTY.md`/`MANIFEST_STATUS.md`/`CAPABILITIES.md` all now exist; no separate durable decision store yet. |
| 35 | Implementation Order | IN PROGRESS | Phase 0: this pass. Phase 1: in flight, blocked on Robert (PR #5). Phases 2–13: PLANNED. |
| 36 | Performance Principles | PARTIAL | Local-context defaults and seat backoff already reflect this; no explicit smallest-team or hardware-aware routing logic. |
| 37 | Testing Duties | PARTIAL | 69 tests cover sandbox/regression/weak-model-review/malformed-JSON; nothing yet for prompt fencing in goal mode, tool-registry completeness, quota failover, repo preflight, provenance, or GUI/backend separation beyond one manual Xvfb smoke test (which Section 37 itself says must not be claimed as real-machine verification — and it hasn't been). |
| 38 | Required Implementation Report | — | Format followed in this session's chat replies; not separately filed per pass. |
| 39 | Non-Goals | IMPLEMENTED (no violations found) | Spot-checked: no self-review path, no uncontrolled recursion, no auto-download of large local models, no self-push to `main`. |
| 40 | Core Principles | — | See readback above. |
| 41 | Definition of Success | PLANNED | The example prompt spans Money Lab + Repo Registry + Model recruitment + adversarial review together; none of those exist yet, let alone combined. |
| 42 | First Instruction to Next Coding AI | IMPLEMENTED (this pass) | This document is that instruction being followed. |

## Baseline recorded this pass

- `git log --oneline origin/main -1`: `bf60d89` (Merge PR #4)
- Open PR: [#5](https://github.com/rbholt80/rbholt80/pull/5) "Add a native Linux GUI for goal mode", unmerged, no review comments yet, `mergeable_state: clean`.
- `python3 -m pytest tests/ -q` on `claude/new-session-xif2fv` (`54c9c30`): **69 passed, 1 skipped, 42 subtests passed.**
- No changes made to `roundtable/`, `work.py`, `worktools.py`, `goal.py`, or `goal_gui.py` in this pass — this pass is documentation-only, per the manifest's own Phase 0 scope.
