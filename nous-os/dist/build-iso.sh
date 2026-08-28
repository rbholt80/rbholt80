#!/usr/bin/env bash
#
# Build a bootable NOUS live image.
#
# The result is a hybrid ISO that boots on UEFI and BIOS, runs the whole system
# from RAM, and carries the installer. You can try NOUS without touching the
# disk, and install from inside the thing you just tried.
#
# Requires: debootstrap, squashfs-tools, xorriso, grub-pc-bin, grub-efi-amd64-bin,
#           mtools. On Debian or Ubuntu:
#   sudo apt install debootstrap squashfs-tools xorriso grub-pc-bin \
#                    grub-efi-amd64-bin mtools

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${HERE}/.." && pwd)"
OUT="${HERE}/out"
WORK="${OUT}/work"
CHROOT="${WORK}/chroot"
ISO="${OUT}/nous-$(cat "${ROOT}/VERSION" 2>/dev/null || echo 0.1.0)-amd64.iso"
SUITE="${SUITE:-stable}"
MIRROR="${MIRROR:-http://deb.debian.org/debian}"

BOLD=$'\033[1m'; DIM=$'\033[2m'; RESET=$'\033[0m'; RED=$'\033[31m'
if [[ -n "${NO_COLOR:-}" ]]; then BOLD=""; DIM=""; RESET=""; RED=""; fi

step() { printf '%s==>%s %s\n' "$BOLD" "$RESET" "$*"; }
die()  { printf '%s error:%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

need() {
  local missing=()
  for tool in "$@"; do command -v "$tool" >/dev/null 2>&1 || missing+=("$tool"); done
  (( ${#missing[@]} == 0 )) || die "missing tools: ${missing[*]}"
}

[[ "$(id -u)" == "0" ]] || die "run this as root (it needs debootstrap and chroot)"
need debootstrap mksquashfs xorriso grub-mkstandalone

step "Building the NOUS binaries"
( cd "$ROOT" && cargo build --release )
for binary in nousd nsh nousctl; do
  [[ -x "${ROOT}/target/release/${binary}" ]] || die "${binary} did not build"
done

step "Bootstrapping ${SUITE}"
rm -rf "$WORK"; mkdir -p "$CHROOT"
debootstrap --arch=amd64 --variant=minbase "$SUITE" "$CHROOT" "$MIRROR"

step "Installing the live system"
# The package set is deliberately small: a kernel, a browser engine for the
# shell, the media tools, and the things the installer needs. NOUS itself has no
# runtime dependencies at all.
chroot "$CHROOT" /bin/bash -eu <<'INCHROOT'
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  linux-image-amd64 live-boot systemd-sysv \
  grub-efi-amd64 grub-pc-bin os-prober efibootmgr \
  chromium xserver-xorg xinit \
  mpv ffmpeg \
  network-manager curl ca-certificates \
  debootstrap parted gdisk dosfstools e2fsprogs \
  sudo less nano
apt-get clean
rm -rf /var/lib/apt/lists/*
INCHROOT

step "Installing NOUS"
for binary in nousd nsh nousctl; do
  install -Dm755 "${ROOT}/target/release/${binary}" "${CHROOT}/usr/bin/${binary}"
done
install -Dm755 "${HERE}/overlay/usr/bin/nous-session" "${CHROOT}/usr/bin/nous-session"
install -Dm755 "${HERE}/install.sh"                   "${CHROOT}/usr/bin/nous-install"
cp -r "${HERE}/overlay/etc/nous"                      "${CHROOT}/etc/"
cp "${HERE}/overlay/usr/lib/systemd/system/"*.service "${CHROOT}/usr/lib/systemd/system/"

# The live user. No password: this is a try-before-you-install image, and a
# password nobody was told is a lock with no key.
chroot "$CHROOT" /bin/bash -eu <<'INCHROOT'
useradd -m -s /bin/bash -G sudo,audio,video nous
passwd -d nous
echo 'nous ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/nous-live
chmod 440 /etc/sudoers.d/nous-live
echo nous-live > /etc/hostname
systemctl enable nousd.service
systemctl set-default multi-user.target

# Start the graphical shell on tty1 once the daemon is up.
mkdir -p /etc/systemd/system/getty@tty1.service.d
cat > /etc/systemd/system/getty@tty1.service.d/autologin.conf <<'UNIT'
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin nous --noclear %I $TERM
UNIT

cat > /home/nous/.bash_profile <<'PROFILE'
# On the first console, bring up the graphical shell. Anywhere else, you get a
# normal login -- the live image has to be usable when the desktop is what you
# are trying to debug.
if [[ "$(tty)" == "/dev/tty1" ]] && [[ -z "${DISPLAY:-}" ]]; then
  exec startx /usr/bin/nous-session
fi
PROFILE
chown nous:nous /home/nous/.bash_profile
INCHROOT

step "Compressing the filesystem"
mkdir -p "${WORK}/image/live"
mksquashfs "$CHROOT" "${WORK}/image/live/filesystem.squashfs" -comp zstd -Xcompression-level 15 -noappend
cp "${CHROOT}"/boot/vmlinuz-*  "${WORK}/image/live/vmlinuz"
cp "${CHROOT}"/boot/initrd.img-* "${WORK}/image/live/initrd"

step "Writing the boot menu"
mkdir -p "${WORK}/image/boot/grub"
cat > "${WORK}/image/boot/grub/grub.cfg" <<'GRUBCFG'
set default=0
set timeout=8

insmod all_video
insmod gfxterm
terminal_output gfxterm

menuentry "Try NOUS" {
  linux  /live/vmlinuz boot=live components quiet splash
  initrd /live/initrd
}

menuentry "Try NOUS (safe graphics)" {
  linux  /live/vmlinuz boot=live components nomodeset
  initrd /live/initrd
}

menuentry "Try NOUS (command shell only)" {
  linux  /live/vmlinuz boot=live components systemd.unit=multi-user.target
  initrd /live/initrd
}
GRUBCFG

step "Building the ISO"
mkdir -p "$OUT"
grub-mkstandalone \
  --format=x86_64-efi \
  --output="${WORK}/bootx64.efi" \
  --locales="" --fonts="" \
  "boot/grub/grub.cfg=${WORK}/image/boot/grub/grub.cfg"

# A FAT image holding the EFI bootloader, which is what makes the ISO bootable
# on UEFI machines as well as BIOS ones.
( cd "${WORK}" && \
  dd if=/dev/zero of=efiboot.img bs=1M count=12 status=none && \
  mkfs.vfat -n NOUSEFI efiboot.img >/dev/null && \
  mmd  -i efiboot.img ::/EFI ::/EFI/BOOT && \
  mcopy -i efiboot.img bootx64.efi ::/EFI/BOOT/BOOTX64.EFI )
cp "${WORK}/efiboot.img" "${WORK}/image/"

xorriso -as mkisofs \
  -iso-level 3 \
  -volid "NOUS" \
  -full-iso9660-filenames \
  -eltorito-alt-boot \
    -e efiboot.img -no-emul-boot -isohybrid-gpt-basdat \
  -output "$ISO" \
  "${WORK}/image"

step "Done"
printf '   %s\n' "$ISO"
printf '   %s%s%s\n' "$DIM" "$(du -h "$ISO" | cut -f1) — write it with: dd if=${ISO} of=/dev/sdX bs=4M status=progress" "$RESET"
