#!/usr/bin/env python3
"""Build a strict BTC/ETH hourly identity allowlist from official Gamma data.

The output is an immutable input to supplemental-data importers.  It is not a
trading configuration: discovery never creates strategy, model, paper, wallet,
signing, or order authority.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Iterable
from urllib.parse import urlencode
from urllib.request import Request, urlopen


class InvalidIdentity(ValueError):
    pass


HOUR_MS = 3_600_000
ASSET_NAMES = {"BTC": "Bitcoin", "ETH": "Ethereum"}


def canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while block := handle.read(1 << 20):
            digest.update(block)
    return digest.hexdigest()


def exact_int(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise InvalidIdentity(f"{field} must be an integer")
    return value


def parse_policy(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    expected = {
        "schema_version", "source_id", "base_url", "maximum_response_bytes",
        "maximum_pages", "page_size", "assets", "required_outcomes",
    }
    if set(value) != expected or value["schema_version"] != 1:
        raise InvalidIdentity("identity policy schema is invalid")
    if value["source_id"] != "official-polymarket-gamma":
        raise InvalidIdentity("identity source must be official Polymarket Gamma")
    if not isinstance(value["base_url"], str) or value["base_url"] != "https://gamma-api.polymarket.com/events":
        raise InvalidIdentity("identity base URL is not the official Gamma endpoint")
    for field in ("maximum_response_bytes", "maximum_pages", "page_size"):
        if exact_int(value[field], field) <= 0:
            raise InvalidIdentity(f"{field} must be positive")
    if value["assets"] != ["BTC", "ETH"] or value["required_outcomes"] != ["Up", "Down"]:
        raise InvalidIdentity("identity universe or outcome mapping is unexpected")
    return value


def parse_time_ms(value: str, field: str) -> int:
    if not isinstance(value, str) or not value:
        raise InvalidIdentity(f"{field} missing")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise InvalidIdentity(f"{field} is not an ISO-8601 timestamp") from error
    if parsed.tzinfo is None:
        raise InvalidIdentity(f"{field} lacks timezone")
    return int(parsed.astimezone(UTC).timestamp() * 1000)


def json_array(value: Any, field: str) -> list[str]:
    if isinstance(value, str):
        try:
            value = json.loads(value)
        except json.JSONDecodeError as error:
            raise InvalidIdentity(f"{field} is not JSON") from error
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise InvalidIdentity(f"{field} must be a string array")
    return value


def contains_asset(value: str, asset: str) -> bool:
    words = value.lower().replace("/", " ").replace("-", " ").split()
    return ASSET_NAMES[asset].lower() in words or asset.lower() in words


def event_start(event: dict[str, Any], market: dict[str, Any]) -> int:
    # Gamma has used both spellings; never infer an opening time from close.
    start = market.get("eventStartTime") or event.get("eventStartTime")
    if start is None:
        start = market.get("startDate") or event.get("startDate")
    return parse_time_ms(start, "event/market start time")


def candidate_from_market(event: dict[str, Any], market: dict[str, Any], asset: str, required_outcomes: list[str]) -> dict[str, Any] | None:
    if not isinstance(event, dict) or not isinstance(market, dict):
        raise InvalidIdentity("official response has non-object event or market")
    title = event.get("title") or market.get("question")
    question = market.get("question")
    if not isinstance(title, str) or not isinstance(question, str):
        raise InvalidIdentity("official market title or question missing")
    if not (contains_asset(title, asset) and contains_asset(question, asset) and "up or down" in title.lower() and "up or down" in question.lower()):
        return None
    start_ms = event_start(event, market)
    end_ms = parse_time_ms(market.get("endDate") or event.get("endDate"), "market end time")
    if start_ms % HOUR_MS or end_ms - start_ms != HOUR_MS:
        return None
    condition_id = market.get("conditionId")
    if not isinstance(condition_id, str) or not condition_id.startswith("0x") or len(condition_id) != 66:
        raise InvalidIdentity("candidate condition ID is invalid")
    outcomes = json_array(market.get("outcomes"), "outcomes")
    token_ids = json_array(market.get("clobTokenIds"), "clobTokenIds")
    if outcomes != required_outcomes or len(token_ids) != 2 or not all(token.isdecimal() for token in token_ids):
        raise InvalidIdentity("candidate outcome/token mapping is invalid")
    description = market.get("description") or event.get("description")
    resolution_source = market.get("resolutionSource") or event.get("resolutionSource")
    if not isinstance(description, str) or not isinstance(resolution_source, str):
        raise InvalidIdentity("candidate rule provenance is invalid")
    rules = {
        "asset": asset, "condition_id": condition_id, "event_slug": event.get("slug"),
        "market_slug": market.get("slug"), "title": title, "question": question,
        "description": description, "resolution_source": resolution_source,
        "start_time_ms": start_ms, "end_time_ms": end_ms,
        "outcomes": outcomes, "token_ids": token_ids,
    }
    return {
        "asset": asset, "condition_id": condition_id, "up_token_id": token_ids[0],
        "down_token_id": token_ids[1], "start_time_ms": start_ms,
        "end_time_ms": end_ms, "rules_fingerprint": sha256_bytes(canonical(rules)),
    }


def iter_events(payload: Any) -> Iterable[dict[str, Any]]:
    if isinstance(payload, list):
        events = payload
    elif isinstance(payload, dict) and isinstance(payload.get("events"), list):
        events = payload["events"]
    else:
        raise InvalidIdentity("official Gamma page must be an events array")
    for event in events:
        if not isinstance(event, dict):
            raise InvalidIdentity("official Gamma event is invalid")
        yield event


def discover(pages: Iterable[Any], policy: dict[str, Any], start_ms: int, end_ms: int) -> list[dict[str, Any]]:
    if start_ms % HOUR_MS or end_ms % HOUR_MS or end_ms <= start_ms:
        raise InvalidIdentity("requested interval must be positive UTC whole hours")
    expected_slots = {(asset, hour) for asset in policy["assets"] for hour in range(start_ms, end_ms, HOUR_MS)}
    found: dict[tuple[str, int], dict[str, Any]] = {}
    seen_conditions: set[str] = set()
    for payload in pages:
        for event in iter_events(payload):
            markets = event.get("markets")
            if not isinstance(markets, list):
                raise InvalidIdentity("official Gamma event lacks markets")
            for market in markets:
                for asset in policy["assets"]:
                    candidate = candidate_from_market(event, market, asset, policy["required_outcomes"])
                    if candidate is None:
                        continue
                    key = (asset, candidate["start_time_ms"])
                    if key not in expected_slots:
                        continue
                    if candidate["condition_id"] in seen_conditions or key in found:
                        raise InvalidIdentity("duplicate candidate identity for requested hour")
                    seen_conditions.add(candidate["condition_id"])
                    found[key] = candidate
    missing = sorted(expected_slots - set(found))
    if missing:
        display = ", ".join(f"{asset}@{datetime.fromtimestamp(hour / 1000, UTC).isoformat()}" for asset, hour in missing[:8])
        raise InvalidIdentity(f"official hourly coverage incomplete: {display}")
    return [found[key] for key in sorted(found, key=lambda item: (item[1], item[0]))]


def load_source(path: Path, max_bytes: int) -> tuple[Any, dict[str, str]]:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > max_bytes:
        raise InvalidIdentity("official source is unsafe or exceeds configured size")
    raw = path.read_bytes()
    return json.loads(raw), {"kind": "operator-supplied", "name": path.name, "sha256": sha256_bytes(raw)}


def gamma_range_query(policy: dict[str, Any], start_ms: int, end_ms: int, offset: int) -> str:
    if start_ms % HOUR_MS or end_ms % HOUR_MS or end_ms <= start_ms:
        raise InvalidIdentity("requested interval must be positive UTC whole hours")
    # The hourly market ending at `start + 1h` is the first requested market;
    # the market ending at `end` is the last.  Bounding on endDate avoids
    # scanning the entire Gamma catalogue and makes the fetch storage/latency
    # cost proportional to the requested interval.
    end_min = datetime.fromtimestamp((start_ms + HOUR_MS) / 1000, UTC).isoformat().replace("+00:00", "Z")
    end_max = datetime.fromtimestamp(end_ms / 1000, UTC).isoformat().replace("+00:00", "Z")
    return urlencode({
        "closed": "true",
        "limit": policy["page_size"],
        "offset": offset,
        "order": "endDate",
        "ascending": "true",
        "end_date_min": end_min,
        "end_date_max": end_max,
    })


def fetch_pages(policy: dict[str, Any], start_ms: int, end_ms: int) -> Iterable[tuple[Any, dict[str, str]]]:
    for offset in range(0, policy["maximum_pages"] * policy["page_size"], policy["page_size"]):
        query = gamma_range_query(policy, start_ms, end_ms, offset)
        request = Request(f"{policy['base_url']}?{query}", headers={"Accept": "application/json", "User-Agent": "poly-research-identity/1"})
        with urlopen(request, timeout=20) as response:  # nosec B310: URL is policy-pinned above
            raw = response.read(policy["maximum_response_bytes"] + 1)
        if len(raw) > policy["maximum_response_bytes"]:
            raise InvalidIdentity("official Gamma response exceeds configured size")
        payload = json.loads(raw)
        events = list(iter_events(payload))
        yield payload, {"kind": "fetched", "url": request.full_url, "sha256": sha256_bytes(raw)}
        if len(events) < policy["page_size"]:
            return
    raise InvalidIdentity("official Gamma pagination reached configured bound")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--start-time-ms", required=True, type=int)
    parser.add_argument("--end-time-ms", required=True, type=int)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--source", type=Path, action="append", help="immutable downloaded official Gamma JSON page")
    parser.add_argument("--fetch", action="store_true", help="bounded read-only fetch from the policy-pinned official Gamma endpoint")
    args = parser.parse_args()
    try:
        if bool(args.source) == args.fetch:
            raise InvalidIdentity("choose exactly one of --source or --fetch")
        if args.out.exists() or args.out.is_symlink():
            raise InvalidIdentity("output manifest must not already exist")
        policy = parse_policy(args.policy)
        if args.fetch:
            collected = list(fetch_pages(policy, args.start_time_ms, args.end_time_ms))
        else:
            collected = [load_source(path, policy["maximum_response_bytes"]) for path in args.source]
        assets = discover((payload for payload, _ in collected), policy, args.start_time_ms, args.end_time_ms)
        manifest = {
            "schema_version": 2, "source": "official-polymarket", "source_id": policy["source_id"],
            "policy_sha256": sha256_file(args.policy), "requested_start_time_ms": args.start_time_ms,
            "requested_end_time_ms": args.end_time_ms, "sources": [source for _, source in collected], "assets": assets,
        }
        args.out.parent.mkdir(parents=True, exist_ok=True)
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
        with os.fdopen(os.open(args.out, flags, 0o600), "w", encoding="utf-8") as handle:
            handle.write(canonical(manifest).decode("utf-8") + "\n")
        print(json.dumps({"assets": len(assets), "manifest_sha256": sha256_file(args.out), "source_pages": len(collected)}, sort_keys=True))
        return 0
    except (InvalidIdentity, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"official identity manifest rejected: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
