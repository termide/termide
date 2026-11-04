# Sourced after setup.sh by the Claude Code tapes: a `claude_code` connection
# that new sessions start on, served by the stand-in adapter in mock-acp/
# (its `npx` comes first on PATH). No network, no sign-in.
export PATH=/opt/shots/mock-acp:$PATH
config=$HOME/.config/termide/config.toml
sed -i 's/^connection = "local"$/connection = "claude"/' "$config"
# Moving between the panels of a column takes Alt+Up, out of VHS's reach: it
# gets Alt+X, and closing keeps Alt+W and F10. It wraps round, so it reaches
# every panel of a column; no letter is left for next_panel (VHS reads Alt+S
# as a duration).
sed -i '/^\[general.keybindings\]$/a prev_panel = "Alt+X"\nclose_panel = ["Alt+W", "F10"]' "$config"
cat >> "$config" <<'EOF'

[ai.connections.claude]
provider = "claude_code"
EOF
