#!/usr/bin/env python3
"""Train a session-walk-forward BTC-hourly path-dependent exit challenger."""
from __future__ import annotations

import argparse
import bisect
import json
import os
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

import numpy as np
from lightgbm import LGBMClassifier

try:
    from scripts.train_btc_hourly_intraday import (
        BPS,
        MICROS,
        InvalidModelEvidence,
        canonical,
        load_rows,
        sha256,
        verify_dataset,
    )
    from scripts.train_btc_hourly_intraday_v2 import (
        aggregate_metrics,
        build_features,
        load_auxiliary,
        partition_sessions,
        positive_integer,
        sorted_positive_list,
        verify_auxiliary,
        walk_forward_folds,
    )
except ModuleNotFoundError:
    from train_btc_hourly_intraday import (
        BPS,
        MICROS,
        InvalidModelEvidence,
        canonical,
        load_rows,
        sha256,
        verify_dataset,
    )
    from train_btc_hourly_intraday_v2 import (
        aggregate_metrics,
        build_features,
        load_auxiliary,
        partition_sessions,
        positive_integer,
        sorted_positive_list,
        verify_auxiliary,
        walk_forward_folds,
    )

ROW_KEY = tuple[str, str, int]


def read_policy(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    expected = {
        "schema_version", "asset", "book_history_lags_ms",
        "spot_history_lags_ms", "flow_windows_ms",
        "source_only_latency_reserve_ms", "maximum_spot_age_ms",
        "minimum_ask_ppm", "maximum_ask_ppm", "maximum_entry_spread_ppm",
        "minimum_session_progress_bps", "maximum_session_progress_bps",
        "take_profit_bps", "stop_loss_bps", "maximum_holding_ms",
        "maximum_exit_frame_gap_ms", "probability_threshold_ppm",
        "minimum_development_sessions", "final_test_sessions",
        "walk_forward_initial_sessions", "walk_forward_fold_sessions",
        "minimum_profitable_cv_folds", "minimum_cv_trades",
        "minimum_cv_trades_per_fold", "minimum_final_test_trades",
        "minimum_final_test_net_pnl_micros",
        "minimum_final_test_profit_factor_milli",
        "maximum_final_test_drawdown_micros", "training_row_stride",
        "lightgbm_estimators", "lightgbm_learning_rate_micros",
        "lightgbm_num_leaves", "lightgbm_min_child_samples", "random_seed",
    }
    if set(value) != expected or value["schema_version"] != 4 or value["asset"] != "BTC":
        raise InvalidModelEvidence("v4 path policy schema invalid")
    for field in expected - {
        "schema_version", "asset", "book_history_lags_ms",
        "spot_history_lags_ms", "flow_windows_ms", "probability_threshold_ppm",
    }:
        positive_integer(value[field], field)
    for field in (
        "book_history_lags_ms", "spot_history_lags_ms", "flow_windows_ms",
        "probability_threshold_ppm",
    ):
        sorted_positive_list(value[field], field)
    if not (
        value["minimum_ask_ppm"] < value["maximum_ask_ppm"] <= MICROS
        and value["minimum_session_progress_bps"] < value["maximum_session_progress_bps"] <= BPS
        and all(threshold < MICROS for threshold in value["probability_threshold_ppm"])
    ):
        raise InvalidModelEvidence("v4 economic bounds invalid")
    if value["lightgbm_learning_rate_micros"] > MICROS:
        raise InvalidModelEvidence("learning rate exceeds one")
    return value


def key(row: dict[str, Any]) -> ROW_KEY:
    return (
        row["condition_id"],
        row["side"],
        int(row["strategy_available_time_ns"]),
    )


def per_share_prices(row: dict[str, Any]) -> tuple[int, int]:
    quantity = int(row["target_shares_micros"])
    if quantity <= 0:
        raise InvalidModelEvidence("target quantity is invalid")
    return (
        int(row["entry_cost_micros"]) * MICROS // quantity,
        int(row["current_exit_proceeds_micros"]) * MICROS // quantity,
    )


def entry_eligible(row: dict[str, Any], policy: dict[str, Any]) -> bool:
    ask, bid = per_share_prices(row)
    progress = int(row["session_progress_bps"])
    return (
        policy["minimum_ask_ppm"] <= ask <= policy["maximum_ask_ppm"]
        and 0 <= ask - bid <= policy["maximum_entry_spread_ppm"]
        and policy["minimum_session_progress_bps"] <= progress
        <= policy["maximum_session_progress_bps"]
    )


def path_labels(
    rows: list[dict[str, Any]], policy: dict[str, Any],
) -> dict[ROW_KEY, tuple[int, int, str]]:
    by_series: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        by_series[(row["condition_id"], row["side"])].append(row)
    labels: dict[ROW_KEY, tuple[int, int, str]] = {}
    for series in by_series.values():
        series.sort(key=lambda row: int(row["strategy_available_time_ns"]))
        times = [int(row["strategy_available_time_ns"]) for row in series]
        for index, row in enumerate(series):
            if not entry_eligible(row, policy):
                continue
            cost = int(row["entry_cost_micros"])
            fee = (cost * int(row["fee_reserve_bps"]) + BPS - 1) // BPS
            profit_target = cost + fee + (cost * policy["take_profit_bps"] + BPS - 1) // BPS
            stop_threshold = max(0, cost - (cost * policy["stop_loss_bps"] + BPS - 1) // BPS)
            deadline = times[index] + policy["maximum_holding_ms"] * MICROS
            deadline_index = bisect.bisect_left(times, deadline, lo=index + 1)
            chosen: tuple[int, int, str] | None = None
            cursor = index + 1
            while cursor < len(series) and times[cursor] <= deadline:
                proceeds = int(series[cursor]["current_exit_proceeds_micros"])
                reason = (
                    "STOP_LOSS" if proceeds <= stop_threshold
                    else "TAKE_PROFIT" if proceeds >= profit_target
                    else ""
                )
                if reason:
                    chosen = (proceeds - cost - fee, times[cursor], reason)
                    break
                cursor += 1
            if chosen is None and deadline_index < len(series):
                gap = times[deadline_index] - deadline
                if gap <= policy["maximum_exit_frame_gap_ms"] * MICROS:
                    proceeds = int(series[deadline_index]["current_exit_proceeds_micros"])
                    chosen = (proceeds - cost - fee, times[deadline_index], "TIMEOUT")
            if chosen is not None:
                labels[key(row)] = chosen
    return labels


def fit_classifier(
    policy: dict[str, Any],
    features: np.ndarray,
    rows: list[dict[str, Any]],
    labels: dict[ROW_KEY, tuple[int, int, str]],
) -> LGBMClassifier:
    targets = np.asarray([labels[key(row)][0] > 0 for row in rows], dtype=np.int8)
    if len(np.unique(targets)) != 2:
        raise InvalidModelEvidence("path classifier fold lacks both labels")
    stride = policy["training_row_stride"]
    model = LGBMClassifier(
        objective="binary",
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


def scores(model: LGBMClassifier, features: np.ndarray) -> np.ndarray:
    probabilities = model.predict_proba(features)
    if probabilities.ndim != 2 or probabilities.shape[1] != 2:
        raise InvalidModelEvidence("path classifier probability shape invalid")
    return probabilities[:, 1] * MICROS


def simulate(
    rows: list[dict[str, Any]],
    predictions: np.ndarray,
    labels: dict[ROW_KEY, tuple[int, int, str]],
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
        chosen, score = max(same, key=lambda item: (item[1], item[0]["side"]))
        label = labels.get(key(chosen))
        if score < threshold or label is None:
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
        for fold in ("train", "validation", "test"):
            all_rows.extend(load_rows(args.dataset / f"{fold}.jsonl", horizons))
        development, final_rows, development_ids, final_ids = partition_sessions(all_rows, policy)
        spot_times, spot_prices, flow = load_auxiliary(args.auxiliary)
        development_x, development_ordered, feature_names = build_features(
            development, policy, spot_times, spot_prices, flow,
        )
        development_labels = path_labels(development_ordered, policy)
        eligible_indices = [
            index for index, row in enumerate(development_ordered)
            if key(row) in development_labels
        ]
        development_x = development_x[eligible_indices]
        development_ordered = [development_ordered[index] for index in eligible_indices]
        by_condition_indices: dict[str, list[int]] = defaultdict(list)
        for index, row in enumerate(development_ordered):
            by_condition_indices[row["condition_id"]].append(index)
        folds = walk_forward_folds(development_ids, policy)
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
                raise InvalidModelEvidence("path fold has no eligible rows")
            model = fit_classifier(
                policy,
                development_x[train_indices],
                [development_ordered[index] for index in train_indices],
                development_labels,
            )
            fold_predictions.append((
                [development_ordered[index] for index in validation_indices],
                scores(model, development_x[validation_indices]),
            ))
        candidates: list[dict[str, Any]] = []
        for threshold in policy["probability_threshold_ppm"]:
            fold_metrics = [
                simulate(rows, predictions, development_labels, threshold)
                for rows, predictions in fold_predictions
            ]
            candidates.append({
                "probability_threshold_ppm": threshold,
                "fold_metrics": fold_metrics,
                "aggregate": aggregate_metrics(fold_metrics),
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
            "schema_version": 4,
            "non_authoritative": True,
            "asset": "BTC",
            "capability": "RESEARCH_MODEL_ONLY",
            "dataset_manifest_sha256": sha256(args.dataset / "manifest.json"),
            "auxiliary_manifest_sha256": sha256(args.auxiliary / "manifest.json"),
            "policy_sha256": sha256(args.policy),
            "dataset_source_revision": auxiliary_manifest["dataset_revision"],
            "dataset_sessions": sum(
                dataset_manifest["folds"][fold]["sessions"]
                for fold in ("train", "validation", "test")
            ),
            "feature_names": feature_names,
            "development_sessions": len(development_ids),
            "development_path_labels": len(development_labels),
            "final_test_sessions": len(final_ids),
            "walk_forward_folds": len(folds),
            "development_candidates": candidates,
        }
        args.out_dir.mkdir(parents=True, exist_ok=False)
        if not eligible:
            result["verdict"] = "REJECTED_NO_PATH_DEPENDENT_EDGE"
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
            frozen_model = fit_classifier(
                policy, development_x, development_ordered, development_labels,
            )
            model_path = args.out_dir / "model.txt"
            frozen_model.booster_.save_model(str(model_path))
            final_x, final_ordered, final_names = build_features(
                final_rows, policy, spot_times, spot_prices, flow,
            )
            if final_names != feature_names:
                raise InvalidModelEvidence("final feature schema drift")
            final_labels = path_labels(final_ordered, policy)
            final_indices = [
                index for index, row in enumerate(final_ordered)
                if key(row) in final_labels
            ]
            final_x = final_x[final_indices]
            final_ordered = [final_ordered[index] for index in final_indices]
            final_metrics = simulate(
                final_ordered,
                scores(frozen_model, final_x),
                final_labels,
                selected["probability_threshold_ppm"],
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
        print(f"v4 path evaluation rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
