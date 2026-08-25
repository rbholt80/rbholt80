# Architecture

## The one idea

A system that lets a model act on your machine needs an answer to *"what is
about to happen?"* that is complete and available *before* it happens. You
cannot get that from a shell script — you cannot know what a script touches
without running it.

So NOUS does not give models a shell. It gives them a language whose every
statement is a capability request, and puts a broker between every statement and
the machine.

```
     you ──▶ intent ──▶ resolver ──▶ plan ──▶ BROKER ──▶ executor ──▶ machine
                            │                   │
                       grammar or           policy ── journal
                       a model, in           engine     (undo)
                       GLYPH
```

Everything else follows from that shape.

## The layers

### `nous-core` — the vocabulary

No third-party dependencies. Everything the daemon, shell and control tool must
agree on.

- **`cap`** — a capability is `domain.action:scope`, e.g. `fs.write:~/notes.md`.
  Each has an intrinsic **risk** from an explicit table, not a heuristic, so an
  operator can read the file and see what the system considers dangerous. An
  unknown capability is `critical`, never safe. A **protected floor** of paths
  (`/boot`, the ESP, `/etc/shadow`, SSH keys, the policy directory) can never be
  written or read, whatever policy says.
- **`policy`** — ordered rules, first match wins, default deny. Written in plain
  text so a human can audit it without tooling. An `allow` cannot lift an agent
  past its risk ceiling; it is downgraded to a confirmation instead.
- **`journal`** — append-only. Every adjudication lands here, *including
  refusals*, so an agent cannot erase evidence through the API it misused. Every
  mutation records its inverse **before** it runs.
- **`glyph`** — the intent language. See [GLYPH.md](GLYPH.md).
- **`proto` / `ipc`** — newline-delimited JSON over a unix socket, so the whole
  system can be driven from `socat` when the desktop is what is broken.

### `nousd` — the system daemon

One process owns the AI subsystems. That is what makes the guarantees hold:
exactly one policy engine, exactly one journal, and no way to reach an executor
except through the broker.

| | |
|---|---|
| **broker** | Adjudicates, executes, journals. Shows a plan in full and runs it in full. |
| **resolve** | Natural language → plan. Grammar first, model second. |
| **router** | Model backends, tried in order, with a separate *small* route. |
| **exec** | Where capabilities become effects: `fsops`, `sysops`, `media`, `curate`. |
| **index** | BM25-style file search over name, path and content head. |
| **sensorium** | Samples the machine; announces a condition once, not every tick. |
| **webui** | Serves the graphical shell on loopback, HTTP + WebSocket. |

### Why plans are approved whole

The broker never stops mid-flight to ask. A plan is shown in full, approved in
full, and then runs. Stopping halfway to ask trains people to click *yes* on a
dialogue whose context they have already lost — which is how consent becomes a
formality. The cost is that you cannot change your mind at step four; the benefit
is that when you say yes, you knew what you were saying yes to.

### Why the curator only proposes

`curate.apply` is expanded **by the broker**, not by the executor. Each move goes
through the ordinary governed path and gets its own journal entry, so nine files
tidied is nine entries and nine independent undos.

This was not the original design. The first version executed the moves inside the
executor, and the first live run moved nine files that could not be undone —
policy was never consulted per move and the journal had nothing to reverse. The
fix is architectural, not a patch: the component that *decides* is not the
component that *acts*.

### Model routing

Two routes, not one:

- **large** — intent resolution and anything you are waiting on.
- **small** — naming, sorting, classifying, summarising. Local-only by default.

Most of what an AI-native OS does all day is small. Sending that to a paid API
would be both expensive and a privacy decision nobody asked for. An `offline`
sentinel anywhere in a route stops the search there, and both `complete()` and
`has_model()` honour it — so "keep it on this machine" means exactly that.

### No model is a supported configuration

The deterministic resolver handles the shapes people actually type — opening a
folder, tidying a directory, asking what is using memory, undoing. It is fast,
private, free, and it comes *first* rather than being a fallback. Opening a
folder should not require an inference.

### Keeping itself in check

NOUS stores a lot on your behalf: a journal of everything it did, a snapshot of
every file before it changed it, a trash store so deletion is reversible,
thumbnails, screenshots. Each is the right call on its own; together they are an
unbounded copy of your disk.

So every store has a bound, and a maintenance pass prunes them on a slow timer
and at startup. One invariant governs it: **a snapshot is never removed while
the action it would undo can still be undone.** History ages out by whole files
— rotation moves a journal, never rewrites it, so the append-only property
survives — and undo degrades by losing the oldest history, never by finding a
journal entry whose backup has gone.

The one exception is deliberate: if the snapshots that are still needed exceed
their ceiling, the oldest go anyway and those actions stop being undoable.
Running out of disk is the worse failure. It is reported, not silent.

`nousctl storage` shows what is kept and what a pass would reclaim; the preview
reports exactly what the real run does, because a preview that under-reports is
worse than none — it teaches you the operation is harmless.

## The graphical shell

Served by the daemon, compiled into its binary, so a half-finished package
upgrade cannot break the desktop. It is a thin client over the capability API:
every list, play and tidy goes through `cap.invoke` and is adjudicated by the
same policy engine that governs `nsh`. **The desktop has no privileged path.**

It ships a strict Content-Security-Policy with no `unsafe-inline`, which caught a
real bug on the first render — every meter bar drew full width because inline
`style` attributes were being refused. Dynamic values go through the CSSOM, which
CSP does not restrict.

## Trust boundaries

| Boundary | Enforced by |
|---|---|
| Model → machine | GLYPH checking + the broker. A hallucinated capability fails at check time. |
| Agent → machine | Policy, with a risk ceiling and a narrower default world than the user's. |
| Anything → secrets | Protected-read paths. Credentials cannot be read by any capability. |
| Anything → boot path | Protected-write paths, plus `ReadOnlyPaths` in the systemd unit, so a broker bug is not sufficient. |
| Shell → daemon | Loopback only, owner-only socket, cross-origin requests refused. |

## Testing

308 Rust tests and 24 shell tests, and the ones that matter most are the adversarial ones: that a
protected path beats an explicit `allow`, that an undone action cannot be undone
twice, that a corrupt journal line does not poison the log, that a hung
subprocess is killed rather than wedging the daemon, that a slow event subscriber
sheds load instead of stalling, and that a duplicate file with the same size but
different contents is not treated as a copy.

Several of the most important defects in this codebase were found by *running*
it, not by testing it — the unreversible tidy-up, and a policy that hardcoded
`/home/**` and so default-denied anyone whose home was elsewhere. Both are now
regression-tested.
