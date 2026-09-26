#!/usr/bin/env python3
"""Starts and stops a local peek stack: fake IAM + fake Ting, the real
peek-server (temp SQLite) and the real peekd in an isolated run (BLUEPRINT
decision 6), all under one directory. Used by run-local.sh / stop.sh and by
e2e.py.

    python3 scripts/e2e/local_stack.py start [--dir DIR] [--no-build] [--no-deepgram]
                                             [--ui-executable PATH]
    python3 scripts/e2e/local_stack.py stop [DIR]
    python3 scripts/e2e/local_stack.py env [DIR]

Nothing touches the real user's state: peekd gets PEEK_SUPPORT_DIR,
PEEK_CACHES_DIR, PEEK_DAEMON_SOCKET and PEEK_NO_SERVICES=1 (never launches
Peek.app, never runs the updater or watchdog); the CLI environment written to
<DIR>/env.sh adds SILICON_HOME and the install hooks. The Deepgram key is read
from PEEK_DEEPGRAM_KEY_FILE (default ~/.peek-operator/deepgram-api-key) and
handed to peek-server through its environment only; it is never printed or
written under DIR.
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import shlex
import signal
import socket
import string
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
TARGET = Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "target")
BIN = TARGET / "debug"
CURRENT = TARGET / "peek-local.current"
DEFAULT_KEY_FILE = Path.home() / ".peek-operator" / "deepgram-api-key"
ORG = "tos"


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def app_secret() -> str:
    alphabet = string.ascii_letters + string.digits
    return "ask_" + "".join(secrets.choice(alphabet) for _ in range(43))


def build() -> None:
    subprocess.run(
        ["cargo", "build", "-p", "silicon-peek", "--bin", "peek-server", "-p", "silicon-peek-daemon", "-p", "silicon-peek-cli"],
        cwd=REPO,
        check=True,
        env={**os.environ, "CARGO_TARGET_DIR": str(TARGET)},
    )


def base_env() -> dict[str, str]:
    """The caller's environment without any PEEK_/SILICON_ variable, so a
    developer's own settings never leak into the isolated stack."""
    return {k: v for k, v in os.environ.items() if not k.startswith(("PEEK_", "SILICON_", "ISI"))}


def wait_until(check, timeout: float, what: str) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if check():
                return
        except OSError:
            pass
        time.sleep(0.1)
    raise TimeoutError(f"timed out waiting for {what}")


def http_ok(url: str) -> bool:
    with urllib.request.urlopen(url, timeout=2) as r:  # noqa: S310 - loopback only
        return r.status == 200


class Stack:
    """Paths, ports and environments of one local stack (see start())."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.state = json.loads((root / "stack.json").read_text()) if (root / "stack.json").exists() else {}

    @property
    def api(self) -> str:
        return f"http://127.0.0.1:{self.state['ports']['server']}"

    @property
    def iam(self) -> str:
        return f"http://127.0.0.1:{self.state['ports']['iam']}"

    @property
    def ting(self) -> str:
        return f"http://127.0.0.1:{self.state['ports']['ting']}"

    @property
    def socket(self) -> str:
        return str(Path(self.state["socket_dir"]) / "peekd.sock")

    def isolated_env(self) -> dict[str, str]:
        """What peekd, the CLI and Peek.app share (decision 6)."""
        return {
            "PEEK_SUPPORT_DIR": str(self.root / "support"),
            "PEEK_CACHES_DIR": str(self.root / "caches"),
            "PEEK_DAEMON_SOCKET": self.socket,
            "PEEK_NO_SERVICES": "1",
            "PEEK_APPLICATIONS_DIR": str(self.root / "Applications"),
            "PEEK_API_URL": self.api,
            "PEEK_TELEMETRY": "off",
        }

    def cli_env(self) -> dict[str, str]:
        return {
            **self.isolated_env(),
            "SILICON_HOME": str(self.root / "silicon"),
            "SILICON_ORG": ORG,
            "PEEK_INSTALL_NO_LAUNCH": "1",
            "PEEK_INSTALL_APPLICATIONS_DIR": str(self.root / "Applications"),
            "PEEK_INSTALL_SUPPORT_DIR": str(self.root / "support"),
        }

    def pids(self) -> dict[str, int]:
        return self.state.get("pids", {})


def start(root: Path | None = None, *, no_build: bool = False, deepgram: bool = True, ui_executable: str | None = None, quiet: bool = False) -> Stack:
    if not no_build:
        build()
    for exe in ("peek-server", "peekd", "peek"):
        if not (BIN / exe).exists():
            raise SystemExit(f"{BIN / exe} is missing; run without --no-build")
    if root is None:
        root = Path(tempfile.mkdtemp(prefix="peek-local."))
    root = root.resolve()
    for d in ("silicon", "support", "caches", "server", "Applications", "logs"):
        (root / d).mkdir(parents=True, exist_ok=True)
        os.chmod(root / d, 0o700)
    os.chmod(root, 0o700)
    ports = {"iam": free_port(), "ting": free_port(), "server": free_port()}
    secret = app_secret()
    secret_file = root / "app-secret"
    secret_file.write_text(secret)
    os.chmod(secret_file, 0o600)
    logs = root / "logs"
    pids: dict[str, int] = {}
    # A Unix socket path must fit in sun_path (104 bytes on macOS), so the
    # socket gets a short private directory of its own (removed by stop()).
    socket_dir = tempfile.mkdtemp(prefix="peek-e2e-", dir="/var/tmp")
    os.chmod(socket_dir, 0o700)
    state = {"ports": ports, "pids": pids, "started_at": time.time(), "deepgram": False, "socket_dir": socket_dir}

    def save() -> None:
        (root / "stack.json").write_text(json.dumps(state, indent=2))

    def spawn(name: str, argv: list[str], env: dict[str, str], cwd: Path) -> subprocess.Popen:
        log = open(logs / f"{name}.log", "ab")  # noqa: SIM115 - handed to the child
        p = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        pids[name] = p.pid
        save()
        return p

    try:
        ready = root / "fakes.json"
        spawn("fakes", [sys.executable, str(HERE / "fake_services.py"), "--iam-port", str(ports["iam"]), "--ting-port", str(ports["ting"]), "--app-secret-file", str(secret_file), "--ready-file", str(ready)], base_env(), root)
        wait_until(ready.exists, 10, "the fake IAM and Ting")

        key = ""
        key_file = Path(os.environ.get("PEEK_DEEPGRAM_KEY_FILE") or DEFAULT_KEY_FILE)
        if deepgram and key_file.is_file():
            key = key_file.read_text().strip()
        state["deepgram"] = bool(key)
        api = f"http://127.0.0.1:{ports['server']}"
        server_env = {
            **base_env(),
            "RUST_LOG": os.environ.get("PEEK_LOCAL_RUST_LOG", "info"),
            "PEEK_BIND": f"127.0.0.1:{ports['server']}",
            "PEEK_DATABASE_PATH": str(root / "server" / "peek.sqlite"),
            "PEEK_TEST_DATABASE_PATH": str(root / "server" / "testing.sqlite"),
            "PEEK_PUBLIC_ORIGIN": api,
            "PEEK_WEB_ORIGINS": "http://localhost:5173",
            "PEEK_ENVIRONMENT": "development",
            "PEEK_IAM_BASE_URL": f"http://127.0.0.1:{ports['iam']}",
            "PEEK_IAM_APP_ID": "peek",
            "PEEK_IAM_APP_SECRET": secret,
            "PEEK_IAM_WEBHOOK_SECRET": secrets.token_hex(32),
            "PEEK_IAM_WEBHOOK_KEY_VERSION": "1",
            "PEEK_IAM_SDK_TELEMETRY": "off",
            "PEEK_TING_BASE_URL": f"http://127.0.0.1:{ports['ting']}",
            "PEEK_HONEYCOMB_URL": "http://127.0.0.1:9",
            "PEEK_ENCRYPTION_KEY": secrets.token_hex(32),
            "PEEK_DEEPGRAM_API_KEY": key,
            "PEEK_TELEMETRY": "off",
            "PEEK_TELEMETRY_HOME": str(root / "server" / "telemetry"),
            "PEEK_GITHUB_ISSUES_TOKEN": "",
        }
        spawn("peek-server", [str(BIN / "peek-server")], server_env, root / "server")
        wait_until(lambda: http_ok(f"{api}/healthz"), 20, "peek-server /healthz")

        stack = Stack(root)
        stack.state = state
        peekd_env = {**base_env(), **stack.isolated_env(), "PEEKD_LOG": os.environ.get("PEEK_LOCAL_PEEKD_LOG", "info")}
        if ui_executable:
            peekd_env["PEEK_UI_EXECUTABLE"] = ui_executable
        spawn("peekd", [str(BIN / "peekd"), "run", "--parent-ui"], peekd_env, root)
        wait_until(lambda: os.path.exists(stack.socket), 20, "peekd's socket")
        save()
        write_env(stack)
        CURRENT.parent.mkdir(parents=True, exist_ok=True)
        CURRENT.write_text(str(root))
        if not quiet:
            print_summary(stack)
        return stack
    except BaseException:
        save()
        stop(root)
        raise


def write_env(stack: Stack) -> None:
    lines = ["# source this file: the CLI talks to the local stack", *(f"export {k}={shlex.quote(v)}" for k, v in sorted(stack.cli_env().items()))]
    lines.append(f"export PATH={shlex.quote(str(BIN))}:\"$PATH\"")
    (stack.root / "env.sh").write_text("\n".join(lines) + "\n")
    app = [f"{k}={v}" for k, v in sorted(stack.isolated_env().items()) if k != "PEEK_TELEMETRY"]
    (stack.root / "app.env").write_text("\n".join(app) + "\n")


def print_summary(stack: Stack) -> None:
    deepgram = "real Deepgram via the speech proxy" if stack.state.get("deepgram") else "no Deepgram key (speech degrades to text)"
    print(f"""peek local stack in {stack.root}
  peek-server  {stack.api}   ({deepgram})
  fake IAM     {stack.iam}   (any SLT logs in as si:e2e-silicon in tos)
  fake Ting    {stack.ting}   (GET {stack.ting}/_fake/tings lists received tings)
  peekd        {stack.socket}
  logs         {stack.root / 'logs'}  (peekd.log is in {stack.root / 'support'})

CLI:      source {stack.root / 'env.sh'} && peek login any-slt && peek register side 3
Peek.app: start the dev build with the isolated variables in {stack.root / 'app.env'}, e.g.
          env $(cat {stack.root / 'app.env'} | xargs) <DerivedData>/Peek.app/Contents/MacOS/Peek
          (peekd outside a bundle accepts any …/Peek.app/Contents/MacOS/Peek)
Stop:     scripts/e2e/stop.sh {stack.root}""")


def stop(root: Path | None = None) -> None:
    if root is None:
        if not CURRENT.exists():
            print("no local stack recorded; pass its directory", file=sys.stderr)
            return
        root = Path(CURRENT.read_text().strip())
    stack = Stack(root)
    pids = stack.pids()
    for name in ("peekd", "peek-server", "fakes"):
        pid = pids.get(name)
        if pid and not terminate(pid):
            print(f"warning: {name} (pid {pid}) did not stop", file=sys.stderr)
    socket_dir = stack.state.get("socket_dir")
    if socket_dir and os.path.isdir(socket_dir):
        for leftover in ("peekd.sock", "peekd.lock"):
            try:
                os.unlink(os.path.join(socket_dir, leftover))
            except FileNotFoundError:
                pass
        try:
            os.rmdir(socket_dir)
        except OSError as e:
            print(f"warning: could not remove {socket_dir}: {e}", file=sys.stderr)
    if CURRENT.exists() and CURRENT.read_text().strip() == str(root):
        CURRENT.unlink()


def gone(pid: int) -> bool:
    """Whether `pid` has exited (reaping it when it is our own child)."""
    try:
        done, _ = os.waitpid(pid, os.WNOHANG)
        if done == pid:
            return True
    except ChildProcessError:
        pass
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    except PermissionError:
        return False
    return False


def terminate(pid: int, grace: float = 10.0) -> bool:
    """SIGTERM the child's process group, then SIGKILL after `grace` seconds."""
    for sig in (signal.SIGTERM, signal.SIGKILL):
        if gone(pid):
            return True
        try:
            os.killpg(pid, sig)
        except (ProcessLookupError, PermissionError):
            try:
                os.kill(pid, sig)
            except (ProcessLookupError, PermissionError):
                pass
        deadline = time.monotonic() + grace
        while time.monotonic() < deadline:
            if gone(pid):
                return True
            time.sleep(0.05)
    return gone(pid)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("start")
    s.add_argument("--dir", type=Path)
    s.add_argument("--no-build", action="store_true")
    s.add_argument("--no-deepgram", action="store_true")
    s.add_argument("--ui-executable", help="the only executable peekd accepts as the UI (PEEK_UI_EXECUTABLE)")
    t = sub.add_parser("stop")
    t.add_argument("dir", nargs="?", type=Path)
    e = sub.add_parser("env")
    e.add_argument("dir", nargs="?", type=Path)
    args = parser.parse_args()
    if args.cmd == "start":
        start(args.dir, no_build=args.no_build, deepgram=not args.no_deepgram, ui_executable=args.ui_executable)
    elif args.cmd == "stop":
        stop(args.dir)
    else:
        root = args.dir or Path(CURRENT.read_text().strip())
        print((root / "env.sh").read_text(), end="")


if __name__ == "__main__":
    main()
