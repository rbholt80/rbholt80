# PROJECT CONTROL — START HERE

**Owner:** Robert "Joey" Holt  
**Control repository:** `rbholt80/rbholt80`  
**Rule:** This file is the cross-project source of truth. Product code remains in its own repository.

## Why this exists

ChatGPT/Codex, Claude, local ZIP checkpoints, handoff files, and GitHub branches have started carrying ideas across project boundaries. From now on:

1. One coding session works on **one canonical repository** unless the task is explicitly a cross-project harvest.
2. A chat transcript is context, **not source control**.
3. A ZIP is a checkpoint/transport copy, **not automatically canonical**.
4. GitHub is canonical for projects that have a canonical repo/branch.
5. Repo-local `PROJECT_IDENTITY.md` defines what the repo owns.
6. Old handoffs lose authority when they conflict with newer repo files, commits, tests, or accepted decisions.
7. Cross-project reuse means **harvest + adapter + provenance**, not copy/paste until two products become tangled.
8. No repository is deleted, renamed, archived, or merged just because another project borrowed ideas from it.

---

# Canonical project map

## ACTIVE PRODUCT / R&D REPOS

### 1. Alien Reality / Ailien-Evo
- Canonical repo: `rbholt80/ailien-evo`
- Canonical branch: `master`
- Confirmed unified baseline: `995e00cd5d387aa712e361d7bbbf968caab00eb4` or newer
- Owns: Rust simulation truth, procedural creatures, genetics/evolution, locomotion, ecology, persistence, living desktop experiments, Godot/Android presentation seams.
- Special offline Windows handoff: `AILIEN-EVO-OFFLINE-WINDOWS-CLAUDE-HANDOFF-2026-09-27.md`
- DO NOT: blindly merge/rewrite the Rust core from another project.

### 2. Joey
- Canonical repo: `rbholt80/Joey`
- Canonical branch: `main`
- Owns: local AI runtime/product shell, local model selection, machine awareness, installer/update/rollback patterns, licensing/productization patterns, safety/teaching patterns.
- Can donate reusable runtime/product blocks.
- DO NOT: turn Joey into the dumping ground for BrainSkatter, Alien Reality, or every experimental feature.

### 3. Splyn
- Canonical repo: `rbholt80/splyn`
- Canonical branch: `main`
- Owns: media/audio/video analysis, edit decisions, timeline/render/export pipeline.
- Reuse target: media-analysis blocks for BrainSkatter or other products.
- DO NOT: treat it as a video generator; its core strength is analysis/editing of existing media.

### 4. sheet2app
- Canonical repo: `rbholt80/sheet2app`
- Canonical branch: `main`
- Owns: spreadsheet parsing, formulas, dependency graphs, spreadsheet-definition/runtime ideas.
- Reuse target: spreadsheet/import/dependency blocks.
- DO NOT: absorb the whole BrainSkatter builder into this repo.

### 5. FourHorsemen
- Canonical repo: `rbholt80/FourHorsemen`
- Canonical branch: `main`
- Owns: deterministic world facts/state/history, perception/perspective, proposal-validation patterns, procedural/game/world experiments including Delve.
- Reuse target: world-truth/history/validation patterns for Alien Reality.
- DO NOT: directly replace Alien Reality's Rust simulation core.

---

# REASONING / CAPTURE REPOS

### 6. discernment-engine
- Canonical repo: `rbholt80/discernment-engine`
- IMPORTANT: its README states the current engine is on branch `claude/tetrahedron-ambiguity-engine-1znii2`, not `main`.
- Owns: evidence-first reasoning, ambiguity handling, reviewer veto, explainable findings, local/private engine/API patterns.
- DO NOT: confuse it with the Android capture app `DiScernment`.

### 7. DiScernment
- Canonical repo: `rbholt80/DiScernment`
- Canonical branch: `main`
- Owns: Android capture/notification/accessibility UI, on-device scam/manipulation analysis integration, encrypted per-sender learning.
- It is a product/client, not the canonical general-purpose reasoning engine.
- DO NOT: use its screen/overlay capture code as a generic dependency unless a project explicitly needs Android capture.

---

# ANCESTRY / DONOR REPO

### 8. Roundtable
- Canonical repo: `rbholt80/roundtable`
- Canonical branch: `main`
- Status: **KEEP as a donor/ancestor; it is NOT BrainSkatter.**
- Owns: multi-model orchestration, independent rounds, provider routing, usage accounting, sandbox/review concepts.
- BrainSkatter may harvest these concepts through adapters/blocks.
- DO NOT: rename the meaning of this repo in-place or treat every newer BrainSkatter decision as if it already exists here.

---

# BRAINSKATTER

### 9. BrainSkatter
- Current status: design/local checkpoints and handoffs; no separate canonical GitHub repo is listed in the current account inventory.
- `roundtable` is ancestry/donor code, not the canonical BrainSkatter product.
- Owns: visual Block Bench, Block Bag, Catalogue, LBM/Block Harvester/Factory, reuse-first assembly, block certification, one-configuration/two-control-path model.
- Search order remains:
  1. Current Bench
  2. My Block Bag
  3. Free Catalogue
  4. Paid Catalogue
  5. Approved external sources
  6. Adapt existing
  7. Modify closest block
  8. Generate missing code
- Until BrainSkatter has its own canonical repo, its master design artifacts must be treated as design documents, not as permission to modify unrelated repos.

---

# LEGACY / HOLD

### 10. getoffurass-istance
- Repo exists but is currently empty.
- Status: HOLD / legacy placeholder.
- Do not build new work here until explicitly reactivated.

### 11. getoffur-assistance
- Repo exists; current role is legacy/unclear from the control inventory.
- Status: HOLD pending deliberate inspection.
- Do not pull it into active projects merely because it may contain reusable Android patterns.

---

# Cross-project build directions

These are integrations, not permission to merge repositories:

- **Persistent Living Universe:** Alien Reality + selected FourHorsemen concepts.
- **BrainSkatter builder:** BrainSkatter + harvested Joey/Roundtable/Discernment blocks.
- **Splyn Smart Editor:** Splyn + selected Joey/Discernment capabilities.
- **Spreadsheet-to-App:** sheet2app + BrainSkatter assembly.
- **Living Desktop Creature:** Alien Reality simulation + selected Joey product-shell patterns.

---

# AI startup rule

For any coding session, the AI must do this in order:

1. Identify exactly **one target repo**.
2. Read that repo's `PROJECT_IDENTITY.md`.
3. Read repo-local `CLAUDE.md`, `AGENTS.md`, `COORDINATION.md`, decisions, or handoff files if present.
4. Inspect the current branch and latest commit before editing.
5. Run the repo's existing verification gate before changing architecture.
6. If borrowing from another repo, name the donor repo and the exact capability being harvested.
7. Prefer an adapter/block boundary instead of copying an entire subsystem.
8. Keep product-specific code in the product repo.
9. End by updating a repo-local handoff with exact files, tests, result, commit/branch, and next three tasks.

**If a chat says one thing and the current repo/accepted decision says another, the repo wins.**

---

# Handoff naming rule

Do not create random master files at the root every session.

Preferred structure:

```
docs/
  handoff/
    YYYY-MM-DD-AGENT-TOPIC.md
```

A root `HANDOFF.md` may exist only as the short pointer to the latest active handoff.

Each handoff must say:
- project/repo
- branch + commit
- purpose of session
- files changed
- tests/commands run and exact results
- known working
- not verified
- decisions made
- donor code used, if any
- next 3 concrete tasks

---

# Checkpoint / ZIP rule

A ZIP named with a date is a transport/archive checkpoint.

It becomes canonical only when:
1. its changes are deliberately reconciled into the canonical repo, and
2. the resulting Git commit/branch is recorded.

Examples:
- `alien-reality-source-2026-09-20-c.zip` is an important historical checkpoint, but current Alien Reality authority is the unified `ailien-evo` repo on `master`.
- Offline Windows work returns as a work folder + worklog; it is reviewed before being merged back.

---

# Deletion policy

Nothing gets deleted during cleanup.

Use these labels:
- ACTIVE
- DONOR
- HOLD
- HISTORICAL CHECKPOINT
- SUPERSEDED HANDOFF

Only after classification and backup should anything be archived or removed.
