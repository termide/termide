#!/bin/sh
# Stop a running job, or every running one when none is named.
#
#   tools/bench/stop.sh [job-name]
#
# Interrupts harbor as Ctrl+C would, so that it records what finished; the
# trial it was running is cut off, and resume.sh runs it again from the start.
# The cut-off trial's containers are removed here.
set -eu
bench=$(cd "$(dirname "$0")" && pwd)
job=${1-}
pattern="harbor (run|job resume).*${job:+(--job-name |/jobs/)$job( |\$)}"
if ! pgrep -f "$pattern" >/dev/null; then
  echo "no job${job:+ $job} is running"
  exit 0
fi
# The jobs being stopped, for their cut-off trials below.
running=""
for dir in "$bench"/jobs/*/; do
  name=$(basename "$dir")
  pgrep -f "harbor (run|job resume).*(--job-name |/jobs/)$name( |\$)" >/dev/null && running="$running $name"
done
pkill -INT -f "$pattern"
i=0
while pgrep -f "$pattern" >/dev/null && [ $i -lt 60 ]; do
  sleep 1
  i=$((i + 1))
done
if pgrep -f "$pattern" >/dev/null; then
  echo "harbor did not stop in 60 s; terminating it"
  pkill -TERM -f "$pattern" || true
  sleep 3
fi
for name in $running; do
  for trial in "$bench/jobs/$name/"*__*/; do
    [ -d "$trial" ] && [ ! -f "$trial/result.json" ] || continue
    prefix=$(basename "$trial" | tr '[:upper:]' '[:lower:]')
    docker ps -aq --filter "name=^${prefix}__" | xargs -r docker rm -f >/dev/null
    docker network ls -q --filter "name=^${prefix}__" | xargs -r docker network rm >/dev/null
    echo "cut off: $(basename "$trial") (resume.sh $name runs it again)"
  done
done
echo "stopped${job:+ $job}"
