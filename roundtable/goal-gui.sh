#!/usr/bin/env bash
set -euo pipefail
roundtable_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ -x "$roundtable_dir/.venv/bin/python" ]]; then
    python="$roundtable_dir/.venv/bin/python"
else
    python=python3
fi

if ! "$python" -c "import tkinter" 2>/dev/null; then
    echo "Goal Mode's GUI needs Tk, which isn't installed for $python." >&2
    echo "On Debian/Ubuntu:" >&2
    echo "    sudo apt install python3-tk" >&2
    echo "Then run this script again." >&2
    exit 1
fi

exec "$python" "$roundtable_dir/goal_gui.py" "$@"
