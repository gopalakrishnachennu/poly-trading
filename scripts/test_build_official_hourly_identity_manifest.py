#!/usr/bin/env python3
"""Focused contract tests for official hourly identity discovery."""
from __future__ import annotations

import importlib.util
from urllib.parse import parse_qs
import unittest
from datetime import UTC, datetime
from pathlib import Path


MODULE = Path(__file__).with_name("build_official_hourly_identity_manifest.py")
SPEC = importlib.util.spec_from_file_location("identity_builder", MODULE)
assert SPEC and SPEC.loader
identity_builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(identity_builder)


def market(asset: str, start: str, condition: str, *, token_ok: bool = True) -> dict:
    tokens = ["1" * 50, "2" * 50] if token_ok else ["not-a-token", "2" * 50]
    return {
        "title": f"{identity_builder.ASSET_NAMES[asset]} Up or Down Hourly",
        "startDate": start,
        "endDate": "2026-07-24T11:00:00Z",
        "markets": [{
            "question": f"{identity_builder.ASSET_NAMES[asset]} Up or Down Hourly",
            "conditionId": condition,
            "startDate": start,
            "endDate": "2026-07-24T11:00:00Z",
            "outcomes": '["Up","Down"]', "clobTokenIds": tokens,
            "description": "rule", "resolutionSource": "source", "slug": f"{asset}-hour",
        }],
    }


POLICY = {"assets": ["BTC", "ETH"], "required_outcomes": ["Up", "Down"]}
START = int(datetime(2026, 7, 24, 10, tzinfo=UTC).timestamp() * 1000)
END = START + identity_builder.HOUR_MS


class OfficialIdentityTests(unittest.TestCase):
    def test_complete_pair_is_ordered_and_fingerprinted(self) -> None:
        pages = [[
            market("BTC", "2026-07-24T10:00:00Z", "0x" + "a" * 64),
            market("ETH", "2026-07-24T10:00:00Z", "0x" + "b" * 64),
        ]]
        result = identity_builder.discover(pages, POLICY, START, END)
        self.assertEqual([entry["asset"] for entry in result], ["BTC", "ETH"])
        self.assertEqual(result[0]["up_token_id"], "1" * 50)
        self.assertEqual(len(result[0]["rules_fingerprint"]), 64)

    def test_missing_asset_rejects_coverage(self) -> None:
        with self.assertRaisesRegex(identity_builder.InvalidIdentity, "coverage incomplete"):
            identity_builder.discover([[market("BTC", "2026-07-24T10:00:00Z", "0x" + "a" * 64)]], POLICY, START, END)

    def test_duplicate_candidate_rejects(self) -> None:
        duplicate = market("BTC", "2026-07-24T10:00:00Z", "0x" + "c" * 64)
        pages = [[market("BTC", "2026-07-24T10:00:00Z", "0x" + "a" * 64), duplicate, market("ETH", "2026-07-24T10:00:00Z", "0x" + "b" * 64)]]
        with self.assertRaisesRegex(identity_builder.InvalidIdentity, "duplicate"):
            identity_builder.discover(pages, POLICY, START, END)

    def test_invalid_token_rejects_candidate(self) -> None:
        pages = [[market("BTC", "2026-07-24T10:00:00Z", "0x" + "a" * 64, token_ok=False), market("ETH", "2026-07-24T10:00:00Z", "0x" + "b" * 64)]]
        with self.assertRaisesRegex(identity_builder.InvalidIdentity, "outcome/token"):
            identity_builder.discover(pages, POLICY, START, END)

    def test_malformed_timestamp_rejects(self) -> None:
        bad = market("BTC", "not-a-time", "0x" + "a" * 64)
        with self.assertRaisesRegex(identity_builder.InvalidIdentity, "ISO-8601"):
            identity_builder.discover([[bad]], POLICY, START, END)

    def test_fetch_query_is_bounded_to_requested_market_closes(self) -> None:
        policy = {"page_size": 100}
        query = parse_qs(identity_builder.gamma_range_query(policy, START, END, 0))
        self.assertEqual(query["end_date_min"], ["2026-07-24T11:00:00Z"])
        self.assertEqual(query["end_date_max"], ["2026-07-24T11:00:00Z"])
        self.assertEqual(query["ascending"], ["true"])
        self.assertEqual(query["offset"], ["0"])

    def test_fetch_query_rejects_non_hour_boundary(self) -> None:
        with self.assertRaisesRegex(identity_builder.InvalidIdentity, "whole hours"):
            identity_builder.gamma_range_query({"page_size": 500}, START + 1, END, 0)


if __name__ == "__main__":
    unittest.main()
