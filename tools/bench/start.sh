#!/bin/sh
# Start a job in the background, detached from the terminal and from the
# program that runs it, so that closing either leaves the job running.
#
#   tools/bench/start.sh <job-name> [set-name] [harbor run args...]
#
# Takes the same arguments and environment as run.sh (AGENT, MODEL, ...).
# Logs to jobs/<job-name>.out; follow it with status.sh, stop it with
# stop.sh, take it up after a reboot with resume.sh.
set -eu
bench=$(cd "$(dirname "$0")" && pwd)
job=${1:?job name}
if [ -e "$bench/jobs/$job/config.json" ]; then
  echo "$job exists already: resume it with resume.sh, or pick another name" >&2
  exit 2
fi
if pgrep -f "harbor (run|job resume)" >/dev/null; then
  echo "another job is running (status.sh): two at once share the model server" >&2
  exit 1
fi
mkdir -p "$bench/jobs"
# A session of its own: nothing that ends the caller's process group (a
# terminal closing, termide quitting) reaches the job.
nohup python3 -I -c 'import os, sys
try: os.setsid()
except OSError: pass
os.execvp(sys.argv[1], sys.argv[1:])' \
  "$bench/run.sh" "$@" >>"$bench/jobs/$job.out" 2>&1 </dev/null &
echo "started $job in the background; log: tools/bench/jobs/$job.out"
# Give run.sh time to write bench-meta.json and harbor time to start, so
# that a mistake shows here rather than in the log.
sleep 15
if ! pgrep -f "harbor (run|job resume).*(--job-name |/jobs/)$job( |\$)" >/dev/null; then
  echo "harbor is not running; the end of the log:" >&2
  tail -15 "$bench/jobs/$job.out" >&2
  exit 1
fi
exec python3 -I "$bench/report.py" status "$job"
