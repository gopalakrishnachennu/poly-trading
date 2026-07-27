#!/usr/bin/env python3
"""Export a frozen 48-hour capture/trade evidence bundle.

The exporter streams newline-complete source prefixes and never reads records
appended after each file's initial stat boundary. All output is simulated or
public observation evidence; it grants no trading authority.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import re
import shutil
import time
from collections import Counter
from dataclasses import dataclass
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path
from typing import Any, Iterable, Iterator

SCHEMA_VERSION = 1
HOURS_48_NS = 48 * 60 * 60 * 1_000_000_000
ASSETS = ("BTC", "ETH")
WINDOWS = ("5m", "15m", "1h")
WINDOW_DIR = {"5m": "research-5m", "15m": "research-15m", "1h": "research-1h"}
CURRENT_TRADE_ACTIONS = {"open", "hedge", "reduce", "exit", "settle"}
CORE_FINANCIAL_KINDS = {
    "paper_start",
    "paper_stop",
    "PAIR_BUY",
    "reservation_acquired",
    "reservation_released",
    "filled_pair",
}
NUMERIC_TEXT = re.compile(r"^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?$")

BOOK_COLUMNS = [
    "schema_version",
    "asset",
    "window",
    "captured_at_ns",
    "captured_at_utc",
    "condition_id",
    "market_start_ms",
    "market_end_ms",
    "token_id",
    "side",
    "best_bid_price_micros",
    "best_bid_quantity_micros",
    "best_ask_price_micros",
    "best_ask_quantity_micros",
    "bid_level_count",
    "ask_level_count",
    "quality_status",
    "quality_reasons",
    "exact_duplicate_capture",
    "bids_json",
    "asks_json",
    "source_path",
    "source_line",
]

IDENTITY_COLUMNS = [
    "schema_version",
    "asset",
    "window",
    "condition_id",
    "market_start_ms",
    "market_start_utc",
    "market_end_ms",
    "market_end_utc",
    "up_token_id",
    "down_token_id",
    "source_path",
    "source_line",
]

REFERENCE_COLUMNS = [
    "schema_version",
    "event_time_ms",
    "event_time_utc",
    "sequence",
    "snapshot_digest",
    "asset",
    "condition_id",
    "reference_price_micros",
    "target_price_micros",
    "up_best_ask_micros",
    "up_best_bid_micros",
    "down_best_ask_micros",
    "down_best_bid_micros",
    "up_bid_size_micros",
    "up_ask_size_micros",
    "down_bid_size_micros",
    "down_ask_size_micros",
    "source_path",
    "source_line",
]

COINBASE_COLUMNS = [
    "schema_version",
    "receive_time_ms",
    "receive_time_utc",
    "event_time_ms",
    "event_time_utc",
    "symbol",
    "bid_raw",
    "ask_raw",
    "mid_raw",
    "bid_size_raw",
    "ask_size_raw",
    "sequence",
    "source",
    "source_path",
    "source_line",
]

DECISION_FIELDS = [
    "strategy_version",
    "condition_id",
    "asset",
    "window",
    "elapsed_secs",
    "observed_at_ns",
    "action",
    "reason",
    "setup_id",
    "side",
    "ask_micros",
    "bid_micros",
    "edge_micros",
    "effective_cost_micros",
    "quantity_micros",
    "stake_micros",
    "mark_value_micros",
    "unrealized_pnl_micros",
    "realized_pnl_micros",
    "hold_secs",
    "outcome",
    "settlement_source",
    "stake_tier",
    "risk_bps",
    "position_state",
    "book",
    "paper_only",
    "real_money",
]
DECISION_COLUMNS = [
    "schema_version",
    "observed_at_utc",
    *DECISION_FIELDS,
    "rejection_reasons_json",
    "source_path",
    "source_line",
]

TRADE_COLUMNS = [
    "schema_version",
    "event_time_ns",
    "event_time_utc",
    "authority_class",
    "event_type",
    "execution_mode",
    "paper_authoritative",
    "real_money",
    "strategy_version",
    "strategy_id",
    "strategy_status",
    "book",
    "campaign_id",
    "trade_id",
    "asset",
    "window",
    "condition_id",
    "side",
    "reason",
    "outcome",
    "quantity_micros",
    "stake_micros",
    "entry_price_micros",
    "exit_bid_micros",
    "up_price_micros",
    "down_price_micros",
    "pair_cost_micros",
    "fee_micros",
    "slippage_micros",
    "realized_pnl_micros",
    "locked_pnl_micros",
    "entry_price_raw",
    "exit_price_raw",
    "stake_raw",
    "shares_raw",
    "realized_pnl_raw",
    "bankroll_after_raw",
    "record_digest",
    "payload_json",
    "source_path",
    "source_line",
]

CORE_EVENT_COLUMNS = [
    "schema_version",
    "event_time_ms",
    "event_time_utc",
    "recorded_time_ms",
    "campaign_id",
    "sequence",
    "stream",
    "kind",
    "runtime_config_id",
    "runtime_config_digest",
    "record_digest",
    "payload_json",
    "source_path",
    "source_line",
]

SOURCE_COLUMNS = [
    "source_path",
    "snapshot_bytes",
    "complete_prefix_bytes",
    "trailing_incomplete_bytes",
    "complete_lines",
    "selected_records",
    "sha256_complete_prefix",
]


class ExportError(RuntimeError):
    """Raised when evidence cannot be exported without ambiguity."""


def parse_json(line: bytes, context: str) -> dict[str, Any]:
    try:
        value = json.loads(line, parse_float=Decimal)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise ExportError(f"malformed complete JSONL record: {context}") from exc
    if not isinstance(value, dict):
        raise ExportError(f"JSONL record is not an object: {context}")
    return value


def text(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, Decimal):
        return format(value, "f")
    return str(value)


def csv_value(value: Any) -> str:
    raw = text(value)
    if raw and raw[0] in ("=", "+", "-", "@") and not NUMERIC_TEXT.fullmatch(raw):
        return "'" + raw
    return raw


def canonical(value: Any) -> str:
    def normalize(item: Any) -> Any:
        if isinstance(item, Decimal):
            return format(item, "f")
        if isinstance(item, dict):
            return {str(key): normalize(val) for key, val in sorted(item.items())}
        if isinstance(item, list):
            return [normalize(val) for val in item]
        return item

    return json.dumps(normalize(value), sort_keys=True, separators=(",", ":"))


def iso_ns(value: int) -> str:
    return datetime.fromtimestamp(value / 1_000_000_000, tz=timezone.utc).isoformat(
        timespec="microseconds"
    ).replace("+00:00", "Z")


def iso_ms(value: int) -> str:
    return iso_ns(value * 1_000_000)


def require_int(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ExportError(f"{label} is not an integer")
    return value


@dataclass
class SourceEvidence:
    path: str
    snapshot_bytes: int
    complete_prefix_bytes: int = 0
    trailing_incomplete_bytes: int = 0
    complete_lines: int = 0
    selected_records: int = 0
    sha256_complete_prefix: str = ""

    def row(self) -> dict[str, Any]:
        return {
            "source_path": self.path,
            "snapshot_bytes": self.snapshot_bytes,
            "complete_prefix_bytes": self.complete_prefix_bytes,
            "trailing_incomplete_bytes": self.trailing_incomplete_bytes,
            "complete_lines": self.complete_lines,
            "selected_records": self.selected_records,
            "sha256_complete_prefix": self.sha256_complete_prefix,
        }


class FrozenJsonl:
    def __init__(self, path: Path):
        metadata = path.lstat()
        if not path.is_file() or path.is_symlink():
            raise ExportError(f"source is not a regular file: {path}")
        self.path = path
        self.evidence = SourceEvidence(str(path.resolve()), metadata.st_size)

    def records(self) -> Iterator[tuple[int, dict[str, Any]]]:
        remaining = self.evidence.snapshot_bytes
        consumed = 0
        hasher = hashlib.sha256()
        with self.path.open("rb") as handle:
            line_number = 0
            while remaining:
                line = handle.readline(remaining)
                if not line:
                    break
                remaining -= len(line)
                consumed += len(line)
                if not line.endswith(b"\n"):
                    self.evidence.trailing_incomplete_bytes = len(line)
                    break
                line_number += 1
                hasher.update(line)
                self.evidence.complete_prefix_bytes += len(line)
                self.evidence.complete_lines = line_number
                if not line.strip():
                    raise ExportError(f"blank complete JSONL line: {self.path}:{line_number}")
                yield line_number, parse_json(line, f"{self.path}:{line_number}")
        if consumed < self.evidence.snapshot_bytes:
            self.evidence.trailing_incomplete_bytes += (
                self.evidence.snapshot_bytes - consumed
            )
        self.evidence.sha256_complete_prefix = hasher.hexdigest()


class CsvOutput:
    def __init__(self, path: Path, columns: list[str]):
        self.path = path
        self.columns = columns
        self.handle = path.open("w", newline="", encoding="utf-8")
        self.writer = csv.DictWriter(
            self.handle, fieldnames=columns, extrasaction="raise", lineterminator="\n"
        )
        self.writer.writeheader()
        self.rows = 0
        self.min_time_ns: int | None = None
        self.max_time_ns: int | None = None

    def write(self, row: dict[str, Any], time_ns: int | None = None) -> None:
        self.writer.writerow({column: csv_value(row.get(column)) for column in self.columns})
        self.rows += 1
        if time_ns is not None:
            self.min_time_ns = (
                time_ns if self.min_time_ns is None else min(self.min_time_ns, time_ns)
            )
            self.max_time_ns = (
                time_ns if self.max_time_ns is None else max(self.max_time_ns, time_ns)
            )

    def close(self) -> None:
        self.handle.flush()
        os.fsync(self.handle.fileno())
        self.handle.close()


def source_files(directory: Path, pattern: str) -> list[Path]:
    if not directory.exists():
        return []
    return sorted(path for path in directory.glob(pattern) if path.is_file())


def validate_level(level: Any, label: str) -> tuple[int, int]:
    if not isinstance(level, list) or len(level) != 2:
        raise ExportError(f"{label} is not a price/quantity pair")
    price = require_int(level[0], f"{label} price")
    quantity = require_int(level[1], f"{label} quantity")
    if not 0 <= price <= 1_000_000 or quantity < 0:
        raise ExportError(f"{label} fixed-point value is invalid")
    return price, quantity


def book_quality(bids: list[Any], asks: list[Any]) -> tuple[str, str]:
    reasons: list[str] = []
    bid_values = [validate_level(level, "bid level") for level in bids]
    ask_values = [validate_level(level, "ask level") for level in asks]
    if not bid_values:
        reasons.append("EMPTY_BIDS")
    if not ask_values:
        reasons.append("EMPTY_ASKS")
    if any(left[0] <= right[0] for left, right in zip(bid_values, bid_values[1:])):
        reasons.append("BIDS_NOT_STRICTLY_DESCENDING")
    if any(left[0] >= right[0] for left, right in zip(ask_values, ask_values[1:])):
        reasons.append("ASKS_NOT_STRICTLY_ASCENDING")
    if bid_values and ask_values and bid_values[0][0] >= ask_values[0][0]:
        reasons.append("CROSSED_OR_LOCKED_BOOK")
    return ("VALID" if not reasons else "OBSERVED_INVALID", "|".join(reasons))


def empty_trade() -> dict[str, Any]:
    return {column: "" for column in TRADE_COLUMNS}


def process_books(
    root: Path,
    start_ns: int,
    end_ns: int,
    outputs: dict[tuple[str, str], CsvOutput],
    identities_output: CsvOutput,
    evidences: list[SourceEvidence],
) -> None:
    identities: dict[tuple[str, str, str], dict[str, Any]] = {}
    token_sides: dict[tuple[str, str], str] = {}
    duplicate_fingerprints: set[bytes] = set()
    for window in WINDOWS:
        for asset in ASSETS:
            for path in source_files(root / "var" / WINDOW_DIR[window], f"{asset.lower()}-*.jsonl"):
                source = FrozenJsonl(path)
                for line_number, record in source.records():
                    kind = record.get("k")
                    if kind == "sys":
                        payload = record.get("payload")
                        if not isinstance(payload, dict) or payload.get("event_type") != "market_identity":
                            raise ExportError(f"unknown system record: {path}:{line_number}")
                        condition = text(payload.get("condition_id"))
                        identity = {
                            "schema_version": SCHEMA_VERSION,
                            "asset": asset,
                            "window": window,
                            "condition_id": condition,
                            "market_start_ms": require_int(payload.get("start_time_ms"), "market start"),
                            "market_end_ms": require_int(payload.get("end_time_ms"), "market end"),
                            "up_token_id": text(payload.get("up_token_id")),
                            "down_token_id": text(payload.get("down_token_id")),
                        }
                        key = (asset, window, condition)
                        prior = identities.get(key)
                        if prior is not None and any(
                            text(prior[field]) != text(identity[field])
                            for field in (
                                "market_start_ms",
                                "market_end_ms",
                                "up_token_id",
                                "down_token_id",
                            )
                        ):
                            raise ExportError(f"conflicting market identity for {key}")
                        if prior is None:
                            identities[key] = identity
                            token_sides[(condition, identity["up_token_id"])] = "UP"
                            token_sides[(condition, identity["down_token_id"])] = "DOWN"
                            if (
                                identity["market_end_ms"] * 1_000_000 >= start_ns
                                and identity["market_start_ms"] * 1_000_000 <= end_ns
                            ):
                                identities_output.write(
                                    {
                                        **identity,
                                        "market_start_utc": iso_ms(identity["market_start_ms"]),
                                        "market_end_utc": iso_ms(identity["market_end_ms"]),
                                        "source_path": str(path.resolve()),
                                        "source_line": line_number,
                                    }
                                )
                                source.evidence.selected_records += 1
                        continue
                    if kind != "book":
                        raise ExportError(f"unknown capture record kind: {path}:{line_number}")
                    captured_at_ns = require_int(record.get("t"), "book capture time")
                    if not start_ns <= captured_at_ns <= end_ns:
                        continue
                    condition = text(record.get("cond"))
                    token = text(record.get("tok"))
                    side = token_sides.get((condition, token))
                    identity = identities.get((asset, window, condition))
                    if side is None or identity is None:
                        raise ExportError(
                            f"book lacks prior immutable identity: {path}:{line_number}"
                        )
                    bids = record.get("bids")
                    asks = record.get("asks")
                    if not isinstance(bids, list) or not isinstance(asks, list):
                        raise ExportError(f"book levels are missing: {path}:{line_number}")
                    quality, quality_reasons = book_quality(bids, asks)
                    fingerprint = hashlib.sha256(
                        canonical(
                            {
                                "asset": asset,
                                "window": window,
                                "condition": condition,
                                "token": token,
                                "t": captured_at_ns,
                                "bids": bids,
                                "asks": asks,
                            }
                        ).encode()
                    ).digest()
                    duplicate = fingerprint in duplicate_fingerprints
                    duplicate_fingerprints.add(fingerprint)
                    best_bid = validate_level(bids[0], "best bid") if bids else ("", "")
                    best_ask = validate_level(asks[0], "best ask") if asks else ("", "")
                    outputs[(asset, window)].write(
                        {
                            "schema_version": SCHEMA_VERSION,
                            "asset": asset,
                            "window": window,
                            "captured_at_ns": captured_at_ns,
                            "captured_at_utc": iso_ns(captured_at_ns),
                            "condition_id": condition,
                            "market_start_ms": identity["market_start_ms"],
                            "market_end_ms": identity["market_end_ms"],
                            "token_id": token,
                            "side": side,
                            "best_bid_price_micros": best_bid[0],
                            "best_bid_quantity_micros": best_bid[1],
                            "best_ask_price_micros": best_ask[0],
                            "best_ask_quantity_micros": best_ask[1],
                            "bid_level_count": len(bids),
                            "ask_level_count": len(asks),
                            "quality_status": quality,
                            "quality_reasons": quality_reasons,
                            "exact_duplicate_capture": duplicate,
                            "bids_json": canonical(bids),
                            "asks_json": canonical(asks),
                            "source_path": str(path.resolve()),
                            "source_line": line_number,
                        },
                        captured_at_ns,
                    )
                    source.evidence.selected_records += 1
                evidences.append(source.evidence)


def process_reference_snapshots(
    root: Path,
    start_ns: int,
    end_ns: int,
    output: CsvOutput,
    evidences: list[SourceEvidence],
) -> None:
    for path in source_files(root / "var" / "research-capture", "snapshots-*.jsonl"):
        source = FrozenJsonl(path)
        for line_number, wrapper in source.records():
            record = wrapper.get("record")
            if not isinstance(record, dict) or record.get("kind") != "observation":
                raise ExportError(f"invalid reference snapshot: {path}:{line_number}")
            event_ms = require_int(record.get("event_time_ms"), "reference event time")
            event_ns = event_ms * 1_000_000
            if not start_ns <= event_ns <= end_ns:
                continue
            payload = record.get("payload")
            if not isinstance(payload, dict):
                raise ExportError(f"reference payload is missing: {path}:{line_number}")
            output.write(
                {
                    "schema_version": SCHEMA_VERSION,
                    "event_time_ms": event_ms,
                    "event_time_utc": iso_ms(event_ms),
                    "sequence": record.get("sequence"),
                    "snapshot_digest": record.get("snapshot_digest"),
                    **{column: payload.get(column) for column in REFERENCE_COLUMNS[5:-2]},
                    "source_path": str(path.resolve()),
                    "source_line": line_number,
                },
                event_ns,
            )
            source.evidence.selected_records += 1
        evidences.append(source.evidence)


def process_coinbase(
    root: Path,
    start_ns: int,
    end_ns: int,
    output: CsvOutput,
    evidences: list[SourceEvidence],
) -> None:
    for path in source_files(root / "var" / "research-coinbase", "*.jsonl"):
        source = FrozenJsonl(path)
        for line_number, record in source.records():
            receive_ms = require_int(record.get("t_recv_ms"), "Coinbase receive time")
            receive_ns = receive_ms * 1_000_000
            if not start_ns <= receive_ns <= end_ns:
                continue
            event_ms = require_int(record.get("t_event_ms"), "Coinbase event time")
            output.write(
                {
                    "schema_version": SCHEMA_VERSION,
                    "receive_time_ms": receive_ms,
                    "receive_time_utc": iso_ms(receive_ms),
                    "event_time_ms": event_ms,
                    "event_time_utc": iso_ms(event_ms),
                    "symbol": record.get("symbol"),
                    "bid_raw": record.get("bid"),
                    "ask_raw": record.get("ask"),
                    "mid_raw": record.get("mid"),
                    "bid_size_raw": record.get("bid_sz"),
                    "ask_size_raw": record.get("ask_sz"),
                    "sequence": record.get("seq"),
                    "source": record.get("src"),
                    "source_path": str(path.resolve()),
                    "source_line": line_number,
                },
                receive_ns,
            )
            source.evidence.selected_records += 1
        evidences.append(source.evidence)


def current_trade_row(
    record: dict[str, Any], path: Path, line_number: int
) -> dict[str, Any]:
    event_ns = require_int(record.get("observed_at_ns"), "decision observed time")
    row = empty_trade()
    row.update(
        {
            "schema_version": SCHEMA_VERSION,
            "event_time_ns": event_ns,
            "event_time_utc": iso_ns(event_ns),
            "authority_class": "CURRENT_WINDOW_PAPER",
            "event_type": record.get("action"),
            "execution_mode": "SIMULATED_PAPER",
            "paper_authoritative": "true",
            "real_money": "false",
            "strategy_version": record.get("strategy_version"),
            "strategy_id": record.get("setup_id"),
            "strategy_status": "CURRENT_PAPER_EVIDENCE",
            "book": record.get("book"),
            "asset": record.get("asset"),
            "window": record.get("window"),
            "condition_id": record.get("condition_id"),
            "side": record.get("side"),
            "reason": record.get("reason"),
            "outcome": record.get("outcome"),
            "quantity_micros": record.get("quantity_micros"),
            "stake_micros": record.get("stake_micros"),
            "entry_price_micros": record.get("ask_micros"),
            "exit_bid_micros": record.get("bid_micros"),
            "realized_pnl_micros": record.get("realized_pnl_micros"),
            "payload_json": canonical(record),
            "source_path": str(path.resolve()),
            "source_line": line_number,
        }
    )
    return row


def process_decisions(
    root: Path,
    start_ns: int,
    end_ns: int,
    decisions_output: CsvOutput,
    trades_output: CsvOutput,
    closed_trades_output: CsvOutput,
    evidences: list[SourceEvidence],
) -> None:
    for path in source_files(root / "var" / "btc-window-paper", "decisions-*.jsonl"):
        source = FrozenJsonl(path)
        for line_number, record in source.records():
            event_ns = require_int(record.get("observed_at_ns"), "decision observed time")
            if not start_ns <= event_ns <= end_ns:
                continue
            if record.get("paper_only") is not True or record.get("real_money") is not False:
                raise ExportError(f"non-paper decision encountered: {path}:{line_number}")
            decisions_output.write(
                {
                    "schema_version": SCHEMA_VERSION,
                    "observed_at_utc": iso_ns(event_ns),
                    **{field: record.get(field) for field in DECISION_FIELDS},
                    "rejection_reasons_json": canonical(
                        record.get("rejection_reasons", [])
                    ),
                    "source_path": str(path.resolve()),
                    "source_line": line_number,
                },
                event_ns,
            )
            if record.get("action") in CURRENT_TRADE_ACTIONS:
                trade_row = current_trade_row(record, path, line_number)
                trades_output.write(trade_row, event_ns)
                if record.get("action") in {"exit", "settle"}:
                    closed_trades_output.write(trade_row, event_ns)
            source.evidence.selected_records += 1
        evidences.append(source.evidence)


def core_trade_row(
    record: dict[str, Any],
    digest: Any,
    path: Path,
    line_number: int,
) -> dict[str, Any]:
    event_ms = require_int(record.get("event_time_ms"), "core event time")
    payload = record.get("payload")
    if not isinstance(payload, dict):
        raise ExportError(f"core payload is missing: {path}:{line_number}")
    kind = text(record.get("kind"))
    row = empty_trade()
    row.update(
        {
            "schema_version": SCHEMA_VERSION,
            "event_time_ns": event_ms * 1_000_000,
            "event_time_utc": iso_ms(event_ms),
            "authority_class": "CORE_PAPER_WEEK",
            "event_type": kind,
            "execution_mode": "SIMULATED_PAPER",
            "paper_authoritative": "true",
            "real_money": "false",
            "strategy_version": "complete_set_core_v1",
            "strategy_id": (
                "complete_set_arbitrage_buy_pair"
                if kind in {"PAIR_BUY", "filled_pair"}
                else ""
            ),
            "strategy_status": "ACTIVE_CORE_PAPER_POLICY",
            "campaign_id": record.get("campaign_id"),
            "trade_id": payload.get("trade_id"),
            "asset": payload.get("asset"),
            "window": "1h",
            "condition_id": payload.get("condition_id"),
            "side": "UP+DOWN" if kind in {"PAIR_BUY", "filled_pair"} else "",
            "reason": payload.get("reason"),
            "quantity_micros": payload.get("quantity_micros"),
            "stake_micros": payload.get("cost_micros"),
            "up_price_micros": payload.get("up_price_micros"),
            "down_price_micros": payload.get("down_price_micros"),
            "pair_cost_micros": payload.get("pair_cost_micros"),
            "fee_micros": payload.get("fee_micros"),
            "slippage_micros": payload.get("slippage_micros"),
            "locked_pnl_micros": payload.get("locked_pnl_micros"),
            "record_digest": digest,
            "payload_json": canonical(payload),
            "source_path": str(path.resolve()),
            "source_line": line_number,
        }
    )
    return row


def process_core_paper(
    root: Path,
    start_ns: int,
    end_ns: int,
    core_output: CsvOutput,
    trades_output: CsvOutput,
    closed_trades_output: CsvOutput,
    evidences: list[SourceEvidence],
) -> None:
    for path in source_files(root / "var" / "paper-campaign", "paper-*.jsonl"):
        source = FrozenJsonl(path)
        for line_number, wrapper in source.records():
            record = wrapper.get("record")
            digest = wrapper.get("record_digest")
            if not isinstance(record, dict) or not isinstance(digest, str):
                raise ExportError(f"invalid core journal wrapper: {path}:{line_number}")
            # The core runtime uses BLAKE3. The Rust audit binary performs full
            # cryptographic verification; this export preserves its digest and
            # validates the structural financial boundary without substituting
            # another hash algorithm.
            event_ms = require_int(record.get("event_time_ms"), "core event time")
            event_ns = event_ms * 1_000_000
            if not start_ns <= event_ns <= end_ns:
                continue
            kind = text(record.get("kind"))
            if kind not in CORE_FINANCIAL_KINDS:
                continue
            payload = record.get("payload")
            if not isinstance(payload, dict):
                raise ExportError(f"core payload is missing: {path}:{line_number}")
            core_output.write(
                {
                    "schema_version": SCHEMA_VERSION,
                    "event_time_ms": event_ms,
                    "event_time_utc": iso_ms(event_ms),
                    "recorded_time_ms": record.get("recorded_time_ms"),
                    "campaign_id": record.get("campaign_id"),
                    "sequence": record.get("sequence"),
                    "stream": record.get("stream"),
                    "kind": kind,
                    "runtime_config_id": record.get("runtime_config_id"),
                    "runtime_config_digest": record.get("runtime_config_digest"),
                    "record_digest": digest,
                    "payload_json": canonical(payload),
                    "source_path": str(path.resolve()),
                    "source_line": line_number,
                },
                event_ns,
            )
            if kind in {"PAIR_BUY", "filled_pair"}:
                trade_row = core_trade_row(record, digest, path, line_number)
                trades_output.write(trade_row, event_ns)
                if kind == "filled_pair":
                    closed_trades_output.write(trade_row, event_ns)
            source.evidence.selected_records += 1
        evidences.append(source.evidence)


def process_legacy(
    root: Path,
    start_ns: int,
    end_ns: int,
    trades_output: CsvOutput,
    closed_trades_output: CsvOutput,
    evidences: list[SourceEvidence],
) -> None:
    live_dir = root / "var" / "live-paper"
    for path in source_files(live_dir, "bets-*.jsonl"):
        source = FrozenJsonl(path)
        for line_number, record in source.records():
            event_ms = require_int(
                record.get("logged_at_ms", record.get("settled_t")),
                "legacy settlement time",
            )
            event_ns = event_ms * 1_000_000
            if not start_ns <= event_ns <= end_ns:
                continue
            row = empty_trade()
            row.update(
                {
                    "schema_version": SCHEMA_VERSION,
                    "event_time_ns": event_ns,
                    "event_time_utc": iso_ms(event_ms),
                    "authority_class": "RETIRED_LEGACY_DIRECTIONAL",
                    "event_type": "settled",
                    "execution_mode": "SIMULATED_PAPER",
                    "paper_authoritative": "false",
                    "real_money": "false",
                    "strategy_id": record.get("strategy"),
                    "strategy_status": "RETIRED_UNSAFE_FLOATING_POINT_SIMULATOR",
                    "asset": record.get("asset"),
                    "window": record.get("window"),
                    "condition_id": record.get("condition_id"),
                    "side": record.get("side"),
                    "outcome": "win" if record.get("won") is True else "loss",
                    "entry_price_raw": record.get("price"),
                    "stake_raw": record.get("stake"),
                    "shares_raw": record.get("shares"),
                    "realized_pnl_raw": record.get("pnl"),
                    "bankroll_after_raw": record.get("bankroll"),
                    "payload_json": canonical(record),
                    "source_path": str(path.resolve()),
                    "source_line": line_number,
                }
            )
            trades_output.write(row, event_ns)
            closed_trades_output.write(row, event_ns)
            source.evidence.selected_records += 1
        evidences.append(source.evidence)
    for path in source_files(live_dir, "intraday-bets-*.jsonl"):
        source = FrozenJsonl(path)
        for line_number, record in source.records():
            event_ms = require_int(
                record.get("logged_at_ms", record.get("closed_ms")),
                "legacy intraday close time",
            )
            event_ns = event_ms * 1_000_000
            if not start_ns <= event_ns <= end_ns:
                continue
            row = empty_trade()
            row.update(
                {
                    "schema_version": SCHEMA_VERSION,
                    "event_time_ns": event_ns,
                    "event_time_utc": iso_ms(event_ms),
                    "authority_class": "RETIRED_LEGACY_INTRADAY",
                    "event_type": "closed",
                    "execution_mode": "SIMULATED_PAPER",
                    "paper_authoritative": "false",
                    "real_money": "false",
                    "strategy_id": "fixed_threshold_intraday_round_trip",
                    "strategy_status": "RETIRED_NEGATIVE_OOS",
                    "asset": record.get("asset"),
                    "window": record.get("window"),
                    "condition_id": record.get("condition_id"),
                    "side": record.get("side_token"),
                    "reason": record.get("exit_reason"),
                    "outcome": "win" if record.get("won") is True else "loss",
                    "entry_price_raw": record.get("entry_price"),
                    "exit_price_raw": record.get("exit_price"),
                    "stake_raw": record.get("stake"),
                    "shares_raw": record.get("shares"),
                    "realized_pnl_raw": record.get("pnl"),
                    "bankroll_after_raw": record.get("bankroll_after"),
                    "payload_json": canonical(record),
                    "source_path": str(path.resolve()),
                    "source_line": line_number,
                }
            )
            trades_output.write(row, event_ns)
            closed_trades_output.write(row, event_ns)
            source.evidence.selected_records += 1
        evidences.append(source.evidence)


def sha256_file(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def manifest_for(
    output_dir: Path,
    start_ns: int,
    end_ns: int,
    outputs: Iterable[CsvOutput],
    evidences: list[SourceEvidence],
) -> dict[str, Any]:
    files = []
    for output in sorted(outputs, key=lambda item: item.path.name):
        files.append(
            {
                "file": output.path.name,
                "bytes": output.path.stat().st_size,
                "rows": output.rows,
                "sha256": sha256_file(output.path),
                "min_time_ns": output.min_time_ns,
                "max_time_ns": output.max_time_ns,
            }
        )
    return {
        "schema_version": SCHEMA_VERSION,
        "interval": {
            "start_ns": start_ns,
            "start_utc": iso_ns(start_ns),
            "end_ns": end_ns,
            "end_utc": iso_ns(end_ns),
            "duration_hours": 48,
            "boundary": "inclusive",
        },
        "paper_only": True,
        "real_money_trades": 0,
        "derived_not_financial_authority": True,
        "output_files": files,
        "sources": [evidence.row() for evidence in evidences],
        "output_directory": str(output_dir.resolve()),
    }


def export(root: Path, output_root: Path, end_ns: int) -> Path:
    start_ns = end_ns - HOURS_48_NS
    label = (
        datetime.fromtimestamp(start_ns / 1_000_000_000, tz=timezone.utc).strftime(
            "%Y%m%dT%H%M%SZ"
        )
        + "_to_"
        + datetime.fromtimestamp(end_ns / 1_000_000_000, tz=timezone.utc).strftime(
            "%Y%m%dT%H%M%SZ"
        )
    )
    final_dir = output_root / label
    temporary = output_root / f".{label}.tmp-{os.getpid()}"
    if final_dir.exists() or temporary.exists():
        raise ExportError(f"output revision already exists: {final_dir}")
    temporary.mkdir(parents=True)
    outputs: list[CsvOutput] = []
    evidences: list[SourceEvidence] = []
    try:
        book_outputs = {
            (asset, window): CsvOutput(
                temporary / f"{asset}-{window}-full-depth-books.csv", BOOK_COLUMNS
            )
            for asset in ASSETS
            for window in WINDOWS
        }
        identities_output = CsvOutput(
            temporary / "market-identities.csv", IDENTITY_COLUMNS
        )
        reference_output = CsvOutput(
            temporary / "reference-snapshots.csv", REFERENCE_COLUMNS
        )
        coinbase_output = CsvOutput(
            temporary / "coinbase-reference.csv", COINBASE_COLUMNS
        )
        decisions_output = CsvOutput(
            temporary / "strategy-decisions.csv", DECISION_COLUMNS
        )
        trades_output = CsvOutput(temporary / "all-paper-trade-events.csv", TRADE_COLUMNS)
        closed_trades_output = CsvOutput(
            temporary / "closed-paper-trades.csv", TRADE_COLUMNS
        )
        core_output = CsvOutput(
            temporary / "core-paper-financial-events.csv", CORE_EVENT_COLUMNS
        )
        outputs.extend(book_outputs.values())
        outputs.extend(
            [
                identities_output,
                reference_output,
                coinbase_output,
                decisions_output,
                trades_output,
                closed_trades_output,
                core_output,
            ]
        )
        process_books(
            root,
            start_ns,
            end_ns,
            book_outputs,
            identities_output,
            evidences,
        )
        process_reference_snapshots(
            root, start_ns, end_ns, reference_output, evidences
        )
        process_coinbase(root, start_ns, end_ns, coinbase_output, evidences)
        process_decisions(
            root,
            start_ns,
            end_ns,
            decisions_output,
            trades_output,
            closed_trades_output,
            evidences,
        )
        process_core_paper(
            root,
            start_ns,
            end_ns,
            core_output,
            trades_output,
            closed_trades_output,
            evidences,
        )
        process_legacy(
            root,
            start_ns,
            end_ns,
            trades_output,
            closed_trades_output,
            evidences,
        )
        source_output = CsvOutput(temporary / "source-manifest.csv", SOURCE_COLUMNS)
        outputs.append(source_output)
        for evidence in evidences:
            source_output.write(evidence.row())
        for output in outputs:
            output.close()
        manifest = manifest_for(final_dir, start_ns, end_ns, outputs, evidences)
        manifest_path = temporary / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        with manifest_path.open("rb") as handle:
            os.fsync(handle.fileno())
        temporary.rename(final_dir)
        return final_dir
    except Exception:
        for output in outputs:
            if not output.handle.closed:
                output.handle.close()
        shutil.rmtree(temporary, ignore_errors=True)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument(
        "--output-root",
        type=Path,
        default=Path("var/research-export/last-48-hours"),
    )
    parser.add_argument("--end-ns", type=int)
    args = parser.parse_args()
    end_ns = args.end_ns if args.end_ns is not None else time.time_ns()
    if end_ns <= HOURS_48_NS:
        raise ExportError("end timestamp is invalid")
    result = export(args.root.resolve(), args.output_root, end_ns)
    manifest = json.loads((result / "manifest.json").read_text())
    print(
        json.dumps(
            {
                "output": str(result.resolve()),
                "interval": manifest["interval"],
                "files": {
                    item["file"]: item["rows"] for item in manifest["output_files"]
                },
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
