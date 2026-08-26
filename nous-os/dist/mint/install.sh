#!/usr/bin/env bash
#
# Install NOUS on top of Linux Mint (or any Debian-family desktop).
#
# This does not replace anything. Cinnamon stays your desktop, Nemo stays your
# file manager, your applications stay your applications. NOUS is added
# alongside them: a daemon running as you, a hotkey that summons a command bar,
# and an entry in Nemo's right-click menu.
#
# It touches nothing outside /usr/local/bin, ~/.config/nous and ~/.local/share,
# and it never goes near the bootloader.

set -euo pipefail

VERSION="0.1.0"
COMMIT=0
HOTKEY="<Control><Alt>space"
PREFIX="/usr/local"
WITH_MEDIA=1
WITH_LOCAL_MODEL=0

BOLD=$'\033[1m'; DIM=$'\033[2m'; RESET=$'\033[0m'
RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'
if [[ -n "${NO_COLOR:-}" ]]; then BOLD=""; DIM=""; RESET=""; RED=""; GREEN=""; YELLOW=""; fi

say()  { printf '%s\n' "$*"; }

step() { printf '%s==>%s %s\n' "$BOLD" "$RESET" "$*"; }
warn() { printf '%s warning:%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
die()  { printf '%s error:%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

# "<Control><Alt>space" is how gsettings wants a binding written. Stripping the
# angle brackets and calling it done printed "  Control  Alt space"; a shortcut
# should read the way it is written on a keyboard.
pretty_hotkey() {
  local h="$1" out="" part
  h="${h//></+}"
  h="${h#<}"
  h="${h/>/+}"
  h="${h//Control/Ctrl}"
  h="${h//Primary/Ctrl}"
  h="${h//Mod4/Super}"
  local IFS='+'
  for part in $h; do
    if [[ -n "$part" ]]; then
      if [[ -n "$out" ]]; then out+="+"; fi
      out+="${part^}"
    fi
  done
  printf '%s' "$out"
}


# One function touches the machine. In preview mode it prints instead.
do_() {
  if (( COMMIT )); then "$@"; else printf '   %swould run:%s %s\n' "$DIM" "$RESET" "$*"; fi
}
# Always root. Package installation needs it wherever the binaries end up.
root_() {
  if (( COMMIT )); then sudo "$@"; else printf '   %swould run:%s sudo %s\n' "$DIM" "$RESET" "$*"; fi
}

# Root only where the destination genuinely needs it. A prefix inside your own
# home does not, and asking for a password you did not need is how people learn
# to type one without reading the prompt.
NEEDS_SUDO=1
prefix_() {
  if (( NEEDS_SUDO )); then root_ "$@"; else do_ "$@"; fi
}

usage() {
  cat <<USAGE
${BOLD}NOUS for Linux Mint${RESET} ${VERSION}

  install.sh [options]

  --commit              actually install (without this, nothing is written)
  --hotkey "<keys>"     summon binding (default: ${HOTKEY})
  --prefix DIR          where the binaries go (default: ${PREFIX})
  --no-media            skip mpv and ffmpeg
  --local-model         also install ollama and pull a small local model
  -h, --help            this

${BOLD}What it installs${RESET}
  ${PREFIX}/bin/{nousd,nsh,nousctl,nous-ask,nous-shell}
  ~/.config/systemd/user/nousd.service     the daemon, running as you
  ~/.local/share/nemo/actions/*.nemo_action   right-click menu entries
  ~/.config/nous/                          your configuration and policy
  a Cinnamon keybinding on ${HOTKEY}

${BOLD}What it does not touch${RESET}
  the bootloader, /etc, your desktop environment, your file manager,
  your existing applications, or anything you have not been told about.

Remove it all again with:  ${PREFIX}/bin/nous-uninstall
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --commit)       COMMIT=1; shift ;;
    --hotkey)       HOTKEY="${2:-}"; shift 2 ;;
    --prefix)       PREFIX="${2:-}"; shift 2 ;;
    --no-media)     WITH_MEDIA=0; shift ;;
    --local-model)  WITH_LOCAL_MODEL=1; shift ;;
    -h|--help)      usage; exit 0 ;;
    *)              die "unknown option '$1' (try --help)" ;;
  esac
done

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${HERE}/../.." && pwd)"

# Every binary the workspace produces. Named once, because build and install
# consulting different lists is exactly how a new binary gets compiled but never
# installed -- or checked for but never built.
BINARIES=(nousd nsh nousctl nous-shell nous)

# Whether this run will compile. Decided in preflight, because the dependency
# step needs to know whether to pull in the toolchain packages.
WILL_BUILD=0

# ------------------------------------------------------------------- checks

preflight() {
  step "Looking at this system"

  [[ "$(id -u)" != "0" ]] || die "run this as your normal user, not root (it will ask for sudo where it needs to)"
  command -v apt-get >/dev/null 2>&1 || die "this installer expects a Debian-family system (Mint, Ubuntu, Debian)"

  local distro="unknown"
  if [[ -r /etc/os-release ]]; then
    distro="$(. /etc/os-release && printf '%s' "${PRETTY_NAME:-$NAME}")"
  fi
  say "   system        ${distro}"
  say "   desktop       ${XDG_CURRENT_DESKTOP:-unknown} on ${XDG_SESSION_TYPE:-unknown}"
  say "   user          ${USER}"

  # Decide up front whether anything here needs root, and say so.
  mkdir -p "${PREFIX}/bin" 2>/dev/null || true
  if [[ -w "${PREFIX}/bin" ]]; then
    NEEDS_SUDO=0
    say "   install       ${PREFIX}/bin ${DIM}(writable — no sudo needed)${RESET}"
  else
    say "   install       ${PREFIX}/bin ${DIM}(needs sudo)${RESET}"
  fi
  say "   memory        $(( $(awk '/^MemTotal:/ {print $2}' /proc/meminfo) / 1024 )) MB"

  if command -v cargo >/dev/null 2>&1; then
    WILL_BUILD=1
    say "   build         ${DIM}from source with cargo${RESET}"
  else
    # Without a toolchain a prebuilt tree is the only way this can work, and it
    # has to be complete. Checked here rather than later so the answer is
    # "install Rust" before a password has been asked for and apt has run --
    # not after.
    local absent=()
    for b in "${BINARIES[@]}"; do
      [[ -x "${ROOT}/target/release/${b}" ]] || absent+=("$b")
    done
    if (( ${#absent[@]} )); then
      die "cargo is not installed, and these are not prebuilt: ${absent[*]}
       Install Rust from https://rustup.rs, then run this again.
       Nothing has been changed on this system."
    fi
    say "   build         ${DIM}no cargo — using the prebuilt binaries${RESET}"
  fi

  if [[ "${XDG_SESSION_TYPE:-}" == "wayland" ]]; then
    warn "on Wayland, window listing and focus are limited; the rest works normally"
  fi

  if ! systemctl --user show-environment >/dev/null 2>&1; then
    warn "no user systemd session detected; the daemon will need starting by hand"
  fi
}

# --------------------------------------------------------------- the steps

install_dependencies() {
  step "Installing what NOUS drives"

  # No browser here any more: the panel is a native X11 window. What is left
  # is what NOUS actually drives -- the window manager, the clipboard, and the
  # desktop's own notification channel.
  local packages=(wmctrl xdotool xclip libnotify-bin xdg-utils curl)
  if (( WITH_MEDIA )); then packages+=(mpv ffmpeg); fi

  # Compiling the panel needs the development packages for the libraries it
  # draws with. A desktop ships libX11.so.6 but not the libX11.so symlink the
  # linker resolves -l against, so without these the build stops at
  #     /usr/bin/ld: cannot find -lX11
  # which says nothing whatsoever about what to install. Only pulled in when
  # this run is actually going to compile.
  if (( WILL_BUILD )); then
    packages+=(build-essential pkg-config libx11-dev libcairo2-dev libpango1.0-dev libglib2.0-dev)
  fi

  local missing=()
  for p in "${packages[@]}"; do
    dpkg -s "$p" >/dev/null 2>&1 || missing+=("$p")
  done

  if (( ${#missing[@]} == 0 )); then
    say "   ${DIM}everything is already installed${RESET}"
    return
  fi
  say "   ${DIM}missing: ${missing[*]}${RESET}"
  root_ apt-get update
  root_ apt-get install -y "${missing[@]}"
}

build_binaries() {
  if (( WILL_BUILD )); then
    # Always build. Cargo already knows what is up to date, and second-guessing
    # it here got it wrong: a target/release left over from an earlier version
    # satisfied a check for one binary, the build was skipped, and the install
    # then failed partway through on a binary that had never been compiled --
    # leaving the previous version's binaries in place and this one believing it
    # had been upgraded.
    step "Building NOUS"
    say "   ${DIM}a minute or so the first time; seconds after that${RESET}"
    do_ env -C "$ROOT" cargo build --release
  else
    step "Using the prebuilt binaries in target/release"
  fi

  # However we got here, everything the install is about to copy must exist now.
  local absent=()
  for b in "${BINARIES[@]}"; do
    [[ -x "${ROOT}/target/release/${b}" ]] || absent+=("$b")
  done
  if (( ${#absent[@]} == 0 )); then return; fi

  # In preview mode nothing was compiled, so on a clean tree this is expected.
  if (( ! COMMIT )); then
    say "   ${DIM}not built yet: ${absent[*]}${RESET}"
    return
  fi
  if (( WILL_BUILD )); then
    die "the build did not produce: ${absent[*]}"
  fi
  die "cargo is not installed and these are not prebuilt: ${absent[*]}
       Install Rust from https://rustup.rs and run this again."
}

install_binaries() {
  step "Installing to ${PREFIX}/bin"
  for binary in "${BINARIES[@]}"; do
    prefix_ install -Dm755 "${ROOT}/target/release/${binary}" "${PREFIX}/bin/${binary}"
  done
  prefix_ install -Dm755 "${HERE}/nous-ask" "${PREFIX}/bin/nous-ask"
  prefix_ install -Dm755 "${HERE}/uninstall.sh" "${PREFIX}/bin/nous-uninstall"
}

# Menu entries and file-manager actions name the binary by absolute path, and
# that path depends on --prefix. Shipping them with /usr/local/bin baked in left
# every --prefix install with menu entries pointing at a binary that is not
# there, which fails silently: the menu item is present and simply does nothing.
install_menu_entry() {
  local src="$1" dest="$2"
  local tmp
  tmp="$(mktemp)"
  sed "s|@BIN@|${PREFIX}/bin|g" "$src" > "$tmp"
  install -Dm644 "$tmp" "$dest"
  rm -f "$tmp"
}

install_desktop_integration() {
  step "Wiring it into your desktop"

  local apps="${HOME}/.local/share/applications"
  local actions="${HOME}/.local/share/nemo/actions"
  local icons="${HOME}/.local/share/icons/hicolor/scalable/apps"
  local autostart="${HOME}/.config/autostart"

  do_ mkdir -p "$apps" "$actions" "$icons" "$autostart"

  # Hand the daemon the session environment at every login, and restart it so it
  # picks it up. The unit is WantedBy=graphical-session.target, but not every
  # Cinnamon build drives that target, and a daemon started at login without
  # DISPLAY can never acquire one afterwards -- a process's environment is fixed
  # when it execs. This runs inside the session, so it always has the answer.
  if (( COMMIT )); then
    cat > "${autostart}/nous-daemon.desktop" <<AUTOSTART
[Desktop Entry]
Type=Application
Name=NOUS daemon
Comment=Give the NOUS daemon this session's display, and start it
Exec=sh -c 'systemctl --user import-environment DISPLAY XAUTHORITY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP; systemctl --user restart nousd.service'
Terminal=false
NoDisplay=true
X-GNOME-Autostart-enabled=true
AUTOSTART
  else
    say "   ${DIM}would write ${autostart}/nous-daemon.desktop${RESET}"
  fi
  do_ install_menu_entry "${HERE}/nous.desktop"     "${apps}/nous.desktop"
  do_ install_menu_entry "${HERE}/nous-ask.desktop" "${apps}/nous-ask.desktop"
  do_ install -Dm644 "${HERE}/nous.svg"         "${icons}/nous.svg"

  if [[ -d "${HOME}/.local/share/nemo" ]] || command -v nemo >/dev/null 2>&1; then
    do_ install_menu_entry "${HERE}/nous.nemo_action"      "${actions}/nous.nemo_action"
    do_ install_menu_entry "${HERE}/nous-tidy.nemo_action" "${actions}/nous-tidy.nemo_action"
    say "   ${DIM}added to Nemo's right-click menu${RESET}"
  else
    say "   ${DIM}Nemo not found; skipping the file manager menu${RESET}"
  fi
}

# Earlier versions summoned the panel by launching Chromium in app mode against
# a local server, with its own browser profile so it would not disturb a window
# already open. The panel is native now and nothing reads that profile, but it
# is tens of megabytes of somebody's browser state sitting in their home
# directory, and an upgrade that leaves it there never mentions it again.
remove_browser_leftovers() {
  local overlay="${HOME}/.local/share/nous/overlay"
  if [[ ! -d "$overlay" ]]; then return; fi
  step "Clearing out the old browser profile"
  say "   ${DIM}${overlay} — the panel no longer uses a browser${RESET}"
  do_ rm -rf "$overlay"
}

install_configuration() {
  step "Setting up your configuration"
  local cfg="${HOME}/.config/nous"
  do_ mkdir -p "${cfg}/policy.d" "${cfg}/secrets"
  do_ chmod 700 "${cfg}/secrets"

  if [[ -f "${cfg}/nous.conf" ]]; then
    say "   ${DIM}keeping your existing ${cfg}/nous.conf${RESET}"
  else
    do_ install -Dm644 "${ROOT}/dist/overlay/etc/nous/nous.conf" "${cfg}/nous.conf"
  fi
  if [[ ! -f "${cfg}/policy.d/10-desktop.conf" ]]; then
    do_ install -Dm644 "${ROOT}/dist/overlay/etc/nous/policy.d/10-desktop.conf" \
        "${cfg}/policy.d/10-desktop.conf"
  fi
}

install_service() {
  step "Starting the daemon"
  local unit="${HOME}/.config/systemd/user"
  do_ mkdir -p "$unit"
  do_ install -Dm644 "${HERE}/nousd.user.service" "${unit}/nousd.service"
  # The unit ships with an absolute path; honour a custom prefix.
  do_ sed -i "s|/usr/local/bin/nousd|${PREFIX}/bin/nousd|" "${unit}/nousd.service"
  do_ systemctl --user daemon-reload

  # The daemon drives the desktop -- windows, clipboard, notifications -- and
  # cannot do any of it without DISPLAY. systemd's user manager does not inherit
  # the session environment, so hand it over explicitly before starting.
  do_ systemctl --user import-environment \
      DISPLAY XAUTHORITY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP

  do_ systemctl --user enable nousd.service

  # A unit left `failed` by an earlier broken install has usually also hit its
  # start limit, and restart then refuses with "start request repeated too
  # quickly". Clearing it first is harmless when the unit is healthy.
  if (( COMMIT )); then
    systemctl --user reset-failed nousd.service >/dev/null 2>&1 || true
  fi

  # restart, not `enable --now`. `--now` only *starts*, and starting an already
  # running unit does nothing at all -- so upgrading installed the new binary
  # and left the previous one serving, with no error and nothing in the output
  # to suggest it. The new panel would then be talking to the old daemon.
  do_ systemctl --user restart nousd.service
}

# Cinnamon keeps custom shortcuts as a list of names, each with its own schema
# path. Re-running must not add a second copy, so an existing NOUS binding is
# found and updated rather than appended.
register_hotkey() {
  step "Binding ${HOTKEY}"

  if ! command -v gsettings >/dev/null 2>&1; then
    warn "gsettings is missing; bind ${PREFIX}/bin/nous-ask to a key yourself"
    return
  fi
  if ! gsettings list-schemas 2>/dev/null | grep -q '^org.cinnamon.desktop.keybindings$'; then
    say "   ${DIM}not Cinnamon — bind ${PREFIX}/bin/nous-ask to a key in your settings${RESET}"
    return
  fi

  local base="org.cinnamon.desktop.keybindings"
  local existing slot="" list
  list="$(gsettings get "$base" custom-list 2>/dev/null || echo "@as []")"

  # Reuse the slot we made last time, if there is one.
  for name in $(printf '%s' "$list" | grep -o "custom[0-9]*" || true); do
    existing="$(gsettings get "${base}.custom-keybinding:/org/cinnamon/desktop/keybindings/custom-keybindings/${name}/" command 2>/dev/null || echo "")"
    if [[ "$existing" == *"nous-ask"* ]]; then slot="$name"; break; fi
  done

  if [[ -z "$slot" ]]; then
    local n=0
    while printf '%s' "$list" | grep -q "'custom${n}'"; do n=$(( n + 1 )); done
    slot="custom${n}"
    local updated
    if [[ "$list" == *"@as []"* || "$list" == "[]" ]]; then
      updated="['${slot}']"
    else
      updated="${list%]}, '${slot}']"
    fi
    do_ gsettings set "$base" custom-list "$updated"
  fi

  local path="${base}.custom-keybinding:/org/cinnamon/desktop/keybindings/custom-keybindings/${slot}/"
  do_ gsettings set "$path" name "Ask NOUS"
  do_ gsettings set "$path" command "${PREFIX}/bin/nous-ask"
  do_ gsettings set "$path" binding "['${HOTKEY}']"
  say "   ${DIM}slot ${slot}${RESET}"
}

install_local_model() {
  (( WITH_LOCAL_MODEL )) || return 0
  step "Installing a local model"
  if ! command -v ollama >/dev/null 2>&1; then
    say "   ${DIM}installing ollama${RESET}"
    do_ bash -c "curl -fsSL https://ollama.com/install.sh | sh"
  fi
  local model="qwen2.5:1.5b-instruct"
  local ram_gb=$(( $(awk '/^MemTotal:/ {print $2}' /proc/meminfo) / 1024 / 1024 ))
  if (( ram_gb >= 30 )); then model="qwen2.5:7b-instruct"; fi
  say "   ${DIM}pulling ${model} (${ram_gb} GB of memory available)${RESET}"
  do_ ollama pull "$model"
}

finish() {
  say ""
  step "Done"

  case ":${PATH}:" in
    *":${PREFIX}/bin:"*) ;;
    *)
      warn "${PREFIX}/bin is not on your PATH. Add this to ~/.profile and log back in:"
      say "           export PATH=\"${PREFIX}/bin:\$PATH\""
      say ""
      ;;
  esac

  if (( ! COMMIT )); then
    say "   ${YELLOW}That was a preview. Re-run with --commit to install.${RESET}"
    return
  fi

  local keys; keys="$(pretty_hotkey "$HOTKEY")"

  # Spelled out at this length because the short version was misread once: the
  # closing message suggested trying "tidy my downloads", it was typed at a bash
  # prompt, and bash answered command-not-found. Saying which window the words
  # go into costs three lines and removes the whole failure.
  say "   ${BOLD}To use it:${RESET}"
  say "     1. Press ${BOLD}${keys}${RESET} — a panel appears in the middle of the screen."
  say "     2. Type into ${BOLD}that panel${RESET}, not into this terminal."
  say "     3. Try: ${BOLD}tidy my downloads${RESET} — then press Enter."
  say ""
  say "   It shows you what it intends to do and waits. Nothing moves until you"
  say "   press Enter again to approve it. Escape throws the plan away."
  say ""
  say "   ${BOLD}Terminal commands${RESET}, if you want them:"
  say "     ${DIM}nsh${RESET}                         the same thing, as a terminal shell"
  say "     ${DIM}nousctl status${RESET}              check that it is running"
  say "     ${DIM}nousctl key set anthropic${RESET}   connect your own AI, if you want one"
  say "     ${DIM}nous-uninstall${RESET}              remove all of this again"
  say ""
  say "   ${DIM}It works with no AI model at all — it resolves what it can on its own.${RESET}"
}

# ---------------------------------------------------------------------- main

say "${BOLD}NOUS for Linux Mint${RESET} ${VERSION}"
say ""
if (( ! COMMIT )); then
  say "${YELLOW}Preview mode.${RESET} Nothing will be written. Add --commit to install."
  say ""
fi

preflight
say ""
install_dependencies
build_binaries
install_binaries
install_desktop_integration
remove_browser_leftovers
install_configuration
install_service
register_hotkey
install_local_model
finish
