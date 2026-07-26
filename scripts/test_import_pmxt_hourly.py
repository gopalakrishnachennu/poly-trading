#!/usr/bin/env python3
"""Regression tests for the bounded PMXT external-data boundary."""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from datetime import UTC, datetime
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq


ROOT = Path(__file__).resolve().parents[1]
IMPORTER = ROOT / "scripts" / "import_pmxt_hourly.py"


class PmxtImportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.policy = self.root / "policy.json"
        self.identities = self.root / "identities.json"
        self.policy.write_text(json.dumps({"schema_version": 1, "source_id": "pmxt-v2", "source_url_template": "https://r2v2.pmxt.dev/polymarket_orderbook_YYYY-MM-DDTHH.parquet", "license": "CC-BY-4.0", "maximum_input_bytes": 10_000_000, "maximum_rows_per_file": 100, "allowed_event_types": ["book", "price_change", "last_trade_price", "tick_size_change"]}))
        self.condition = "0x" + "a" * 64
        self.token = "123456789"
        self.identities.write_text(json.dumps({"schema_version": 1, "source": "official-polymarket", "assets": [{"asset": "BTC", "condition_id": self.condition, "up_token_id": self.token, "down_token_id": "987654321", "start_time_ms": 1_780_000_000_000, "end_time_ms": 1_780_003_600_000, "rules_fingerprint": "rules"}]}))

    def tearDown(self) -> None:
        self.temp.cleanup()

    def fixture(self, *, token: str | None = None, received_before_source: bool = False) -> Path:
        source = self.root / "fixture.parquet"
        instant = datetime(2026, 6, 2, 12, 0, tzinfo=UTC)
        received = datetime(2026, 6, 2, 12, 0, 1 if not received_before_source else 0, tzinfo=UTC)
        rows = {"timestamp_received": [received], "timestamp": [instant], "market": [self.condition], "event_type": ["book"], "asset_id": [token or self.token], "bids": ['[["0.40","10"]]'], "asks": ['[["0.41","10"]]'], "price": [None], "size": [None], "side": [None], "best_bid": [None], "best_ask": [None], "fee_rate_bps": [None], "transaction_hash": [None], "old_tick_size": [None], "new_tick_size": [None]}
        pq.write_table(pa.Table.from_pydict(rows), source)
        return source

    def run_import(self, source: Path, out: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run([sys.executable, str(IMPORTER), "--policy", str(self.policy), "--identities", str(self.identities), "--source", str(source), "--out-root", str(out)], text=True, capture_output=True, check=False)

    def test_valid_allowlisted_event_creates_checksums(self) -> None:
        result = self.run_import(self.fixture(), self.root / "out")
        self.assertEqual(result.returncode, 0, result.stderr)
        part = self.root / "out" / "BTC-data" / "2026-06-02" / "12"
        record = json.loads((part / "pmxt-events.jsonl").read_text())
        self.assertEqual(record["condition_id"], self.condition)
        self.assertEqual(record["price"], None)
        manifest = json.loads((part / "manifest.json").read_text())
        self.assertEqual(manifest["source_id"], "pmxt-v2")
        self.assertTrue(manifest["data_sha256"])

    def test_unknown_token_is_not_imported(self) -> None:
        result = self.run_import(self.fixture(token="111111111"), self.root / "out")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.root / "out").exists())

    def test_provenance_bound_v2_manifest_is_accepted(self) -> None:
        self.identities.write_text(json.dumps({
            "schema_version": 2, "source": "official-polymarket", "source_id": "official-polymarket-gamma",
            "policy_sha256": "a" * 64, "requested_start_time_ms": 1_780_005_600_000,
            "requested_end_time_ms": 1_780_009_200_000,
            "sources": [{"kind": "operator-supplied", "name": "official.json", "sha256": "b" * 64}],
            "assets": [{"asset": "BTC", "condition_id": self.condition, "up_token_id": self.token, "down_token_id": "987654321", "start_time_ms": 1_780_000_000_000, "end_time_ms": 1_780_003_600_000, "rules_fingerprint": "rules"}],
        }))
        result = self.run_import(self.fixture(), self.root / "out")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_bad_timestamp_order_is_rejected_without_output(self) -> None:
        source = self.fixture(received_before_source=True)
        # Make source one second after receipt, which violates the archive contract.
        table = pq.read_table(source).set_column(1, "timestamp", pa.array([datetime(2026, 6, 2, 12, 0, 1, tzinfo=UTC)]))
        pq.write_table(table, source)
        result = self.run_import(source, self.root / "out")
        self.assertEqual(result.returncode, 2)
        self.assertFalse((self.root / "out").exists())


if __name__ == "__main__":
    unittest.main()
