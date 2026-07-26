#!/usr/bin/env python3
"""Failure-oriented tests for path-dependent BTC-hourly exit labels."""
from __future__ import annotations

import copy
import unittest
from pathlib import Path

from scripts.train_btc_hourly_path_model_v4 import key, path_labels, read_policy


POLICY = read_policy(
    Path(__file__).parents[1] / "config" / "btc-hourly-path-model-v4-policy.example.json"
)


def row(time_ms: int, *, cost: int = 10_000_000, proceeds: int = 9_900_000) -> dict:
    return {
        "condition_id": "condition",
        "side": "UP",
        "strategy_available_time_ns": time_ms * 1_000_000,
        "target_shares_micros": "20000000",
        "entry_cost_micros": str(cost),
        "current_exit_proceeds_micros": str(proceeds),
        "fee_reserve_bps": 100,
        "session_progress_bps": 1000,
    }


class PathLabelTests(unittest.TestCase):
    def test_first_take_profit_is_used_with_exact_fee(self) -> None:
        entry = row(1_000)
        rows = [entry, row(6_000, proceeds=10_300_000), row(11_000, proceeds=10_700_000)]
        label = path_labels(rows, POLICY)[key(entry)]
        self.assertEqual(label, (600_000, 11_000_000_000, "TAKE_PROFIT"))

    def test_stop_loss_precedes_later_profit(self) -> None:
        entry = row(1_000)
        rows = [entry, row(6_000, proceeds=9_400_000), row(11_000, proceeds=10_700_000)]
        label = path_labels(rows, POLICY)[key(entry)]
        self.assertEqual(label, (-700_000, 6_000_000_000, "STOP_LOSS"))

    def test_timeout_requires_bounded_future_frame(self) -> None:
        policy = copy.deepcopy(POLICY)
        policy["maximum_holding_ms"] = 10_000
        entry = row(1_000)
        rows = [entry, row(12_000, proceeds=10_200_000)]
        label = path_labels(rows, policy)[key(entry)]
        self.assertEqual(label, (100_000, 12_000_000_000, "TIMEOUT"))
        policy["maximum_exit_frame_gap_ms"] = 500
        self.assertNotIn(key(entry), path_labels(rows, policy))

    def test_wide_entry_spread_is_not_labeled(self) -> None:
        entry = row(1_000, proceeds=8_000_000)
        rows = [entry, row(6_000, proceeds=10_700_000)]
        self.assertNotIn(key(entry), path_labels(rows, POLICY))


if __name__ == "__main__":
    unittest.main()
