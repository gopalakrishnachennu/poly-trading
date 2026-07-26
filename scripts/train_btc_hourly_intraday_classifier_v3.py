#!/usr/bin/env python3
"""Classify profitable BTC-hourly exits with session walk-forward controls."""
from __future__ import annotations

import argparse
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
        MICROS,
        InvalidModelEvidence,
        canonical,
        exact_pnl,
        load_rows,
        sha256,
        simulate,
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
        MICROS,
        InvalidModelEvidence,
        canonical,
        exact_pnl,
        load_rows,
        sha256,
        simulate,
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


def read_policy(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    expected = {
        "schema_version", "asset", "horizons_ms", "book_history_lags_ms",
        "spot_history_lags_ms", "flow_windows_ms", "probability_threshold_ppm",
        "source_only_latency_reserve_ms", "maximum_spot_age_ms",
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
    if set(value) != expected or value["schema_version"] != 3 or value["asset"] != "BTC":
        raise InvalidModelEvidence("v3 classifier policy schema invalid")
    for field in expected - {
        "schema_version", "asset", "horizons_ms", "book_history_lags_ms",
        "spot_history_lags_ms", "flow_windows_ms", "probability_threshold_ppm",
    }:
        positive_integer(value[field], field)
    for field in (
        "horizons_ms", "book_history_lags_ms", "spot_history_lags_ms",
        "flow_windows_ms", "probability_threshold_ppm",
    ):
        sorted_positive_list(value[field], field)
    if any(value >= MICROS for value in value["probability_threshold_ppm"]):
        raise InvalidModelEvidence("probability threshold must be below one")
    if value["lightgbm_learning_rate_micros"] > MICROS:
        raise InvalidModelEvidence("learning rate exceeds one")
    return value


def fit_classifier(
    policy: dict[str, Any],
    features: np.ndarray,
    rows: list[dict[str, Any]],
    horizon: int,
) -> LGBMClassifier:
    labels = np.asarray([exact_pnl(row, horizon) > 0 for row in rows], dtype=np.int8)
    if len(np.unique(labels)) != 2:
        raise InvalidModelEvidence("classifier fold does not contain both labels")
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
    model.fit(features[::stride], labels[::stride])
    return model


def positive_scores(model: LGBMClassifier, features: np.ndarray) -> np.ndarray:
    probabilities = model.predict_proba(features)
    if probabilities.ndim != 2 or probabilities.shape[1] != 2:
        raise InvalidModelEvidence("classifier probability shape invalid")
    return probabilities[:, 1] * MICROS


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
        verify_dataset(args.dataset)
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
                model = fit_classifier(
                    policy,
                    development_x[train_indices],
                    [development_ordered[index] for index in train_indices],
                    horizon,
                )
                fold_predictions.append((
                    [development_ordered[index] for index in validation_indices],
                    positive_scores(model, development_x[validation_indices]),
                ))
            for threshold in policy["probability_threshold_ppm"]:
                fold_metrics = [
                    simulate(rows, predictions, horizon, float(threshold))
                    for rows, predictions in fold_predictions
                ]
                candidates.append({
                    "horizon_ms": horizon,
                    "probability_threshold_ppm": threshold,
                    "fold_metrics": fold_metrics,
                    "aggregate": aggregate_metrics(fold_metrics),
                })
        eligible = [
            candidate for candidate in candidates
            if candidate["aggregate"]["trades"] >= policy["minimum_cv_trades"]
            and min(metric["trades"] for metric in candidate["fold_metrics"])
            >= policy["minimum_cv_trades_per_fold"]
            and candidate["aggregate"]["profitable_folds"] >= policy["minimum_profitable_cv_folds"]
            and int(candidate["aggregate"]["net_pnl_micros"]) > 0
        ]
        result: dict[str, Any] = {
            "schema_version": 3,
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
            result["verdict"] = "REJECTED_NO_WALK_FORWARD_CLASSIFIER_EDGE"
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
            frozen_model = fit_classifier(
                policy,
                development_x,
                development_ordered,
                selected["horizon_ms"],
            )
            model_path = args.out_dir / "model.txt"
            frozen_model.booster_.save_model(str(model_path))
            final_x, final_ordered, final_names = build_features(
                final_rows, policy, spot_times, spot_prices, flow,
            )
            if final_names != feature_names:
                raise InvalidModelEvidence("final feature schema drift")
            final_metrics = simulate(
                final_ordered,
                positive_scores(frozen_model, final_x),
                selected["horizon_ms"],
                float(selected["probability_threshold_ppm"]),
            )
            passes = (
                final_metrics["trades"] >= policy["minimum_final_test_trades"]
                and int(final_metrics["net_pnl_micros"]) >= policy["minimum_final_test_net_pnl_micros"]
                and final_metrics["profit_factor_milli"] >= policy["minimum_final_test_profit_factor_milli"]
                and int(final_metrics["max_drawdown_micros"])
                <= policy["maximum_final_test_drawdown_micros"]
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
        print(f"v3 classifier evaluation rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
