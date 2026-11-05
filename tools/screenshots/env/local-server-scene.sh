# Sourced after setup.sh by the tapes of a local model server, with the
# server it stands in for: `source local-server-scene.sh llamacpp` or `… omlx`.
# Starts the mock model as that server, on its usual port, and puts in place
# of the recordings' `local` connection one to it, named `llama` or `omlx`,
# set up as its guide sets it up. No network.
case "$1" in
    llamacpp) name=llama port=8080 extra='model = "qwen3.6-35b-a3b"
context_window_fallback = 65536
max_concurrent_requests = 1' ;;
    omlx) name=omlx port=8000 extra='model = "Qwen3.8-Flash-Next-oQ4e-mtp"' ;;
    *) echo "local-server-scene.sh: llamacpp or omlx, not '$1'" >&2; return 1 ;;
esac
if ! (exec 3<>/dev/tcp/127.0.0.1/$port) 2>/dev/null; then
    python3 /opt/shots/mock-llm/server.py "$port" "$1" 2>/tmp/mock-$1.log &
    disown
    sleep 0.3
fi
config=$HOME/.config/termide/config.toml
sed -i "s/^connection = \"local\"\$/connection = \"$name\"/" "$config"
sed -i '/^\[ai.connections.local\]$/,/^$/d' "$config"
# The panel keys of cli-agent-scene.sh: Alt+X moves between the panels of a
# column, Alt+W and F10 close one.
sed -i '/^\[general.keybindings\]$/a prev_panel = "Alt+X"\nclose_panel = ["Alt+W", "F10"]' "$config"
printf '\n[ai.connections.%s]\nprovider = "openai_compatible"\nbase_url = "http://127.0.0.1:%s/v1"\n%s\n' \
    "$name" "$port" "$extra" >> "$config"
