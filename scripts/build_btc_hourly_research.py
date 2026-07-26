#!/usr/bin/env python3
"""Create conservative BTC one-hour intraday L2 research data.

The builder is read-only. It cannot train or promote a model, modify paper
trading, access credentials, or place an order. Financial values stay in integer
micros and returns in integer basis points.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from bisect import bisect_left
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any

MICROS = 1_000_000
BPS = 10_000


class InvalidTape(ValueError):
    """The input cannot support trustworthy derived research."""


@dataclass(frozen=True)
class Policy:
    asset: str
    window_ms: int
    decision_spacing_ms: int
    strategy_latency_ms: int
    max_source_time_lead_ms: int
    start_buffer_ms: int
    end_buffer_ms: int
    max_future_frame_gap_ms: int
    entry_shares_micros: int
    taker_fee_reserve_bps: int
    minimum_completed_sessions: int
    horizons_ms: tuple[int, ...]
    digest: str


@dataclass(frozen=True)
class Market:
    condition_id: str
    start_ms: int
    end_ms: int
    up_token: str
    down_token: str


@dataclass(frozen=True)
class Frame:
    condition_id: str
    side: str
    source_sequence: int
    event_ns: int
    received_ns: int | None
    available_ns: int
    entry_cost_micros: int
    exit_proceeds_micros: int
    bid_depth_micros: int
    ask_depth_micros: int
    bid_levels: int
    ask_levels: int


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def integer(value: Any, field: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise InvalidTape(f"{field} must be an integer")
    return value


def read_policy(path: Path) -> Policy:
    raw = json.loads(path.read_text())
    required = {
        "schema_version", "asset", "window_ms", "decision_spacing_ms",
        "strategy_latency_ms", "max_source_time_lead_ms", "start_buffer_ms", "end_buffer_ms",
        "max_future_frame_gap_ms", "entry_shares_micros",
        "taker_fee_reserve_bps", "minimum_completed_sessions", "horizons_ms",
    }
    if set(raw) != required or raw["schema_version"] != 1 or raw["asset"] != "BTC":
        raise InvalidTape("invalid research policy schema")
    int_keys = required - {"schema_version", "asset", "horizons_ms"}
    if any(integer(raw[key], key) <= 0 for key in int_keys):
        raise InvalidTape("policy has non-positive value")
    horizons = tuple(raw["horizons_ms"])
    if (not horizons or tuple(sorted(horizons)) != horizons
            or any(integer(value, "horizon") <= 0 for value in horizons)):
        raise InvalidTape("policy horizons are invalid")
    if raw["window_ms"] != 3_600_000 or raw["taker_fee_reserve_bps"] > BPS:
        raise InvalidTape("policy window or fee reserve is invalid")
    return Policy(
        asset=raw["asset"], window_ms=raw["window_ms"],
        decision_spacing_ms=raw["decision_spacing_ms"],
        strategy_latency_ms=raw["strategy_latency_ms"],
        max_source_time_lead_ms=raw["max_source_time_lead_ms"],
        start_buffer_ms=raw["start_buffer_ms"], end_buffer_ms=raw["end_buffer_ms"],
        max_future_frame_gap_ms=raw["max_future_frame_gap_ms"],
        entry_shares_micros=raw["entry_shares_micros"],
        taker_fee_reserve_bps=raw["taker_fee_reserve_bps"],
        minimum_completed_sessions=raw["minimum_completed_sessions"],
        horizons_ms=horizons,
        digest=hashlib.sha256(canonical_bytes(raw)).hexdigest(),
    )


def identity(row: dict[str, Any], policy: Policy) -> Market | None:
    payload = row.get("payload")
    if not isinstance(payload, dict) or payload.get("event_type") != "market_identity":
        return None
    if payload.get("asset") != policy.asset:
        return None
    condition = payload.get("condition_id")
    up = payload.get("up_token_id")
    down = payload.get("down_token_id")
    if not all(isinstance(value, str) and value for value in (condition, up, down)):
        raise InvalidTape("identity has missing token or condition")
    start = integer(payload.get("start_time_ms"), "start_time_ms")
    end = integer(payload.get("end_time_ms"), "end_time_ms")
    if end - start != policy.window_ms:
        return None
    return Market(condition, start, end, up, down)


def load_identities(path: Path, policy: Policy) -> tuple[dict[str, Market], dict[str, tuple[str, str]]]:
    markets: dict[str, Market] = {}
    tokens: dict[str, tuple[str, str]] = {}
    with path.open() as tape:
        for number, line in enumerate(tape, 1):
            try:
                row = json.loads(line)
            except json.JSONDecodeError as error:
                raise InvalidTape(f"line {number}: invalid JSON") from error
            if row.get("k") != "sys":
                continue
            market = identity(row, policy)
            if market is None:
                continue
            if market.condition_id in markets and markets[market.condition_id] != market:
                raise InvalidTape("conflicting immutable identity")
            markets[market.condition_id] = market
            for token, side in ((market.up_token, "UP"), (market.down_token, "DOWN")):
                candidate = (market.condition_id, side)
                if token in tokens and tokens[token] != candidate:
                    raise InvalidTape("token has conflicting identity")
                tokens[token] = candidate
    return markets, tokens


def walk(levels: Any, quantity: int, buy: bool) -> tuple[int, int, int] | None:
    """Return consideration, total displayed depth, and levels consumed."""
    if not isinstance(levels, list):
        return None
    remaining, consideration, depth, used = quantity, 0, 0, 0
    last: int | None = None
    for level in levels:
        if not isinstance(level, list) or len(level) != 2:
            return None
        price, size = integer(level[0], "book price"), integer(level[1], "book quantity")
        if price <= 0 or price > MICROS or size < 0:
            return None
        if last is not None and ((buy and price < last) or (not buy and price > last)):
            return None
        last = price
        depth += size
        take = min(remaining, size)
        if take:
            product = price * take
            consideration += ((product + MICROS - 1) // MICROS if buy else product // MICROS)
            remaining -= take
            used += 1
        if remaining == 0:
            return consideration, depth, used
    return None


def collect_frames(path: Path, policy: Policy, markets: dict[str, Market],
                   tokens: dict[str, tuple[str, str]]) -> dict[tuple[str, str], list[Frame]]:
    frames: dict[tuple[str, str], list[Frame]] = defaultdict(list)
    next_sample: dict[tuple[str, str], int] = {}
    received_history: dict[tuple[str, str], int] = {}
    with path.open() as tape:
        for number, line in enumerate(tape, 1):
            try:
                row = json.loads(line)
            except json.JSONDecodeError as error:
                raise InvalidTape(f"line {number}: invalid JSON") from error
            if row.get("k") != "book":
                continue
            token = row.get("tok")
            if not isinstance(token, str) or token not in tokens:
                continue
            condition, side = tokens[token]
            if row.get("cond") != condition:
                raise InvalidTape("book condition/token mismatch")
            event_ns = integer(row.get("t"), "event time")
            raw_received = row.get("r")
            if raw_received is None:
                if row.get("rq") != "UNAVAILABLE_EXTERNAL_ARCHIVE":
                    raise InvalidTape("missing receive time lacks explicit quality")
                received_ns = None
                ordering_ns = event_ns
                available_base_ns = integer(row.get("a"), "assumed available time")
                if available_base_ns < event_ns:
                    raise InvalidTape("assumed available time precedes event")
            else:
                received_ns = integer(raw_received, "received time")
                ordering_ns = received_ns
                available_base_ns = received_ns
            sequence = integer(row.get("seq"), "sequence")
            if event_ns < 0 or ordering_ns < 0:
                raise InvalidTape("book timestamps invalid")
            if received_ns is not None and event_ns - received_ns > policy.max_source_time_lead_ms * MICROS:
                raise InvalidTape("book source clock lead exceeds policy")
            key = (condition, side)
            if key in received_history and ordering_ns < received_history[key]:
                raise InvalidTape("book receive timestamp regressed")
            received_history[key] = ordering_ns
            market = markets[condition]
            available_ns = available_base_ns + policy.strategy_latency_ms * MICROS
            available_ms = available_ns // MICROS
            lo, hi = market.start_ms + policy.start_buffer_ms, market.end_ms - policy.end_buffer_ms
            if available_ms < lo or available_ms > hi:
                continue
            scheduled = next_sample.setdefault(key, lo)
            if available_ms < scheduled:
                continue
            next_sample[key] = lo + ((available_ms - lo) // policy.decision_spacing_ms + 1) * policy.decision_spacing_ms
            buy = walk(row.get("asks"), policy.entry_shares_micros, True)
            sell = walk(row.get("bids"), policy.entry_shares_micros, False)
            if buy is None or sell is None:
                continue
            cost, ask_depth, ask_levels = buy
            proceeds, bid_depth, bid_levels = sell
            frames[key].append(Frame(
                condition, side, sequence, event_ns, received_ns, available_ns,
                cost, proceeds, bid_depth, ask_depth, bid_levels, ask_levels,
            ))
    return frames


def net_return_bps(cost: int, proceeds: int, fee_bps: int) -> int:
    fee = (cost * fee_bps + BPS - 1) // BPS
    return ((proceeds - cost - fee) * BPS) // cost


def label(series: list[Frame], index: int, policy: Policy) -> dict[str, Any] | None:
    current = series[index]
    timestamps = [frame.available_ns for frame in series]
    result: dict[str, Any] = {}
    for horizon_ms in policy.horizons_ms:
        target = current.available_ns + horizon_ms * MICROS
        future_index = bisect_left(timestamps, target, lo=index + 1)
        if future_index == len(series):
            return None
        future = series[future_index]
        if future.available_ns - target > policy.max_future_frame_gap_ms * MICROS:
            return None
        result[str(horizon_ms)] = {
            "exit_source_sequence": future.source_sequence,
            "exit_available_ns": future.available_ns,
            "exit_proceeds_micros": str(future.exit_proceeds_micros),
            "net_return_bps": net_return_bps(
                current.entry_cost_micros, future.exit_proceeds_micros,
                policy.taker_fee_reserve_bps,
            ),
        }
    return result


def write_jsonl(path: Path, rows: list[dict[str, Any]]) -> str:
    digest = hashlib.sha256()
    with path.open("xb") as output:
        for row in rows:
            encoded = canonical_bytes(row) + b"\n"
            output.write(encoded)
            digest.update(encoded)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, default=Path("var/research-l2/clob-l2.jsonl"))
    parser.add_argument("--policy", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    args = parser.parse_args()
    try:
        policy = read_policy(args.policy)
        markets, tokens = load_identities(args.input, policy)
        frames = collect_frames(args.input, policy, markets, tokens)
    except (OSError, InvalidTape, json.JSONDecodeError) as error:
        print(f"REJECTED: {error}", file=sys.stderr)
        return 2
    rows_by_session: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for (condition, side), series in frames.items():
        market = markets[condition]
        for index, frame in enumerate(series):
            future = label(series, index, policy)
            if future is None:
                continue
            rows_by_session[condition].append({
                "schema_version": 1, "non_authoritative": True, "asset": policy.asset,
                "condition_id": condition, "side": side,
                "session_start_ms": market.start_ms, "session_end_ms": market.end_ms,
                "source_sequence": frame.source_sequence, "event_time_ns": frame.event_ns,
                "received_time_ns": frame.received_ns,
                "receive_time_quality": (
                    "OBSERVED" if frame.received_ns is not None
                    else "UNAVAILABLE_EXTERNAL_ARCHIVE"
                ),
                "strategy_available_time_ns": frame.available_ns,
                "time_to_expiry_ms": max(
                    0, market.end_ms - frame.available_ns // MICROS
                ),
                "session_progress_bps": min(
                    BPS,
                    max(
                        0,
                        ((frame.available_ns // MICROS - market.start_ms) * BPS)
                        // policy.window_ms,
                    ),
                ),
                "source_clock_lead_ns": (
                    max(0, frame.event_ns - frame.received_ns)
                    if frame.received_ns is not None else None
                ),
                "entry_cost_micros": str(frame.entry_cost_micros),
                "current_exit_proceeds_micros": str(frame.exit_proceeds_micros),
                "bid_depth_micros": str(frame.bid_depth_micros),
                "ask_depth_micros": str(frame.ask_depth_micros),
                "bid_levels_consumed": frame.bid_levels,
                "ask_levels_consumed": frame.ask_levels,
                "target_shares_micros": str(policy.entry_shares_micros),
                "fee_reserve_bps": policy.taker_fee_reserve_bps,
                "policy_digest": policy.digest, "future": future,
            })
    sessions = sorted(
        (key for key, rows in rows_by_session.items() if rows),
        key=lambda key: (markets[key].start_ms, key),
    )
    args.out_dir.mkdir(parents=True, exist_ok=False)
    report: dict[str, Any] = {
        "schema_version": 1, "non_authoritative": True, "asset": policy.asset,
        "policy_digest": policy.digest, "input": str(args.input),
        "sessions_with_rows": len(sessions),
        "minimum_completed_sessions": policy.minimum_completed_sessions,
    }
    if len(sessions) < policy.minimum_completed_sessions:
        report["eligibility"] = "NO_MODEL_INSUFFICIENT_SESSIONS"
        (args.out_dir / "NO_MODEL.json").write_bytes(canonical_bytes(report) + b"\n")
        print(json.dumps(report, indent=2))
        return 0
    train_end, validation_end = len(sessions) * 60 // 100, len(sessions) * 80 // 100
    folds = {
        "train": sessions[:train_end],
        "validation": sessions[train_end:validation_end],
        "test": sessions[validation_end:],
    }
    if any(not values for values in folds.values()):
        raise InvalidTape("chronological split has empty fold")
    report["eligibility"], report["folds"] = "DATASET_ONLY_NOT_A_MODEL", {}
    for name, conditions in folds.items():
        rows = [row for condition in conditions for row in rows_by_session[condition]]
        report["folds"][name] = {
            "sessions": len(conditions), "rows": len(rows),
            "sha256": write_jsonl(args.out_dir / f"{name}.jsonl", rows),
            "start_ms": markets[conditions[0]].start_ms,
            "end_ms": markets[conditions[-1]].end_ms,
        }
    (args.out_dir / "manifest.json").write_bytes(canonical_bytes(report) + b"\n")
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
