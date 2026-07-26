#!/usr/bin/env python3
"""Cost-aware label construction.

Given feature rows from features/one_second.py, attach two labels per row:

  1. `y_settle_up`: 1 iff the contract eventually resolved YES (UP-token
     closed near $1).  This is the plain probability target — what the
     LightGBM model in the plan §6 must learn to output as a calibrated
     probability.

  2. `y_profit_up_cost_aware`: 1 iff buying UP token at the observed ask
     at this row's timestamp and holding to resolution would have paid
     STRICTLY MORE than fees + slippage + safety margin.  This is the plan
     §7 net-edge target — a model can be highly accurate on `y_settle_up`
     and still lose money if it always buys expensive UP when it does
     agree with the market.

Also emits the symmetric target on DOWN token so we can train a two-sided
decision.

Cost parameters (matching the plan's exact structure):
    taker_fee_frac_of_price   — Polymarket taker fee formula approximation
    slippage_cost              — flat expected slippage per share
    safety_margin              — required buffer for a bet to count as "profit"

Output row shape (adds to the input feature row):
    "y_settle_up":                1 | 0            # did YES resolve?
    "y_profit_up_cost_aware":     1 | 0            # buy-UP profitable after all costs?
    "y_profit_down_cost_aware":   1 | 0            # buy-DOWN profitable?
    "settle_up_price_final":      0.99 | 0.01      # last observed up_ask
    "cost_breakdown_up":          {fee, slippage, safety, gross_edge, net_edge}
    "cost_breakdown_down":        {fee, slippage, safety, gross_edge, net_edge}

Read-only.  Determines contract resolution from the tape ONLY at end-of-window
(no look-ahead into decision-time features).

Usage:
    python3 -m scripts.profit_engine.labels.cost_aware \
        --features var/features/features-15m.jsonl \
        --polymarket 'var/research-15m/*.jsonl' \
        --out var/features/features-15m-labeled.jsonl
"""
from __future__ import annotations
import argparse, glob, json, os, sys
from collections import defaultdict


DOLLAR = 1_000_000

DEFAULT_TAKER_FEE = 0.02          # 2% on the fill price — rough Polymarket taker
DEFAULT_SLIPPAGE = 0.005          # 0.5¢ per share expected slippage
DEFAULT_SAFETY = 0.01             # 1¢ margin required to count as tradeable edge


def taker_fee(price: float, fee_rate: float) -> float:
    """Simple approximation of Polymarket's taker fee.

    Actual formula (per plan §7 citation): highest around 50¢ probability,
    lower at the extremes.  Approximated here as fee_rate * min(p, 1-p) * 2.
    That gives a peak around 50/50 markets and drops to 0 at the extremes,
    which is the qualitative shape of the real formula.
    """
    return fee_rate * min(price, 1 - price) * 2


def resolve_contracts_from_tape(polymarket_paths: list[str]) -> dict[str, dict]:
    """Return {condition_id: {up_won: bool, close_up_ask, close_down_ask}}
    by finding the last observed prices per market.  Only resolve markets
    where the close is unambiguous (winner near 1, loser near 0)."""
    close_state: dict[str, dict] = {}
    up_tok: dict[str, str] = {}; down_tok: dict[str, str] = {}
    for path in polymarket_paths:
        with open(path) as h:
            for line in h:
                try: row = json.loads(line)
                except ValueError: continue
                if row.get("k") == "sys":
                    p = row.get("payload") or {}
                    if p.get("event_type") == "market_identity":
                        c = p["condition_id"]
                        up_tok[c] = p["up_token_id"]
                        down_tok[c] = p["down_token_id"]
                        close_state.setdefault(c, {"cond": c, "end_ms": p["end_time_ms"]})
                elif row.get("k") == "book":
                    c = row.get("cond"); tok = row.get("tok")
                    asks = row.get("asks") or []
                    if not asks or c not in close_state: continue
                    px = asks[0][0] / DOLLAR
                    if tok == up_tok.get(c):
                        close_state[c]["last_up_ask"] = px
                        close_state[c]["last_up_t"] = row.get("t", 0) // 1_000_000
                    elif tok == down_tok.get(c):
                        close_state[c]["last_down_ask"] = px
                        close_state[c]["last_down_t"] = row.get("t", 0) // 1_000_000
    resolved: dict[str, dict] = {}
    for c, s in close_state.items():
        ua = s.get("last_up_ask"); da = s.get("last_down_ask")
        if ua is None or da is None: continue
        # Require the LAST observation to be at or after end_ms - 30s so we
        # know we're near resolution rather than just missing tape.
        last_seen = max(s.get("last_up_t", 0), s.get("last_down_t", 0))
        if last_seen < s["end_ms"] - 30_000: continue
        if ua <= 0.05 and da >= 0.90: resolved[c] = {"up_won": False, "close_up": ua, "close_down": da}
        elif da <= 0.05 and ua >= 0.90: resolved[c] = {"up_won": True, "close_up": ua, "close_down": da}
        # ambiguous close → skip; row will still get y_settle_up but marked unresolved
    return resolved


def cost_breakdown(price: float, model_p: float,
                    fee_rate: float, slippage: float, safety: float) -> dict:
    fee = round(taker_fee(price, fee_rate), 6)
    gross = round(model_p - price, 6)
    net = round(gross - fee - slippage - safety, 6)
    return {"fee": fee, "slippage": slippage, "safety": safety,
            "gross_edge": gross, "net_edge": net}


def label_row(row: dict, resolved: dict, args) -> dict:
    """Attach labels to a feature row."""
    cond = row["contract_cond"]
    r = resolved.get(cond)
    labeled = dict(row)
    if r is None:
        # Contract didn't resolve cleanly → cannot label honestly.
        labeled["resolved"] = False
        return labeled
    up_won = r["up_won"]
    labeled["resolved"] = True
    labeled["y_settle_up"] = 1 if up_won else 0
    labeled["settle_up_close"] = r["close_up"]
    labeled["settle_down_close"] = r["close_down"]

    # For the cost-aware label we need a MODEL PROBABILITY.  Here we use the
    # actual outcome (1 if UP wins, 0 if DOWN wins) as the "true probability"
    # that a perfectly calibrated model would output, then check whether
    # buying at the current ask would pay after costs.  This label is the
    # answer to "was this ROW worth buying with a perfect crystal ball?" —
    # the model's job later is to approximate that decision.
    up_ask = row["poly_up_ask"]; down_ask = row["poly_down_ask"]
    truth_up = 1.0 if up_won else 0.0
    truth_down = 0.0 if up_won else 1.0
    up_costs = cost_breakdown(up_ask, truth_up,
                              args.taker_fee, args.slippage, args.safety)
    down_costs = cost_breakdown(down_ask, truth_down,
                                args.taker_fee, args.slippage, args.safety)
    labeled["cost_breakdown_up"] = up_costs
    labeled["cost_breakdown_down"] = down_costs
    labeled["y_profit_up_cost_aware"] = 1 if up_costs["net_edge"] > 0 else 0
    labeled["y_profit_down_cost_aware"] = 1 if down_costs["net_edge"] > 0 else 0
    return labeled


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--features", required=True, help="input features jsonl")
    ap.add_argument("--polymarket", nargs="+", required=True,
                    help="glob(s) of Polymarket capture files (for resolution)")
    ap.add_argument("--out", required=True, help="output labeled jsonl")
    ap.add_argument("--taker-fee", type=float, default=DEFAULT_TAKER_FEE)
    ap.add_argument("--slippage", type=float, default=DEFAULT_SLIPPAGE)
    ap.add_argument("--safety", type=float, default=DEFAULT_SAFETY)
    args = ap.parse_args()

    poly_paths: list[str] = []
    for pat in args.polymarket: poly_paths.extend(sorted(glob.glob(pat)))
    print(f"resolving contracts from {len(poly_paths)} tape files…", file=sys.stderr)
    resolved = resolve_contracts_from_tape(poly_paths)
    print(f"  cleanly resolved: {len(resolved)} contracts", file=sys.stderr)

    n_in = n_out = n_resolved = 0
    n_up_profitable = n_down_profitable = 0
    os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
    with open(args.features) as fin, open(args.out, "w") as fout:
        for line in fin:
            try: row = json.loads(line)
            except ValueError: continue
            n_in += 1
            labeled = label_row(row, resolved, args)
            if labeled.get("resolved"):
                n_resolved += 1
                if labeled.get("y_profit_up_cost_aware"): n_up_profitable += 1
                if labeled.get("y_profit_down_cost_aware"): n_down_profitable += 1
            fout.write(json.dumps(labeled, separators=(",", ":")) + "\n")
            n_out += 1

    summary = {
        "rows_in": n_in, "rows_out": n_out,
        "rows_with_resolution": n_resolved,
        "rows_where_buy_up_was_profitable": n_up_profitable,
        "rows_where_buy_down_was_profitable": n_down_profitable,
        "share_up_profitable": n_up_profitable / max(1, n_resolved),
        "share_down_profitable": n_down_profitable / max(1, n_resolved),
    }
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
