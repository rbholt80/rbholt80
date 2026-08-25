#!/usr/bin/env bash
#
# Remove NOUS from this system.
#
# Your files, your journal and your configuration are left alone unless you ask
# for them to go too — undoing an installation should not be a way to lose the
# record of everything the system did for you.

set -euo pipefail
PREFIX="${PREFIX:-/usr/local}"
PURGE=0
BOLD=$'\033[1m'; DIM=$'\033[2m'; RESET=$'\033[0m'
if [[ -n "${NO_COLOR:-}" ]]; then BOLD=""; DIM=""; RESET=""; fi

if [[ "${1:-}" == "--purge" ]]; then PURGE=1; fi
if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  echo "nous-uninstall [--purge]"
  echo "  --purge   also delete ~/.config/nous and ~/.local/state/nous"
  exit 0
fi

echo "${BOLD}Removing NOUS${RESET}"

systemctl --user disable --now nousd.service 2>/dev/null || true
rm -f "${HOME}/.config/systemd/user/nousd.service"
systemctl --user daemon-reload 2>/dev/null || true

for b in nousd nsh nousctl nous-ask nous-shell nous-uninstall; do
  sudo rm -f "${PREFIX}/bin/${b}"
done

rm -f "${HOME}/.local/share/applications/nous.desktop" \
      "${HOME}/.local/share/applications/nous-ask.desktop" \
      "${HOME}/.local/share/nemo/actions/nous.nemo_action" \
      "${HOME}/.local/share/nemo/actions/nous-tidy.nemo_action" \
      "${HOME}/.local/share/icons/hicolor/scalable/apps/nous.svg"
rm -rf "${HOME}/.local/share/nous"

# Take back the keybinding slot rather than leaving a dead shortcut behind.
if command -v gsettings >/dev/null 2>&1 &&
   gsettings list-schemas 2>/dev/null | grep -q '^org.cinnamon.desktop.keybindings$'; then
  base="org.cinnamon.desktop.keybindings"
  list="$(gsettings get "$base" custom-list 2>/dev/null || echo "[]")"
  for name in $(printf '%s' "$list" | grep -o "custom[0-9]*" || true); do
    path="${base}.custom-keybinding:/org/cinnamon/desktop/keybindings/custom-keybindings/${name}/"
    if [[ "$(gsettings get "$path" command 2>/dev/null || echo "")" == *"nous-ask"* ]]; then
      gsettings set "$path" binding "[]" 2>/dev/null || true
      gsettings set "$path" command "" 2>/dev/null || true
      gsettings set "$path" name "" 2>/dev/null || true
      echo "  ${DIM}released keybinding slot ${name}${RESET}"
    fi
  done
fi

if (( PURGE )); then
  rm -rf "${HOME}/.config/nous" "${HOME}/.local/state/nous"
  echo "  ${DIM}configuration, journal and trash store deleted${RESET}"
else
  echo ""
  echo "  Kept: ${DIM}~/.config/nous${RESET} (your settings and keys)"
  echo "        ${DIM}~/.local/state/nous${RESET} (the journal, trash store and index)"
  echo "  Delete those too with: nous-uninstall --purge"
fi

echo ""
echo "Done. Nothing else on this system was changed."
