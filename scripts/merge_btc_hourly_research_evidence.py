#!/usr/bin/env python3
"""Merge non-overlapping, checksummed BTC-hourly research evidence."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any, BinaryIO

import pyarrow as pa
import pyarrow.parquet as pq

try:
    from scripts.normalize_local_btc_hourly_capture import canonical, sha256_file
except ModuleNotFoundError:
    from normalize_local_btc_hourly_capture import canonical, sha256_file


class InvalidEvidenceMerge(ValueError):
    """Research artifacts are incompatible or corrupt."""


def load_manifest(root: Path, *, local: bool) -> dict[str, Any]:
    if root.is_symlink() or not root.is_dir():
        raise InvalidEvidenceMerge("evidence root is missing or unsafe")
    manifest = json.loads((root / "manifest.json").read_text())
    tape = root / "clob-l2.jsonl"
    expected = manifest.get("data_sha256")
    if not isinstance(expected, str) or sha256_file(tape) != expected:
        raise InvalidEvidenceMerge("tape checksum mismatch")
    if local:
        for name in ("spot", "trades"):
            if sha256_file(root / f"{name}.parquet") != manifest.get(f"{name}_sha256"):
                raise InvalidEvidenceMerge(f"local {name} checksum mismatch")
        if manifest.get("source_time_semantics") != "LOCAL_GATEWAY_POLL_OBSERVATION":
            raise InvalidEvidenceMerge("local observation semantics mismatch")
    return manifest


def identity_key(row: dict[str, Any]) -> tuple[str, int, int, str, str] | None:
    if row.get("k") != "sys":
        return None
    payload = row.get("payload")
    if not isinstance(payload, dict) or payload.get("event_type") != "market_identity":
        return None
    if payload.get("asset") != "BTC":
        return None
    values = (
        payload.get("condition_id"),
        payload.get("start_time_ms"),
        payload.get("end_time_ms"),
        payload.get("up_token_id"),
        payload.get("down_token_id"),
    )
    if (
        not isinstance(values[0], str)
        or isinstance(values[1], bool) or not isinstance(values[1], int)
        or isinstance(values[2], bool) or not isinstance(values[2], int)
        or not isinstance(values[3], str)
        or not isinstance(values[4], str)
    ):
        raise InvalidEvidenceMerge("identity schema invalid")
    return values


def scan_tape(root: Path) -> tuple[dict[str, tuple[str, int, int, str, str]], int, int]:
    identities: dict[str, tuple[str, int, int, str, str]] = {}
    minimum: int | None = None
    maximum: int | None = None
    with (root / "clob-l2.jsonl").open() as source:
        for number, line in enumerate(source, 1):
            try:
                row = json.loads(line)
            except json.JSONDecodeError as error:
                raise InvalidEvidenceMerge(f"corrupt tape JSON at line {number}") from error
            identity = identity_key(row)
            if identity is not None:
                previous = identities.get(identity[0])
                if previous is not None and previous != identity:
                    raise InvalidEvidenceMerge("identity changes within tape")
                identities[identity[0]] = identity
                continue
            if row.get("k") != "book":
                continue
            timestamp = row.get("t")
            if isinstance(timestamp, bool) or not isinstance(timestamp, int) or timestamp <= 0:
                raise InvalidEvidenceMerge("book timestamp invalid")
            minimum = timestamp if minimum is None else min(minimum, timestamp)
            maximum = timestamp if maximum is None else max(maximum, timestamp)
    if not identities or minimum is None or maximum is None:
        raise InvalidEvidenceMerge("tape has no identities or books")
    return identities, minimum, maximum


def copy_books(root: Path, output: BinaryIO, digest: Any) -> int:
    count = 0
    with (root / "clob-l2.jsonl").open("rb") as source:
        for line in source:
            try:
                row = json.loads(line)
            except json.JSONDecodeError as error:
                raise InvalidEvidenceMerge("tape changed after checksum verification") from error
            if row.get("k") != "book":
                continue
            encoded = canonical(row) + b"\n"
            output.write(encoded)
            digest.update(encoded)
            count += 1
    return count


def merge_spot(historical: Path, local: Path) -> pa.Table:
    values: dict[int, int] = {}
    for root in (historical, local):
        for row in pq.read_table(root / "spot.parquet").to_pylist():
            timestamp = row.get("timestamp_ms")
            price = row.get("price_micros")
            if (
                isinstance(timestamp, bool) or not isinstance(timestamp, int)
                or isinstance(price, bool) or not isinstance(price, int) or price <= 0
            ):
                raise InvalidEvidenceMerge("spot row invalid")
            previous = values.get(timestamp)
            if previous is not None and previous != price:
                raise InvalidEvidenceMerge("spot sources conflict at same timestamp")
            values[timestamp] = price
    schema = pq.read_schema(historical / "spot.parquet")
    return pa.Table.from_pylist(
        [{"timestamp_ms": timestamp, "price_micros": price} for timestamp, price in sorted(values.items())],
        schema=schema,
    )


def merge_trades(historical: Path, local: Path) -> pa.Table:
    schema = pq.read_schema(historical / "trades.parquet")
    rows: dict[tuple[Any, ...], dict[str, Any]] = {}
    for root in (historical, local):
        table = pq.read_table(root / "trades.parquet")
        if table.schema != schema:
            raise InvalidEvidenceMerge("trade schemas differ")
        for row in table.to_pylist():
            key = (
                row["condition_id"], row["token_id"], row["timestamp_ms"],
                row["aggressor_side"], row["price_micros"],
                row["size_usdc_micros"], row["source"],
            )
            previous = rows.get(key)
            if previous is not None and previous != row:
                raise InvalidEvidenceMerge("duplicate trade identity conflicts")
            rows[key] = row
    ordered = sorted(
        rows.values(),
        key=lambda row: (
            row["timestamp_ms"], row["condition_id"], row["token_id"],
            row["aggressor_side"], row["price_micros"],
        ),
    )
    return pa.Table.from_pylist(ordered, schema=schema)


def merge(historical: Path, historical_auxiliary: Path, local: Path, out_root: Path) -> dict[str, Any]:
    if out_root.exists() or out_root.is_symlink():
        raise InvalidEvidenceMerge("merged output root must be new")
    historical_manifest = load_manifest(historical, local=False)
    historical_aux_manifest = json.loads((historical_auxiliary / "manifest.json").read_text())
    for name in ("spot", "trades"):
        if sha256_file(historical_auxiliary / f"{name}.parquet") != historical_aux_manifest.get(f"{name}_sha256"):
            raise InvalidEvidenceMerge(f"historical auxiliary {name} checksum mismatch")
    local_manifest = load_manifest(local, local=True)
    historical_identities, historical_min, historical_max = scan_tape(historical)
    local_identities, local_min, local_max = scan_tape(local)
    if historical_max >= local_min:
        raise InvalidEvidenceMerge("historical and local book intervals overlap or regress")
    combined = dict(historical_identities)
    for condition, identity in local_identities.items():
        previous = combined.get(condition)
        if previous is not None and previous != identity:
            raise InvalidEvidenceMerge("same condition has conflicting identity across sources")
        combined[condition] = identity
    out_root.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".combined-btc-hourly-", dir=out_root.parent) as temporary:
        staging = Path(temporary)
        tape = staging / "clob-l2.jsonl"
        digest = hashlib.sha256()
        with tape.open("xb") as output:
            for identity in sorted(combined.values(), key=lambda value: (value[1], value[0])):
                row = {
                    "k": "sys",
                    "payload": {
                        "event_type": "market_identity",
                        "asset": "BTC",
                        "condition_id": identity[0],
                        "start_time_ms": identity[1],
                        "end_time_ms": identity[2],
                        "up_token_id": identity[3],
                        "down_token_id": identity[4],
                    },
                }
                encoded = canonical(row) + b"\n"
                output.write(encoded)
                digest.update(encoded)
            historical_books = copy_books(historical, output, digest)
            local_books = copy_books(local, output, digest)
        spot = merge_spot(historical_auxiliary, local)
        trades = merge_trades(historical_auxiliary, local)
        spot_path = staging / "spot.parquet"
        trades_path = staging / "trades.parquet"
        pq.write_table(spot, spot_path, compression="zstd")
        pq.write_table(trades, trades_path, compression="zstd")
        out_root.mkdir(parents=False, exist_ok=False)
        for source in (tape, spot_path, trades_path):
            os.replace(source, out_root / source.name)
    manifest = {
        "schema_version": 1,
        "non_authoritative": True,
        "capability": "COMBINED_HISTORICAL_AND_LOCAL_TAKER_RESEARCH_ONLY",
        "source_id": "combined-historical-and-local-btc-hourly",
        "dataset_revision": hashlib.sha256(canonical({
            "historical": sha256_file(historical / "manifest.json"),
            "historical_auxiliary": sha256_file(historical_auxiliary / "manifest.json"),
            "local": sha256_file(local / "manifest.json"),
        })).hexdigest(),
        "spot_symbol": "btcusdt",
        "spot_source": "binance",
        "sessions": len(combined),
        "historical_sessions": len(historical_identities),
        "local_sessions": len(local_identities),
        "historical_books": historical_books,
        "local_books": local_books,
        "spot_rows": spot.num_rows,
        "trades": trades.num_rows,
        "start_event_ns": historical_min,
        "end_event_ns": local_max,
        "data_sha256": sha256_file(out_root / "clob-l2.jsonl"),
        "spot_sha256": sha256_file(out_root / "spot.parquet"),
        "trades_sha256": sha256_file(out_root / "trades.parquet"),
        "historical_manifest_sha256": sha256_file(historical / "manifest.json"),
        "historical_auxiliary_manifest_sha256": sha256_file(historical_auxiliary / "manifest.json"),
        "local_manifest_sha256": sha256_file(local / "manifest.json"),
    }
    (out_root / "manifest.json").write_bytes(canonical(manifest) + b"\n")
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--historical", required=True, type=Path)
    parser.add_argument("--historical-auxiliary", required=True, type=Path)
    parser.add_argument("--local", required=True, type=Path)
    parser.add_argument("--out-root", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        result = merge(
            arguments.historical, arguments.historical_auxiliary,
            arguments.local, arguments.out_root,
        )
        print(json.dumps(result, sort_keys=True))
        return 0
    except (InvalidEvidenceMerge, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"BTC evidence merge rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
