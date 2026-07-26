#!/usr/bin/env python3
"""Train a causal BTC-hourly path net-value challenger."""
from __future__ import annotations

import argparse
import json
import os
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

import numpy as np
from lightgbm import LGBMRegressor

try:
    from scripts.train_btc_hourly_intraday import (
        InvalidModelEvidence, canonical, load_rows, sha256, verify_dataset,
    )
    from scripts.train_btc_hourly_intraday_v2 import (
        MICROS, aggregate_metrics, build_features, load_auxiliary,
        partition_sessions, positive_integer, sorted_positive_list,
        verify_auxiliary, walk_forward_folds,
    )
    from scripts.train_btc_hourly_path_model_v4 import (
        key, path_labels,
    )
except ModuleNotFoundError:
    from train_btc_hourly_intraday import (
        InvalidModelEvidence, canonical, load_rows, sha256, verify_dataset,
    )
    from train_btc_hourly_intraday_v2 import (
        MICROS, aggregate_metrics, build_features, load_auxiliary,
        partition_sessions, positive_integer, sorted_positive_list,
        verify_auxiliary, walk_forward_folds,
    )
    from train_btc_hourly_path_model_v4 import (
        key, path_labels,
    )


def read_policy(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    expected = {
        "schema_version", "asset", "book_history_lags_ms",
        "spot_history_lags_ms", "flow_windows_ms",
        "source_only_latency_reserve_ms", "maximum_spot_age_ms",
        "minimum_ask_ppm", "maximum_ask_ppm", "maximum_entry_spread_ppm",
        "minimum_session_progress_bps", "maximum_session_progress_bps",
        "take_profit_bps", "stop_loss_bps", "maximum_holding_ms",
        "maximum_exit_frame_gap_ms", "model_objectives",
        "minimum_predicted_net_pnl_micros", "minimum_development_sessions",
        "final_test_sessions", "walk_forward_initial_sessions",
        "walk_forward_fold_sessions", "minimum_profitable_cv_folds",
        "minimum_cv_trades", "minimum_cv_trades_per_fold",
        "minimum_final_test_trades", "minimum_final_test_net_pnl_micros",
        "minimum_final_test_profit_factor_milli",
        "maximum_final_test_drawdown_micros", "training_row_stride",
        "lightgbm_estimators", "lightgbm_learning_rate_micros",
        "lightgbm_num_leaves", "lightgbm_min_child_samples", "random_seed",
    }
    if set(value) != expected or value["schema_version"] != 5 or value["asset"] != "BTC":
        raise InvalidModelEvidence("v5 path-value policy schema invalid")
    for field in expected - {
        "schema_version", "asset", "book_history_lags_ms",
        "spot_history_lags_ms", "flow_windows_ms", "model_objectives",
        "minimum_predicted_net_pnl_micros",
    }:
        positive_integer(value[field], field)
    for field in ("book_history_lags_ms", "spot_history_lags_ms", "flow_windows_ms"):
        sorted_positive_list(value[field], field)
    sorted_positive_list(
        value["minimum_predicted_net_pnl_micros"],
        "minimum_predicted_net_pnl_micros",
    )
    if value["model_objectives"] != ["regression_l1", "regression_l2"]:
        raise InvalidModelEvidence("v5 objectives must be frozen l1 and l2")
    if not (
        value["minimum_ask_ppm"] < value["maximum_ask_ppm"] <= MICROS
        and value["minimum_session_progress_bps"]
        < value["maximum_session_progress_bps"] <= 10_000
        and value["lightgbm_learning_rate_micros"] <= MICROS
    ):
        raise InvalidModelEvidence("v5 economic bounds invalid")
    return value


def consistent_features(
    rows: list[dict[str, Any]],
    policy: dict[str, Any],
    spot_times: list[int],
    spot_prices: list[int],
    flow: dict[str, tuple[list[int], list[int], list[int]]],
) -> tuple[np.ndarray, list[dict[str, Any]], list[str]]:
    full, ordered, names = build_features(rows, policy, spot_times, spot_prices, flow)
    allowed = {
        "ask_ppm", "bid_ppm", "spread_ppm", "session_progress_bps",
        "time_to_expiry_ms", "side_up", "pair_ask_sum_ppm",
        "spot_distance_from_open_ppm", "spot_age_ms",
    }
    allowed.update(name for name in names if name.startswith("book_mid_delta_"))
    allowed.update(name for name in names if name.startswith("spot_return_"))
    indices = [index for index, name in enumerate(names) if name in allowed]
    selected = [names[index] for index in indices]
    if not selected or any(
        name.startswith(("flow_", "depth_")) or name in {"log_bid_depth", "log_ask_depth"}
        for name in selected
    ):
        raise InvalidModelEvidence("consistent feature boundary failed")
    return full[:, indices], ordered, selected


def fit_model(
    policy: dict[str, Any],
    objective: str,
    features: np.ndarray,
    rows: list[dict[str, Any]],
    labels: dict[tuple[str, str, int], tuple[int, int, str]],
) -> LGBMRegressor:
    targets = np.asarray([labels[key(row)][0] for row in rows], dtype=np.float64)
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
    model.fit(features[::stride], targets[::stride])
    return model


def simulate(
    rows: list[dict[str, Any]],
    predictions: np.ndarray,
    labels: dict[tuple[str, str, int], tuple[int, int, str]],
    threshold: int,
) -> dict[str, Any]:
    candidates = sorted(
        zip(rows, predictions.tolist()),
        key=lambda item: (
            item[0]["condition_id"],
            int(item[0]["strategy_available_time_ns"]),
            -item[1],
            item[0]["side"],
        ),
    )
    pnl: list[int] = []
    reasons: dict[str, int] = defaultdict(int)
    open_until: dict[str, int] = {}
    cursor = 0
    while cursor < len(candidates):
        row, _ = candidates[cursor]
        condition = row["condition_id"]
        now = int(row["strategy_available_time_ns"])
        same: list[tuple[dict[str, Any], float]] = []
        while cursor < len(candidates):
            candidate = candidates[cursor]
            if (
                candidate[0]["condition_id"] != condition
                or int(candidate[0]["strategy_available_time_ns"]) != now
            ):
                break
            same.append(candidate)
            cursor += 1
        if now < open_until.get(condition, -1):
            continue
        chosen, prediction = max(same, key=lambda item: (item[1], item[0]["side"]))
        label = labels.get(key(chosen))
        if prediction < threshold or label is None:
            continue
        value, exit_time, reason = label
        pnl.append(value)
        reasons[reason] += 1
        open_until[condition] = exit_time
    equity = peak = drawdown = gains = losses = 0
    for value in pnl:
        equity += value
        peak = max(peak, equity)
        drawdown = max(drawdown, peak - equity)
        if value > 0:
            gains += value
        elif value < 0:
            losses -= value
    return {
        "trades": len(pnl),
        "wins": sum(value > 0 for value in pnl),
        "net_pnl_micros": str(sum(pnl)),
        "gross_profit_micros": str(gains),
        "gross_loss_micros": str(losses),
        "profit_factor_milli": gains * 1000 // losses if losses else (1_000_000 if gains else 0),
        "max_drawdown_micros": str(drawdown),
        "exit_reasons": dict(sorted(reasons.items())),
    }


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
        horizons = sorted(
            int(value) for value in next(iter(
                json.loads(line)["future"]
                for line in (args.dataset / "train.jsonl").open()
                if line.strip()
            )).keys()
        )
        all_rows: list[dict[str, Any]] = []
        for fold_name in ("train", "validation", "test"):
            all_rows.extend(load_rows(args.dataset / f"{fold_name}.jsonl", horizons))
        development, final_rows, development_ids, final_ids = partition_sessions(all_rows, policy)
        spot_times, spot_prices, flow = load_auxiliary(args.auxiliary)
        development_x, ordered, feature_names = consistent_features(
            development, policy, spot_times, spot_prices, flow,
        )
        labels = path_labels(ordered, policy)
        eligible_indices = [index for index, row in enumerate(ordered) if key(row) in labels]
        development_x = development_x[eligible_indices]
        ordered = [ordered[index] for index in eligible_indices]
        by_condition: dict[str, list[int]] = defaultdict(list)
        for index, row in enumerate(ordered):
            by_condition[row["condition_id"]].append(index)
        folds = walk_forward_folds(development_ids, policy)
        fold_data: dict[str, list[tuple[list[dict[str, Any]], np.ndarray]]] = {}
        for objective in policy["model_objectives"]:
            predictions: list[tuple[list[dict[str, Any]], np.ndarray]] = []
            for train_ids, validation_ids in folds:
                train_indices = [
                    index for condition in train_ids for index in by_condition.get(condition, [])
                ]
                validation_indices = [
                    index for condition in validation_ids for index in by_condition.get(condition, [])
                ]
                if not train_indices or not validation_indices:
                    raise InvalidModelEvidence("path-value fold has no eligible rows")
                model = fit_model(
                    policy, objective, development_x[train_indices],
                    [ordered[index] for index in train_indices], labels,
                )
                predictions.append((
                    [ordered[index] for index in validation_indices],
                    model.predict(development_x[validation_indices]),
                ))
            fold_data[objective] = predictions
        candidates: list[dict[str, Any]] = []
        for objective, predictions in fold_data.items():
            for threshold in policy["minimum_predicted_net_pnl_micros"]:
                metrics = [
                    simulate(rows, scores, labels, threshold)
                    for rows, scores in predictions
                ]
                candidates.append({
                    "objective": objective,
                    "minimum_predicted_net_pnl_micros": str(threshold),
                    "fold_metrics": metrics,
                    "aggregate": aggregate_metrics(metrics),
                })
        eligible = [
            candidate for candidate in candidates
            if candidate["aggregate"]["trades"] >= policy["minimum_cv_trades"]
            and min(metric["trades"] for metric in candidate["fold_metrics"])
            >= policy["minimum_cv_trades_per_fold"]
            and candidate["aggregate"]["profitable_folds"]
            >= policy["minimum_profitable_cv_folds"]
            and int(candidate["aggregate"]["net_pnl_micros"]) > 0
        ]
        result: dict[str, Any] = {
            "schema_version": 5,
            "non_authoritative": True,
            "asset": "BTC",
            "capability": "RESEARCH_MODEL_ONLY",
            "dataset_manifest_sha256": sha256(args.dataset / "manifest.json"),
            "auxiliary_manifest_sha256": sha256(args.auxiliary / "manifest.json"),
            "policy_sha256": sha256(args.policy),
            "dataset_source_revision": auxiliary_manifest["dataset_revision"],
            "dataset_sessions": sum(
                dataset_manifest["folds"][name]["sessions"]
                for name in ("train", "validation", "test")
            ),
            "feature_names": feature_names,
            "development_sessions": len(development_ids),
            "development_path_labels": len(labels),
            "final_test_sessions": len(final_ids),
            "walk_forward_folds": len(folds),
            "development_candidates": candidates,
        }
        args.out_dir.mkdir(parents=True, exist_ok=False)
        if not eligible:
            result["verdict"] = "REJECTED_NO_PATH_NET_VALUE_EDGE"
            result["final_test_evaluated"] = False
        else:
            selected = max(
                eligible,
                key=lambda candidate: (
                    int(candidate["aggregate"]["net_pnl_micros"]),
                    candidate["aggregate"]["profitable_folds"],
                    candidate["aggregate"]["profit_factor_milli"],
                ),
            )
            frozen = fit_model(policy, selected["objective"], development_x, ordered, labels)
            model_path = args.out_dir / "model.txt"
            frozen.booster_.save_model(str(model_path))
            final_x, final_ordered, final_names = consistent_features(
                final_rows, policy, spot_times, spot_prices, flow,
            )
            if final_names != feature_names:
                raise InvalidModelEvidence("final feature schema drift")
            final_labels = path_labels(final_ordered, policy)
            final_indices = [
                index for index, row in enumerate(final_ordered) if key(row) in final_labels
            ]
            final_x = final_x[final_indices]
            final_ordered = [final_ordered[index] for index in final_indices]
            final_metrics = simulate(
                final_ordered, frozen.predict(final_x), final_labels,
                int(selected["minimum_predicted_net_pnl_micros"]),
            )
            passes = (
                final_metrics["trades"] >= policy["minimum_final_test_trades"]
                and int(final_metrics["net_pnl_micros"])
                >= policy["minimum_final_test_net_pnl_micros"]
                and final_metrics["profit_factor_milli"]
                >= policy["minimum_final_test_profit_factor_milli"]
                and int(final_metrics["max_drawdown_micros"])
                <= policy["maximum_final_test_drawdown_micros"]
            )
            result.update({
                "selected": selected,
                "final_test_evaluated": True,
                "final_metrics": final_metrics,
                "final_path_labels": len(final_labels),
                "model_sha256": sha256(model_path),
                "verdict": "PAPER_CANDIDATE_ONLY" if passes else "REJECTED_UNSEEN_TEST",
            })
        temporary = args.out_dir / "report.json.tmp"
        temporary.write_bytes(canonical(result) + b"\n")
        os.replace(temporary, args.out_dir / "report.json")
        print(json.dumps({
            "verdict": result["verdict"],
            "selected": result.get("selected"),
            "final_metrics": result.get("final_metrics"),
            "report": str(args.out_dir / "report.json"),
        }, indent=2))
        return 0
    except (InvalidModelEvidence, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"v5 path-value evaluation rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
