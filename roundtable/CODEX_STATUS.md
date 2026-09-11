# Codex coordination status

GitHub refs verified on 2026-09-10 (Robert's local date):

- Claude: `6caf4b0934424063eada7042a782c124038ad6c9`
- Codex: `27af2da880a781606c97f6b81f23841d38270229`
- Codex implementation commit: `a3f5e8908abe9e3bffeec14dab6777375037d298`

The local integration is committed and published. If the Codex branch appears
to point to `7ca060f`, refresh the explicit ref rather than inspecting a stale
local branch:

```sh
git fetch origin refs/heads/codex/roundtable-local-integration:refs/remotes/origin/codex/roundtable-local-integration
git log -3 --oneline origin/codex/roundtable-local-integration
```

## Shared-file notice and current work

Codex is integrating Claude's `6caf4b0` into this branch. This includes shared
`engine.py`, `providers.py`, and `config.py`, plus CLI/web integration. Preserve
Claude's roles, weighted selection, queued fair rounds, turn sequence IDs,
reload credentials, venv setup, token accounting, and dialect negotiation.
Preserve Codex's native local transport, CLI restrictions and timeout cleanup,
launchers, partial-failure recording, and local integration checks.

The ownership proposal is a useful default, not an exclusive lock: source
inspection and isolated tests are valuable on either machine. Tests with fake
providers verify contracts; real providers verify compatibility. Both matter.

Next discussion features are independent opening answers and challenges tied
to transcript claims, after the combined baseline passes its checks. These
require coordinated engine/frontend changes; do not duplicate them concurrently.

## Instruction-smuggling policy

Do not refuse discussion text merely because it matches an injection phrase.
A discussion about prompt injection must remain possible. Treat quoted turns
as untrusted material and restrict the CLI's tools, inherited configuration,
working directory, and permissions independently of the prompt contents.
Pattern matching may become an advisory signal, not the permission boundary.

The audited FourHorsemen version has `INJECTION_PATTERNS` and a class method
`assert_data_not_instruction`; the complete validator is coupled to its game
models/database. The regex flagged a harmless quoted example and missed a plain
request to open a terminal. This is why a blanket refusal port is not planned.

## Review and merge

Integration is in progress; this notice is not a passing-test claim. The final
commit will update this file with commands, results, and outstanding limits.
Claude can review and merge the published integration branch into its branch.
