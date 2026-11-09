#!/bin/sh
# Stop a running `harbor run` as soon as OMLX loads a model other than $1:
# two large models do not fit in memory together, and the trials would then
# fail on Metal out-of-memory errors that say nothing about the agent.
# Polls the server log; exits once process $2 (the caller) is gone.
keep=${1:?model to keep}
parent=${2:?caller pid}
log="$HOME/.omlx/logs/server.log"
seen=$(wc -l <"$log")
while kill -0 "$parent" 2>/dev/null; do
  sleep 5
  now=$(wc -l <"$log")
  # The log rotates at midnight: start over on the new file.
  [ "$now" -lt "$seen" ] && seen=0
  [ "$now" -eq "$seen" ] && continue
  tail -n +"$((seen + 1))" "$log" | head -n "$((now - seen))" |
    grep "Loading model: " | grep -v "Loading model: $keep" |
    while read -r line; do
      echo "$(date +%T) watchdog: $line"
      pkill -INT -f "harbor run" && echo "$(date +%T) watchdog: harbor stopped"
    done
  seen=$now
done
