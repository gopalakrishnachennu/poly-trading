#!/usr/bin/env python3
"""Failure-oriented tests for the single-file BTC five-minute CSV merger."""
from __future__ import annotations

import csv
import hashlib
import json
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

from scripts import export_btc_5m_per_second as export_schema
from scripts import merge_btc_5m_per_second_csv as subject


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class SingleFileMergeTests(unittest.TestCase):
    def write_source(
        self,
        root: Path,
        *,
        window: str = "5m",
        window_ms: int = 300_000,
        seconds: int = 300,
    ) -> tuple[Path, Path]:
        source_root = root / "source"
        revision = "a" * 64
        dataset = source_root / revision
        partitions = []
        total_rows = 0
        for market_number, start in enumerate(
            (1_784_937_600_000, 1_784_941_200_000), start=1
        ):
            condition = "0x" + f"{market_number:064x}"
            partition = datetime.fromtimestamp(
                start / 1000, timezone.utc
            ).strftime("%Y-%m-%d/%H")
            folder = dataset / "BTC-data" / partition
            folder.mkdir(parents=True)
            csv_path = folder / "observations.csv"
            with csv_path.open("x", newline="", encoding="utf-8") as output:
                writer = csv.DictWriter(
                    output,
                    fieldnames=export_schema.CSV_COLUMNS,
                    lineterminator="\n",
                )
                writer.writeheader()
                for index in range(seconds):
                    row = {column: "" for column in export_schema.CSV_COLUMNS}
                    row.update(
                        {
                            "schema_version": "1",
                            "asset": "BTC",
                            "condition_id": condition,
                            "up_token_id": str(market_number * 2 - 1),
                            "down_token_id": str(market_number * 2),
                            "window_start_ms": str(start),
                            "window_end_ms": str(start + window_ms),
                            "second_index": str(index),
                            "second_start_ms": str(start + index * 1000),
                            "sample_time_ms": str(start + index * 1000 + 999),
                            "seconds_remaining": str(seconds - 1 - index),
                            "market_ready": "false",
                            "row_status": "MISSING_UP|MISSING_DOWN",
                            "quality_flags": "missing_up|missing_down",
                            "up_observed_this_second": "false",
                            "down_observed_this_second": "false",
                            "up_fresh": "false",
                            "down_fresh": "false",
                            "up_bids_json": "[]",
                            "up_asks_json": "[]",
                            "down_bids_json": "[]",
                            "down_asks_json": "[]",
                            "reference_ready": "false",
                        }
                    )
                    writer.writerow(row)
            entry = {
                "schema_version": 1,
                "asset": "BTC",
                "window": window,
                "utc_partition": partition,
                "rows": seconds,
                "ready_rows": 0,
                "observations_csv_sha256": sha256_file(csv_path),
                "derived_not_financial_authority": True,
            }
            (folder / "manifest.json").write_text(
                json.dumps(entry, sort_keys=True, separators=(",", ":")) + "\n"
            )
            partitions.append(entry)
            total_rows += seconds
        manifest = {
            "schema_version": 1,
            "asset": "BTC",
            "window": window,
            "revision": revision,
            "rows": total_rows,
            "expected_rows": total_rows,
            "markets": 2,
            "partitions": partitions,
            "derived_not_financial_authority": True,
        }
        manifest_path = dataset / "manifest.json"
        manifest_path.write_text(
            json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n"
        )
        latest = {
            "schema_version": 1,
            "revision": revision,
            "dataset_path": str(dataset.resolve()),
            "manifest_sha256": sha256_file(manifest_path),
        }
        latest_path = source_root / "latest.json"
        latest_path.write_text(
            json.dumps(latest, sort_keys=True, separators=(",", ":")) + "\n"
        )
        return source_root, latest_path

    def test_merges_and_reuses_verified_revision(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            source_root, latest = self.write_source(root)
            output_root = root / "merged"
            report = subject.merge(latest, output_root, source_root)
            reused = subject.merge(latest, output_root, source_root)
            self.assertEqual(report["rows"], 600)
            self.assertEqual(report["markets"], 2)
            self.assertEqual(report["partitions"], 2)
            self.assertTrue(reused["idempotent_reuse"])
            pointer = json.loads((output_root / "latest.json").read_text())
            with Path(pointer["csv_path"]).open(newline="", encoding="utf-8") as source:
                rows = list(csv.DictReader(source))
            self.assertEqual(len(rows), 600)
            self.assertEqual(rows[0]["second_index"], "0")
            self.assertEqual(rows[-1]["second_index"], "299")

    def test_partition_checksum_mismatch_fails_without_publication(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            source_root, latest = self.write_source(root)
            dataset = source_root / ("a" * 64)
            first_csv = sorted(dataset.glob("BTC-data/*/*/observations.csv"))[0]
            first_csv.write_text(first_csv.read_text() + "\n")
            output_root = root / "merged"
            with self.assertRaisesRegex(subject.InvalidMergeSource, "checksum"):
                subject.merge(latest, output_root, source_root)
            self.assertFalse((output_root / "latest.json").exists())

    def test_merges_fifteen_minute_source_with_900_rows_per_market(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            source_root, latest = self.write_source(
                root, window="15m", window_ms=900_000, seconds=900
            )
            report = subject.merge(latest, root / "merged", source_root)
            self.assertEqual(report["window"], "15m")
            self.assertEqual(report["rows"], 1800)
            self.assertEqual(report["seconds_per_market"], 900)

    def test_corrupt_existing_derivative_is_not_silently_reused(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            source_root, latest = self.write_source(root)
            output_root = root / "merged"
            subject.merge(latest, output_root, source_root)
            pointer = json.loads((output_root / "latest.json").read_text())
            csv_path = Path(pointer["csv_path"])
            csv_path.write_text(csv_path.read_text() + "\n")
            with self.assertRaisesRegex(
                subject.InvalidMergeSource, "differs from deterministic output"
            ):
                subject.merge(latest, output_root, source_root)

    def test_duplicate_condition_second_fails_without_publication(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            source_root, latest = self.write_source(root)
            dataset = source_root / ("a" * 64)
            paths = sorted(dataset.glob("BTC-data/*/*/observations.csv"))
            with paths[0].open(newline="", encoding="utf-8") as source:
                first_market_rows = list(csv.DictReader(source))
            with paths[1].open(newline="", encoding="utf-8") as source:
                second_market_rows = list(csv.DictReader(source))
            second_market_rows[0]["condition_id"] = first_market_rows[0]["condition_id"]
            with paths[1].open("w", newline="", encoding="utf-8") as output:
                writer = csv.DictWriter(
                    output,
                    fieldnames=export_schema.CSV_COLUMNS,
                    lineterminator="\n",
                )
                writer.writeheader()
                writer.writerows(second_market_rows)
            partition_manifest_path = paths[1].parent / "manifest.json"
            partition_manifest = json.loads(partition_manifest_path.read_text())
            partition_manifest["observations_csv_sha256"] = sha256_file(paths[1])
            partition_manifest_path.write_text(
                json.dumps(partition_manifest, sort_keys=True, separators=(",", ":"))
                + "\n"
            )
            root_manifest_path = dataset / "manifest.json"
            root_manifest = json.loads(root_manifest_path.read_text())
            root_manifest["partitions"][1] = partition_manifest
            root_manifest_path.write_text(
                json.dumps(root_manifest, sort_keys=True, separators=(",", ":")) + "\n"
            )
            latest_value = json.loads(latest.read_text())
            latest_value["manifest_sha256"] = sha256_file(root_manifest_path)
            latest.write_text(
                json.dumps(latest_value, sort_keys=True, separators=(",", ":")) + "\n"
            )
            with self.assertRaisesRegex(subject.InvalidMergeSource, "duplicate"):
                subject.merge(latest, root / "merged", source_root)
            self.assertFalse((root / "merged" / "latest.json").exists())

    def test_source_path_escape_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            root = Path(directory)
            source_root, latest = self.write_source(root)
            value = json.loads(latest.read_text())
            value["dataset_path"] = str((root / ("a" * 64)).resolve())
            latest.write_text(
                json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n"
            )
            with self.assertRaisesRegex(subject.InvalidMergeSource, "escapes"):
                subject.merge(latest, root / "merged", source_root)


if __name__ == "__main__":
    unittest.main()
