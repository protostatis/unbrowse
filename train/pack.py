#!/usr/bin/env python3
"""T3 — pack per-domain aggregates into the runtime prefit bundle.

Reads `train/aggregates/{domain}.json` files (produced by T2) and
optional framework_priors, writes `prefit/v1.bundle.json` — the file
the runtime loads via `include_str!` in src/prefit.rs.

For v0: the runtime expects JSON, not MessagePack (spec §6.5 calls for
MessagePack, deferred). Schema in src/prefit.rs's PrefitBundle.

Schema versions:
  v1 — domains: blocklist_additions, settle_distribution, framework,
       shape_hint, required_patterns. No posteriors.
  v2 — adds `posteriors: {decision_key → BetaPosterior}` per domain.
       BetaPosterior = {alpha: f64, beta: f64, n: u64}. Decision keys
       are stable strings the runtime queries: `block:<host>` for
       blocklist additions, `settle_fast:<framework>` for settle
       outcomes. Missing posteriors map to "no information" (caller
       should default to Beta(1, 1)).

The runtime loader (src/prefit.rs R1) accepts both v1 and v2 bundles.
We bump to v2 here so newly-trained bundles ship the posteriors, but
older v1 bundles in the wild still parse.

Validation: re-reads the written bundle to confirm round-trip parses,
and warns if any aggregate has no settle_distribution or blocklist_additions
(which would mean T2 produced a degenerate row).

Usage:
  python3 train/pack.py [--in DIR] [--out FILE] [--corpus-size N]
                        [--training-pipeline-version STR]
                        [--schema-version 2]

Defaults:
  --in              train/aggregates/
  --out             prefit/v1.bundle.json
  --schema-version  2
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


# Hand-curated framework_priors used as fallback when the domain isn't
# in the bundle but its framework can be detected at runtime. Keeping
# these in pack.py rather than aggregating them from T1 events for v0 —
# real per-framework aggregation is a follow-up.
FRAMEWORK_PRIORS = {
    "react-18": {
        "framework": "react-18",
        "blocklist_additions": [],
        "settle_distribution": {"p50_ms": 1500, "p90_ms": 3000, "p95_ms": 5000},
    },
    "next-14": {
        "framework": "next-14",
        "blocklist_additions": [],
        "settle_distribution": {"p50_ms": 1200, "p90_ms": 2500, "p95_ms": 4000},
    },
    "static_ssr": {
        "framework": "static_ssr",
        "blocklist_additions": [],
        "settle_distribution": {"p50_ms": 100, "p90_ms": 400, "p95_ms": 800},
    },
}


def _placeholder_posteriors_for(domain_entry: dict) -> dict:
    """For a domain with no observed posteriors, synthesize Beta(1, 1)
    placeholders for each blocklist addition. This signals "no
    information yet, defer to defaults" to the runtime — Thompson
    sampling against Beta(1, 1) is a coin flip, which the threshold
    gate (default 0.5) will round to ~50% take rate. Drivers that want
    deterministic behavior on placeholder rows should set the threshold
    higher; the runtime emits the alpha/beta/n in `posterior_consulted`
    so this is observable.
    """
    out: dict[str, dict] = {}
    for host in domain_entry.get("blocklist_additions", []) or []:
        out[f"block:{host}"] = {"alpha": 1.0, "beta": 1.0, "n": 0}
    fw = domain_entry.get("framework")
    if fw:
        out[f"settle_fast:{fw}"] = {"alpha": 1.0, "beta": 1.0, "n": 0}
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="in_dir", default=str(REPO / "train" / "aggregates"))
    ap.add_argument("--out", default=str(REPO / "prefit" / "v1.bundle.json"))
    ap.add_argument("--corpus-size", type=int, default=10)
    ap.add_argument("--training-pipeline-version", default="v0-from-aggregates")
    ap.add_argument("--schema-version", type=int, default=2,
                    help="bundle schema version (1=no posteriors, 2=with posteriors)")
    args = ap.parse_args()

    if args.schema_version not in (1, 2):
        sys.exit(f"unsupported --schema-version {args.schema_version}; want 1 or 2")

    in_dir = Path(args.in_dir)
    out_path = Path(args.out)

    if not in_dir.exists():
        sys.exit(f"input dir does not exist: {in_dir}\n"
                 f"run train/aggregate.py first")

    domains: dict[str, dict] = {}
    for f in sorted(in_dir.glob("*.json")):
        try:
            d = json.loads(f.read_text())
        except json.JSONDecodeError as e:
            print(f"WARN: skipping malformed {f.name}: {e}", file=sys.stderr)
            continue
        if "domain" not in d:
            print(f"WARN: skipping {f.name}: missing 'domain' field", file=sys.stderr)
            continue

        # v2 normalization: ensure posteriors field is present. T2 will
        # populate it; older T2 outputs without the field get
        # placeholder Beta(1, 1) per blocklist host so the bundle shape
        # is uniform regardless of when the aggregate was produced.
        if args.schema_version >= 2 and "posteriors" not in d:
            d["posteriors"] = _placeholder_posteriors_for(d)

        # v1 mode: drop posteriors so the schema-v1 contract holds even
        # if the input aggregate was produced by the new T2.
        if args.schema_version == 1:
            d.pop("posteriors", None)

        domains[d["domain"]] = d
        if not d.get("settle_distribution") and not d.get("blocklist_additions"):
            print(f"WARN: {d['domain']} has empty settle + empty blocklist additions — degenerate row",
                  file=sys.stderr)

    bundle = {
        "schema_version": args.schema_version,
        "fit_timestamp": int(time.time()),
        "fit_corpus_size": args.corpus_size,
        "training_pipeline_version": args.training_pipeline_version,
        "domains": domains,
        "framework_priors": FRAMEWORK_PRIORS,
    }

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(bundle, indent=2))
    # Show the path relative to REPO when possible (cleaner output for
    # the default in-repo destination); fall back to the full path
    # when --out points outside the repo (e.g. /tmp staging).
    try:
        shown = out_path.relative_to(REPO)
    except ValueError:
        shown = out_path
    print(f"wrote {shown} "
          f"({len(domains)} domains, {len(FRAMEWORK_PRIORS)} framework priors, "
          f"schema_version={args.schema_version})")

    # Validate: round-trip parse so we catch schema drift early.
    try:
        roundtrip = json.loads(out_path.read_text())
        assert roundtrip["schema_version"] == args.schema_version
        assert isinstance(roundtrip["domains"], dict)
        for d, p in roundtrip["domains"].items():
            assert "domain" in p, f"{d}: missing 'domain' field"
            assert isinstance(p.get("blocklist_additions", []), list)
            if args.schema_version >= 2:
                posts = p.get("posteriors", {})
                assert isinstance(posts, dict), f"{d}: posteriors must be dict"
                for k, v in posts.items():
                    assert isinstance(v, dict) and "alpha" in v and "beta" in v and "n" in v, (
                        f"{d}: posterior {k!r} malformed"
                    )
                    assert v["alpha"] > 0 and v["beta"] > 0, (
                        f"{d}: posterior {k!r} has non-positive params"
                    )
        print("validate: ok")
    except (AssertionError, KeyError) as e:
        sys.exit(f"validate FAILED: {e}")


if __name__ == "__main__":
    main()
