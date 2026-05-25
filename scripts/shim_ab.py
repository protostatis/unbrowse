#!/usr/bin/env python3
"""A/B runner for stable vs enhanced unbrowser shims.

Outputs one JSON object per (url, mode) with cheap materialization metrics:
text length, DOM counts, density flags, script outcome, network capture count,
and challenge provider. Intended for corpus sweeps, not correctness judging.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


def rpc(binary: str, mode: str, requests: list[dict[str, Any]], timeout: float) -> list[dict[str, Any]]:
    proc = subprocess.Popen(
        [binary, "--shims", mode],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    assert proc.stdin is not None and proc.stdout is not None
    try:
        for req in requests:
            proc.stdin.write(json.dumps(req) + "\n")
        proc.stdin.flush()
        out: list[dict[str, Any]] = []
        deadline = time.time() + timeout
        while len(out) < len(requests):
            if time.time() > deadline:
                raise TimeoutError(f"timed out waiting for {mode}")
            line = proc.stdout.readline()
            if not line:
                break
            out.append(json.loads(line))
        return out
    finally:
        try:
            proc.stdin.close()
        except OSError:
            pass
        try:
            proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=2)


def density(blockmap: dict[str, Any]) -> dict[str, Any]:
    d = blockmap.get("density") or {}
    return {
        "thin_shell": d.get("thin_shell"),
        "likely_js_filled": d.get("likely_js_filled"),
        "json_scripts": d.get("json_scripts"),
        "li_total": (d.get("li") or {}).get("total"),
        "li_filled": (d.get("li") or {}).get("filled"),
        "td_total": (d.get("td") or {}).get("total"),
        "td_filled": (d.get("td") or {}).get("filled"),
    }


def summarize(url: str, mode: str, responses: list[dict[str, Any]], elapsed_ms: int) -> dict[str, Any]:
    nav_resp = responses[0] if responses else {}
    text_resp = responses[1] if len(responses) > 1 else {}
    result = nav_resp.get("result") or {}
    text = text_resp.get("result") if "result" in text_resp else ""
    blockmap = result.get("blockmap") or {}
    interactives = blockmap.get("interactives") or {}
    scripts = result.get("scripts") or {}
    script_errors = scripts.get("errors")
    script_errors_count = scripts.get("errors_count")
    if script_errors_count is None and isinstance(script_errors, list):
        script_errors_count = len(script_errors)
    network = result.get("network_stores") or {}
    challenge = result.get("challenge") or {}
    row = {
        "url": url,
        "mode": mode,
        "ok": "error" not in nav_resp,
        "error": (nav_resp.get("error") or {}).get("message"),
        "status": result.get("status"),
        "final_url": result.get("url"),
        "shim_mode": result.get("shim_mode"),
        "elapsed_ms": elapsed_ms,
        "bytes": result.get("bytes"),
        "title": blockmap.get("title"),
        "structure_count": len(blockmap.get("structure") or []),
        "heading_count": len(blockmap.get("headings") or []),
        "link_count": interactives.get("links"),
        "button_count": interactives.get("buttons"),
        "input_count": len(interactives.get("inputs") or []),
        "form_count": len(interactives.get("forms") or []),
        "text_chars": len(text or ""),
        "scripts_executed": scripts.get("executed"),
        "scripts_budget_ms": scripts.get("budget_ms"),
        "scripts_errors_count": script_errors_count,
        "scripts_errors": script_errors,
        "scripts_interrupted": scripts.get("interrupted"),
        "scripts_budget_exhausted": scripts.get("budget_exhausted"),
        "scripts_budget_skipped": scripts.get("budget_skipped"),
        "network_capture_count": network.get("capture_count"),
        "challenge_blocked": challenge.get("blocked") if isinstance(challenge, dict) else None,
        "challenge_provider": challenge.get("provider") if isinstance(challenge, dict) else None,
    }
    row.update(density(blockmap))
    return row


def run_one(binary: str, url: str, mode: str, exec_scripts: bool, timeout: float) -> dict[str, Any]:
    requests = [
        {"id": 1, "method": "navigate", "params": {"url": url, "exec_scripts": exec_scripts}},
        {"id": 2, "method": "text_clean", "params": {"max_chars": 200_000}},
        {"id": 3, "method": "close", "params": {}},
    ]
    start = time.time()
    try:
        responses = rpc(binary, mode, requests, timeout)
        return summarize(url, mode, responses, int((time.time() - start) * 1000))
    except Exception as exc:
        return {"url": url, "mode": mode, "ok": False, "error": str(exc)}


def load_urls(args: argparse.Namespace) -> list[str]:
    urls = list(args.url or [])
    if args.file:
        for line in Path(args.file).read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#"):
                urls.append(line)
    return urls


def main() -> int:
    parser = argparse.ArgumentParser(description="Compare stable vs enhanced unbrowser shims")
    parser.add_argument("--binary", default="./target/debug/unbrowser")
    parser.add_argument("--url", action="append", help="URL to test; repeatable")
    parser.add_argument("--file", help="newline-delimited URL corpus")
    parser.add_argument("--timeout", type=float, default=45.0)
    parser.add_argument("--no-exec-scripts", action="store_true")
    args = parser.parse_args()

    urls = load_urls(args)
    if not urls:
        parser.error("provide --url or --file")

    run_id = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    run_index = 0
    for url_index, url in enumerate(urls):
        for mode in ("stable", "enhanced"):
            row = run_one(args.binary, url, mode, not args.no_exec_scripts, args.timeout)
            row["run_id"] = run_id
            row["run_index"] = run_index
            row["url_index"] = url_index
            print(json.dumps(row, sort_keys=True))
            sys.stdout.flush()
            run_index += 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
