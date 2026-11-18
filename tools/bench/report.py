#!/usr/bin/env python3
"""Summarise Harbor jobs as Markdown.

    report.py <job>              one job: every trial and why it ended
    report.py <job> <job>...     a matrix: each task's outcome in each job,
                                 to compare harnesses or two termide builds;
                                 tasks where the jobs differ are in bold;
                                 <job>+<rerun> takes the rerun's trials in
                                 place of the job's for the tasks it ran
    report.py grid <set> --model <model>   the harnesses on one model
    report.py grid <set> --agent <agent>   one harness on each model and
                                 connection; both take the latest job of each
                                 cell that ran <set> (a file in sets/, or
                                 `all`), one table per profile of conditions
                                 (machine, trials at once, agent time), with
                                 the conditions that differ within it listed
    report.py status [<job>...]  where the running jobs (or the given ones)
                                 stand: progress, the trial that runs, its
                                 phase and time, its agent's last step, the
                                 model's last request, the power
    report.py trace <job> <task> one termide trial step by step: each call
                                 with its time, failed calls with their output,
                                 how much it reasoned, loops cut
    report.py thinking <job> <task>  that trial's reasoning, as written, and
                                 the arguments of its long calls

<job> is a name under tools/bench/jobs/ or a path. Outcomes:

    pass          reward 1 within the agent's time
    late          reward 1, but the agent ran out of time: Harbor cancels
                  the agent and verifies, yet leaves its process running in
                  the container, so the work may have been finished during
                  the verification. Not counted as a pass. (The termide
                  adapter kills its process; other agents' runs on.)
    fail          the agent finished, the tests failed
    timeout       the agent ran out of time and the tests failed
    agent-error   the agent exited with an error (provider, crash)
    env           the verifier or the environment failed, not the agent:
                  verifier timeout, image build, setup
    infra         the model server failed (Metal OOM and the like)
    running       no result yet

`env` and `infra` say nothing about the agent, so the score is also given
without them. Tool counts come from termide's event log and are blank for
other agents.
"""

import json
import re
import subprocess
import sys
from datetime import datetime, timedelta
from pathlib import Path

BENCH = Path(__file__).resolve().parent
ORDER = ["pass", "late", "fail", "timeout", "agent-error", "env", "infra", "running"]
INFRA_MARKERS = ("[METAL]", "Insufficient Memory", "OutOfMemory")
# Lines a verifier prints when installing its test runner failed.
VERIFIER_SETUP_FAILURES = ("uvx: command not found", "pytest: command not found", "failed to download")
# Published Terminal-Bench 2.1 scores to read a table of the same model
# against: GigaCode Universal Agent's comparison,
# https://habr.com/ru/companies/sberbank/articles/1104670/
REFERENCE_MODEL = "Qwen3.6-35B-A3B"
REFERENCE = "Universal Agent 65.5% · OpenCode 62.1% · Qwen Code 41.4%"


def job_dir(arg: str) -> Path:
    path = Path(arg)
    return path if path.is_dir() else BENCH / "jobs" / arg


def parse_time(s: str) -> datetime:
    """An aware time from Harbor's ISO strings; the naive ones are local."""
    t = datetime.fromisoformat(s.replace("Z", "+00:00"))
    return t if t.tzinfo else t.astimezone()


def window(phase: dict | None) -> tuple[datetime, datetime] | None:
    if not phase or not phase.get("started_at") or not phase.get("finished_at"):
        return None
    return parse_time(phase["started_at"]), parse_time(phase["finished_at"])


def seconds(phase: dict | None) -> float | None:
    w = window(phase)
    return None if w is None else (w[1] - w[0]).total_seconds()


def events(trial: Path) -> list[dict]:
    log = trial / "agent" / "termide.jsonl"
    if not log.exists():
        return []
    out = []
    for line in log.read_text(errors="replace").splitlines():
        try:
            out.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    return out


def verifier_broke(trial: Path) -> str | None:
    """Why the verifier could not run its tests, when its own setup failed
    (a download, the test runner missing): the reward 0 it then reports says
    nothing about the agent."""
    log = trial / "verifier" / "test-stdout.txt"
    if not log.exists():
        return None
    text = log.read_text(errors="replace")
    return next((marker for marker in VERIFIER_SETUP_FAILURES if marker in text), None)


def verifier_timed_out(trial: Path) -> bool:
    log = trial / "trial.log"
    return log.exists() and "VerifierTimeoutError" in log.read_text(errors="replace")


def token_total(final: dict) -> tuple[int, int] | None:
    """(input + cache, output) tokens of the whole run, from builds that
    report `usage_total`."""
    usage = final.get("usage_total")
    if not usage:
        return None
    return usage["input"] + usage["cache_read"] + usage["cache_write"], usage["output"]


def agent_name(info: dict) -> str:
    """`name version` from Harbor's agent_info; termide's version already has its name."""
    name, version = info.get("name") or "", info.get("version") or ""
    return version if version.startswith(name) else f"{name} {version}".strip()


def trial_row(trial: Path) -> dict:
    result_path = trial / "result.json"
    result = json.loads(result_path.read_text()) if result_path.exists() else {}
    evs = events(trial)
    final = next((e for e in reversed(evs) if e.get("type") == "result"), {})
    tools = [e for e in evs if e.get("type") == "tool_use"]
    tool_errors = sum(1 for e in evs if e.get("type") == "tool_result" and e.get("error"))
    rewards = (result.get("verifier_result") or {}).get("rewards") or {}
    reward = rewards.get("reward")
    exc = (result.get("exception_info") or {}).get("exception_type") or ""
    agent_error = final.get("error") or ""

    if not result.get("finished_at"):
        outcome = "running"
    elif reward == 1:
        outcome = "late" if exc == "AgentTimeoutError" else "pass"
    elif any(m in agent_error for m in INFRA_MARKERS):
        outcome = "infra"
    elif exc == "NonZeroAgentExitCodeError" or agent_error:
        outcome = "agent-error"
    elif reward is None:
        # No verdict: the verifier timed out or failed to run, so whether
        # the agent solved the task is unknown, even when it ran out of time.
        outcome = "env"
    elif broke := verifier_broke(trial):
        outcome = "env"
        exc = f"verifier setup: {broke}"
    elif exc == "AgentTimeoutError":
        outcome = "timeout"
    else:
        outcome = "fail"
    if outcome == "env" and verifier_timed_out(trial):
        exc = "VerifierTimeoutError"

    info = result.get("agent_info") or {}
    return {
        "task": result.get("task_name", trial.name).split("/")[-1]
        if result
        else trial.name.rsplit("__", 1)[0],
        "agent": agent_name(info),
        "outcome": outcome,
        "reward": reward,
        "agent_s": seconds(result.get("agent_execution")),
        "window": window(result.get("agent_execution")),
        "verifier_s": seconds(result.get("verifier")),
        "tokens": token_total(final),
        "tools": len(tools) if evs else None,
        "tool_errors": tool_errors,
        "stop": final.get("stop_reason") or "",
        "exception": exc,
        "error": agent_error,
    }


def load(job: Path) -> list[dict]:
    rows = [trial_row(t) for t in sorted(job.iterdir()) if (t / "config.json").exists() and t.is_dir()]
    for r in rows:
        r["job"] = job
    return sorted(rows, key=lambda r: (ORDER.index(r["outcome"]), r["task"]))


def fmt_s(value: float | None) -> str:
    return "" if value is None else f"{value / 60:.1f}m"


def score(rows: list[dict]) -> str:
    done = [r for r in rows if r["outcome"] != "running"]
    passed = sum(r["outcome"] == "pass" for r in done)
    clean = [r for r in done if r["outcome"] not in ("env", "infra")]
    parts = [f"**{passed}/{len(done)}** passed ({100 * passed / max(len(done), 1):.1f}%)"]
    if len(clean) != len(done):
        parts.append(
            f"{passed}/{len(clean)} ({100 * passed / max(len(clean), 1):.1f}%) without env/infra failures"
        )
    counts = {o: sum(r["outcome"] == o for r in rows) for o in ORDER}
    parts.append(", ".join(f"{o} {n}" for o, n in counts.items() if n))
    return " · ".join(parts)


def meta(job: Path) -> dict:
    """bench-meta.json as run.sh wrote it, else what Harbor's job config says."""
    path = job / "bench-meta.json"
    if path.exists():
        return json.loads(path.read_text())
    config = job_config(job)
    if not config:
        return {}
    agent = (config.get("agents") or [{}])[0]
    return {
        "model": (agent.get("model_name") or "").split("/", 1)[-1],
        "dataset": ((config.get("datasets") or [{}])[0]).get("name", ""),
        "concurrent": config.get("n_concurrent_trials"),
    }


def job_config(job: Path) -> dict:
    path = job / "config.json"
    return json.loads(path.read_text()) if path.exists() else {}


def task_names(job: Path) -> frozenset[str] | None:
    """The tasks the job was limited to, or None for the whole dataset."""
    names = ((job_config(job).get("datasets") or [{}])[0]).get("task_names") or []
    return frozenset(n.split("/")[-1] for n in names) or None


def set_file_tasks(name: str) -> frozenset[str]:
    path = BENCH / "sets" / f"{name}.txt"
    lines = path.read_text().splitlines() if path.exists() else []
    return frozenset(line.strip() for line in lines if line.strip() and not line.startswith("#"))


def task_set(job: Path) -> str:
    """The set in sets/ the job ran, `all`, or `custom` (a rerun's few tasks)."""
    if name := meta(job).get("set"):
        return name
    names = task_names(job)
    if names is None:
        return "all"
    for path in sorted((BENCH / "sets").glob("*.txt")):
        if set_file_tasks(path.stem) == names:
            return path.stem
    return "custom"


def multipliers(job: Path) -> dict[str, float]:
    """Harbor's timeout multipliers the job was started with, 1 when absent."""
    config = job_config(job)
    return {
        kind: float(config.get(f"{kind}_timeout_multiplier") or 1)
        for kind in ("agent", "agent_setup", "verifier")
    }


def agent_key(job: Path) -> str:
    """The harness: termide or the name of Harbor's agent."""
    if agent := meta(job).get("agent"):
        return agent
    name = ((job_config(job).get("agents") or [{}])[0]).get("name") or "?"
    return "termide" if name.endswith(":TermideAgent") else name


def connection(job: Path) -> str:
    m = meta(job)
    if m.get("connection"):
        return m["connection"]
    sys.path.insert(0, str(BENCH))
    import conditions

    return conditions.connection(m.get("base_url") or "") if m.get("base_url") else "?"


def machine(job: Path) -> dict:
    return (meta(job).get("conditions") or {}).get("machine") or {}


def profile(job: Path) -> str:
    """The conditions a score depends on and is only comparable under: the
    machine (the containers run there, and a local model too), how many
    trials ran at once, and how long the agent was given."""
    mach = machine(job)
    name = mach.get("name") or (f"{mach['chip']} · {mach['memory_gb']} GB" if mach.get("chip") else "machine not recorded")
    agent_x = multipliers(job)["agent"]
    return f"{name} · {meta(job).get('concurrent') or 1} at once · agent time ×{agent_x:g}"


def details(job: Path) -> dict[str, str]:
    """The conditions that may differ within one profile: shown when they do."""
    m = meta(job)
    cond = m.get("conditions") or {}
    mach, dock, serv = cond.get("machine") or {}, cond.get("docker") or {}, cond.get("server") or {}
    x = multipliers(job)
    return {
        "os": mach.get("os") or "not recorded",
        "docker": f"{dock['cpus']} CPU, {dock['memory_gb']} GB, {dock['version']}" if dock else "not recorded",
        "server": " ".join(str(v) for v in (serv.get("kind"), serv.get("version")) if v) or connection(job),
        "server settings": json.dumps(serv.get("settings"), sort_keys=True) if serv.get("settings") else "not recorded",
        "termide": f"{m['describe']} · {m.get('reasoning')}" + (f" ≤{m['reasoning_budget']}" if m.get("reasoning_budget") else "")
        if m.get("describe") else "—",
        "verifier time": f"×{x['verifier']:g}" + (" with mirror" if m.get("verifier_mirror") else ""),
        "setup time": f"×{x['agent_setup']:g}",
    }


def started(job: Path) -> datetime:
    path = job / "result.json"
    if path.exists() and (at := json.loads(path.read_text()).get("started_at")):
        return parse_time(at)
    return datetime.fromtimestamp(job.stat().st_mtime).astimezone()


def agent_label(job: Path, rows: list[dict]) -> str:
    """The agent and its version: termide by commit, others as Harbor saw them."""
    m = meta(job)
    if m.get("describe"):
        reasoning = f" · {m['reasoning']}" if m.get("reasoning") else ""
        if m.get("reasoning_budget"):
            reasoning += f" ≤{m['reasoning_budget']}"
        return f"termide {m['describe']}{reasoning}"
    return next((r["agent"] for r in rows if r["agent"]), m.get("agent", "?"))


def meta_line(job: Path) -> str:
    m = meta(job)
    if not m:
        return ""
    return (
        f"{agent_label(job, load(job))} · model `{m.get('model')}` · {connection(job)} · "
        f"{m.get('dataset')} · {profile(job)}"
    )


def report_one(job: Path) -> None:
    rows = load(job)
    print(f"# {job.name}\n")
    if line := meta_line(job):
        print(line + "\n")
    print(score(rows) + "\n")
    print("| task | outcome | agent | verifier | tokens in/out | tools (err) | stop | note |")
    print("|---|---|---|---|---|---|---|---|")
    for r in rows:
        note = r["error"][:80] if r["error"] else r["exception"]
        tokens = f"{r['tokens'][0] / 1000:.0f}k / {r['tokens'][1] / 1000:.1f}k" if r["tokens"] else ""
        print(
            f"| {r['task']} | {r['outcome']} | {fmt_s(r['agent_s'])} | {fmt_s(r['verifier_s'])} "
            f"| {tokens} | {tools_cell(r)} | {r['stop']} | {note} |"
        )
    print()
    for key, value in measured(rows, meta(job).get("concurrent") or 1).items():
        print(f"- {key}: {value}")


# One line of OMLX's server log per request: when it ended, the model, the
# tokens it wrote and how long it took from arrival (prompt processing
# included).
REQUEST = re.compile(
    r"^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d),\d+ .*Chat completion: model=([^,]+), (\d+) tokens in ([\d.]+)s"
)
OMLX_LOGS = Path.home() / ".omlx" / "logs"
_log_cache: dict[Path, list[tuple[str, datetime, datetime, int]]] = {}


def read_requests(path: Path) -> list[tuple[str, datetime, datetime, int]]:
    if path not in _log_cache:
        out = []
        for line in path.read_text(errors="replace").splitlines():
            if m := REQUEST.match(line):
                end = datetime.fromisoformat(m[1]).astimezone()
                out.append((m[2], end - timedelta(seconds=float(m[4])), end, int(m[3])))
        _log_cache[path] = out
    return _log_cache[path]


def requests(job: Path) -> list[tuple[datetime, datetime, int]]:
    """The job's model's requests to OMLX: from the copy watchdog.sh keeps in
    the job, else from OMLX's own logs while they last."""
    if connection(job) != "omlx":
        return []
    own = job / "requests.log"
    files = [own] if own.exists() else sorted(OMLX_LOGS.glob("server.log*"))
    model = meta(job).get("model")
    return sorted((s, e, n) for f in files for (mdl, s, e, n) in read_requests(f) if mdl == model)


def power(job: Path) -> list[tuple[datetime, str]]:
    path = job / "power.log"
    if not path.exists():
        return []
    out = []
    for line in path.read_text().splitlines():
        stamp, _, source = line.partition(" ")
        if stamp.isdigit():
            out.append((datetime.fromtimestamp(int(stamp)).astimezone(), source))
    return out


SLEEP_GAP = timedelta(minutes=3)  # watchdog.sh samples every minute


def measured(rows: list[dict], concurrent: int = 1) -> dict[str, str]:
    """What the trials ran into, measured as they ran: the speed OMLX served,
    the trials whose requests overlapped (another client, or the harness's
    own parallel calls), the trials on battery and the ones the machine
    slept through."""
    tokens = busy = 0.0
    contended = battery = slept = timed = powered = 0
    by_job: dict[Path, tuple[list, list]] = {}
    for r in rows:
        if r["window"] is None:
            continue
        job = r["job"]
        if job not in by_job:
            by_job[job] = (requests(job), power(job))
        reqs, samples = by_job[job]
        start, end = r["window"]
        inside = [(s, e, n) for s, e, n in reqs if start <= e <= end + timedelta(seconds=5)]
        if inside:
            timed += 1
            tokens += sum(n for _, _, n in inside)
            busy += sum((e - s).total_seconds() for s, e, _ in inside)
            if any(b[0] < a[1] - timedelta(seconds=1) for a, b in zip(inside, inside[1:])):
                contended += 1
        near = [(t, src) for t, src in samples if start - SLEEP_GAP <= t <= end + SLEEP_GAP]
        if near and near[0][0] <= start and near[-1][0] >= end:
            powered += 1
            battery += any("Battery" in src for t, src in near if start <= t <= end)
            slept += any(b[0] - a[0] > SLEEP_GAP for a, b in zip(near, near[1:]))
    n = sum(r["window"] is not None for r in rows)
    return {
        "tok/s out": f"{tokens / busy:.0f}" if busy else "—",
        # Trials running side by side overlap anyway.
        "overlapping requests": f"{contended}/{timed}" if timed and concurrent == 1 else "—",
        "on battery": f"{battery}/{powered}" if powered else "—",
        "slept": f"{slept}/{powered}" if powered else "—",
        "power recorded": f"{powered}/{n}",
    }


def tools_cell(r: dict) -> str:
    return "" if r["tools"] is None else f"{r['tools']} ({r['tool_errors']})"


def pass_rate(rows: list[dict], clean: bool) -> str:
    done = [r for r in rows if r["outcome"] != "running"]
    if clean:
        done = [r for r in done if r["outcome"] not in ("env", "infra")]
    passed = sum(r["outcome"] == "pass" for r in done)
    return f"{passed}/{len(done)} ({100 * passed / max(len(done), 1):.0f}%)"


def load_merged(spec: str) -> dict[str, dict]:
    """`job+rerun+...`: the first job's trials, each later job's replacing the
    same task's (a rerun of the trials that got no verdict)."""
    merged: dict[str, dict] = {}
    for part in spec.split("+"):
        merged |= {r["task"]: r for r in load(job_dir(part))}
    return merged


def table(labels: list[str], loaded: list[dict[str, dict]]) -> None:
    """Tasks down, jobs across: each task's outcome and agent time, the
    scores, and what the trials ran into."""
    print("| task | " + " | ".join(labels) + " |")
    print("|---|" + "---|" * len(labels))
    for task in sorted(set().union(*loaded)):
        cells = [rows.get(task) for rows in loaded]
        outcomes = {c["outcome"] if c else "—" for c in cells}
        name = f"**{task}**" if len(outcomes) > 1 else task
        print(
            f"| {name} | "
            + " | ".join(f"{c['outcome']} {fmt_s(c['agent_s'])}" if c else "—" for c in cells)
            + " |"
        )
    for title, clean in (("passed", False), ("without env/infra", True)):
        print(f"| *{title}* | " + " | ".join(pass_rate(list(rows.values()), clean) for rows in loaded) + " |")
    measures = [
        measured(list(rows.values()), max((meta(r["job"]).get("concurrent") or 1 for r in rows.values()), default=1))
        for rows in loaded
    ]
    for key in measures[0]:
        print(f"| *{key}* | " + " | ".join(m[key] for m in measures) + " |")


def reference(models: set[str]) -> None:
    """The published scores, under a table of the model they were taken with."""
    if models and all(REFERENCE_MODEL in (m or "") for m in models):
        print(f"\nPublished on the whole dataset, {REFERENCE_MODEL}: {REFERENCE}.")


def report_matrix(specs: list[str]) -> None:
    jobs = [job_dir(spec.split("+")[0]) for spec in specs]
    loaded = [load_merged(spec) for spec in specs]
    labels = [agent_label(job, list(rows.values())) for job, rows in zip(jobs, loaded)]
    print("# " + " vs ".join(job.name for job in jobs) + "\n")
    for spec, job, label in zip(specs, jobs, labels):
        m = meta(job)
        print(f"- **{spec}**: {label} · model `{m.get('model')}` · {connection(job)} · {profile(job)}")
    print()
    table(labels, loaded)
    reference({meta(job).get("model") for job in jobs})


PLANNED_AGENTS = ["termide", "opencode", "qwen-coder"]


def all_jobs() -> list[Path]:
    root = BENCH / "jobs"
    return sorted(j for j in root.iterdir() if (j / "config.json").exists()) if root.exists() else []


def grid(set_name: str, axis: str, value: str) -> None:
    """One slice of the jobs that ran `set_name`: with `--model`, the
    harnesses on that model; with `--agent`, that harness on each model and
    connection. A table per profile of conditions, since scores taken under
    different ones do not compare. The latest job fills a cell; a later job
    of the same cell on some of its tasks (a rerun) replaces their trials."""
    jobs = all_jobs()
    column = (lambda j: agent_key(j)) if axis == "model" else (lambda j: f"`{meta(j).get('model')}` · {connection(j)}")
    fixed = (lambda j: meta(j).get("model") == value) if axis == "model" else (lambda j: agent_key(j) == value)
    mine = [j for j in jobs if fixed(j)]
    primaries = [j for j in mine if task_set(j) == set_name]
    if not primaries:
        sys.exit(f"no job ran set {set_name} with {axis} {value}")
    tasks = set_file_tasks(set_name) if set_name != "all" else None
    everything = {column(j) for j in jobs if meta(j).get("model") and connection(j) != "?"}
    if axis == "model":
        everything |= set(PLANNED_AGENTS)

    print(f"# {set_name} · {axis} {value}\n")
    for prof in sorted({profile(j) for j in primaries}):
        cells: dict[str, list[Path]] = {}
        for j in sorted((j for j in primaries if profile(j) == prof), key=started):
            cells.setdefault(column(j), []).append(j)
        names = sorted(cells)
        specs, older = [], []
        for name in names:
            latest = cells[name][-1]
            older += cells[name][:-1]
            reruns = [
                j for j in mine
                if j != latest and task_set(j) == "custom" and column(j) == name and profile(j) == prof
                and agent_label(j, []) == agent_label(latest, []) and started(j) > started(latest)
                and (tasks is None or (task_names(j) or frozenset()) <= tasks)
            ]
            specs.append("+".join(j.name for j in [latest, *sorted(reruns, key=started)]))
        loaded = [load_merged(spec) for spec in specs]
        heads = [job_dir(spec.split("+")[0]) for spec in specs]
        labels = [
            agent_label(job, list(rows.values())) if axis == "model" else name
            for job, rows, name in zip(heads, loaded, names)
        ]
        print(f"## {prof}\n")
        for spec, label in zip(specs, labels):
            print(f"- {label}: `{spec}`")
        print()
        table(labels, loaded)
        info = [details(job) for job in heads]
        if axis == "model":
            for d in info:
                del d["termide"]  # the column labels name the build
        differ = [k for k in info[0] if len({d[k] for d in info}) > 1]
        same = [k for k in info[0] if k not in differ]
        print()
        print("Same in every column: " + "; ".join(f"{k} {info[0][k]}" for k in same) + ".")
        for k in differ:
            print(f"Differs, {k}: " + "; ".join(f"{label} {d[k]}" for label, d in zip(labels, info)) + ".")
        if missing := sorted(everything - set(names)):
            print("Not run under these conditions: " + ", ".join(missing) + ".")
        if older:
            print("Older jobs of the same cells: " + ", ".join(j.name for j in older) + ".")
        if axis == "model":
            reference({value})
        print()


def harbor_pid(job: Path) -> str | None:
    """The pid of the harbor process running the job, if one is."""
    out = subprocess.run(["pgrep", "-f", f"harbor (run|job resume).*(--job-name |/jobs/){re.escape(job.name)}( |$)"], capture_output=True, text=True)
    pids = out.stdout.split()
    return pids[0] if pids else None


def task_limit(trial: Path, job: Path) -> float | None:
    """The agent's time limit for the trial's task, in seconds: the task's
    own, times the job's multiplier."""
    task = (json.loads((trial / "config.json").read_text()).get("task") or {})
    name, ref = task.get("name", ""), (task.get("ref") or "").removeprefix("sha256:")
    path = Path.home() / ".cache" / "harbor" / "tasks" / "packages" / name / ref / "task.toml"
    if not path.exists():
        return None
    import tomllib

    agent = tomllib.loads(path.read_text()).get("agent") or {}
    return agent["timeout_sec"] * multipliers(job)["agent"] if "timeout_sec" in agent else None


def live_events(trial: Path) -> list[dict]:
    """termide's event log as it is being written, inside the task container."""
    container = f"{trial.name.lower()}__env-main-1"
    out = subprocess.run(
        ["docker", "exec", container, "cat", "/logs/agent/termide.jsonl"], capture_output=True, text=True
    )
    evs = []
    for line in out.stdout.splitlines():
        try:
            evs.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    return evs


def fmt_clock(s: float) -> str:
    s = int(s)
    return f"{s // 3600}:{s % 3600 // 60:02}:{s % 60:02}" if s >= 3600 else f"{s // 60}:{s % 60:02}"


MARK = {"pass": "✓", "env": "⚠", "infra": "⚠", "running": "▶"}  # ✗ for the rest
WHY = {
    "late": "solved after its time",
    "timeout": "timeout",
    "fail": "tests failed",
    "agent-error": "agent error",
    "env": "environment broke",
    "infra": "server broke",
}
PENDING_LISTED = 20  # more pending tasks than this collapse into one line


def tests_passed(trial: Path) -> str:
    """`tests 3/7` from the verifier's report, when it wrote one."""
    path = trial / "verifier" / "ctrf.json"
    if not path.exists():
        return ""
    summary = (json.loads(path.read_text()).get("results") or {}).get("summary") or {}
    return f"tests {summary['passed']}/{summary['tests']}" if summary.get("tests") else ""


def trial_started(trial: Path) -> datetime:
    path = trial / "result.json"
    if path.exists() and (at := json.loads(path.read_text()).get("started_at")):
        return parse_time(at)
    return datetime.fromtimestamp((trial / "config.json").stat().st_mtime).astimezone()


def trial_wall(trial: Path) -> str:
    """The agent's own time on the task: not the container's start, the
    agent's install (minutes of apt for some harnesses) or the verifier."""
    path = trial / "result.json"
    result = json.loads(path.read_text()) if path.exists() else {}
    s = seconds(result.get("agent_execution"))
    return fmt_clock(s) if s is not None else ""

def agent_log_born(trial: Path) -> datetime | None:
    """When the agent began the task, while it runs: the creation of its log
    (termide.jsonl, claude-code.txt...) in the container's /logs/agent, which
    the run command writes and the install does not. Harbor copies the logs
    out only when the trial ends."""
    container = f"{trial.name.lower()}__env-main-1"
    out = subprocess.run(
        ["docker", "exec", container, "sh", "-c",
         'stat -c %W /logs/agent/*.jsonl /logs/agent/*.txt 2>/dev/null'],
        capture_output=True, text=True,
    ).stdout.split()
    born = [int(s) for s in out if s.isdigit() and int(s) > 0]
    return datetime.fromtimestamp(min(born)).astimezone() if born else None


def running_lines(trial: Path, job: Path, pid: str | None, now: datetime) -> tuple[str, list[str]]:
    """The running trial's phase and, under it, the agent's last step."""
    setup = trial / "agent" / "setup"
    verifier = trial / "verifier"
    if verifier.exists() and any(verifier.iterdir()):
        return "verifier", []
    if not setup.exists():
        return "environment", []
    began = agent_log_born(trial) if pid else None
    if pid and began is None:
        # Installing the agent: its time does not count against the limit.
        since = datetime.fromtimestamp(setup.stat().st_mtime).astimezone()
        return f"agent setup {fmt_clock((now - since).total_seconds())}", []
    phase = "agent"
    if began:
        ran = (now - began).total_seconds()
        limit = task_limit(trial, job)
        phase += f" {fmt_clock(ran)}" + (f" of {fmt_clock(limit)}" if limit else "")
    evs = live_events(trial) if pid else []
    if not evs:
        return phase, []
    calls = [e for e in evs if e.get("type") == "tool_use"]
    errors = sum(1 for e in evs if e.get("type") == "tool_result" and e.get("error"))
    phase += f" · {len(calls)} calls ({errors} failed)"
    last = evs[-1]
    if last.get("type") == "result":
        return phase, [f"finished at {fmt_clock(last.get('elapsed_ms', 0) / 1000)}: stop {last.get('stop_reason')}"
                       + (f", error {last.get('error')}" if last.get("error") else "")]
    what = {"thinking": "reasoning", "tool_input": "writing a call", "message": "writing"}.get(
        last.get("type"), last.get("type")
    )
    detail = [f"{what} since {fmt_clock(last.get('elapsed_ms', 0) / 1000)}"]
    if calls:
        c = calls[-1]
        subject = (c.get("subject") or "").splitlines()[0] if c.get("subject") else ""
        subject = subject if len(subject) <= 60 else subject[:59] + "…"
        detail.append(f"last call {fmt_clock(c.get('elapsed_ms', 0) / 1000)} {c.get('name')} {subject}".rstrip())
    return phase, detail


def status(job: Path) -> None:
    """Where a job stands: its conditions (machine, power, model speed) on
    top, then every task in the order it ran, marked with its outcome, the
    running one with its phase and its agent's last step, then the pending."""
    now = datetime.now().astimezone()
    pid = harbor_pid(job)
    rows = load(job)
    done = [r for r in rows if r["outcome"] != "running"]
    m = meta(job)
    if pid:
        state = f"running {fmt_clock((now - started(job)).total_seconds())}"
    else:
        state = "not running"
    print(f"{job.name} · {state} · {task_set(job)} · {now:%H:%M}")
    print(f"  {agent_label(job, rows)} · {m.get('model')} · {connection(job)}")

    conditions = [profile(job)]
    samples = power(job)
    if samples:
        t, src = samples[-1]
        age = (now - t).total_seconds()
        conditions.append(src.replace("AC Power", "on AC").replace("Battery Power", "ON BATTERY")
                          + (f" (sampled {fmt_clock(age)} ago)" if age > 120 else ""))
    elif pid:
        conditions.append("power not recorded")
    print("  " + " · ".join(conditions))
    reqs = requests(job)
    if reqs:
        recent = [(rs, re_, rn) for rs, re_, rn in reqs if now - re_ < timedelta(minutes=10)]
        busy = sum((re_ - rs).total_seconds() for rs, re_, _ in recent)
        if busy:
            print(f"  model: {sum(rn for _, _, rn in recent) / busy:.0f} tok/s over 10 min")

    total = json.loads((job / "result.json").read_text()).get("n_total_trials") if (job / "result.json").exists() else None
    total = total or (len(task_names(job)) if task_names(job) else len(done))
    print()
    # The marks below count the outcomes; only the share passed goes here.
    summary = " · ".join(score(done).replace("**", "").split(" · ")[:-1]) if done else "no results yet"
    print(f"  {summary} · done {len(done)} of {total}")
    print()

    by_task = {r["task"]: r for r in rows}
    trials = sorted((t for t in job.iterdir() if t.is_dir() and (t / "config.json").exists()), key=trial_started)
    width = max((len(t.name.rsplit("__", 1)[0]) for t in trials), default=0) + 2
    for trial in trials:
        task = trial.name.rsplit("__", 1)[0]
        r = by_task.get(task) or trial_row(trial)
        outcome = r["outcome"]
        mark = MARK.get(outcome, "✗")
        if outcome == "running":
            phase, detail = running_lines(trial, job, pid, now)
            print(f"  {mark} {task:<{width}}{phase}")
            for line in detail:
                print(f"      {line}")
            continue
        parts = [trial_wall(trial)]
        if outcome != "pass":
            tests = tests_passed(trial) if outcome in ("fail", "timeout", "late") else ""
            # A plain failure is its tests: `tests 0/4` says it all.
            if not (outcome == "fail" and tests):
                parts.append(WHY.get(outcome, outcome))
            parts.append(tests)
            if outcome in ("env", "infra", "agent-error") and (r["exception"] or r["error"]):
                parts.append((r["exception"] or r["error"]).splitlines()[0][:60])
        print(f"  {mark} {task:<{width}}" + " · ".join(p for p in parts if p))

    if tasks := task_names(job):
        started_tasks = {t.name.rsplit("__", 1)[0] for t in trials}
        pending = sorted(tasks - started_tasks)
        if len(pending) > PENDING_LISTED:
            print(f"  ◷ {len(pending)} pending: " + ", ".join(pending))
        else:
            for task in pending:
                print(f"  ◷ {task}")


def status_all(names: list[str]) -> None:
    jobs = [job_dir(n) for n in names] if names else [j for j in all_jobs() if harbor_pid(j)]
    if not jobs:
        latest = max(all_jobs(), key=started, default=None)
        print("no job is running" + (f"; the latest is {latest.name}:" if latest else ""))
        jobs = [latest] if latest else []
    for job in jobs:
        status(job)


def trace(job: Path, task: str) -> None:
    """One termide trial, step by step: when each call started, what it ran,
    and the output of the calls that failed."""
    trials = [t for t in sorted(job.iterdir()) if t.name.rsplit("__", 1)[0] == task]
    if not trials:
        sys.exit(f"no trial of {task} in {job}")
    print(f"# {job.name} · {task}\n")
    thought = 0  # reasoning characters since the last other event
    stamp = ""
    for e in events(trials[-1]):
        at = e.get("elapsed_ms")
        kind = e.get("type")
        if kind == "tool_input":
            stamp = f"{at / 1000:7.1f}s" if at is not None else "        "
            print(f"{stamp}  ~ drafting {e.get('name')} arguments: {len(e.get('text', ''))} more characters")
            continue
        if kind == "thinking":
            thought += len(e.get("text", ""))
            stamp = f"{at / 1000:7.1f}s" if at is not None else "        "
            continue
        if thought:
            print(f"{stamp}  ~ reasoned {thought} characters")
            thought = 0
        stamp = f"{at / 1000:7.1f}s" if at is not None else "        "
        if kind == "loop":
            what = "cut, retrying without reasoning" if e.get("retrying") else "cut again, run stopped"
            print(f"{stamp}  ↺ loop ({e.get('repeats')} repeats): {what}")
            for t in e.get("thoughts") or []:
                print(f"{' ' * 12}- {t[:160]}")
        elif kind == "cut_off":
            what = "retrying" if e.get("retrying") else "run stopped"
            print(f"{stamp}  ✂ reply hit the output limit with nothing in it: {what}")
        elif kind == "reasoning_budget":
            what = "retrying without reasoning" if e.get("retrying") else "run stopped"
            print(f"{stamp}  ✂ reasoning ran past its budget of {e.get('budget')} tokens: {what}")
        elif kind == "malformed_call":
            print(f"{stamp}  ✂ the server could not parse a tool call: nothing run, retrying")
        elif kind == "tool_use":
            subject = e.get("subject", "").replace("\n", " ⏎ ")
            print(f"{stamp}  {e.get('name', ''):<6} {subject[:160]}")
        elif kind == "tool_result" and e.get("error"):
            output = (e.get("output") or "(no output recorded)").strip().replace("\n", "\n" + " " * 12)
            print(f"{stamp}  ! {output[:600]}")
        elif kind == "message":
            print(f"{stamp}  » {e.get('text', '').strip().splitlines()[0][:160]}")
        elif kind == "result":
            print(f"{stamp}  = stop {e.get('stop_reason')} · error {e.get('error')}")
    if thought:
        print(f"{stamp}  ~ reasoned {thought} characters, then the log ends")


def thinking(job: Path, task: str) -> None:
    """The reasoning of one trial as written, each call's after a rule."""
    trials = [t for t in sorted(job.iterdir()) if t.name.rsplit("__", 1)[0] == task]
    if not trials:
        sys.exit(f"no trial of {task} in {job}")
    in_call = False
    for e in events(trials[-1]):
        if e.get("type") == "tool_input":
            print(f"\n----- {e.get('elapsed_ms', 0) / 1000:.1f}s {e.get('name')} arguments")
            sys.stdout.write(e.get("text", ""))
            in_call = False
        elif e.get("type") == "thinking":
            if not in_call:
                print(f"\n----- {e.get('elapsed_ms', 0) / 1000:.1f}s")
            in_call = True
            sys.stdout.write(e.get("text", ""))
        else:
            in_call = False


def main() -> None:
    args = sys.argv[1:]
    if args and args[0] == "status":
        status_all(args[1:])
    elif len(args) == 3 and args[0] == "trace":
        trace(job_dir(args[1]), args[2])
    elif len(args) == 4 and args[0] == "grid" and args[2] in ("--model", "--agent"):
        grid(args[1], args[2][2:], args[3])
    elif len(args) == 3 and args[0] == "thinking":
        thinking(job_dir(args[1]), args[2])
    elif len(args) == 1:
        report_one(job_dir(args[0]))
    elif len(args) > 1:
        report_matrix(args)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
