#!/usr/bin/env bash
set -euo pipefail
roundtable_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ -x "$roundtable_dir/.venv/bin/python" ]]; then
    exec "$roundtable_dir/.venv/bin/python" "$roundtable_dir/goal_gui.py" "$@"
fi
exec python3 "$roundtable_dir/goal_gui.py" "$@"
