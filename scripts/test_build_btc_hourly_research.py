from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from build_btc_hourly_research import InvalidTape, Policy, walk


POLICY = {
    "schema_version": 1, "asset": "BTC", "window_ms": 3_600_000,
    "decision_spacing_ms": 5_000, "strategy_latency_ms": 250,
    "max_source_time_lead_ms": 30_000,
    "start_buffer_ms": 120_000, "end_buffer_ms": 120_000,
    "max_future_frame_gap_ms": 15_000, "entry_shares_micros": 20_000_000,
    "taker_fee_reserve_bps": 100, "minimum_completed_sessions": 100,
    "horizons_ms": [15_000, 30_000, 60_000],
}


def record(kind: str, **fields: object) -> dict:
    return {"k": kind, **fields}


class BtcHourlyResearchTests(unittest.TestCase):
    def test_depth_walk_is_conservative_and_requires_full_quantity(self) -> None:
        self.assertEqual(
            walk([[200_000, 10_000_000], [300_000, 10_000_000]], 20_000_000, True),
            (5_000_000, 20_000_000, 2),
        )
        self.assertEqual(
            walk([[300_000, 10_000_000], [200_000, 10_000_000]], 20_000_000, False),
            (5_000_000, 20_000_000, 2),
        )
        self.assertIsNone(walk([[200_000, 10_000_000]], 20_000_000, True))

    def test_builder_refuses_to_make_model_from_insufficient_sessions(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy = root / "policy.json"
            tape = root / "tape.jsonl"
            out = root / "out"
            policy.write_text(json.dumps(POLICY))
            condition, up, down = "condition", "up", "down"
            start, end = 1_000_000, 4_600_000
            identity = {
                "event_type": "market_identity", "asset": "BTC",
                "condition_id": condition, "up_token_id": up, "down_token_id": down,
                "start_time_ms": start, "end_time_ms": end,
            }
            rows = [record("sys", payload=identity)]
            for offset in range(120_000, 241_000, 5_000):
                for token in (up, down):
                    at_ns = (start + offset) * 1_000_000
                    rows.append(record(
                        "book", cond=condition, tok=token, seq=len(rows), t=at_ns,
                        r=at_ns + 1, bids=[[390_000, 25_000_000]],
                        asks=[[400_000, 25_000_000]],
                    ))
            tape.write_text("".join(json.dumps(row) + "\n" for row in rows))
            completed = subprocess.run(
                [sys.executable, str(Path(__file__).parent / "build_btc_hourly_research.py"),
                 "--input", str(tape), "--policy", str(policy), "--out-dir", str(out)],
                text=True, capture_output=True, check=False,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            result = json.loads((out / "NO_MODEL.json").read_text())
            self.assertEqual(result["eligibility"], "NO_MODEL_INSUFFICIENT_SESSIONS")
            self.assertFalse((out / "train.jsonl").exists())

    def test_non_monotonic_book_side_is_rejected(self) -> None:
        self.assertIsNone(
            walk([[300_000, 20_000_000], [200_000, 20_000_000]], 30_000_000, True)
        )


if __name__ == "__main__":
    unittest.main()
