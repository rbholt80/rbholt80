# NOUS on Linux Mint

You do not have to replace your operating system to use NOUS. This installs it
**alongside** Linux Mint: Cinnamon stays your desktop, Nemo stays your file
manager, and your applications stay exactly as they are. What you gain is a
system that can see and act on all of it.

Everything below applies equally to Ubuntu, Debian, Pop!_OS and any other
Debian-family desktop. Cinnamon gets the keyboard shortcut and the Nemo menu
registered automatically; other desktops need one manual step, noted below.

## Installing

```console
git clone <this repository> && cd nous-os

# Look at what it would do. Nothing is written.
./dist/mint/install.sh

# Do it.
./dist/mint/install.sh --commit
```

Run it as **yourself**, not as root. It asks for `sudo` only to put five
binaries in `/usr/local/bin`, and refuses to run if you are root.

Useful options:

```console
--hotkey "<Super>space"    a different summon key (default: Ctrl+Alt+Space)
--local-model              also install ollama and pull a model that fits
--no-media                 skip mpv and ffmpeg
--prefix ~/.local          put the binaries in your home, so only
                           the package install needs sudo
```

### What lands where

| | |
|---|---|
| `/usr/local/bin/` | `nousd`, `nsh`, `nousctl`, `nous-ask`, `nous-shell` |
| `~/.config/systemd/user/nousd.service` | the daemon, running as you |
| `~/.config/nous/` | your settings, policy and keys |
| `~/.local/state/nous/` | the journal, trash store and index |
| `~/.local/share/nemo/actions/` | the right-click menu entries |
| a Cinnamon keybinding | the summon key |

### What it does not touch

The bootloader. `/etc`. Your desktop environment. Your file manager. Your
existing applications. Anything not in the table above.

```console
nous-uninstall            # removes all of it, keeps your journal and settings
nous-uninstall --purge    # removes those too
```

## Using it

### The summon key

Press **Ctrl+Alt+Space** anywhere — in a browser, a terminal, a document — and
a command bar appears over whatever you were doing.

It already knows the window you were looking at. It captures that *before* it
appears; otherwise the answer would always be "NOUS". Type what you want, see
the plan, press Enter. Escape dismisses it.

### The right-click menu

Select files in Nemo, right-click, **Ask NOUS about…**. The command bar opens
with that selection attached, so:

- *"open these"* opens each in its usual application
- *"delete these"* moves them to the trash store, reversibly
- *"copy the paths"* puts them on your clipboard
- *"tidy these"* scans the folder they are in

A folder you name still wins over a selection — *"open my downloads"* is a
folder listing even when files happen to be selected.

### The full shell

```console
nous-shell     # the desktop shell, as its own window
nsh            # a terminal shell where language and commands share one prompt
nousctl status # is it running, and what is it routing to
```

`nsh` is worth a moment. `!` runs a shell command, `:` is a builtin, and
anything else is an intent:

```console
❯ what's using my disk
❯ !df -h
❯ :undo
❯ find that invoice from March
```

The `!` prefix is a deliberate, direct exec rather than a governed
`shell.exec` — that is you stepping outside the policed path on purpose, and
dressing it up as policed would be a lie. Everything the system does on its own
initiative still goes through the broker.

## What it can reach

Installing over Mint adds a `desk` capability domain on top of the file, media,
system and curation capabilities:

| | |
|---|---|
| `desk.apps` | everything installed, from your `.desktop` files |
| `desk.windows` | what is open, on which workspace |
| `desk.launch` `desk.focus` `desk.close` | start, switch to, ask to close |
| `desk.open` | open a file in its usual application |
| `desk.notify` | a desktop notification |
| `desk.copy` `desk.clipboard` | write and read the clipboard |
| `desk.screenshot` | capture the screen |
| `desk.setting` | read and change desktop settings via gsettings |
| `desk.session` | lock or log out |

Two of those are classed **elevated** rather than read: `desk.clipboard` and
`desk.screenshot`. Whatever is on your clipboard or your screen right now may be
a password or somebody else's message, and unlike a file you did not choose to
put it in front of the system. Both ask before they run, and agents are denied
both outright.

```console
$ nousctl check desk.screenshot
desk.screenshot  confirm  (elevated risk)
needs confirmation — so may whatever is on screen
```

## Things it drives

The installer pulls these in. NOUS itself has no runtime dependencies; these are
the programs it asks to do desktop work:

| Package | For |
|---|---|
| `wmctrl` | listing, focusing and closing windows |
| `xdotool` | which window is focused |
| `xclip` | the clipboard |
| `libnotify-bin` | notifications |
| `xdg-utils` | opening a file in its usual application |
| `chromium` | the shell's rendering engine |
| `mpv`, `ffmpeg` | playback, and the editing pipeline |

Anything missing produces a sentence naming the package, not a failure:

```console
cannot read the clipboard — no tool for it is installed. Try: sudo apt install xclip
```

## Adding a model

It works with none — the deterministic resolver handles the shapes people
actually type, and it is faster and more private than asking a model to open a
folder.

```console
nousctl key set anthropic       # or openai, openrouter
./dist/mint/install.sh --local-model --commit   # or a local one
nousctl models
```

On a laptop the sensible arrangement is both: a small local model for the
constant, routine work — naming, sorting, classifying — which never leaves the
machine, and an API key for the genuinely hard requests. That is the default
routing.

## Other desktops

GNOME, KDE and XFCE all work. Two things the installer only does for Cinnamon:

**The keyboard shortcut.** Bind `/usr/local/bin/nous-ask` to a key in your own
settings. GNOME: Settings → Keyboard → Custom Shortcuts. KDE: System Settings →
Shortcuts → Custom.

**The file manager menu.** The `.nemo_action` files are Nemo-specific. For
Dolphin use a Service Menu, for Nautilus a script in
`~/.local/share/nautilus/scripts`, both calling
`nous-ask --paths "$@"`.

## Wayland

Mint defaults to X11, where everything works. On Wayland, window listing,
focusing and closing are restricted by the compositor and will report themselves
unavailable. Files, media, curation, search, the clipboard (via `wl-clipboard`)
and the shell all work normally.

## When something is wrong

```console
systemctl --user status nousd     # is the daemon up
journalctl --user -u nousd -f     # what is it saying
nousctl status                    # what does it think it is doing
nousctl doctor                    # this machine, and its model profile
nousctl ledger                    # everything it has done
nousctl undo                      # take back the last thing
```

If the summon key does nothing, check the daemon is running and that
`nous-ask` works from a terminal — it will tell you what is missing.

The shell parts of this integration have their own tests, because the Rust
suite cannot reach them and every bug found here so far has been one that only
appears on a real desktop — a locale that slices strings differently, a flag
that parses to zero instead of failing:

```console
$ dist/mint/selftest.sh
24 passed
```
