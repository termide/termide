"""The conditions a job runs under: the machine, Docker, the model server.

run.sh records them in bench-meta.json under `conditions`; report.py groups
jobs by them, since Terminal-Bench limits agents by wall-clock time and a
score is only comparable to one taken under the same conditions.

    python3 conditions.py <base-url>    print them as JSON
"""

import json
import os
import platform
import subprocess
import sys
from pathlib import Path
from urllib.parse import urlparse

OMLX_SETTINGS = Path.home() / ".omlx" / "settings.json"


def run(*cmd: str) -> str:
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=20).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def connection(base_url: str) -> str:
    """What serves the model, from the endpoint the container is given."""
    url = urlparse(base_url)
    if url.scheme == "claude-subscription":  # run.sh's name for a Claude login
        return "claude subscription"
    if url.port == 10000:
        return "omlx"
    host = url.hostname or base_url
    if "openrouter" in host:
        return "openrouter"
    return host


def machine() -> dict:
    """The hardware, never the host name: reports get published. BENCH_MACHINE
    names the machine where its hardware alone does not tell it apart."""
    info = {"name": os.environ["BENCH_MACHINE"]} if os.environ.get("BENCH_MACHINE") else {}
    return info | hardware()


def hardware() -> dict:
    gib = 1024**3
    if platform.system() == "Darwin":
        return {
            "chip": run("sysctl", "-n", "machdep.cpu.brand_string"),
            "memory_gb": round(int(run("sysctl", "-n", "hw.memsize") or 0) / gib),
            "os": "macOS " + platform.mac_ver()[0],
        }
    pages = Path("/proc/meminfo").read_text().split()[1] if Path("/proc/meminfo").exists() else "0"
    return {
        "chip": platform.processor() or platform.machine(),
        "memory_gb": round(int(pages) * 1024 / gib),
        "os": f"{platform.system()} {platform.release()}",
    }


def docker() -> dict:
    out = run("docker", "info", "--format", "{{.NCPU}} {{.MemTotal}} {{.ServerVersion}}").split()
    if len(out) != 3:
        return {}
    return {"cpus": int(out[0]), "memory_gb": round(int(out[1]) / 1024**3, 1), "version": out[2]}


def server(base_url: str) -> dict:
    kind = connection(base_url)
    info = {"kind": kind}
    if kind == "omlx":
        binary = Path.home() / ".omlx" / "bin" / "omlx"
        info["version"] = run(str(binary), "--version") if binary.exists() else ""
        try:
            settings = json.loads(OMLX_SETTINGS.read_text())
        except (OSError, json.JSONDecodeError):
            settings = {}
        scheduler, cache = settings.get("scheduler", {}), settings.get("cache", {})
        # The settings that change how fast a request is served.
        info["settings"] = {
            "max_concurrent_requests": scheduler.get("max_concurrent_requests"),
            "chunked_prefill": scheduler.get("chunked_prefill"),
            "prefill_priority": scheduler.get("prefill_priority"),
            "cache": cache.get("enabled"),
            "hot_cache_max_size": cache.get("hot_cache_max_size"),
        }
    return info


def static(base_url: str) -> dict:
    return {"machine": machine(), "docker": docker(), "server": server(base_url)}


if __name__ == "__main__":
    print(json.dumps(static(sys.argv[1]), indent=2))
