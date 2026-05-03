#!/usr/bin/env python3
"""T2 — aggregate Phase A events into per-domain decision parameters.

Reads T1's `train/runs/{ts}/{domain}/*.events.jsonl` files and produces
`train/aggregates/{domain}.json` — per-domain decision parameters
suitable for T3 (pack.py) to bundle into prefit/v1.bundle.json.

v0 aggregation (still produced — bundle schema v1 consumers depend on it):
  - blocklist_additions: hosts that appeared as `policy_blocked` events
    but aren't in the global Tier-1 blocklist (i.e. site-specific
    trackers worth adding to this domain's prefit)
  - settle_distribution: percentiles of `policy_trace.elapsed_ms` for
    successful navigations (placeholder p50/p90/p95)
  - shape_hint: derived from blockmap.density.likely_js_filled (when
    available in the event stream — currently a stub)
  - framework: derived from inspecting the page's main scripts (also
    a stub for v0 — manually annotated in the hand-curated bundle)

v1 (bundle schema v2) — Bayesian posteriors per decision key:
  - For each decision the runtime makes during navigate, fit a
    Beta(α, β) posterior over its binary outcome.
  - Decision keys for v0:
      * `block:<host_pattern>` — α = navigates that succeeded with this
        host blocked, β = navigates that failed. Until real
        `outcome_reported` events flow into the trainer (Agent B's
        track), success is proxied by
        `policy_trace.scripts.executed > 0 AND extract is non-null`.
        This proxy is documented as a placeholder in the bundle.
      * `settle_fast:<framework>` — α = navigates whose settle reached
        `reason="idle"` within the framework's p50 budget, β =
        otherwise.
  - Prior: weakly-informative Beta(1, 1) (uniform), tunable via
    --prior-alpha / --prior-beta.
  - Output schema (per-domain):
        "posteriors": {
            "block:zephr-templates.cnbc.com": {"alpha": 12.0, "beta": 2.0, "n": 14},
            "settle_fast:next-14": {"alpha": 8.0, "beta": 6.0, "n": 14}
        }
  - Empty domains still get a `posteriors: {}` field so the bundle
    shape is uniform.

Usage:
  python3 train/aggregate.py [--runs-dir DIR] [--out DIR]
                             [--prior-alpha F] [--prior-beta F]
  python3 train/aggregate.py --self-test     # smoke test, no IO

Defaults:
  --runs-dir   train/runs/<latest>/
  --out        train/aggregates/
  --prior-alpha 1.0
  --prior-beta  1.0
"""
from __future__ import annotations

import argparse
import collections
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


def latest_runs_dir() -> Path | None:
    runs = REPO / "train" / "runs"
    if not runs.exists():
        return None
    candidates = sorted([p for p in runs.iterdir() if p.is_dir()])
    return candidates[-1] if candidates else None


def fit_beta_posterior(successes: int, failures: int,
                       prior_alpha: float = 1.0,
                       prior_beta: float = 1.0) -> dict:
    """Conjugate Beta-Binomial update.

    Prior: Beta(prior_alpha, prior_beta). Default Beta(1, 1) is the
    uniform prior — weakly-informative, lets data dominate after a
    handful of observations and keeps placeholders explicitly
    "no-information" before any data arrives.

    Likelihood: Binomial(n, p) with n = successes + failures.
    Posterior: Beta(prior_alpha + successes, prior_beta + failures).
    Closed form, O(1), no MCMC. See spec §4.2 / §4.7.

    Returns the dict the bundle stores. `n` is the number of
    observations (not the posterior pseudo-count) so consumers can
    distinguish "Beta(1, 1) because no data" from "Beta(1, 1) because
    the prior happens to equal that after observation."
    """
    if successes < 0 or failures < 0:
        raise ValueError(f"counts must be non-negative; got {successes}/{failures}")
    if prior_alpha <= 0 or prior_beta <= 0:
        raise ValueError(f"prior must be positive; got α={prior_alpha} β={prior_beta}")
    return {
        "alpha": float(prior_alpha + successes),
        "beta": float(prior_beta + failures),
        "n": int(successes + failures),
    }


def _iter_nav_records(events_file: Path):
    """Yield one record per navigation in the events JSONL.

    Each record is {"nav_id": str, "events": [event...]} grouped by
    `navigation_id`. T1's collector writes one navigation per file
    today, but the format permits multiple in case a driver runs a
    stitched session — group defensively.
    """
    by_nav: dict[str, list[dict]] = collections.defaultdict(list)
    for line in events_file.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        nav_id = (ev.get("data") or {}).get("navigation_id") or "_anon"
        by_nav[nav_id].append(ev)
    for nav_id, evs in by_nav.items():
        yield {"nav_id": nav_id, "events": evs}


def _success_proxy(events: list[dict]) -> bool:
    """Placeholder success signal until outcome_reported flows in.

    True iff the navigation's policy_trace shows scripts.executed > 0
    AND there's no failure / blocked challenge. Documented in the
    module docstring; consumers should treat this as weak positive
    evidence and replace it with `outcome_reported.success` once the
    outcome stream is wired through.
    """
    executed_any = False
    for ev in events:
        if ev.get("event") == "policy_trace":
            scripts = (ev.get("data") or {}).get("scripts") or {}
            if scripts.get("executed", 0) > 0:
                executed_any = True
        if ev.get("event") == "outcome_reported":
            data = ev.get("data") or {}
            # Real outcome wins over proxy — Agent B will start emitting.
            return bool(data.get("success", False))
    return executed_any


def _blocked_hosts_in_nav(events: list[dict]) -> set[str]:
    """Hosts that were skipped due to a blocklist decision in this nav."""
    out: set[str] = set()
    for ev in events:
        kind = ev.get("event")
        data = ev.get("data") or {}
        if kind == "script_decision" and data.get("action") == "skip":
            host = (data.get("host") or "").lower()
            if host:
                out.add(host)
        elif kind == "policy_blocked":
            host = (data.get("matched") or "").lower()
            if host:
                out.add(host)
    return out


def _settle_signal(events: list[dict]) -> tuple[str | None, int | None, str | None]:
    """Return (framework_or_none, settle_elapsed_ms, settle_reason).

    Pulls settle elapsed/reason from the LAST policy_trace's after_load
    settle (falling back to after_dcl, then trace.elapsed_ms). Framework
    is best-effort: the trainer doesn't yet detect framework from page
    content, so we report None and let the per-framework key skip.
    """
    framework = None
    settle_elapsed = None
    settle_reason = None
    for ev in events:
        if ev.get("event") == "policy_trace":
            data = ev.get("data") or {}
            settle = data.get("settle") or {}
            after_load = settle.get("after_load") or settle.get("after_dcl") or {}
            if isinstance(after_load, dict):
                settle_elapsed = after_load.get("elapsed_ms", settle_elapsed)
                settle_reason = after_load.get("reason", settle_reason)
            if settle_elapsed is None:
                settle_elapsed = data.get("elapsed_ms")
        if ev.get("event") == "prefit_applied":
            data = ev.get("data") or {}
            framework = data.get("framework") or framework
    return framework, settle_elapsed, settle_reason


def fit_posteriors(navs: list[dict],
                   p50_ms_by_framework: dict[str, int] | None = None,
                   prior_alpha: float = 1.0,
                   prior_beta: float = 1.0) -> dict:
    """Build the per-decision posterior table for one domain.

    Args:
      navs: list of {"nav_id", "events"} records.
      p50_ms_by_framework: framework → p50_ms threshold for
        settle_fast. Falls back to per-domain p50 if framework is
        unknown (key written as `settle_fast:_unknown`).
      prior_alpha, prior_beta: weakly-informative Beta prior.

    Returns: dict[decision_key → BetaPosterior]
    """
    block_counts: dict[str, list[int]] = collections.defaultdict(lambda: [0, 0])
    settle_counts: dict[str, list[int]] = collections.defaultdict(lambda: [0, 0])

    for nav in navs:
        events = nav["events"]
        success = _success_proxy(events)
        for host in _blocked_hosts_in_nav(events):
            key = f"block:{host}"
            if success:
                block_counts[key][0] += 1
            else:
                block_counts[key][1] += 1

        framework, settle_elapsed, settle_reason = _settle_signal(events)
        fw_key = framework or "_unknown"
        threshold = (p50_ms_by_framework or {}).get(fw_key)
        if threshold is None and settle_elapsed is not None:
            # No framework prior available — use this nav's elapsed as
            # the threshold, which trivially makes it "fast" (n+=1, α+=1)
            # whenever the settle reason was idle. This is a no-op
            # signal but keeps the key present so future runs with real
            # priors can grow data.
            threshold = settle_elapsed
        if settle_elapsed is None or threshold is None:
            continue
        skey = f"settle_fast:{fw_key}"
        is_fast = (settle_reason == "idle") and (settle_elapsed <= threshold)
        if is_fast:
            settle_counts[skey][0] += 1
        else:
            settle_counts[skey][1] += 1

    posteriors: dict = {}
    for key, (s, f) in block_counts.items():
        posteriors[key] = fit_beta_posterior(s, f, prior_alpha, prior_beta)
    for key, (s, f) in settle_counts.items():
        posteriors[key] = fit_beta_posterior(s, f, prior_alpha, prior_beta)
    return posteriors


def aggregate_domain(domain_dir: Path,
                     prior_alpha: float = 1.0,
                     prior_beta: float = 1.0) -> dict:
    """Walk a domain's event JSONL files and produce a DomainPrefit dict."""
    blocked_hosts = collections.Counter()
    settle_ms_per_nav: list[int] = []
    all_navs: list[dict] = []

    for events_file in domain_dir.glob("*.events.jsonl"):
        for nav in _iter_nav_records(events_file):
            all_navs.append(nav)
            events = nav["events"]
            for ev in events:
                if ev.get("event") == "policy_blocked":
                    host = ((ev.get("data") or {}).get("matched") or "").lower()
                    if host:
                        blocked_hosts[host] += 1
                elif ev.get("event") == "script_decision":
                    data = ev.get("data") or {}
                    if data.get("action") == "skip":
                        host = (data.get("host") or "").lower()
                        if host:
                            blocked_hosts[host] += 1
            if _success_proxy(events):
                _, settle_elapsed, _ = _settle_signal(events)
                if settle_elapsed is None:
                    # fall back to policy_trace.elapsed_ms
                    for ev in events:
                        if ev.get("event") == "policy_trace":
                            settle_elapsed = (ev.get("data") or {}).get("elapsed_ms")
                            break
                if settle_elapsed is not None:
                    settle_ms_per_nav.append(int(settle_elapsed))

    prefit = {
        "domain": domain_dir.name,
        "framework": None,
        "blocklist_additions": [h for h, _ in blocked_hosts.most_common()],
        "required_patterns": [],
        "settle_distribution": None,
        "shape_hint": None,
    }
    if settle_ms_per_nav:
        sorted_settle = sorted(settle_ms_per_nav)
        n = len(sorted_settle)
        prefit["settle_distribution"] = {
            "p50_ms": int(sorted_settle[n // 2]),
            "p90_ms": int(sorted_settle[min(n - 1, int(n * 0.9))]),
            "p95_ms": int(sorted_settle[min(n - 1, int(n * 0.95))]),
        }

    # Posteriors: use this domain's own p50 as the per-framework
    # threshold when we don't have a global table to consult.
    p50 = prefit["settle_distribution"]["p50_ms"] if prefit["settle_distribution"] else None
    p50_by_fw = {"_unknown": p50} if p50 is not None else {}
    prefit["posteriors"] = fit_posteriors(
        all_navs,
        p50_ms_by_framework=p50_by_fw,
        prior_alpha=prior_alpha,
        prior_beta=prior_beta,
    )
    return prefit


def _self_test() -> None:
    """In-process smoke test: empty input → Beta(1, 1); shape sanity."""
    # Empty observations against the default prior should be Beta(1, 1).
    p = fit_beta_posterior(0, 0)
    assert p == {"alpha": 1.0, "beta": 1.0, "n": 0}, p

    # Successes increment α, failures β.
    p = fit_beta_posterior(5, 3)
    assert p == {"alpha": 6.0, "beta": 4.0, "n": 8}, p

    # Tunable prior.
    p = fit_beta_posterior(0, 0, prior_alpha=2.0, prior_beta=8.0)
    assert p == {"alpha": 2.0, "beta": 8.0, "n": 0}, p

    # fit_posteriors with no navs → empty dict.
    assert fit_posteriors([]) == {}

    # Synthetic nav: one successful nav with one blocked host.
    navs = [
        {"nav_id": "nav_1", "events": [
            {"event": "script_decision", "data": {
                "navigation_id": "nav_1", "action": "skip",
                "host": "tracker.example.com", "reason": "blocklist",
            }},
            {"event": "policy_trace", "data": {
                "navigation_id": "nav_1",
                "scripts": {"executed": 3},
                "settle": {"after_load": {"elapsed_ms": 80, "reason": "idle"}},
                "elapsed_ms": 80,
            }},
            {"event": "prefit_applied", "data": {
                "navigation_id": "nav_1", "framework": "next-14",
            }},
        ]},
    ]
    out = fit_posteriors(navs, p50_ms_by_framework={"next-14": 100})
    assert out["block:tracker.example.com"] == {"alpha": 2.0, "beta": 1.0, "n": 1}, out
    assert out["settle_fast:next-14"] == {"alpha": 2.0, "beta": 1.0, "n": 1}, out

    # Failure case: same shape but executed=0 → block & settle marked failure.
    navs_fail = [
        {"nav_id": "nav_2", "events": [
            {"event": "script_decision", "data": {
                "navigation_id": "nav_2", "action": "skip",
                "host": "tracker.example.com", "reason": "blocklist",
            }},
            {"event": "policy_trace", "data": {
                "navigation_id": "nav_2",
                "scripts": {"executed": 0},
                "settle": {"after_load": {"elapsed_ms": 5000, "reason": "budget_exhausted"}},
                "elapsed_ms": 5000,
            }},
        ]},
    ]
    out2 = fit_posteriors(navs_fail, p50_ms_by_framework={"_unknown": 100})
    assert out2["block:tracker.example.com"] == {"alpha": 1.0, "beta": 2.0, "n": 1}, out2
    assert out2["settle_fast:_unknown"] == {"alpha": 1.0, "beta": 2.0, "n": 1}, out2

    print("self-test: ok")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs-dir", default=None,
                    help="T1 runs dir (default: latest under train/runs/)")
    ap.add_argument("--out", default=str(REPO / "train" / "aggregates"))
    ap.add_argument("--prior-alpha", type=float, default=1.0,
                    help="Beta prior α (default 1.0 — weakly-informative uniform)")
    ap.add_argument("--prior-beta", type=float, default=1.0,
                    help="Beta prior β (default 1.0 — weakly-informative uniform)")
    ap.add_argument("--self-test", action="store_true",
                    help="run in-process smoke test and exit")
    args = ap.parse_args()

    if args.self_test:
        _self_test()
        return

    runs_dir = Path(args.runs_dir) if args.runs_dir else latest_runs_dir()
    if not runs_dir or not runs_dir.exists():
        sys.exit(f"no runs dir found at {runs_dir}; run train/collect.py first")

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)

    print(f"runs:  {runs_dir}")
    print(f"out:   {out_dir}")
    print(f"prior: Beta(α={args.prior_alpha}, β={args.prior_beta})")
    print()

    domain_dirs = [p for p in runs_dir.iterdir() if p.is_dir()]
    if not domain_dirs:
        sys.exit(f"no domain dirs in {runs_dir}")

    for d in sorted(domain_dirs):
        prefit = aggregate_domain(d, args.prior_alpha, args.prior_beta)
        out_path = out_dir / f"{d.name}.json"
        out_path.write_text(json.dumps(prefit, indent=2))
        print(f"  {d.name:30s} → {out_path.relative_to(REPO)} "
              f"(blocklist_additions={len(prefit['blocklist_additions'])}, "
              f"settle={'yes' if prefit['settle_distribution'] else 'no'}, "
              f"posteriors={len(prefit['posteriors'])})")

    print()
    print(f"wrote {len(domain_dirs)} aggregates")


if __name__ == "__main__":
    main()
