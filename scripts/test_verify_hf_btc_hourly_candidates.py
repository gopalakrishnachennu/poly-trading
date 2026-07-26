#!/usr/bin/env python3
"""Contract tests for official verification of supplemental hourly candidates."""
from __future__ import annotations

import importlib.util
import unittest
from datetime import UTC, datetime
from pathlib import Path


MODULE = Path(__file__).with_name("verify_hf_btc_hourly_candidates.py")
SPEC = importlib.util.spec_from_file_location("candidate_verifier", MODULE)
assert SPEC and SPEC.loader
candidate_verifier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(candidate_verifier)

START = int(datetime(2026, 4, 11, 0, tzinfo=UTC).timestamp() * 1000)
CONDITION = "0x" + "a" * 64
UP = "1" * 50
DOWN = "2" * 50


def selected(*, resolution: int = 1) -> dict:
    return {
        "101": {
            "market_id": "101",
            "start_time_ms": START,
            "end_time_ms": START + candidate_verifier.HOUR_MS,
            "condition_id": CONDITION,
            "up_token_id": UP,
            "down_token_id": DOWN,
            "resolution": resolution,
        }
    }


def official(*, condition: str = CONDITION, prices: str = '["1","0"]', start: str = "2026-04-11T00:00:00Z") -> dict:
    return {
        "id": "101",
        "question": "Bitcoin Up or Down - April 10, 8PM ET",
        "conditionId": condition,
        "eventStartTime": start,
        "endDate": "2026-04-11T01:00:00Z",
        "outcomes": '["Up","Down"]',
        "clobTokenIds": f'["{UP}","{DOWN}"]',
        "description": "BTC/USDT hourly candle rule",
        "resolutionSource": "https://www.binance.com/en/trade/BTC_USDT",
        "outcomePrices": prices,
        "closed": True,
        "slug": "bitcoin-up-or-down-april-10-8pm-et",
    }


class CandidateVerificationTests(unittest.TestCase):
    def test_valid_official_market_produces_fixed_label(self) -> None:
        identities, labels = candidate_verifier.validate_official(selected(), [official()])
        self.assertEqual(len(identities), 1)
        self.assertEqual(labels[0]["resolved_side"], "UP")
        self.assertEqual(labels[0]["start_time_ms"], START)

    def test_condition_mismatch_rejects(self) -> None:
        with self.assertRaisesRegex(candidate_verifier.InvalidIdentity, "identity disagree"):
            candidate_verifier.validate_official(selected(), [official(condition="0x" + "b" * 64)])

    def test_resolution_mismatch_rejects(self) -> None:
        with self.assertRaisesRegex(candidate_verifier.InvalidIdentity, "resolution disagree"):
            candidate_verifier.validate_official(selected(), [official(prices='["0","1"]')])

    def test_missing_supplemental_resolution_uses_closed_official_label(self) -> None:
        identities, labels = candidate_verifier.validate_official(
            selected(resolution=-1),
            [official(prices='["0","1"]')],
        )
        self.assertEqual(len(identities), 1)
        self.assertEqual(labels[0]["resolved_side"], "DOWN")

    def test_non_hourly_official_time_rejects(self) -> None:
        with self.assertRaisesRegex(candidate_verifier.InvalidIdentity, "not BTC Up or Down"):
            candidate_verifier.validate_official(selected(), [official(start="2026-04-11T00:05:00Z")])

    def test_missing_official_evidence_rejects(self) -> None:
        with self.assertRaisesRegex(candidate_verifier.InvalidIdentity, "coverage incomplete"):
            candidate_verifier.validate_official(selected(), [])


if __name__ == "__main__":
    unittest.main()
