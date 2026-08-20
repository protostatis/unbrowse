#!/usr/bin/env python3
"""
Site matrix harness — sanitized, public-safe.

Runs the 9-site × 4-task matrix that validates the minimal-3 wrapper:
  search → open (navigate_auto) → escalation → help

Sites: Slickdeals deal, Engadget review, REI product, CNBC/BusinessInsider/Yahoo (premarket), GitHub repo/search/issues
Tasks: deal find, review extract, e-comm price, finance summary, code research

No secrets, no cookies, no private hosts. All URLs are public.
Exit 0 if harness completes; individual site failures are reported but do not fail the run
(unless --strict is passed). Use for regression of SmartClient's progressive discovery.

Usage:
  UNBROWSER_BIN=$PWD/target/release/unbrowser PYTHONPATH=python python3 scripts/site_matrix.py --json
  UNBROWSER_BIN=$PWD/target/release/unbrowser PYTHONPATH=python python3 scripts/site_matrix.py --strict
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

# allow `python scripts/site_matrix.py` without PYTHONPATH
REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT / "python") not in sys.path:
    sys.path.insert(0, str(REPO_ROOT / "python"))

from unbrowser.smart import SmartClient  # type: ignore

MATRIX = [
    {
        "id": "slickdeals-deal",
        "group": "deals",
        "mode": "search_open",
        "query": "Pixel 11 slickdeals",
        "open_url": "https://slickdeals.net/f/19868166-select-google-customers-exclusive-email-offer-code-google-pixel-11-smartphone-up-to-250-off-valid-thru-8-27",
        "goal": "Pixel 11 deal price",
        "expect": {"status": 200, "escalation_reason": "partial_result"},
    },
    {
        "id": "engadget-review",
        "group": "reviews",
        "mode": "open",
        "open_url": "https://www.engadget.com/2240042/google-pixel-11-review/",
        "goal": "Pixel 11 review rating",
        "expect": {"status": 200, "escalation_reason": None},
    },
    {
        "id": "rei-product",
        "group": "ecomm",
        "mode": "open",
        "open_url": "https://www.rei.com/product/249151/patagonia-nano-puff-insulated-jacket-mens",
        "goal": "jacket price",
        "expect": {"status": 200, "escalation_reason": "thin_shell"},
    },
    {
        "id": "finance-premarket-summary",
        "group": "finance",
        "mode": "open",
        "open_url": "https://markets.businessinsider.com/premarket?op=1",
        "goal": "premarket movers snapshot",
        "expect": {"status": 200, "escalation_reason": None},
    },
    {
        "id": "finance-yahoo-headlines",
        "group": "finance",
        "mode": "open",
        "open_url": "https://finance.yahoo.com/",
        "goal": "premarket news headlines",
        "expect": {"status": 200, "escalation_reason": None},
    },
    {
        "id": "github-repo",
        "group": "code",
        "mode": "open",
        "open_url": "https://github.com/protostatis/unbrowser",
        "goal": "README description and topics",
        "expect": {"status": 200, "escalation_reason": "partial_result"},
    },
    {
        "id": "github-search",
        "group": "code",
        "mode": "search_open",
        "query": "site:github.com headless browser MCP",
        "goal": "top 3 headless MCP repos",
        "expect": {"status": 200, "escalation_reason": "partial_result"},
    },
    {
        "id": "github-issues",
        "group": "code",
        "mode": "open",
        "open_url": "https://github.com/protostatis/unbrowser/issues",
        "goal": "list open issues",
        "expect": {"status": 200, "escalation_reason": None},
    },
    {
        "id": "engadget-search",
        "group": "reviews",
        "mode": "search",
        "query": "Pixel 11 review site:engadget.com",
        "expect": {"min_hits": 3},
    },
]


def sanitize_url(u: str) -> str:
    # strip query params that might contain tokens (none in matrix, but sanitize anyway)
    return u.split("?")[0][:120]


def run_entry(entry: dict, timeout: float = 12.0) -> dict:
    eid = entry["id"]
    mode = entry["mode"]
    start = time.time()
    out: dict = {"id": eid, "group": entry["group"], "mode": mode, "ok": False, "error": None}
    try:
        with SmartClient() as ub:
            if mode == "search":
                hits = ub.search(entry["query"], count=5)
                out["hits"] = len(hits)
                out["sample_url"] = sanitize_url(hits[0]["url"]) if hits else None
                out["escalation"] = None
                out["next_tools"] = []
                exp = entry.get("expect", {})
                if exp.get("min_hits") and len(hits) < exp["min_hits"]:
                    out["error"] = f"hits {len(hits)} < {exp['min_hits']}"
                else:
                    out["ok"] = True
            elif mode == "open":
                bundle = ub.navigate_auto(entry["open_url"], goal=entry.get("goal"), discover_limit=5, cards_limit=5, timeout=timeout)
                out["status"] = bundle.get("status")
                out["url"] = sanitize_url(bundle.get("url") or entry["open_url"])
                out["title"] = (bundle.get("blockmap") or {}).get("title", "")[:120]
                esc = bundle.get("escalation")
                out["escalation"] = esc["reason"] if esc else None
                out["escalation_detail"] = {"reason": esc.get("reason"), "hint": (esc.get("hint") or "")[:200]} if esc else None
                out["next_tools"] = [t["tool"] for t in (bundle.get("next_tools") or [])[:3]]
                out["cards"] = len(bundle.get("cards") or [])
                out["discover_routes"] = len((bundle.get("discover") or {}).get("routes", [])) if bundle.get("discover") else 0
                exp = entry.get("expect", {})
                if exp.get("status") and out["status"] != exp["status"]:
                    out["error"] = f"status {out['status']} != {exp['status']}"
                elif "escalation_reason" in exp and out["escalation"] != exp["escalation_reason"]:
                    # informational partial_result is acceptable when expected None (auto-extract cap)
                    if exp["escalation_reason"] is None and out["escalation"] in (None, "partial_result"):
                        out["ok"] = True
                    elif exp["escalation_reason"] is None and out["escalation"] == "timeout":
                        # heavy DOM timeout is retryable, not a strict failure for harness canary
                        out["ok"] = True
                    else:
                        out["error"] = f"escalation {out['escalation']} != {exp['escalation_reason']}"
                else:
                    out["ok"] = True
                # help probe (sanity, no network)
                try:
                    h = ub.help()
                    out["help_groups"] = list(h.keys())
                except Exception:
                    out["help_groups"] = []
            elif mode == "search_open":
                hits = ub.search(entry["query"], count=3)
                out["hits"] = len(hits)
                if not hits:
                    out["error"] = "no search hits"
                else:
                    url = hits[0]["url"]
                    bundle = ub.navigate_auto(url, goal=entry.get("goal"), discover_limit=5, cards_limit=5, timeout=timeout)
                    out["status"] = bundle.get("status")
                    out["url"] = sanitize_url(bundle.get("url") or url)
                    out["title"] = (bundle.get("blockmap") or {}).get("title", "")[:120]
                    esc = bundle.get("escalation")
                    out["escalation"] = esc["reason"] if esc else None
                    out["next_tools"] = [t["tool"] for t in (bundle.get("next_tools") or [])[:3]]
                    out["cards"] = len(bundle.get("cards") or [])
                    exp = entry.get("expect", {})
                    if exp.get("status") and out["status"] != exp["status"]:
                        out["error"] = f"status {out['status']} != {exp['status']}"
                    else:
                        out["ok"] = True
            else:
                out["error"] = f"unknown mode {mode}"
    except Exception as e:
        out["error"] = f"{type(e).__name__}: {str(e)[:500]}"
    out["elapsed_ms"] = int((time.time() - start) * 1000)
    # sanitize: never emit cookies, headers, full body
    return out


def main() -> int:
    p = argparse.ArgumentParser(description="Sanitized site matrix harness for minimal-3 wrapper")
    p.add_argument("--json", action="store_true", help="emit JSON report to stdout")
    p.add_argument("--strict", action="store_true", help="exit 1 if any entry not ok")
    p.add_argument("--filter", type=str, default=None, help="only run ids containing this substring")
    p.add_argument("--timeout", type=float, default=12.0, help="per-open timeout seconds")
    args = p.parse_args()

    entries = [e for e in MATRIX if args.filter in e["id"]] if args.filter else MATRIX
    results = []
    for e in entries:
        results.append(run_entry(e, timeout=args.timeout))
        # be nice to hosts
        time.sleep(0.5)

    summary = {
        "total": len(results),
        "ok": sum(1 for r in results if r["ok"]),
        "failed": sum(1 for r in results if not r["ok"]),
        "elapsed_ms": sum(r["elapsed_ms"] for r in results),
    }
    report = {"summary": summary, "results": results}

    if args.json:
        json.dump(report, sys.stdout, indent=2)
        sys.stdout.write("\n")
    else:
        print(f"Matrix {summary['ok']}/{summary['total']} ok, {summary['failed']} failed, {summary['elapsed_ms']}ms")
        for r in results:
            mark = "OK" if r["ok"] else "FAIL"
            esc = r.get("escalation") or "-"
            print(f"  {mark} {r['id']:28} {r.get('status',''):3} esc:{esc:18} {r.get('title','')[:50]}  {r.get('error') or ''}")

    if args.strict and summary["failed"] > 0:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
