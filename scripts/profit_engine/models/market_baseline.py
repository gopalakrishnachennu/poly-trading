#!/usr/bin/env python3
"""Market-implied probability baseline (plan §6 stage 1).

The plan is explicit: every model must beat the market-implied probability.
For a "will UP win?" question the market's own answer is:

    P(UP) = mid / (mid + mid_down)  or simpler: up_bid + (up_ask - up_bid)/2
    (using the depth-adjusted microprice can be marginally better)

This is a MODEL — its predict method returns probabilities.  It has no fit
step (nothing to learn) but implements the same interface as logistic and
LightGBM so the walk-forward runner can compare all three uniformly.
"""
from __future__ import annotations


class MarketBaseline:
    name = "market_baseline_microprice"

    def fit(self, rows: list[dict]) -> "MarketBaseline":
        # Nothing to learn.  Method kept for interface parity.
        return self

    def predict_proba(self, rows: list[dict]) -> list[float]:
        out: list[float] = []
        for r in rows:
            ua = r.get("poly_up_ask"); ub = r.get("poly_up_bid")
            uas = r.get("poly_up_ask_sz", 0); ubs = r.get("poly_up_bid_sz", 0)
            if ua is None or ub is None:
                out.append(0.5); continue
            tot = uas + ubs
            if tot > 0:
                # microprice: leans toward the side with less resting size
                p = (ua * ubs + ub * uas) / tot
            else:
                p = (ua + ub) / 2
            out.append(max(0.001, min(0.999, p)))
        return out
