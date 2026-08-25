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

  local packages=(wmctrl xdotool xclip libnotify-bin xdg-utils curl)
  if ! command -v chromium >/dev/null 2>&1 && ! command -v chromium-browser >/dev/null 2>&1; then
    packages+=(chromium)
  fi
  if (( WITH_MEDIA )); then packages+=(mpv ffmpeg); fi

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
  if [[ -x "${ROOT}/target/release/nousd" ]]; then
    say "   ${DIM}using the binaries already built${RESET}"
    return
  fi
  step "Building NOUS"
  command -v cargo >/dev/null 2>&1 || die "cargo is not installed. Install Rust from https://rustup.rs, or use a prebuilt release."
  do_ env -C "$ROOT" cargo build --release
}

install_binaries() {
  step "Installing to ${PREFIX}/bin"
  for binary in nousd nsh nousctl; do
    prefix_ install -Dm755 "${ROOT}/target/release/${binary}" "${PREFIX}/bin/${binary}"
  done
  prefix_ install -Dm755 "${HERE}/nous-ask" "${PREFIX}/bin/nous-ask"
  prefix_ install -Dm755 "${ROOT}/dist/overlay/usr/bin/nous-session" "${PREFIX}/bin/nous-shell"
  prefix_ install -Dm755 "${HERE}/uninstall.sh" "${PREFIX}/bin/nous-uninstall"
}

install_desktop_integration() {
  step "Wiring it into your desktop"

  local apps="${HOME}/.local/share/applications"
  local actions="${HOME}/.local/share/nemo/actions"
  local icons="${HOME}/.local/share/icons/hicolor/scalable/apps"

  do_ mkdir -p "$apps" "$actions" "$icons"
  do_ install -Dm644 "${HERE}/nous.desktop"     "${apps}/nous.desktop"
  do_ install -Dm644 "${HERE}/nous-ask.desktop" "${apps}/nous-ask.desktop"
  do_ install -Dm644 "${HERE}/nous.svg"         "${icons}/nous.svg"

  if [[ -d "${HOME}/.local/share/nemo" ]] || command -v nemo >/dev/null 2>&1; then
    do_ install -Dm644 "${HERE}/nous.nemo_action"      "${actions}/nous.nemo_action"
    do_ install -Dm644 "${HERE}/nous-tidy.nemo_action" "${actions}/nous-tidy.nemo_action"
    say "   ${DIM}added to Nemo's right-click menu${RESET}"
  else
    say "   ${DIM}Nemo not found; skipping the file manager menu${RESET}"
  fi
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
  do_ systemctl --user enable --now nousd.service
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
  say "   Press ${BOLD}${HOTKEY//[<>]/ }${RESET} anywhere and say what you want."
  say ""
  say "   ${DIM}nous-shell${RESET}         the full desktop shell"
  say "   ${DIM}nsh${RESET}                a shell where language and commands share a prompt"
  say "   ${DIM}nousctl status${RESET}     check on it"
  say "   ${DIM}nousctl key set anthropic${RESET}   add a model, if you want one"
  say ""
  say "   It works with no model at all. Try ${BOLD}tidy my downloads${RESET} right now."
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
install_configuration
install_service
register_hotkey
install_local_model
finish
