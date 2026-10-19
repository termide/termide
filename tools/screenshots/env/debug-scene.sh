# Sourced (hidden) by the debug tape after setup.sh, in the project: a script
# to debug and a debug command for it. The package's modules use relative
# imports, so they cannot be debugged as scripts themselves.
cat > report_north.py <<'PY'
"""Print today's reorder report for the north store."""
from inventory.cli import load_store
from inventory.report import render

store = load_store("north")
print(render(store))
PY
cat >> .termide/commands.toml <<'EOF'

[debug-report]
name = "Debug reorder report"
command = "report_north.py"
debugger = "debugpy"
console = "panel"
EOF
clear
