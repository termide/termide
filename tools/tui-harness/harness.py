#!/usr/bin/env python3
"""Run termide headlessly against scenario scripts and check what it draws.

termide runs in a pty of a fixed size with a throwaway HOME, a fresh
configuration and, by default, the synthetic project of the screenshots
(tools/screenshots/fixtures/make-demo.sh). Its output is fed to a pyte
screen, which also answers the queries termide sends at startup (device
attributes, cursor position); the kitty keyboard query goes unanswered, so
keys are sent in the legacy xterm encoding. A scenario sends keys and text,
waits for text on the screen, asserts on it and saves the screen as text.

Usage: run.sh [options] [scenario ...]   (see README.md; no scenario = all)
"""

import argparse
import fcntl
import inspect
import os
import pty
import re
import select
import shlex
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

import pyte

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
SHOTS = REPO / "tools" / "screenshots"
SCENARIOS = HERE / "scenarios"
DEFAULT_TIMEOUT = 10.0
VS16 = "\ufe0f".encode()


class Failure(Exception):
    pass


# --- keys ------------------------------------------------------------------

SPECIAL = {
    # name: (plain bytes, CSI form for modifiers: (number, final))
    "enter": (b"\r", None),
    "esc": (b"\x1b", None),
    "tab": (b"\t", None),
    "backspace": (b"\x7f", None),
    "space": (b" ", None),
    "up": (b"\x1b[A", ("1", "A")),
    "down": (b"\x1b[B", ("1", "B")),
    "right": (b"\x1b[C", ("1", "C")),
    "left": (b"\x1b[D", ("1", "D")),
    "home": (b"\x1b[H", ("1", "H")),
    "end": (b"\x1b[F", ("1", "F")),
    "insert": (b"\x1b[2~", ("2", "~")),
    "delete": (b"\x1b[3~", ("3", "~")),
    "pageup": (b"\x1b[5~", ("5", "~")),
    "pagedown": (b"\x1b[6~", ("6", "~")),
    "f1": (b"\x1bOP", ("1", "P")),
    "f2": (b"\x1bOQ", ("1", "Q")),
    "f3": (b"\x1bOR", ("1", "R")),
    "f4": (b"\x1bOS", ("1", "S")),
    "f5": (b"\x1b[15~", ("15", "~")),
    "f6": (b"\x1b[17~", ("17", "~")),
    "f7": (b"\x1b[18~", ("18", "~")),
    "f8": (b"\x1b[19~", ("19", "~")),
    "f9": (b"\x1b[20~", ("20", "~")),
    "f10": (b"\x1b[21~", ("21", "~")),
    "f11": (b"\x1b[23~", ("23", "~")),
    "f12": (b"\x1b[24~", ("24", "~")),
}
ALIASES = {
    "return": "enter",
    "escape": "esc",
    "pgup": "pageup",
    "pgdn": "pagedown",
    "del": "delete",
}


def key_bytes(spec):
    """`Alt+T`, `Ctrl+Shift+Up`, `F5`, `Enter`, `x` -> bytes, xterm style."""
    parts = spec.split("+")
    name = parts[-1] if parts[-1] else "+"
    mods = {p.lower() for p in parts[:-1]}
    unknown = mods - {"ctrl", "alt", "shift"}
    if unknown:
        raise Failure(f"unknown modifier in key {spec!r}")
    lname = ALIASES.get(name.lower(), name.lower())
    if lname in SPECIAL:
        plain, csi = SPECIAL[lname]
        if lname == "tab" and mods == {"shift"}:
            return b"\x1b[Z"
        if not mods:
            return plain
        if csi is None:
            return b"\x1b" + plain if mods == {"alt"} else plain
        code = 1 + ("shift" in mods) + 2 * ("alt" in mods) + 4 * ("ctrl" in mods)
        number, final = csi
        return f"\x1b[{number};{code}{final}".encode()
    if len(name) != 1:
        raise Failure(f"unknown key {spec!r}")
    ch = name.upper() if "shift" in mods else name.lower() if mods else name
    if "ctrl" in mods:
        if ch == " ":
            out = b"\x00"
        elif ch.isalpha() or ch in "@[\\]^_":
            out = bytes([ord(ch.upper()) & 0x1F])
        elif ch == "/":
            out = b"\x1f"
        else:
            raise Failure(f"Ctrl with {name!r} has no legacy encoding")
    else:
        out = ch.encode()
    return b"\x1b" + out if "alt" in mods else out


# --- the program under test ---------------------------------------------------


class Screen(pyte.Screen):
    """A pyte screen whose replies to queries go back to the program."""

    def __init__(self, cols, rows, reply):
        super().__init__(cols, rows)
        self._reply = reply

    def write_process_input(self, data):
        self._reply(data.encode())


class Term:
    def __init__(self, argv, env, cwd, cols, rows, log):
        self.cols, self.rows = cols, rows
        self.log = log
        self.screen = Screen(cols, rows, self.write)
        self.stream = pyte.ByteStream(self.screen)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            try:
                os.chdir(cwd)
                os.execvpe(argv[0], argv, env)
            finally:
                os._exit(127)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.exit_status = None

    def write(self, data):
        try:
            os.write(self.fd, data)
        except OSError:
            pass

    def alive(self):
        if self.exit_status is not None:
            return False
        pid, status = os.waitpid(self.pid, os.WNOHANG)
        if pid == 0:
            return True
        self.exit_status = os.waitstatus_to_exitcode(status)
        return False

    def pump(self, seconds):
        """Read output for `seconds` (at least one pass); False once it is gone."""
        end = time.monotonic() + seconds
        while True:
            left = max(0.0, end - time.monotonic())
            ready, _, _ = select.select([self.fd], [], [], left)
            if ready:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    data = b""
                if not data:
                    return self.alive()
                self.log.write(data)
                # pyte loses the character after U+FE0F (VS16). termide is
                # told VS16 does not widen (TERMIDE_VS16_WIDE=0), so the
                # selector changes no width and can go.
                self.stream.feed(data.replace(VS16, b""))
            if time.monotonic() >= end:
                return True

    def text(self):
        # Not `screen.display`: pyte's own renderer fails on the empty cell
        # that follows a wide character. Joining the cells leaves that one
        # out, so a wide character reads as one column of text.
        buffer = self.screen.buffer
        return "\n".join(
            "".join(buffer[y][x].data for x in range(self.cols)).rstrip() for y in range(self.rows)
        )

    def stop(self, grace=2.0):
        if self.alive():
            for sig in (signal.SIGTERM, signal.SIGKILL):
                try:
                    os.kill(self.pid, sig)
                except ProcessLookupError:
                    break
                deadline = time.monotonic() + grace
                while self.alive() and time.monotonic() < deadline:
                    self.pump(0.05)
                if not self.alive():
                    break
        try:
            os.close(self.fd)
        except OSError:
            pass


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def config_dir(home):
    # termide finds its config through `dirs::config_dir()`, which ignores
    # XDG variables on macOS.
    if sys.platform == "darwin":
        return home / "Library" / "Application Support" / "termide"
    return home / ".config" / "termide"


# --- a scenario -------------------------------------------------------------------


class Run:
    def __init__(self, path, args):
        self.path = path
        self.name = path.stem
        self.args = args
        self.cols, self.rows = 120, 36
        self.project_kind = "demo"
        self.config = HERE / "config.toml"
        self.mock_llm = None
        self.mock_acp = None
        self.lsp = False
        self.term = None
        self.helpers = []
        self.files = []
        self.snapshots = 0
        self.out = args.out / self.name
        shutil.rmtree(self.out, ignore_errors=True)
        self.out.mkdir(parents=True)
        # /tmp rather than macOS's long per-user temp dir: paths in titles
        # and the status bar stay readable in snapshots.
        base = "/tmp" if os.path.isdir("/tmp") else None
        self.scratch = Path(tempfile.mkdtemp(prefix=f"tui-harness-{self.name}-", dir=base))
        self.home = self.scratch / "home"
        self.project = None

    # setup verbs, before `start`

    def v_size(self, spec):
        m = re.fullmatch(r"(\d+)x(\d+)", spec)
        if not m:
            raise Failure("size takes COLSxROWS")
        self.cols, self.rows = int(m[1]), int(m[2])

    def v_project(self, kind):
        if kind not in ("demo", "empty"):
            raise Failure("project takes demo or empty")
        self.project_kind = kind

    def v_config(self, file):
        self.config = (self.path.parent / file).resolve()

    def v_mock_llm(self, flavor="default"):
        self.mock_llm = flavor

    def v_lsp(self):
        # Language servers run: no `--no-lsp`, and a rustup-installed server
        # finds its toolchains though HOME is the throwaway one.
        self.lsp = True

    def v_mock_acp(self, provider):
        if provider not in ("claude_code", "codex", "gemini_cli"):
            raise Failure("mock-acp takes claude_code, codex or gemini_cli")
        self.mock_acp = provider

    def env(self):
        env = {
            "HOME": str(self.home),
            "USER": "demo",
            "LOGNAME": "demo",
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "TERM": "xterm-256color",
            "LANG": "en_US.UTF-8",
            "LC_ALL": "en_US.UTF-8",
            "TZ": "UTC",
            # Emoji widths as pyte (wcwidth) counts them, without the probe.
            "TERMIDE_VS16_WIDE": "0",
            "SHELL": "/bin/bash",
            "BASH_SILENCE_DEPRECATION_WARNING": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
            "GIT_CONFIG_NOSYSTEM": "1",
            "XDG_CONFIG_HOME": str(self.home / ".config"),
            "XDG_DATA_HOME": str(self.home / ".local" / "share"),
            "XDG_STATE_HOME": str(self.home / ".local" / "state"),
            "XDG_CACHE_HOME": str(self.home / ".cache"),
        }
        if self.mock_acp:
            env["PATH"] = f"{SHOTS / 'mock-acp'}:{env['PATH']}"
            env["MOCK_ACP_LOG"] = str(self.out / "mock-acp.log")
        if self.lsp:
            real = Path.home()
            env["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(real / ".rustup"))
            env["CARGO_HOME"] = os.environ.get("CARGO_HOME", str(real / ".cargo"))
        return env

    def prepare(self):
        self.home.mkdir()
        (self.home / ".gitconfig").write_text(
            "[user]\n\tname = Demo User\n\temail = demo@example.com\n"
            "[init]\n\tdefaultBranch = main\n"
        )
        (self.home / ".bashrc").write_text("PS1='demo$ '\nHISTFILE=/dev/null\n")
        env = self.env()
        if self.project_kind == "demo":
            root = self.scratch / "demo"
            subprocess.run(
                ["bash", str(SHOTS / "fixtures" / "make-demo.sh")],
                env=dict(env, DEMO_ROOT=str(root)),
                check=True,
                stdout=subprocess.DEVNULL,
            )
            self.project = root / "inventory"
        else:
            self.project = self.scratch / "project"
            self.project.mkdir()
        port = free_port()
        if self.mock_llm:
            # Open as long as the mock runs; closed in cleanup().
            log = open(self.out / "mock-llm.log", "w")  # noqa: SIM115
            self.files.append(log)
            self.helpers.append(
                subprocess.Popen(
                    [
                        sys.executable,
                        str(SHOTS / "mock-llm" / "server.py"),
                        str(port),
                        self.mock_llm,
                    ],
                    stdout=log,
                    stderr=log,
                )
            )
            wait_for_port(port)
        config = self.config.read_text().replace("{mock_llm_port}", str(port))
        if self.mock_acp:
            name = {"claude_code": "claude", "codex": "codex", "gemini_cli": "gemini"}[
                self.mock_acp
            ]
            config = re.sub(r'(?m)^connection = "local"$', f'connection = "{name}"', config)
            config += f'\n[ai.connections.{name}]\nprovider = "{self.mock_acp}"\n'
        cdir = config_dir(self.home)
        cdir.mkdir(parents=True)
        (cdir / "config.toml").write_text(config)
        return env

    def v_start(self, *extra):
        env = self.prepare()
        argv = [str(self.args.bin), *([] if self.lsp else ["--no-lsp"]), *extra]
        argv = [a.replace("{project}", str(self.project)) for a in argv]
        # Every byte termide wrote, for replaying a failure; closed in cleanup().
        self.log = open(self.out / "output.bin", "wb")  # noqa: SIM115
        self.term = Term(argv, env, self.project, self.cols, self.rows, self.log)
        # The first frame: the status bar is drawn last.
        self.wait_until(lambda: self.term.text().strip(), 15, "the first frame")

    # interaction verbs

    def need_term(self):
        if self.term is None:
            raise Failure("start termide first")
        return self.term

    def v_key(self, *keys):
        term = self.need_term()
        for k in keys:
            term.write(key_bytes(k))
            term.pump(self.args.key_delay)

    def v_type(self, text):
        term = self.need_term()
        for ch in text:
            term.write(ch.encode())
            term.pump(0.01)
        term.pump(self.args.key_delay)

    def v_paste(self, text):
        term = self.need_term()
        term.write(b"\x1b[200~" + text.encode() + b"\x1b[201~")
        term.pump(self.args.key_delay)

    def v_sleep(self, seconds):
        self.need_term().pump(float(seconds))

    def wait_until(self, test, timeout, what):
        term = self.need_term()
        end = time.monotonic() + float(timeout)
        while True:
            if test():
                return
            if not term.pump(0.05):
                raise Failure(f"termide exited ({term.exit_status}) while waiting for {what}")
            if time.monotonic() > end:
                raise Failure(f"timed out after {timeout}s waiting for {what}")

    def v_wait(self, text, timeout=DEFAULT_TIMEOUT):
        self.wait_until(lambda: text in self.term.text(), timeout, repr(text))

    def v_wait_re(self, pattern, timeout=DEFAULT_TIMEOUT):
        rx = re.compile(pattern, re.MULTILINE)
        self.wait_until(lambda: rx.search(self.term.text()), timeout, f"/{pattern}/")

    def v_wait_gone(self, text, timeout=DEFAULT_TIMEOUT):
        self.wait_until(lambda: text not in self.term.text(), timeout, f"{text!r} to go")

    def v_expect(self, text):
        self.need_term().pump(0.05)
        if text not in self.term.text():
            raise Failure(f"expected {text!r} on the screen")

    def v_expect_not(self, text):
        self.need_term().pump(0.05)
        if text in self.term.text():
            raise Failure(f"did not expect {text!r} on the screen")

    def v_expect_re(self, pattern):
        self.need_term().pump(0.05)
        if not re.search(pattern, self.term.text(), re.MULTILINE):
            raise Failure(f"expected /{pattern}/ on the screen")

    def v_expect_row(self, row, pattern):
        self.need_term().pump(0.05)
        lines = self.term.text().split("\n")
        index = int(row) if int(row) >= 0 else len(lines) + int(row)
        line = lines[index] if 0 <= index < len(lines) else ""
        if not re.search(pattern, line):
            raise Failure(f"expected /{pattern}/ on row {row}, found {line!r}")

    def v_file_contains(self, rel, text, timeout=DEFAULT_TIMEOUT):
        path = self.project / rel
        self.wait_until(
            lambda: path.exists() and text in path.read_text(errors="replace"),
            timeout,
            f"{text!r} in {rel}",
        )

    def v_mock_llm_asked(self, model, timeout=DEFAULT_TIMEOUT):
        """Wait until the mock model has had a request for `model`: where a
        side model setting (subagents, goal judge, handoff, reviewer) sent its
        call. The mock logs `asked MODEL (tools|service)` for each one."""
        if not self.mock_llm:
            raise Failure("mock-llm-asked needs mock-llm")
        log = self.out / "mock-llm.log"
        line = re.compile(rf"^mock-llm: asked {re.escape(model)} ", re.MULTILINE)
        self.wait_until(
            lambda: line.search(log.read_text(errors="replace")),
            timeout,
            f"a request for the model {model!r}",
        )

    def v_mock_acp_got(self, text, timeout=DEFAULT_TIMEOUT):
        """Wait until the ACP stand-in's log holds `text`: a stand-in that
        keeps one (the Claude Code one) writes a line for each session it
        opens and each prompt block it is sent."""
        if not self.mock_acp:
            raise Failure("mock-acp-got needs mock-acp")
        log = self.out / "mock-acp.log"
        self.wait_until(
            lambda: log.exists() and text in log.read_text(errors="replace"),
            timeout,
            f"{text!r} in the ACP stand-in's log",
        )

    def v_mock_acp_count(self, text, count):
        """Check that the ACP stand-in's log holds `text` exactly `count` times."""
        if not self.mock_acp:
            raise Failure("mock-acp-count needs mock-acp")
        log = self.out / "mock-acp.log"
        seen = log.read_text(errors="replace").count(text) if log.exists() else 0
        if seen != int(count):
            raise Failure(f"{text!r} {seen} times in the ACP stand-in's log, not {count}")

    def v_snapshot(self, name):
        self.need_term().pump(0.05)
        self.snapshots += 1
        file = self.out / f"{self.snapshots:02d}-{name}.txt"
        file.write_text(self.term.text() + "\n")

    def v_run(self, command):
        """Run a shell command in the project directory, on the host."""
        result = subprocess.run(
            command,
            shell=True,
            cwd=self.project or self.scratch,
            env=self.env(),
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode:
            raise Failure(f"`{command}` failed ({result.returncode}): {result.stderr.strip()}")

    def v_quit(self, *flags):
        """Alt+Q; `quit --force` also confirms a "Quit anyway?" question."""
        if set(flags) - {"--force"}:
            raise Failure("quit takes only --force")
        term = self.need_term()
        timeout = 5
        term.write(key_bytes("Alt+Q"))
        if "--force" in flags:
            end = time.monotonic() + 1
            while term.alive() and time.monotonic() < end:
                term.pump(0.05)
                if "Quit anyway?" in term.text():
                    term.write(b"y")
                    break
        end = time.monotonic() + float(timeout)
        while term.alive() and time.monotonic() < end:
            term.pump(0.05)
        if term.alive():
            raise Failure(f"termide still runs {timeout}s after Alt+Q")

    # the run

    def execute(self):
        lines = self.path.read_text().splitlines()
        started = time.monotonic()
        lineno = 0
        try:
            for lineno, raw in enumerate(lines, 1):
                line = raw.strip()
                if not line or line.startswith("#"):
                    continue
                words = shlex.split(line, comments=True)
                verb = getattr(self, "v_" + words[0].replace("-", "_"), None)
                if verb is None:
                    raise Failure(f"unknown step {words[0]!r}")
                try:
                    inspect.signature(verb).bind(*words[1:])
                except TypeError as e:
                    raise Failure(f"{words[0]}: {e}") from None
                verb(*words[1:])
                if self.args.verbose:
                    print(f"  {lineno:3}: {line}", file=sys.stderr)
            return None, time.monotonic() - started
        except Failure as e:
            return f"{self.path.name}:{lineno}: {e}", time.monotonic() - started
        finally:
            self.cleanup()

    def cleanup(self):
        """Stop termide and the mocks and remove the scratch directory, each
        whatever happened to the others."""
        steps = []
        if self.term:
            steps += [self.save_final_screen, self.term.stop, self.log.close]
        for helper in self.helpers:
            steps.append(lambda h=helper: stop_helper(h))
        steps += [f.close for f in self.files]
        if self.args.keep:
            steps.append(lambda: print(f"  scratch kept: {self.scratch}", file=sys.stderr))
        else:
            steps.append(lambda: shutil.rmtree(self.scratch, ignore_errors=True))
        for step in steps:
            try:
                step()
            except Exception as e:  # noqa: BLE001 - report and go on cleaning
                print(f"  cleanup: {e!r}", file=sys.stderr)

    def save_final_screen(self):
        self.term.pump(0.05)
        (self.out / "final.txt").write_text(self.term.text() + "\n")


def stop_helper(helper):
    helper.terminate()
    try:
        helper.wait(3)
    except subprocess.TimeoutExpired:
        helper.kill()
        helper.wait()


def wait_for_port(port, timeout=5.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            socket.create_connection(("127.0.0.1", port), 0.2).close()
            return
        except OSError:
            time.sleep(0.05)
    raise Failure(f"mock model did not listen on {port}")


def main():
    default_target = Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "target")
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument(
        "scenarios", nargs="*", help="scenario files or names (default: every scenarios/*.scn)"
    )
    p.add_argument(
        "--bin",
        type=Path,
        default=Path(os.environ.get("TERMIDE_BIN") or default_target / "debug" / "termide"),
    )
    p.add_argument("--out", type=Path, default=default_target / "tui-harness" / "out")
    p.add_argument(
        "--keep", action="store_true", help="keep each scenario's scratch HOME and project"
    )
    p.add_argument(
        "--key-delay", type=float, default=0.15, help="seconds to read output after each key"
    )
    p.add_argument("-v", "--verbose", action="store_true", help="print each step as it runs")
    args = p.parse_args()

    if not args.bin.is_file():
        sys.exit(f"tui-harness: no termide binary at {args.bin}; run `cargo build` or pass --bin")
    paths = []
    for s in args.scenarios or sorted(str(f) for f in SCENARIOS.glob("*.scn")):
        path = Path(s)
        if not path.exists():
            path = SCENARIOS / (s if s.endswith(".scn") else s + ".scn")
        if not path.exists():
            sys.exit(f"tui-harness: no scenario {s}")
        paths.append(path.resolve())

    failed = 0
    for path in paths:
        error, seconds = Run(path, args).execute()
        if error:
            failed += 1
            print(f"FAIL {path.stem} ({seconds:.1f}s)\n  {error}")
            final = args.out / path.stem / "final.txt"
            if final.exists():
                print(f"  screen ({final}):")
                for line in final.read_text().rstrip("\n").split("\n"):
                    print("  | " + line)
        else:
            print(f"ok   {path.stem} ({seconds:.1f}s)")
    print(f"{len(paths) - failed} passed, {failed} failed; screens in {args.out}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
