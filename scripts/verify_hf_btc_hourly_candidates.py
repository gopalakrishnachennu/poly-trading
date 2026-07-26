#!/usr/bin/env python3
"""Verify supplemental BTC-hourly candidates against official Gamma markets."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Iterable
from urllib.request import Request, urlopen

try:
    import pyarrow.parquet as pq
except ImportError as error:  # pragma: no cover - deployment dependency
    raise SystemExit("pyarrow is required for candidate verification") from error

try:
    from scripts.build_official_hourly_identity_manifest import (
        HOUR_MS,
        InvalidIdentity,
        candidate_from_market,
        canonical,
        json_array,
        sha256_bytes,
        sha256_file,
    )
except ModuleNotFoundError:  # Direct `python scripts/...` execution.
    from build_official_hourly_identity_manifest import (
        HOUR_MS,
        InvalidIdentity,
        candidate_from_market,
        canonical,
        json_array,
        sha256_bytes,
        sha256_file,
    )


REQUIRED_COLUMNS = {
    "market_id", "crypto", "timeframe", "resolution", "end_ts",
    "condition_id", "up_token_id", "down_token_id",
}


def exact_int(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise InvalidIdentity(f"{field} must be an integer")
    return value


def parse_policy(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    expected = {
        "schema_version", "source_id", "dataset_repo", "dataset_revision",
        "markets_path", "markets_sha256", "bbo_path", "bbo_sha256",
        "ticks_path", "ticks_sha256", "spot_path", "spot_sha256",
        "license", "official_market_url_template", "assets", "timeframe",
        "maximum_candidate_bytes", "maximum_candidate_rows",
        "maximum_bbo_bytes", "maximum_bbo_rows",
        "maximum_ticks_bytes", "maximum_ticks_rows",
        "maximum_spot_bytes", "maximum_spot_rows",
        "derived_bbo_spacing_ms", "source_only_latency_reserve_ms",
        "maximum_derived_rows",
        "maximum_official_response_bytes", "maximum_total_evidence_bytes",
        "maximum_official_requests", "request_timeout_seconds",
    }
    if set(value) != expected or value["schema_version"] != 1:
        raise InvalidIdentity("candidate-verification policy schema invalid")
    if value["source_id"] != "hf-aliplayer1-polymarket-crypto-updown":
        raise InvalidIdentity("supplemental source identity invalid")
    if value["dataset_repo"] != "aliplayer1/polymarket-crypto-updown":
        raise InvalidIdentity("supplemental repository is not pinned")
    revision = value["dataset_revision"]
    if not isinstance(revision, str) or len(revision) != 40 or any(c not in "0123456789abcdef" for c in revision):
        raise InvalidIdentity("dataset revision must be a git SHA")
    for field in ("markets_sha256", "bbo_sha256", "ticks_sha256", "spot_sha256"):
        digest = value[field]
        if not isinstance(digest, str) or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
            raise InvalidIdentity(f"{field} must be SHA-256")
    if (value["markets_path"] != "data/markets.parquet"
            or value["bbo_path"] != "data/orderbook/crypto=BTC/timeframe=1-hour/part-0.parquet"
            or value["ticks_path"] != "data/ticks/crypto=BTC/timeframe=1-hour/part-0.parquet"
            or value["spot_path"] != "data/spot_prices/part-0.parquet"):
        raise InvalidIdentity("supplemental file paths are not pinned")
    if value["license"] != "MIT" or value["assets"] != ["BTC"] or value["timeframe"] != "1-hour":
        raise InvalidIdentity("supplemental universe, timeframe, or license invalid")
    if value["official_market_url_template"] != "https://gamma-api.polymarket.com/markets/{market_id}":
        raise InvalidIdentity("official market endpoint is not pinned")
    for field in (
        "maximum_candidate_bytes", "maximum_candidate_rows",
        "maximum_bbo_bytes", "maximum_bbo_rows",
        "maximum_ticks_bytes", "maximum_ticks_rows",
        "maximum_spot_bytes", "maximum_spot_rows",
        "derived_bbo_spacing_ms", "source_only_latency_reserve_ms",
        "maximum_derived_rows",
        "maximum_official_response_bytes", "maximum_total_evidence_bytes",
        "maximum_official_requests", "request_timeout_seconds",
    ):
        if exact_int(value[field], field) <= 0:
            raise InvalidIdentity(f"{field} must be positive")
    return value


def load_candidates(path: Path, policy: dict[str, Any], start_ms: int, end_ms: int) -> dict[str, dict[str, Any]]:
    if start_ms % HOUR_MS or end_ms % HOUR_MS or end_ms <= start_ms:
        raise InvalidIdentity("requested interval must be positive UTC whole hours")
    if path.is_symlink() or not path.is_file() or path.stat().st_size > policy["maximum_candidate_bytes"]:
        raise InvalidIdentity("candidate catalogue is unsafe or exceeds configured size")
    if sha256_file(path) != policy["markets_sha256"]:
        raise InvalidIdentity("candidate catalogue checksum does not match policy")
    parquet = pq.ParquetFile(path)
    if parquet.metadata.num_rows > policy["maximum_candidate_rows"]:
        raise InvalidIdentity("candidate catalogue exceeds configured row bound")
    names = set(parquet.schema_arrow.names)
    if not REQUIRED_COLUMNS.issubset(names):
        raise InvalidIdentity("candidate catalogue schema is incomplete")
    selected: dict[str, dict[str, Any]] = {}
    hours: set[int] = set()
    for batch in parquet.iter_batches(columns=sorted(REQUIRED_COLUMNS), batch_size=65_536):
        for row in batch.to_pylist():
            if row["crypto"] != "BTC" or row["timeframe"] != "1-hour":
                continue
            end_seconds = exact_int(row["end_ts"], "end_ts")
            start = end_seconds * 1000 - HOUR_MS
            if not (start_ms <= start < end_ms):
                continue
            market_id = row["market_id"]
            if not isinstance(market_id, str) or not market_id.isdecimal():
                raise InvalidIdentity("candidate market ID invalid")
            if start in hours or market_id in selected:
                raise InvalidIdentity("duplicate candidate for requested hour")
            resolution = exact_int(row["resolution"], "resolution")
            if resolution not in {-1, 0, 1}:
                raise InvalidIdentity("candidate resolution state is invalid")
            for field in ("condition_id", "up_token_id", "down_token_id"):
                if not isinstance(row[field], str) or not row[field]:
                    raise InvalidIdentity(f"candidate {field} invalid")
            selected[market_id] = {
                "market_id": market_id,
                "start_time_ms": start,
                "end_time_ms": end_seconds * 1000,
                "condition_id": row["condition_id"],
                "up_token_id": row["up_token_id"],
                "down_token_id": row["down_token_id"],
                "resolution": resolution,
            }
            hours.add(start)
    expected = set(range(start_ms, end_ms, HOUR_MS))
    missing = sorted(expected - hours)
    if missing:
        shown = ", ".join(datetime.fromtimestamp(ms / 1000, UTC).isoformat() for ms in missing[:8])
        raise InvalidIdentity(f"candidate hourly coverage incomplete: {shown}")
    return selected


def official_resolution(market: dict[str, Any]) -> int:
    if market.get("closed") is not True:
        raise InvalidIdentity("official market is not closed")
    prices = json_array(market.get("outcomePrices"), "outcomePrices")
    if prices == ["1", "0"]:
        return 1
    if prices == ["0", "1"]:
        return 0
    raise InvalidIdentity("official market is not binary-resolved")


def validate_official(
    selected: dict[str, dict[str, Any]], official: Iterable[dict[str, Any]]
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    seen: set[str] = set()
    identities: list[dict[str, Any]] = []
    labels: list[dict[str, Any]] = []
    for market in official:
        if not isinstance(market, dict):
            raise InvalidIdentity("official evidence row must be an object")
        market_id = market.get("id")
        if not isinstance(market_id, str) or market_id not in selected or market_id in seen:
            raise InvalidIdentity("official evidence market ID unexpected or duplicated")
        expected = selected[market_id]
        identity = candidate_from_market(
            {"title": market.get("question"), "description": market.get("description"),
             "resolutionSource": market.get("resolutionSource")},
            market,
            "BTC",
            ["Up", "Down"],
        )
        if identity is None:
            raise InvalidIdentity("official market is not BTC Up or Down")
        comparisons = {
            "start_time_ms": expected["start_time_ms"],
            "end_time_ms": expected["end_time_ms"],
            "condition_id": expected["condition_id"],
            "up_token_id": expected["up_token_id"],
            "down_token_id": expected["down_token_id"],
        }
        if any(identity[field] != expected_value for field, expected_value in comparisons.items()):
            raise InvalidIdentity("official and supplemental identity disagree")
        resolution = official_resolution(market)
        # A pinned archive may have captured the market before its resolution
        # was backfilled. In that case -1 is missing supplemental evidence, not
        # a label. The official closed Gamma market is the sole label authority.
        if expected["resolution"] in {0, 1} and resolution != expected["resolution"]:
            raise InvalidIdentity("official and supplemental resolution disagree")
        identities.append(identity)
        labels.append({
            "schema_version": 1,
            "asset": "BTC",
            "market_id": market_id,
            "condition_id": identity["condition_id"],
            "start_time_ms": identity["start_time_ms"],
            "end_time_ms": identity["end_time_ms"],
            "resolved_side": "UP" if resolution == 1 else "DOWN",
        })
        seen.add(market_id)
    if seen != set(selected):
        raise InvalidIdentity("official evidence coverage incomplete")
    identities.sort(key=lambda row: row["start_time_ms"])
    labels.sort(key=lambda row: row["start_time_ms"])
    return identities, labels


def read_official_source(path: Path, policy: dict[str, Any]) -> tuple[list[dict[str, Any]], bytes]:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > policy["maximum_total_evidence_bytes"]:
        raise InvalidIdentity("official evidence source is unsafe or exceeds configured size")
    raw = path.read_bytes()
    records: list[dict[str, Any]] = []
    for line_number, line in enumerate(raw.splitlines(), 1):
        if not line:
            continue
        value = json.loads(line)
        if not isinstance(value, dict):
            raise InvalidIdentity(f"official evidence line {line_number} is invalid")
        records.append(value)
    return records, raw


def fetch_official(selected: dict[str, dict[str, Any]], policy: dict[str, Any]) -> tuple[list[dict[str, Any]], bytes]:
    if len(selected) > policy["maximum_official_requests"]:
        raise InvalidIdentity("official request count exceeds configured bound")
    records: list[dict[str, Any]] = []
    total = 0
    for market_id in sorted(selected, key=lambda item: selected[item]["start_time_ms"]):
        url = policy["official_market_url_template"].format(market_id=market_id)
        request = Request(url, headers={"Accept": "application/json", "User-Agent": "poly-research-candidate-verifier/1"})
        with urlopen(request, timeout=policy["request_timeout_seconds"]) as response:  # nosec B310: policy-pinned HTTPS URL
            raw = response.read(policy["maximum_official_response_bytes"] + 1)
        if len(raw) > policy["maximum_official_response_bytes"]:
            raise InvalidIdentity("official market response exceeds configured size")
        total += len(raw)
        if total > policy["maximum_total_evidence_bytes"]:
            raise InvalidIdentity("official aggregate evidence exceeds configured size")
        value = json.loads(raw)
        if not isinstance(value, dict):
            raise InvalidIdentity("official market response must be an object")
        records.append(value)
    evidence = b"".join(canonical(record) + b"\n" for record in records)
    return records, evidence


def write_exclusive(path: Path, payload: bytes) -> None:
    if path.exists() or path.is_symlink():
        raise InvalidIdentity(f"output already exists: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    with os.fdopen(os.open(path, flags, 0o600), "wb") as handle:
        handle.write(payload)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--candidates", required=True, type=Path)
    parser.add_argument("--start-time-ms", required=True, type=int)
    parser.add_argument("--end-time-ms", required=True, type=int)
    parser.add_argument("--official-source", type=Path)
    parser.add_argument("--fetch", action="store_true")
    parser.add_argument("--evidence-out", required=True, type=Path)
    parser.add_argument("--manifest-out", required=True, type=Path)
    parser.add_argument("--labels-out", required=True, type=Path)
    args = parser.parse_args()
    try:
        if bool(args.official_source) == args.fetch:
            raise InvalidIdentity("choose exactly one of --official-source or --fetch")
        policy = parse_policy(args.policy)
        selected = load_candidates(args.candidates, policy, args.start_time_ms, args.end_time_ms)
        if args.fetch:
            official, evidence = fetch_official(selected, policy)
            source_kind = "fetched"
        else:
            official, evidence = read_official_source(args.official_source, policy)
            source_kind = "operator-supplied"
        identities, labels = validate_official(selected, official)
        evidence_digest = sha256_bytes(evidence)
        manifest = {
            "schema_version": 2,
            "source": "official-polymarket",
            "source_id": "official-polymarket-gamma",
            "policy_sha256": sha256_file(args.policy),
            "requested_start_time_ms": args.start_time_ms,
            "requested_end_time_ms": args.end_time_ms,
            "sources": [{
                "kind": source_kind,
                "name": args.evidence_out.name,
                "url": policy["official_market_url_template"],
                "sha256": evidence_digest,
            }],
            "assets": identities,
        }
        label_document = {
            "schema_version": 1,
            "non_authoritative": True,
            "source_id": policy["source_id"],
            "dataset_repo": policy["dataset_repo"],
            "dataset_revision": policy["dataset_revision"],
            "license": policy["license"],
            "candidate_sha256": sha256_file(args.candidates),
            "official_evidence_sha256": evidence_digest,
            "supplemental_resolved_labels": sum(
                candidate["resolution"] in {0, 1}
                for candidate in selected.values()
            ),
            "official_only_resolved_labels": sum(
                candidate["resolution"] == -1
                for candidate in selected.values()
            ),
            "labels": labels,
        }
        # Validate every artifact before publishing any of them.
        payloads = (
            (args.evidence_out, evidence),
            (args.manifest_out, canonical(manifest) + b"\n"),
            (args.labels_out, canonical(label_document) + b"\n"),
        )
        if len({path.resolve() for path, _ in payloads}) != 3:
            raise InvalidIdentity("output paths must be distinct")
        for path, _ in payloads:
            if path.exists() or path.is_symlink():
                raise InvalidIdentity(f"output already exists: {path}")
        for path, payload in payloads:
            write_exclusive(path, payload)
        print(json.dumps({
            "verified_sessions": len(labels),
            "evidence_sha256": evidence_digest,
            "manifest_sha256": sha256_file(args.manifest_out),
            "labels_sha256": sha256_file(args.labels_out),
        }, sort_keys=True))
        return 0
    except (InvalidIdentity, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"candidate verification rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
