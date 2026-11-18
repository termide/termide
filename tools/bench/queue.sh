#!/bin/sh
# Run a list of jobs one after another, unattended, in the background.
#
#   tools/bench/queue.sh <queue-file>
#
# Each line of the queue file is one job: its name, its task set, then the
# environment run.sh takes and the rest for harbor run, as start.sh takes
# them (`#` starts a comment):
#
#   tb21-cc-sonnet55  all  AGENT=claude-code MODEL=claude-sonnet-5-5 --agent-setup-timeout-multiplier 4
#
# A finished job is skipped and an interrupted one resumed (resume.sh), so
# after a reboot or a stop the same command takes the queue up where it
# stopped. Before each job it checks that Docker runs amd64 containers, the
# task images' platform, which a broken Rosetta in the Docker VM stops
# without a word. It stops the queue rather than go on when a job ends
# unfinished: stop.sh on the running job stops the queue with it. Logs to
# jobs/queue-<file name>.out, each job to jobs/<job>.out as usual.
set -eu
bench=$(cd "$(dirname "$0")" && pwd)
file=${1:?queue file}
[ -f "$file" ] || { echo "no queue $file" >&2; exit 2; }
file=$(cd "$(dirname "$file")" && pwd)/$(basename "$file")
name=$(basename "$file" .txt)
log="$bench/jobs/queue-$name.out"

finished() {
  python3 -I - "$bench/jobs/$1" <<'EOF'
import json, sys
from pathlib import Path
job = Path(sys.argv[1])
try:
    result = json.loads((job / "result.json").read_text())
except (OSError, ValueError):
    sys.exit(1)
done = sum(1 for t in job.glob("*__*/result.json"))
sys.exit(0 if result.get("finished_at") and done >= (result.get("n_total_trials") or 1) else 1)
EOF
}

if [ "${BENCH_DETACHED:-}" != 1 ]; then
  if pgrep -f "queue.sh $file" | grep -vqx "$$" || pgrep -f "harbor (run|job resume)" >/dev/null; then
    echo "a queue or a job is running already (status.sh)" >&2
    exit 1
  fi
  mkdir -p "$bench/jobs"
  # A session of its own: nothing that ends the caller's process group (a
  # terminal closing, termide quitting) reaches the queue.
  BENCH_DETACHED=1 nohup python3 -I -c 'import os, sys
try: os.setsid()
except OSError: pass
os.execvp(sys.argv[1], sys.argv[1:])' \
    "$0" "$file" >>"$log" 2>&1 </dev/null &
  echo "queue $name started in the background; log: tools/bench/jobs/queue-$name.out"
  exit 0
fi

say() { echo "$(date '+%F %T') queue $name: $*"; }
# An amd64 image already here (build.sh's), so the check pulls nothing.
image=$(sed -n 's/^image=//p' "$bench/build.sh")
say "started"
status=0
grep -v -e '^[[:space:]]*#' -e '^[[:space:]]*$' "$file" | while read -r job set rest; do
  if [ -e "$bench/jobs/$job/config.json" ] && finished "$job"; then
    say "$job finished already"
    continue
  fi
  if [ "$(docker run --rm --platform linux/amd64 "$image" uname -m 2>/dev/null)" != x86_64 ]; then
    say "Docker runs no amd64 container (restart the Docker VM: orb stop && orb start); stopping before $job"
    exit 1
  fi
  if [ -e "$bench/jobs/$job/config.json" ]; then
    say "$job: resuming"
    BENCH_DETACHED=1 sh "$bench/resume.sh" "$job" >>"$bench/jobs/$job.out" 2>&1 </dev/null || true
  else
    say "$job: starting ($set $rest)"
    # The line's VAR=value words go to the environment, the rest to harbor.
    # shellcheck disable=SC2086
    (
      set -f
      set -- $rest
      while [ $# -gt 0 ]; do
        case "$1" in [A-Z_]*=*) export "$1"; shift ;; *) break ;; esac
      done
      exec "$bench/run.sh" "$job" "$set" "$@"
    ) >>"$bench/jobs/$job.out" 2>&1 </dev/null || true
  fi
  if ! finished "$job"; then
    say "$job ended unfinished; stopping (queue.sh $file takes it up again)"
    exit 1
  fi
  say "$job finished: $(python3 -I "$bench/report.py" status "$job" 2>/dev/null | sed -n 5p | sed 's/^ *//')"
done || status=$?
[ "$status" -ne 0 ] || say "done"
exit "$status"
