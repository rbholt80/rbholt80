<div align="center">

# NOUS

**An operating system where the intelligence is part of the system, not an app running on it.**

`0.1.0` · Linux · x86-64 / ARM64 · Apache-2.0

</div>

---

## What this is

Most "AI on the desktop" is a chat window with shell access. That arrangement has
a problem you cannot fix by making the model smarter: you cannot see what it is
about to do, you cannot constrain it, and you cannot take it back.

NOUS is built the other way round. Every effect on the machine — reading a file,
moving a folder, installing a package, rendering a video — is a **capability**
that must be requested, adjudicated against **policy**, and recorded in an
append-only **journal** that knows how to reverse it. The model does not act on
your computer. It proposes a program, in a language designed so that the program
can be checked before it runs.

That single decision is what the rest of the system is built out of:

| You get | Because |
|---|---|
| You see the whole plan before anything happens | Intents compile to a capability manifest, computed statically |
| A hallucinated command is a syntax error, not an incident | The model's output language is [GLYPH](docs/GLYPH.md), which is checked against the capability registry |
| Everything is undoable | Mutations record their inverse *before* they run |
| It works with no model at all | A deterministic resolver handles what people actually type |
| Routine work is free and private | Small tasks route to a local model and never leave the machine |
| Your keys cannot be read back | Credentials sit on the capability system's protected-read list |

---

## What it looks like

The shell has no taskbar, no start menu and no overlapping windows. Those exist
to manage *applications*, and NOUS is not organised around applications — it is
organised around what you are trying to do. So the primary surface is a place to
say it, and the second surface is a record of what the system did about it.

```
❯ tidy up my downloads

  understood locally
    allow  look for things to tidy          curate.scan
    allow  work out what to move            curate.propose

  ✓ found 3 things to tidy (195.3 KB reclaimable)
      3 media files sitting in Downloads   (439.5 KB that belong in Music or Videos)
      2 identical copies of album-track.mp3 (195.3 KB wasted; the oldest copy would be kept)
      6 screenshots loose in your folders   (42 B that could gather into Pictures/Screenshots)

  ✓ proposed 9 moves affecting 634.8 KB
```

Nothing has moved yet. It proposed. You decide. And when you say yes, each of
those nine moves is journalled separately, so `nousctl undo` reverses them one at
a time — or the Ledger does it with a click.

---

## The pieces

```
nous-core     capability model · policy engine · journal · GLYPH · wire protocol
nousd         the system daemon: broker, resolver, model router, index, sensorium
nsh           a shell where language and commands share one prompt
nousctl       control and inspection, for when the desktop is what broke
NOUS Shell    the graphical desktop, served by the daemon over loopback
```

`nous-core` has **no third-party dependencies**. The system builds on an
air-gapped machine with nothing but a Rust toolchain, and there is no supply
chain between an inference result and your bootloader.

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how it fits together.

---

## GLYPH

The language the model writes in. Every statement is a capability request, which
is what makes a program answerable *before* it runs.

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

```console
$ nousctl glyph check tidy.glyph
flow tidy-downloads  — 3 actions across curate (read risk)

  what it may do
      allow  curate.scan:~/Downloads
      allow  curate.propose
    confirm  curate.apply
```

Software that predates GLYPH is reached with `use foreign`, which compiles to a
governed `shell.exec` rather than an escape hatch out of the model. Portability
comes from capabilities being abstract, with `on linux { }` / `on windows { }`
blocks for what genuinely differs. See [docs/GLYPH.md](docs/GLYPH.md).

---

## Will it run on your computer?

Almost certainly, and you probably do not need to buy anything. The OS layer is
a static daemon and a compositor; what varies is how much of the intelligence
runs locally.

| RAM | Profile | What that means |
|---|---|---|
| 4 GB | `hosted` | Desktop, files and media all fine. Intelligence comes from your API key. |
| 8 GB | `hybrid` | A small local model does the routine work privately; your key handles the rest. |
| 16 GB + 8 GB VRAM | `local` | A 7B model runs everything. No key required. |
| 32 GB+ | `workstation` | Large local model. Works with no network at all. |

Baseline: any 64-bit CPU from roughly 2012, 4 GB RAM, 20 GB disk. No GPU needed
for anything except a large local model.

```console
$ nousd --check          # inspects this machine and picks a profile
```

Full detail in [docs/HARDWARE.md](docs/HARDWARE.md).

---

## Installing

NOUS assumes dual boot rather than treating it as a special case. It installs
into free space you have already made, **adopts** the existing EFI System
Partition instead of replacing it, and runs `os-prober` so whatever was on the
machine before still appears in the boot menu.

```console
# 1. From your existing system, shrink its partition to make free space,
#    then create one partition in that space.

# 2. Boot the NOUS live image and look at the plan:
sudo nous-install --target /dev/nvme0n1p5 --user yourname

# 3. Nothing is written until you say so:
sudo nous-install --target /dev/nvme0n1p5 --user yourname --commit
```

Without `--commit` the installer prints exactly what it would do and stops. It
never resizes, deletes or formats anything you did not name.

Build the image yourself with `sudo dist/build-iso.sh`.

---

## Building from source

```console
git clone <this repository>
cd nous-os
cargo build --release        # no network needed: there are no dependencies
cargo test                   # 262 tests

./target/release/nousd &     # the daemon
./target/release/nsh         # the shell
```

Then open <http://127.0.0.1:7666> for the graphical shell, or run
`nous-session` to launch it as a desktop.

---

## Adding a model

NOUS works with none. To add one:

```console
nousctl key set anthropic     # or openai, openrouter — read from stdin
ollama pull qwen2.5:1.5b      # or a local model, for the private small route
nousctl models                # what is reachable now
```

Keys are stored owner-only at `~/.config/nous/secrets/providers.conf`, which is
on the capability system's protected-read list — no agent, flow or model can read
one back.

---

## Safety, concretely

- **Deletion is a move.** Nothing in NOUS calls `unlink` on your files; delete
  relocates into a trash store and the journal knows the way back.
- **The curator cannot delete at all.** It has no such capability. It only ever
  proposes moves.
- **A protected floor policy cannot reach past.** `/boot`, the ESP, `/etc/shadow`,
  SSH keys, the policy directory itself. An explicit `allow` does not override it.
- **Agents get a narrower world than you do**, with a risk ceiling that downgrades
  an over-broad grant to a confirmation rather than honouring it.
- **Refusals are journalled too**, so a misbehaving agent cannot erase the
  evidence through the API it misused.

```console
$ nousctl check fs.write:/boot/grub/grub.cfg
fs.write:/boot/grub/grub.cfg  deny  (critical risk)
denied — '/boot/grub/grub.cfg' is on the protected list (/boot/**) [protected-paths]
```

---

## Where it stands

This is `0.1.0`: a real system you can run, install and dual-boot, with 262
tests. It is not a finished consumer OS. Honest gaps:

- The media Studio's edit graph compiles and renders; its UI is a timeline view,
  not a full editor yet.
- Third-party agents have a defined place in the model, but the SDK is not written.
- No package repository or update channel yet — you build and install it yourself.
- Tested on x86-64 Debian-family systems. ARM64 should work; it has not been run
  on real hardware.

## Licence

Apache-2.0.
