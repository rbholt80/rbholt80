#!/usr/bin/env bash
#
# NOUS OS installer.
#
# The one thing this script must never do is destroy an operating system you
# still wanted. So it does not repartition disks, it does not format an ESP, and
# it does not write anything at all unless you pass --commit. Its default mode
# is to tell you exactly what it would do and stop.
#
# Dual boot is the assumed case, not a special one: it installs into free space
# you have already made, adopts the existing EFI System Partition rather than
# replacing it, and runs os-prober so whatever was on the machine before still
# appears in the boot menu.

set -euo pipefail

VERSION="0.1.0"
COMMIT=0
TARGET=""
ESP=""
HOSTNAME_NEW="nous"
USERNAME=""
PROFILE=""

BOLD=$'\033[1m'; DIM=$'\033[2m'; RESET=$'\033[0m'
RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'
if [[ -n "${NO_COLOR:-}" ]]; then BOLD=""; DIM=""; RESET=""; RED=""; GREEN=""; YELLOW=""; fi

say()  { printf '%s\n' "$*"; }
step() { printf '%s==>%s %s\n' "$BOLD" "$RESET" "$*"; }
warn() { printf '%s warning:%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
die()  { printf '%s error:%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

# In preview mode every mutating command is printed instead of run. There is
# exactly one function that touches the machine, which makes the blast radius
# of this script auditable by reading one place.
do_() {
  if (( COMMIT )); then
    "$@"
  else
    printf '   %swould run:%s %s\n' "$DIM" "$RESET" "$*"
  fi
}

usage() {
  cat <<USAGE
${BOLD}NOUS OS installer${RESET} ${VERSION}

  install.sh [options]

  --target /dev/sdXN     partition to install onto (must already exist)
  --esp /dev/sdXN        EFI System Partition to add a boot entry to
                         (default: the one already mounted or found)
  --user NAME            account to create
  --hostname NAME        machine name (default: nous)
  --profile P            hosted | hybrid | local | workstation
                         (default: whatever the hardware suggests)
  --commit               actually make the changes
  -h, --help             this

Without --commit nothing is written: the script prints the plan and exits.

${BOLD}Before you run this${RESET}
  Shrink your existing system's partition from inside that system, so you have
  unallocated space. Windows: Disk Management. Linux: GParted from a live USB.
  Then create one partition in that space and pass it as --target.

  This installer will not resize, delete or format anything you did not name.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)   TARGET="${2:-}"; shift 2 ;;
    --esp)      ESP="${2:-}"; shift 2 ;;
    --user)     USERNAME="${2:-}"; shift 2 ;;
    --hostname) HOSTNAME_NEW="${2:-}"; shift 2 ;;
    --profile)  PROFILE="${2:-}"; shift 2 ;;
    --commit)   COMMIT=1; shift ;;
    -h|--help)  usage; exit 0 ;;
    *)          die "unknown option '$1' (try --help)" ;;
  esac
done

# --------------------------------------------------------------- inspection

detect_firmware() {
  if [[ -d /sys/firmware/efi ]]; then echo "uefi"; else echo "bios"; fi
}

# The ESP is shared with every other operating system on the machine. Finding
# the existing one and adding to it is the whole of dual-boot safety.
find_esp() {
  local candidate
  candidate="$(lsblk -rno PATH,PARTTYPE 2>/dev/null \
    | awk '$2 == "c12a7328-f81f-11d2-ba4b-00a0c93ec93b" { print $1; exit }')" || true
  if [[ -n "$candidate" ]]; then echo "$candidate"; return 0; fi
  findmnt -no SOURCE /boot/efi 2>/dev/null && return 0
  return 1
}

detect_other_systems() {
  if command -v os-prober >/dev/null 2>&1; then
    os-prober 2>/dev/null | cut -d: -f2 | sed 's/^ *//' || true
  fi
}

suggest_profile() {
  local ram_kb ram_gb
  ram_kb="$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)"
  ram_gb=$(( ram_kb / 1024 / 1024 ))
  if   (( ram_gb >= 60 )); then echo workstation
  elif (( ram_gb >= 30 )); then echo local
  elif (( ram_gb >= 7  )); then echo hybrid
  else                          echo hosted
  fi
}

# ------------------------------------------------------------------- checks

preflight() {
  step "Looking at this machine"

  local firmware; firmware="$(detect_firmware)"
  say "   firmware      ${firmware}"
  say "   memory        $(( $(awk '/^MemTotal:/ {print $2}' /proc/meminfo) / 1024 )) MB"
  say "   cores         $(nproc)"

  if [[ -z "$PROFILE" ]]; then PROFILE="$(suggest_profile)"; fi
  say "   profile       ${PROFILE}"

  if [[ "$firmware" == "uefi" && -z "$ESP" ]]; then
    ESP="$(find_esp || true)"
    if [[ -z "$ESP" ]]; then die "no EFI System Partition found — name one with --esp"; fi
    say "   esp           ${ESP} ${DIM}(existing — will be added to, not replaced)${RESET}"
  fi

  local others; others="$(detect_other_systems)"
  if [[ -n "$others" ]]; then
    say ""
    say "   ${BOLD}Already installed on this machine:${RESET}"
    while IFS= read -r os; do if [[ -n "$os" ]]; then say "     · ${os}"; fi; done <<< "$others"
    say "   ${DIM}These will be kept and added to the boot menu.${RESET}"
  elif command -v os-prober >/dev/null 2>&1; then
    say "   ${DIM}No other operating systems detected.${RESET}"
  else
    warn "os-prober is not installed; other systems may not appear in the boot menu"
  fi

  if [[ -z "$TARGET" ]]; then die "name the partition to install onto with --target (see --help)"; fi
  [[ -b "$TARGET" ]] || die "$TARGET is not a block device"

  # Refuse a partition that currently holds a mounted filesystem: that is
  # somebody's running system.
  if findmnt -no TARGET "$TARGET" >/dev/null 2>&1; then
    die "$TARGET is mounted at $(findmnt -no TARGET "$TARGET") — refusing to touch it"
  fi

  local size_bytes size_gb
  size_bytes="$(blockdev --getsize64 "$TARGET" 2>/dev/null || echo 0)"
  size_gb=$(( size_bytes / 1024 / 1024 / 1024 ))
  (( size_gb >= 15 )) || die "$TARGET is ${size_gb} GB; NOUS needs at least 15 GB"
  say "   target        ${TARGET} (${size_gb} GB)"

  local existing
  existing="$(lsblk -rno FSTYPE "$TARGET" 2>/dev/null || true)"
  if [[ -n "$existing" ]]; then
    say ""
    warn "$TARGET already contains a ${existing} filesystem. Installing will erase it."
    say "   ${DIM}Everything else on this disk is left alone.${RESET}"
  fi

  if [[ -z "$USERNAME" ]]; then die "give an account name with --user"; fi
}

# ---------------------------------------------------------------- the steps

format_target() {
  step "Preparing ${TARGET}"
  do_ mkfs.ext4 -F -L NOUS "$TARGET"
  do_ mkdir -p /mnt/nous
  do_ mount "$TARGET" /mnt/nous
}

install_base() {
  step "Installing the base system"
  # debootstrap keeps this honest: the result is an ordinary Debian-family
  # system with NOUS on top, not an opaque image.
  do_ debootstrap --arch=amd64 --include=systemd-sysv,linux-image-amd64,grub-efi-amd64,os-prober,ca-certificates \
      stable /mnt/nous http://deb.debian.org/debian
}

install_nous() {
  step "Installing NOUS"
  local here; here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

  for binary in nousd nsh nousctl; do
    do_ install -Dm755 "${here}/../target/release/${binary}" "/mnt/nous/usr/bin/${binary}"
  done
  do_ install -Dm755 "${here}/overlay/usr/bin/nous-session" /mnt/nous/usr/bin/nous-session
  do_ install -Dm644 "${here}/overlay/usr/lib/systemd/system/nousd.service" \
      /mnt/nous/usr/lib/systemd/system/nousd.service
  do_ install -Dm644 "${here}/overlay/usr/lib/systemd/system/nous-shell.service" \
      /mnt/nous/usr/lib/systemd/system/nous-shell.service
  do_ install -Dm644 "${here}/overlay/etc/nous/nous.conf" /mnt/nous/etc/nous/nous.conf
  do_ install -Dm644 "${here}/overlay/etc/nous/policy.d/10-desktop.conf" \
      /mnt/nous/etc/nous/policy.d/10-desktop.conf

  # Record the hardware profile the installer chose, so first boot routes
  # models the way this machine can actually manage.
  do_ sed -i "s|^route = .*|route = $(profile_route)|" /mnt/nous/etc/nous/nous.conf
}

profile_route() {
  case "$PROFILE" in
    hosted)  echo "anthropic,openai,offline" ;;
    *)       echo "ollama,anthropic,openai,offline" ;;
  esac
}

configure_system() {
  step "Configuring the system"
  do_ bash -c "echo '${HOSTNAME_NEW}' > /mnt/nous/etc/hostname"
  do_ chroot /mnt/nous useradd -m -s /bin/bash -G sudo,audio,video "$USERNAME"
  do_ chroot /mnt/nous systemctl enable nousd.service
  do_ chroot /mnt/nous systemctl enable nous-shell.service

  say "   ${DIM}You will be asked to set a password for ${USERNAME}.${RESET}"
  do_ chroot /mnt/nous passwd "$USERNAME"
}

install_bootloader() {
  step "Adding NOUS to the boot menu"

  if [[ "$(detect_firmware)" == "uefi" ]]; then
    do_ mkdir -p /mnt/nous/boot/efi
    # Mounted, never formatted: the ESP belongs to every system on the machine.
    do_ mount "$ESP" /mnt/nous/boot/efi
    do_ chroot /mnt/nous grub-install --target=x86_64-efi --efi-directory=/boot/efi \
        --bootloader-id=NOUS --recheck
  else
    # The BIOS bootloader goes on the disk, not the partition. An empty answer
    # here would mean running `grub-install /dev/`, so it is fatal rather than
    # something to paper over.
    local disk; disk="$(lsblk -no PKNAME "$TARGET" 2>/dev/null | head -1)"
    if [[ -z "$disk" ]]; then
      die "cannot work out which disk ${TARGET} is on; install GRUB by hand or use UEFI"
    fi
    if [[ ! -b "/dev/${disk}" ]]; then
      die "/dev/${disk} is not a disk — refusing to install a bootloader there"
    fi
    do_ chroot /mnt/nous grub-install "/dev/${disk}"
  fi

  # This is the line that makes dual boot work. Without it GRUB writes a menu
  # containing only NOUS, and the machine looks like it ate Windows.
  do_ bash -c "echo 'GRUB_DISABLE_OS_PROBER=false' >> /mnt/nous/etc/default/grub"
  do_ chroot /mnt/nous update-grub

  if (( COMMIT )); then
    local found
    found="$(grep -c menuentry /mnt/nous/boot/grub/grub.cfg 2>/dev/null || echo 0)"
    say "   ${GREEN}${found} boot entries written${RESET}"
    if (( found < 2 )) && [[ -n "$(detect_other_systems)" ]]; then
      warn "other systems were detected but only one boot entry was written."
      warn "Do not reboot yet. Check /mnt/nous/boot/grub/grub.cfg first."
    fi
  fi
}

finish() {
  do_ umount -R /mnt/nous
  say ""
  step "Done"
  say "   Reboot and choose NOUS from the boot menu."
  say "   ${DIM}Your other systems are still there and still listed.${RESET}"
  say ""
  say "   First boot lands in the shell (nsh). At its prompt, try: ${BOLD}tidy my downloads${RESET}"
  if [[ "$PROFILE" == "hosted" ]]; then
    say "   This machine uses a hosted model. Add a key with:"
    say "     ${BOLD}nousctl key set anthropic${RESET}"
  fi
}

# ---------------------------------------------------------------------- main

say "${BOLD}NOUS OS${RESET} ${VERSION} installer"
say ""

if (( ! COMMIT )); then
  say "${YELLOW}Preview mode.${RESET} Nothing will be written. Add --commit to install."
  say ""
fi

[[ "$(id -u)" == "0" ]] || die "run this as root"

preflight
say ""

format_target
install_base
install_nous
configure_system
install_bootloader
finish
