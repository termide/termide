#!/bin/sh
# Run an agent on a Harbor dataset (Terminal-Bench 2.1 by default): termide,
# or any agent Harbor has built in, for comparison on the same model.
#
#   tools/bench/run.sh <job-name> [set-name] [harbor run args...]
#
# set-name names a file in sets/ (one task name per line); without it, or
# with `all`, the whole dataset runs. Results go to jobs/<job-name>/, with
# bench-meta.json recording the agent, the termide commit, the model and the
# conditions (machine, Docker, model server; see conditions.py). While it
# runs, watchdog.sh samples the power source and copies OMLX's request log.
#
# Environment:
#   AGENT     termide (default) or a Harbor agent: opencode, qwen-coder,
#             pi, terminus-2... (`harbor agent list`)
#   MODEL     model id as the endpoint names it (Qwen3.6-35B-A3B-8bit)
#   BASE_URL  endpoint as the container sees it (OMLX on the host)
#   API_KEY   key for it; for OMLX it is read from ~/.omlx/settings.json
#   DATASET   Harbor dataset (terminal-bench/terminal-bench-2-1)
#   CONCURRENT  trials at once (1): a local model server splits its
#               generation between them, so each runs slower against its
#               wall-clock limit for little more throughput
#   REASONING termide's reasoning level (high, its default): off, minimal,
#             low, medium, high, xhigh, max
#   REASONING_BUDGET  tokens one call of termide's may reason before it is
#             cut and asked again without reasoning (0, the default: no limit)
#   MIRROR_PORT the port of mirror.sh serve (10001); while it answers, the
#               verifiers take uv, Python and pytest from it
set -eu
bench=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$bench/../.." && pwd)
job=${1:?job name}
shift
set_file=""
set_name=all
if [ $# -gt 0 ] && [ "${1#-}" = "$1" ]; then
  if [ "$1" != all ]; then
    set_name=$1
    set_file="$bench/sets/$1.txt"
    [ -f "$set_file" ] || { echo "no set $set_file" >&2; exit 2; }
  fi
  shift
fi
# Each task of the set becomes `-i */<task>`, ahead of the caller's args.
if [ -n "$set_file" ]; then
  tasks=$(grep -v -e '^#' -e '^$' "$set_file")
  for task in $tasks; do set -- -i "*/$task" "$@"; done
fi

AGENT=${AGENT:-termide}
MODEL=${MODEL:-Qwen3.6-35B-A3B-8bit}
BASE_URL=${BASE_URL:-http://host.docker.internal:10000/v1}
DATASET=${DATASET:-terminal-bench/terminal-bench-2-1}
CONCURRENT=${CONCURRENT:-1}
REASONING=${REASONING:-high}
REASONING_BUDGET=${REASONING_BUDGET:-0}
omlx=false
case "$BASE_URL" in *:10000/*) omlx=true ;; esac
if [ -z "${API_KEY:-}" ] && $omlx; then
  API_KEY=$(python3 -I -c 'import json,os;print(json.load(open(os.path.expanduser("~/.omlx/settings.json")))["auth"]["api_key"])')
fi
if [ "$AGENT" = termide ]; then
  [ -x "$bench/bin/termide" ] || { echo "run $bench/build.sh first" >&2; exit 2; }
  set -- -a harbor_termide.agent:TermideAgent \
    --ae "TERMIDE_MODEL=$MODEL" \
    --ae "TERMIDE_BASE_URL=$BASE_URL" \
    --ae "TERMIDE_API_KEY=$API_KEY" \
    --ae "TERMIDE_REASONING=$REASONING" \
    --ae "TERMIDE_REASONING_BUDGET=$REASONING_BUDGET" \
    "$@"
else
  # Harbor's own agents read the key from this variable and take the
  # endpoint from --base-url.
  export OPENAI_API_KEY="$API_KEY"
  set -- -a "$AGENT" --base-url "$BASE_URL" "$@"
fi

# Verifiers install uv, Python and pytest on every trial; with the local
# mirror up they take them from the host instead of GitHub and PyPI.
mirror=false
port=${MIRROR_PORT:-10001}
if curl -sf -o /dev/null "http://127.0.0.1:$port/simple/pytest/"; then
  mirror=true
  m="http://host.docker.internal:$port"
  set -- --ve "UV_DOWNLOAD_URL=$m/uv/0.9.5" \
    --ve "UV_PYTHON_INSTALL_MIRROR=$m/python" \
    --ve "UV_INDEX_URL=$m/simple/" \
    --ve "PIP_INDEX_URL=$m/simple/" \
    --ve "PIP_TRUSTED_HOST=host.docker.internal" \
    "$@"
fi

mkdir -p "$bench/jobs/$job"
python3 -I - "$bench/jobs/$job/bench-meta.json" "$root" "$AGENT" "$MODEL" "$BASE_URL" "$DATASET" "$CONCURRENT" "$mirror" "$REASONING" "$REASONING_BUDGET" "$set_name" <<'EOF'
import json, subprocess, sys
out, root, agent, model, base_url, dataset, concurrent, mirror, reasoning, budget, set_name = sys.argv[1:]
sys.path.insert(0, root + "/tools/bench")
import conditions
git = lambda *a: subprocess.run(["git", "-C", root, *a], capture_output=True, text=True).stdout.strip()
meta = {"agent": agent}
if agent == "termide":
    meta |= {
        "commit": git("rev-parse", "HEAD"),
        "dirty": bool(git("status", "--porcelain", "--untracked-files=no")),
        "describe": git("describe", "--tags", "--always", "--dirty"),
        "reasoning": reasoning,
        "reasoning_budget": int(budget),
    }
meta |= {"model": model, "base_url": base_url, "connection": conditions.connection(base_url),
         "dataset": dataset, "set": set_name, "concurrent": int(concurrent),
         "verifier_mirror": mirror == "true", "conditions": conditions.static(base_url)}
json.dump(meta, open(out, "w"), indent=2)
EOF

# Two models do not fit in OMLX at once: the watchdog stops the run if
# another one loads. It also records the power source and OMLX's requests.
keep=""
$omlx && keep=$MODEL
sh "$bench/watchdog.sh" "$keep" $$ "$bench/jobs/$job" >>"$bench/jobs/$job/watchdog.log" 2>&1 &

cd "$bench"
export PYTHONPATH="$bench"
# A Mac that sleeps freezes the containers and the model server mid-trial,
# while the agent timeout keeps counting: keep it awake for the run.
awake=""
command -v caffeinate >/dev/null && awake="caffeinate -is"
$awake harbor run -y \
  -d "$DATASET" \
  -m "openai/$MODEL" \
  -n "$CONCURRENT" \
  -o "$bench/jobs" --job-name "$job" \
  "$@"
