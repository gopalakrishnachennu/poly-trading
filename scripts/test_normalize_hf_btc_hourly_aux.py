#!/usr/bin/env python3
"""Failure-oriented tests for BTC-hourly spot and trade-flow normalization."""
from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from datetime import UTC, datetime
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq


MODULE = Path(__file__).with_name("normalize_hf_btc_hourly_aux.py")
SPEC = importlib.util.spec_from_file_location("aux_normalizer", MODULE)
assert SPEC and SPEC.loader
normalizer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(normalizer)

START = int(datetime(2026, 4, 11, 0, tzinfo=UTC).timestamp() * 1000)
CONDITION = "0x" + "a" * 64
UP = "1" * 50
DOWN = "2" * 50


class AuxiliaryNormalizationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.identity = self.root / "identities.json"
        self.labels = self.root / "labels.json"
        evidence = "e" * 64
        self.identity.write_text(json.dumps({
            "schema_version": 2,
            "source": "official-polymarket",
            "sources": [{"sha256": evidence}],
            "assets": [{
                "asset": "BTC",
                "condition_id": CONDITION,
                "up_token_id": UP,
                "down_token_id": DOWN,
                "start_time_ms": START,
                "end_time_ms": START + 3_600_000,
                "rules_fingerprint": "rules",
            }],
        }))
        self.labels.write_text(json.dumps({
            "schema_version": 1,
            "official_evidence_sha256": evidence,
            "labels": [{
                "asset": "BTC",
                "market_id": "101",
                "condition_id": CONDITION,
                "start_time_ms": START,
                "end_time_ms": START + 3_600_000,
                "resolved_side": "UP",
            }],
        }))

    def tearDown(self) -> None:
        self.temp.cleanup()

    def sources(
        self,
        *,
        outcome: str = "Up",
        side: str = "BUY",
        source: str = "onchain",
        price: float = 0.41,
        token: str = UP,
    ) -> tuple[Path, Path]:
        ticks = self.root / "ticks.parquet"
        pq.write_table(pa.Table.from_pydict({
            "market_id": ["101"],
            "timestamp_ms": [START + 1_000],
            "token_id": [token],
            "outcome": [outcome],
            "side": [side],
            "price": [price],
            "size_usdc": [12.5],
            "tx_hash": ["0x" + "b" * 64],
            "block_number": [123],
            "log_index": [4],
            "source": [source],
            "spot_price_usdt": [None],
            "spot_price_ts_ms": [None],
            "local_recv_ts_ns": [None],
        }), ticks)
        spot = self.root / "spot.parquet"
        pq.write_table(pa.Table.from_pydict({
            "ts_ms": [START - 1_000, START + 1_000, START + 1_000],
            "symbol": ["btcusdt", "btcusdt", "btc/usd"],
            "price": [84_000.125, 84_001.5, 99_999.0],
            "source": ["binance", "binance", "chainlink"],
        }), spot)
        return ticks, spot

    def policy(self, ticks: Path, spot: Path) -> dict:
        return {
            "maximum_ticks_bytes": 10_000_000,
            "ticks_sha256": normalizer.sha256_file(ticks),
            "maximum_ticks_rows": 100,
            "maximum_spot_bytes": 10_000_000,
            "spot_sha256": normalizer.sha256_file(spot),
            "maximum_spot_rows": 100,
            "source_id": "fixture",
            "dataset_revision": "a" * 40,
        }

    def test_valid_sources_publish_fixed_point_and_filter_other_spot(self) -> None:
        ticks, spot = self.sources()
        out = self.root / "out"
        result = normalizer.normalize(
            self.policy(ticks, spot), ticks, spot, self.identity, self.labels, out,
        )
        trade = pq.read_table(out / "trades.parquet").to_pylist()[0]
        prices = pq.read_table(out / "spot.parquet").to_pylist()
        self.assertEqual(trade["price_micros"], 410_000)
        self.assertEqual(trade["size_usdc_micros"], 12_500_000)
        self.assertEqual([row["price_micros"] for row in prices], [84_000_125_000, 84_001_500_000])
        self.assertEqual(result["trades"], 1)
        self.assertEqual(result["spot_rows"], 2)

    def test_token_outcome_mismatch_rejects_without_output(self) -> None:
        ticks, spot = self.sources(token=DOWN, outcome="Up")
        out = self.root / "out"
        with self.assertRaisesRegex(normalizer.InvalidIdentity, "mapping disagrees"):
            normalizer.normalize(
                self.policy(ticks, spot), ticks, spot, self.identity, self.labels, out,
            )
        self.assertFalse(out.exists())

    def test_invalid_side_rejects(self) -> None:
        ticks, spot = self.sources(side="HOLD")
        with self.assertRaisesRegex(normalizer.InvalidIdentity, "side invalid"):
            normalizer.normalize(
                self.policy(ticks, spot), ticks, spot, self.identity, self.labels,
                self.root / "out",
            )

    def test_checksum_mismatch_rejects(self) -> None:
        ticks, spot = self.sources()
        policy = self.policy(ticks, spot)
        policy["ticks_sha256"] = "0" * 64
        with self.assertRaisesRegex(normalizer.InvalidIdentity, "checksum mismatch"):
            normalizer.normalize(
                policy, ticks, spot, self.identity, self.labels, self.root / "out",
            )


if __name__ == "__main__":
    unittest.main()
