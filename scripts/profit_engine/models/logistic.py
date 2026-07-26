#!/usr/bin/env python3
"""L2-regularised logistic regression, pure Python, no external deps.

Trained by mini-batch gradient descent.  Standardises features (fit on
training rows only — no test contamination).  Handles missing values by
imputing the training median.  Interface matches MarketBaseline and the
future LightGBM model:

    model = LogisticModel(feature_names=[...])
    model.fit(train_rows)                # rows = list[dict]
    probs = model.predict_proba(test_rows)  # list[float] in (0,1)

The plan (§6 stage 1) makes logistic regression a mandatory baseline:
LightGBM must beat both the market and this after costs, or nothing gets
promoted.
"""
from __future__ import annotations
import math, random, statistics


def _sigmoid(z: float) -> float:
    if z >= 0:
        e = math.exp(-z)
        return 1.0 / (1.0 + e)
    e = math.exp(z)
    return e / (1.0 + e)


class LogisticModel:
    def __init__(self, feature_names: list[str],
                 l2: float = 1e-3, lr: float = 0.05,
                 epochs: int = 40, batch_size: int = 256,
                 seed: int = 17):
        self.features = list(feature_names)
        self.l2 = l2; self.lr = lr; self.epochs = epochs
        self.batch_size = batch_size; self.seed = seed
        self.means: list[float] = []; self.stds: list[float] = []
        self.medians: list[float] = []
        self.weights: list[float] = []
        self.bias: float = 0.0
        self.name = f"logistic(l2={l2},lr={lr},epochs={epochs})"

    def _extract(self, rows: list[dict]) -> list[list[float | None]]:
        return [[r.get(f) for f in self.features] for r in rows]

    def _fit_scaler(self, X: list[list[float | None]]) -> None:
        n = len(self.features)
        cols = [[x[i] for x in X if x[i] is not None and math.isfinite(x[i])]
                for i in range(n)]
        self.medians = [statistics.median(c) if c else 0.0 for c in cols]
        self.means = [statistics.fmean(c) if c else 0.0 for c in cols]
        self.stds = [statistics.pstdev(c) if len(c) > 1 else 1.0 for c in cols]
        # Guard against zero-variance columns
        self.stds = [s if s > 1e-9 else 1.0 for s in self.stds]

    def _standardise(self, X: list[list[float | None]]) -> list[list[float]]:
        out: list[list[float]] = []
        for row in X:
            out.append([
                ((row[i] if (row[i] is not None and math.isfinite(row[i])) else self.medians[i])
                 - self.means[i]) / self.stds[i]
                for i in range(len(self.features))
            ])
        return out

    def fit(self, rows: list[dict], y_key: str = "y_settle_up") -> "LogisticModel":
        X_raw = self._extract(rows)
        y = [int(r.get(y_key, 0)) for r in rows]
        # Drop rows where the label is missing (r.get returned 0 default but
        # the row was actually unresolved).  We filter in the runner instead.
        self._fit_scaler(X_raw)
        X = self._standardise(X_raw)
        n = len(X); d = len(self.features)
        self.weights = [0.0] * d
        self.bias = 0.0
        rng = random.Random(self.seed)
        idx = list(range(n))
        for _ in range(self.epochs):
            rng.shuffle(idx)
            for start in range(0, n, self.batch_size):
                batch = idx[start:start + self.batch_size]
                gw = [0.0] * d; gb = 0.0
                for i in batch:
                    z = self.bias + sum(self.weights[j] * X[i][j] for j in range(d))
                    p = _sigmoid(z)
                    err = p - y[i]
                    for j in range(d):
                        gw[j] += err * X[i][j]
                    gb += err
                bs = max(1, len(batch))
                for j in range(d):
                    self.weights[j] -= self.lr * (gw[j] / bs + self.l2 * self.weights[j])
                self.bias -= self.lr * (gb / bs)
        return self

    def predict_proba(self, rows: list[dict]) -> list[float]:
        X = self._standardise(self._extract(rows))
        out: list[float] = []
        for row in X:
            z = self.bias + sum(self.weights[j] * row[j] for j in range(len(self.features)))
            out.append(max(0.001, min(0.999, _sigmoid(z))))
        return out
