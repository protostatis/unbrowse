#!/usr/bin/env python3
"""MCP server for the minimal 3-tool SmartClient (PR #48).

Exposes as MCP tools:
  - search(query, count?) -> [{title,url,snippet}]
  - open(url, goal?, discover_limit?, cards_limit?) -> bundle with escalation/next_tools
  - help(topic?) -> grouped catalog

Run:
  UNBROWSER_BIN=$PWD/target/release/unbrowser PYTHONPATH=python python3 -m unbrowser.smart_mcp
  or via .mcp.json entry unbrowser-smart-py

Sanitized: no cookies/secrets emitted beyond SmartClient bundle (which is already sanitized).
"""

import json
import sys
from typing import Any, Optional

from . import __version__ as PKG_VERSION
from .smart import SmartClient, HELP_CATALOG

TOOLS = [
    {
        "name": "search",
        "title": "Web search",
        "description": "Brave HTML search (DDG fallback, optional BRAVE_API_KEY) -> [{title,url,snippet,display_url}]. Minimal entry 1.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query"},
                "count": {"type": "integer", "description": "Max hits (default 5)", "minimum": 1, "maximum": 20},
            },
            "required": ["query"],
        },
        "annotations": {"title": "Web search", "readOnlyHint": False, "destructiveHint": False, "idempotentHint": True, "openWorldHint": True},
    },
    {
        "name": "open",
        "title": "Open page",
        "description": "Fetch URL + bounded auto-discover (discover/cards) + stable escalation (partial_result/thin_shell/etc.), next_tools, avoid, micro_hint. Minimal entry 2 (navigate as open).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "Absolute URL (relative hrefs like '/' are resolved via last_url)"},
                "goal": {"type": "string", "description": "Task goal for ranking (e.g. 'Pixel 11 deal')"},
                "discover_limit": {"type": "integer", "description": "Max discover routes (default 12)"},
                "cards_limit": {"type": "integer", "description": "Max cards (default 10)"},
            },
            "required": ["url"],
        },
        "annotations": {"title": "Open page", "readOnlyHint": False, "destructiveHint": False, "idempotentHint": True, "openWorldHint": True},
    },
    {
        "name": "help",
        "title": "Tool catalog",
        "description": "Progressive discovery: grouped catalog of all 32 tools with when to use and example. help(topic) to filter.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "topic": {"type": "string", "description": "Optional filter: query, reading, discovery, extraction, interaction, session, or tool name"},
            },
            "required": [],
        },
        "annotations": {"title": "Tool catalog", "readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False},
    },
]

# Cross-tool guidance delivered to the model at handshake (MCP InitializeResult
# `instructions` field — read by Claude/ChatGPT/Codex hosts).
SERVER_INSTRUCTIONS = (
    "Start with open(url): the bundle carries blockmap, discover routes, cards, "
    "next_tools (ranked candidates), avoid (tools with nothing to act on), and "
    "micro_hint (the single next concrete step). Follow micro_hint when present; "
    "when tool_entropy marks the page ambiguous, gather information (query_debug/"
    "text_main) instead of committing to a tool. For bot-walled sites, replay a "
    "clearance cookie from the user's browser via cookies_set."
)

_client: Optional[SmartClient] = None


def get_client() -> SmartClient:
    global _client
    if _client is None:
        _client = SmartClient()
    return _client


def handle_tools_call(name: str, args: dict) -> Any:
    ub = get_client()
    if name == "search":
        return ub.search(args["query"], count=int(args.get("count", 5)))
    if name == "open":
        return ub.navigate_auto(
            args["url"],
            goal=args.get("goal"),
            discover_limit=int(args.get("discover_limit", 12)),
            cards_limit=int(args.get("cards_limit", 10)),
        )
    if name == "help":
        from .smart import _help_catalog

        return _help_catalog(args.get("topic"))
    # Teaching error: suggest close names so a typo'd call corrects in one
    # round-trip instead of failing twice.
    import difflib

    known = [t["name"] for t in TOOLS] + [n for fam in ("query", "reading", "extraction", "discovery", "interaction", "session") for n in (
        {"query": ["query", "query_debug", "query_text", "find_text", "text_around"],
         "reading": ["text", "text_main", "text_clean", "blockmap", "body"],
         "extraction": ["extract_table", "extract_list", "extract_cards", "table_to_json"],
         "discovery": ["discover", "route_discover", "page_model", "network_extract", "network_stores"],
         "interaction": ["click", "type", "submit", "activate", "settle"],
         "session": ["cookies_set", "cookies_get", "cookies_clear", "report_outcome"]}[fam]
    )]
    near = difflib.get_close_matches(name, known, n=3)
    hint = f" Did you mean: {', '.join(near)}?" if near else ""
    raise ValueError(f"unknown tool: {name}.{hint} Call help(topic) for the catalog.")


def main() -> None:
    import sys

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as e:
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": f"parse error: {e}"}}) + "\n")
            sys.stdout.flush()
            continue
        method = req.get("method", "")
        mid = req.get("id")
        params = req.get("params", {})
        # notifications have no id
        is_notification = mid is None
        try:
            if method == "initialize":
                result = {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "unbrowser", "title": "unbrowser — web access for LLM agents", "version": PKG_VERSION},
                    "instructions": SERVER_INSTRUCTIONS,
                }
            elif method == "ping":
                result = {}
            elif method == "tools/list":
                result = {"tools": TOOLS}
            elif method == "tools/call":
                name = params.get("name", "")
                arguments = params.get("arguments", {})
                value = handle_tools_call(name, arguments)
                result = {"content": [{"type": "text", "text": json.dumps(value, indent=2)}], "isError": False}
            elif method in ("resources/list", "prompts/list"):
                result = {"resources" if "resources" in method else "prompts": []}
            elif method in ("notifications/initialized", "notifications/cancelled"):
                continue
            else:
                raise ValueError(f"method not found: {method}")
        except Exception as e:
            if is_notification:
                continue
            err = {"code": -32603, "message": str(e)}
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": mid, "error": err}) + "\n")
            sys.stdout.flush()
            continue
        if is_notification:
            continue
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": mid, "result": result}) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
