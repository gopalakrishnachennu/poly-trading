#!/usr/bin/env python3
"""Tests for chronological BTC research evidence merging."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq

from scripts import merge_btc_hourly_research_evidence as subject
from scripts.normalize_local_btc_hourly_capture import SPOT_SCHEMA, TRADE_SCHEMA


class EvidenceMergeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def evidence(self, name: str, timestamp: int, *, local: bool) -> Path:
        root = self.root / name
        root.mkdir()
        condition = f"{name}-condition"
        rows = [
            {
                "k": "sys",
                "payload": {
                    "event_type": "market_identity", "asset": "BTC",
                    "condition_id": condition, "start_time_ms": timestamp // 1_000_000,
                    "end_time_ms": timestamp // 1_000_000 + 3_600_000,
                    "up_token_id": f"{name}-up", "down_token_id": f"{name}-down",
                },
            },
            {
                "k": "book", "cond": condition, "tok": f"{name}-up",
                "seq": 1, "t": timestamp, "r": timestamp,
                "bids": [[400_000, 1_000_000]], "asks": [[500_000, 1_000_000]],
            },
        ]
        tape = root / "clob-l2.jsonl"
        tape.write_text("".join(json.dumps(row) + "\n" for row in rows))
        manifest = {
            "data_sha256": subject.sha256_file(tape),
            "schema_version": 1,
            "non_authoritative": True,
        }
        if local:
            pq.write_table(
                pa.Table.from_pylist(
                    [{"timestamp_ms": timestamp // 1_000_000, "price_micros": 80_000_000_000}],
                    schema=SPOT_SCHEMA,
                ),
                root / "spot.parquet",
            )
            pq.write_table(pa.Table.from_pylist([], schema=TRADE_SCHEMA), root / "trades.parquet")
            manifest.update({
                "source_time_semantics": "LOCAL_GATEWAY_POLL_OBSERVATION",
                "spot_sha256": subject.sha256_file(root / "spot.parquet"),
                "trades_sha256": subject.sha256_file(root / "trades.parquet"),
            })
        (root / "manifest.json").write_text(json.dumps(manifest))
        return root

    def auxiliary(self, timestamp: int) -> Path:
        root = self.root / "aux"
        root.mkdir()
        pq.write_table(
            pa.Table.from_pylist(
                [{"timestamp_ms": timestamp // 1_000_000, "price_micros": 79_000_000_000}],
                schema=SPOT_SCHEMA,
            ),
            root / "spot.parquet",
        )
        pq.write_table(pa.Table.from_pylist([], schema=TRADE_SCHEMA), root / "trades.parquet")
        (root / "manifest.json").write_text(json.dumps({
            "spot_sha256": subject.sha256_file(root / "spot.parquet"),
            "trades_sha256": subject.sha256_file(root / "trades.parquet"),
        }))
        return root

    def test_non_overlapping_sources_merge(self) -> None:
        historical = self.evidence("historical", 1_000_000_000, local=False)
        local = self.evidence("local", 2_000_000_000, local=True)
        result = subject.merge(
            historical, self.auxiliary(900_000_000), local, self.root / "out",
        )
        self.assertEqual(result["sessions"], 2)
        self.assertEqual(result["historical_books"], 1)
        self.assertEqual(result["local_books"], 1)

    def test_overlapping_or_regressed_sources_reject(self) -> None:
        historical = self.evidence("historical", 2_000_000_000, local=False)
        local = self.evidence("local", 1_000_000_000, local=True)
        with self.assertRaisesRegex(subject.InvalidEvidenceMerge, "overlap or regress"):
            subject.merge(
                historical, self.auxiliary(900_000_000), local, self.root / "out",
            )


if __name__ == "__main__":
    unittest.main()
