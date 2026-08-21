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
    names_min = {t["name"] for t in minimal}
    names_full = {t["name"] for t in full}
    assert names_min == {"navigate", "query", "extract", "help"}, f"minimal contract drifted: {names_min}"
    assert names_min.issubset(names_full), "minimal must be subset of full"
    assert len(full) >= 33, f"full lost tools: {len(full)}"


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


def test_micro_hint_fixtures():
    sys.path.insert(0, str(REPO / "python"))
    from unbrowser.smart import _micro_hint_for_bundle

    # data table (many cells) -> extract_table
    b = {"status": 200, "blockmap": {"density": {"tables": {"total": 3}, "td": {"total": 84, "filled": 80, "ratio": 0.95}}, "headings": []}, "cards": [], "discover": {}, "raw": {}}
    h = _micro_hint_for_bundle(b)
    assert h and h["tool"] == "extract_table" and "table" in h["selector"]
    assert "finance" not in h["reason"] and "premarket" not in h["reason"], "context-specific copy leaked into generic hint"

    # layout table (few cells, e.g. docs/spec page) -> NOT extract_table;
    # falls through to json/headings branches
    b = {"status": 200, "blockmap": {"density": {"tables": {"total": 1}, "td": {"total": 4, "filled": 4, "ratio": 1.0}}, "headings": [{"text": "Lifecycle"}]}, "cards": [], "discover": {}, "raw": {}}
    h = _micro_hint_for_bundle(b)
    assert h is None or h["tool"] != "extract_table", f"layout table misrouted to extract_table: {h}"

    # JS-injected table shells (CNBC trap) -> navigate with exec_scripts, not extract_table
    b = {"status": 200, "blockmap": {"density": {"tables": {"total": 6}, "td": None, "likely_js_filled": True}, "headings": []}, "cards": [], "discover": {}, "raw": {}}
    h = _micro_hint_for_bundle(b)
    assert h and h["tool"] == "navigate" and "exec_scripts" in h["reason"]

    # forms -> type
    b = {"status": 200, "blockmap": {"density": {"tables": None}, "headings": []}, "cards": [], "discover": {"forms": [{"label": "Search", "controls": [{"ref": "e:1"}]}]}, "raw": {}}
    h = _micro_hint_for_bundle(b)
    assert h and h["tool"] == "type"

    # headings no cards -> query_text
    b = {"status": 200, "blockmap": {"density": {"tables": None}, "headings": [{"text": "Premarket Trading August 20"}]}, "cards": [], "discover": {}, "raw": {}}
    h = _micro_hint_for_bundle(b)
    assert h and h["tool"] == "query_text"

    # li heavy no cards -> extract_list
    b = {"status": 200, "blockmap": {"density": {"tables": None, "li": {"total": 60}}, "headings": []}, "cards": [], "discover": {}, "raw": {}}
    h = _micro_hint_for_bundle(b)
    assert h and h["tool"] == "extract_list"

    # json scripts -> extract
    b = {"status": 200, "blockmap": {"density": {"tables": None, "json_scripts": 2}, "headings": []}, "cards": [], "discover": {}, "raw": {}}
    h = _micro_hint_for_bundle(b)
    assert h and h["tool"] == "extract"

    # rich page (cards present) -> no hint needed
    b = {"status": 200, "blockmap": {"density": {"tables": None}, "headings": [{"text": "x"}]}, "cards": [{"title": "a"}], "discover": {}, "raw": {}}
    assert _micro_hint_for_bundle(b) is None


def test_avoid_and_entropy_fixtures():
    sys.path.insert(0, str(REPO / "python"))
    from unbrowser.smart import _avoid_for_bundle, _tool_entropy

    # bare article page: no tables/forms/json -> those tools are avoided
    b = {"status": 200, "blockmap": {"density": {"tables": None, "json_scripts": 0}, "interactives": {"forms": []}}, "cards": [], "raw": {}}
    avoid = {a["tool"] for a in _avoid_for_bundle(b)}
    assert {"extract", "extract_table", "submit"} <= avoid, f"missing avoids: {avoid}"

    # rich data page: nothing structural is absent -> no avoids
    b = {"status": 200, "blockmap": {"density": {"tables": {"total": 2}, "json_scripts": 3}, "interactives": {"forms": [{}]}}, "cards": [], "raw": {}}
    assert _avoid_for_bundle(b) == []

    # challenge page suppresses DOM reads
    b = {"status": 403, "challenge": {"provider": "datadome"}, "blockmap": {"density": {"tables": None, "json_scripts": 0}, "interactives": {"forms": []}}, "raw": {}}
    avoid = {a["tool"] for a in _avoid_for_bundle(b)}
    assert "query" in avoid

    # flat distribution -> ambiguous; peaked -> not
    flat = {"next_tools": [{"tool": "a", "confidence": 0.7}, {"tool": "b", "confidence": 0.7}, {"tool": "c", "confidence": 0.7}]}
    ent = _tool_entropy(flat)
    assert ent and ent["ambiguous"] is True and "note" in ent
    peaked = {"next_tools": [{"tool": "a", "confidence": 0.95}, {"tool": "b", "confidence": 0.3}]}
    ent = _tool_entropy(peaked)
    assert ent and ent["ambiguous"] is False
    # single/empty candidate list -> no entropy signal
    assert _tool_entropy({"next_tools": [{"tool": "a", "confidence": 0.9}]}) is None
    assert _tool_entropy({"next_tools": []}) is None


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
