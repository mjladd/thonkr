#!/bin/sh
# Run the A2B parity harness. See parity.py for what it measures.
#
#   tests/parity/parity.sh
#   tests/parity/parity.sh --seeds 8 --duration 60
#
# It needs a Python with numpy. Set PYTHON to choose which one.
set -eu
cd "$(dirname "$0")/../.."

PY="${PYTHON:-python3}"
if ! "$PY" -c 'import numpy' 2>/dev/null; then
    echo "error: $PY has no numpy. Try: PYTHON=/path/to/python tests/parity/parity.sh" >&2
    exit 1
fi
exec "$PY" tests/parity/parity.py "$@"
