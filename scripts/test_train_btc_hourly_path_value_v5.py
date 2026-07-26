#!/usr/bin/env python3
"""Failure and accounting tests for the path net-value challenger."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from scripts import train_btc_hourly_path_value_v5 as subject


class PathValueModelTests(unittest.TestCase):
    def row(self, timestamp: int, side: str) -> dict:
        return {
            "condition_id": "condition",
            "side": side,
            "strategy_available_time_ns": timestamp,
        }

    def test_simulation_selects_value_and_respects_open_position(self) -> None:
        up_1 = self.row(1_000, "UP")
        down_1 = self.row(1_000, "DOWN")
        up_2 = self.row(2_000, "UP")
        labels = {
            subject.key(up_1): (500_000, 3_000, "TAKE_PROFIT"),
            subject.key(down_1): (-1_000_000, 3_000, "STOP_LOSS"),
            subject.key(up_2): (900_000, 4_000, "TAKE_PROFIT"),
        }
        result = subject.simulate(
            [up_1, down_1, up_2],
            subject.np.asarray([600_000.0, 100_000.0, 900_000.0]),
            labels,
            250_000,
        )
        self.assertEqual(result["trades"], 1)
        self.assertEqual(result["net_pnl_micros"], "500000")

    def test_policy_rejects_unapproved_objective(self) -> None:
        policy = json.loads(
            Path("config/btc-hourly-path-value-v5-policy.example.json").read_text()
        )
        policy["model_objectives"] = ["regression_l2"]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            path.write_text(json.dumps(policy))
            with self.assertRaisesRegex(subject.InvalidModelEvidence, "objectives"):
                subject.read_policy(path)


if __name__ == "__main__":
    unittest.main()
