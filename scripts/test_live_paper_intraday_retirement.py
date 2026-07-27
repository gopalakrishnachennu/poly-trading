#!/usr/bin/env python3
"""Regression tests for the retired fixed-threshold intraday challenger."""
from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


MODULE = Path(__file__).with_name("live_paper_intraday.py")
SPEC = importlib.util.spec_from_file_location("live_paper_intraday", MODULE)
assert SPEC and SPEC.loader
intraday = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(intraday)

LEGACY_MODULE = Path(__file__).with_name("live_paper_trader.py")
LEGACY_SPEC = importlib.util.spec_from_file_location("live_paper_trader", LEGACY_MODULE)
assert LEGACY_SPEC and LEGACY_SPEC.loader
legacy = importlib.util.module_from_spec(LEGACY_SPEC)
LEGACY_SPEC.loader.exec_module(legacy)


class IntradayRetirementTests(unittest.TestCase):
    def test_missing_acknowledgement_never_enables_new_entries(self) -> None:
        self.assertFalse(intraday.experiment_enabled({}))
        self.assertFalse(intraday.experiment_enabled({"POLY_ENABLE_UNVALIDATED_INTRADAY": "1"}))
        self.assertFalse(intraday.experiment_enabled({"POLY_INTRADAY_EXPERIMENT_ACK": intraday.EXPERIMENT_ACKNOWLEDGEMENT}))

    def test_both_exact_deployment_acknowledgements_are_required(self) -> None:
        self.assertTrue(intraday.experiment_enabled({
            "POLY_ENABLE_UNVALIDATED_INTRADAY": "1",
            "POLY_INTRADAY_EXPERIMENT_ACK": intraday.EXPERIMENT_ACKNOWLEDGEMENT,
        }))

    def test_retirement_frame_is_explicit_and_non_authorizing(self) -> None:
        frame = intraday.retirement_frame()
        self.assertEqual(frame["strategy_status"], "DISABLED")
        self.assertFalse(frame["new_entries_allowed"])
        self.assertTrue(frame["historical_evidence_only"])

    def test_legacy_directional_simulator_is_retired_without_two_flags(self) -> None:
        self.assertFalse(legacy.legacy_experiment_enabled({}))
        self.assertFalse(legacy.legacy_experiment_enabled({"POLY_ENABLE_LEGACY_DIRECTIONAL_SIMULATOR": "1"}))
        self.assertTrue(legacy.legacy_experiment_enabled({
            "POLY_ENABLE_LEGACY_DIRECTIONAL_SIMULATOR": "1",
            "POLY_LEGACY_DIRECTIONAL_ACK": legacy.EXPERIMENT_ACKNOWLEDGEMENT,
        }))
        frame = legacy.retirement_frame()
        self.assertEqual(frame["strategy_status"], "DISABLED")
        self.assertFalse(frame["new_entries_allowed"])
        self.assertEqual(legacy.DEFAULT_SLATE, [])
        for name in legacy.DISABLED_LEGACY_TRIGGERS:
            self.assertIn(name, legacy.BUILDERS)


if __name__ == "__main__":
    unittest.main()
