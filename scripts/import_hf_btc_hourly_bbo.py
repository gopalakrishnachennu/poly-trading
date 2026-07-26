#!/usr/bin/env python3
"""Normalize verified supplemental BTC-hourly BBO into fixed-point research tape."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
from decimal import Decimal, InvalidOperation, ROUND_CEILING, ROUND_FLOOR
from pathlib import Path
from typing import Any

try:
    import pyarrow as pa
    import pyarrow.compute as pc
    import pyarrow.parquet as pq
except ImportError as error:  # pragma: no cover
    raise SystemExit("pyarrow is required for BBO import") from error

try:
    from scripts.build_btc_hourly_research import MICROS, read_policy as read_research_policy
    from scripts.verify_hf_btc_hourly_candidates import InvalidIdentity, canonical, parse_policy, sha256_file
except ModuleNotFoundError:  # Direct `python scripts/...` execution.
    from build_btc_hourly_research import MICROS, read_policy as read_research_policy
    from verify_hf_btc_hourly_candidates import InvalidIdentity, canonical, parse_policy, sha256_file


REQUIRED_COLUMNS = {
    "ts_ms", "market_id", "token_id", "outcome", "best_bid", "best_ask",
    "best_bid_size", "best_ask_size", "local_recv_ts_ns",
}
CONSERVATIVE_BUCKET_EXTREMA = "conservative-bucket-extrema"
LATEST_USABLE_SNAPSHOT = "latest-usable-snapshot"


def integer(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise InvalidIdentity(f"{field} must be an integer")
    return value


def fixed_micros(value: Any, field: str, rounding: str) -> int:
    if isinstance(value, bool) or value is None:
        raise InvalidIdentity(f"{field} missing or invalid")
    try:
        decimal = Decimal(str(value))
    except (InvalidOperation, ValueError) as error:
        raise InvalidIdentity(f"{field} is not decimal") from error
    if not decimal.is_finite() or decimal < 0:
        raise InvalidIdentity(f"{field} is outside bounds")
    return int((decimal * MICROS).to_integral_value(rounding=rounding))


def load_boundaries(identity_path: Path, labels_path: Path) -> tuple[dict[str, dict[str, Any]], dict[tuple[str, str], tuple[str, str]]]:
    manifest = json.loads(identity_path.read_text(encoding="utf-8"))
    labels = json.loads(labels_path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 2 or manifest.get("source") != "official-polymarket":
        raise InvalidIdentity("official identity manifest invalid")
    if labels.get("schema_version") != 1 or labels.get("official_evidence_sha256") != manifest["sources"][0].get("sha256"):
        raise InvalidIdentity("resolution labels are not bound to official evidence")
    by_condition: dict[str, dict[str, Any]] = {}
    for row in manifest.get("assets", []):
        if row.get("asset") != "BTC":
            continue
        condition = row.get("condition_id")
        if not isinstance(condition, str) or condition in by_condition:
            raise InvalidIdentity("official identity condition invalid or duplicated")
        by_condition[condition] = row
    label_conditions: dict[str, dict[str, Any]] = {}
    market_to_token: dict[tuple[str, str], tuple[str, str]] = {}
    for row in labels.get("labels", []):
        if row.get("asset") != "BTC":
            raise InvalidIdentity("non-BTC label in BTC source")
        condition = row.get("condition_id")
        market_id = row.get("market_id")
        identity = by_condition.get(condition)
        if identity is None or condition in label_conditions or not isinstance(market_id, str):
            raise InvalidIdentity("label identity missing or duplicated")
        if row.get("start_time_ms") != identity.get("start_time_ms") or row.get("end_time_ms") != identity.get("end_time_ms"):
            raise InvalidIdentity("label and identity time disagree")
        label_conditions[condition] = row
        market_to_token[(market_id, identity["up_token_id"])] = (condition, "UP")
        market_to_token[(market_id, identity["down_token_id"])] = (condition, "DOWN")
    if set(label_conditions) != set(by_condition):
        raise InvalidIdentity("identity and label coverage disagree")
    return by_condition, market_to_token


def write_record(handle: Any, digest: Any, value: dict[str, Any]) -> None:
    encoded = canonical(value) + b"\n"
    handle.write(encoded)
    digest.update(encoded)


def import_bbo(
    source: Path,
    identity_path: Path,
    labels_path: Path,
    source_policy: dict[str, Any],
    research_policy: Any,
    out_root: Path,
    compaction_mode: str = CONSERVATIVE_BUCKET_EXTREMA,
) -> dict[str, Any]:
    if compaction_mode not in {CONSERVATIVE_BUCKET_EXTREMA, LATEST_USABLE_SNAPSHOT}:
        raise InvalidIdentity("unsupported BBO compaction mode")
    if out_root.exists() or out_root.is_symlink():
        raise InvalidIdentity("output root must be new")
    if source.is_symlink() or not source.is_file() or source.stat().st_size > source_policy["maximum_bbo_bytes"]:
        raise InvalidIdentity("BBO source is unsafe or exceeds configured size")
    input_digest = sha256_file(source)
    if input_digest != source_policy["bbo_sha256"]:
        raise InvalidIdentity("BBO checksum does not match pinned policy")
    parquet = pq.ParquetFile(source)
    if parquet.metadata.num_rows > source_policy["maximum_bbo_rows"]:
        raise InvalidIdentity("BBO source exceeds configured row bound")
    if not REQUIRED_COLUMNS.issubset(set(parquet.schema_arrow.names)):
        raise InvalidIdentity("BBO source schema incomplete")
    identities, token_map = load_boundaries(identity_path, labels_path)
    out_root.parent.mkdir(parents=True, exist_ok=True)
    rows = 0
    skipped_outside = 0
    skipped_empty = 0
    source_only_rows = 0
    known_receive_rows = 0
    covered: set[str] = set()
    spacing_ms = source_policy["derived_bbo_spacing_ms"]
    source_only_reserve_ns = source_policy["source_only_latency_reserve_ms"] * 1_000_000
    selected_markets = pa.array(sorted({market_id for market_id, _ in token_map}))
    allowed_tokens = {token for _, token in token_map}
    start_by_market = {
        market_id: identities[condition]["start_time_ms"]
        for (market_id, _), (condition, _) in token_map.items()
    }
    end_by_market = {
        market_id: identities[condition]["end_time_ms"]
        for (market_id, _), (condition, _) in token_map.items()
    }
    # One deterministic record per market/token/time bucket. Input row groups
    # are append-generated and not globally sorted, so ordering while scanning
    # would create a false replay. The legacy mode combines conservative extrema
    # within a bucket. The latest-snapshot mode retains one actual usable source
    # row and therefore cannot manufacture a bid/ask combination that never
    # coexisted.
    compact: dict[tuple[str, str, int], dict[str, Any]] = {}
    with tempfile.TemporaryDirectory(prefix=".hf-bbo-", dir=out_root.parent) as temporary:
        tape = Path(temporary) / "clob-l2.jsonl"
        for batch in parquet.iter_batches(columns=sorted(REQUIRED_COLUMNS), batch_size=262_144):
            market_column = batch.column(batch.schema.get_field_index("market_id"))
            selected_batch = batch.filter(pc.is_in(market_column, value_set=selected_markets))
            if selected_batch.num_rows == 0:
                continue
            frame = selected_batch.to_pandas()
            if frame[["ts_ms", "market_id", "token_id", "outcome", "best_bid", "best_ask",
                      "best_bid_size", "best_ask_size"]].isnull().any().any():
                raise InvalidIdentity("selected BBO row has missing required value")
            if not frame["token_id"].isin(allowed_tokens).all():
                raise InvalidIdentity("selected BBO market contains unknown token")
            frame["session_start"] = frame["market_id"].map(start_by_market)
            frame["session_end"] = frame["market_id"].map(end_by_market)
            inside = (frame["ts_ms"] >= frame["session_start"]) & (frame["ts_ms"] <= frame["session_end"])
            skipped_outside += int((~inside).sum())
            frame = frame.loc[inside]
            if frame.empty:
                continue
            if ((frame["ts_ms"] < 0) | (frame["best_bid"] < 0) | (frame["best_ask"] < 0)
                    | (frame["best_ask"] > 1) | (frame["best_bid_size"] < 0)
                    | (frame["best_ask_size"] < 0)).any():
                raise InvalidIdentity("BBO numeric value is outside bounds")
            positive = (frame["best_bid"] > 0) & (frame["best_ask"] > 0)
            if (positive & (frame["best_bid"] >= frame["best_ask"])).any():
                raise InvalidIdentity("BBO price or spread invalid")
            usable = positive & (frame["best_bid_size"] > 0) & (frame["best_ask_size"] > 0)
            skipped_empty += int((~usable).sum())
            frame = frame.loc[usable].copy()
            if frame.empty:
                continue
            frame["bucket"] = frame["ts_ms"] // spacing_ms
            group_keys = ["market_id", "token_id", "outcome", "bucket"]
            if compaction_mode == CONSERVATIVE_BUCKET_EXTREMA:
                grouped = frame.groupby(
                    group_keys,
                    sort=False,
                    observed=True,
                ).agg(
                    ts_ms=("ts_ms", "max"),
                    best_bid=("best_bid", "min"),
                    best_ask=("best_ask", "max"),
                    best_bid_size=("best_bid_size", "min"),
                    best_ask_size=("best_ask_size", "min"),
                    local_recv_ts_ns=("local_recv_ts_ns", "max"),
                    observed_receive_count=("local_recv_ts_ns", "count"),
                    group_count=("ts_ms", "count"),
                ).reset_index()
            else:
                latest_times = frame.groupby(
                    group_keys, sort=False, observed=True,
                )["ts_ms"].transform("max")
                latest = frame.loc[frame["ts_ms"] == latest_times].copy()
                # Exact-timestamp duplicates can still disagree. Select one real
                # row deterministically: lowest bid, then highest ask, then
                # smallest displayed sizes. No fields are combined across rows.
                latest.sort_values(
                    group_keys + [
                        "best_bid", "best_ask", "best_bid_size",
                        "best_ask_size", "local_recv_ts_ns",
                    ],
                    ascending=[
                        True, True, True, True,
                        True, False, True, True, True,
                    ],
                    na_position="last",
                    kind="mergesort",
                    inplace=True,
                )
                grouped = latest.groupby(
                    group_keys, sort=False, observed=True, as_index=False,
                ).head(1).copy()
                grouped["observed_receive_count"] = grouped["local_recv_ts_ns"].notna().astype(int)
                grouped["group_count"] = 1
            for row in grouped.to_dict("records"):
                market_id, token = row["market_id"], row["token_id"]
                mapped = token_map.get((market_id, token))
                if mapped is None:
                    raise InvalidIdentity("selected BBO identity disappeared during grouping")
                condition, side = mapped
                if row["outcome"] != ("Up" if side == "UP" else "Down"):
                    raise InvalidIdentity("BBO outcome/token mapping disagrees")
                key = (market_id, token, int(row["bucket"]))
                candidate = {
                    "condition": condition,
                    "side": side,
                    "market_id": market_id,
                    "token": token,
                    "event_ms": int(row["ts_ms"]),
                    "bid": fixed_micros(row["best_bid"], "best_bid", ROUND_FLOOR),
                    "ask": fixed_micros(row["best_ask"], "best_ask", ROUND_CEILING),
                    "bid_size": fixed_micros(row["best_bid_size"], "best_bid_size", ROUND_FLOOR),
                    "ask_size": fixed_micros(row["best_ask_size"], "best_ask_size", ROUND_FLOOR),
                    "receive_ns": (
                        int(row["local_recv_ts_ns"])
                        if int(row["observed_receive_count"]) == int(row["group_count"])
                        else None
                    ),
                }
                previous = compact.get(key)
                if previous is None:
                    compact[key] = candidate
                elif compaction_mode == LATEST_USABLE_SNAPSHOT:
                    candidate_rank = (
                        candidate["event_ms"],
                        -candidate["bid"],
                        candidate["ask"],
                        -candidate["bid_size"],
                        -candidate["ask_size"],
                        -(candidate["receive_ns"] or -1),
                    )
                    previous_rank = (
                        previous["event_ms"],
                        -previous["bid"],
                        previous["ask"],
                        -previous["bid_size"],
                        -previous["ask_size"],
                        -(previous["receive_ns"] or -1),
                    )
                    if candidate_rank > previous_rank:
                        compact[key] = candidate
                else:
                    previous["event_ms"] = max(previous["event_ms"], candidate["event_ms"])
                    previous["bid"] = min(previous["bid"], candidate["bid"])
                    previous["ask"] = max(previous["ask"], candidate["ask"])
                    previous["bid_size"] = min(previous["bid_size"], candidate["bid_size"])
                    previous["ask_size"] = min(previous["ask_size"], candidate["ask_size"])
                    if previous["receive_ns"] is None or candidate["receive_ns"] is None:
                        previous["receive_ns"] = None
                    else:
                        previous["receive_ns"] = max(previous["receive_ns"], candidate["receive_ns"])
        rows = len(compact)
        if rows > source_policy["maximum_derived_rows"]:
            raise InvalidIdentity("derived BBO row bound exceeded")
        digest = hashlib.sha256()
        with tape.open("xb") as output:
            for condition, identity in sorted(identities.items(), key=lambda item: item[1]["start_time_ms"]):
                write_record(output, digest, {
                    "k": "sys",
                    "payload": {
                        "event_type": "market_identity",
                        "asset": "BTC",
                        "condition_id": condition,
                        "up_token_id": identity["up_token_id"],
                        "down_token_id": identity["down_token_id"],
                        "start_time_ms": identity["start_time_ms"],
                        "end_time_ms": identity["end_time_ms"],
                    },
                })
            ordered = sorted(compact.values(), key=lambda row: (
                row["event_ms"], row["condition"], row["side"], row["token"]
            ))
            for source_sequence, row in enumerate(ordered, 1):
                event_ns = row["event_ms"] * 1_000_000
                received_ns = row["receive_ns"]
                if received_ns is None:
                    available_base_ns = event_ns + source_only_reserve_ns
                    receive_quality = "UNAVAILABLE_EXTERNAL_ARCHIVE"
                    source_only_rows += 1
                else:
                    available_base_ns = received_ns
                    receive_quality = "OBSERVED"
                    known_receive_rows += 1
                covered.add(row["condition"])
                write_record(output, digest, {
                    "k": "book",
                    "tok": row["token"],
                    "cond": row["condition"],
                    "t": event_ns,
                    "r": received_ns,
                    "rq": receive_quality,
                    "a": available_base_ns,
                    "seq": source_sequence,
                    "bids": [[row["bid"], row["bid_size"]]],
                    "asks": [[row["ask"], row["ask_size"]]],
                })
        if len(covered) < research_policy.minimum_completed_sessions:
            raise InvalidIdentity("usable BBO session coverage is below research minimum")
        out_root.mkdir(parents=False, exist_ok=False)
        final_tape = out_root / "clob-l2.jsonl"
        os.replace(tape, final_tape)
        manifest = {
            "schema_version": 1,
            "non_authoritative": True,
            "capability": "BEST_LEVEL_IMMEDIATE_TAKER_RESEARCH_ONLY",
            "source_id": source_policy["source_id"],
            "dataset_repo": source_policy["dataset_repo"],
            "dataset_revision": source_policy["dataset_revision"],
            "license": source_policy["license"],
            "source_path": source_policy["bbo_path"],
            "source_bytes": source.stat().st_size,
            "source_rows": parquet.metadata.num_rows,
            "source_sha256": input_digest,
            "identity_manifest_sha256": sha256_file(identity_path),
            "resolution_labels_sha256": sha256_file(labels_path),
            "derived_spacing_ms": source_policy["derived_bbo_spacing_ms"],
            "compaction_mode": compaction_mode,
            "source_only_latency_reserve_ms": source_policy["source_only_latency_reserve_ms"],
            "derived_rows": rows,
            "covered_sessions": len(covered),
            "source_only_rows_considered": source_only_rows,
            "known_receive_rows_considered": known_receive_rows,
            "skipped_outside_session": skipped_outside,
            "skipped_empty_best_level": skipped_empty,
            "data_sha256": digest.hexdigest(),
        }
        (out_root / "manifest.json").write_bytes(canonical(manifest) + b"\n")
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-policy", required=True, type=Path)
    parser.add_argument("--research-policy", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--identities", required=True, type=Path)
    parser.add_argument("--labels", required=True, type=Path)
    parser.add_argument("--out-root", required=True, type=Path)
    parser.add_argument(
        "--compaction",
        choices=[CONSERVATIVE_BUCKET_EXTREMA, LATEST_USABLE_SNAPSHOT],
        default=CONSERVATIVE_BUCKET_EXTREMA,
    )
    args = parser.parse_args()
    try:
        source_policy = parse_policy(args.source_policy)
        research_policy = read_research_policy(args.research_policy)
        manifest = import_bbo(
            args.source, args.identities, args.labels, source_policy,
            research_policy, args.out_root, args.compaction,
        )
        print(json.dumps(manifest, sort_keys=True))
        return 0
    except (InvalidIdentity, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"BBO import rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
