import csv
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from export_last_48h_capture import HOURS_48_NS, ExportError, export


def line(value):
    return json.dumps(value, separators=(",", ":")) + "\n"


class Last48HourExportTests(unittest.TestCase):
    def fixture(self, root: Path, end_ns: int):
        for window in ("5m", "15m", "1h"):
            directory = root / "var" / f"research-{window}"
            directory.mkdir(parents=True)
            for asset in ("btc", "eth"):
                condition = f"{asset}-{window}"
                content = line(
                    {
                        "k": "sys",
                        "payload": {
                            "event_type": "market_identity",
                            "asset": asset.upper(),
                            "condition_id": condition,
                            "up_token_id": condition + "-up",
                            "down_token_id": condition + "-down",
                            "start_time_ms": (end_ns // 1_000_000) - 60_000,
                            "end_time_ms": (end_ns // 1_000_000) + 60_000,
                        },
                    }
                )
                content += line(
                    {
                        "k": "book",
                        "tok": condition + "-up",
                        "cond": condition,
                        "t": end_ns - 1,
                        "bids": [[400_000, 10_000_000]],
                        "asks": [[410_000, 11_000_000]],
                    }
                )
                (directory / f"{asset}-fixture.jsonl").write_text(content)

        snapshots = root / "var" / "research-capture"
        snapshots.mkdir(parents=True)
        (snapshots / "snapshots-fixture.jsonl").write_text(
            line(
                {
                    "record": {
                        "kind": "observation",
                        "event_time_ms": (end_ns // 1_000_000) - 1,
                        "sequence": 1,
                        "snapshot_digest": "digest",
                        "payload": {
                            "asset": "BTC",
                            "condition_id": "btc-1h",
                            "reference_price_micros": "100",
                            "target_price_micros": "99",
                        },
                    }
                }
            )
        )

        coinbase = root / "var" / "research-coinbase"
        coinbase.mkdir(parents=True)
        (coinbase / "btcusd-fixture.jsonl").write_text(
            line(
                {
                    "t_recv_ms": (end_ns // 1_000_000) - 1,
                    "t_event_ms": (end_ns // 1_000_000) - 2,
                    "symbol": "BTCUSD",
                    "bid": 100.01,
                    "ask": 100.02,
                    "mid": 100.015,
                    "bid_sz": 1.2,
                    "ask_sz": 1.3,
                    "seq": 1,
                    "src": "fixture",
                }
            )
        )

        decisions = root / "var" / "btc-window-paper"
        decisions.mkdir(parents=True)
        decision = {
            "strategy_version": "btc_window_engine_v1",
            "condition_id": "btc-5m",
            "asset": "BTC",
            "window": "5m",
            "elapsed_secs": 10,
            "observed_at_ns": end_ns - 1,
            "action": "open",
            "reason": "ENTRY_FILLED_AT_ASK",
            "rejection_reasons": [],
            "setup_id": "fixture_setup",
            "side": "up",
            "ask_micros": 400_000,
            "bid_micros": 390_000,
            "stake_micros": 10_000_000,
            "paper_only": True,
            "real_money": False,
        }
        (decisions / "decisions-fixture.jsonl").write_text(line(decision))

        campaign = root / "var" / "paper-campaign"
        campaign.mkdir(parents=True)
        record = {
            "campaign_id": "paper-fixture",
            "event_time_ms": (end_ns // 1_000_000) - 1,
            "kind": "PAIR_BUY",
            "payload": {
                "asset": "BTC",
                "condition_id": "btc-1h",
                "quantity_micros": 1_000_000,
            },
            "recorded_time_ms": (end_ns // 1_000_000) - 1,
            "sequence": 1,
            "stream": "strategy",
        }
        digest = hashlib.sha256(json.dumps(record, sort_keys=True).encode()).hexdigest()
        (campaign / "paper-fixture.jsonl").write_text(
            line({"record": record, "record_digest": digest})
        )
        (root / "var" / "live-paper").mkdir(parents=True)

    def test_exports_all_windows_and_trade_authorities(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            end_ns = HOURS_48_NS + 10_000_000_000
            self.fixture(root, end_ns)
            output = export(root, root / "out", end_ns)
            manifest = json.loads((output / "manifest.json").read_text())
            counts = {item["file"]: item["rows"] for item in manifest["output_files"]}
            for asset in ("BTC", "ETH"):
                for window in ("5m", "15m", "1h"):
                    self.assertEqual(
                        counts[f"{asset}-{window}-full-depth-books.csv"], 1
                    )
            self.assertEqual(counts["strategy-decisions.csv"], 1)
            self.assertEqual(counts["all-paper-trade-events.csv"], 2)
            with (output / "all-paper-trade-events.csv").open(newline="") as handle:
                authorities = {
                    row["authority_class"] for row in csv.DictReader(handle)
                }
            self.assertEqual(
                authorities, {"CURRENT_WINDOW_PAPER", "CORE_PAPER_WEEK"}
            )

    def test_incomplete_tail_is_excluded_and_reported(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            end_ns = HOURS_48_NS + 10_000_000_000
            self.fixture(root, end_ns)
            path = root / "var" / "research-5m" / "btc-fixture.jsonl"
            with path.open("ab") as handle:
                handle.write(b'{"k":"book"')
            output = export(root, root / "out", end_ns)
            manifest = json.loads((output / "manifest.json").read_text())
            evidence = next(
                item
                for item in manifest["sources"]
                if item["source_path"].endswith("btc-fixture.jsonl")
                and "research-5m" in item["source_path"]
            )
            self.assertGreater(evidence["trailing_incomplete_bytes"], 0)

    def test_conflicting_identity_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            end_ns = HOURS_48_NS + 10_000_000_000
            self.fixture(root, end_ns)
            path = root / "var" / "research-5m" / "btc-fixture.jsonl"
            conflict = {
                "k": "sys",
                "payload": {
                    "event_type": "market_identity",
                    "asset": "BTC",
                    "condition_id": "btc-5m",
                    "up_token_id": "different",
                    "down_token_id": "btc-5m-down",
                    "start_time_ms": (end_ns // 1_000_000) - 60_000,
                    "end_time_ms": (end_ns // 1_000_000) + 60_000,
                },
            }
            with path.open("a") as handle:
                handle.write(line(conflict))
            with self.assertRaises(ExportError):
                export(root, root / "out", end_ns)


if __name__ == "__main__":
    unittest.main()
