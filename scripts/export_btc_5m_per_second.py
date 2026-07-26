#!/usr/bin/env python3
"""Create an exact per-second BTC window dataset from local captures.

The source REST collector timestamps a batch before sequential Up/Down requests
finish. This exporter therefore uses a policy-bound availability delay. It
emits every second but never invents a book: missing and stale rows are explicit.
All financial values remain integer fixed point.
"""
from __future__ import annotations

import argparse
import bisect
import csv
import glob
import hashlib
import json
import os
import shutil
import tempfile
from dataclasses import dataclass
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any


class InvalidFiveMinuteCapture(RuntimeError):
    """The capture or policy cannot produce trustworthy research evidence."""


EXPORTER_SCHEMA_VERSION = 2


@dataclass(frozen=True)
class Identity:
    condition_id: str
    up_token_id: str
    down_token_id: str
    start_ms: int
    end_ms: int


@dataclass(frozen=True)
class Book:
    token_id: str
    condition_id: str
    batch_start_ns: int
    batch_start_ms: int
    available_at_ms: int
    bids: tuple[tuple[int, int], ...]
    asks: tuple[tuple[int, int], ...]
    source_path: str
    source_line: int


@dataclass(frozen=True)
class Reference:
    received_at_ms: int
    event_time_ms: int
    bid_micros: int
    ask_micros: int
    mid_micros: int
    source: str
    future_skew_ms: int


CONFIG_KEYS = {
    "schema_version",
    "asset",
    "window_ms",
    "seconds_per_window",
    "source_globs",
    "reference_globs",
    "output_root",
    "source_blob_root",
    "capture_availability_delay_ms",
    "maximum_book_age_ms",
    "maximum_reference_age_ms",
    "maximum_reference_future_skew_ms",
    "maximum_depth_levels",
    "maximum_source_files",
    "maximum_source_bytes",
}

SUPPORTED_WINDOWS = {
    300_000: ("5m", 300),
    900_000: ("15m", 900),
    3_600_000: ("1h", 3600),
}

CSV_COLUMNS = (
    "schema_version",
    "asset",
    "condition_id",
    "up_token_id",
    "down_token_id",
    "window_start_ms",
    "window_end_ms",
    "second_index",
    "second_start_ms",
    "sample_time_ms",
    "seconds_remaining",
    "market_ready",
    "row_status",
    "quality_flags",
    "up_observed_this_second",
    "down_observed_this_second",
    "up_batch_start_ms",
    "down_batch_start_ms",
    "up_batch_start_ns",
    "down_batch_start_ns",
    "up_available_at_ms",
    "down_available_at_ms",
    "up_age_ms",
    "down_age_ms",
    "up_fresh",
    "down_fresh",
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
    "up_bids_json",
    "up_asks_json",
    "down_bids_json",
    "down_asks_json",
    "reference_ready",
    "reference_source",
    "reference_event_time_ms",
    "reference_received_at_ms",
    "reference_age_ms",
    "reference_future_skew_ms",
    "btc_bid_micros",
    "btc_ask_micros",
    "btc_mid_micros",
)


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


def verified_tree_equal(left: Path, right: Path) -> bool:
    if left.is_symlink() or right.is_symlink() or not left.is_dir() or not right.is_dir():
        return False
    left_files = sorted(
        path.relative_to(left)
        for path in left.rglob("*")
        if path.is_file() and not path.is_symlink()
    )
    right_files = sorted(
        path.relative_to(right)
        for path in right.rglob("*")
        if path.is_file() and not path.is_symlink()
    )
    if left_files != right_files:
        return False
    return all(
        sha256_file(left / relative) == sha256_file(right / relative)
        for relative in left_files
    )


def positive_int(value: Any, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise InvalidFiveMinuteCapture(f"{name} must be a positive integer")
    return value


def nonnegative_int(value: Any, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise InvalidFiveMinuteCapture(f"{name} must be a non-negative integer")
    return value


def load_config(path: Path, project_root: Path) -> dict[str, Any]:
    raw = json.loads(path.read_text())
    if not isinstance(raw, dict) or set(raw) != CONFIG_KEYS:
        raise InvalidFiveMinuteCapture("configuration keys are not exact")
    if raw["schema_version"] != 1 or raw["asset"] != "BTC":
        raise InvalidFiveMinuteCapture("only schema 1 BTC export is supported")
    supported = SUPPORTED_WINDOWS.get(raw["window_ms"])
    if supported is None or raw["seconds_per_window"] != supported[1]:
        raise InvalidFiveMinuteCapture("window duration and second count are unsupported")
    for name in (
        "capture_availability_delay_ms",
        "maximum_book_age_ms",
        "maximum_reference_age_ms",
        "maximum_reference_future_skew_ms",
    ):
        nonnegative_int(raw[name], name)
    for name in (
        "maximum_depth_levels",
        "maximum_source_files",
        "maximum_source_bytes",
    ):
        positive_int(raw[name], name)
    for name in ("source_globs", "reference_globs"):
        patterns = raw[name]
        if not isinstance(patterns, list) or not patterns or not all(
            isinstance(pattern, str) and pattern for pattern in patterns
        ):
            raise InvalidFiveMinuteCapture(f"{name} must be a non-empty string list")
    output = (project_root / raw["output_root"]).resolve()
    blob_root = (project_root / raw["source_blob_root"]).resolve()
    try:
        output.relative_to(project_root.resolve())
        blob_root.relative_to(project_root.resolve())
    except ValueError as error:
        raise InvalidFiveMinuteCapture("output root escapes the project") from error
    raw["_output_root"] = output
    raw["_source_blob_root"] = blob_root
    return raw


def match_sources(
    project_root: Path,
    patterns: list[str],
    maximum_files: int,
    maximum_bytes: int,
) -> list[Path]:
    paths: set[Path] = set()
    for pattern in patterns:
        for match in glob.glob(str(project_root / pattern)):
            path = Path(match).resolve()
            try:
                path.relative_to(project_root.resolve())
            except ValueError as error:
                raise InvalidFiveMinuteCapture("source escapes project") from error
            if path.is_symlink() or not path.is_file():
                raise InvalidFiveMinuteCapture(f"source is not a regular file: {path}")
            if path.stat().st_size > maximum_bytes:
                raise InvalidFiveMinuteCapture(f"source exceeds byte budget: {path}")
            paths.add(path)
    if not paths or len(paths) > maximum_files:
        raise InvalidFiveMinuteCapture("source file count is invalid")
    return sorted(paths)


def freeze_source(path: Path, blob_root: Path, maximum_bytes: int) -> dict[str, Any]:
    source_size = path.stat().st_size
    if source_size <= 0 or source_size > maximum_bytes:
        raise InvalidFiveMinuteCapture(f"source size is invalid: {path}")
    with path.open("rb") as source:
        payload = source.read(source_size)
    if len(payload) != source_size:
        raise InvalidFiveMinuteCapture(f"source changed while freezing: {path}")
    newline = payload.rfind(b"\n")
    if newline < 0:
        raise InvalidFiveMinuteCapture(f"source has no complete JSONL row: {path}")
    frozen = payload[: newline + 1]
    digest = hashlib.sha256(frozen).hexdigest()
    blob_root.mkdir(parents=True, exist_ok=True)
    blob_path = blob_root / f"{digest}.jsonl"
    if blob_path.exists():
        if blob_path.is_symlink() or sha256_file(blob_path) != digest:
            raise InvalidFiveMinuteCapture("source blob checksum collision")
    else:
        temporary = blob_root / f".{digest}.{os.getpid()}.tmp"
        with temporary.open("xb") as output:
            output.write(frozen)
            output.flush()
            os.fsync(output.fileno())
        try:
            os.link(temporary, blob_path)
        except FileExistsError:
            pass
        finally:
            temporary.unlink(missing_ok=True)
        if sha256_file(blob_path) != digest:
            raise InvalidFiveMinuteCapture("published source blob checksum mismatch")
    return {
        "original_path": str(path),
        "original_size_at_freeze": source_size,
        "frozen_path": str(blob_path),
        "frozen_bytes": len(frozen),
        "excluded_incomplete_tail_bytes": source_size - len(frozen),
        "sha256": digest,
    }


def validate_id(value: Any, name: str, *, hexadecimal: bool) -> str:
    if not isinstance(value, str) or not value:
        raise InvalidFiveMinuteCapture(f"{name} is missing")
    if hexadecimal:
        if len(value) != 66 or not value.startswith("0x"):
            raise InvalidFiveMinuteCapture(f"{name} is not a condition id")
        try:
            int(value[2:], 16)
        except ValueError as error:
            raise InvalidFiveMinuteCapture(f"{name} is not hexadecimal") from error
    elif not value.isdigit():
        raise InvalidFiveMinuteCapture(f"{name} is not a token id")
    return value


def parse_identity(payload: Any, config: dict[str, Any]) -> Identity:
    if not isinstance(payload, dict) or payload.get("event_type") != "market_identity":
        raise InvalidFiveMinuteCapture("system row is not a market identity")
    if payload.get("asset") != "BTC":
        raise InvalidFiveMinuteCapture("non-BTC identity entered BTC source")
    condition = validate_id(payload.get("condition_id"), "condition_id", hexadecimal=True)
    up = validate_id(payload.get("up_token_id"), "up_token_id", hexadecimal=False)
    down = validate_id(payload.get("down_token_id"), "down_token_id", hexadecimal=False)
    if up == down:
        raise InvalidFiveMinuteCapture("Up and Down token identities are equal")
    start = nonnegative_int(payload.get("start_time_ms"), "start_time_ms")
    end = positive_int(payload.get("end_time_ms"), "end_time_ms")
    if end - start != config["window_ms"] or start % config["window_ms"] != 0:
        raise InvalidFiveMinuteCapture("identity is not an aligned configured window")
    return Identity(condition, up, down, start, end)


def parse_levels(
    value: Any, *, bids: bool, maximum_depth: int
) -> tuple[tuple[int, int], ...]:
    if not isinstance(value, list) or len(value) > maximum_depth:
        raise InvalidFiveMinuteCapture("book depth is invalid")
    levels: list[tuple[int, int]] = []
    for raw in value:
        if not isinstance(raw, list) or len(raw) != 2:
            raise InvalidFiveMinuteCapture("book level shape is invalid")
        price = nonnegative_int(raw[0], "price_micros")
        quantity = positive_int(raw[1], "quantity_micros")
        if price > 1_000_000:
            raise InvalidFiveMinuteCapture("binary price exceeds one dollar")
        levels.append((price, quantity))
    prices = [level[0] for level in levels]
    expected = sorted(prices, reverse=bids)
    if prices != expected or len(prices) != len(set(prices)):
        raise InvalidFiveMinuteCapture("book levels are unsorted or duplicated")
    return tuple(levels)


def parse_decimal_micros(value: Any, name: str) -> int:
    try:
        decimal = value if isinstance(value, Decimal) else Decimal(str(value))
        scaled = decimal * 1_000_000
    except (InvalidOperation, ValueError, TypeError) as error:
        raise InvalidFiveMinuteCapture(f"{name} is not decimal") from error
    integral = scaled.to_integral_value()
    if scaled != integral or integral < 0:
        raise InvalidFiveMinuteCapture(f"{name} loses fixed-point precision")
    return int(integral)


def load_identities(
    sources: list[Path], config: dict[str, Any]
) -> tuple[dict[str, Identity], int]:
    identities: dict[str, Identity] = {}
    duplicate_rows = 0
    for path in sources:
        with path.open() as source:
            for line_number, line in enumerate(source, 1):
                try:
                    row = json.loads(line)
                except json.JSONDecodeError as error:
                    raise InvalidFiveMinuteCapture(
                        f"invalid JSON at {path}:{line_number}"
                    ) from error
                if row.get("k") != "sys":
                    continue
                identity = parse_identity(row.get("payload"), config)
                previous = identities.get(identity.condition_id)
                if previous is not None:
                    if previous != identity:
                        raise InvalidFiveMinuteCapture("market identity equivocation")
                    duplicate_rows += 1
                identities[identity.condition_id] = identity
    if not identities:
        raise InvalidFiveMinuteCapture("no BTC configured-window identities found")
    return identities, duplicate_rows


def load_books(
    sources: list[Path],
    identities: dict[str, Identity],
    config: dict[str, Any],
) -> tuple[dict[tuple[str, str], list[Book]], dict[str, int], set[str]]:
    books: dict[tuple[str, str], list[Book]] = {}
    exact_seen: set[bytes] = set()
    counts = {
        "book_rows": 0,
        "exact_duplicate_books": 0,
        "outside_window_books": 0,
        "one_sided_books": 0,
        "same_time_equivocations": 0,
    }
    quarantined_conditions: set[str] = set()
    for path in sources:
        relative_path = str(path)
        with path.open() as source:
            for line_number, line in enumerate(source, 1):
                row = json.loads(line)
                if row.get("k") != "book":
                    continue
                counts["book_rows"] += 1
                condition = row.get("cond")
                identity = identities.get(condition)
                if identity is None:
                    raise InvalidFiveMinuteCapture("book references unknown condition")
                token = validate_id(row.get("tok"), "book token", hexadecimal=False)
                if token not in (identity.up_token_id, identity.down_token_id):
                    raise InvalidFiveMinuteCapture("book references unknown market token")
                timestamp_ns = nonnegative_int(row.get("t"), "book timestamp_ns")
                batch_start_ms = timestamp_ns // 1_000_000
                if not (identity.start_ms <= batch_start_ms < identity.end_ms):
                    counts["outside_window_books"] += 1
                    continue
                bids = parse_levels(
                    row.get("bids"), bids=True,
                    maximum_depth=config["maximum_depth_levels"],
                )
                asks = parse_levels(
                    row.get("asks"), bids=False,
                    maximum_depth=config["maximum_depth_levels"],
                )
                if not bids or not asks:
                    counts["one_sided_books"] += 1
                elif bids[0][0] >= asks[0][0]:
                    raise InvalidFiveMinuteCapture("crossed book entered capture")
                fingerprint = canonical({
                    "condition": condition,
                    "token": token,
                    "timestamp_ns": timestamp_ns,
                    "bids": bids,
                    "asks": asks,
                })
                if fingerprint in exact_seen:
                    counts["exact_duplicate_books"] += 1
                    continue
                exact_seen.add(fingerprint)
                book = Book(
                    token,
                    condition,
                    timestamp_ns,
                    batch_start_ms,
                    batch_start_ms + config["capture_availability_delay_ms"],
                    bids,
                    asks,
                    relative_path,
                    line_number,
                )
                books.setdefault((condition, token), []).append(book)
    for key, values in books.items():
        values.sort(
            key=lambda item: (
                item.available_at_ms,
                item.batch_start_ms,
                item.batch_start_ns,
                item.source_path,
                item.source_line,
            )
        )
        seen_times: dict[int, Book] = {}
        for book in values:
            previous = seen_times.get(book.batch_start_ns)
            if previous is not None and (
                previous.bids != book.bids or previous.asks != book.asks
            ):
                counts["same_time_equivocations"] += 1
                quarantined_conditions.add(book.condition_id)
            seen_times[book.batch_start_ns] = book
    for condition in quarantined_conditions:
        for token in (
            identities[condition].up_token_id,
            identities[condition].down_token_id,
        ):
            books.pop((condition, token), None)
    return books, counts, quarantined_conditions


def load_references(
    sources: list[Path], config: dict[str, Any]
) -> tuple[list[Reference], dict[str, int]]:
    references: list[Reference] = []
    seen: dict[tuple[int, int], Reference] = {}
    counts = {"reference_rows": 0, "reference_future_skew_rows": 0}
    for path in sources:
        with path.open() as source:
            for line_number, line in enumerate(source, 1):
                try:
                    row = json.loads(line, parse_float=Decimal)
                except json.JSONDecodeError as error:
                    raise InvalidFiveMinuteCapture(
                        f"invalid reference JSON at {path}:{line_number}"
                    ) from error
                if row.get("symbol") != "BTCUSD":
                    raise InvalidFiveMinuteCapture("non-BTCUSD reference entered source")
                counts["reference_rows"] += 1
                received = nonnegative_int(row.get("t_recv_ms"), "reference receive time")
                event = nonnegative_int(row.get("t_event_ms"), "reference event time")
                future_skew = max(0, event - received)
                if future_skew > config["maximum_reference_future_skew_ms"]:
                    raise InvalidFiveMinuteCapture(
                        "reference future skew exceeds policy"
                    )
                if future_skew:
                    counts["reference_future_skew_rows"] += 1
                reference = Reference(
                    received,
                    event,
                    parse_decimal_micros(row.get("bid"), "reference bid"),
                    parse_decimal_micros(row.get("ask"), "reference ask"),
                    parse_decimal_micros(row.get("mid"), "reference mid"),
                    str(row.get("src") or "unknown"),
                    future_skew,
                )
                if reference.bid_micros > reference.ask_micros:
                    raise InvalidFiveMinuteCapture("crossed reference quote")
                key = (received, event)
                previous = seen.get(key)
                if previous is not None and previous != reference:
                    raise InvalidFiveMinuteCapture("reference timestamp equivocation")
                if previous is None:
                    seen[key] = reference
                    references.append(reference)
    references.sort(key=lambda item: (item.received_at_ms, item.event_time_ms))
    return references, counts


def bool_text(value: bool) -> str:
    return "true" if value else "false"


def level_value(book: Book | None, side: str, index: int) -> str:
    if book is None:
        return ""
    values = book.bids if side == "bid" else book.asks
    return str(values[0][index]) if values else ""


def depth_json(book: Book | None, side: str) -> str:
    if book is None:
        return ""
    values = book.bids if side == "bids" else book.asks
    return canonical([list(level) for level in values]).decode()


def latest_at(
    values: list[Any],
    times: list[int],
    sample_time_ms: int,
) -> Any | None:
    index = bisect.bisect_right(times, sample_time_ms) - 1
    return values[index] if index >= 0 else None


def row_status(
    up: Book | None,
    down: Book | None,
    up_fresh: bool,
    down_fresh: bool,
) -> tuple[str, list[str]]:
    flags: list[str] = []
    if up is None:
        flags.append("missing_up")
    elif not up.bids or not up.asks:
        flags.append("one_sided_up")
    elif not up_fresh:
        flags.append("stale_up")
    if down is None:
        flags.append("missing_down")
    elif not down.bids or not down.asks:
        flags.append("one_sided_down")
    elif not down_fresh:
        flags.append("stale_down")
    return ("READY" if not flags else "|".join(flag.upper() for flag in flags), flags)


def atomic_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    with temporary.open("xb") as output:
        output.write(canonical(value) + b"\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)


def export(config_path: Path) -> dict[str, Any]:
    project_root = Path(__file__).resolve().parent.parent
    config = load_config(config_path.resolve(), project_root)
    live_sources = match_sources(
        project_root,
        config["source_globs"],
        config["maximum_source_files"],
        config["maximum_source_bytes"],
    )
    live_reference_sources = match_sources(
        project_root,
        config["reference_globs"],
        config["maximum_source_files"],
        config["maximum_source_bytes"],
    )
    source_evidence = [
        freeze_source(
            path, config["_source_blob_root"], config["maximum_source_bytes"]
        )
        for path in live_sources
    ]
    reference_evidence = [
        freeze_source(
            path, config["_source_blob_root"], config["maximum_source_bytes"]
        )
        for path in live_reference_sources
    ]
    sources = [Path(evidence["frozen_path"]) for evidence in source_evidence]
    reference_sources = [
        Path(evidence["frozen_path"]) for evidence in reference_evidence
    ]
    public_config = {key: config[key] for key in CONFIG_KEYS}
    window_label = SUPPORTED_WINDOWS[config["window_ms"]][0]
    revision = hashlib.sha256(canonical({
        "exporter_schema_version": EXPORTER_SCHEMA_VERSION,
        "config": public_config,
        "sources": source_evidence,
        "reference_sources": reference_evidence,
    })).hexdigest()
    output_root: Path = config["_output_root"]
    final_root = output_root / revision
    manifest_path = final_root / "manifest.json"

    identities, duplicate_identity_rows = load_identities(sources, config)
    books, book_counts, quarantined_conditions = load_books(
        sources, identities, config
    )
    capture_watermark_ms = max(
        (
            book.batch_start_ms
            for condition_books in books.values()
            for book in condition_books
        ),
        default=None,
    )
    if capture_watermark_ms is None:
        raise InvalidFiveMinuteCapture("capture has no usable book watermark")
    unfinished_conditions = {
        condition
        for condition, identity in identities.items()
        if identity.end_ms > capture_watermark_ms
    }
    accepted_identities = {
        condition: identity
        for condition, identity in identities.items()
        if condition not in quarantined_conditions
        and condition not in unfinished_conditions
    }
    if not accepted_identities:
        raise InvalidFiveMinuteCapture(
            "all BTC configured-window identities were quarantined"
        )
    references, reference_counts = load_references(reference_sources, config)
    reference_times = [reference.received_at_ms for reference in references]

    output_root.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".staging-", dir=output_root))
    partition_handles: dict[str, tuple[Any, csv.DictWriter]] = {}
    partition_counts: dict[str, dict[str, int]] = {}
    market_rows: list[dict[str, Any]] = []
    total_rows = 0
    ready_rows = 0
    reference_ready_rows = 0
    try:
        for identity in sorted(
            accepted_identities.values(),
            key=lambda item: (item.start_ms, item.condition_id),
        ):
            up_values = books.get((identity.condition_id, identity.up_token_id), [])
            down_values = books.get((identity.condition_id, identity.down_token_id), [])
            up_times = [book.available_at_ms for book in up_values]
            down_times = [book.available_at_ms for book in down_values]
            up_observed_seconds = {book.batch_start_ms // 1000 for book in up_values}
            down_observed_seconds = {book.batch_start_ms // 1000 for book in down_values}
            date_hour = datetime.fromtimestamp(
                identity.start_ms / 1000,
                tz=timezone.utc,
            )
            partition = f"{date_hour:%Y-%m-%d}/{date_hour:%H}"
            if partition not in partition_handles:
                directory = staging / "BTC-data" / f"{date_hour:%Y-%m-%d}" / f"{date_hour:%H}"
                directory.mkdir(parents=True, exist_ok=True)
                handle = (directory / "observations.csv").open(
                    "x", newline="", encoding="utf-8"
                )
                writer = csv.DictWriter(handle, fieldnames=CSV_COLUMNS)
                writer.writeheader()
                partition_handles[partition] = (handle, writer)
                partition_counts[partition] = {"rows": 0, "ready_rows": 0}
            handle, writer = partition_handles[partition]
            market_ready_rows = 0
            market_reference_rows = 0
            for second_index in range(config["seconds_per_window"]):
                second_start_ms = identity.start_ms + second_index * 1000
                sample_time_ms = second_start_ms + 999
                up = latest_at(up_values, up_times, sample_time_ms)
                down = latest_at(down_values, down_times, sample_time_ms)
                up_age = sample_time_ms - up.available_at_ms if up else None
                down_age = sample_time_ms - down.available_at_ms if down else None
                up_fresh = (
                    up is not None
                    and bool(up.bids)
                    and bool(up.asks)
                    and up_age <= config["maximum_book_age_ms"]
                )
                down_fresh = (
                    down is not None
                    and bool(down.bids)
                    and bool(down.asks)
                    and down_age <= config["maximum_book_age_ms"]
                )
                market_ready = up_fresh and down_fresh
                status, flags = row_status(up, down, up_fresh, down_fresh)
                reference = latest_at(
                    references, reference_times, sample_time_ms
                )
                reference_age = (
                    sample_time_ms - reference.received_at_ms if reference else None
                )
                reference_ready = (
                    reference is not None
                    and reference_age <= config["maximum_reference_age_ms"]
                )
                if reference is None:
                    flags.append("missing_reference")
                elif not reference_ready:
                    flags.append("stale_reference")
                if reference is not None and reference.future_skew_ms:
                    flags.append("reference_future_clock_skew")
                row = {
                    "schema_version": "1",
                    "asset": "BTC",
                    "condition_id": identity.condition_id,
                    "up_token_id": identity.up_token_id,
                    "down_token_id": identity.down_token_id,
                    "window_start_ms": str(identity.start_ms),
                    "window_end_ms": str(identity.end_ms),
                    "second_index": str(second_index),
                    "second_start_ms": str(second_start_ms),
                    "sample_time_ms": str(sample_time_ms),
                    "seconds_remaining": str(
                        config["seconds_per_window"] - 1 - second_index
                    ),
                    "market_ready": bool_text(market_ready),
                    "row_status": status,
                    "quality_flags": "|".join(flags) if flags else "ok",
                    "up_observed_this_second": bool_text(
                        second_start_ms // 1000 in up_observed_seconds
                    ),
                    "down_observed_this_second": bool_text(
                        second_start_ms // 1000 in down_observed_seconds
                    ),
                    "up_batch_start_ms": str(up.batch_start_ms) if up else "",
                    "down_batch_start_ms": str(down.batch_start_ms) if down else "",
                    "up_batch_start_ns": str(up.batch_start_ns) if up else "",
                    "down_batch_start_ns": str(down.batch_start_ns) if down else "",
                    "up_available_at_ms": str(up.available_at_ms) if up else "",
                    "down_available_at_ms": str(down.available_at_ms) if down else "",
                    "up_age_ms": str(up_age) if up_age is not None else "",
                    "down_age_ms": str(down_age) if down_age is not None else "",
                    "up_fresh": bool_text(up_fresh),
                    "down_fresh": bool_text(down_fresh),
                    "up_best_bid_micros": level_value(up, "bid", 0),
                    "up_best_ask_micros": level_value(up, "ask", 0),
                    "up_best_bid_quantity_micros": level_value(up, "bid", 1),
                    "up_best_ask_quantity_micros": level_value(up, "ask", 1),
                    "down_best_bid_micros": level_value(down, "bid", 0),
                    "down_best_ask_micros": level_value(down, "ask", 0),
                    "down_best_bid_quantity_micros": level_value(down, "bid", 1),
                    "down_best_ask_quantity_micros": level_value(down, "ask", 1),
                    "up_spread_micros": (
                        str(up.asks[0][0] - up.bids[0][0])
                        if up and up.bids and up.asks else ""
                    ),
                    "down_spread_micros": (
                        str(down.asks[0][0] - down.bids[0][0])
                        if down and down.bids and down.asks else ""
                    ),
                    "buy_pair_cost_micros": (
                        str(up.asks[0][0] + down.asks[0][0])
                        if up and down and up.asks and down.asks else ""
                    ),
                    "sell_pair_proceeds_micros": (
                        str(up.bids[0][0] + down.bids[0][0])
                        if up and down and up.bids and down.bids else ""
                    ),
                    "up_bids_json": depth_json(up, "bids"),
                    "up_asks_json": depth_json(up, "asks"),
                    "down_bids_json": depth_json(down, "bids"),
                    "down_asks_json": depth_json(down, "asks"),
                    "reference_ready": bool_text(reference_ready),
                    "reference_source": reference.source if reference else "",
                    "reference_event_time_ms": (
                        str(reference.event_time_ms) if reference else ""
                    ),
                    "reference_received_at_ms": (
                        str(reference.received_at_ms) if reference else ""
                    ),
                    "reference_age_ms": (
                        str(reference_age) if reference_age is not None else ""
                    ),
                    "reference_future_skew_ms": (
                        str(reference.future_skew_ms) if reference else ""
                    ),
                    "btc_bid_micros": (
                        str(reference.bid_micros) if reference else ""
                    ),
                    "btc_ask_micros": (
                        str(reference.ask_micros) if reference else ""
                    ),
                    "btc_mid_micros": (
                        str(reference.mid_micros) if reference else ""
                    ),
                }
                writer.writerow(row)
                total_rows += 1
                partition_counts[partition]["rows"] += 1
                if market_ready:
                    ready_rows += 1
                    market_ready_rows += 1
                    partition_counts[partition]["ready_rows"] += 1
                if reference_ready:
                    reference_ready_rows += 1
                    market_reference_rows += 1
            market_rows.append({
                "condition_id": identity.condition_id,
                "window_start_ms": identity.start_ms,
                "window_end_ms": identity.end_ms,
                "rows": config["seconds_per_window"],
                "up_book_observations": len(up_values),
                "down_book_observations": len(down_values),
                "market_ready_rows": market_ready_rows,
                "reference_ready_rows": market_reference_rows,
            })
        for handle, _ in partition_handles.values():
            handle.flush()
            os.fsync(handle.fileno())
            handle.close()
        partition_handles.clear()

        index_path = staging / "market-index.csv"
        with index_path.open("x", newline="", encoding="utf-8") as output:
            writer = csv.DictWriter(output, fieldnames=tuple(market_rows[0]))
            writer.writeheader()
            writer.writerows(market_rows)
            output.flush()
            os.fsync(output.fileno())

        partition_manifests = []
        for partition, counts in sorted(partition_counts.items()):
            csv_path = staging / "BTC-data" / partition / "observations.csv"
            partition_manifest = {
                "schema_version": 1,
                "asset": "BTC",
                "window": window_label,
                "utc_partition": partition,
                **counts,
                "observations_csv_sha256": sha256_file(csv_path),
                "derived_not_financial_authority": True,
            }
            atomic_json(csv_path.parent / "manifest.json", partition_manifest)
            partition_manifests.append(partition_manifest)
        manifest = {
            "schema_version": 1,
            "exporter_schema_version": EXPORTER_SCHEMA_VERSION,
            "asset": "BTC",
            "window": window_label,
            "revision": revision,
            "policy": public_config,
            "sources": source_evidence,
            "reference_sources": reference_evidence,
            "markets_discovered": len(identities),
            "markets": len(accepted_identities),
            "quarantined_conditions": sorted(quarantined_conditions),
            "unfinished_conditions_excluded": sorted(unfinished_conditions),
            "capture_watermark_ms": capture_watermark_ms,
            "rows": total_rows,
            "expected_rows": (
                len(accepted_identities) * config["seconds_per_window"]
            ),
            "market_ready_rows": ready_rows,
            "reference_ready_rows": reference_ready_rows,
            "duplicate_identity_rows": duplicate_identity_rows,
            **book_counts,
            **reference_counts,
            "partitions": partition_manifests,
            "market_index_sha256": sha256_file(index_path),
            "derived_not_financial_authority": True,
        }
        atomic_json(staging / "manifest.json", manifest)
        if final_root.exists():
            if not verified_tree_equal(staging, final_root):
                raise InvalidFiveMinuteCapture(
                    "existing dataset revision differs from deterministic output"
                )
            shutil.rmtree(staging)
            manifest["idempotent_reuse"] = True
        else:
            os.replace(staging, final_root)
        atomic_json(output_root / "latest.json", {
            "schema_version": 1,
            "revision": revision,
            "dataset_path": str(final_root),
            "manifest_sha256": sha256_file(manifest_path),
        })
        return manifest
    except Exception:
        for handle, _ in partition_handles.values():
            handle.close()
        if staging.exists():
            shutil.rmtree(staging)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--config",
        type=Path,
        default=Path("config/btc-5m-per-second-export.example.json"),
    )
    arguments = parser.parse_args()
    try:
        report = export(arguments.config)
        print(json.dumps({
            key: report[key]
            for key in (
                "revision",
                "markets",
                "rows",
                "expected_rows",
                "market_ready_rows",
                "reference_ready_rows",
            )
        }, sort_keys=True))
        return 0
    except (OSError, ValueError, InvalidFiveMinuteCapture) as error:
        print(json.dumps({"error": str(error)}, sort_keys=True))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
