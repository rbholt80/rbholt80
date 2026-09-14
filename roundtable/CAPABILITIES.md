# Capability Ledger

Per `MASTER_MANIFEST_OF_DUTY.md` Section 33. Maturity levels: **PLANNED**,
**BUILT** (code exists, not exercised), **TESTED** (covered by the
automated suite and/or a scripted smoke test), **VERIFIED_REAL_MACHINE**
(observed working on Robert's actual hardware, not a mock/Xvfb run),
**DEGRADED**, **DEPRECATED**.

Section 37 of the manifest is explicit: *do not claim real-machine
verification from Xvfb/mock tests.* That rule is why `goal_gui.py` below
is marked TESTED, not VERIFIED_REAL_MACHINE, despite being fully exercised
under Xvfb in this session.

## Discussion mode

| Capability | Maturity | Basis |
|---|---|---|
| Turn-taking (round-robin / weighted) | VERIFIED_REAL_MACHINE | Live sessions across 9 real seats on Robert's machine. |
| Provider: `cli` (Claude CLI, Codex CLI) | VERIFIED_REAL_MACHINE | Real signed-in CLI turns observed. |
| Provider: `ollama` | VERIFIED_REAL_MACHINE | 7 real local models answered live. |
| Provider: `anthropic`/`openai`-compatible/`gemini` (hosted API) | BUILT | Per `MERGE_NOTES.md`: "not live-tested here." No hosted API key has been exercised in this project's real-machine verification to date. |
| Provider: `mock` | TESTED | Test-only by design; not meant to be verified real-machine. |
| Browser front end | VERIFIED_REAL_MACHINE | Reload, live streaming, exact-quote challenge all observed live. |
| Terminal front end | VERIFIED_REAL_MACHINE | Used throughout development. |
| Per-seat context budgeting | VERIFIED_REAL_MACHINE | The 8192→2048 OOM fix and the challenge-budget lockout fix were both found and confirmed via live failures. |
| Seat backoff/cooldown | VERIFIED_REAL_MACHINE | Motivated by an observed 80-consecutive-failure live incident; behavior unit-tested. |
| `safety.py` injection fencing | VERIFIED_REAL_MACHINE | Verified against a real Claude CLI end to end per `HANDOFF.md`. |
| Auto draft/independent-review loop (`autopilot.py`) | VERIFIED_REAL_MACHINE | Real Claude + Codex draft/review cycles observed per `CODEX_STATUS.md`. |

## Goal mode

| Capability | Maturity | Basis |
|---|---|---|
| Sandbox (bubblewrap primary, `unshare --net` fallback, resource limits, path confinement) | VERIFIED_REAL_MACHINE | Network isolation, `/dev`/`/proc` bind, `ulimit`/`prlimit` limits all confirmed via direct inspection on Robert's real machine. |
| Fails safe with no sandbox available | TESTED | Code path confirmed by inspection (`worktools.py` `_run`); refuses execution rather than running unsandboxed. Not yet observed on a machine actually lacking both `bwrap` and `unshare`. |
| Evidence-grounded `finish` check | VERIFIED_REAL_MACHINE | Observed correctly refusing an unsubstantiated `finish` claim live. |
| Independent-reviewer requirement (no self-review) | VERIFIED_REAL_MACHINE | Structurally enforced in code; observed live. |
| Reviewer-retry-not-rotation fix (this session) | VERIFIED_REAL_MACHINE | Watched live: the same seat (Gemma2) got repeated attempts instead of being rotated away, eventually completing inspection. |
| `diff` sandbox tool | VERIFIED_REAL_MACHINE | Broken (`unknown tool: 'diff'`) until this session; fixed, then used successfully in the run that reached `ready`. |
| JSON trailing-commentary tolerance (`raw_decode`) | VERIFIED_REAL_MACHINE | Confirmed fixing the exact live failure text (`Extra data: line 4 column 1`). |
| Full loop: worker → sandboxed run → `finish` → independent review → `ready` + patch | VERIFIED_REAL_MACHINE | Completed exactly once, live, after the two fixes above landed. Not yet routine — see Section 4/19 gaps in `MANIFEST_STATUS.md`. |
| `goal.py` (terminal driver) | VERIFIED_REAL_MACHINE | Used throughout to drive every real run to date. |
| `goal_gui.py` (native Tkinter GUI) | TESTED, not VERIFIED_REAL_MACHINE | Constructed and driven through a full `create → start → live-updating log` cycle under Xvfb with the mock provider; dialogs and viewers open/close cleanly. **No human has looked at it.** Awaiting Robert's real-display review on PR #5. |
| Goal-mode prompt-injection fencing | PLANNED | Does not exist. Named gap — see `MANIFEST_STATUS.md` §18. |
| Structured evidence schema (typed, hashed, trust-classed) | PARTIAL/BUILT | Evidence files exist and are cited by ID; fields are `id`/`kind`/`at`/free-form data, not the full schema in manifest §17. |
| Repo Registry / reuse preflight | PLANNED | Does not exist. |
| Model capability cards / reputation | PLANNED | Does not exist. |
| Dynamic team formation (smallest capable team) | PLANNED | `WorkManager.seats()` returns every eligible seat, always. |
| Adaptive review (SIMPLE/FULL split, PASS/FAIL/UNKNOWN) | PLANNED | Current protocol is binary accept/revise with no simplified path for weak models. |
| Task decomposition graph | PLANNED | Goal mode is strictly single-worker, single-thread-of-execution. |
| Experiment engine | PLANNED | Does not exist. |
| Money Lab scoring/validation | PLANNED | `money` is a mode string only. |

## Governance / process

| Capability | Maturity | Basis |
|---|---|---|
| Branch → PR → owner-merge workflow | VERIFIED_REAL_MACHINE | Every merge to `main` (#2–#4) followed this; #5 is correctly held open pending owner review. |
| Test suite | VERIFIED_REAL_MACHINE | `69 passed, 1 skipped, 42 subtests passed` as of `54c9c30`. |
| `HANDOFF.md` / `CHARTER.md` / `MASTER_MANIFEST_OF_DUTY.md` / `MANIFEST_STATUS.md` / `CAPABILITIES.md` | BUILT | All exist as of this pass. Living docs — accuracy depends on being updated as reality changes, per manifest §38. |
| Decision store (durable architecture decisions, manifest §34) | PLANNED | Does not exist as a separate artifact; decisions currently live only in commit messages and PR descriptions. |
