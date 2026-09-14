# ROUNDTABLE — MASTER MANIFEST OF DUTY
## Owner-controlled multi-AI research, invention, coding, verification, and opportunity system

**Owner:** Robert
**Status:** Master product/engineering directive
**Purpose:** Define Roundtable's duties, architecture, development priorities, and non-negotiable implementation principles.

---

# 0. MANDATORY READING PROTOCOL — DO NOT SKIM

Any coding AI assigned to Roundtable MUST read this file completely before architectural changes.

Before editing:
1. Read this entire manifest beginning to end.
2. Read current `HANDOFF.md`.
3. Read `README.md`.
4. Read `COLLABORATION.md`.
5. Read `CHARTER.md`.
6. Inspect the actual source tree and current branch/PR state.
7. Run the current tests and record the baseline.
8. Map every major section here to IMPLEMENTED / PARTIAL / PLANNED / BLOCKED / OBSOLETE.
9. Create/update `MANIFEST_STATUS.md`.
10. Create/update a capability ledger.
11. Only then implement.

At session start, write a concise **Manifest Readback** into `MANIFEST_STATUS.md` containing:
- product mission in your own words;
- five most important invariants;
- current phase;
- working components that must not break;
- next smallest safe change.

This is not a request for hidden chain-of-thought. It is a compliance/readback check.

If this manifest conflicts with verified repository reality, do not silently choose. Record the conflict, preserve working behavior, and resolve the documentation discrepancy explicitly.

---

# 1. PRODUCT MISSION

Roundtable is not merely a multi-model chat room.

It is intended to become an owner-controlled AI workbench that coordinates local models, installed AI CLIs, hosted providers, discovered online models, deterministic tools, web research, repositories, code execution, sandboxed experimentation, and independent review.

Roundtable should support:
- multi-AI discussion;
- bounded autonomous Goal Mode;
- software research;
- current web research;
- repository discovery and inspection;
- code discovery;
- code invention;
- combining concepts from multiple projects;
- adapting/modifying code where licensing and permissions allow;
- writing new code;
- executing and testing code;
- adversarial review;
- evidence-driven disagreement resolution;
- business/software opportunity discovery;
- demand and competition validation;
- dynamic team formation;
- free/local/subscription/paid model routing;
- model discovery beyond installed seats;
- owner-approved local model installation;
- durable evidence, decisions, failures, outcomes, and model performance;
- a polished Linux desktop control center.

**Core concept: Roundtable owns the goal. Models are replaceable specialists.**

No individual AI provider should become Roundtable's architectural center.

---

# 2. OWNER SOVEREIGNTY AND PROVIDER INDEPENDENCE

Roundtable's own workflow, configuration, budgets, permissions, model selection, projects, and product behavior are controlled by the owner.

A model/provider is a capability provider, not Roundtable's governor.

Provider-specific limitations must not automatically become universal Roundtable limitations.

Operational limitations that should normally trigger routing, decomposition, substitution, experimentation, or another technical approach include:
- quota exhaustion;
- rate limits;
- subscription usage limits;
- provider outages;
- model size/capability;
- insufficient context;
- poor structured output;
- missing browsing;
- missing tool calling;
- missing modality;
- high latency;
- excessive cost;
- local hardware incompatibility.

Classify seat outcomes:
CAPABILITY / QUOTA / RATE_LIMIT / CONTEXT / TOOL / AUTH / OFFLINE / COST / PROVIDER_POLICY / MALFORMED_OUTPUT / UNKNOWN.

A seat's inability/refusal must be visible in the run history.

Roundtable may legitimately exceed the capability of any individual model through decomposition, synthesis, local tools, deterministic software, independent specialists, and experiments. It must not be deliberately engineered to defeat another provider's safety controls by splitting a prohibited operation across models. Provider limitations remain local to that provider; Roundtable can pursue other legitimate technical approaches without making circumvention itself the design goal.

---

# 3. PRESERVE THE CURRENT WORKING FOUNDATION

The repository's verified state is authoritative.

Based on the current project status, preserve:
- Discussion Mode;
- local Ollama seats;
- Claude/Codex CLI seats;
- hosted/API provider architecture;
- browser/terminal discussion interfaces;
- Goal Mode;
- OS-level sandbox/path confinement/resource limits;
- worker execution;
- evidence-grounded finish;
- separate independent reviewer;
- patch generation;
- Discussion Mode prompt fencing;
- current tests;
- native Linux Goal GUI work.

Do not rewrite working systems merely to fit this manifest.

Real bugs/lessons that must not regress:
- documented tools must actually exist;
- weak models may append commentary after valid JSON;
- reviewer retry behavior must not burn the pool incorrectly;
- context-budget lockout;
- stale round-robin/cooldown indexes;
- SSE duplicate-turn races;
- local-model OOM from oversized defaults;
- fabricated speaker turns.

---

# 4. FIRST-CLASS PRODUCT MODES

## Discussion
Multi-AI brainstorming, debate, challenges, architecture, planning, and draft/review.

## Goals
Bounded autonomous work:
goal → project → plan → worker → sandbox → evidence → independent reviewer → revision → artifacts → outcome.

## Build
UNDERSTAND → REUSE PREFLIGHT → WEB/CODE RESEARCH → DESIGN → BUILD → RUN → TEST → BREAK → IMPROVE → REVIEW → DELIVER.

## Research
QUESTION → SEARCH → SOURCES → CLAIM EXTRACTION → CROSS-CHECK → CONTRADICTIONS → ANALYSIS → CHALLENGE → EVIDENCE-GROUNDED RESULT.

## Money Lab
SCOUT → FIND PAIN/DEMAND → VALIDATE → COMPETITION → ECONOMICS → OWNER-CODE REUSE → CHALLENGE → RANK → PROTOTYPE → TEST → SCORE → KEEP/IMPROVE/KILL.

## Projects
Persistent repository/workspace context: branches, architecture, decisions, research, evidence, tests, runs, limitations, sources.

## AI Workforce
Installed local models, CLIs, APIs, discovered candidates, temporary seats, capability cards, health, quotas, costs, latency, benchmark/reputation.

## Evidence / Activity
Durable record of what actually happened.

Consequential spending, financial transactions, public publishing, and similar external actions remain human-gated under the project charter.

---

# 5. DESKTOP GUI — A CONTROL CENTER

The GUI should evolve into a coherent native Linux workbench, not merely a chat window.

Do not throw away a working thin Goal GUI solely for visual purity. Validate it on a real display, merge working behavior, then evolve incrementally.

Long-term navigation:
- Discussion
- Goals
- Build
- Research
- Money Lab
- Projects
- AI Workforce
- Evidence
- Activity
- Settings

Goal/Build views should expose:
- goal/project;
- phase/task graph;
- worker/reviewer;
- active models;
- research activity;
- tools;
- changed files;
- evidence IDs;
- tests;
- disagreements;
- experiments;
- step and money budgets;
- provider usage;
- elapsed time;
- artifacts;
- verdict.

Controls should include, where safe:
Pause / Resume / Stop / Approve / Reject / Ask Why / Inspect Evidence / Change Budget / Replace Worker / Replace Reviewer / Research Deeper / Find Better Way / Invent / Build Prototype.

Do not expose hidden chain-of-thought. Expose observable actions, concise rationales, evidence, decisions, experiments, and outcomes.

---

# 6. CODE & KNOWLEDGE SCOUT

Roundtable must search beyond installed knowledge when tasks require it.

Potential permitted sources:
- GitHub/GitLab/Codeberg/public repositories;
- package registries;
- official/API documentation;
- standards/specifications;
- issue trackers/discussions;
- technical Q&A;
- blogs/articles;
- release notes;
- ordinary HTML pages;
- public examples/snippets;
- authorized private owner repositories.

HTML pipeline:
HTML → meaningful text → code blocks/commands/links/API examples/version info → provenance → structured findings.

Do not blindly copy discovered code. Determine purpose, technique, language, version, license, compatibility, whether source reuse is needed, and whether concept-only reimplementation is better.

---

# 7. REUSE BEFORE BUILD

Turn the charter's "check before you build" rule into machinery.

Create a Repo Registry/capability index tracking:
- path;
- purpose;
- languages;
- important modules;
- capabilities;
- license/provenance;
- last indexed commit;
- search terms;
- compatibility.

Before substantial coding:
GOAL → determine capabilities → search owner repos → inspect candidates → record reuse evidence → reuse/adapt/build.

Do not wait for a giant knowledge base.

---

# 8. CODE MINER

The Scout finds material; the Miner judges usefulness.

Structured component card:
- source/project/commit/version;
- purpose/language/technique;
- license;
- compatibility;
- useful concepts;
- reusable source;
- dependencies;
- risks;
- recommendation.

Recommendation states:
USE DIRECTLY / ADAPT / REIMPLEMENT CONCEPT / REFERENCE ONLY / INCOMPATIBLE / LICENSE REVIEW REQUIRED / IGNORE.

---

# 9. COMBINATION & INVENTION ENGINE

Roundtable should combine useful ideas across sources.

Example:
A dependency graph + B caching + C model router + owner's evidence ledger → new integrated architecture.

`Invent` should solicit initially independent approaches:
- conventional best;
- simplest;
- radically different;
- newly possible using recent technology;
- maximum reuse of owner code.

Then compare/synthesize.

`Find Better Way` challenges current code by searching alternatives, owner repos, recent technologies, simpler architecture, and experiments.

---

# 10. WEB RESEARCH MUST FEED CODING

A coding worker may request research for:
- current API behavior;
- library docs;
- examples;
- releases;
- errors;
- known bugs;
- standards;
- competitors;
- alternative libraries.

Research returns as structured evidence, not loose prose.

External factual claims used for build decisions should retain provenance.

---

# 11. RESEARCH PACKETS

Reusable packet fields:
ID / question / date / scope / sources / claims / contradictions / code references / versions / licenses / conclusions / confidence / freshness.

Freshness:
CURRENT / POSSIBLY_STALE / REFRESH_REQUIRED.

---

# 12. MODEL DISCOVERY & RECRUITMENT

The workforce is not limited to installed models.

Build a Model Scout capable of discovering candidate models/services from permitted public catalogs/sources.

Evaluate:
- provider/model;
- local/remote;
- free/paid/subscription;
- current price/free allowance;
- context;
- tools;
- structured output;
- coding/research/multimodal ability;
- latency;
- license;
- local hardware requirements;
- availability;
- Roundtable benchmark history.

Recruitment:
1. REMOTE TEMPORARY — one task/run.
2. REMOTE PERSISTENT — configured seat.
3. LOCAL INSTALL — owner-policy-approved compatible model.

Before large local downloads inspect RAM, VRAM, disk, architecture/runtime, and estimated requirements.

New candidate models remain subject to owner approval/vetting rules.

---

# 13. MODEL CAPABILITY CARDS & EARNED REPUTATION

Track coding, debugging, review, security, architecture, research, planning, adversarial testing, JSON/tool reliability, context, speed, cost, resource footprint.

Use categorical ratings until enough observations justify numbers.

Reputation comes from outcomes:
- tests;
- independent review;
- bugs caught;
- protocol reliability;
- later regressions.

Penalize fabricated APIs/tests, malformed protocols, failed patches, missed regressions, repeated failed behavior.

Reputation is domain-specific and models do not grade themselves.

---

# 14. DYNAMIC TEAM FORMATION

Do not seat every AI automatically.

Select the smallest capable team using:
task / capability / reputation / availability / quota / latency / privacy / hardware / budget / protocol compatibility.

The GUI should explain why seats were selected.

---

# 15. PROVIDER HEALTH, QUOTA & BUDGET ROUTING

Provider states:
READY / DEGRADED / RATE_LIMITED / QUOTA_LOW / QUOTA_EXHAUSTED / AUTH_FAILED / OFFLINE / MODEL_MISSING / COOLDOWN.

Distinguish:
- local/free;
- free provider tier;
- subscription;
- metered API.

Owner controls:
- prefer free/local;
- per-goal paid max;
- daily max;
- monthly max;
- paid escalation ASK/AUTO/NEVER.

On operational limits, route to another qualified seat rather than repeatedly failing.

---

# 16. TASK DECOMPOSITION & HELP REQUESTS

Large goals become bounded dependency-aware graphs:
understand → reuse → research → design → implement → targeted test → regression → adversarial review → integration → artifacts.

Independent nodes may run concurrently. Dependent nodes wait.

No uncontrolled recursive spawning.

Graphs remain bounded, visible, interruptible, auditable, budgeted.

Workers may request specialist help: research, stronger coding, architecture, docs, experiment, security review, long-context analysis. Roundtable owns fulfillment.

---

# 17. STRUCTURED EVIDENCE

Evidence is first-class.

Suggested fields:
ID / type / producer / timestamp / action/command / exit code / source / artifact / hash / summary / trust class.

Types:
command result / stdout-stderr / diff / hash / tests / benchmark / static analysis / web source / repo source / browser observation / experiment / real-machine observation / reviewer observation.

"I tested it" is not equivalent to recorded test evidence.

Reviewer verdicts should cite evidence IDs.

---

# 18. GOAL-MODE TRUST BOUNDARY / PROMPT INJECTION

Goal Mode must protect reviewers/orchestrators from untrusted repository text, web pages, tool output, and worker evidence.

Filesystem sandboxing does not protect prompts.

Untrusted material is data, not authority.

Attach provenance/trust labels and separate:
- Roundtable instructions;
- tool protocol;
- evidence;
- untrusted content.

Instruction-shaped text found inside source material must not silently become orchestration instructions.

---

# 19. ADAPTIVE & ADVERSARIAL REVIEW

Do not force weak models through protocols they repeatedly cannot execute.

FULL REVIEW:
interactive inspection/tools/experiments/verdict.

SIMPLE REVIEW:
Roundtable preassembles diff/tests/requirements; reviewer returns constrained verdict.

PASS / FAIL / UNKNOWN.

UNKNOWN is valid.

High-risk/unresolved work may escalate to stronger reviewer.

Reviewers try to break work:
malformed input / empty input / boundaries / regression / invalid config / rollback / unexpected type / concurrency / original bug reproduction.

Worker cannot verify itself.

---

# 20. EXPERIMENT ENGINE & DISAGREEMENT

Do not merely vote.

Represent shared facts, hypotheses, disputed proposition, missing evidence.

Ask: "What is the cheapest safe experiment that distinguishes these hypotheses?"

PROPOSE → DISAGREE → DISPUTED FACT → SAFE TEST → SANDBOX → OBSERVE → UPDATE → CONTINUE.

Prefer deterministic experiments over unsupported opinions.

For important questions, support blind independent commitments before models see peers' answers.

Classify independent agreement/disagreement/partial agreement/same conclusion-different reasoning.

---

# 21. DECISION LEDGER, FAILURE MEMORY, RUN TIMELINE

Decision records:
ID / topic / decision / rationale / evidence / rejected alternatives / date / status / affected components.

Failure records:
task / attempt / reason / observed failure / evidence / replacement / result / lesson.

Run timeline records:
goal created / seat selected / research / source / reuse check / file change / test / finish / reviewer / experiment / verdict / artifact.

Current verified code outranks stale memory.

---

# 22. EVIDENCE-GROUNDED WHY

"Why did you accept this?" must be answered from actual evidence/decisions, not invented after the fact.

---

# 23. TOOL CAPABILITY MANIFEST

Machine-readable tool registry:
name / availability / risk / sandbox state / allowed inputs / paths / side effects / evidence / failure behavior.

Prompts derive from the real registry.

Tests must catch advertised-but-unimplemented tools.

---

# 24. ROBUST PROTOCOLS & CONTEXT BUDGETS

Separate protocol parse failure / task failure / review failure / tool failure.

Use explicit schemas and validation. Tolerant extraction is acceptable only when meaning remains unambiguous.

Each model gets context appropriate to its actual capacity.

Prefer summaries, evidence references, targeted excerpts, structured state.

Do not repeatedly feed impossible prompts.

---

# 25. CACHING / STATE FINGERPRINTS

Fingerprint stable repo/file/test/evidence/task/model state to avoid repeated work.

Invalidate when underlying state changes.

Never allow model/config-specific cache poisoning.

---

# 26. SPECIALIZED REVIEWERS & BENCHMARKS

Potential review specialties:
correctness / security / regression / requirements / performance / simplicity.

Risk determines depth.

Permanent benchmark tasks:
coding / bug repair / tests / review / adversarial review / architecture / research / JSON / tools / evidence / disagreement / experiment design.

Record success, tests, review, retries, malformed output, latency, resources, cost.

Use benchmarks for routing, not vanity.

---

# 27. VERIFIED COMPLETION

Worker `finish` is a claim.

WORKER CLAIM → REQUIRED EVIDENCE → INDEPENDENT REVIEW → ADVERSARIAL TEST IF WARRANTED → PASS/FAIL/UNKNOWN.

Prefer "cannot verify yet" over false success.

---

# 28. SELF-PROFILING & LOOP DETECTION

Measure seat latency, failures, malformed output, retries, tool/context failures, reviewer failures, successful goals, steps, rejection rate, experiments, provider use, cost.

Detect repeated commands/tools/patch oscillation/reviewer complaints/hypotheses without new evidence.

Replan, switch worker, escalate, or ask owner rather than blindly spending steps.

---

# 29. MONEY LAB

Search for actual opportunity signals:
- repeated complaints;
- expensive manual work;
- bad software;
- underserved professions;
- new technology creating needs;
- spreadsheet-heavy workflows;
- pricing gaps;
- newly practical local-AI workflows;
- expensive services software could reduce;
- recurring issue/forum pain;
- fragmented tools that can be combined.

Score candidates using:
demand / competition / build difficulty / prototype time / operating cost / plausible price / support burden / owner-code reuse / distribution difficulty / confidence.

Challenge assumptions:
- What became possible recently?
- What assumption may no longer be true?
- What two technologies have not been connected usefully?
- What workflow is still painfully manual?
- What can local AI now do cheaply?

Never present speculative scores as guaranteed revenue.

---

# 30. FRONTIER / INVENTION THINKING

Roundtable should challenge soft constraints.

When encountering "can't":
classify constraint → identify source → hard vs soft → search alternatives → test assumption → local implementation? → provider substitution? → architecture change? → business-model change? → experiment.

A model saying "impossible" without evidence should not automatically end investigation. Mark UNPROVEN and challenge where appropriate.

---

# 31. PROVENANCE & LICENSE COMPLIANCE

Track external code/material from discovery:
source URL / repo / commit/version / author/project / license / retrieval date / exact code reused? / concept only? / modified? / destination.

Classify:
IDEA ONLY / PERMISSIVE REUSE / ATTRIBUTION REQUIRED / LICENSE REVIEW REQUIRED / UNKNOWN LICENSE / BLOCKED PENDING REVIEW.

Do not wait until release to discover provenance problems.

Follow CHARTER commercial/license rules.

---

# 32. SELF-IMPROVEMENT & HUMAN GATES

Roundtable may improve Roundtable only through:
branch → implementation → tests → independent review → PR → owner/normal merge.

Never autonomously push self-modifications directly to `main`.

Consequential external actions defined by CHARTER, including spending and public publishing, remain owner-approved.

---

# 33. CAPABILITY LEDGER — REALITY MUST BE EXPLICIT

Maintain `CAPABILITIES.md` or machine-readable equivalent.

Maturity:
PLANNED / BUILT / TESTED / VERIFIED_REAL_MACHINE / DEGRADED / DEPRECATED.

Only verified capabilities should be described as working in authoritative docs.

This exists specifically to prevent planning prose from outrunning reality.

---

# 34. DOCUMENTATION HIERARCHY

`HANDOFF.md`: concise current reality and next steps.
`CHARTER.md`: ownership/business/safety principles.
`MASTER_MANIFEST_OF_DUTY.md`: detailed product/engineering directive.
`MANIFEST_STATUS.md`: mapping of manifest to actual implementation.
`CAPABILITIES.md`: capability maturity ledger.
Decision store: durable architecture decisions.

Do not turn HANDOFF into a giant manifesto.

---

# 35. IMPLEMENTATION ORDER

## Phase 0 — Reconcile truth
Read docs, inspect source, run tests, reconcile discrepancies, create/update status and capability ledger.

## Phase 1 — Finish current GUI
Real-machine Goal GUI visual test, fix usability, preserve WorkManager boundary, merge after owner approval.

## Phase 2 — Trust boundary
Fence untrusted Goal evidence/tool/repo/web content; provenance/trust labels; tests.

## Phase 3 — Structured evidence
Schema, IDs, storage, tool evidence, reviewer references, Why view.

## Phase 4 — Adaptive review
Full/simple protocols, PASS/FAIL/UNKNOWN, adversarial review, capability matching.

## Phase 5 — Repo Registry/reuse preflight
Index owner repos, manifests, enforce check-before-build.

## Phase 6 — Web/Code Scout
Web research, HTML extraction, public repo/docs/code discovery, provenance, Research Packets.

## Phase 7 — Model workforce
Capability cards, health, quota/cost, benchmarks, dynamic routing.

## Phase 8 — Model Scout/recruitment
Discover candidates, compatibility, benchmark, temporary/persistent remote seats, owner-approved local install.

## Phase 9 — Task decomposition
Dependency plans, bounded subtasks, help requests, safe concurrency.

## Phase 10 — Experiment/disagreement
Hypotheses, disputes, safe experiments, evidence updates, independent consensus.

## Phase 11 — Build workbench
Code/Research/Sources/Tests/Review/Evidence/Files/Terminal plus Find Better Way and Invent.

## Phase 12 — Money Lab
Opportunity scout, validation, competition, economics, reuse, challenge, prototype, outcome scoreboard.

## Phase 13 — Unified polish
Coherent desktop shell, accessibility, keyboard navigation, dark/light, responsive panes, robust states, dashboards.

---

# 36. PERFORMANCE PRINCIPLES

- deterministic tools before model opinions when possible;
- local/free models where adequate;
- smallest capable team;
- targeted context;
- cache stable research/state with invalidation;
- stream large inputs;
- do not wake every seat;
- hardware-check local downloads;
- measure before optimizing.

---

# 37. TESTING DUTIES

Every phase adds tests.

Include:
current regressions / sandbox / path confinement / evidence immutability / reviewer independence / weak-model review / malformed JSON / prompt fencing / tool registry / quota failover / routing / repo preflight / provenance / research source handling / experiment safety / budgets / loops / GUI-backend separation / pause-stop safety.

Do not claim real-machine verification from Xvfb/mock tests.

---

# 38. REQUIRED IMPLEMENTATION REPORT

After each meaningful pass report:
1. What changed.
2. Files changed.
3. Tests run.
4. Exact results.
5. New capability maturity.
6. Limitations.
7. Real-machine tests Robert must run.
8. License/provenance concerns.
9. Security/trust concerns.
10. Performance impact.
11. Next smallest task.

Update HANDOFF when reality changes, plus MANIFEST_STATUS and capability ledger.

---

# 39. NON-GOALS

Do not:
- rewrite working Roundtable;
- replace backend solely for elegance;
- seat every model every time;
- use retries as universal solution;
- confuse votes with evidence;
- treat prose claims as proof;
- allow self-review;
- let untrusted web/repo content become orchestration instructions;
- auto-download huge models without checks;
- hide costs;
- claim planned features exist;
- create uncontrolled recursive agents;
- self-push to main;
- build a giant knowledge system before lightweight indexes prove necessary.

---

# 40. CORE PRINCIPLES

1. Roundtable owns the goal.
2. Models are replaceable specialists.
3. No provider defines the entire system's capability ceiling.
4. Evidence beats votes.
5. Independent review beats self-certification.
6. Disagreement requests evidence.
7. Experiments beat unsupported opinions.
8. Use the smallest capable team.
9. Use free/local intelligently where adequate.
10. Paid capability is budget-aware.
11. Search owner code before rebuilding.
12. Search current public knowledge when needed.
13. Track provenance during discovery.
14. UNKNOWN beats fake certainty.
15. Autonomy is bounded, visible, interruptible, auditable.
16. Verified reality outranks stale docs.
17. One model's operational limitation is not automatically Roundtable's limitation.
18. The owner controls Roundtable's own policy/configuration.
19. Planning docs must not outrun implementation.
20. Build incrementally and preserve working code.

---

# 41. DEFINITION OF SUCCESS

A mature Roundtable should accept:

"Find a real software opportunity. Research current demand and competitors. Search my existing repositories first. Keep paid AI usage under my budget. Recruit free/local models where useful. Find current code and documentation. Design something better than the obvious implementation. Build a sandboxed prototype. Test it. Have another model try to break it. If models disagree, run an experiment. Show me the evidence and artifacts."

The GUI should expose:
plan / research / sources / code discoveries / reuse / recruited models / quotas / costs / changed files / tests / evidence / reviewer challenges / experiments / verdict / artifacts.

Success is not:
"Several AIs agreed."

Success is:
"Roundtable researched the problem, found and evaluated existing work, selected appropriate specialists, built the candidate, executed tests, independently challenged it, recorded evidence, and can show why the result is ready."

---

# 42. FIRST INSTRUCTION TO THE NEXT CODING AI

Do not answer this manifest with another architecture essay.

Read it completely.
Read the current repo docs.
Inspect actual source.
Run tests.
Create/update `MANIFEST_STATUS.md` and capability ledger.
Confirm current Goal GUI/branch state.
Then implement the smallest safe next phase according to ACTUAL repository reality.

Use small patches and tests.
Preserve working behavior.
When unsure whether something exists, inspect and test rather than infer from planning prose.

**Roundtable is being built into an owner-controlled AI software laboratory and work manager: it finds knowledge, finds code, finds specialists, combines ideas, builds, tests, challenges, verifies, remembers outcomes, and keeps the process visible.**
