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
  # A session of its own: nothing that ends the caller's process group (a
  # terminal closing, termide quitting) reaches the job.
  BENCH_DETACHED=1 nohup python3 -I -c 'import os, sys
try: os.setsid()
except OSError: pass
os.execvp(sys.argv[1], sys.argv[1:])' \
    "$0" "$job" >>"$bench/jobs/$job.out" 2>&1 </dev/null &
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

# Harbor scrubs the secrets out of the config of a trial once it ran, leaving
# `[REDACTED]` or `sk-a****gAA` where the job's config keeps its `${VAR}`
# reference; a further resume then finds the trial unlike the plan and refuses
# the job ("Existing trial config does not match planned job config"). Put
# the job's value back.
python3 -I - "$dir" <<'EOF'
import json, sys
from pathlib import Path
job = Path(sys.argv[1])
planned = (json.loads((job / "config.json").read_text()).get("agents") or [{}])[0].get("env") or {}
for path in job.glob("*__*/config.json"):
    config = json.loads(path.read_text())
    env = (config.get("agent") or {}).get("env") or {}
    fixed = {k: planned[k] for k, v in env.items()
             if k in planned and ("[REDACTED]" in v or "****" in v)}
    if fixed:
        env.update(fixed)
        path.write_text(json.dumps(config, indent=4))
        print(f"resume.sh: restored {', '.join(fixed)} in {path.parent.name}")
EOF

# Harbor keeps a secret in the job's config as `${VAR}` and pulls the value
# from the host environment: put them back here, from the sources run.sh
# used. Without them harbor resumes with an empty key and every trial fails.
cfg="$dir/config.json"
case "$(cat "$cfg")" in *'${CLAUDE_CODE_OAUTH_TOKEN}'*)
  export CLAUDE_CODE_OAUTH_TOKEN=${CLAUDE_CODE_OAUTH_TOKEN:-$(security find-generic-password -s termide-bench-claude-oauth -w 2>/dev/null || true)}
  [ -n "$CLAUDE_CODE_OAUTH_TOKEN" ] || { echo "no Claude token in the keychain (termide-bench-claude-oauth)" >&2; exit 2; }
  ;;
esac
case "$(cat "$cfg")" in *'${CLAUDE_FORCE_OAUTH}'*) export CLAUDE_FORCE_OAUTH=1 ;; esac
case "$(cat "$cfg")" in *'${TERMIDE_API_KEY}'*)
  export TERMIDE_API_KEY=${TERMIDE_API_KEY:-$(python3 -I -c 'import json,os;print(json.load(open(os.path.expanduser("~/.omlx/settings.json")))["auth"]["api_key"])' 2>/dev/null || true)}
  [ -n "$TERMIDE_API_KEY" ] || { echo "no OMLX key in ~/.omlx/settings.json" >&2; exit 2; }
  ;;
esac

# Harbor's own agents (opencode, qwen-coder) read the endpoint key from
# OPENAI_API_KEY, as run.sh exports it; a job on the local OMLX takes it from there.
case "$(cat "$dir/bench-meta.json" 2>/dev/null)" in
  *:10000/*)
    if [ -z "${OPENAI_API_KEY:-}" ]; then
      OPENAI_API_KEY=$(python3 -I -c 'import json,os;print(json.load(open(os.path.expanduser("~/.omlx/settings.json")))["auth"]["api_key"])' 2>/dev/null || true)
      export OPENAI_API_KEY
    fi
    ;;
esac

port=${MIRROR_PORT:-10001}
if [ -d "$bench/mirror" ] && ! curl -sf -o /dev/null "http://127.0.0.1:$port/simple/pytest/"; then
  echo "$(date +%T) resume.sh: starting the mirror"
  nohup "$bench/mirror.sh" serve >>"$bench/jobs/mirror.log" 2>&1 &
fi

keep=""
case "$(cat "$dir/bench-meta.json" 2>/dev/null)" in
  *:10000/*) keep=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1])).get("model",""))' "$dir/bench-meta.json" 2>/dev/null || true) ;;
esac
sh "$bench/watchdog.sh" "$keep" $$ "$dir" >>"$dir/watchdog.log" 2>&1 &

cd "$bench"
export PYTHONPATH="$bench"
awake=""
command -v caffeinate >/dev/null && awake="caffeinate -is"
$awake harbor job resume -p "$dir"
