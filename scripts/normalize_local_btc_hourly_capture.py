#!/usr/bin/env python3
"""Normalize checksum-bound local BTC-hourly observations for research only."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pyarrow as pa
import pyarrow.parquet as pq

MICROS = 1_000_000

SPOT_SCHEMA = pa.schema([
    ("timestamp_ms", pa.int64()),
    ("price_micros", pa.int64()),
])
TRADE_SCHEMA = pa.schema([
    ("timestamp_ms", pa.int64()),
    ("local_receive_time_ns", pa.int64()),
    ("market_id", pa.string()),
    ("condition_id", pa.string()),
    ("token_id", pa.string()),
    ("outcome", pa.string()),
    ("aggressor_side", pa.string()),
    ("price_micros", pa.int64()),
    ("size_usdc_micros", pa.int64()),
    ("source", pa.string()),
])


class InvalidLocalCapture(ValueError):
    """The local evidence cannot safely enter research."""


@dataclass(frozen=True)
class Market:
    condition_id: str
    start_ms: int
    end_ms: int
    up_token: str
    down_token: str


def canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def integer(value: Any, field: str, *, positive: bool = False) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise InvalidLocalCapture(f"{field} must be an integer")
    if positive and value <= 0:
        raise InvalidLocalCapture(f"{field} must be positive")
    return value


def read_policy(path: Path) -> dict[str, Any]:
    raw = json.loads(path.read_text())
    expected = {
        "schema_version", "asset", "window_ms", "maximum_book_levels",
        "book_sources", "reference_sources", "source_time_semantics",
        "reference_source",
    }
    if set(raw) != expected or raw["schema_version"] != 1 or raw["asset"] != "BTC":
        raise InvalidLocalCapture("local normalization policy schema invalid")
    if raw["window_ms"] != 3_600_000:
        raise InvalidLocalCapture("only exact one-hour windows are eligible")
    integer(raw["maximum_book_levels"], "maximum_book_levels", positive=True)
    if raw["source_time_semantics"] != "LOCAL_GATEWAY_POLL_OBSERVATION":
        raise InvalidLocalCapture("source time semantics are not eligible")
    if raw["reference_source"] != "binance":
        raise InvalidLocalCapture("reference source must be binance")
    for group in ("book_sources", "reference_sources"):
        sources = raw[group]
        if not isinstance(sources, list) or not sources:
            raise InvalidLocalCapture(f"{group} must be non-empty")
        for source in sources:
            if set(source) != {"path", "sha256", "maximum_bytes", "maximum_rows"}:
                raise InvalidLocalCapture(f"{group} source schema invalid")
            if not isinstance(source["path"], str) or not source["path"]:
                raise InvalidLocalCapture("source path invalid")
            if (
                not isinstance(source["sha256"], str)
                or len(source["sha256"]) != 64
                or any(character not in "0123456789abcdef" for character in source["sha256"])
            ):
                raise InvalidLocalCapture("source checksum invalid")
            integer(source["maximum_bytes"], "maximum_bytes", positive=True)
            integer(source["maximum_rows"], "maximum_rows", positive=True)
    return raw


def resolve_source(policy_path: Path, source: dict[str, Any]) -> Path:
    candidate = Path(source["path"])
    path = candidate if candidate.is_absolute() else policy_path.parent.parent / candidate
    path = path.resolve()
    if path.is_symlink() or not path.is_file():
        raise InvalidLocalCapture(f"source is missing or unsafe: {candidate}")
    if path.stat().st_size > source["maximum_bytes"]:
        raise InvalidLocalCapture(f"source exceeds byte bound: {candidate}")
    if sha256_file(path) != source["sha256"]:
        raise InvalidLocalCapture(f"source checksum mismatch: {candidate}")
    return path


def parse_identity(payload: Any, window_ms: int) -> Market | None:
    if not isinstance(payload, dict) or payload.get("event_type") != "market_identity":
        return None
    if payload.get("asset") != "BTC":
        return None
    values = (
        payload.get("condition_id"),
        payload.get("up_token_id"),
        payload.get("down_token_id"),
    )
    if not all(isinstance(value, str) and value for value in values):
        raise InvalidLocalCapture("BTC identity is incomplete")
    start = integer(payload.get("start_time_ms"), "start_time_ms")
    end = integer(payload.get("end_time_ms"), "end_time_ms")
    if end - start != window_ms:
        raise InvalidLocalCapture("BTC identity is not an exact one-hour window")
    return Market(values[0], start, end, values[1], values[2])


def validate_levels(levels: Any, *, bids: bool, maximum: int) -> tuple[tuple[int, int], ...]:
    if not isinstance(levels, list) or not levels or len(levels) > maximum:
        raise InvalidLocalCapture("book depth count invalid")
    result: list[tuple[int, int]] = []
    previous: int | None = None
    for level in levels:
        if not isinstance(level, list) or len(level) != 2:
            raise InvalidLocalCapture("book level shape invalid")
        price = integer(level[0], "book price", positive=True)
        size = integer(level[1], "book size", positive=True)
        if price >= MICROS:
            raise InvalidLocalCapture("book price outside binary-token bounds")
        if previous is not None and ((bids and price >= previous) or (not bids and price <= previous)):
            raise InvalidLocalCapture("book levels are unsorted or duplicated")
        previous = price
        result.append((price, size))
    return tuple(result)


def load_books(
    policy_path: Path,
    policy: dict[str, Any],
) -> tuple[dict[str, Market], list[dict[str, Any]], dict[str, int]]:
    markets: dict[str, Market] = {}
    tokens: dict[str, tuple[str, str]] = {}
    observations: dict[tuple[str, int, str], tuple[tuple[tuple[int, int], ...], tuple[tuple[int, int], ...]]] = {}
    rows_read = duplicates = outside = 0
    for source in policy["book_sources"]:
        path = resolve_source(policy_path, source)
        source_rows = 0
        with path.open() as handle:
            for line_number, line in enumerate(handle, 1):
                source_rows += 1
                rows_read += 1
                if source_rows > source["maximum_rows"]:
                    raise InvalidLocalCapture(f"book source row bound exceeded: {path}")
                try:
                    row = json.loads(line)
                except json.JSONDecodeError as error:
                    raise InvalidLocalCapture(f"{path}:{line_number}: corrupt JSON") from error
                if row.get("k") == "sys":
                    market = parse_identity(row.get("payload"), policy["window_ms"])
                    if market is None:
                        continue
                    previous = markets.get(market.condition_id)
                    if previous is not None and previous != market:
                        raise InvalidLocalCapture("immutable market identity changed")
                    markets[market.condition_id] = market
                    continue
                if row.get("k") != "book":
                    continue
                condition = row.get("cond")
                token = row.get("tok")
                if not isinstance(condition, str) or not isinstance(token, str):
                    raise InvalidLocalCapture("book identity fields invalid")
                timestamp = integer(row.get("t"), "observation time", positive=True)
                bids = validate_levels(
                    row.get("bids"), bids=True, maximum=policy["maximum_book_levels"],
                )
                asks = validate_levels(
                    row.get("asks"), bids=False, maximum=policy["maximum_book_levels"],
                )
                if bids[0][0] >= asks[0][0]:
                    raise InvalidLocalCapture("crossed or locked book is ineligible")
                key = (condition, timestamp, token)
                value = (bids, asks)
                previous = observations.get(key)
                if previous is not None:
                    if previous != value:
                        raise InvalidLocalCapture("conflicting duplicate book observation")
                    duplicates += 1
                else:
                    observations[key] = value
    if not markets:
        raise InvalidLocalCapture("no BTC hourly identities found")
    if not observations:
        raise InvalidLocalCapture("no BTC book observations found")
    evidence_cutoff_ms = max(timestamp for _, timestamp, _ in observations) // MICROS
    incomplete_conditions = {
        condition for condition, market in markets.items()
        if market.end_ms > evidence_cutoff_ms
    }
    markets = {
        condition: market for condition, market in markets.items()
        if condition not in incomplete_conditions
    }
    if not markets:
        raise InvalidLocalCapture("no completed BTC hourly sessions found")
    for market in markets.values():
        for token, side in ((market.up_token, "UP"), (market.down_token, "DOWN")):
            previous = tokens.get(token)
            candidate = (market.condition_id, side)
            if previous is not None and previous != candidate:
                raise InvalidLocalCapture("token identity changed across markets")
            tokens[token] = candidate
    grouped: dict[tuple[str, int], dict[str, tuple[str, Any]]] = {}
    for (condition, timestamp, token), depth in observations.items():
        if condition in incomplete_conditions:
            continue
        mapped = tokens.get(token)
        if mapped is None:
            raise InvalidLocalCapture("book token has no immutable BTC identity")
        if mapped[0] != condition:
            raise InvalidLocalCapture("book condition/token mismatch")
        market = markets[condition]
        timestamp_ms = timestamp // MICROS
        if timestamp_ms < market.start_ms or timestamp_ms > market.end_ms:
            outside += 1
            continue
        side = mapped[1]
        pair = grouped.setdefault((condition, timestamp), {})
        if side in pair and pair[side] != (token, depth):
            raise InvalidLocalCapture("atomic frame has conflicting side")
        pair[side] = (token, depth)
    emitted: list[dict[str, Any]] = []
    partial = 0
    sequence = 0
    for (condition, timestamp), pair in sorted(
        grouped.items(), key=lambda item: (item[0][1], item[0][0]),
    ):
        if set(pair) != {"UP", "DOWN"}:
            partial += 1
            continue
        for side in ("UP", "DOWN"):
            token, (bids, asks) = pair[side]
            emitted.append({
                "k": "book",
                "cond": condition,
                "tok": token,
                "seq": sequence,
                "t": timestamp,
                "r": timestamp,
                "rq": "OBSERVED_LOCAL_GATEWAY_POLL",
                "bids": [list(level) for level in bids],
                "asks": [list(level) for level in asks],
            })
            sequence += 1
    if not emitted:
        raise InvalidLocalCapture("no complete atomic BTC frames found")
    return markets, emitted, {
        "source_rows": rows_read,
        "exact_duplicates": duplicates,
        "outside_session_books": outside,
        "partial_atomic_frames": partial,
        "incomplete_sessions_excluded": len(incomplete_conditions),
        "evidence_cutoff_ms": evidence_cutoff_ms,
        "emitted_books": len(emitted),
    }


def load_spot(
    policy_path: Path,
    policy: dict[str, Any],
    markets: dict[str, Market],
) -> tuple[list[dict[str, int]], dict[str, int]]:
    values: dict[int, tuple[int, str, int]] = {}
    source_rows = duplicates = excluded = 0
    for source in policy["reference_sources"]:
        path = resolve_source(policy_path, source)
        rows = 0
        with path.open() as handle:
            for line_number, line in enumerate(handle, 1):
                rows += 1
                source_rows += 1
                if rows > source["maximum_rows"]:
                    raise InvalidLocalCapture(f"reference source row bound exceeded: {path}")
                try:
                    outer = json.loads(line)
                except json.JSONDecodeError as error:
                    raise InvalidLocalCapture(f"{path}:{line_number}: corrupt JSON") from error
                record = outer.get("record")
                if not isinstance(record, dict):
                    raise InvalidLocalCapture("reference record envelope invalid")
                payload = record.get("payload")
                if not isinstance(payload, dict) or payload.get("asset") != "BTC":
                    continue
                timestamp = integer(record.get("event_time_ms"), "reference event_time_ms", positive=True)
                condition = payload.get("condition_id")
                reference = payload.get("reference_price_micros")
                target = payload.get("target_price_micros")
                if (
                    not isinstance(condition, str)
                    or not isinstance(reference, str)
                    or not reference.isdigit()
                    or not isinstance(target, str)
                    or not target.isdigit()
                ):
                    raise InvalidLocalCapture("BTC reference fields invalid")
                price = int(reference)
                target_price = int(target)
                if price <= 0 or target_price <= 0:
                    raise InvalidLocalCapture("BTC reference price invalid")
                market = markets.get(condition)
                if market is None or not (market.start_ms <= timestamp <= market.end_ms):
                    excluded += 1
                    continue
                value = (price, condition, target_price)
                previous = values.get(timestamp)
                if previous is not None:
                    if previous != value:
                        raise InvalidLocalCapture("conflicting BTC reference timestamp")
                    duplicates += 1
                else:
                    values[timestamp] = value
    if not values:
        raise InvalidLocalCapture("no matching BTC reference observations found")
    return (
        [{"timestamp_ms": timestamp, "price_micros": value[0]} for timestamp, value in sorted(values.items())],
        {
            "source_rows": source_rows,
            "exact_duplicates": duplicates,
            "excluded_reference_rows": excluded,
            "emitted_spot_rows": len(values),
        },
    )


def normalize(policy_path: Path, out_root: Path) -> dict[str, Any]:
    if out_root.exists() or out_root.is_symlink():
        raise InvalidLocalCapture("output root must be new")
    policy = read_policy(policy_path)
    markets, books, book_stats = load_books(policy_path, policy)
    spot, spot_stats = load_spot(policy_path, policy, markets)
    identities = [
        {
            "k": "sys",
            "payload": {
                "event_type": "market_identity",
                "asset": "BTC",
                "condition_id": market.condition_id,
                "up_token_id": market.up_token,
                "down_token_id": market.down_token,
                "start_time_ms": market.start_ms,
                "end_time_ms": market.end_ms,
            },
        }
        for market in sorted(markets.values(), key=lambda item: (item.start_ms, item.condition_id))
    ]
    out_root.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".local-btc-hourly-", dir=out_root.parent) as temporary:
        staging = Path(temporary)
        tape = staging / "clob-l2.jsonl"
        digest = hashlib.sha256()
        with tape.open("xb") as output:
            for row in identities + books:
                encoded = canonical(row) + b"\n"
                output.write(encoded)
                digest.update(encoded)
        spot_path = staging / "spot.parquet"
        trades_path = staging / "trades.parquet"
        pq.write_table(pa.Table.from_pylist(spot, schema=SPOT_SCHEMA), spot_path, compression="zstd")
        pq.write_table(pa.Table.from_pylist([], schema=TRADE_SCHEMA), trades_path, compression="zstd")
        out_root.mkdir(parents=False, exist_ok=False)
        for source in (tape, spot_path, trades_path):
            os.replace(source, out_root / source.name)
    manifest = {
        "schema_version": 1,
        "non_authoritative": True,
        "capability": "LOCAL_OBSERVED_FULL_DEPTH_TAKER_RESEARCH_ONLY",
        "asset": "BTC",
        "source_time_semantics": policy["source_time_semantics"],
        "spot_symbol": "btcusdt",
        "spot_source": policy["reference_source"],
        "sessions": len(markets),
        "books": len(books),
        "spot_rows": len(spot),
        "trades": 0,
        "data_sha256": sha256_file(out_root / "clob-l2.jsonl"),
        "spot_sha256": sha256_file(out_root / "spot.parquet"),
        "trades_sha256": sha256_file(out_root / "trades.parquet"),
        "policy_sha256": sha256_file(policy_path),
        "book_source_sha256": [source["sha256"] for source in policy["book_sources"]],
        "reference_source_sha256": [source["sha256"] for source in policy["reference_sources"]],
        "book_stats": book_stats,
        "spot_stats": spot_stats,
    }
    (out_root / "manifest.json").write_bytes(canonical(manifest) + b"\n")
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--out-root", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        print(json.dumps(normalize(arguments.policy, arguments.out_root), sort_keys=True))
        return 0
    except (InvalidLocalCapture, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"local BTC normalization rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
