#!/usr/bin/env python3
"""Walk-forward train and one-shot test a causal BTC-hourly challenger."""
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
import pyarrow.parquet as pq
from lightgbm import LGBMRegressor

try:
    from scripts.train_btc_hourly_intraday import (
        BPS,
        MICROS,
        InvalidModelEvidence,
        canonical,
        exact_pnl,
        load_rows,
        sha256,
        simulate,
        verify_dataset,
    )
except ModuleNotFoundError:
    from train_btc_hourly_intraday import (
        BPS,
        MICROS,
        InvalidModelEvidence,
        canonical,
        exact_pnl,
        load_rows,
        sha256,
        simulate,
        verify_dataset,
    )


def positive_integer(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise InvalidModelEvidence(f"{field} must be a positive integer")
    return value


def sorted_positive_list(value: Any, field: str, *, allow_zero: bool = False) -> list[int]:
    if not isinstance(value, list) or not value or value != sorted(set(value)):
        raise InvalidModelEvidence(f"{field} must be a sorted unique list")
    minimum = 0 if allow_zero else 1
    if any(isinstance(item, bool) or not isinstance(item, int) or item < minimum for item in value):
        raise InvalidModelEvidence(f"{field} contains an invalid value")
    return value


def read_policy(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    expected = {
        "schema_version", "asset", "horizons_ms", "book_history_lags_ms",
        "spot_history_lags_ms", "flow_windows_ms", "model_objectives",
        "entry_threshold_bps", "source_only_latency_reserve_ms",
        "maximum_spot_age_ms", "minimum_development_sessions",
        "final_test_sessions", "walk_forward_initial_sessions",
        "walk_forward_fold_sessions", "minimum_profitable_cv_folds",
        "minimum_cv_trades", "minimum_final_test_trades",
        "minimum_final_test_net_pnl_micros",
        "minimum_final_test_profit_factor_milli",
        "maximum_final_test_drawdown_micros", "training_row_stride",
        "lightgbm_estimators", "lightgbm_learning_rate_micros",
        "lightgbm_num_leaves", "lightgbm_min_child_samples", "random_seed",
    }
    if set(value) != expected or value["schema_version"] != 2 or value["asset"] != "BTC":
        raise InvalidModelEvidence("v2 model policy schema invalid")
    for field in expected - {
        "schema_version", "asset", "horizons_ms", "book_history_lags_ms",
        "spot_history_lags_ms", "flow_windows_ms", "model_objectives",
        "entry_threshold_bps",
    }:
        positive_integer(value[field], field)
    for field in (
        "horizons_ms", "book_history_lags_ms", "spot_history_lags_ms",
        "flow_windows_ms",
    ):
        sorted_positive_list(value[field], field)
    sorted_positive_list(value["entry_threshold_bps"], "entry_threshold_bps", allow_zero=True)
    if value["model_objectives"] != sorted(set(value["model_objectives"])):
        raise InvalidModelEvidence("model_objectives must be sorted and unique")
    if not value["model_objectives"] or any(
        objective not in {"regression_l1", "regression_l2"}
        for objective in value["model_objectives"]
    ):
        raise InvalidModelEvidence("unsupported model objective")
    if value["lightgbm_learning_rate_micros"] > MICROS:
        raise InvalidModelEvidence("learning rate exceeds one")
    return value


def verify_auxiliary(root: Path) -> dict[str, Any]:
    manifest_path = root / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    if (
        manifest.get("schema_version") != 1
        or manifest.get("non_authoritative") is not True
        or manifest.get("spot_symbol") != "btcusdt"
        or manifest.get("spot_source") != "binance"
    ):
        raise InvalidModelEvidence("auxiliary manifest is not eligible")
    for name in ("trades", "spot"):
        path = root / f"{name}.parquet"
        if sha256(path) != manifest.get(f"{name}_sha256"):
            raise InvalidModelEvidence(f"auxiliary {name} checksum mismatch")
    return manifest


def load_auxiliary(root: Path) -> tuple[
    list[int],
    list[int],
    dict[str, tuple[list[int], list[int], list[int]]],
]:
    spot_rows = pq.read_table(root / "spot.parquet").to_pylist()
    spot_times = [int(row["timestamp_ms"]) for row in spot_rows]
    spot_prices = [int(row["price_micros"]) for row in spot_rows]
    if spot_times != sorted(set(spot_times)):
        raise InvalidModelEvidence("spot evidence is not strictly chronological")
    flow_rows: dict[str, list[tuple[int, int, int]]] = defaultdict(list)
    for row in pq.read_table(root / "trades.parquet").to_pylist():
        timestamp = int(row["timestamp_ms"])
        volume = int(row["size_usdc_micros"])
        positive = (
            (row["outcome"] == "UP" and row["aggressor_side"] == "BUY")
            or (row["outcome"] == "DOWN" and row["aggressor_side"] == "SELL")
        )
        flow_rows[row["condition_id"]].append((timestamp, volume if positive else -volume, volume))
    flow: dict[str, tuple[list[int], list[int], list[int]]] = {}
    for condition, values in flow_rows.items():
        values.sort()
        times: list[int] = []
        signed_prefix = [0]
        total_prefix = [0]
        for timestamp, signed, total in values:
            times.append(timestamp)
            signed_prefix.append(signed_prefix[-1] + signed)
            total_prefix.append(total_prefix[-1] + total)
        flow[condition] = (times, signed_prefix, total_prefix)
    return spot_times, spot_prices, flow


def session_map(rows: list[dict[str, Any]]) -> dict[str, int]:
    sessions: dict[str, int] = {}
    for row in rows:
        condition = row["condition_id"]
        start = int(row["session_start_ms"])
        previous = sessions.get(condition)
        if previous is not None and previous != start:
            raise InvalidModelEvidence("condition has conflicting session boundary")
        sessions[condition] = start
    return sessions


def partition_sessions(
    rows: list[dict[str, Any]], policy: dict[str, Any],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]], list[str], list[str]]:
    sessions = session_map(rows)
    ordered = [condition for condition, _ in sorted(sessions.items(), key=lambda item: (item[1], item[0]))]
    final_count = policy["final_test_sessions"]
    if len(ordered) < policy["minimum_development_sessions"] + final_count:
        raise InvalidModelEvidence("insufficient independent sessions")
    development_ids = ordered[:-final_count]
    final_ids = ordered[-final_count:]
    development_set, final_set = set(development_ids), set(final_ids)
    development = [row for row in rows if row["condition_id"] in development_set]
    final = [row for row in rows if row["condition_id"] in final_set]
    if development_set & final_set:
        raise InvalidModelEvidence("development and final sessions overlap")
    return development, final, development_ids, final_ids


def prior_value(times: list[int], values: list[int], cutoff: int) -> tuple[int, int] | None:
    index = bisect.bisect_right(times, cutoff) - 1
    return None if index < 0 else (times[index], values[index])


def build_features(
    rows: list[dict[str, Any]],
    policy: dict[str, Any],
    spot_times: list[int],
    spot_prices: list[int],
    flow: dict[str, tuple[list[int], list[int], list[int]]],
) -> tuple[np.ndarray, list[dict[str, Any]], list[str]]:
    book_lags = policy["book_history_lags_ms"]
    spot_lags = policy["spot_history_lags_ms"]
    flow_windows = policy["flow_windows_ms"]
    reserve = policy["source_only_latency_reserve_ms"]
    by_series: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        by_series[(row["condition_id"], row["side"])].append(row)
    series_data: dict[tuple[str, str], tuple[list[int], list[int], list[int]]] = {}
    for key, series in by_series.items():
        series.sort(key=lambda row: int(row["strategy_available_time_ns"]))
        times = [int(row["strategy_available_time_ns"]) // MICROS for row in series]
        asks = [int(row["entry_cost_micros"]) * MICROS // int(row["target_shares_micros"]) for row in series]
        bids = [int(row["current_exit_proceeds_micros"]) * MICROS // int(row["target_shares_micros"]) for row in series]
        series_data[key] = (times, asks, bids)
    feature_names = [
        "ask_ppm", "bid_ppm", "spread_ppm", "depth_imbalance_ppm",
        "log_bid_depth", "log_ask_depth", "session_progress_bps",
        "time_to_expiry_ms", "side_up", "pair_ask_sum_ppm",
        "spot_distance_from_open_ppm", "spot_age_ms",
    ]
    feature_names += [f"book_mid_delta_{lag}ms_ppm" for lag in book_lags]
    feature_names += [f"spot_return_{lag}ms_ppm" for lag in spot_lags]
    feature_names += [f"flow_imbalance_{window}ms_ppm" for window in flow_windows]
    feature_names += [f"flow_log_volume_{window}ms" for window in flow_windows]
    vectors: list[list[float]] = []
    ordered_rows: list[dict[str, Any]] = []
    for key in sorted(by_series):
        times, asks, bids = series_data[key]
        opposite = series_data.get((key[0], "DOWN" if key[1] == "UP" else "UP"))
        condition_flow = flow.get(key[0])
        for index, row in enumerate(by_series[key]):
            now = times[index]
            source_cutoff = now - reserve
            current_spot = prior_value(spot_times, spot_prices, source_cutoff)
            opening_spot = prior_value(spot_times, spot_prices, int(row["session_start_ms"]))
            if current_spot is None or opening_spot is None:
                continue
            spot_age = source_cutoff - current_spot[0]
            if spot_age < 0 or spot_age > policy["maximum_spot_age_ms"]:
                continue
            ask, bid = asks[index], bids[index]
            bid_depth, ask_depth = int(row["bid_depth_micros"]), int(row["ask_depth_micros"])
            depth_total = max(1, bid_depth + ask_depth)
            pair_sum = 2 * MICROS
            if opposite is not None:
                opposite_index = bisect.bisect_right(opposite[0], now) - 1
                if opposite_index >= 0:
                    pair_sum = ask + opposite[1][opposite_index]
            current_mid = (ask + bid) // 2
            book_deltas: list[float] = []
            for lag in book_lags:
                prior_index = bisect.bisect_right(times, now - lag, hi=index + 1) - 1
                prior_mid = (
                    (asks[prior_index] + bids[prior_index]) // 2
                    if prior_index >= 0 else current_mid
                )
                book_deltas.append(float(current_mid - prior_mid))
            spot_returns: list[float] = []
            for lag in spot_lags:
                prior_spot = prior_value(spot_times, spot_prices, source_cutoff - lag)
                if prior_spot is None or prior_spot[1] <= 0:
                    spot_returns.append(0.0)
                else:
                    spot_returns.append(float(
                        (current_spot[1] - prior_spot[1]) * MICROS // prior_spot[1]
                    ))
            flow_imbalances: list[float] = []
            flow_volumes: list[float] = []
            for window in flow_windows:
                signed = total = 0
                if condition_flow is not None:
                    trade_times, signed_prefix, total_prefix = condition_flow
                    right = bisect.bisect_right(trade_times, source_cutoff)
                    left = bisect.bisect_right(trade_times, source_cutoff - window)
                    signed = signed_prefix[right] - signed_prefix[left]
                    total = total_prefix[right] - total_prefix[left]
                flow_imbalances.append(float(signed * MICROS // max(1, total)))
                flow_volumes.append(math.log1p(total))
            vectors.append([
                float(ask), float(bid), float(ask - bid),
                float((bid_depth - ask_depth) * MICROS // depth_total),
                math.log1p(bid_depth), math.log1p(ask_depth),
                float(row["session_progress_bps"]), float(row["time_to_expiry_ms"]),
                1.0 if row["side"] == "UP" else 0.0, float(pair_sum),
                float((current_spot[1] - opening_spot[1]) * MICROS // opening_spot[1]),
                float(spot_age), *book_deltas, *spot_returns,
                *flow_imbalances, *flow_volumes,
            ])
            ordered_rows.append(row)
    return np.asarray(vectors, dtype=np.float64), ordered_rows, feature_names


def fit_model(
    policy: dict[str, Any],
    objective: str,
    features: np.ndarray,
    rows: list[dict[str, Any]],
    horizon: int,
) -> LGBMRegressor:
    labels = np.asarray(
        [row["future"][str(horizon)]["net_return_bps"] for row in rows],
        dtype=np.float64,
    )
    stride = policy["training_row_stride"]
    model = LGBMRegressor(
        objective=objective,
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


def aggregate_metrics(fold_metrics: list[dict[str, Any]]) -> dict[str, Any]:
    trades = sum(metric["trades"] for metric in fold_metrics)
    wins = sum(metric["wins"] for metric in fold_metrics)
    net = sum(int(metric["net_pnl_micros"]) for metric in fold_metrics)
    gains = sum(int(metric["gross_profit_micros"]) for metric in fold_metrics)
    losses = sum(int(metric["gross_loss_micros"]) for metric in fold_metrics)
    return {
        "folds": len(fold_metrics),
        "profitable_folds": sum(int(metric["net_pnl_micros"]) > 0 for metric in fold_metrics),
        "trades": trades,
        "wins": wins,
        "net_pnl_micros": str(net),
        "gross_profit_micros": str(gains),
        "gross_loss_micros": str(losses),
        "profit_factor_milli": gains * 1000 // losses if losses else (1_000_000 if gains else 0),
        "maximum_fold_drawdown_micros": str(max(
            (int(metric["max_drawdown_micros"]) for metric in fold_metrics),
            default=0,
        )),
    }


def walk_forward_folds(
    development_ids: list[str], policy: dict[str, Any],
) -> list[tuple[list[str], list[str]]]:
    initial = policy["walk_forward_initial_sessions"]
    width = policy["walk_forward_fold_sessions"]
    folds: list[tuple[list[str], list[str]]] = []
    cursor = initial
    while cursor + width <= len(development_ids):
        folds.append((development_ids[:cursor], development_ids[cursor:cursor + width]))
        cursor += width
    if len(folds) < policy["minimum_profitable_cv_folds"]:
        raise InvalidModelEvidence("insufficient walk-forward folds")
    return folds


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", required=True, type=Path)
    parser.add_argument("--auxiliary", required=True, type=Path)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--out-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.out_dir.exists() or args.out_dir.is_symlink():
            raise InvalidModelEvidence("output directory must be new")
        policy = read_policy(args.policy)
        dataset_manifest = verify_dataset(args.dataset)
        auxiliary_manifest = verify_auxiliary(args.auxiliary)
        all_rows: list[dict[str, Any]] = []
        for fold in ("train", "validation", "test"):
            all_rows.extend(load_rows(args.dataset / f"{fold}.jsonl", policy["horizons_ms"]))
        development, final_rows, development_ids, final_ids = partition_sessions(all_rows, policy)
        spot_times, spot_prices, flow = load_auxiliary(args.auxiliary)
        development_x, development_ordered, feature_names = build_features(
            development, policy, spot_times, spot_prices, flow,
        )
        by_condition_indices: dict[str, list[int]] = defaultdict(list)
        for index, row in enumerate(development_ordered):
            by_condition_indices[row["condition_id"]].append(index)
        folds = walk_forward_folds(development_ids, policy)
        candidates: list[dict[str, Any]] = []
        for objective in policy["model_objectives"]:
            for horizon in policy["horizons_ms"]:
                fold_predictions: list[tuple[list[dict[str, Any]], np.ndarray]] = []
                for train_ids, validation_ids in folds:
                    train_indices = [
                        index for condition in train_ids
                        for index in by_condition_indices.get(condition, [])
                    ]
                    validation_indices = [
                        index for condition in validation_ids
                        for index in by_condition_indices.get(condition, [])
                    ]
                    if not train_indices or not validation_indices:
                        raise InvalidModelEvidence("walk-forward fold has no causal feature rows")
                    model = fit_model(
                        policy,
                        objective,
                        development_x[train_indices],
                        [development_ordered[index] for index in train_indices],
                        horizon,
                    )
                    fold_predictions.append((
                        [development_ordered[index] for index in validation_indices],
                        model.predict(development_x[validation_indices]),
                    ))
                for threshold in policy["entry_threshold_bps"]:
                    metrics = [
                        simulate(rows, predictions, horizon, float(threshold))
                        for rows, predictions in fold_predictions
                    ]
                    candidates.append({
                        "objective": objective,
                        "horizon_ms": horizon,
                        "threshold_net_return_bps": threshold,
                        "fold_metrics": metrics,
                        "aggregate": aggregate_metrics(metrics),
                    })
        eligible = [
            candidate for candidate in candidates
            if candidate["aggregate"]["trades"] >= policy["minimum_cv_trades"]
            and candidate["aggregate"]["profitable_folds"] >= policy["minimum_profitable_cv_folds"]
            and int(candidate["aggregate"]["net_pnl_micros"]) > 0
        ]
        result: dict[str, Any] = {
            "schema_version": 2,
            "non_authoritative": True,
            "asset": "BTC",
            "capability": "RESEARCH_MODEL_ONLY",
            "dataset_manifest_sha256": sha256(args.dataset / "manifest.json"),
            "auxiliary_manifest_sha256": sha256(args.auxiliary / "manifest.json"),
            "policy_sha256": sha256(args.policy),
            "dataset_source_revision": auxiliary_manifest["dataset_revision"],
            "feature_names": feature_names,
            "development_sessions": len(development_ids),
            "final_test_sessions": len(final_ids),
            "walk_forward_folds": len(folds),
            "development_candidates": candidates,
        }
        args.out_dir.mkdir(parents=True, exist_ok=False)
        if not eligible:
            result["verdict"] = "REJECTED_NO_WALK_FORWARD_EDGE"
            result["final_test_evaluated"] = False
        else:
            selected = max(
                eligible,
                key=lambda candidate: (
                    int(candidate["aggregate"]["net_pnl_micros"]),
                    candidate["aggregate"]["profitable_folds"],
                    candidate["aggregate"]["profit_factor_milli"],
                    -candidate["horizon_ms"],
                ),
            )
            frozen_model = fit_model(
                policy,
                selected["objective"],
                development_x,
                development_ordered,
                selected["horizon_ms"],
            )
            model_path = args.out_dir / "model.txt"
            frozen_model.booster_.save_model(str(model_path))
            # The model, horizon and threshold are frozen before final feature/label evaluation.
            final_x, final_ordered, final_names = build_features(
                final_rows, policy, spot_times, spot_prices, flow,
            )
            if final_names != feature_names:
                raise InvalidModelEvidence("final feature schema drift")
            final_metrics = simulate(
                final_ordered,
                frozen_model.predict(final_x),
                selected["horizon_ms"],
                float(selected["threshold_net_return_bps"]),
            )
            passes = (
                final_metrics["trades"] >= policy["minimum_final_test_trades"]
                and int(final_metrics["net_pnl_micros"]) >= policy["minimum_final_test_net_pnl_micros"]
                and final_metrics["profit_factor_milli"] >= policy["minimum_final_test_profit_factor_milli"]
                and int(final_metrics["max_drawdown_micros"]) <= policy["maximum_final_test_drawdown_micros"]
            )
            result.update({
                "selected": selected,
                "final_test_evaluated": True,
                "final_metrics": final_metrics,
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
            "final_metrics": result.get("final_metrics"),
            "report": str(report_path),
        }, indent=2))
        return 0
    except (InvalidModelEvidence, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"v2 model evaluation rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
