#!/usr/bin/env python3
"""Merge a sealed BTC per-second window export into one verified CSV.

The source revision remains immutable. This program verifies the latest pointer,
root manifest, partition manifests, partition checksums, schema, fixed-point
fields, configured timing grid and per-condition completeness before publishing
a separate content-addressed derived dataset.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import shutil
import tempfile
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

try:
    from scripts.export_btc_5m_per_second import CSV_COLUMNS
except ModuleNotFoundError:
    from export_btc_5m_per_second import CSV_COLUMNS


class InvalidMergeSource(RuntimeError):
    """The source export cannot safely produce a merged dataset."""


MERGER_SCHEMA_VERSION = 1
SOURCE_SCHEMA_VERSION = 1
EXPECTED_ASSET = "BTC"
SUPPORTED_WINDOWS = {
    "5m": (300_000, 300),
    "15m": (900_000, 900),
    "1h": (3_600_000, 3600),
}
MAXIMUM_CSV_FIELD_BYTES = 16 * 1024 * 1024

INTEGER_COLUMNS = {
    "schema_version",
    "window_start_ms",
    "window_end_ms",
    "second_index",
    "second_start_ms",
    "sample_time_ms",
    "seconds_remaining",
    "up_batch_start_ms",
    "down_batch_start_ms",
    "up_batch_start_ns",
    "down_batch_start_ns",
    "up_available_at_ms",
    "down_available_at_ms",
    "up_age_ms",
    "down_age_ms",
    "up_best_bid_micros",
    "up_best_ask_micros",
    "up_best_bid_quantity_micros",
    "up_best_ask_quantity_micros",
    "down_best_bid_micros",
    "down_best_ask_micros",
    "down_best_bid_quantity_micros",
    "down_best_ask_quantity_micros",
    "up_spread_micros",
    "down_spread_micros",
    "buy_pair_cost_micros",
    "sell_pair_proceeds_micros",
    "reference_event_time_ms",
    "reference_received_at_ms",
    "reference_age_ms",
    "reference_future_skew_ms",
    "btc_bid_micros",
    "btc_ask_micros",
    "btc_mid_micros",
}

BOOLEAN_COLUMNS = {
    "market_ready",
    "up_observed_this_second",
    "down_observed_this_second",
    "up_fresh",
    "down_fresh",
    "reference_ready",
}


def canonical(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def load_json_object(path: Path, description: str) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file():
        raise InvalidMergeSource(f"{description} is not a regular file")
    try:
        value = json.loads(path.read_text())
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise InvalidMergeSource(f"{description} is unreadable") from error
    if not isinstance(value, dict):
        raise InvalidMergeSource(f"{description} must be a JSON object")
    return value


def exact_nonnegative_int(value: Any, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise InvalidMergeSource(f"{name} must be a non-negative integer")
    return value


def valid_sha256(value: Any, name: str) -> str:
    if not isinstance(value, str) or len(value) != 64:
        raise InvalidMergeSource(f"{name} is not a SHA-256 digest")
    try:
        int(value, 16)
    except ValueError as error:
        raise InvalidMergeSource(f"{name} is not a SHA-256 digest") from error
    return value


def valid_revision(value: Any, name: str) -> str:
    return valid_sha256(value, name)


def atomic_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = canonical(value) + b"\n"
    temporary = path.parent / f".{path.name}.{os.getpid()}.tmp"
    with temporary.open("xb") as output:
        output.write(payload)
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)


def resolve_source(
    latest_path: Path, expected_source_root: Path | None
) -> tuple[dict[str, Any], Path, dict[str, Any], str]:
    latest_path = latest_path.resolve()
    latest = load_json_object(latest_path, "latest pointer")
    if set(latest) != {
        "schema_version",
        "revision",
        "dataset_path",
        "manifest_sha256",
    }:
        raise InvalidMergeSource("latest pointer keys are not exact")
    if latest["schema_version"] != SOURCE_SCHEMA_VERSION:
        raise InvalidMergeSource("latest pointer schema is unsupported")
    revision = valid_revision(latest["revision"], "latest revision")
    expected_manifest_digest = valid_sha256(
        latest["manifest_sha256"], "latest manifest checksum"
    )
    if not isinstance(latest["dataset_path"], str):
        raise InvalidMergeSource("latest dataset path is invalid")
    source_root = Path(latest["dataset_path"]).resolve()
    if source_root.name != revision:
        raise InvalidMergeSource("latest dataset path does not bind its revision")
    if expected_source_root is not None:
        expected_source_root = expected_source_root.resolve()
        try:
            relative = source_root.relative_to(expected_source_root)
        except ValueError as error:
            raise InvalidMergeSource("source revision escapes configured root") from error
        if relative.parts != (revision,):
            raise InvalidMergeSource("source revision is not an immediate root child")
    if source_root.is_symlink() or not source_root.is_dir():
        raise InvalidMergeSource("source revision is not a regular directory")
    manifest_path = source_root / "manifest.json"
    if sha256_file(manifest_path) != expected_manifest_digest:
        raise InvalidMergeSource("root manifest checksum does not match latest pointer")
    manifest = load_json_object(manifest_path, "root manifest")
    if (
        manifest.get("schema_version") != SOURCE_SCHEMA_VERSION
        or manifest.get("revision") != revision
        or manifest.get("asset") != EXPECTED_ASSET
        or manifest.get("derived_not_financial_authority") is not True
    ):
        raise InvalidMergeSource("root manifest identity is invalid")
    window = manifest.get("window")
    if window not in SUPPORTED_WINDOWS:
        raise InvalidMergeSource("root manifest window is unsupported")
    _, seconds_per_window = SUPPORTED_WINDOWS[window]
    rows = exact_nonnegative_int(manifest.get("rows"), "root rows")
    expected_rows = exact_nonnegative_int(
        manifest.get("expected_rows"), "root expected rows"
    )
    markets = exact_nonnegative_int(manifest.get("markets"), "root markets")
    if rows != expected_rows or rows != markets * seconds_per_window:
        raise InvalidMergeSource("root row conservation invariant failed")
    return latest, source_root, manifest, expected_manifest_digest


def validate_partition_entry(
    entry: Any, expected_window: str, seconds_per_window: int
) -> tuple[str, int, str]:
    if not isinstance(entry, dict):
        raise InvalidMergeSource("partition entry is not an object")
    expected_keys = {
        "schema_version",
        "asset",
        "window",
        "utc_partition",
        "rows",
        "ready_rows",
        "observations_csv_sha256",
        "derived_not_financial_authority",
    }
    if set(entry) != expected_keys:
        raise InvalidMergeSource("partition manifest keys are not exact")
    if (
        entry["schema_version"] != SOURCE_SCHEMA_VERSION
        or entry["asset"] != EXPECTED_ASSET
        or entry["window"] != expected_window
        or entry["derived_not_financial_authority"] is not True
    ):
        raise InvalidMergeSource("partition identity is invalid")
    partition = entry["utc_partition"]
    if not isinstance(partition, str):
        raise InvalidMergeSource("partition name is invalid")
    try:
        parsed = datetime.strptime(partition, "%Y-%m-%d/%H").replace(
            tzinfo=timezone.utc
        )
    except ValueError as error:
        raise InvalidMergeSource("partition name is not an exact UTC hour") from error
    if parsed.strftime("%Y-%m-%d/%H") != partition:
        raise InvalidMergeSource("partition name is not canonical")
    rows = exact_nonnegative_int(entry["rows"], "partition rows")
    ready_rows = exact_nonnegative_int(entry["ready_rows"], "partition ready rows")
    if ready_rows > rows or rows == 0 or rows % seconds_per_window != 0:
        raise InvalidMergeSource("partition row invariant failed")
    digest = valid_sha256(
        entry["observations_csv_sha256"], "partition CSV checksum"
    )
    return partition, rows, digest


def parse_integer_cell(row: dict[str, str], column: str, *, required: bool) -> int | None:
    value = row[column]
    if value == "":
        if required:
            raise InvalidMergeSource(f"required integer {column} is empty")
        return None
    if value.startswith("+") or value.strip() != value:
        raise InvalidMergeSource(f"{column} is not a canonical integer")
    try:
        parsed = int(value)
    except ValueError as error:
        raise InvalidMergeSource(f"{column} is not an integer") from error
    if str(parsed) != value:
        raise InvalidMergeSource(f"{column} is not a canonical integer")
    return parsed


def validate_condition_id(value: str) -> None:
    if len(value) != 66 or not value.startswith("0x"):
        raise InvalidMergeSource("condition_id is invalid")
    try:
        int(value[2:], 16)
    except ValueError as error:
        raise InvalidMergeSource("condition_id is invalid") from error


def validate_depth_json(value: str, name: str) -> None:
    if value == "":
        return
    try:
        levels = json.loads(value)
    except json.JSONDecodeError as error:
        raise InvalidMergeSource(f"{name} is invalid JSON") from error
    if not isinstance(levels, list):
        raise InvalidMergeSource(f"{name} is not an array")
    for level in levels:
        if (
            not isinstance(level, list)
            or len(level) != 2
            or any(isinstance(item, bool) or not isinstance(item, int) for item in level)
            or level[0] < 0
            or level[0] > 1_000_000
            or level[1] < 0
        ):
            raise InvalidMergeSource(f"{name} contains an invalid fixed-point level")


def validate_row(
    row: dict[str, str],
    partition: str,
    seen: set[tuple[str, int]],
    condition_indices: dict[str, set[int]],
    window_ms: int,
    seconds_per_window: int,
) -> None:
    if set(row) != set(CSV_COLUMNS) or None in row:
        raise InvalidMergeSource("CSV row does not match the canonical schema")
    if row["asset"] != EXPECTED_ASSET or row["schema_version"] != "1":
        raise InvalidMergeSource("CSV row identity is invalid")
    validate_condition_id(row["condition_id"])
    for name in BOOLEAN_COLUMNS:
        if row[name] not in {"true", "false"}:
            raise InvalidMergeSource(f"{name} is not canonical boolean text")
    required_integer_columns = {
        "schema_version",
        "window_start_ms",
        "window_end_ms",
        "second_index",
        "second_start_ms",
        "sample_time_ms",
        "seconds_remaining",
    }
    parsed = {
        name: parse_integer_cell(
            row, name, required=name in required_integer_columns
        )
        for name in INTEGER_COLUMNS
    }
    start = parsed["window_start_ms"]
    end = parsed["window_end_ms"]
    index = parsed["second_index"]
    second_start = parsed["second_start_ms"]
    sample_time = parsed["sample_time_ms"]
    seconds_remaining = parsed["seconds_remaining"]
    assert start is not None and end is not None and index is not None
    assert second_start is not None and sample_time is not None
    assert seconds_remaining is not None
    if (
        end - start != window_ms
        or not 0 <= index < seconds_per_window
        or second_start != start + index * 1000
        or sample_time != second_start + 999
        or seconds_remaining != seconds_per_window - 1 - index
    ):
        raise InvalidMergeSource("CSV row violates the exact configured grid")
    derived_partition = datetime.fromtimestamp(
        start / 1000, timezone.utc
    ).strftime("%Y-%m-%d/%H")
    if derived_partition != partition:
        raise InvalidMergeSource("CSV row is stored in the wrong UTC partition")
    key = (row["condition_id"], index)
    if key in seen:
        raise InvalidMergeSource("duplicate condition/second row")
    seen.add(key)
    condition_indices[row["condition_id"]].add(index)
    for name in (
        "up_bids_json",
        "up_asks_json",
        "down_bids_json",
        "down_asks_json",
    ):
        validate_depth_json(row[name], name)


def merge(
    latest_path: Path,
    output_root: Path,
    expected_source_root: Path | None = None,
) -> dict[str, Any]:
    latest, source_root, source_manifest, source_manifest_digest = resolve_source(
        latest_path, expected_source_root
    )
    window = source_manifest["window"]
    window_ms, seconds_per_window = SUPPORTED_WINDOWS[window]
    partitions_raw = source_manifest.get("partitions")
    if not isinstance(partitions_raw, list) or not partitions_raw:
        raise InvalidMergeSource("root manifest has no partitions")
    validated_partitions = [
        validate_partition_entry(item, window, seconds_per_window)
        for item in partitions_raw
    ]
    partition_names = [item[0] for item in validated_partitions]
    if partition_names != sorted(partition_names) or len(set(partition_names)) != len(
        partition_names
    ):
        raise InvalidMergeSource("partitions are not unique canonical order")
    if sum(item[1] for item in validated_partitions) != source_manifest["rows"]:
        raise InvalidMergeSource("partition rows do not reconcile to root rows")

    output_root = output_root.resolve()
    output_root.mkdir(parents=True, exist_ok=True)
    revision_basis = {
        "merger_schema_version": MERGER_SCHEMA_VERSION,
        "source_revision": latest["revision"],
        "source_manifest_sha256": source_manifest_digest,
        "window": window,
        "window_ms": window_ms,
        "seconds_per_window": seconds_per_window,
        "csv_columns": list(CSV_COLUMNS),
    }
    revision = hashlib.sha256(canonical(revision_basis)).hexdigest()
    final_root = output_root / revision
    final_csv = final_root / f"btc-{window}-per-second.csv"
    final_manifest = final_root / "manifest.json"

    staging = Path(
        tempfile.mkdtemp(prefix=f".{revision}.", dir=str(output_root))
    )
    staged_csv = staging / final_csv.name
    seen: set[tuple[str, int]] = set()
    condition_indices: dict[str, set[int]] = defaultdict(set)
    rows_written = 0
    try:
        csv.field_size_limit(MAXIMUM_CSV_FIELD_BYTES)
        with staged_csv.open("x", newline="", encoding="utf-8") as output:
            writer = csv.DictWriter(
                output, fieldnames=CSV_COLUMNS, lineterminator="\n"
            )
            writer.writeheader()
            for entry, (partition, expected_rows, expected_digest) in zip(
                partitions_raw, validated_partitions, strict=True
            ):
                csv_path = source_root / "BTC-data" / partition / "observations.csv"
                partition_manifest_path = csv_path.parent / "manifest.json"
                local_manifest = load_json_object(
                    partition_manifest_path, f"partition manifest {partition}"
                )
                if local_manifest != entry:
                    raise InvalidMergeSource(
                        f"partition manifest {partition} differs from root"
                    )
                if csv_path.is_symlink() or not csv_path.is_file():
                    raise InvalidMergeSource(
                        f"partition CSV {partition} is not a regular file"
                    )
                if sha256_file(csv_path) != expected_digest:
                    raise InvalidMergeSource(
                        f"partition CSV checksum failed for {partition}"
                    )
                partition_rows = 0
                with csv_path.open(newline="", encoding="utf-8") as source:
                    reader = csv.DictReader(source)
                    if tuple(reader.fieldnames or ()) != CSV_COLUMNS:
                        raise InvalidMergeSource(
                            f"partition CSV schema failed for {partition}"
                        )
                    for row in reader:
                        validate_row(
                            row,
                            partition,
                            seen,
                            condition_indices,
                            window_ms,
                            seconds_per_window,
                        )
                        writer.writerow(row)
                        partition_rows += 1
                        rows_written += 1
                if partition_rows != expected_rows:
                    raise InvalidMergeSource(
                        f"partition row count failed for {partition}"
                    )
            output.flush()
            os.fsync(output.fileno())

        expected_indices = set(range(seconds_per_window))
        incomplete = [
            condition
            for condition, indices in condition_indices.items()
            if indices != expected_indices
        ]
        if incomplete:
            raise InvalidMergeSource(
                "one or more conditions lack the exact configured rows"
            )
        if (
            rows_written != source_manifest["rows"]
            or len(condition_indices) != source_manifest["markets"]
        ):
            raise InvalidMergeSource("merged row/market conservation invariant failed")

        csv_digest = sha256_file(staged_csv)
        manifest = {
            "schema_version": MERGER_SCHEMA_VERSION,
            "asset": EXPECTED_ASSET,
            "window": window,
            "revision": revision,
            "source_revision": latest["revision"],
            "source_dataset_path": str(source_root),
            "source_manifest_sha256": source_manifest_digest,
            "csv_file": final_csv.name,
            "csv_sha256": csv_digest,
            "rows": rows_written,
            "markets": len(condition_indices),
            "partitions": len(validated_partitions),
            "seconds_per_market": seconds_per_window,
            "derived_not_financial_authority": True,
        }
        atomic_json(staging / "manifest.json", manifest)
        if final_root.exists():
            existing_manifest = load_json_object(
                final_manifest, "existing merged manifest"
            )
            if (
                existing_manifest != manifest
                or final_csv.is_symlink()
                or not final_csv.is_file()
                or sha256_file(final_csv) != csv_digest
            ):
                raise InvalidMergeSource(
                    "existing merged revision differs from deterministic output"
                )
            shutil.rmtree(staging)
            manifest["idempotent_reuse"] = True
        else:
            os.replace(staging, final_root)
        atomic_json(
            output_root / "latest.json",
            {
                "schema_version": MERGER_SCHEMA_VERSION,
                "revision": revision,
                "dataset_path": str(final_root),
                "csv_path": str(final_csv),
                "manifest_sha256": sha256_file(final_manifest),
            },
        )
        return manifest
    except Exception:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source-latest",
        type=Path,
        default=Path("var/research-export/BTC-5m-per-second/latest.json"),
    )
    parser.add_argument(
        "--source-root",
        type=Path,
        default=Path("var/research-export/BTC-5m-per-second"),
    )
    parser.add_argument(
        "--output-root",
        type=Path,
        default=Path("var/research-export/BTC-5m-per-second-merged"),
    )
    arguments = parser.parse_args()
    try:
        report = merge(
            arguments.source_latest,
            arguments.output_root,
            arguments.source_root,
        )
        print(
            json.dumps(
                {
                    key: report[key]
                    for key in (
                        "revision",
                        "source_revision",
                        "rows",
                        "markets",
                        "partitions",
                        "csv_sha256",
                    )
                },
                sort_keys=True,
            )
        )
        return 0
    except (OSError, ValueError, InvalidMergeSource) as error:
        print(json.dumps({"error": str(error)}, sort_keys=True))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
