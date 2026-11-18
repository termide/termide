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
| `resume.sh` | takes an interrupted job up again in the background |
| `queue.sh`, `queues/*.txt` | runs a list of jobs one after another in the background |
| `status.sh` | where the running jobs stand; `--watch` keeps redrawing it |
| `sets/*.txt` | named task subsets, one task name per line |
| `report.py` | one job as a table of outcomes, jobs compared, or a matrix slice (`grid`) |
| `conditions.py` | the machine, Docker and model server a job runs under |
| `watchdog.sh` | stops a run when OMLX loads another model; records power and OMLX requests |
| `mirror.sh`, `mirror_server.py` | local mirror of what the verifiers download (see Setup) |
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
uv install script still come from the network. About a quarter of the tasks
also install other packages for their tests (numpy, torch, pandas…) or pin
other versions: `mirror_server.py` serves PyPI's index pages with the
mirrored wheels pointed at the local copies, so those come from PyPI, and a
Python the mirror lacks (3.11) is redirected to its GitHub release. torch and
torchvision come from PyTorch's CPU-only index: the default wheels pull about
3 GB of CUDA libraries the verifiers cannot use, more than a slow line fetches
within a verifier's 15 minutes.

## Running

```sh
tools/bench/run.sh debug8-qwen debug8            # the 8-task smoke set
tools/bench/run.sh sub18-qwen sub18              # 18 tasks for measuring a change
tools/bench/run.sh tb21-qwen all                 # the whole dataset
MODEL=other-model tools/bench/run.sh name debug8 --agent-timeout-multiplier 3
AGENT=opencode tools/bench/run.sh debug8-opencode debug8   # another harness, same model
```

`AGENT` takes any agent Harbor has built in (`harbor agent list`): `opencode`
and `qwen-coder` are the two in the published comparison below, `pi`,
`terminus-2`, `codex` and others work the same way. Harbor installs them in
the task container from npm or PyPI, at their latest version unless
`--ak version=…` pins one. `run.sh` pins the harnesses of the published
comparisons (`claude-code`, `opencode`, `qwen-coder`, and the ACP adapter of
`termide-claude-code`), so that every job of a comparison runs the same code;
`AGENT_VERSION` overrides a pin. The version a trial ran is in its
`result.json` and heads the report's column, the pin in `bench-meta.json`.

`AGENT=claude-code` runs Claude Code on a Claude subscription rather than an
endpoint: `MODEL` is a Claude model's full id (`claude-sonnet-5-5`), and the
login is the OAuth token `claude setup-token` prints, taken from
`CLAUDE_CODE_OAUTH_TOKEN` or the login keychain (`security
add-generic-password -a "$USER" -s termide-bench-claude-oauth -w`, which asks
for it). The model is not on this machine, but the tasks are: run these one
at a time too, since trials side by side share its CPUs and memory against
their wall-clock limits.

`AGENT=termide-claude-code` runs termide on the same subscription the way the
app does: termide drives Claude Code over ACP, so the model works through
termide's system prompt and tools inside Claude Code's loop. The container
gets Node 22 and the `@agentclientprotocol/claude-agent-acp` adapter at its
pin, whose Claude Agent SDK bundles the Claude Code version bare
`claude-code` is pinned to, and the model reaches it as `ANTHROPIC_MODEL`, as
bare Claude Code takes `--model`. The adapter offers an older model such as
`claude-sonnet-5` only when that variable names it. The job records the
harness as `termide`, the connection as `claude subscription`, and the
reasoning as `agent-default`: termide sets no effort on an ACP agent, so the
pair with `claude-code` compares the harnesses at Claude Code's default
effort.

Everything after the set name goes to `harbor run` as is
(`--agent-timeout-multiplier`, `--verifier-timeout-multiplier`, `-k` for
several attempts, `-i`/`-x` for more filters).

A full run takes a day or two on a local model, and it lives only as long as
the terminal it was started in, the program running that terminal, and the
machine. After a reboot or a closed terminal, `tools/bench/resume.sh <job>`
takes the job up again: it drops the trial that was cut off (its containers
and directory) so that it runs again from the start, starts the mirror if it
is down, and resumes in the background, detached from the terminal, logging
to `jobs/<job>.out`. It is
`PYTHONPATH=tools/bench harbor job resume -p tools/bench/jobs/<job>`
underneath (`-f <ErrorType>` reruns the trials that failed with that error).
Resuming keeps the job's settings: Harbor refuses a job whose `config.json`
was edited. To give the trials that got no verdict another go with other
limits, run them as a job of their own and merge it in the report:

```sh
AGENT=opencode tools/bench/run.sh debug8-opencode-r all -i '*/cobol-modernization' \
  --verifier-timeout-multiplier 3
tools/bench/report.py debug8-qwen debug8-opencode+debug8-opencode-r
```

A comparison of several jobs is a queue: `tools/bench/queue.sh
queues/<name>.txt` runs the jobs a file lists, one line each (`job set
VAR=value… harbor args…`), one after another in the background, logging to
`jobs/queue-<name>.out`. `queues/tb21-full.txt` is the full comparison. A
finished job is skipped and an interrupted one resumed, so after a reboot
the same command takes the queue up where it stopped. Before each job it
checks that Docker still runs amd64 containers: when the Docker VM's Rosetta
breaks, every task fails at once (`exec /bin/sh: no such file or
directory`), and `orb stop && orb start` mends it. A job that ends
unfinished stops the queue, so `stop.sh` stops a queue too.

A job started this way belongs to no terminal, so nothing in termide shows
it. `tools/bench/status.sh` does. On top are the conditions: the machine, how
many trials run at once, the agent's time multiplier, the power source and the
model's speed over the last 10 minutes. Then come the score so far and every
task in the order it ran: `✓` passed, `✗` failed (with why, and how many of
its tests passed), `⚠` broken by the environment rather than the agent, `▶`
running, `◷` pending. The running task shows its phase (environment, agent
setup, agent, verifier), the agent's time against its limit and termide's
last step, read live from the task container. Times are the agent's own,
from its start on the task: the container's start, the agent's install and
the verifier are not in them.
`status.sh --watch` redraws it every 15 seconds; a terminal panel running it
follows the job, and closing the panel leaves the job alone.

`bench-meta.json` in the job records the termide commit (and whether the tree
was dirty), the model and the dataset, so a result can be traced to the code
that produced it. Rebuild with `build.sh` before a run after a code change:
the run uses whatever `bin/termide` holds.

## Reading the results

```sh
tools/bench/report.py debug8-qwen                 # per trial: outcome, times, tool calls
tools/bench/report.py debug8-qwen debug8-opencode debug8-qwencode   # task × harness matrix
tools/bench/report.py debug8-before debug8-after  # two termide builds the same way
tools/bench/report.py grid sub18 --model Qwen3.8-Flash-Next-oQ4e-mtp   # harnesses on one model
tools/bench/report.py grid sub18 --agent termide  # termide on each model and connection
tools/bench/report.py trace debug8-qwen extract-elf   # one trial step by step
harbor view tools/bench/jobs                      # Harbor's own trajectory browser
```

Outcomes: `pass`, `late` (tests passed, but only after the agent's time ran
out; see the pitfalls), `fail` (finished, tests failed), `timeout` (agent out of
time, tests failed), `agent-error` (the agent exited with an error), `env`
(no verdict: the verifier timed out or did not run) and `infra` (the model
server failed). `env` and `infra` say nothing about the agent, so the score is
also given without them. In the matrix a task in bold had different outcomes
in different jobs, and under a table of Qwen3.6-35B-A3B the published scores
(below) are printed for reference. Tool counts come from termide's own log and are left blank
for other agents. Each trial's `agent/termide.jsonl` is the agent's
event stream: tool calls with their subjects and outputs, messages, the final
result with the run's token usage, every event with its time; `trace` reads
it as a timeline.

### Matrices and conditions

Every job is a point in harness × model and connection × task set, taken
under some conditions. `grid` cuts one slice of them: `--model` fixes the
model and puts the harnesses side by side, `--agent` fixes the harness and
puts the models and connections (OMLX, OpenRouter, …) side by side. It finds
the jobs by their `bench-meta.json`: the latest job that ran the set fills a
cell, and a later job of the same cell on some of its tasks (a rerun) replaces
their trials. Cells nothing has filled yet are listed as not run.

Terminal-Bench limits the agent in seconds, so a score compares only with one
taken under the same conditions. `run.sh` records them in `bench-meta.json`
(`conditions.py` prints them): the machine (chip, memory, OS; never its host
name, since reports get published — `BENCH_MACHINE=<name>` names it when the
hardware alone does not tell two machines apart), Docker's
CPUs and memory, the model server, its version and the settings that change
its speed. `grid` makes a table per profile — the machine, the trials at once
and the agent's time multiplier — and lists under it the conditions that
differ between its columns: another Docker limit, another server version,
another termide build.

While a job runs, `watchdog.sh` writes what the trials ran into:
`power.log`, the power source every minute (a gap is the machine asleep), and
`requests.log`, OMLX's line for each request, which outlives OMLX's own logs.
Under each table, per column:

- `tok/s out`: tokens written per second of request, prompt processing
  included: the speed the model was served at.
- `overlapping requests`: trials in which one request started before the
  previous one ended — another client on the same model, or a harness's own
  parallel calls (title generation, subagents). Blank when trials ran side by
  side.
- `on battery`, `slept`: trials that drew on the battery or that the machine
  slept through; the latter are spoiled, rerun them.

Jobs from before 2026-11-17 have only their machine filled in afterwards,
and their speed only while OMLX keeps its logs (a few days).

## Pitfalls

- **One model in OMLX at a time.** Two large models do not fit in memory; when
  a second one loads (any client asking for it reloads it), generation slows
  to a crawl or fails with Metal out-of-memory errors. `run.sh` starts
  `watchdog.sh`, which stops the run when that happens; unload the other
  model, keep its clients off it, and resume the job.
- **Wall-clock timeouts.** Terminal-Bench times the agent in seconds, not
  tokens, so a slower machine scores lower with the same harness. On a Mac
  with two agents at once and 40–70k-token contexts generation runs at
  15–30 tokens/s, against ~100 for a short single request: the two share
  the server's generation for little more throughput in all, and one's
  prompt processing stalls the other. `run.sh` therefore runs one trial at
  a time unless `CONCURRENT` says otherwise, and a comparison keeps it so
  for every harness, a hosted model's too: trials side by side also share
  the machine's CPUs, memory and Docker. For harness
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
- **Reasoning across harnesses.** On a local server termide's level goes in
  the chat template's `enable_thinking`, on for every level but `off`: a
  Qwen model reasons at `high` as it does by default, and `opencode` and
  `qwen-coder`, which send no level, get the same default. Only termide sends
  the model's reasoning back with the turn (`preserve_thinking`), which is
  the harness's own doing. On the Claude subscription neither harness sets an
  effort (`agent-default`).
- **Runaway reasoning.** The model sometimes reasons for tens of thousands
  of tokens in one turn (12 minutes with no output, or up to OMLX's
  32768-token cap, `stop` = `length`). One such turn uses up a task's whole
  time; `trace` shows it as a long gap after the last tool result.
  termide asks again without reasoning when a reply reasoned to the limit;
  `REASONING_BUDGET=12000` cuts such a call earlier, and `trace` marks it
  `✂ reasoning ran past its budget`.
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
