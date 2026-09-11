#!/usr/bin/env bash
set -euo pipefail
roundtable_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$roundtable_dir/launch.py" "$@"
