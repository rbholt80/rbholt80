# Roundtable integration handoff

This branch publishes the implementation already running on the user's computer. It intentionally branches from the common original commit so both agents' changes remain visible for a three-way merge.

- Repository: `rbholt80/rbholt80`
- Common base: `7ca060f` — Add Roundtable: a multi-model conversation harness
- Local implementation: `codex/roundtable-local-integration`
- Other implementation inspected: `claude/new-session-xif2fv` at `4052c7b` (advanced from `82738c8` while this handoff was being published)
- Final reconciliation belongs to the agent reviewing this handoff. This branch has not merged, replaced, or reset the other branch.

## Preserve from this branch

- `local.py` talks directly to Ollama without requiring the OpenAI SDK. `/api/show` capabilities select completion models and exclude embedders. Every installed chat model gets its own seat.
- CLI login checks exclude signed-out Claude/Codex from automatic seating. Installed does not mean authenticated or proven healthy; live model checks are recorded separately in `VERIFIED.md`.
- CLI subprocesses have temporary working directories, bounded lifetime, concurrent stdin/stderr handling, and process-group cleanup. Claude uses safe mode with tools disabled; Codex uses a read-only sandbox and disables the principal execution and integration features.
- Adapter failures raise `ProviderError`. The engine preserves partial text, appends the error note, and explicitly records `error=True`. Do not reintroduce bracket-based error detection.
- Unique JSONL transcript names and Markdown export after completed model replies.
- Browser rounds stop after a finite number of turns and pause on errors. Refresh keeps the token in session storage and restores partial-reply state. New topics refresh discovery.
- Browser/terminal launchers, automatic reuse of a running local server, stop command, and checkout-specific desktop shortcut generation.
- Eight focused regression tests in `tests/test_local.py`.

## Changes on the other branch to reconcile

| Area | This branch | Other branch at 4052c7b | Merge consideration |
| --- | --- | --- | --- |
| Ollama capabilities | `/api/show`, completion required | `/api/show`, name-based fallback when capabilities are unavailable | Both already use capability detection. Decide explicitly whether to retain the older-server fallback. |
| Ollama transport | Native HTTP/NDJSON, no extra SDK | OpenAI-compatible adapter, requires OpenAI SDK | Preserve the working dependency-free local path unless there is a concrete reason to replace it. |
| Seat names | `Ollama-gemma3:1b`, full model tag | `Gemma3`, generated short names | Preserve stable internal identity; consider a separate display label instead of silently renaming configured seats. |
| Health checks | Claude/Codex login status during discovery | `doctor --probe` performs a live call | These are complementary. Port the probe and adapt it to exception-based failures; disclose that it invokes models. |
| CLI compatibility | Flags verified on this installed version | Flags gated on CLI help | Bring across capability checks, but do not silently drop sandbox/tool restrictions and inherit broader permissions on an unfamiliar CLI. Report incompatibility instead. |
| Probe failure detection | Provider exceptions and explicit engine flag | Now also uses exceptions; probe catches `ProviderError` | The failure-contract fix now overlaps. Preserve both implementations’ partial-output behavior and keep the native CLI adapter’s process cleanup. |

## Concurrent update: roles and weighted scheduling

Commit `4052c7b` on the other branch also adds `role`, `weight`, `moderate_every`, role-specific prompts, and an `auto` policy that selects weighted random turns when weights differ. It assigns local models below 4B parameters a panel role and weight 0.35. These changes have been inspected but are not merged into this publication branch.

- Reconcile the participant dataclass, TOML fields, CLI options, engine constructor, and browser new-topic path together; merging only the scheduler would leave incompatible configuration behavior.
- **Weighted random turns conflict with the current “Run one round” promise.** A fixed budget of N randomly weighted turns can repeat some participants and omit others. Preserve one-turn-per-participant semantics for that button, or expose a separately named weighted mode with a clear turn budget.
- Treat the 4B cutoff as a configurable hypothesis, not a demonstrated quality boundary. The short demo does not establish a reliable ranking across tasks.
- Preserve the local text-only discussion instructions when merging role-specific prompts.

## Known issues to carry forward

1. **SSE reconnect duplication is reproduced, not fixed.** `_stream()` subscribes before creating a snapshot. A message arriving in between appears both in snapshot history and the queued events. Snapshot/subscription need a consistent state boundary, event sequence IDs, and replay deduplication. The passing partial-state reconnect test does not cover this race.
2. **Host interjections need transcript revisions.** A reply generated from an older history snapshot can be committed after a new host message. Record which revision the reply saw and serialize state changes without blocking generation.
3. **New topic rediscovery replaces a custom roster.** The current browser behavior is documented, but a future merge should decide whether explicit configurations remain authoritative.
4. **Long context is limited by turn count, not per-model token budgets.** The native Ollama adapter requests an 8192-token context; 40 long turns may exceed it.
5. **No weighted turn policy was added.** The short demo is insufficient evidence for fixed model-quality rankings. Independent opening answers and targeted challenges should be evaluated before granting permanent weights.

## Validation and boundaries

Run from `roundtable/`:

```bash
python3 -m unittest discover -s tests -v
python3 -m compileall -q roundtable launch.py create_shortcuts.py
```

All nine configured local/subscription seats returned real replies during local verification: Claude, Codex, and seven Ollama chat models. A browser discussion between Claude, Codex, and Gemma completed through the HTTP/SSE server. Those checks establish connectivity, not comparative reasoning quality or complete concurrency coverage.

The original hosted API adapters and example model IDs were not live-tested here. Grok is not automatically connected; manual pasted replies are documented. No claim is made that a paid API is the only possible integration route.

The running application is outside this Git checkout. Its runtime token, PID files, logs, and conversation transcripts are excluded from this branch, as are generated shortcuts containing local absolute paths. Do not replace the user's running installation during review; integrate and validate the source first.
