#!/usr/bin/env python3
"""Walk-forward evaluation — time-split, no leakage, no random shuffle.

Splits rows chronologically by CONTRACT (not by row) so adjacent-second
observations within the same market never straddle the train/test boundary.
This is the plan §10 requirement.

Per fold:
  1. Sort contracts by start_ms.
  2. train: earliest N contracts.  val: next M.  test: next K.
  3. Fit model on train's rows.
  4. Fit calibrator on val's rows using model's raw probs.
  5. Report metrics on test's rows (calibrated + raw).
  6. Slide window forward by K contracts and repeat.

Compares three models per fold: market_baseline vs logistic (raw) vs
logistic (calibrated).  LightGBM slot is imported opportunistically — if
lightgbm is installed, it's added to the comparison; otherwise skipped.
"""
from __future__ import annotations
import argparse, json, os, sys
from collections import defaultdict

# Package imports; support running as -m from repo root
try:
    from scripts.profit_engine.models.market_baseline import MarketBaseline
    from scripts.profit_engine.models.logistic import LogisticModel
    from scripts.profit_engine.models.calibration import PlattCalibrator, IsotonicCalibrator
    from scripts.profit_engine.evaluation.metrics import (
        brier, log_loss, reliability_curve, expected_calibration_error,
        precision_by_bucket,
    )
except ImportError:
    # allow direct execution
    here = os.path.dirname(os.path.abspath(__file__))
    sys.path.insert(0, os.path.abspath(os.path.join(here, "..", "..", "..")))
    from scripts.profit_engine.models.market_baseline import MarketBaseline
    from scripts.profit_engine.models.logistic import LogisticModel
    from scripts.profit_engine.models.calibration import PlattCalibrator, IsotonicCalibrator
    from scripts.profit_engine.evaluation.metrics import (
        brier, log_loss, reliability_curve, expected_calibration_error,
        precision_by_bucket,
    )


DEFAULT_FEATURES = [
    "poly_up_ask", "poly_up_bid", "poly_up_bid_sz", "poly_up_ask_sz",
    "poly_down_ask", "poly_down_bid",
    "poly_spread", "poly_up_microprice", "poly_up_obi_1",
    "seconds_remaining", "session_progress",
    "binance_return_1s", "binance_return_5s", "binance_return_60s",
    "binance_realized_vol_60s",
    "distance_to_threshold", "z_distance",
]


def load_labeled(path: str) -> list[dict]:
    rows: list[dict] = []
    with open(path) as h:
        for line in h:
            try: r = json.loads(line)
            except ValueError: continue
            if not r.get("resolved"): continue
            rows.append(r)
    return rows


def group_by_contract(rows: list[dict]) -> list[list[dict]]:
    by_cond: dict[str, list[dict]] = defaultdict(list)
    for r in rows: by_cond[r["contract_cond"]].append(r)
    contracts = sorted(by_cond.values(), key=lambda group: group[0]["start_ms"])
    return contracts


def evaluate(probs: list[float], y: list[int]) -> dict:
    return {
        "n": len(probs),
        "positive_rate": (sum(y) / len(y)) if y else 0.0,
        "brier": brier(probs, y),
        "log_loss": log_loss(probs, y),
        "ece": expected_calibration_error(probs, y, n_bins=10),
        "reliability": reliability_curve(probs, y, n_bins=10),
        "precision_by_bucket": precision_by_bucket(probs, y, n_bins=5),
    }


def run_walk_forward(rows: list[dict], features: list[str], y_key: str,
                     n_train: int, n_val: int, n_test: int) -> list[dict]:
    contracts = group_by_contract(rows)
    n_total = len(contracts)
    if n_total < n_train + n_val + n_test:
        return [{"error": f"only {n_total} contracts; need at least "
                          f"{n_train + n_val + n_test}"}]

    folds: list[dict] = []
    start = 0
    fold_idx = 0
    while start + n_train + n_val + n_test <= n_total:
        train_c = contracts[start: start + n_train]
        val_c = contracts[start + n_train: start + n_train + n_val]
        test_c = contracts[start + n_train + n_val:
                           start + n_train + n_val + n_test]
        train_rows = [r for g in train_c for r in g]
        val_rows = [r for g in val_c for r in g]
        test_rows = [r for g in test_c for r in g]
        y_test = [int(r[y_key]) for r in test_rows]

        fold: dict = {
            "fold": fold_idx,
            "train_contracts": len(train_c), "train_rows": len(train_rows),
            "val_contracts": len(val_c), "val_rows": len(val_rows),
            "test_contracts": len(test_c), "test_rows": len(test_rows),
            "test_positive_rate": sum(y_test) / max(1, len(y_test)),
            "models": {},
        }

        # 1) Market baseline (no fit)
        mb = MarketBaseline().fit(train_rows)
        fold["models"]["market_baseline"] = evaluate(mb.predict_proba(test_rows), y_test)

        # 2) Logistic (raw)
        log = LogisticModel(features).fit(train_rows, y_key=y_key)
        raw_val_probs = log.predict_proba(val_rows)
        raw_test_probs = log.predict_proba(test_rows)
        fold["models"]["logistic_raw"] = evaluate(raw_test_probs, y_test)

        # 3) Logistic (Platt-calibrated on val)
        platt = PlattCalibrator().fit(raw_val_probs, [int(r[y_key]) for r in val_rows])
        fold["models"]["logistic_platt"] = evaluate(platt.transform(raw_test_probs), y_test)

        # 4) Logistic (isotonic-calibrated on val) — useful when val is big enough
        iso = IsotonicCalibrator().fit(raw_val_probs, [int(r[y_key]) for r in val_rows])
        fold["models"]["logistic_isotonic"] = evaluate(iso.transform(raw_test_probs), y_test)

        # 5) LightGBM — optional. Only runs when the package is installed AND
        # its native library (libomp on macOS) loads AND there are enough
        # samples for it to matter (>500 train rows). Any of those missing
        # and we skip the fold-level LightGBM entry silently — logistic
        # remains the challenger baseline.
        try:
            import lightgbm as lgb  # noqa: F401
            if len(train_rows) >= 500:
                fold["models"]["lightgbm"] = _fit_lightgbm(train_rows, val_rows, test_rows,
                                                            features, y_key)
        except (ImportError, OSError):
            pass

        folds.append(fold)
        start += n_test  # slide by one test-window
        fold_idx += 1

    return folds


def _fit_lightgbm(train_rows, val_rows, test_rows, features, y_key):
    """Deferred: requires lightgbm.  Returns evaluate() dict."""
    import lightgbm as lgb  # type: ignore
    import numpy as np

    def to_matrix(rows):
        X = np.array([[r.get(f, 0.0) if r.get(f) is not None else 0.0
                       for f in features] for r in rows], dtype=float)
        y = np.array([int(r[y_key]) for r in rows], dtype=int)
        return X, y

    Xtr, ytr = to_matrix(train_rows)
    Xva, yva = to_matrix(val_rows)
    Xte, yte = to_matrix(test_rows)
    dtr = lgb.Dataset(Xtr, ytr); dva = lgb.Dataset(Xva, yva, reference=dtr)
    params = {"objective": "binary", "metric": "binary_logloss",
              "learning_rate": 0.03, "num_leaves": 31, "min_data_in_leaf": 20,
              "feature_fraction": 0.9, "verbose": -1}
    model = lgb.train(params, dtr, num_boost_round=500, valid_sets=[dva],
                      callbacks=[lgb.early_stopping(20, verbose=False)])
    probs = model.predict(Xte).tolist()
    return evaluate(probs, list(map(int, yte)))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--labeled", required=True, help="labeled feature jsonl")
    ap.add_argument("--y-key", default="y_settle_up",
                    choices=["y_settle_up", "y_profit_up_cost_aware"])
    ap.add_argument("--train", type=int, default=20)
    ap.add_argument("--val", type=int, default=5)
    ap.add_argument("--test", type=int, default=5)
    ap.add_argument("--report", required=True)
    args = ap.parse_args()

    rows = load_labeled(args.labeled)
    print(f"loaded {len(rows)} resolved rows across "
          f"{len({r['contract_cond'] for r in rows})} contracts", file=sys.stderr)
    folds = run_walk_forward(rows, DEFAULT_FEATURES, args.y_key,
                             args.train, args.val, args.test)
    os.makedirs(os.path.dirname(args.report) or ".", exist_ok=True)
    with open(args.report, "w") as fp:
        json.dump({"y_key": args.y_key, "features": DEFAULT_FEATURES,
                   "n_train": args.train, "n_val": args.val, "n_test": args.test,
                   "folds": folds}, fp, indent=2)
    print(f"wrote {args.report}", file=sys.stderr)

    # Quick headline summary
    if folds and "error" not in folds[0]:
        print("\n=== fold headlines (test-set Brier — lower is better) ===")
        for f in folds:
            summ = " | ".join(f"{m}={s['brier']:.4f}"
                              for m, s in f["models"].items())
            print(f"  fold {f['fold']:>2}  test_n={f['test_rows']:>5}  {summ}")
    else:
        print(json.dumps(folds, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
