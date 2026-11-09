#!/usr/bin/env python3
"""Summarise Harbor jobs as Markdown.

    report.py <job>              one job: every trial and why it ended
    report.py <job> <job>...     a matrix: each task's outcome in each job,
                                 to compare harnesses or two termide builds;
                                 tasks where the jobs differ are in bold;
                                 <job>+<rerun> takes the rerun's trials in
                                 place of the job's for the tasks it ran
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
import sys
from datetime import datetime
from pathlib import Path

BENCH = Path(__file__).resolve().parent
ORDER = ["pass", "late", "fail", "timeout", "agent-error", "env", "infra", "running"]
INFRA_MARKERS = ("[METAL]", "Insufficient Memory", "OutOfMemory")
# Lines a verifier prints when installing its test runner failed.
VERIFIER_SETUP_FAILURES = ("uvx: command not found", "pytest: command not found", "failed to download")
# Published full-dataset scores to read the matrix against: GigaCode Universal
# Agent's comparison, Qwen3.6-35B-A3B, https://habr.com/ru/companies/sberbank/articles/1104670/
REFERENCE = {
    "terminal-bench-2-1": "Universal Agent 65.5% · OpenCode 62.1% · Qwen Code 41.4%",
}


def job_dir(arg: str) -> Path:
    path = Path(arg)
    return path if path.is_dir() else BENCH / "jobs" / arg


def seconds(phase: dict | None) -> float | None:
    if not phase or not phase.get("started_at") or not phase.get("finished_at"):
        return None
    parse = lambda s: datetime.fromisoformat(s.replace("Z", "+00:00"))
    return (parse(phase["finished_at"]) - parse(phase["started_at"])).total_seconds()


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
    config_path = job / "config.json"
    if not config_path.exists():
        return {}
    config = json.loads(config_path.read_text())
    agent = (config.get("agents") or [{}])[0]
    return {
        "model": (agent.get("model_name") or "").split("/", 1)[-1],
        "dataset": ((config.get("datasets") or [{}])[0]).get("name", ""),
        "concurrent": config.get("n_concurrent_trials"),
    }


def agent_label(job: Path, rows: list[dict]) -> str:
    """The agent and its version: termide by commit, others as Harbor saw them."""
    m = meta(job)
    if m.get("describe"):
        reasoning = f" · {m['reasoning']}" if m.get("reasoning") else ""
        return f"termide {m['describe']}{reasoning}"
    return next((r["agent"] for r in rows if r["agent"]), m.get("agent", "?"))


def meta_line(job: Path) -> str:
    m = meta(job)
    if not m:
        return ""
    return (
        f"{agent_label(job, load(job))} · model `{m.get('model')}` · "
        f"{m.get('dataset')} · {m.get('concurrent')} at once"
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


def report_matrix(specs: list[str]) -> None:
    jobs = [job_dir(spec.split("+")[0]) for spec in specs]
    loaded = [load_merged(spec) for spec in specs]
    labels = [agent_label(job, list(rows.values())) for job, rows in zip(jobs, loaded)]
    print("# " + " vs ".join(job.name for job in jobs) + "\n")
    for spec, job, label in zip(specs, jobs, labels):
        m = meta(job)
        print(f"- **{spec}**: {label} · model `{m.get('model')}` · {m.get('concurrent')} at once")
    print()
    print("| task | " + " | ".join(labels) + " |")
    print("|---|" + "---|" * len(jobs))
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
        print(
            f"| *{title}* | "
            + " | ".join(pass_rate(list(rows.values()), clean) for rows in loaded)
            + " |"
        )
    datasets = {meta(job).get("dataset", "") for job in jobs}
    for key, line in REFERENCE.items():
        if any(key in d for d in datasets):
            print(f"\nPublished on the whole dataset, same model: {line}.")


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
    if len(args) == 3 and args[0] == "trace":
        trace(job_dir(args[1]), args[2])
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
