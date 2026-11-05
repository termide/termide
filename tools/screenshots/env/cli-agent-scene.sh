# Sourced after setup.sh by the tapes of a CLI agent, with its provider:
# `source cli-agent-scene.sh claude_code`, `… codex` or `… gemini_cli`. Adds a
# connection of that provider, named `claude`, `codex` or `gemini`, that new sessions start on,
# served by the stand-in adapters in mock-acp/ (whose `npx` comes first on
# PATH). No network, no sign-in.
case "$1" in
    claude_code) name=claude ;;
    codex) name=codex ;;
    gemini_cli) name=gemini ;;
    *) echo "cli-agent-scene.sh: claude_code, codex or gemini_cli, not '$1'" >&2; return 1 ;;
esac
export PATH=/opt/shots/mock-acp:$PATH
config=$HOME/.config/termide/config.toml
sed -i "s/^connection = \"local\"\$/connection = \"$name\"/" "$config"
# Moving between the panels of a column takes Alt+Up, out of VHS's reach: it
# gets Alt+X, and closing keeps Alt+W and F10. It wraps round, so it reaches
# every panel of a column; no letter is left for next_panel (VHS reads Alt+S
# as a duration).
sed -i '/^\[general.keybindings\]$/a prev_panel = "Alt+X"\nclose_panel = ["Alt+W", "F10"]' "$config"
printf '\n[ai.connections.%s]\nprovider = "%s"\n' "$name" "$1" >> "$config"
