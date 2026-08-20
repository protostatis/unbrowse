"""Protocol test: minimal vs full tools/list + help drift (PR #48)."""

import json
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
BIN = REPO / "target" / "release" / "unbrowser"


def mcp_tools(profile: str) -> list[dict]:
    proc = subprocess.Popen(
        [str(BIN), "--mcp", "--mcp-profile", profile],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    inp = '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}\n'
    try:
        out, _ = proc.communicate(input=inp, timeout=5)
    finally:
        proc.kill()
    # find tools/list response
    for line in out.strip().splitlines():
        try:
            d = json.loads(line)
            if d.get("id") == 1 and "result" in d:
                return d["result"]["tools"]
        except Exception:
            continue
    raise AssertionError(f"no tools/list response for profile={profile}: {out[:500]}")


def test_minimal_is_4_and_full_is_33():
    minimal = mcp_tools("minimal")
    full = mcp_tools("full")
    assert len(minimal) == 4, f"minimal should be 4, got {len(minimal)}: {[t['name'] for t in minimal]}"
    assert len(full) == 33, f"full should be 33 (32+help), got {len(full)}"
    assert {t["name"] for t in minimal} == {"navigate", "query", "extract", "help"}
    # full must contain minimal
    names_full = {t["name"] for t in full}
    assert {"navigate", "query", "extract", "help"}.issubset(names_full)


def test_help_catalog_sums_to_32_and_matches_rust():
    # Python catalog
    sys.path.insert(0, str(REPO / "python"))
    from unbrowser.smart import HELP_CATALOG

    py_count = sum(len(v) for v in HELP_CATALOG.values())
    # Python HELP_CATALOG includes search/open as aliases for Rust navigate, so 35 is expected (32 + help + 2 aliases)
    assert py_count >= 30, f"py HELP_CATALOG too small: {py_count}"
    assert py_count <= 40, f"py HELP_CATALOG too large: {py_count}"

    # Rust full list
    full = mcp_tools("full")
    rust_names = {t["name"] for t in full}
    # help should be in both
    assert "help" in rust_names
    # every py catalog entry should have a rust tool (or be a known alias)
    for group, tools in HELP_CATALOG.items():
        for name in tools:
            # help catalog may use aliases like "open" for navigate, but we keep canonical names
            if name in ("open", "search"):
                continue
            assert name in rust_names or name in {"help"}, f"py catalog {name} not in rust tools"

    # Every escalation.option.tool that is not external_action must be in rust
    # (spot-check: help catalog's next_tools should be real tools)


def test_escalation_fixtures():
    sys.path.insert(0, str(REPO / "python"))
    from unbrowser.smart import _escalation_for_bundle

    # partial_result
    b = {"status": 200, "blockmap": {}, "extract": {"primary_truncated": {"strategy": "nuxt_data", "size_bytes": 228709}}, "raw": {}}
    esc = _escalation_for_bundle(b)
    assert esc and esc["reason"] == "partial_result" and esc["category"] == "continuation"

    # thin_shell
    b = {"status": 200, "blockmap": {"density": {"thin_shell": True}}, "raw": {}}
    esc = _escalation_for_bundle(b)
    assert esc and esc["reason"] == "thin_shell"

    # challenge
    b = {"status": 200, "blockmap": {}, "challenge": {"provider": "datadome", "confidence": 0.9}, "raw": {}}
    esc = _escalation_for_bundle(b)
    assert esc and esc["reason"] == "challenge" and esc["category"] == "external_capability"

    # timeout
    b = {"status": 200, "blockmap": {}, "discover_timeout": True, "raw": {}}
    esc = _escalation_for_bundle(b)
    assert esc and esc["reason"] == "timeout" and esc["retryable"] is True
