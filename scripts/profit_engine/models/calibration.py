#!/usr/bin/env python3
"""Probability calibration — Platt scaling and isotonic regression, pure Python.

Wraps any model that has predict_proba().  Fits on VALIDATION data (never
training) — the plan is explicit that calibration on training data is
overfitting.
"""
from __future__ import annotations
import math


def _sigmoid(z: float) -> float:
    if z >= 0:
        e = math.exp(-z); return 1.0 / (1.0 + e)
    e = math.exp(z); return e / (1.0 + e)


class PlattCalibrator:
    """1D logistic on the model's raw probabilities: q = sigmoid(a*p + b).

    Small-sample calibrator per plan §6 stage 3.  Uses batch gradient descent
    with L2 (mirrors LogisticModel to keep behaviour predictable).
    """
    def __init__(self, lr: float = 0.1, epochs: int = 400, l2: float = 1e-4):
        self.a = 1.0; self.b = 0.0
        self.lr = lr; self.epochs = epochs; self.l2 = l2

    def fit(self, probs: list[float], y: list[int]) -> "PlattCalibrator":
        n = len(probs)
        if n == 0: return self
        for _ in range(self.epochs):
            ga = 0.0; gb = 0.0
            for p, target in zip(probs, y):
                pred = _sigmoid(self.a * p + self.b)
                err = pred - target
                ga += err * p; gb += err
            ga = ga / n + self.l2 * self.a
            gb = gb / n
            self.a -= self.lr * ga
            self.b -= self.lr * gb
        return self

    def transform(self, probs: list[float]) -> list[float]:
        return [max(0.001, min(0.999, _sigmoid(self.a * p + self.b))) for p in probs]


class IsotonicCalibrator:
    """Isotonic (non-parametric monotone) calibration.  Plan §6 stage 3 for
    larger validation sets.  Implements pool-adjacent-violators.
    """
    def __init__(self):
        self.thresholds: list[float] = []  # sorted p breakpoints
        self.values: list[float] = []       # corresponding calibrated probs

    def fit(self, probs: list[float], y: list[int]) -> "IsotonicCalibrator":
        pairs = sorted(zip(probs, y))
        if not pairs:
            self.thresholds = []; self.values = []; return self
        xs = [p for p, _ in pairs]
        ys = [float(t) for _, t in pairs]
        weights = [1.0] * len(ys)
        # Pool Adjacent Violators
        i = 0
        while i < len(ys) - 1:
            if ys[i] > ys[i + 1]:
                new_w = weights[i] + weights[i + 1]
                new_y = (ys[i] * weights[i] + ys[i + 1] * weights[i + 1]) / new_w
                ys[i] = new_y; weights[i] = new_w
                del ys[i + 1]; del weights[i + 1]; del xs[i + 1]
                if i > 0: i -= 1
            else:
                i += 1
        self.thresholds = xs; self.values = [max(0.001, min(0.999, v)) for v in ys]
        return self

    def transform(self, probs: list[float]) -> list[float]:
        if not self.thresholds: return list(probs)
        out: list[float] = []
        for p in probs:
            # Piecewise-constant interpolation between thresholds
            if p <= self.thresholds[0]: out.append(self.values[0]); continue
            if p >= self.thresholds[-1]: out.append(self.values[-1]); continue
            lo, hi = 0, len(self.thresholds) - 1
            while lo < hi - 1:
                mid = (lo + hi) // 2
                if self.thresholds[mid] <= p: lo = mid
                else: hi = mid
            # Linear interpolation between neighbours
            x0, x1 = self.thresholds[lo], self.thresholds[hi]
            y0, y1 = self.values[lo], self.values[hi]
            span = x1 - x0
            out.append(y0 + (y1 - y0) * ((p - x0) / span) if span > 0 else y0)
        return out
