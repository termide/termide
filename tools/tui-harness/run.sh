#!/bin/sh
# Run tools/tui-harness scenarios: makes the Python venv with pyte under the
# target directory on first use, then hands every argument to harness.py.
set -e
here=$(cd "$(dirname "$0")" && pwd)
target=${CARGO_TARGET_DIR:-$here/../../target}
venv=$target/tui-harness/venv
if [ ! -x "$venv/bin/python" ] || ! "$venv/bin/python" -c 'import pyte' 2>/dev/null; then
    python3 -m venv "$venv"
    "$venv/bin/pip" install --quiet --disable-pip-version-check 'pyte>=0.8'
fi
exec "$venv/bin/python" -I "$here/harness.py" "$@"
