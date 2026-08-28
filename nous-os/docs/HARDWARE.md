# What you need to run NOUS

Short answer: **if your computer can run a modern browser, it can run NOUS.**
You almost certainly do not need to buy anything.

The OS layer itself is small. `nousd` is a static binary with no dependencies
and a footprint measured in megabytes; the desktop is a browser engine used as a
compositor, which is the same bet GNOME Shell makes with JavaScript and Windows
makes with its own engine. What actually varies between machines is **how much
of the intelligence runs locally**, and that is a spectrum rather than a
requirement.

## The floor

| | Minimum | Comfortable |
|---|---|---|
| CPU | 64-bit x86-64 or ARM64, 2 cores (~2012 onward) | 4 cores |
| RAM | 4 GB | 8 GB |
| Disk | 20 GB free | 40 GB, SSD |
| GPU | none — integrated graphics is fine | none, unless running a large local model |
| Firmware | UEFI or BIOS | UEFI, for painless dual boot |

Below 4 GB the daemon and `nsh` still run; the graphical shell will not be
comfortable. `nousd --check` says so plainly rather than letting you find out.

## Profiles

The installer probes the machine and picks one. You can override it at any time
in `/etc/nous/nous.conf`.

### `hosted` — 4 to 8 GB, no local model

Everything about the desktop, files, media and editing works. Intelligence comes
from an API key you supply. Nothing is downloaded and nothing runs locally.

The trade: routine background work — naming a download, classifying a file — is
simply *not done* rather than being quietly sent to a paid API. The small route
is set to `offline`, and the deterministic resolver still handles most of what
you type without any model at all.

### `hybrid` — 8 to 16 GB *(most laptops, and what the system is designed around)*

A small local model (~1 GB, `qwen2.5:1.5b-instruct`) handles the constant, small
work: sorting, naming, classifying, summarising a folder. It runs on CPU, needs
no GPU, and **never leaves the machine**. Harder requests — a genuinely ambiguous
intent — go to your API key.

This is the profile that makes the economics work. Most of what an AI-native OS
does all day is small, and small work should be free and private.

### `local` — 32 GB RAM, or 16 GB with an 8 GB GPU

A 7–8 B model handles everything, including intent resolution. An API key remains
useful for the hardest requests but is no longer required. On CPU alone expect
roughly 5–15 tokens/second, which is usable for this workload because the model
is emitting short GLYPH programs, not essays. With a GPU it is instant.

### `workstation` — 64 GB RAM, or a 24 GB GPU

A large local model. The whole system runs with no network at all.

## What actually costs memory

| | Approximate |
|---|---|
| `nousd` | 15–40 MB |
| Graphical shell (browser engine) | 400 MB – 1 GB |
| `qwen2.5:1.5b` (Q4) | ~1.2 GB |
| `qwen2.5:7b` (Q4) | ~5.5 GB |
| `qwen2.5:14b` (Q4) | ~9 GB |

A local model is only resident while it is answering, and `ollama` unloads it
after an idle period.

## GPUs

Not required. If you have one:

- **NVIDIA** — detected through `nvidia-smi`; VRAM decides the profile.
- **AMD** — detected through sysfs (`mem_info_vram_total`).
- **Intel / integrated** — detected, but shared memory means it does not raise
  the profile.

## Checking a specific machine

Run this on the machine in question, from the live image or an existing Linux
install:

```console
$ nousd --check
  CPU     Intel(R) Xeon(R) Processor @ 2.10GHz (4 cores, x86_64)
  Memory  16075 MB
  Disk    30520 MB free on /
  GPU     none detected

  Profile hybrid
          A small local model handles routine work — naming, sorting,
          classifying — and never leaves the machine. Harder requests go to
          your API key.
          Local model: qwen2.5:1.5b-instruct

  Note    16 GB is enough for a 7B local model if you want one — set the
          profile to `local` after installing.
```

## Dual boot

NOUS assumes it. The installer adopts the existing EFI System Partition rather
than replacing it, and enables `os-prober` so your other systems stay in the boot
menu. It will not resize or format anything you did not name — make free space
from inside your existing system first (Windows Disk Management, or GParted),
then point the installer at one partition in that space.

Reserve **20 GB minimum**, 40 GB if you want local models and a media library.
