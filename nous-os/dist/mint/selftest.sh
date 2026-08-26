#!/usr/bin/env bash
#
# Tests for the shell parts of the Mint integration.
#
# These exist because the Rust suite cannot reach them, and because every bug
# found in this directory so far has been of a kind that only shows up on a
# real desktop: a locale that slices strings differently, a flag that parses to
# zero instead of failing, a `set -e` abort on a false test at the end of a
# script. Each one is pinned here.
#
#   dist/mint/selftest.sh

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0
FAIL=0

ok()   { printf '  \033[32mok\033[0m    %s\n' "$1"; PASS=$(( PASS + 1 )); }
bad()  { printf '  \033[31mFAIL\033[0m  %s\n' "$1"; printf '        %s\n' "$2"; FAIL=$(( FAIL + 1 )); }
check() { if [[ "$2" == "$3" ]]; then ok "$1"; else bad "$1" "expected: $3
        got:      $2"; fi; }

# The percent-encoding tests that used to live here are gone with the thing they
# tested: nous-ask no longer builds a URL for a browser, it execs the native
# panel. The bug class they guarded -- a filename mangled on the way through --
# is still real, so it is tested below against the new mechanism instead.

echo "argument passing"

# A fake nous-shell that records exactly what it was handed, one argument per
# line. Anything that mangles a filename shows up as a wrong line count or a
# wrong line.
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
cat > "${TMP}/nous-shell" <<'FAKE'
#!/usr/bin/env bash
printf '%s\n' "$@" > "${NOUS_ARGV_OUT}"
FAKE
chmod +x "${TMP}/nous-shell"

# Run nous-ask against the fake, with no X tools on PATH so the focus lookup is
# skipped and the output is deterministic.
summon() {
  NOUS_SHELL="${TMP}/nous-shell" NOUS_ARGV_OUT="${TMP}/argv" \
    PATH="/usr/bin:/bin" bash "${HERE}/nous-ask" "$@" >/dev/null 2>&1
  cat "${TMP}/argv"
}

# The bug the encoder existed to prevent: a non-ASCII name arriving corrupted.
# Passed as an argv element it cannot be re-encoded at all, which is the point.
got="$(summon --paths '/home/j/café.png')"
check "accented name survives" "$got" "--cwd
/home/j
--paths
/home/j/café.png"

got="$(summon --paths '/a/日本語.mp4' '/a/🎬.mkv')"
check "cjk and astral names survive" "$got" "--cwd
/a
--paths
/a/日本語.mp4
/a/🎬.mkv"

# A space in a filename is the classic way a shell wrapper splits one file into
# two. It must arrive as a single argument.
got="$(summon --paths '/a/holiday photos.zip')"
check "a space does not split one file into two" "$got" "--cwd
/a
--paths
/a/holiday photos.zip"

got="$(summon --paths "/a/it's & 100%.txt")"
check "quoting metacharacters survive" "$got" "--cwd
/a
--paths
/a/it's & 100%.txt"

# --paths consumes the rest of its arguments, so it has to be emitted last or it
# swallows the options that follow it.
got="$(summon --ask 'tidy this' --paths '/a/x' '/a/y')"
check "--paths is passed last" "$got" "--cwd
/a
--ask
tidy this
--paths
/a/x
/a/y"

# A plain hotkey summon has no arguments at all. This is the case that a
# trailing false test under `set -e` used to abort before the panel opened.
got="$(summon)"
check "a bare summon passes nothing and still runs" "$got" ""

echo
echo "stale builds"

# The bug: build_binaries checked for ONE binary, so a target/release left over
# from a previous version satisfied it, the build was skipped, and the install
# then died partway through on a binary that had never been compiled -- leaving
# the old binaries in place and the user believing they had upgraded.
#
# The real functions are pulled out of the installer, so this cannot drift from
# what actually ships.
build_harness() {
  local root="$1" will_build="$2" commit="$3"
  (
    set -uo pipefail
    BOLD=""; DIM=""; RESET=""; RED=""; YELLOW=""; GREEN=""
    ROOT="$root"; WILL_BUILD="$will_build"; COMMIT="$commit"
    BINARIES=(nousd nsh nousctl nous-shell)
    say()  { printf '%s\n' "$*"; }
    step() { printf '==> %s\n' "$*"; }
    die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
    do_()  { if (( COMMIT )); then "$@"; else printf 'would run: %s\n' "$*"; fi; }
    eval "$(sed -n '/^build_binaries() {/,/^}/p' "${HERE}/install.sh")"
    build_binaries
  ) 2>&1
}

STALE="$(mktemp -d)"
mkdir -p "${STALE}/target/release"
for b in nousd nsh nousctl; do
  printf '#!/bin/sh\n' > "${STALE}/target/release/${b}"
  chmod +x "${STALE}/target/release/${b}"
done

# No cargo, committing, nous-shell never built: it must refuse by name and not
# let the install proceed to copy a half-set of binaries.
out="$(build_harness "$STALE" 0 1)"; rc=$?
if (( rc != 0 )) && [[ "$out" == *"nous-shell"* ]]; then
  ok "a stale target/ is refused by name, not installed around"
else
  bad "a stale target/ is refused by name, not installed around" "rc=${rc}: ${out}"
fi

# With cargo present the build must ALWAYS run. Skipping it on the strength of
# an old binary is the defect itself.
out="$(build_harness "$STALE" 1 0)"
if [[ "$out" == *"cargo build --release"* ]]; then
  ok "a stale target/ does not skip the build"
else
  bad "a stale target/ does not skip the build" "no build was attempted: ${out}"
fi

# A complete tree installs without complaint.
printf '#!/bin/sh\n' > "${STALE}/target/release/nous-shell"
chmod +x "${STALE}/target/release/nous-shell"
out="$(build_harness "$STALE" 0 1)"; rc=$?
if (( rc == 0 )); then
  ok "a complete prebuilt tree is accepted"
else
  bad "a complete prebuilt tree is accepted" "rc=${rc}: ${out}"
fi
rm -rf "$STALE"

# build and install must read the SAME list, or a new binary gets compiled and
# never installed.
if grep -q 'for binary in "${BINARIES\[@\]}"' "${HERE}/install.sh"; then
  ok "install_binaries uses the shared binary list"
else
  bad "install_binaries uses the shared binary list" "it has its own copy, which will drift"
fi

echo
echo "build dependencies"

# The panel links against X11, Cairo and Pango. A desktop ships libX11.so.6 but
# not the libX11.so symlink the linker resolves -l against, so without the -dev
# packages the build stops at "cannot find -lX11" -- which says nothing about
# what to install.
for pkg in pkg-config libx11-dev libcairo2-dev libpango1.0-dev libglib2.0-dev; do
  if grep -q -- "$pkg" "${HERE}/install.sh"; then
    ok "installer pulls in ${pkg}"
  else
    bad "installer pulls in ${pkg}" "the build will fail with 'cannot find -l...'"
  fi
done

echo
echo "launching"

# The panel is a native window now. A browser on this path would mean the thing
# the shell was rewritten to stop doing has come back.
if grep -qiE 'chromium|google-chrome|--app=|brave-browser' "${HERE}/nous-ask"; then
  bad "no browser is launched" "nous-ask still references a browser"
else
  ok "no browser is launched"
fi
if grep -q 'xdotool getactivewindow' "${HERE}/nous-ask"; then
  ok "captures the focused window before taking focus"
else
  bad "captures the focused window before taking focus" "expected an xdotool lookup"
fi

echo
echo "install paths"

# Menu entries name the binary by absolute path, and that path depends on
# --prefix. Baking /usr/local/bin into the shipped files left every --prefix
# install with menu items that silently did nothing.
for f in "${HERE}"/*.desktop "${HERE}"/*.nemo_action; do
  if grep -q '^Exec=/usr/local/bin' "$f"; then
    bad "$(basename "$f"): no hardcoded prefix" "Exec= names /usr/local/bin"
  else
    ok "$(basename "$f"): no hardcoded prefix"
  fi
  if grep -q '^Exec=@BIN@/' "$f"; then
    ok "$(basename "$f"): prefix is substituted at install time"
  else
    bad "$(basename "$f"): prefix is substituted at install time" "no @BIN@ in Exec"
  fi
done

if grep -q 's|@BIN@|' "${HERE}/install.sh"; then
  ok "install.sh substitutes @BIN@"
else
  bad "install.sh substitutes @BIN@" "the placeholder would be installed literally"
fi

echo
echo "set -e safety"

# A false `[[ ... ]] && cmd` returns non-zero, and under `set -e` that ends the
# script. Twice now this has silently aborted an installer mid-run.
for f in nous-ask install.sh uninstall.sh; do
  # Skip comments: the explanation of this very bug contains the pattern.
  hits="$(grep -n '\]\] &&' "${HERE}/${f}" \
    | grep -v ':[[:space:]]*#' | grep -v 'if \[\[' | grep -v '&& -' || true)"
  if [[ -z "$hits" ]]; then
    ok "${f}: no bare '[[ ]] &&' statements"
  else
    bad "${f}: no bare '[[ ]] &&' statements" "$hits"
  fi
done

echo
echo "menu entries"

# Nemo substitutes %U %F %P %f %p %D %e. Anything else renders literally, so a
# menu item reads "Ask NOUS about %N" to the person right-clicking.
for f in "${HERE}"/*.nemo_action; do
  name="$(grep '^Name=' "$f" | head -1)"
  if [[ "$name" == *%* ]]; then
    bad "$(basename "$f"): label has no substitution token" "$name"
  else
    ok "$(basename "$f"): label has no substitution token"
  fi
  if grep -q '^Exec=.*%F' "$f"; then
    ok "$(basename "$f"): passes the selection with %F"
  else
    bad "$(basename "$f"): passes the selection with %F" "no %F in Exec"
  fi
done

echo
echo "syntax"
for f in nous-ask install.sh uninstall.sh selftest.sh; do
  if bash -n "${HERE}/${f}" 2>/dev/null; then ok "${f} parses"; else bad "${f} parses" "syntax error"; fi
done

echo
if (( FAIL == 0 )); then
  printf '\033[32m%d passed\033[0m\n' "$PASS"
  exit 0
fi
printf '\033[31m%d failed\033[0m, %d passed\n' "$FAIL" "$PASS"
exit 1
