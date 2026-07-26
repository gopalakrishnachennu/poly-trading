#!/usr/bin/env python3
"""Focused causal/backtest tests for the BTC-hourly challenger."""
from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

import numpy as np


MODULE = Path(__file__).with_name("train_btc_hourly_intraday.py")
SPEC = importlib.util.spec_from_file_location("intraday_trainer", MODULE)
assert SPEC and SPEC.loader
trainer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(trainer)


def row(time_ns: int, side: str, pnl: int, exit_ns: int) -> dict:
    cost = 1_000_000
    fee = 10_000
    return {
        "condition_id": "condition", "side": side,
        "strategy_available_time_ns": time_ns,
        "entry_cost_micros": str(cost),
        "current_exit_proceeds_micros": "900000",
        "target_shares_micros": "1000000",
        "bid_depth_micros": "10000000", "ask_depth_micros": "10000000",
        "session_progress_bps": 1000, "time_to_expiry_ms": 100000,
        "fee_reserve_bps": 100,
        "future": {"15000": {
            "exit_proceeds_micros": str(cost + fee + pnl),
            "exit_available_ns": exit_ns,
            "net_return_bps": pnl * 10000 // cost,
        }},
    }


class IntradayTrainerTests(unittest.TestCase):
    def test_simulator_keeps_one_non_overlapping_position(self) -> None:
        rows = [
            row(1_000_000_000, "UP", 100_000, 3_000_000_000),
            row(1_000_000_000, "DOWN", -100_000, 3_000_000_000),
            row(2_000_000_000, "UP", 100_000, 4_000_000_000),
            row(4_000_000_000, "UP", 100_000, 5_000_000_000),
        ]
        metrics = trainer.simulate(rows, np.asarray([2.0, 1.0, 3.0, 2.0]), 15000, 1.5)
        self.assertEqual(metrics["trades"], 2)
        self.assertEqual(metrics["net_pnl_micros"], "200000")

    def test_exact_pnl_uses_integer_fee_reserve(self) -> None:
        value = row(1, "UP", 12345, 2)
        self.assertEqual(trainer.exact_pnl(value, 15000), 12345)


if __name__ == "__main__":
    unittest.main()
