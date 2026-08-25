# GLYPH

The intent language of NOUS OS.

## Why it exists

If a model is going to act on your computer, you need to know what it is about to
do — completely, and *before* it does it. No general-purpose language can give
you that. You cannot know what a Python script or a shell one-liner will touch
without running it.

GLYPH is not a general-purpose language, and that is the point. Every statement
is a capability request. Nothing else is expressible. So a GLYPH program can be
read statically into an exact list of everything it may do, checked against
policy, and shown to a human — all before the first step executes.

```console
$ nousctl glyph check tidy.glyph
flow tidy-downloads  — 3 actions across curate (read risk)
  Move stray media out of Downloads

  what it may do
      allow  curate.scan:~/Downloads
      allow  curate.propose
    confirm  curate.apply

  1 confirmation(s), 1 gate(s)

✓ checks out
```

## A program

```glyph
flow tidy-downloads {
  meta description "Move stray media out of Downloads"

  found = curate.scan    roots: [~/Downloads]
  plan  = curate.propose kinds: [misfiled_media, duplicate]

  gate plan.count > 0
  ask  "Move ${plan.count} files?"

  curate.apply steps: plan.steps
}
```

## The whole grammar

```
flow NAME { ... }              a program
meta KEY VALUE                 metadata (description, author)
name = domain.action args...   call a capability, bind the result
domain.action args...          call a capability, discard the result
gate EXPR                      stop unless the condition holds
ask "text"                     require a human yes before continuing
on PLATFORM { ... }            a platform-conditional block
use foreign NAME cmd: "..."    make an existing program callable
```

That is all of it. There are no loops, no functions, no arithmetic and no
variables beyond binding a step's result. Each omission is deliberate: every one
of them would make the static manifest an approximation instead of an answer.

### Values

| | |
|---|---|
| strings | `"text with ${binding.field}"` |
| numbers | `42`, `1.5`, and units: `1GB`, `500MB`, `30s`, `5m`, `2h` |
| paths | `~/Downloads`, `/etc/hosts`, `./out.mp4` |
| lists | `[duplicate, screenshots]` |
| booleans | `true`, `false` |
| references | `plan.count`, `plan.steps` |
| flags | `-i`, `--preset` (for foreign tools) |

Commas between arguments are optional. Comments run from `#` to end of line.

### Scope inference

A capability's scope comes from a conventional argument — `path`, `from`,
`target`, `name`, `unit`, `output`, `project` — so you write the path once:

```glyph
fs.write path: ~/notes.md content: "hello"
```
is the capability `fs.write:~/notes.md`.

When the scope depends on an earlier result, the checker says so and marks it
`scope_known: false`; policy is then re-evaluated at run time against the
concrete value.

## Compatibility with software that predates it

`use foreign` makes an existing binary a first-class node. It compiles to a
governed `shell.exec`, so it appears in the manifest and is policed like
everything else — it is not an escape hatch out of the model.

```glyph
flow transcode {
  use foreign handbrake cmd: "HandBrakeCLI"    on: [linux, macos]
  use foreign handbrake cmd: "HandBrakeCLI.exe" on: [windows]

  ask "Transcode the holiday clip?"
  handbrake args: [-i, ~/Videos/holiday.mkv, -o, ~/Videos/holiday.mp4, --preset, "Fast 1080p30"]
}
```

The checker verifies a binding exists for the platform you are on, and refuses
the program if it does not — so a flow that would silently do nothing on Windows
fails when you lint it on Linux.

## Portability

Capabilities are abstract; the executors beneath them are platform-specific.
`fs.move` means the same thing everywhere and is implemented differently on each.
Where behaviour genuinely differs, say so:

```glyph
flow install-player {
  on linux   { pkg.install name: mpv }
  on macos   { brew   args: [install, mpv] }
  on windows { winget args: [install, mpv] }
}
```

A plan only ever lowers the blocks for the platform it will run on, so what you
are shown is what will happen on *your* machine.

## What the checker gives you

`glyph::check` returns a `Manifest`:

- every capability the flow may exercise, with its risk
- whether each scope is known statically or only at run time
- which platform each call belongs to
- how many confirmations and gates the flow contains
- a **blast radius** summary: `"3 actions across curate, fs (elevated risk)"`
- `preflight(policy, subject)` — the policy verdict for every capability, before
  anything runs

Errors are hard failures, not warnings: an unknown capability, a reference to an
unbound name, a foreign tool with no binding for this platform. A model that
hallucinates `fs.incinerate` produces a check failure, not an incident.

## Writing it yourself

You do not have to — the shell writes GLYPH for you when it resolves an intent,
and shows you the result. But a `.glyph` file is a good way to keep a routine you
run often:

```console
$ nousctl glyph check ~/flows/weekly-tidy.glyph
$ nousctl glyph run   ~/flows/weekly-tidy.glyph
```

## Design notes

**Why not JSON or YAML?** A model emits them reliably enough, but a human cannot
read a capability manifest out of them at a glance, and the whole value here is
that a person looks at the program and understands it.

**Why no loops?** A loop makes the manifest unbounded, and an unbounded manifest
is not an answer to "what is about to happen?". Where iteration is genuinely
needed, a capability does it internally and reports what it did — `curate.apply`
takes a list of steps rather than GLYPH iterating over one.

**Why is `ask` a statement rather than a policy decision?** Because policy answers
"may this happen?" and `ask` answers "does the author of this flow think you
should look?". They are different questions, and a flow author knows things
policy does not.
