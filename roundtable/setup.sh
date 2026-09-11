#!/usr/bin/env bash
# Set Roundtable up in a virtual environment.
#
# Debian, Ubuntu and Fedora mark the system Python as "externally managed"
# (PEP 668), so `pip install` into it refuses outright. That is the OS being
# careful, not a problem with this project -- a venv is the supported answer.
set -euo pipefail
cd "$(dirname "$0")"

if ! python3 -m venv .venv 2>/tmp/roundtable-venv-err; then
    echo "Could not create the virtual environment:" >&2
    sed 's/^/    /' /tmp/roundtable-venv-err >&2
    echo >&2
    echo "On Debian or Ubuntu this usually means one apt package is missing:" >&2
    echo "    sudo apt install python3-venv" >&2
    exit 1
fi

./.venv/bin/pip install --quiet --upgrade pip
./.venv/bin/pip install -e ".[all]"

cat <<DONE

Done. Either activate the environment for this shell:

    source $(pwd)/.venv/bin/activate
    roundtable doctor --probe

or put it on your PATH once and forget about it:

    mkdir -p ~/.local/bin && ln -sf $(pwd)/.venv/bin/roundtable ~/.local/bin/roundtable
    roundtable doctor --probe
DONE
