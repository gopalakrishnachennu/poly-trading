"""Profit engine — calibrated probability models for Polymarket + intraday BTC.

This is the giant-system implementation of the plan documented in
scripts/profit_engine/README.md.  Nothing here changes the running live
paper trader or dashboard.  Every module is a research building block that
compiles into a shadow model first, and only after out-of-sample
validation does anything approach real trading.

Package layout maps 1:1 to the plan's five phases:
  collectors/  — phase 1: multi-exchange event capture
  features/    — phase 1: normalisation and per-second feature engine
  labels/      — phase 2: cost-aware label construction
  models/      — phase 2: baselines (market-implied, logistic, LightGBM)
  evaluation/  — phase 2-3: walk-forward, calibration, metrics
"""
__all__ = ["collectors", "features", "labels", "models", "evaluation"]
