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
