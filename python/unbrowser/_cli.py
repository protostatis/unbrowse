"""Entry-point script that wraps the bundled native binary.

Registered in pyproject.toml as `[project.scripts] unbrowser = ...`, so
`pip install pyunbrowser` puts a real `unbrowser` command on $PATH that
agents and MCP hosts can use directly (e.g. `command: "unbrowser"` in
.mcp.json).

The wrapper keeps the native binary as the execution engine and exposes a
useful `--help` surface. Invocations are passed through to the binary.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from . import find_binary


def _usage() -> None:
    print(
        """unbrowser

Usage:
  unbrowser session start [--id <id>] [--profile <name>] [--policy=blocklist] [--shims stable|enhanced]
  unbrowser session exec [--pretty] <id|socket> <method> [params-json | shorthand args]
  unbrowser exec [--pretty] <id|socket> <method> [params-json | shorthand args]
  unbrowser session stop <id|socket>
  unbrowser session list
  unbrowser session prune
  unbrowser navigate <url> [--exec-scripts] [--json] [--events] [--shims stable|enhanced]
  unbrowser router <url> [--cookie-service <url>] [--no-auto-cookie-service]
  unbrowser cookie-service [--headless|--no-headless] [--port <port>] [--allow-host <host>] [--allow-remote-bind]
  unbrowser policy-check <url> [<url>...]
  unbrowser --list-profiles
  unbrowser --prefit-info
  unbrowser [--profile <name>] [--policy=blocklist] [--shims stable|enhanced] [--mcp]
  unbrowser --version

Examples:
  unbrowser session start --id demo
  unbrowser exec demo navigate https://news.ycombinator.com
  unbrowser exec --pretty demo blockmap
  unbrowser session stop demo
  unbrowser navigate https://news.ycombinator.com --json
  unbrowser cookie-service --headless --profile unbrowser-cookie-service
  unbrowser router https://example.com/protected
  unbrowser policy-check https://www.bbc.com/news
  printf '{\"id\":1,\"method\":\"navigate\",\"params\":{\"url\":\"https://news.ycombinator.com\"}}\n' | unbrowser

`navigate` delegates to the native binary; output is always the binary's JSON.
"""
    )


def _is_help_flag(arg: str) -> bool:
    return arg in {"-h", "--help"}


def _navigate(args: list[str]) -> None:
    binary = find_binary()
    completed = subprocess.run([binary, "navigate", *args], check=False)
    raise SystemExit(completed.returncode)


def _cookie_service(args: list[str]) -> None:
    try:
        from . import cookie_service
    except ImportError:
        _exec_source_script("cookie_service.py", args)
    sys.argv = ["unbrowser cookie-service", *args]
    raise SystemExit(cookie_service.main())


def _router(args: list[str]) -> None:
    try:
        from . import router
    except ImportError:
        _exec_source_script("router.py", args)
    sys.argv = ["unbrowser router", *args]
    router._demo()


def _exec_source_script(name: str, args: list[str]) -> None:
    repo = Path(__file__).resolve().parents[2]
    script = repo / "scripts" / name
    if not script.is_file():
        print(
            f"unbrowser: {name} is not bundled in this install. "
            "Install with `pip install 'pyunbrowser[solver]'` or run from a source checkout.",
            file=sys.stderr,
        )
        raise SystemExit(2)
    os.execv(sys.executable, [sys.executable, str(script), *args])


def main() -> None:
    argv = sys.argv[1:]
    if not argv or _is_help_flag(argv[0]):
        _usage()
        return

    if argv[0] == "navigate":
        _navigate(argv[1:])
        return

    if argv[0] in {"cookie-service", "cookie_service"}:
        _cookie_service(argv[1:])
        return

    if argv[0] == "router":
        _router(argv[1:])
        return

    binary = find_binary()
    # Preserve the native binary behavior for every other command.
    os.execv(binary, ["unbrowser", *argv])


if __name__ == "__main__":
    main()
