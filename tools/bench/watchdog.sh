#!/bin/sh
# Watch a running `harbor run` or `harbor job resume` and record the
# conditions it runs under, until process $2 (the caller) is gone.
#
#   watchdog.sh <model|""> <caller-pid> <job-dir>
#
# With a model (an OMLX job): stops the run as soon as OMLX loads another
# model, since two large models do not fit in memory together and the trials
# would then fail on Metal out-of-memory errors that say nothing about the
# agent; and copies OMLX's per-request lines to <job-dir>/requests.log, which
# outlives OMLX's own logs (report.py reads speed and contention from them).
# Always: every minute appends `<epoch> <power source>` to <job-dir>/power.log;
# a gap in it is the machine asleep.
keep=${1-}
parent=${2:?caller pid}
dir=${3:?job dir}
log="$HOME/.omlx/logs/server.log"
seen=0
[ -n "$keep" ] && [ -f "$log" ] && seen=$(wc -l <"$log")
tick=0
while kill -0 "$parent" 2>/dev/null; do
  if [ $((tick % 12)) -eq 0 ]; then
    power=$(pmset -g ps 2>/dev/null | head -1 | sed -n "s/.*'\(.*\)'.*/\1/p")
    echo "$(date +%s) ${power:-unknown}" >>"$dir/power.log"
  fi
  tick=$((tick + 1))
  sleep 5
  [ -n "$keep" ] && [ -f "$log" ] || continue
  now=$(wc -l <"$log")
  # The log rotates at midnight: start over on the new file.
  [ "$now" -lt "$seen" ] && seen=0
  [ "$now" -eq "$seen" ] && continue
  new=$(tail -n +"$((seen + 1))" "$log" | head -n "$((now - seen))")
  seen=$now
  printf '%s\n' "$new" | grep "Chat completion: " >>"$dir/requests.log"
  printf '%s\n' "$new" | grep "Loading model: " | grep -v "Loading model: $keep" |
    while read -r line; do
      echo "$(date +%T) watchdog: $line"
      pkill -INT -f "harbor (run|job resume)" && echo "$(date +%T) watchdog: harbor stopped"
    done
done
