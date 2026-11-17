#!/bin/sh
# Where the running benchmark jobs stand (report.py status).
#
#   tools/bench/status.sh [--watch [seconds]] [job...]
#
# With --watch it redraws every 15 seconds (or the given number) until
# Ctrl+C: keep it in a terminal panel to follow a run. It only reads, so
# closing the panel leaves the job alone.
bench=$(cd "$(dirname "$0")" && pwd)
if [ "${1-}" = --watch ]; then
  shift
  every=15
  case "${1-}" in '' | *[!0-9]*) ;; *) every=$1; shift ;; esac
  while :; do
    out=$(python3 -I "$bench/report.py" status "$@" 2>&1)
    printf '\033[H\033[2J%s  ·  every %ss, Ctrl+C to stop\n\n%s\n' "$(date +%T)" "$every" "$out"
    sleep "$every"
  done
fi
exec python3 -I "$bench/report.py" status "$@"
