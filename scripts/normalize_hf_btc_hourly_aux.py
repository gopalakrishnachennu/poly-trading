#!/usr/bin/env python3
"""Normalize pinned Binance spot and verified Polymarket flow into fixed point."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.parquet as pq

try:
    from scripts.import_hf_btc_hourly_bbo import fixed_micros, load_boundaries
    from scripts.verify_hf_btc_hourly_candidates import InvalidIdentity, canonical, parse_policy, sha256_file
except ModuleNotFoundError:
    from import_hf_btc_hourly_bbo import fixed_micros, load_boundaries
    from verify_hf_btc_hourly_candidates import InvalidIdentity, canonical, parse_policy, sha256_file

from decimal import ROUND_FLOOR


TICK_COLUMNS = {
    "market_id", "timestamp_ms", "token_id", "outcome", "side", "price",
    "size_usdc", "tx_hash", "block_number", "log_index", "source",
    "spot_price_usdt", "spot_price_ts_ms", "local_recv_ts_ns",
}
SPOT_COLUMNS = {"ts_ms", "symbol", "price", "source"}

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
SPOT_SCHEMA = pa.schema([
    ("timestamp_ms", pa.int64()),
    ("price_micros", pa.int64()),
])


def validate_source(path: Path, maximum_bytes: int, expected_sha: str, maximum_rows: int, columns: set[str]) -> pq.ParquetFile:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > maximum_bytes:
        raise InvalidIdentity("auxiliary source is unsafe or exceeds configured size")
    if sha256_file(path) != expected_sha:
        raise InvalidIdentity("auxiliary source checksum mismatch")
    parquet = pq.ParquetFile(path)
    if parquet.metadata.num_rows > maximum_rows:
        raise InvalidIdentity("auxiliary source row bound exceeded")
    if not columns.issubset(set(parquet.schema_arrow.names)):
        raise InvalidIdentity("auxiliary source schema incomplete")
    return parquet


def normalize(
    policy: dict[str, Any],
    ticks_path: Path,
    spot_path: Path,
    identities_path: Path,
    labels_path: Path,
    out_root: Path,
) -> dict[str, Any]:
    if out_root.exists() or out_root.is_symlink():
        raise InvalidIdentity("auxiliary output root must be new")
    ticks_file = validate_source(
        ticks_path, policy["maximum_ticks_bytes"], policy["ticks_sha256"],
        policy["maximum_ticks_rows"], TICK_COLUMNS,
    )
    spot_file = validate_source(
        spot_path, policy["maximum_spot_bytes"], policy["spot_sha256"],
        policy["maximum_spot_rows"], SPOT_COLUMNS,
    )
    identities, token_map = load_boundaries(identities_path, labels_path)
    market_ids = sorted({market_id for market_id, _ in token_map})
    market_array = pa.array(market_ids)
    start_by_market = {
        market_id: identities[condition]["start_time_ms"]
        for (market_id, _), (condition, _) in token_map.items()
    }
    end_by_market = {
        market_id: identities[condition]["end_time_ms"]
        for (market_id, _), (condition, _) in token_map.items()
    }
    trades: dict[tuple[Any, ...], dict[str, Any]] = {}
    for batch in ticks_file.iter_batches(columns=sorted(TICK_COLUMNS), batch_size=131_072):
        market_col = batch.column(batch.schema.get_field_index("market_id"))
        selected = batch.filter(pc.is_in(market_col, value_set=market_array))
        for row in selected.to_pylist():
            market_id, token = row["market_id"], row["token_id"]
            mapped = token_map.get((market_id, token))
            if mapped is None:
                raise InvalidIdentity("trade token is not in verified market")
            condition, outcome = mapped
            if row["outcome"] != ("Up" if outcome == "UP" else "Down"):
                raise InvalidIdentity("trade outcome/token mapping disagrees")
            timestamp = row["timestamp_ms"]
            if isinstance(timestamp, bool) or not isinstance(timestamp, int):
                raise InvalidIdentity("trade timestamp invalid")
            if not (start_by_market[market_id] <= timestamp <= end_by_market[market_id]):
                continue
            side = row["side"]
            if side not in {"BUY", "SELL"}:
                raise InvalidIdentity("trade side invalid")
            source = row["source"]
            if source not in {"onchain", "websocket"}:
                raise InvalidIdentity("trade source invalid")
            price = fixed_micros(row["price"], "trade price", ROUND_FLOOR)
            size = fixed_micros(row["size_usdc"], "trade size", ROUND_FLOOR)
            if price <= 0 or price > 1_000_000 or size <= 0:
                raise InvalidIdentity("trade price or size outside bounds")
            tx_hash, log_index = row["tx_hash"], row["log_index"]
            if source == "onchain":
                if not isinstance(tx_hash, str) or not tx_hash.startswith("0x") or not isinstance(log_index, int):
                    raise InvalidIdentity("on-chain trade identity invalid")
                key = ("chain", tx_hash.lower(), log_index)
            else:
                order_hash = row.get("order_hash")
                key = ("ws", market_id, token, timestamp, side, price, size, order_hash)
            record = {
                "timestamp_ms": timestamp,
                "local_receive_time_ns": row["local_recv_ts_ns"],
                "market_id": market_id,
                "condition_id": condition,
                "token_id": token,
                "outcome": outcome,
                "aggressor_side": side,
                "price_micros": price,
                "size_usdc_micros": size,
                "source": source,
            }
            previous = trades.get(key)
            if previous is not None and previous != record:
                raise InvalidIdentity("duplicate trade identity has conflicting facts")
            trades[key] = record
    range_start = min(row["start_time_ms"] for row in identities.values()) - 600_000
    range_end = max(row["end_time_ms"] for row in identities.values())
    spot: dict[int, int] = {}
    for batch in spot_file.iter_batches(columns=sorted(SPOT_COLUMNS), batch_size=262_144):
        table = pa.Table.from_batches([batch])
        mask = pc.and_(
            pc.equal(table["symbol"], "btcusdt"),
            pc.equal(table["source"], "binance"),
        )
        selected = table.filter(mask)
        for row in selected.to_pylist():
            timestamp = row["ts_ms"]
            if isinstance(timestamp, bool) or not isinstance(timestamp, int):
                raise InvalidIdentity("spot timestamp invalid")
            if not (range_start <= timestamp <= range_end):
                continue
            price = fixed_micros(row["price"], "spot price", ROUND_FLOOR)
            if price <= 0:
                raise InvalidIdentity("spot price outside bounds")
            previous = spot.get(timestamp)
            if previous is not None and previous != price:
                raise InvalidIdentity("same spot timestamp has conflicting price")
            spot[timestamp] = price
    if not trades or not spot:
        raise InvalidIdentity("auxiliary evidence has no usable rows")
    trade_rows = sorted(trades.values(), key=lambda row: (
        row["timestamp_ms"], row["condition_id"], row["token_id"], row["aggressor_side"]
    ))
    spot_rows = [{"timestamp_ms": timestamp, "price_micros": price} for timestamp, price in sorted(spot.items())]
    out_root.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".hf-aux-", dir=out_root.parent) as temporary:
        temp = Path(temporary)
        trade_file = temp / "trades.parquet"
        spot_file_out = temp / "spot.parquet"
        pq.write_table(
            pa.Table.from_pylist(trade_rows, schema=TRADE_SCHEMA),
            trade_file,
            compression="zstd",
        )
        pq.write_table(
            pa.Table.from_pylist(spot_rows, schema=SPOT_SCHEMA),
            spot_file_out,
            compression="zstd",
        )
        out_root.mkdir(parents=False, exist_ok=False)
        os.replace(trade_file, out_root / "trades.parquet")
        os.replace(spot_file_out, out_root / "spot.parquet")
    manifest = {
        "schema_version": 1,
        "non_authoritative": True,
        "source_id": policy["source_id"],
        "dataset_revision": policy["dataset_revision"],
        "identity_manifest_sha256": sha256_file(identities_path),
        "resolution_labels_sha256": sha256_file(labels_path),
        "raw_ticks_sha256": sha256_file(ticks_path),
        "raw_spot_sha256": sha256_file(spot_path),
        "trades": len(trade_rows),
        "spot_rows": len(spot_rows),
        "trades_sha256": sha256_file(out_root / "trades.parquet"),
        "spot_sha256": sha256_file(out_root / "spot.parquet"),
        "spot_symbol": "btcusdt",
        "spot_source": "binance",
    }
    (out_root / "manifest.json").write_bytes(canonical(manifest) + b"\n")
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--ticks", required=True, type=Path)
    parser.add_argument("--spot", required=True, type=Path)
    parser.add_argument("--identities", required=True, type=Path)
    parser.add_argument("--labels", required=True, type=Path)
    parser.add_argument("--out-root", required=True, type=Path)
    args = parser.parse_args()
    try:
        result = normalize(
            parse_policy(args.policy), args.ticks, args.spot, args.identities,
            args.labels, args.out_root,
        )
        print(json.dumps(result, sort_keys=True))
        return 0
    except (InvalidIdentity, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"auxiliary normalization rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
