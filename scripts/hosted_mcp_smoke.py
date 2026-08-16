#!/usr/bin/env python3
"""Smoke test the hosted unbrowser MCP endpoint on unchainedsky.com."""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from typing import Any


DEFAULT_ENDPOINT = "https://unchainedsky.com/unbrowser-mcp"
EXPECTED_TOOL_COUNT = 32


def request(
    url: str,
    *,
    payload: dict[str, Any] | None = None,
    session_id: str = "",
    timeout: int = 60,
) -> tuple[int, dict[str, str], str]:
    headers = {
        "Accept": "application/json, text/event-stream",
        "User-Agent": "unbrowser-hosted-smoke/1.0",
    }
    data = None
    method = "GET"
    if payload is not None:
        method = "POST"
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    if session_id:
        headers["mcp-session-id"] = session_id
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, dict(resp.headers), resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:
        return exc.code, dict(exc.headers), exc.read().decode("utf-8", "replace")


def rpc(endpoint: str, payload: dict[str, Any], session_id: str = "") -> tuple[dict[str, Any], str]:
    status, headers, body = request(endpoint, payload=payload, session_id=session_id)
    if status not in (200, 202):
        raise AssertionError(f"RPC HTTP {status}: {body[:300]}")
    if not body:
        return {}, headers.get("mcp-session-id") or headers.get("Mcp-Session-Id") or ""
    parsed = json.loads(body)
    if "error" in parsed:
        raise AssertionError(f"RPC error: {parsed['error']}")
    return parsed, headers.get("mcp-session-id") or headers.get("Mcp-Session-Id") or ""


def initialize(endpoint: str) -> tuple[str, dict[str, Any]]:
    payload = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "hosted-mcp-smoke", "version": "1.0"},
        },
    }
    parsed, session_id = rpc(endpoint, payload)
    if not session_id:
        raise AssertionError("initialize did not return mcp-session-id")
    info = parsed.get("result", {}).get("serverInfo", {})
    if info.get("name") != "unbrowser":
        raise AssertionError(f"unexpected serverInfo.name: {info!r}")
    rpc(endpoint, {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}, session_id)
    return session_id, info


def call_tool(endpoint: str, session_id: str, name: str, arguments: dict[str, Any], request_id: int) -> dict[str, Any]:
    payload = {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    }
    parsed, _ = rpc(endpoint, payload, session_id)
    result = parsed.get("result", {})
    if result.get("isError"):
        raise AssertionError(f"tool {name} returned isError: {result}")
    text = (result.get("content") or [{}])[0].get("text", "")
    return json.loads(text)


def status_payload_is_ready(payload: dict[str, Any]) -> bool:
    """Accept the legacy deployment map and the current capacity contract."""
    if payload.get("server_instances", {}).get("default") == "configured":
        return True

    active_sessions = payload.get("active_sessions")
    capacity = payload.get("capacity")
    return (
        payload.get("status") == "ok"
        and isinstance(active_sessions, int)
        and not isinstance(active_sessions, bool)
        and isinstance(capacity, int)
        and not isinstance(capacity, bool)
        and capacity > 0
        and 0 <= active_sessions <= capacity
    )


def assert_status_endpoint(endpoint: str) -> None:
    status, _, body = request(endpoint.rstrip("/") + "/status", timeout=20)
    if status != 200:
        raise AssertionError(f"status endpoint returned HTTP {status}: {body[:300]}")
    parsed = json.loads(body)
    if not status_payload_is_ready(parsed):
        raise AssertionError(f"unexpected status payload: {parsed!r}")


def assert_sse_disabled(endpoint: str) -> None:
    status, _, body = request(endpoint.rstrip("/") + "/sse", timeout=20)
    if status != 404:
        raise AssertionError(f"/sse returned HTTP {status}, expected 404: {body[:300]}")


def assert_tools(endpoint: str, session_id: str) -> list[dict[str, Any]]:
    parsed, _ = rpc(
        endpoint,
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
        session_id,
    )
    tools = parsed.get("result", {}).get("tools", [])
    if len(tools) != EXPECTED_TOOL_COUNT:
        raise AssertionError(f"tools/list returned {len(tools)}, expected {EXPECTED_TOOL_COUNT}")
    if not any(tool.get("name") == "navigate" for tool in tools):
        raise AssertionError("tools/list missing navigate")
    return tools


def assert_hn_navigate(endpoint: str, session_id: str) -> None:
    nav = call_tool(endpoint, session_id, "navigate", {"url": "https://news.ycombinator.com"}, 3)
    title = nav.get("blockmap", {}).get("title")
    if nav.get("status") != 200 or title != "Hacker News" or nav.get("challenge") is not None:
        raise AssertionError(f"unexpected HN navigate result: status={nav.get('status')} title={title!r} challenge={nav.get('challenge')!r}")


def assert_internal_targets_blocked(endpoint: str, session_id: str) -> None:
    targets = [
        "http://private-core:8770",
        "http://relay:8765/health",
        "http://localhost:8770",
        "http://169.254.169.254/latest/meta-data/",
    ]
    for idx, url in enumerate(targets, start=10):
        nav = call_tool(endpoint, session_id, "navigate", {"url": url}, idx)
        if nav.get("status") != 403:
            raise AssertionError(f"internal target was not blocked with 403: {url} -> {nav.get('status')}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--endpoint", default=DEFAULT_ENDPOINT)
    parser.add_argument("--skip-ssrf", action="store_true", help="Skip internal-target SSRF checks")
    args = parser.parse_args()

    endpoint = args.endpoint.rstrip("/")
    assert_status_endpoint(endpoint)
    assert_sse_disabled(endpoint)

    session_id, info = initialize(endpoint)
    explicit_session_id, explicit_info = initialize(endpoint + "/mcp")
    tools = assert_tools(endpoint, session_id)
    assert_hn_navigate(endpoint, session_id)
    if not args.skip_ssrf:
        assert_internal_targets_blocked(endpoint, session_id)

    print(json.dumps({
        "ok": True,
        "endpoint": endpoint,
        "serverInfo": info,
        "explicitMcpServerInfo": explicit_info,
        "explicitMcpSession": bool(explicit_session_id),
        "tools": len(tools),
        "ssrfChecked": not args.skip_ssrf,
    }, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001 - script should print concise CI failure.
        print(json.dumps({"ok": False, "error": str(exc)}, sort_keys=True), file=sys.stderr)
        raise SystemExit(1)
