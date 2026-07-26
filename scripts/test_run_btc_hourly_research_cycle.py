#!/usr/bin/env python3
"""Failure-oriented tests for rolling BTC-hourly research cycles."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from scripts import run_btc_hourly_research_cycle as subject


class RollingResearchCycleTests(unittest.TestCase):
    def test_freeze_excludes_only_incomplete_active_tail(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "active.jsonl"
            source.write_bytes(b'{"complete":1}\n{"partial":')
            result = subject.freeze_jsonl_prefix(source, root / "blobs", 10_000, 10)
            blob = Path(result["path"])
            self.assertEqual(blob.read_bytes(), b'{"complete":1}\n')
            self.assertEqual(result["maximum_rows"], 1)
            self.assertEqual(result["excluded_incomplete_tail_bytes"], len(b'{"partial":'))
            self.assertEqual(subject.sha256_file(blob), result["sha256"])

    def test_same_prefix_reuses_content_addressed_blob(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "capture.jsonl"
            source.write_bytes(b'{"one":1}\n')
            first = subject.freeze_jsonl_prefix(source, root / "blobs", 10_000, 10)
            second = subject.freeze_jsonl_prefix(source, root / "blobs", 10_000, 10)
            self.assertEqual(first["path"], second["path"])
            self.assertEqual(first["sha256"], second["sha256"])

    def test_config_rejects_unknown_or_zero_budget(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = json.loads(
                Path("config/btc-hourly-rolling-research.example.json").read_text()
            )
            config["retrain_interval_seconds"] = 0
            path = root / "config.json"
            path.write_text(json.dumps(config))
            with self.assertRaisesRegex(subject.InvalidRollingCycle, "positive integer"):
                subject.read_config(path, Path.cwd())

    def test_passing_model_is_paper_only_and_has_no_external_authority(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            model = root / "model.txt"
            model.write_text("frozen-model")
            report = {
                "verdict": "PAPER_CANDIDATE_ONLY",
                "final_test_evaluated": True,
                "final_metrics": {"trades": 10, "net_pnl_micros": "1000000"},
                "model_sha256": subject.sha256_file(model),
            }
            (root / "report.json").write_text(json.dumps(report))
            admission = subject.build_paper_admission(
                "a" * 64, [("path_net_value", report, root)],
            )
            self.assertEqual(admission["status"], "AUTO_ADMITTED_PAPER_ONLY")
            self.assertTrue(admission["paper_simulation_eligible"])
            self.assertFalse(admission["capital_authority"])
            self.assertFalse(admission["live_trading_authority"])

    def test_candidate_checksum_mismatch_rejects_admission(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "model.txt").write_text("changed")
            report = {
                "verdict": "PAPER_CANDIDATE_ONLY",
                "final_test_evaluated": True,
                "final_metrics": {"trades": 10},
                "model_sha256": "0" * 64,
            }
            (root / "report.json").write_text(json.dumps(report))
            with self.assertRaisesRegex(subject.InvalidRollingCycle, "incomplete"):
                subject.build_paper_admission(
                    "a" * 64, [("path_net_value", report, root)],
                )

    def test_registry_republish_verifies_evidence_and_denies_authority(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cycles = root / "research-cycles"
            cycle = cycles / ("a" * 64)
            cycle.mkdir(parents=True)
            admission = subject.build_paper_admission("a" * 64, [])
            admission_path = cycle / "paper-admission.json"
            admission_path.write_bytes(subject.canonical(admission) + b"\n")
            report = {
                "revision": "a" * 64,
                "decision": "NO_TRADE",
                "paper_admission_sha256": subject.sha256_file(admission_path),
            }
            subject.publish_paper_registry(cycles, cycle, report)
            registry = json.loads(
                (root / "paper-model-registry" / "latest.json").read_text()
            )
            self.assertEqual(registry["status"], "NO_ELIGIBLE_MODEL")
            self.assertFalse(registry["paper_simulation_eligible"])
            self.assertEqual(registry["models"], [])

            admission["submission_authority"] = True
            admission_path.write_bytes(subject.canonical(admission) + b"\n")
            report["paper_admission_sha256"] = subject.sha256_file(admission_path)
            with self.assertRaisesRegex(
                subject.InvalidRollingCycle, "submission_authority"
            ):
                subject.publish_paper_registry(cycles, cycle, report)


if __name__ == "__main__":
    unittest.main()
