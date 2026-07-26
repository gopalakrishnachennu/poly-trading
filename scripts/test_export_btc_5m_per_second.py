#!/usr/bin/env python3
"""Failure-oriented tests for the BTC five-minute per-second exporter."""
from __future__ import annotations

import csv
import json
import tempfile
import unittest
from pathlib import Path

from scripts import export_btc_5m_per_second as subject


CONDITION = "0x" + "a" * 64
UP = "1" * 20
DOWN = "2" * 20
NEXT_CONDITION = "0x" + "b" * 64
NEXT_UP = "3" * 20
NEXT_DOWN = "4" * 20


class FiveMinutePerSecondExportTests(unittest.TestCase):
    def config(
        self, root: Path, *, window_ms: int = 300_000, seconds: int = 300
    ) -> Path:
        config = {
            "schema_version": 1,
            "asset": "BTC",
            "window_ms": window_ms,
            "seconds_per_window": seconds,
            "source_globs": ["capture/*.jsonl"],
            "reference_globs": ["reference/*.jsonl"],
            "output_root": "output",
            "source_blob_root": "source-blobs",
            "capture_availability_delay_ms": 0,
            "maximum_book_age_ms": 2000,
            "maximum_reference_age_ms": 2000,
            "maximum_reference_future_skew_ms": 500,
            "maximum_depth_levels": 10,
            "maximum_source_files": 10,
            "maximum_source_bytes": 1000000,
        }
        path = root / "config.json"
        path.write_text(json.dumps(config))
        return path

    def write_sources(
        self,
        root: Path,
        *,
        conflict: bool = False,
        window_ms: int = 300_000,
    ) -> None:
        capture = root / "capture"
        reference = root / "reference"
        capture.mkdir()
        reference.mkdir()
        start = 1_784_937_600_000
        identity = {
            "k": "sys",
            "payload": {
                "event_type": "market_identity",
                "asset": "BTC",
                "condition_id": CONDITION,
                "up_token_id": UP,
                "down_token_id": DOWN,
                "start_time_ms": start,
                "end_time_ms": start + window_ms,
            },
        }
        next_identity = {
            "k": "sys",
            "payload": {
                "event_type": "market_identity",
                "asset": "BTC",
                "condition_id": NEXT_CONDITION,
                "up_token_id": NEXT_UP,
                "down_token_id": NEXT_DOWN,
                "start_time_ms": start + window_ms,
                "end_time_ms": start + window_ms * 2,
            },
        }
        rows = [identity, identity, next_identity]
        if conflict:
            changed = json.loads(json.dumps(identity))
            changed["payload"]["down_token_id"] = "3" * 20
            rows.append(changed)
        for token, bid, ask in ((UP, 400000, 410000), (DOWN, 590000, 600000)):
            rows.append({
                "k": "book",
                "tok": token,
                "cond": CONDITION,
                "t": (start + 1000) * 1_000_000,
                "bids": [[bid, 1000000]],
                "asks": [[ask, 2000000]],
            })
        for token, bid, ask in (
            (NEXT_UP, 400000, 410000),
            (NEXT_DOWN, 590000, 600000),
        ):
            rows.append({
                "k": "book",
                "tok": token,
                "cond": NEXT_CONDITION,
                "t": (start + window_ms + 1000) * 1_000_000,
                "bids": [[bid, 1000000]],
                "asks": [[ask, 2000000]],
            })
        (capture / "btc-test.jsonl").write_text(
            "".join(json.dumps(row, separators=(",", ":")) + "\n" for row in rows)
        )
        reference_row = {
            "t_recv_ms": start + 1000,
            "t_event_ms": start + 900,
            "symbol": "BTCUSD",
            "bid": 64000.01,
            "ask": 64000.02,
            "mid": 64000.015,
            "src": "fixture",
        }
        (reference / "btcusd-test.jsonl").write_text(
            json.dumps(reference_row, separators=(",", ":")) + "\n"
        )

    def test_exact_grid_preserves_missing_and_stale_seconds(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            self.write_sources(root)
            config_path = self.config(root)
            original = subject.__file__
            try:
                subject.__file__ = str(root / "scripts" / "export.py")
                report = subject.export(config_path)
                reused = subject.export(config_path)
            finally:
                subject.__file__ = original
            self.assertEqual(report["rows"], 300)
            self.assertEqual(report["expected_rows"], 300)
            self.assertEqual(
                report["unfinished_conditions_excluded"], [NEXT_CONDITION]
            )
            latest = json.loads((root / "output" / "latest.json").read_text())
            csv_path = (
                Path(latest["dataset_path"])
                / "BTC-data"
                / "2026-07-25"
                / "00"
                / "observations.csv"
            )
            with csv_path.open(newline="") as source:
                rows = list(csv.DictReader(source))
            self.assertEqual(len(rows), 300)
            self.assertEqual(rows[0]["row_status"], "MISSING_UP|MISSING_DOWN")
            self.assertEqual(rows[1]["market_ready"], "true")
            self.assertEqual(rows[1]["buy_pair_cost_micros"], "1010000")
            self.assertEqual(rows[4]["market_ready"], "false")
            self.assertEqual(rows[-1]["second_index"], "299")
            self.assertTrue(reused["idempotent_reuse"])

    def test_conflicting_identity_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            self.write_sources(root, conflict=True)
            config_path = self.config(root)
            original = subject.__file__
            try:
                subject.__file__ = str(root / "scripts" / "export.py")
                with self.assertRaisesRegex(
                    subject.InvalidFiveMinuteCapture, "equivocation"
                ):
                    subject.export(config_path)
            finally:
                subject.__file__ = original

    def test_fifteen_minute_window_emits_exact_900_second_grid(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            self.write_sources(root, window_ms=900_000)
            config_path = self.config(root, window_ms=900_000, seconds=900)
            original = subject.__file__
            try:
                subject.__file__ = str(root / "scripts" / "export.py")
                report = subject.export(config_path)
            finally:
                subject.__file__ = original
            self.assertEqual(report["window"], "15m")
            self.assertEqual(report["rows"], 900)
            latest = json.loads((root / "output" / "latest.json").read_text())
            csv_path = (
                Path(latest["dataset_path"])
                / "BTC-data"
                / "2026-07-25"
                / "00"
                / "observations.csv"
            )
            with csv_path.open(newline="") as source:
                rows = list(csv.DictReader(source))
            self.assertEqual(len(rows), 900)
            self.assertEqual(rows[-1]["second_index"], "899")
            self.assertEqual(rows[-1]["seconds_remaining"], "0")

    def test_freeze_excludes_incomplete_tail(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            source = root / "active.jsonl"
            source.write_bytes(b'{"complete":1}\n{"partial":')
            evidence = subject.freeze_source(
                source, root / "blobs", maximum_bytes=1000
            )
            frozen = Path(evidence["frozen_path"])
            self.assertEqual(frozen.read_bytes(), b'{"complete":1}\n')
            self.assertEqual(
                evidence["excluded_incomplete_tail_bytes"], len(b'{"partial":')
            )

    def test_same_nanosecond_book_conflict_quarantines_condition(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            source = root / "capture.jsonl"
            start = 1_784_937_600_000
            identity = subject.Identity(CONDITION, UP, DOWN, start, start + 300000)
            rows = []
            for quantity in (1000000, 2000000):
                rows.append({
                    "k": "book",
                    "tok": UP,
                    "cond": CONDITION,
                    "t": (start + 1000) * 1_000_000,
                    "bids": [[400000, quantity]],
                    "asks": [[410000, quantity]],
                })
            source.write_text(
                "".join(
                    json.dumps(row, separators=(",", ":")) + "\n"
                    for row in rows
                )
            )
            books, counts, quarantined = subject.load_books(
                [source],
                {CONDITION: identity},
                {
                    "maximum_depth_levels": 10,
                    "capture_availability_delay_ms": 0,
                },
            )
            self.assertEqual(counts["same_time_equivocations"], 1)
            self.assertEqual(quarantined, {CONDITION})
            self.assertEqual(books, {})


if __name__ == "__main__":
    unittest.main()
