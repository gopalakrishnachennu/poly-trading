#!/usr/bin/env python3
"""Tests for the consolidated paper-trade strategy export."""
from __future__ import annotations

import csv
import json
import tempfile
import unittest
from pathlib import Path

from scripts import export_paper_trades_with_strategies as subject


class TradeHistoryExportTests(unittest.TestCase):
    def fixture(self, root: Path) -> tuple[Path, Path]:
        audit = {
            "schema_version": 1,
            "sources": [{"path": "paper.jsonl", "bytes": 1, "lines": 2}],
            "trades": [
                {
                    "source_path": "paper.jsonl",
                    "source_line": 2,
                    "record_digest": "a" * 64,
                    "record": {
                        "campaign_id": "campaign",
                        "runtime_config_id": "runtime",
                        "runtime_config_digest": "b" * 64,
                        "event_time_ms": 1000,
                        "kind": "filled_pair",
                        "payload": {
                            "asset": "BTC",
                            "condition_id": "condition",
                            "trade_id": "trade",
                            "state": "FILLED_PAIR_LOCKED",
                            "quantity_micros": "1000000",
                            "up_price_micros": "400000",
                            "down_price_micros": "590000",
                            "fee_micros": "1000",
                            "slippage_micros": "500",
                            "cost_micros": "991500",
                            "locked_pnl_micros": "8500",
                            "decision_at_ms": 1000,
                        },
                    },
                    "decision": {
                        "event_time_ms": 1000,
                        "kind": "PAIR_BUY",
                        "payload": {
                            "asset": "BTC",
                            "condition_id": "condition",
                            "fee_micros": 1000,
                            "slippage_micros": 500,
                            "pair_cost_micros": 990000,
                            "net_cost_micros": 991500,
                            "quantity_micros": 1000000,
                            "policy_id": "policy",
                            "policy_digest": "c" * 64,
                            "reason": "edge",
                        },
                    },
                }
            ],
        }
        audit_path = root / "audit.json"
        audit_path.write_text(json.dumps(audit, separators=(",", ":")) + "\n")
        legacy = root / "legacy"
        legacy.mkdir()
        settlement = {
            "t": 2000,
            "settled_t": 3000,
            "asset": "BTC",
            "window": "5m",
            "market": "market",
            "side": "UP",
            "price": 0.4,
            "stake": 10.0,
            "shares": 25.0,
            "won": True,
            "pnl": 15.0,
            "bankroll": 1015.0,
            "strategy": "recorded_strategy",
        }
        (legacy / "bets-day.jsonl").write_text(
            json.dumps(settlement, separators=(",", ":")) + "\n"
        )
        intraday = {
            "asset": "BTC",
            "window": "15m",
            "condition_id": "condition-2",
            "side_token": "DOWN",
            "opened_ms": 4000,
            "closed_ms": 5000,
            "entry_price": 0.25,
            "exit_price": 0.4,
            "stake": 10.0,
            "shares": 40.0,
            "pnl": 6.0,
            "won": True,
            "bankroll_after": 1021.0,
            "exit_reason": "target_hit",
        }
        (legacy / "intraday-bets-day.jsonl").write_text(
            json.dumps(intraday, separators=(",", ":")) + "\n"
        )
        contract = root / "live_paper_intraday.py"
        contract.write_text("# contract\n")
        return audit_path, legacy

    def test_exports_all_trade_classes_with_strategy_provenance(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            audit, legacy = self.fixture(root)
            original = subject.__file__
            try:
                subject.__file__ = str(root / "exporter.py")
                report = subject.export(audit, legacy, root / "output")
            finally:
                subject.__file__ = original
            self.assertEqual(report["rows"], 3)
            pointer = json.loads((root / "output" / "latest.json").read_text())
            with Path(pointer["csv_path"]).open(newline="", encoding="utf-8") as source:
                rows = list(csv.DictReader(source))
            self.assertEqual(
                {row["record_class"] for row in rows},
                {
                    "CORE_PAPER_PAIR",
                    "LEGACY_DIRECTIONAL_SETTLEMENT",
                    "LEGACY_INTRADAY_ROUND_TRIP",
                },
            )
            self.assertTrue(all(row["real_money"] == "false" for row in rows))

    def test_missing_recorded_strategy_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            _, legacy = self.fixture(root)
            path = legacy / "bets-day.jsonl"
            value = json.loads(path.read_text())
            value.pop("strategy")
            path.write_text(json.dumps(value) + "\n")
            with self.assertRaisesRegex(subject.InvalidTradeHistory, "strategy"):
                subject.load_legacy_settlement(path)


if __name__ == "__main__":
    unittest.main()
