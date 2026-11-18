"""Harbor adapter for termide's headless agent (`termide --prompt`).

Harbor uploads the static x86_64 musl binary that `tools/bench/build.sh`
leaves in `tools/bench/bin/` (or TERMIDE_BINARY) into the task container,
writes a config with one OpenAI-compatible connection and runs one task:

    termide --prompt <instruction> --output stream-json

The model endpoint must be reachable from the container: TERMIDE_BASE_URL
(default: OMLX on the host), TERMIDE_MODEL (default: the model part of
`-m`), the key in TERMIDE_API_KEY, the window in TERMIDE_CONTEXT_WINDOW.

With TERMIDE_PROVIDER=claude_code termide drives Claude Code over ACP instead,
on the subscription of CLAUDE_CODE_OAUTH_TOKEN: Node 22 (from nvm, on
termide's PATH only) and the ACP adapter at TERMIDE_ACP_ADAPTER_VERSION go
into the container, an agent `bench` runs the adapter, and TERMIDE_MODEL
reaches it as ANTHROPIC_MODEL, the model its sessions start on.
"""

import asyncio
import json
import shlex
from pathlib import Path

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext
from harbor.utils.scripts import ensure_acp_node_command

BENCH_DIR = Path(__file__).resolve().parent.parent
REMOTE_BIN = "/usr/local/bin/termide"
LOG_NAME = "termide.jsonl"
# Where the agent's config lives; XDG_CONFIG_HOME points there, whatever the
# agent user's $HOME is.
CONFIG_HOME = "/installed-agent/config"
# Claude Code's ACP adapter. A `claude_code` connection would run its latest
# release through npx; the bench pins one instead (TERMIDE_ACP_ADAPTER_VERSION,
# 0.108.1 by default: it bundles Claude Code 2.1.326, the version bare
# claude-code runs at), installs it at setup and runs it as the `command` of
# an agent of its own, named on the command line.
ACP_ADAPTER = "@agentclientprotocol/claude-agent-acp"
ACP_ADAPTER_VERSION = "0.108.1"
ACP_AGENT = "bench"
# The directory of the Node install puts in front of termide's PATH.
NODE_PATH_FILE = f"{CONFIG_HOME}/node-bin"
WITH_NODE = f'PATH="$(cat {NODE_PATH_FILE}):$PATH"; export PATH; '
# Stop termide and the commands it started (its direct children), found by
# /proc in plain sh, since task images may lack ps, pkill, even awk.
KILL_AGENT = f"""
agents=""
for p in /proc/[0-9]*; do
  [ "$p/exe" -ef {REMOTE_BIN} ] && agents="$agents ${{p#/proc/}}"
done
for p in /proc/[0-9]*; do
  ppid=""
  while read -r key value; do
    [ "$key" = PPid: ] && ppid=$value && break
  done < "$p/status" 2>/dev/null
  case " $agents " in *" $ppid "*) kill -KILL "${{p#/proc/}}" 2>/dev/null ;; esac
done
for pid in $agents; do kill -KILL "$pid" 2>/dev/null; done
true
"""


def _toml_str(value: str) -> str:
    return json.dumps(value)  # a JSON string is a valid TOML basic string


class TermideAgent(BaseInstalledAgent):
    @staticmethod
    def name() -> str:
        return "termide"

    def get_version_command(self) -> str | None:
        return f"{REMOTE_BIN} --version"

    async def install(self, environment: BaseEnvironment) -> None:
        binary = Path(
            self._get_env("TERMIDE_BINARY") or BENCH_DIR / "bin" / "termide"
        ).expanduser()
        if not binary.is_file():
            raise FileNotFoundError(f"{binary}: run tools/bench/build.sh first")
        await environment.upload_file(binary, REMOTE_BIN)
        await self.exec_as_root(environment, command=f"chmod 755 {REMOTE_BIN}")
        if self._provider() == "claude_code":
            await self._install_node(environment)
        # Harbor 0.25 calls no configure() hook yet; write the config here.
        await self._write_config(environment)

    async def _install_node(self, environment: BaseEnvironment) -> None:
        # The adapter wants Node 20 or later; the distribution's is often
        # older, and the task may depend on it, so a newer one comes from nvm
        # and only termide's PATH points at it (NODE_PATH_FILE, read by run).
        await self.ensure_system_dependencies(environment, ("curl", "bash"))
        result = await self.exec_as_agent(
            environment,
            command=(
                "set -e; "
                + ensure_acp_node_command()
                + '; echo "NODE_BIN=$(dirname "$(command -v node)")"'
            ),
        )
        lines = [
            line
            for line in (result.stdout or "").splitlines()
            if line.startswith("NODE_BIN=")
        ]
        if not lines:
            raise RuntimeError(f"no Node for the ACP adapter (got {result.stdout!r})")
        node_bin = lines[-1].removeprefix("NODE_BIN=")
        await self.exec_as_root(
            environment,
            command=(
                f"mkdir -p {CONFIG_HOME} && chmod 777 {CONFIG_HOME} && "
                f"printf %s {shlex.quote(node_bin)} > {NODE_PATH_FILE}"
            ),
        )
        version = self._adapter_version()
        await self.exec_as_agent(
            environment,
            command=(
                WITH_NODE
                + f"npm install -g {ACP_ADAPTER}@{shlex.quote(version)}"
                + ' && test -x "$(cat ' + NODE_PATH_FILE + ')/claude-agent-acp"'
            ),
        )

    def _adapter_version(self) -> str:
        return self._get_env("TERMIDE_ACP_ADAPTER_VERSION") or ACP_ADAPTER_VERSION

    def _provider(self) -> str:
        return self._get_env("TERMIDE_PROVIDER") or "openai_compatible"

    def _model(self) -> str:
        return self._get_env("TERMIDE_MODEL") or (self.model_name or "").split("/")[-1]

    async def _write_config(self, environment: BaseEnvironment) -> None:
        base_url = (
            self._get_env("TERMIDE_BASE_URL") or "http://host.docker.internal:10000/v1"
        )
        model = self._model()
        context = self._get_env("TERMIDE_CONTEXT_WINDOW") or "262144"
        reasoning = self._get_env("TERMIDE_REASONING") or "high"
        budget = self._get_env("TERMIDE_REASONING_BUDGET") or "0"
        config = "\n".join(
            [
                "[ai]",
                f"reasoning = {_toml_str(reasoning)}",
                "[ai.permissions]",
                # No one answers a prompt in a task container.
                'mode = "all"',
                "[ai.connections.bench]",
            ]
            + (
                # The `bench` agent (below) runs Claude Code whatever the
                # connection says; one must still be there.
                ['provider = "claude_code"']
                if self._provider() == "claude_code"
                else [
                    'provider = "openai_compatible"',
                    f"base_url = {_toml_str(base_url)}",
                    f"model = {_toml_str(model)}",
                    'api_key_env = "TERMIDE_API_KEY"',
                    f"context_window_fallback = {int(context)}",
                    f"reasoning_budget = {int(budget)}",
                ]
            )
            + [""]
        )
        await self.exec_as_root(
            environment,
            command=f"mkdir -p {CONFIG_HOME}/termide && chmod -R 777 {CONFIG_HOME}",
        )
        await self._upload_config_text(
            environment,
            content=config,
            remote_path=f"{CONFIG_HOME}/termide/config.toml",
            filename="config.toml",
        )
        if self._provider() == "claude_code":
            await self._write_acp_agent(environment)

    async def _write_acp_agent(self, environment: BaseEnvironment) -> None:
        # The pinned adapter as an agent's `command` (not run through a shell,
        # so its absolute path), recognised as Claude Code's for termide's
        # prompt and tools. The model is ANTHROPIC_MODEL's (see run), which
        # the adapter starts each session on, as bare Claude Code takes
        # --model; a full id such as claude-sonnet-5-5 matches none of the
        # adapter's aliases by name, so the agent names none. Claude Code
        # logs in on its own.
        result = await self.exec_as_agent(environment, command=f"cat {NODE_PATH_FILE}")
        node_bin = (result.stdout or "").strip()
        agent_dir = f"{CONFIG_HOME}/termide/ai/agents/{ACP_AGENT}"
        await self.exec_as_root(
            environment, command=f"mkdir -p {agent_dir} && chmod -R 777 {CONFIG_HOME}"
        )
        await self._upload_config_text(
            environment,
            content="\n".join(
                [
                    "---",
                    "description: Claude Code through its pinned ACP adapter",
                    f"command: {node_bin}/claude-agent-acp",
                    "adapter: claude_code",
                    "---",
                    "",
                ]
            ),
            remote_path=f"{agent_dir}/AGENT.md",
            filename="AGENT.md",
        )

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        env = {
            "TERMIDE_API_KEY": self._get_env("TERMIDE_API_KEY") or "none",
            "XDG_CONFIG_HOME": CONFIG_HOME,
        }
        prefix = ""
        agent = ""
        if self._provider() == "claude_code":
            env["CLAUDE_CODE_OAUTH_TOKEN"] = self._get_env("CLAUDE_CODE_OAUTH_TOKEN") or ""
            env["CLAUDE_FORCE_OAUTH"] = "1"
            # The adapter starts each session on the model this names (and
            # lists an older one such as claude-sonnet-5 only then).
            env["ANTHROPIC_MODEL"] = self._model()
            prefix = WITH_NODE
            agent = f" --agent {ACP_AGENT}"
        try:
            await self.exec_as_agent(
                environment,
                command=(
                    f"{prefix}{REMOTE_BIN} --prompt {shlex.quote(instruction)}{agent} --output stream-json"
                    f" 2>/logs/agent/termide.stderr | tee /logs/agent/{LOG_NAME}"
                ),
                env=env,
            )
        except asyncio.CancelledError:
            # Harbor's agent timeout cancels this coroutine and moves on to the
            # verifier, but the process in the container runs on: it would go
            # on editing files under the tests and loading the model server.
            await self.exec_as_root(environment, command=KILL_AGENT)
            raise

    def populate_context_post_run(self, context: AgentContext) -> None:
        log = self.logs_dir / LOG_NAME
        if not log.exists():
            return
        result = None
        for line in log.read_text(errors="replace").splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if event.get("type") == "result":
                result = event
        if not result:
            return
        # The whole run's usage; builds before `usage_total` report only the
        # last turn's.
        usage = result.get("usage_total") or result.get("usage") or {}
        context.n_input_tokens = usage.get("input")
        context.n_output_tokens = usage.get("output")
        context.n_cache_tokens = usage.get("cache_read")
        context.metadata = {
            "stop_reason": result.get("stop_reason"),
            "tool_calls": len(result.get("tools") or []),
            "error": result.get("error"),
        }
