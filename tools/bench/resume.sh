#!/bin/sh
# Take up an interrupted job where it stopped: after a reboot, a closed
# terminal or a killed harbor.
#
#   tools/bench/resume.sh <job-name>
#
# A trial with no result.json was cut off mid-run: its containers and its
# directory go, and Harbor runs it again from the start. The mirror is started
# if it does not answer, and the run goes on in the background, detached from
# the terminal, logging to jobs/<job-name>.out; closing the terminal or the
# program it runs in does not stop it.
set -eu
bench=$(cd "$(dirname "$0")" && pwd)
job=${1:?job name}
dir="$bench/jobs/$job"
[ -f "$dir/config.json" ] || { echo "no job $dir" >&2; exit 2; }

if [ "${BENCH_DETACHED:-}" != 1 ]; then
  if pgrep -f "harbor (run|job resume).*$job" >/dev/null; then
    echo "$job is running already" >&2
    exit 1
  fi
  BENCH_DETACHED=1 nohup "$0" "$job" >>"$bench/jobs/$job.out" 2>&1 &
  echo "resuming $job in the background; log: jobs/$job.out"
  exit 0
fi

echo "$(date '+%F %T') resume.sh: resuming $job"
for trial in "$dir"/*__*/; do
  trial=${trial%/}
  [ -f "$trial/result.json" ] && continue
  name=$(basename "$trial" | tr '[:upper:]' '[:lower:]')
  echo "$(date +%T) resume.sh: dropping the unfinished $(basename "$trial")"
  docker ps -aq --filter "name=^${name}__" | xargs -r docker rm -f >/dev/null
  docker network ls -q --filter "name=^${name}__" | xargs -r docker network rm >/dev/null
  rm -rf "$trial"
done

port=${MIRROR_PORT:-10001}
if [ -d "$bench/mirror" ] && ! curl -sf -o /dev/null "http://127.0.0.1:$port/simple/pytest/"; then
  echo "$(date +%T) resume.sh: starting the mirror"
  nohup "$bench/mirror.sh" serve >>"$bench/jobs/mirror.log" 2>&1 &
fi

model=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1])).get("model",""))' "$dir/bench-meta.json" 2>/dev/null || true)
case "$(cat "$dir/bench-meta.json" 2>/dev/null)" in
  *:10000/*) [ -n "$model" ] && sh "$bench/watchdog.sh" "$model" $$ >>"$dir/watchdog.log" 2>&1 & ;;
esac

cd "$bench"
export PYTHONPATH="$bench"
awake=""
command -v caffeinate >/dev/null && awake="caffeinate -is"
$awake harbor job resume -p "$dir"
