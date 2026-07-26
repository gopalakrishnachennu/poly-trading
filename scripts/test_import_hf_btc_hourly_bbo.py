#!/usr/bin/env python3
"""Failure-oriented tests for supplemental BTC-hourly BBO normalization."""
from __future__ import annotations

import importlib.util
import json
import tempfile
import types
import unittest
from datetime import UTC, datetime
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq


MODULE = Path(__file__).with_name("import_hf_btc_hourly_bbo.py")
SPEC = importlib.util.spec_from_file_location("bbo_importer", MODULE)
assert SPEC and SPEC.loader
bbo_importer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bbo_importer)

START = int(datetime(2026, 4, 11, 0, tzinfo=UTC).timestamp() * 1000)
CONDITION = "0x" + "a" * 64
UP = "1" * 50
DOWN = "2" * 50


class BboImportTests(unittest.TestCase):
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
                "asset": "BTC", "condition_id": CONDITION,
                "up_token_id": UP, "down_token_id": DOWN,
                "start_time_ms": START, "end_time_ms": START + 3_600_000,
                "rules_fingerprint": "rules",
            }],
        }))
        self.labels.write_text(json.dumps({
            "schema_version": 1,
            "official_evidence_sha256": evidence,
            "labels": [{
                "asset": "BTC", "market_id": "101", "condition_id": CONDITION,
                "start_time_ms": START, "end_time_ms": START + 3_600_000,
                "resolved_side": "UP",
            }],
        }))

    def tearDown(self) -> None:
        self.temp.cleanup()

    def source(self, *, bid: float = 0.4, ask: float = 0.41, token: str = UP, times: list[int] | None = None) -> Path:
        path = self.root / "bbo.parquet"
        times = times or [START * 1_000_000 + 1_000_000]
        count = len(times)
        pq.write_table(pa.Table.from_pydict({
            "ts_ms": [START + 1] * count,
            "market_id": ["101"] * count,
            "token_id": [token] * count,
            "outcome": ["Up"] * count,
            "best_bid": [bid] * count,
            "best_ask": [ask] * count,
            "best_bid_size": [100.0] * count,
            "best_ask_size": [100.0] * count,
            "local_recv_ts_ns": times,
        }), path)
        return path

    def policy(self, source: Path) -> dict:
        return {
            "maximum_bbo_bytes": 10_000_000,
            "bbo_sha256": bbo_importer.sha256_file(source),
            "maximum_bbo_rows": 100,
            "derived_bbo_spacing_ms": 1,
            "source_only_latency_reserve_ms": 2000,
            "maximum_derived_rows": 100,
            "source_id": "fixture",
            "dataset_repo": "fixture",
            "dataset_revision": "a" * 40,
            "license": "MIT",
            "bbo_path": "fixture.parquet",
        }

    def research(self) -> types.SimpleNamespace:
        return types.SimpleNamespace(minimum_completed_sessions=1)

    def test_valid_row_is_fixed_point_and_checksum_bound(self) -> None:
        source = self.source()
        report = bbo_importer.import_bbo(
            source, self.identity, self.labels, self.policy(source),
            self.research(), self.root / "out",
        )
        rows = [json.loads(line) for line in (self.root / "out" / "clob-l2.jsonl").read_text().splitlines()]
        book = next(row for row in rows if row["k"] == "book")
        self.assertEqual(book["bids"], [[400000, 100000000]])
        self.assertEqual(book["asks"], [[410000, 100000000]])
        self.assertEqual(report["covered_sessions"], 1)
        self.assertEqual(len(report["data_sha256"]), 64)

    def test_crossed_book_rejects_without_output(self) -> None:
        source = self.source(bid=0.42, ask=0.41)
        with self.assertRaisesRegex(bbo_importer.InvalidIdentity, "spread invalid"):
            bbo_importer.import_bbo(
                source, self.identity, self.labels, self.policy(source),
                self.research(), self.root / "out",
            )
        self.assertFalse((self.root / "out").exists())

    def test_token_outcome_mismatch_rejects(self) -> None:
        source = self.source(token=DOWN)
        with self.assertRaisesRegex(bbo_importer.InvalidIdentity, "mapping disagrees"):
            bbo_importer.import_bbo(
                source, self.identity, self.labels, self.policy(source),
                self.research(), self.root / "out",
            )

    def test_unordered_source_rows_are_deterministically_compacted(self) -> None:
        source = self.source(times=[START * 1_000_000 + 2_000_000, START * 1_000_000 + 1_000_000])
        report = bbo_importer.import_bbo(
            source, self.identity, self.labels, self.policy(source),
            self.research(), self.root / "out",
        )
        self.assertEqual(report["derived_rows"], 1)

    def test_latest_snapshot_mode_preserves_one_real_source_row(self) -> None:
        source = self.root / "bbo.parquet"
        pq.write_table(pa.Table.from_pydict({
            "ts_ms": [START + 1, START + 2],
            "market_id": ["101", "101"],
            "token_id": [UP, UP],
            "outcome": ["Up", "Up"],
            "best_bid": [0.40, 0.45],
            "best_ask": [0.41, 0.46],
            "best_bid_size": [100.0, 80.0],
            "best_ask_size": [90.0, 70.0],
            "local_recv_ts_ns": [None, None],
        }), source)
        policy = self.policy(source)
        policy["derived_bbo_spacing_ms"] = 1_000
        report = bbo_importer.import_bbo(
            source, self.identity, self.labels, policy,
            self.research(), self.root / "out",
            bbo_importer.LATEST_USABLE_SNAPSHOT,
        )
        books = [
            json.loads(line)
            for line in (self.root / "out" / "clob-l2.jsonl").read_text().splitlines()
            if json.loads(line)["k"] == "book"
        ]
        self.assertEqual(books[0]["bids"], [[450000, 80000000]])
        self.assertEqual(books[0]["asks"], [[460000, 70000000]])
        self.assertEqual(report["compaction_mode"], "latest-usable-snapshot")


if __name__ == "__main__":
    unittest.main()
