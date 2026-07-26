#!/usr/bin/env python3
"""Export all recorded paper trades with explicit strategy provenance.

Core pair fills must first pass the Rust BLAKE3/sequence/campaign audit. Retired
legacy simulations remain clearly separated and their original decimal text is
preserved rather than promoted into fixed-point financial authority.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import shutil
import tempfile
from collections import Counter
from decimal import Decimal
from pathlib import Path
from typing import Any


class InvalidTradeHistory(RuntimeError):
    """Trade history is incomplete, ambiguous, corrupt or misclassified."""


EXPORT_SCHEMA_VERSION = 1
CSV_COLUMNS = (
    "schema_version",
    "record_class",
    "execution_mode",
    "paper_authoritative",
    "real_money",
    "source_path",
    "source_line",
    "source_record_digest",
    "campaign_id",
    "trade_id",
    "asset",
    "window",
    "market_id",
    "condition_id",
    "strategy_id",
    "strategy_parameters",
    "strategy_provenance",
    "strategy_status",
    "decision_time_ms",
    "entry_time_ms",
    "exit_time_ms",
    "market_start_ms",
    "market_end_ms",
    "side",
    "state",
    "won",
    "exit_reason",
    "quantity_micros",
    "up_price_micros",
    "down_price_micros",
    "pair_cost_micros",
    "fee_micros",
    "slippage_micros",
    "cost_micros",
    "locked_pnl_micros",
    "entry_price_raw",
    "exit_price_raw",
    "stake_raw",
    "shares_raw",
    "realized_pnl_raw",
    "bankroll_after_raw",
    "runtime_config_id",
    "runtime_config_digest",
    "policy_id",
    "policy_digest",
    "backfilled",
    "notes",
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


def regular_complete_file(path: Path, description: str) -> None:
    if path.is_symlink() or not path.is_file() or path.stat().st_size <= 0:
        raise InvalidTradeHistory(f"{description} is not a non-empty regular file")
    with path.open("rb") as source:
        source.seek(-1, os.SEEK_END)
        if source.read(1) != b"\n":
            raise InvalidTradeHistory(f"{description} has an incomplete tail")


def source_evidence(path: Path) -> dict[str, Any]:
    regular_complete_file(path, str(path))
    return {
        "path": str(path.resolve()),
        "bytes": path.stat().st_size,
        "sha256": sha256_file(path),
    }


def exact_object(value: Any, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise InvalidTradeHistory(f"{name} is not an object")
    return value


def text(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, Decimal):
        return format(value, "f")
    if isinstance(value, (str, int)):
        return str(value)
    raise InvalidTradeHistory("trade scalar has an unsupported type")


def empty_row() -> dict[str, str]:
    return {column: "" for column in CSV_COLUMNS}


def load_core_audit(path: Path) -> tuple[list[dict[str, str]], dict[str, Any]]:
    regular_complete_file(path, "core audit")
    raw = json.loads(path.read_text())
    document = exact_object(raw, "core audit")
    if (
        document.get("schema_version") != 1
        or not isinstance(document.get("sources"), list)
        or not isinstance(document.get("trades"), list)
    ):
        raise InvalidTradeHistory("core audit contract is invalid")
    rows: list[dict[str, str]] = []
    seen: set[str] = set()
    for item in document["trades"]:
        trade = exact_object(item, "verified core trade")
        record = exact_object(trade.get("record"), "verified core record")
        decision = exact_object(trade.get("decision"), "verified core decision")
        payload = exact_object(record.get("payload"), "verified core payload")
        decision_payload = exact_object(
            decision.get("payload"), "verified core decision payload"
        )
        if record.get("kind") != "filled_pair" or decision.get("kind") != "PAIR_BUY":
            raise InvalidTradeHistory("core audit contains a non-pair trade")
        if (
            record.get("event_time_ms") != decision.get("event_time_ms")
            or payload.get("asset") != decision_payload.get("asset")
            or payload.get("condition_id") != decision_payload.get("condition_id")
        ):
            raise InvalidTradeHistory("core trade/decision join is inconsistent")
        trade_id = text(payload.get("trade_id"))
        if not trade_id or trade_id in seen:
            raise InvalidTradeHistory("core trade id is missing or duplicated")
        seen.add(trade_id)
        row = empty_row()
        row.update(
            {
                "schema_version": str(EXPORT_SCHEMA_VERSION),
                "record_class": "CORE_PAPER_PAIR",
                "execution_mode": "SIMULATED_PAPER",
                "paper_authoritative": "true",
                "real_money": "false",
                "source_path": text(trade.get("source_path")),
                "source_line": text(trade.get("source_line")),
                "source_record_digest": text(trade.get("record_digest")),
                "campaign_id": text(record.get("campaign_id")),
                "trade_id": trade_id,
                "asset": text(payload.get("asset")),
                "window": "1h",
                "condition_id": text(payload.get("condition_id")),
                "strategy_id": "complete_set_arbitrage_buy_pair",
                "strategy_parameters": canonical(
                    {
                        "fee_micros": decision_payload.get("fee_micros"),
                        "net_cost_micros": decision_payload.get("net_cost_micros"),
                        "pair_cost_micros": decision_payload.get("pair_cost_micros"),
                        "quantity_micros": decision_payload.get("quantity_micros"),
                        "slippage_micros": decision_payload.get("slippage_micros"),
                    }
                ).decode(),
                "strategy_provenance": "VERIFIED_PAIR_BUY_DECISION",
                "strategy_status": "ACTIVE_CORE_PAPER_POLICY",
                "decision_time_ms": text(payload.get("decision_at_ms")),
                "entry_time_ms": text(record.get("event_time_ms")),
                "side": "UP+DOWN",
                "state": text(payload.get("state")),
                "quantity_micros": text(payload.get("quantity_micros")),
                "up_price_micros": text(payload.get("up_price_micros")),
                "down_price_micros": text(payload.get("down_price_micros")),
                "pair_cost_micros": text(decision_payload.get("pair_cost_micros")),
                "fee_micros": text(payload.get("fee_micros")),
                "slippage_micros": text(payload.get("slippage_micros")),
                "cost_micros": text(payload.get("cost_micros")),
                "locked_pnl_micros": text(payload.get("locked_pnl_micros")),
                "runtime_config_id": text(record.get("runtime_config_id")),
                "runtime_config_digest": text(record.get("runtime_config_digest")),
                "policy_id": text(decision_payload.get("policy_id")),
                "policy_digest": text(decision_payload.get("policy_digest")),
                "backfilled": "false",
                "notes": text(decision_payload.get("reason")),
            }
        )
        rows.append(row)
    evidence = {
        "path": str(path.resolve()),
        "bytes": path.stat().st_size,
        "sha256": sha256_file(path),
        "verified_journals": document["sources"],
        "verified_trades": len(rows),
    }
    return rows, evidence


def load_legacy_settlement(path: Path) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    with path.open() as source:
        for line_number, line in enumerate(source, 1):
            record = json.loads(line, parse_float=Decimal)
            record = exact_object(record, "legacy settlement trade")
            digest = hashlib.sha256(line.encode()).hexdigest()
            strategy = text(record.get("strategy"))
            if not strategy:
                raise InvalidTradeHistory("legacy settlement strategy is missing")
            row = empty_row()
            row.update(
                {
                    "schema_version": str(EXPORT_SCHEMA_VERSION),
                    "record_class": "LEGACY_DIRECTIONAL_SETTLEMENT",
                    "execution_mode": "SIMULATED_PAPER",
                    "paper_authoritative": "false",
                    "real_money": "false",
                    "source_path": str(path.resolve()),
                    "source_line": str(line_number),
                    "source_record_digest": digest,
                    "trade_id": f"legacy-settlement-{digest[:24]}",
                    "asset": text(record.get("asset")),
                    "window": text(record.get("window")),
                    "market_id": text(record.get("market")),
                    "condition_id": text(record.get("condition_id")),
                    "strategy_id": strategy,
                    "strategy_parameters": strategy,
                    "strategy_provenance": "RECORDED_JOURNAL_FIELD",
                    "strategy_status": "RETIRED_UNSAFE_FLOATING_POINT_SIMULATOR",
                    "decision_time_ms": text(record.get("t")),
                    "entry_time_ms": text(record.get("t")),
                    "exit_time_ms": text(record.get("settled_t")),
                    "market_start_ms": text(record.get("start_ms")),
                    "market_end_ms": text(record.get("end_ms")),
                    "side": text(record.get("side")),
                    "state": "SETTLED_SIMULATION",
                    "won": text(record.get("won")),
                    "entry_price_raw": text(record.get("price")),
                    "stake_raw": text(record.get("stake")),
                    "shares_raw": text(record.get("shares")),
                    "realized_pnl_raw": text(record.get("pnl")),
                    "bankroll_after_raw": text(record.get("bankroll")),
                    "backfilled": text(record.get("backfilled") or False),
                    "notes": text(record.get("note")),
                }
            )
            rows.append(row)
    return rows


def load_legacy_intraday(path: Path) -> list[dict[str, str]]:
    parameters = canonical(
        {
            "buy_threshold": "0.25",
            "sell_threshold": "0.40",
            "windows": ["15m", "1h"],
        }
    ).decode()
    rows: list[dict[str, str]] = []
    with path.open() as source:
        for line_number, line in enumerate(source, 1):
            record = json.loads(line, parse_float=Decimal)
            record = exact_object(record, "legacy intraday trade")
            digest = hashlib.sha256(line.encode()).hexdigest()
            row = empty_row()
            row.update(
                {
                    "schema_version": str(EXPORT_SCHEMA_VERSION),
                    "record_class": "LEGACY_INTRADAY_ROUND_TRIP",
                    "execution_mode": "SIMULATED_PAPER",
                    "paper_authoritative": "false",
                    "real_money": "false",
                    "source_path": str(path.resolve()),
                    "source_line": str(line_number),
                    "source_record_digest": digest,
                    "trade_id": f"legacy-intraday-{digest[:24]}",
                    "asset": text(record.get("asset")),
                    "window": text(record.get("window")),
                    "condition_id": text(record.get("condition_id")),
                    "strategy_id": "fixed_threshold_intraday_round_trip",
                    "strategy_parameters": parameters,
                    "strategy_provenance": "INFERRED_FROM_RETIRED_ENGINE_CONTRACT",
                    "strategy_status": "RETIRED_NEGATIVE_OOS",
                    "decision_time_ms": text(record.get("opened_ms")),
                    "entry_time_ms": text(record.get("opened_ms")),
                    "exit_time_ms": text(record.get("closed_ms")),
                    "market_start_ms": text(record.get("start_ms")),
                    "market_end_ms": text(record.get("end_ms")),
                    "side": text(record.get("side_token")),
                    "state": "CLOSED_SIMULATION",
                    "won": text(record.get("won")),
                    "exit_reason": text(record.get("exit_reason")),
                    "entry_price_raw": text(record.get("entry_price")),
                    "exit_price_raw": text(record.get("exit_price")),
                    "stake_raw": text(record.get("stake")),
                    "shares_raw": text(record.get("shares")),
                    "realized_pnl_raw": text(record.get("pnl")),
                    "bankroll_after_raw": text(record.get("bankroll_after")),
                    "backfilled": "false",
                    "notes": "Historical retired intraday evidence; strategy name was absent from the journal.",
                }
            )
            rows.append(row)
    return rows


def write_atomic_json(path: Path, value: dict[str, Any]) -> None:
    temporary = path.parent / f".{path.name}.{os.getpid()}.tmp"
    with temporary.open("xb") as output:
        output.write(canonical(value) + b"\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)


def export(core_audit: Path, legacy_dir: Path, output_root: Path) -> dict[str, Any]:
    core_rows, core_evidence = load_core_audit(core_audit.resolve())
    legacy_dir = legacy_dir.resolve()
    settlement_paths = sorted(legacy_dir.glob("bets-*.jsonl"))
    intraday_paths = sorted(legacy_dir.glob("intraday-bets-*.jsonl"))
    if not settlement_paths or not intraday_paths:
        raise InvalidTradeHistory("legacy trade source families are incomplete")
    legacy_evidence = [
        source_evidence(path) for path in settlement_paths + intraday_paths
    ]
    contract_path = Path(__file__).resolve().parent / "live_paper_intraday.py"
    contract_evidence = source_evidence(contract_path)
    rows = list(core_rows)
    for path in settlement_paths:
        rows.extend(load_legacy_settlement(path))
    for path in intraday_paths:
        rows.extend(load_legacy_intraday(path))
    ids = [row["trade_id"] for row in rows]
    if len(ids) != len(set(ids)):
        raise InvalidTradeHistory("trade identities are not globally unique")
    rows.sort(
        key=lambda row: (
            int(row["entry_time_ms"] or "0"),
            row["record_class"],
            row["trade_id"],
        )
    )
    revision_basis = {
        "schema_version": EXPORT_SCHEMA_VERSION,
        "columns": list(CSV_COLUMNS),
        "core_audit_sha256": core_evidence["sha256"],
        "legacy_sources": legacy_evidence,
        "intraday_contract_sha256": contract_evidence["sha256"],
    }
    revision = hashlib.sha256(canonical(revision_basis)).hexdigest()
    output_root = output_root.resolve()
    output_root.mkdir(parents=True, exist_ok=True)
    final_root = output_root / revision
    final_csv = final_root / "paper-trades-with-strategies.csv"
    staging = Path(tempfile.mkdtemp(prefix=f".{revision}.", dir=output_root))
    try:
        staged_csv = staging / final_csv.name
        with staged_csv.open("x", newline="", encoding="utf-8") as output:
            writer = csv.DictWriter(
                output, fieldnames=CSV_COLUMNS, lineterminator="\n"
            )
            writer.writeheader()
            writer.writerows(rows)
            output.flush()
            os.fsync(output.fileno())
        counts_by_class = Counter(row["record_class"] for row in rows)
        counts_by_strategy = Counter(row["strategy_id"] for row in rows)
        manifest = {
            "schema_version": EXPORT_SCHEMA_VERSION,
            "revision": revision,
            "csv_file": final_csv.name,
            "csv_sha256": sha256_file(staged_csv),
            "rows": len(rows),
            "counts_by_class": dict(sorted(counts_by_class.items())),
            "counts_by_strategy": dict(sorted(counts_by_strategy.items())),
            "core_audit": core_evidence,
            "legacy_sources": legacy_evidence,
            "intraday_contract": contract_evidence,
            "real_money_trades": 0,
            "derived_not_live_financial_authority": True,
        }
        write_atomic_json(staging / "manifest.json", manifest)
        if final_root.exists():
            existing_manifest = json.loads((final_root / "manifest.json").read_text())
            if (
                existing_manifest != manifest
                or not final_csv.is_file()
                or sha256_file(final_csv) != manifest["csv_sha256"]
            ):
                raise InvalidTradeHistory(
                    "existing trade-history revision differs from deterministic output"
                )
            shutil.rmtree(staging)
            manifest["idempotent_reuse"] = True
        else:
            os.replace(staging, final_root)
        write_atomic_json(
            output_root / "latest.json",
            {
                "schema_version": EXPORT_SCHEMA_VERSION,
                "revision": revision,
                "dataset_path": str(final_root),
                "csv_path": str(final_csv),
                "manifest_sha256": sha256_file(final_root / "manifest.json"),
            },
        )
        return manifest
    except Exception:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--core-audit",
        type=Path,
        default=Path("var/research-export/trade-history-core-audit.json"),
    )
    parser.add_argument("--legacy-dir", type=Path, default=Path("var/live-paper"))
    parser.add_argument(
        "--output-root",
        type=Path,
        default=Path("var/research-export/paper-trade-history"),
    )
    arguments = parser.parse_args()
    try:
        report = export(
            arguments.core_audit, arguments.legacy_dir, arguments.output_root
        )
        print(
            json.dumps(
                {
                    "revision": report["revision"],
                    "rows": report["rows"],
                    "counts_by_class": report["counts_by_class"],
                    "csv_sha256": report["csv_sha256"],
                },
                sort_keys=True,
            )
        )
        return 0
    except (OSError, ValueError, InvalidTradeHistory) as error:
        print(json.dumps({"error": str(error)}, sort_keys=True))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
