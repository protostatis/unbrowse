"""Entry-point script that wraps the bundled native binary.

Registered in pyproject.toml as `[project.scripts] unbrowser = ...`, so
`pip install pyunbrowser` puts a real `unbrowser` command on $PATH that
agents and MCP hosts can use directly (e.g. `command: "unbrowser"` in
.mcp.json).

The wrapper keeps the native binary as the execution engine. Help follows
progressive-disclosure conventions (clig.dev): `--help` shows the core path
plus grouped tool families; `unbrowser help <topic>` drills into any family
or tool; unknown commands get did-you-mean suggestions on stderr with
exit code 2.
"""

from __future__ import annotations

import difflib
import json
import os
import subprocess
import sys
from pathlib import Path

from . import find_binary


# Grouped tool families — mirrors HELP_CATALOG in unbrowser/smart.py and the
# Rust MCP surface. Kept as plain data so `--help` renders without importing
# the smart layer.
TOOL_FAMILIES: dict[str, list[str]] = {
    "reading": ["text", "text_main", "text_clean", "blockmap", "body"],
    "query": ["query", "query_debug", "query_text", "find_text", "text_around"],
    "extraction": ["extract", "extract_table", "extract_list", "extract_cards", "table_to_json"],
    "discovery": ["discover", "route_discover", "page_model", "network_extract", "network_stores"],
    "interaction": ["click", "type", "submit", "activate", "settle", "eval"],
    "session": ["cookies_set", "cookies_get", "cookies_clear", "report_outcome"],
}

_KNOWN_COMMANDS = [
    "navigate", "search", "open", "help", "exec", "session",
    "router", "cookie-service", "policy-check", "--mcp", "--version",
    "--list-profiles", "--prefit-info",
]


def _usage() -> None:
    fams = "\n".join(f"  {fam:<12} {' '.join(tools)}" for fam, tools in TOOL_FAMILIES.items())
    print(
        f"""unbrowser — web access for LLM agents. One static binary. No Chrome.

START HERE
  unbrowser navigate <url> [--exec-scripts]    fetch a page -> low-token BlockMap
  unbrowser search "<query>" [--count N]       web search (Brave->DDG) -> [{{title,url,snippet}}]
  unbrowser open <url> [--goal G]              fetch + auto-discover + next-step hints
  unbrowser --mcp                              MCP server mode for agent hosts

MULTI-STEP SESSIONS (cookies + last page persist)
  unbrowser session start [--id <id>] [--profile <name>] [--policy=blocklist]
  unbrowser exec [--pretty] <id|socket> <method> [params-json | shorthand args]
  unbrowser session stop <id|socket> | session list | session prune

TOOLS — call via `unbrowser exec <id> <method> '{{...}}'`, or over MCP
{fams}

  unbrowser help <family|tool>    details + examples (e.g. `unbrowser help extraction`)

MORE
  unbrowser router <url>                 bot-wall cookie handoff via local Chrome
  unbrowser cookie-service [--headless]  local solver service (needs [solver] extra)
  unbrowser policy-check <url> [<url>...]
  unbrowser --list-profiles | --prefit-info | --version

Every result carries routing hints: micro_hint (the next concrete step),
next_tools (ranked candidates), avoid (tools with nothing to act on).
"""
    )


def _help_topic(topic: str | None) -> int:
    """Render the grouped catalog, one family, or one tool. Exit 0."""
    try:
        from .smart import HELP_CATALOG
    except ImportError:
        print("help catalog unavailable in this install", file=sys.stderr)
        return 1
    if not topic:
        for fam, tools in HELP_CATALOG.items():
            print(f"{fam}:")
            for name, info in tools.items():
                print(f"  {name:<16} {info.get('when', '')}")
        print("\nDrill in: unbrowser help <family|tool>  e.g. unbrowser help extract_table")
        return 0
    t = topic.lower()
    for fam, tools in HELP_CATALOG.items():
        if t == fam:
            print(f"{fam}:")
            for name, info in tools.items():
                print(f"\n  {name}\n    {info.get('when', '')}")
                if info.get("example"):
                    print(f"    e.g. {info['example']}")
            return 0
        if t in tools:
            info = tools[t]
            print(f"{t}  ({fam})\n  {info.get('when', '')}")
            if info.get("example"):
                print(f"  e.g. {info['example']}")
            return 0
    # fuzzy fallback
    matches = difflib.get_close_matches(t, [n for f_ in HELP_CATALOG.values() for n in f_], n=3)
    if matches:
        print(f"unknown topic '{topic}'. Did you mean: {', '.join(matches)}?")
    else:
        print(f"unknown topic '{topic}'")
    return 1


def _suggest_and_exit(bad: str) -> None:
    matches = difflib.get_close_matches(bad, _KNOWN_COMMANDS + [n for f_ in TOOL_FAMILIES.values() for n in f_], n=3)
    hint = f" Did you mean: {', '.join(matches)}?" if matches else ""
    print(f"unbrowser: unknown command '{bad}'.{hint}\nRun `unbrowser --help` to see what's available.", file=sys.stderr)
    raise SystemExit(2)


def _pop_value(args: list[str], flag: str) -> str | None:
    """Pop `flag <value>` from args; exit 2 cleanly when the value is missing."""
    if flag not in args:
        return None
    i = args.index(flag)
    if i + 1 >= len(args):
        print(
            f"unbrowser: {flag} requires a value.\n"
            "Run `unbrowser --help` to see what's available.",
            file=sys.stderr,
        )
        raise SystemExit(2)
    val = args[i + 1]
    del args[i : i + 2]
    return val


def _cmd_search(args: list[str]) -> None:
    count = 5
    raw_count = _pop_value(args, "--count")
    if raw_count is not None:
        try:
            count = int(raw_count)
        except ValueError:
            print("unbrowser: --count must be an integer.", file=sys.stderr)
            raise SystemExit(2)
    query = " ".join(a for a in args if not a.startswith("-"))
    if not query:
        print("usage: unbrowser search \"<query>\" [--count N]", file=sys.stderr)
        raise SystemExit(2)
    from .smart import SmartClient

    with SmartClient() as ub:
        hits = ub.search(query, count=count)
    print(json.dumps(hits, indent=2))


def _cmd_open(args: list[str]) -> None:
    goal = _pop_value(args, "--goal")
    url = next((a for a in args if not a.startswith("-")), None)
    if not url:
        print("usage: unbrowser open <url> [--goal G]", file=sys.stderr)
        raise SystemExit(2)
    from .smart import SmartClient

    with SmartClient() as ub:
        bundle = ub.navigate_auto(url, goal=goal)
    print(json.dumps(bundle, indent=2))


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

    if argv[0] == "help":
        raise SystemExit(_help_topic(argv[1] if len(argv) > 1 else None))

    if argv[0] == "search":
        _cmd_search(argv[1:])
        return

    if argv[0] == "open":
        _cmd_open(argv[1:])
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

    # Pass through known binary commands and flags; anything else gets a
    # did-you-mean instead of a cryptic binary error.
    if argv[0].startswith("-") or argv[0] in {"session", "exec", "policy-check"}:
        binary = find_binary()
        os.execv(binary, ["unbrowser", *argv])
    _suggest_and_exit(argv[0])


if __name__ == "__main__":
    main()
