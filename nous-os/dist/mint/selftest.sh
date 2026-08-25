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

# Pull the function under test out of the real script, so the test cannot drift
# from the thing it is testing.
eval "$(sed -n '/^urlencode() {/,/^}/p' "${HERE}/nous-ask")"

echo "percent-encoding"

# The bug this was written for: in a UTF-8 locale bash slices by character, so
# "café" encoded as %E9 (the code point) rather than %C3%A9 (the UTF-8 bytes),
# and every non-ASCII filename from the file manager arrived corrupted.
for loc in C C.UTF-8 en_US.UTF-8; do
  out="$(LC_ALL="$loc" bash -c "$(declare -f urlencode); urlencode '/home/j/café.png'" 2>/dev/null)"
  check "accented name under LC_ALL=$loc" "$out" "%2Fhome%2Fj%2Fcaf%C3%A9.png"
done

check "cjk name"        "$(urlencode '日本語.mp4')"    "%E6%97%A5%E6%9C%AC%E8%AA%9E.mp4"
check "astral plane"    "$(urlencode '🎬.mkv')"        "%F0%9F%8E%AC.mkv"
check "spaces"          "$(urlencode 'a b.txt')"       "a%20b.txt"
check "ampersand/hash"  "$(urlencode 'a&b#c')"         "a%26b%23c"
check "literal percent" "$(urlencode '100%')"          "100%25"
check "apostrophe"      "$(urlencode "it's")"          "it%27s"
check "newline"         "$(urlencode $'a\nb')"         "a%0Ab"
check "unreserved kept" "$(urlencode 'A-z_0.9~')"      "A-z_0.9~"

echo
echo "window placement"

# Chromium's --window-position takes x,y. "center" is not a value: it parses as
# zero, which put the summon overlay in the top-left corner of the screen.
if grep -q -- '--window-position=center' "${HERE}/nous-ask"; then
  bad "no literal 'center' position" "--window-position=center parses as 0,0"
else
  ok "no literal 'center' position"
fi
if grep -q 'getdisplaygeometry' "${HERE}/nous-ask"; then
  ok "centres against real screen geometry"
else
  bad "centres against real screen geometry" "expected a geometry lookup"
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
