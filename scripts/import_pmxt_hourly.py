#!/usr/bin/env python3
"""Import allowlisted PMXT v2 CLOB events into immutable research partitions.

This importer deliberately has no network client. An operator supplies already
downloaded, content-addressable Parquet files and an identity manifest produced
from official Polymarket metadata. This prevents an external archive from
silently selecting markets, creating unbounded storage use, or becoming a
source of market identity truth.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from collections import defaultdict
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

try:
    import pyarrow.parquet as pq
except ImportError as error:  # pragma: no cover - deployment dependency
    raise SystemExit("pyarrow is required for PMXT Parquet import") from error


class InvalidExternalData(ValueError):
    pass


REQUIRED_COLUMNS = {"timestamp_received", "timestamp", "market", "event_type", "asset_id"}
OPTIONAL_COLUMNS = {"bids", "asks", "price", "size", "side", "best_bid", "best_ask", "fee_rate_bps", "transaction_hash", "old_tick_size", "new_tick_size"}


def canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while block := handle.read(1 << 20):
            digest.update(block)
    return digest.hexdigest()


def exact_int(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise InvalidExternalData(f"{field} must be an integer")
    return value


def parse_policy(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    expected = {"schema_version", "source_id", "source_url_template", "license", "maximum_input_bytes", "maximum_rows_per_file", "allowed_event_types"}
    if set(value) != expected or value["schema_version"] != 1 or value["source_id"] != "pmxt-v2" or value["license"] != "CC-BY-4.0":
        raise InvalidExternalData("invalid PMXT policy")
    if not isinstance(value["source_url_template"], str) or not value["source_url_template"].startswith("https://"):
        raise InvalidExternalData("invalid PMXT URL template")
    for field in ("maximum_input_bytes", "maximum_rows_per_file"):
        if exact_int(value[field], field) <= 0:
            raise InvalidExternalData("invalid PMXT bounds")
    if value["allowed_event_types"] != ["book", "price_change", "last_trade_price", "tick_size_change"]:
        raise InvalidExternalData("unexpected PMXT event policy")
    return value


def parse_identities(path: Path) -> dict[tuple[str, str], tuple[str, str]]:
    value = json.loads(path.read_text())
    v1_fields = {"schema_version", "source", "assets"}
    v2_fields = {"schema_version", "source", "source_id", "policy_sha256", "requested_start_time_ms", "requested_end_time_ms", "sources", "assets"}
    if value.get("source") != "official-polymarket" or frozenset(value) not in {frozenset(v1_fields), frozenset(v2_fields)}:
        raise InvalidExternalData("identity manifest must be a recognized official-polymarket schema")
    if value.get("schema_version") == 2:
        if value.get("source_id") != "official-polymarket-gamma" or not isinstance(value.get("policy_sha256"), str) or len(value["policy_sha256"]) != 64:
            raise InvalidExternalData("identity manifest v2 provenance invalid")
        start, end = value.get("requested_start_time_ms"), value.get("requested_end_time_ms")
        if exact_int(start, "requested_start_time_ms") % 3_600_000 or exact_int(end, "requested_end_time_ms") <= start:
            raise InvalidExternalData("identity manifest v2 range invalid")
        if not isinstance(value.get("sources"), list) or not value["sources"]:
            raise InvalidExternalData("identity manifest v2 source evidence invalid")
        for source in value["sources"]:
            if not isinstance(source, dict) or set(source) - {"kind", "name", "url", "sha256"} or not isinstance(source.get("kind"), str) or not isinstance(source.get("sha256"), str) or len(source["sha256"]) != 64:
                raise InvalidExternalData("identity manifest v2 source entry invalid")
    elif value.get("schema_version") != 1 or set(value) != v1_fields:
        raise InvalidExternalData("identity manifest version invalid")
    if not isinstance(value["assets"], list) or not value["assets"]:
        raise InvalidExternalData("identity manifest has no assets")
    tokens: dict[tuple[str, str], tuple[str, str]] = {}
    conditions: set[str] = set()
    for entry in value["assets"]:
        if not isinstance(entry, dict) or set(entry) != {"asset", "condition_id", "up_token_id", "down_token_id", "start_time_ms", "end_time_ms", "rules_fingerprint"}:
            raise InvalidExternalData("identity manifest entry schema invalid")
        asset, condition = entry["asset"], entry["condition_id"]
        if asset not in {"BTC", "ETH"} or not isinstance(condition, str) or not condition.startswith("0x") or condition in conditions:
            raise InvalidExternalData("identity asset or condition invalid")
        start, end = exact_int(entry["start_time_ms"], "start_time_ms"), exact_int(entry["end_time_ms"], "end_time_ms")
        if end - start != 3_600_000 or not isinstance(entry["rules_fingerprint"], str) or not entry["rules_fingerprint"]:
            raise InvalidExternalData("identity hour or rules invalid")
        conditions.add(condition)
        for side, token in (("UP", entry["up_token_id"]), ("DOWN", entry["down_token_id"])):
            if not isinstance(token, str) or not token.isdecimal() or (condition, token) in tokens:
                raise InvalidExternalData("identity token invalid or duplicated")
            tokens[(condition, token)] = (asset, side)
    return tokens


def timestamp_ms(value: Any, field: str) -> int:
    # pyarrow TimestampScalar arrives as datetime; fixtures may provide datetime too.
    if not isinstance(value, datetime) or value.tzinfo is None:
        raise InvalidExternalData(f"{field} must be UTC timestamp")
    return int(value.timestamp() * 1000)


def string_decimal(value: Any, field: str, *, allow_empty: bool = True) -> str | None:
    if value is None:
        return None
    text = str(value)
    if not text and allow_empty:
        return None
    if not text or any(character not in "0123456789.-" for character in text) or text.count(".") > 1 or text.count("-") > 1 or ("-" in text and not text.startswith("-")):
        raise InvalidExternalData(f"{field} decimal invalid")
    return text


def json_book(value: Any, field: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str):
        raise InvalidExternalData(f"{field} must be JSON string")
    decoded = json.loads(value)
    if not isinstance(decoded, list):
        raise InvalidExternalData(f"{field} must be a JSON list")
    for level in decoded:
        if not isinstance(level, list) or len(level) != 2 or not all(isinstance(part, str) for part in level):
            raise InvalidExternalData(f"{field} level invalid")
    return value


def output_path(root: Path, asset: str, received_ms: int) -> Path:
    date = datetime.fromtimestamp(received_ms / 1000, UTC)
    return root / f"{asset}-data" / date.strftime("%Y-%m-%d") / date.strftime("%H")


def import_file(source: Path, tokens: dict[tuple[str, str], tuple[str, str]], policy: dict[str, Any], out_root: Path) -> dict[str, int]:
    if source.is_symlink() or not source.is_file() or source.stat().st_size > policy["maximum_input_bytes"]:
        raise InvalidExternalData("source file is unsafe or exceeds configured size")
    parquet = pq.ParquetFile(source)
    names = set(parquet.schema_arrow.names)
    if not REQUIRED_COLUMNS.issubset(names) or not names.issubset(REQUIRED_COLUMNS | OPTIONAL_COLUMNS):
        raise InvalidExternalData("PMXT Parquet schema is unexpected")
    if parquet.metadata.num_rows > policy["maximum_rows_per_file"]:
        raise InvalidExternalData("PMXT file exceeds configured row bound")
    input_digest = sha256(source)
    pending: dict[Path, list[dict[str, Any]]] = defaultdict(list)
    accepted = 0
    last_received: dict[tuple[str, str], int] = {}
    for batch in parquet.iter_batches(batch_size=65_536):
        for row in batch.to_pylist():
            condition, token = row["market"], row["asset_id"]
            if isinstance(condition, bytes):
                condition = condition.decode("ascii")
            if not isinstance(condition, str) or not isinstance(token, str):
                raise InvalidExternalData("PMXT condition or token invalid")
            identity = tokens.get((condition, token))
            if identity is None:
                continue
            event = row["event_type"]
            if event not in policy["allowed_event_types"]:
                raise InvalidExternalData("PMXT event type is not allowlisted")
            source_ms, received_ms = timestamp_ms(row["timestamp"], "timestamp"), timestamp_ms(row["timestamp_received"], "timestamp_received")
            if source_ms < 0 or received_ms < source_ms:
                raise InvalidExternalData("PMXT timestamp ordering invalid")
            key = (condition, token)
            if received_ms < last_received.get(key, -1):
                raise InvalidExternalData("PMXT receive timestamp regressed")
            last_received[key] = received_ms
            asset, side = identity
            payload = {
                "schema_version": 1, "source_id": policy["source_id"], "input_sha256": input_digest,
                "asset": asset, "side": side, "condition_id": condition, "token_id": token,
                "event_type": event, "source_time_ms": source_ms, "received_time_ms": received_ms,
                "bids": json_book(row.get("bids"), "bids"), "asks": json_book(row.get("asks"), "asks"),
                "price": string_decimal(row.get("price"), "price"), "size": string_decimal(row.get("size"), "size"),
                "best_bid": string_decimal(row.get("best_bid"), "best_bid"), "best_ask": string_decimal(row.get("best_ask"), "best_ask"),
                "side_at_venue": row.get("side"), "fee_rate_bps": row.get("fee_rate_bps"),
                "transaction_hash": row.get("transaction_hash"), "old_tick_size": string_decimal(row.get("old_tick_size"), "old_tick_size"),
                "new_tick_size": string_decimal(row.get("new_tick_size"), "new_tick_size"),
            }
            if payload["side_at_venue"] not in {None, "BUY", "SELL"}:
                raise InvalidExternalData("PMXT venue side invalid")
            pending[output_path(out_root, asset, received_ms)].append(payload)
            accepted += 1
    if not accepted:
        return {"accepted": 0, "partitions": 0}
    for directory, records in pending.items():
        if directory.exists():
            raise InvalidExternalData(f"output partition already exists: {directory}")
    for directory, records in pending.items():
        directory.mkdir(parents=True, exist_ok=False)
        data = directory / "pmxt-events.jsonl"
        with data.open("x", encoding="utf-8") as handle:
            for record in records:
                handle.write(canonical(record).decode() + "\n")
        manifest = {"schema_version": 1, "non_authoritative": True, "source_id": policy["source_id"], "license": policy["license"], "input_file": source.name, "input_sha256": input_digest, "rows": len(records), "data_sha256": sha256(data)}
        with (directory / "manifest.json").open("x", encoding="utf-8") as handle:
            handle.write(json.dumps(manifest, sort_keys=True, separators=(",", ":")))
    return {"accepted": accepted, "partitions": len(pending)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--identities", required=True, type=Path)
    parser.add_argument("--source", required=True, action="append", type=Path)
    parser.add_argument("--out-root", required=True, type=Path)
    args = parser.parse_args()
    try:
        policy, tokens = parse_policy(args.policy), parse_identities(args.identities)
        if args.out_root.exists():
            raise InvalidExternalData("output root must be a new directory")
        results = [import_file(path, tokens, policy, args.out_root) for path in args.source]
        print(json.dumps({"source_id": policy["source_id"], "files": len(results), "accepted": sum(item["accepted"] for item in results), "partitions": sum(item["partitions"] for item in results)}, sort_keys=True))
        return 0
    except (InvalidExternalData, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"PMXT import rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
