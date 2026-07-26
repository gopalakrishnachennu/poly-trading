#!/usr/bin/env python3
"""Train and frozen-test a BTC-hourly intraday return challenger."""
from __future__ import annotations

import argparse
import bisect
import hashlib
import json
import math
import os
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

import numpy as np
from lightgbm import LGBMRegressor


BPS = 10_000
MICROS = 1_000_000


class InvalidModelEvidence(ValueError):
    pass


def canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while block := handle.read(1 << 20):
            digest.update(block)
    return digest.hexdigest()


def integer(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise InvalidModelEvidence(f"{field} must be integer")
    return value


def read_policy(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    expected = {
        "schema_version", "asset", "horizons_ms", "history_lags_ms",
        "prediction_quantiles_bps", "minimum_validation_trades",
        "minimum_test_trades", "minimum_test_net_pnl_micros",
        "minimum_test_profit_factor_milli", "maximum_test_drawdown_micros",
        "training_row_stride", "lightgbm_estimators",
        "lightgbm_learning_rate_micros", "lightgbm_num_leaves",
        "lightgbm_min_child_samples", "random_seed",
    }
    if set(value) != expected or value["schema_version"] != 1 or value["asset"] != "BTC":
        raise InvalidModelEvidence("model policy schema invalid")
    for field in expected - {"schema_version", "asset", "horizons_ms", "history_lags_ms", "prediction_quantiles_bps"}:
        if integer(value[field], field) <= 0:
            raise InvalidModelEvidence(f"{field} must be positive")
    for field in ("horizons_ms", "history_lags_ms", "prediction_quantiles_bps"):
        items = value[field]
        if not isinstance(items, list) or not items or items != sorted(set(items)) or any(integer(item, field) <= 0 for item in items):
            raise InvalidModelEvidence(f"{field} invalid")
    if any(item >= BPS for item in value["prediction_quantiles_bps"]):
        raise InvalidModelEvidence("prediction quantile must be below 10000 bps")
    if value["lightgbm_learning_rate_micros"] > MICROS:
        raise InvalidModelEvidence("learning rate exceeds one")
    return value


def verify_dataset(dataset: Path) -> dict[str, Any]:
    manifest = json.loads((dataset / "manifest.json").read_text())
    if manifest.get("eligibility") != "DATASET_ONLY_NOT_A_MODEL" or manifest.get("asset") != "BTC":
        raise InvalidModelEvidence("dataset is not eligible frozen BTC evidence")
    for fold in ("train", "validation", "test"):
        path = dataset / f"{fold}.jsonl"
        expected = manifest.get("folds", {}).get(fold, {}).get("sha256")
        if not isinstance(expected, str) or sha256(path) != expected:
            raise InvalidModelEvidence(f"{fold} checksum mismatch")
    train_end = manifest["folds"]["train"]["end_ms"]
    validation_start = manifest["folds"]["validation"]["start_ms"]
    validation_end = manifest["folds"]["validation"]["end_ms"]
    test_start = manifest["folds"]["test"]["start_ms"]
    if train_end > validation_start or validation_end > test_start:
        raise InvalidModelEvidence("dataset folds overlap")
    return manifest


def load_rows(path: Path, horizons: list[int]) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open() as handle:
        for line_number, line in enumerate(handle, 1):
            row = json.loads(line)
            if row.get("asset") != "BTC" or row.get("receive_time_quality") not in {"OBSERVED", "UNAVAILABLE_EXTERNAL_ARCHIVE"}:
                raise InvalidModelEvidence(f"line {line_number}: row provenance invalid")
            future = row.get("future")
            if not isinstance(future, dict) or any(str(horizon) not in future for horizon in horizons):
                raise InvalidModelEvidence(f"line {line_number}: future labels incomplete")
            rows.append(row)
    return rows


def causal_features(rows: list[dict[str, Any]], lags: list[int]) -> tuple[np.ndarray, list[dict[str, Any]], list[str]]:
    by_series: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        by_series[(row["condition_id"], row["side"])].append(row)
    series_data: dict[tuple[str, str], tuple[list[int], list[int], list[int]]] = {}
    for key, series in by_series.items():
        series.sort(key=lambda row: integer(row["strategy_available_time_ns"], "strategy_available_time_ns"))
        times = [row["strategy_available_time_ns"] // MICROS for row in series]
        asks = [int(row["entry_cost_micros"]) * MICROS // int(row["target_shares_micros"]) for row in series]
        bids = [int(row["current_exit_proceeds_micros"]) * MICROS // int(row["target_shares_micros"]) for row in series]
        series_data[key] = (times, asks, bids)
    feature_names = [
        "ask_ppm", "bid_ppm", "spread_ppm", "depth_imbalance_ppm",
        "log_bid_depth", "log_ask_depth", "session_progress_bps",
        "time_to_expiry_ms", "side_up", "pair_ask_sum_ppm",
    ] + [f"mid_delta_{lag}ms_ppm" for lag in lags]
    vectors: list[list[float]] = []
    ordered_rows: list[dict[str, Any]] = []
    for key in sorted(by_series):
        series = by_series[key]
        times, asks, bids = series_data[key]
        opposite_key = (key[0], "DOWN" if key[1] == "UP" else "UP")
        opposite = series_data.get(opposite_key)
        for index, row in enumerate(series):
            time_ms = times[index]
            ask, bid = asks[index], bids[index]
            bid_depth, ask_depth = int(row["bid_depth_micros"]), int(row["ask_depth_micros"])
            denominator = max(1, bid_depth + ask_depth)
            pair_sum = 2 * MICROS
            if opposite is not None:
                other_times, other_asks, _ = opposite
                other_index = bisect.bisect_right(other_times, time_ms) - 1
                if other_index >= 0:
                    pair_sum = ask + other_asks[other_index]
            current_mid = (ask + bid) // 2
            deltas: list[float] = []
            for lag in lags:
                prior_index = bisect.bisect_right(times, time_ms - lag, hi=index + 1) - 1
                if prior_index < 0:
                    deltas.append(float("nan"))
                else:
                    prior_mid = (asks[prior_index] + bids[prior_index]) // 2
                    deltas.append(float(current_mid - prior_mid))
            vectors.append([
                float(ask), float(bid), float(ask - bid),
                float((bid_depth - ask_depth) * MICROS // denominator),
                math.log1p(bid_depth), math.log1p(ask_depth),
                float(row["session_progress_bps"]), float(row["time_to_expiry_ms"]),
                1.0 if row["side"] == "UP" else 0.0, float(pair_sum),
                *deltas,
            ])
            ordered_rows.append(row)
    return np.asarray(vectors, dtype=np.float64), ordered_rows, feature_names


def exact_pnl(row: dict[str, Any], horizon: int) -> int:
    cost = int(row["entry_cost_micros"])
    fee_bps = integer(row["fee_reserve_bps"], "fee_reserve_bps")
    fee = (cost * fee_bps + BPS - 1) // BPS
    proceeds = int(row["future"][str(horizon)]["exit_proceeds_micros"])
    return proceeds - cost - fee


def simulate(rows: list[dict[str, Any]], predictions: np.ndarray, horizon: int, threshold: float) -> dict[str, Any]:
    if len(rows) != len(predictions):
        raise InvalidModelEvidence("prediction length mismatch")
    candidates = sorted(
        zip(rows, predictions.tolist()),
        key=lambda item: (item[0]["condition_id"], item[0]["strategy_available_time_ns"], -item[1], item[0]["side"]),
    )
    pnl: list[int] = []
    open_until: dict[str, int] = {}
    index = 0
    while index < len(candidates):
        row, _ = candidates[index]
        condition, now = row["condition_id"], row["strategy_available_time_ns"]
        same: list[tuple[dict[str, Any], float]] = []
        while index < len(candidates):
            candidate = candidates[index]
            if candidate[0]["condition_id"] != condition or candidate[0]["strategy_available_time_ns"] != now:
                break
            same.append(candidate)
            index += 1
        if now < open_until.get(condition, -1):
            continue
        chosen_row, score = max(same, key=lambda item: (item[1], item[0]["side"]))
        if score < threshold:
            continue
        pnl.append(exact_pnl(chosen_row, horizon))
        open_until[condition] = chosen_row["future"][str(horizon)]["exit_available_ns"]
    equity = peak = drawdown = 0
    gains = losses = 0
    for value in pnl:
        equity += value
        peak = max(peak, equity)
        drawdown = max(drawdown, peak - equity)
        if value > 0:
            gains += value
        elif value < 0:
            losses -= value
    profit_factor_milli = gains * 1000 // losses if losses else (1_000_000 if gains else 0)
    return {
        "trades": len(pnl),
        "wins": sum(value > 0 for value in pnl),
        "net_pnl_micros": str(sum(pnl)),
        "gross_profit_micros": str(gains),
        "gross_loss_micros": str(losses),
        "profit_factor_milli": profit_factor_milli,
        "max_drawdown_micros": str(drawdown),
    }


def train_model(policy: dict[str, Any], features: np.ndarray, rows: list[dict[str, Any]], horizon: int) -> LGBMRegressor:
    stride = policy["training_row_stride"]
    labels = np.asarray([row["future"][str(horizon)]["net_return_bps"] for row in rows], dtype=np.float64)
    model = LGBMRegressor(
        objective="regression_l1",
        n_estimators=policy["lightgbm_estimators"],
        learning_rate=policy["lightgbm_learning_rate_micros"] / MICROS,
        num_leaves=policy["lightgbm_num_leaves"],
        min_child_samples=policy["lightgbm_min_child_samples"],
        subsample=1.0,
        colsample_bytree=1.0,
        random_state=policy["random_seed"],
        deterministic=True,
        force_col_wise=True,
        n_jobs=1,
        verbosity=-1,
    )
    model.fit(features[::stride], labels[::stride])
    return model


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", required=True, type=Path)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--out-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.out_dir.exists() or args.out_dir.is_symlink():
            raise InvalidModelEvidence("output directory must be new")
        policy = read_policy(args.policy)
        manifest = verify_dataset(args.dataset)
        train_rows = load_rows(args.dataset / "train.jsonl", policy["horizons_ms"])
        validation_rows = load_rows(args.dataset / "validation.jsonl", policy["horizons_ms"])
        train_x, train_ordered, feature_names = causal_features(train_rows, policy["history_lags_ms"])
        validation_x, validation_ordered, validation_names = causal_features(validation_rows, policy["history_lags_ms"])
        if feature_names != validation_names:
            raise InvalidModelEvidence("feature schema drift")
        candidates: list[dict[str, Any]] = []
        models: dict[int, LGBMRegressor] = {}
        for horizon in policy["horizons_ms"]:
            model = train_model(policy, train_x, train_ordered, horizon)
            models[horizon] = model
            predictions = model.predict(validation_x)
            thresholds = sorted({
                float(np.quantile(predictions, quantile / BPS))
                for quantile in policy["prediction_quantiles_bps"]
            })
            for threshold in thresholds:
                metrics = simulate(validation_ordered, predictions, horizon, threshold)
                candidates.append({"horizon_ms": horizon, "threshold_net_return_bps": threshold, "metrics": metrics})
        eligible = [
            candidate for candidate in candidates
            if candidate["metrics"]["trades"] >= policy["minimum_validation_trades"]
            and int(candidate["metrics"]["net_pnl_micros"]) > 0
        ]
        result: dict[str, Any] = {
            "schema_version": 1,
            "non_authoritative": True,
            "asset": "BTC",
            "capability": "RESEARCH_MODEL_ONLY",
            "dataset_manifest_sha256": sha256(args.dataset / "manifest.json"),
            "policy_sha256": sha256(args.policy),
            "feature_names": feature_names,
            "train_sessions": manifest["folds"]["train"]["sessions"],
            "validation_sessions": manifest["folds"]["validation"]["sessions"],
            "test_sessions": manifest["folds"]["test"]["sessions"],
            "validation_candidates": candidates,
        }
        args.out_dir.mkdir(parents=True, exist_ok=False)
        if not eligible:
            result["verdict"] = "REJECTED_NO_POSITIVE_VALIDATION_STRATEGY"
            result["test_evaluated"] = False
        else:
            selected = max(
                eligible,
                key=lambda candidate: (
                    int(candidate["metrics"]["net_pnl_micros"]),
                    candidate["metrics"]["profit_factor_milli"],
                    -candidate["horizon_ms"],
                ),
            )
            horizon = selected["horizon_ms"]
            model = models[horizon]
            model_path = args.out_dir / "model.txt"
            model.booster_.save_model(str(model_path))
            # Selection is now frozen. Only now is the untouched test fold read.
            test_rows = load_rows(args.dataset / "test.jsonl", policy["horizons_ms"])
            test_x, test_ordered, test_names = causal_features(test_rows, policy["history_lags_ms"])
            if test_names != feature_names:
                raise InvalidModelEvidence("test feature schema drift")
            test_metrics = simulate(
                test_ordered, model.predict(test_x), horizon,
                selected["threshold_net_return_bps"],
            )
            passes = (
                test_metrics["trades"] >= policy["minimum_test_trades"]
                and int(test_metrics["net_pnl_micros"]) >= policy["minimum_test_net_pnl_micros"]
                and test_metrics["profit_factor_milli"] >= policy["minimum_test_profit_factor_milli"]
                and int(test_metrics["max_drawdown_micros"]) <= policy["maximum_test_drawdown_micros"]
            )
            result.update({
                "selected": selected,
                "test_evaluated": True,
                "test_metrics": test_metrics,
                "model_sha256": sha256(model_path),
                "verdict": "PAPER_CANDIDATE_ONLY" if passes else "REJECTED_UNSEEN_TEST",
            })
        report_path = args.out_dir / "report.json"
        temporary = args.out_dir / "report.json.tmp"
        temporary.write_bytes(canonical(result) + b"\n")
        os.replace(temporary, report_path)
        print(json.dumps({
            "verdict": result["verdict"],
            "selected": result.get("selected"),
            "test_metrics": result.get("test_metrics"),
            "report": str(report_path),
        }, indent=2))
        return 0
    except (InvalidModelEvidence, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"model evaluation rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
