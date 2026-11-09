# Agent benchmarks

Runs termide's headless agent (`termide --prompt … --output stream-json`) on
public agent benchmarks through [Harbor](https://docs.harborframework.com/),
first of all to see where the agent goes wrong and what to optimise, and
second to have numbers to compare with other harnesses on the same model.

Terminal-Bench 2.1 (`terminal-bench/terminal-bench-2-1`, 89 tasks) is the
default dataset; `DATASET=…` picks another Harbor dataset whose tasks run in a
container (the SWE-bench family included; its registry name is not checked yet).

## Layout

| Path | What |
|---|---|
| `harbor_termide/agent.py` | Harbor adapter: uploads the binary, writes the config, runs one task, reads the result |
| `build.sh` | builds this checkout as a static x86_64 musl binary into `bin/` |
| `run.sh` | runs a job: dataset, model, task set, watchdog, `bench-meta.json` |
| `sets/*.txt` | named task subsets, one task name per line |
| `report.py` | one job as a table of outcomes, or two jobs compared |
| `watchdog.sh` | stops a run when OMLX loads another model (see below) |
| `mirror.sh` | local mirror of what the verifiers download (see Setup) |
| `bin/`, `jobs/`, `mirror/` | build output, results, mirrored files; not committed |

## Setup

- Docker for amd64 images (OrbStack or Docker Desktop with Rosetta on Apple
  silicon): the Terminal-Bench images are amd64 only.
- Harbor: `uv tool install harbor` (0.25 or later).
- A model endpoint the containers can reach. The default is OMLX on the host
  at `http://host.docker.internal:10000/v1`, its key read from
  `~/.omlx/settings.json`.

```sh
tools/bench/build.sh          # ~15 min the first time under Rosetta, then incremental
tools/bench/mirror.sh fetch   # once: uv, Python and pytest for the verifiers (~80 MB)
tools/bench/mirror.sh serve   # leave running in a terminal during runs
```

Every verifier installs uv (from GitHub), Python 3.13 (GitHub) and pytest
(PyPI) before its tests; on a slow or flaky network that fails or runs out of
time and the trial gets no verdict. While `mirror.sh serve` answers on port
10001, `run.sh` points the verifiers at it (`UV_DOWNLOAD_URL`,
`UV_PYTHON_INSTALL_MIRROR`, `UV_INDEX_URL`, `PIP_INDEX_URL`), and
`bench-meta.json` records `verifier_mirror`. The files are the same ones (uv
checks their hashes), the tests unchanged; only `apt-get install curl` and the
uv install script still come from the network.

## Running

```sh
tools/bench/run.sh debug8-qwen debug8            # the 8-task smoke set
tools/bench/run.sh tb21-qwen all                 # the whole dataset
MODEL=other-model CONCURRENT=1 tools/bench/run.sh name debug8 --agent-timeout-multiplier 3
AGENT=opencode tools/bench/run.sh debug8-opencode debug8   # another harness, same model
```

`AGENT` takes any agent Harbor has built in (`harbor agent list`): `opencode`
and `qwen-coder` are the two in the published comparison below, `pi`,
`terminus-2`, `codex` and others work the same way. Harbor installs them in
the task container from npm or PyPI at their latest version unless
`--ak version=…` pins one; the version it ran is in each trial's
`result.json` and heads the report's column.

Everything after the set name goes to `harbor run` as is
(`--agent-timeout-multiplier`, `--verifier-timeout-multiplier`, `-k` for
several attempts, `-i`/`-x` for more filters). An interrupted job resumes with
`PYTHONPATH=tools/bench harbor job resume -p tools/bench/jobs/<job>`
(`-f <ErrorType>` reruns the trials that failed with that error). Resuming
keeps the job's settings: Harbor refuses a job whose `config.json` was
edited. To give the trials that got no verdict another go with other limits,
run them as a job of their own and merge it in the report:

```sh
AGENT=opencode tools/bench/run.sh debug8-opencode-r all -i '*/cobol-modernization' \
  --verifier-timeout-multiplier 3
tools/bench/report.py debug8-qwen debug8-opencode+debug8-opencode-r
```

`bench-meta.json` in the job records the termide commit (and whether the tree
was dirty), the model and the dataset, so a result can be traced to the code
that produced it. Rebuild with `build.sh` before a run after a code change:
the run uses whatever `bin/termide` holds.

## Reading the results

```sh
tools/bench/report.py debug8-qwen                 # per trial: outcome, times, tool calls
tools/bench/report.py debug8-qwen debug8-opencode debug8-qwencode   # task × harness matrix
tools/bench/report.py debug8-before debug8-after  # two termide builds the same way
tools/bench/report.py trace debug8-qwen extract-elf   # one trial step by step
harbor view tools/bench/jobs                      # Harbor's own trajectory browser
```

Outcomes: `pass`, `late` (tests passed, but only after the agent's time ran
out; see the pitfalls), `fail` (finished, tests failed), `timeout` (agent out of
time, tests failed), `agent-error` (the agent exited with an error), `env`
(no verdict: the verifier timed out or did not run) and `infra` (the model
server failed). `env` and `infra` say nothing about the agent, so the score is
also given without them. In the matrix a task in bold had different outcomes
in different jobs, and the published scores of the dataset are printed under
it for reference. Tool counts come from termide's own log and are left blank
for other agents. Each trial's `agent/termide.jsonl` is the agent's
event stream: tool calls with their subjects and outputs, messages, the final
result with the run's token usage, every event with its time; `trace` reads
it as a timeline.

## Pitfalls

- **One model in OMLX at a time.** Two large models do not fit in memory; when
  a second one loads (any client asking for it reloads it), generation slows
  to a crawl or fails with Metal out-of-memory errors. `run.sh` starts
  `watchdog.sh`, which stops the run when that happens; unload the other
  model, keep its clients off it, and resume the job.
- **Wall-clock timeouts.** Terminal-Bench times the agent in seconds, not
  tokens, so a slower machine scores lower with the same harness. On a Mac
  with two agents at once and 40–70k-token contexts generation runs at
  15–30 tokens/s, against ~100 for a short single request. For harness
  comparisons run with `--agent-timeout-multiplier` and also report the 1×
  score; say which one a number is.
- **Slow mirrors.** Without `mirror.sh serve`, verifiers time out or fail
  their downloads (`env`); with it, `--verifier-timeout-multiplier` still
  covers a slow `apt-get`. Agents lose minutes in `pip`/`apt` either way: the
  mirror serves the verifiers, not the agents. Harbor's own agents
  install Node.js and their CLI in every container, which overruns the 6-minute
  setup limit on such a network: run them with
  `--agent-setup-timeout-multiplier 4` (termide is uploaded as one binary
  and needs no installing). Setup time does not count against the agent.
- **Harbor leaves a timed-out agent running.** On the agent timeout Harbor
  cancels its side and starts the verifier, but the agent's process in the
  container runs on: it keeps changing files while the tests run and keeps
  the model server busy for the trial next to it. The termide adapter kills
  its process on cancellation; Harbor's own agents run on, so a task they
  finish after their time shows as `late`, which does not count as a pass.
- **A sleeping Mac spoils the trials it interrupts.** Sleep freezes the
  containers and OMLX, and the trial ends as `timeout` or `fail` with
  far more agent time than its limit. `run.sh` holds `caffeinate -is`, which
  prevents idle sleep, and system sleep on AC power only: closing the lid on
  battery still sleeps. Check `pmset -g log | grep -E ' (Sleep|Wake) '` when
  a trial's agent time exceeds its limit, and rerun those trials.
- **Runaway reasoning.** The model sometimes reasons for tens of thousands
  of tokens in one turn (12 minutes with no output, or up to OMLX's
  32768-token cap, `stop` = `length`). One such turn uses up a task's whole
  time; `trace` shows it as a long gap after the last tool result.
- **Old builds log less.** Builds before `usage_total` report only the last
  turn's tokens and no tool outputs or times, so their token column and
  `trace` stay blank.
- **Harbor 0.25** calls no `configure()` hook; the adapter writes its config
  in `install()`.

## Reference numbers

From [GigaCode Universal Agent's comparison](https://habr.com/ru/companies/sberbank/articles/1104670/)
(Sber, November 2026), all on Qwen3.6-35B-A3B; hardware, attempts and
timeouts not stated:

| Benchmark | Universal Agent | OpenCode | Qwen Code |
|---|---|---|---|
| SWE-bench Verified | 69.9% | 66.4% | 65.0% |
| SWE-bench Multilingual | 62.4% | 63.8% | 61.7% |
| SWE-bench Pro | 46.8% | 46.9% | 45.3% |
| Terminal-Bench 2.1 | 65.5% | 62.1% | 41.4% |
