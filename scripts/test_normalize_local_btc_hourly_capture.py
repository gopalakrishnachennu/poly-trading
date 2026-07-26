#!/usr/bin/env python3
"""Failure-oriented tests for local BTC-hourly research normalization."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import pyarrow.parquet as pq

from scripts import normalize_local_btc_hourly_capture as subject


class LocalCaptureNormalizationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.config = self.root / "config"
        self.config.mkdir()
        self.book = self.root / "book.jsonl"
        self.reference = self.root / "reference.jsonl"
        self.start = 1_800_000_000_000
        self.condition = "condition"
        self.up = "up"
        self.down = "down"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def identity(self) -> dict:
        return {
            "k": "sys",
            "payload": {
                "event_type": "market_identity",
                "asset": "BTC",
                "condition_id": self.condition,
                "up_token_id": self.up,
                "down_token_id": self.down,
                "start_time_ms": self.start,
                "end_time_ms": self.start + 3_600_000,
            },
        }

    def write_sources(self, *, include_down: bool = True, crossed: bool = False) -> Path:
        timestamp = (self.start + 3_600_000) * 1_000_000
        rows = [self.identity()]
        tokens = (self.up, self.down) if include_down else (self.up,)
        for token in tokens:
            rows.append({
                "k": "book", "cond": self.condition, "tok": token, "t": timestamp,
                "bids": [[500_000 if crossed else 490_000, 20_000_000]],
                "asks": [[500_000, 20_000_000]],
            })
        self.book.write_text("".join(json.dumps(row) + "\n" for row in rows))
        record = {
            "record": {
                "event_time_ms": self.start + 1_000,
                "payload": {
                    "asset": "BTC", "condition_id": self.condition,
                    "reference_price_micros": "84000125000",
                    "target_price_micros": "84000000000",
                },
            },
        }
        self.reference.write_text(json.dumps(record) + "\n")
        policy = {
            "schema_version": 1,
            "asset": "BTC",
            "window_ms": 3_600_000,
            "maximum_book_levels": 10,
            "source_time_semantics": "LOCAL_GATEWAY_POLL_OBSERVATION",
            "reference_source": "binance",
            "book_sources": [{
                "path": str(self.book),
                "sha256": subject.sha256_file(self.book),
                "maximum_bytes": 100_000,
                "maximum_rows": 10,
            }],
            "reference_sources": [{
                "path": str(self.reference),
                "sha256": subject.sha256_file(self.reference),
                "maximum_bytes": 100_000,
                "maximum_rows": 10,
            }],
        }
        path = self.config / "policy.json"
        path.write_text(json.dumps(policy))
        return path

    def test_complete_atomic_pair_and_spot_are_published(self) -> None:
        policy = self.write_sources()
        out = self.root / "out"
        manifest = subject.normalize(policy, out)
        rows = [json.loads(line) for line in (out / "clob-l2.jsonl").read_text().splitlines()]
        books = [row for row in rows if row["k"] == "book"]
        self.assertEqual(len(books), 2)
        self.assertTrue(all(row["r"] == row["t"] for row in books))
        self.assertEqual(manifest["source_time_semantics"], "LOCAL_GATEWAY_POLL_OBSERVATION")
        self.assertEqual(
            pq.read_table(out / "spot.parquet").to_pylist(),
            [{"timestamp_ms": self.start + 1_000, "price_micros": 84_000_125_000}],
        )

    def test_partial_pair_is_not_published(self) -> None:
        policy = self.write_sources(include_down=False)
        with self.assertRaisesRegex(subject.InvalidLocalCapture, "no complete atomic"):
            subject.normalize(policy, self.root / "out")

    def test_crossed_book_rejects(self) -> None:
        policy = self.write_sources(crossed=True)
        with self.assertRaisesRegex(subject.InvalidLocalCapture, "crossed or locked"):
            subject.normalize(policy, self.root / "out")

    def test_changed_source_rejects_checksum(self) -> None:
        policy = self.write_sources()
        self.book.write_text(self.book.read_text() + "{}\n")
        with self.assertRaisesRegex(subject.InvalidLocalCapture, "checksum mismatch"):
            subject.normalize(policy, self.root / "out")


if __name__ == "__main__":
    unittest.main()
