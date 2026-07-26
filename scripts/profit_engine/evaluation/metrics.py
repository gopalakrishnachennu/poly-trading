#!/usr/bin/env python3
"""Prediction metrics: Brier, log loss, calibration error, reliability curve.

Every function is pure Python.  All are needed to answer the plan §10
question — "is this model actually calibrated on out-of-sample data?"
"""
from __future__ import annotations
import math


def brier(probs: list[float], y: list[int]) -> float:
    """Mean squared error between predicted probability and outcome.
    Lower is better.  0.25 is the constant-0.5 baseline."""
    if not probs: return 0.0
    return sum((p - t) ** 2 for p, t in zip(probs, y)) / len(probs)


def log_loss(probs: list[float], y: list[int], eps: float = 1e-6) -> float:
    """Negative log-likelihood.  Lower is better."""
    if not probs: return 0.0
    total = 0.0
    for p, t in zip(probs, y):
        p = max(eps, min(1 - eps, p))
        total += -(t * math.log(p) + (1 - t) * math.log(1 - p))
    return total / len(probs)


def reliability_curve(probs: list[float], y: list[int], n_bins: int = 10) -> list[dict]:
    """Bin predictions and report bucket mean predicted vs mean actual."""
    if not probs: return []
    buckets: list[list[tuple[float, int]]] = [[] for _ in range(n_bins)]
    for p, t in zip(probs, y):
        b = min(n_bins - 1, max(0, int(p * n_bins)))
        buckets[b].append((p, t))
    out: list[dict] = []
    for i, bucket in enumerate(buckets):
        if not bucket:
            out.append({"bin": i, "n": 0}); continue
        preds = [p for p, _ in bucket]; ys = [t for _, t in bucket]
        out.append({
            "bin": i, "n": len(bucket),
            "range": [i / n_bins, (i + 1) / n_bins],
            "mean_pred": sum(preds) / len(preds),
            "mean_actual": sum(ys) / len(ys),
        })
    return out


def expected_calibration_error(probs: list[float], y: list[int], n_bins: int = 10) -> float:
    """Weighted average |mean_pred - mean_actual| across bins."""
    curve = reliability_curve(probs, y, n_bins)
    total = sum(b["n"] for b in curve)
    if total == 0: return 0.0
    return sum(b["n"] / total * abs(b["mean_pred"] - b["mean_actual"])
               for b in curve if b["n"] > 0)


def precision_by_bucket(probs: list[float], y: list[int], n_bins: int = 5) -> list[dict]:
    """Sort by predicted probability, split into equal-count buckets, report
    win rate per bucket.  Answers 'is the model informative at the extremes?'"""
    if not probs: return []
    pairs = sorted(zip(probs, y))
    per_bin = max(1, len(pairs) // n_bins)
    out: list[dict] = []
    for b in range(n_bins):
        chunk = pairs[b * per_bin: (b + 1) * per_bin if b < n_bins - 1 else None]
        if not chunk: continue
        preds = [p for p, _ in chunk]; ys = [t for _, t in chunk]
        out.append({
            "bucket": b, "n": len(chunk),
            "pred_lo": min(preds), "pred_hi": max(preds),
            "mean_pred": sum(preds) / len(preds),
            "mean_actual": sum(ys) / len(ys),
        })
    return out
