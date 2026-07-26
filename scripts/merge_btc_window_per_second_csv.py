#!/usr/bin/env python3
"""Compatibility-neutral entry point for verified BTC single-file merges."""

try:
    from scripts.merge_btc_5m_per_second_csv import main
except ModuleNotFoundError:
    from merge_btc_5m_per_second_csv import main


if __name__ == "__main__":
    raise SystemExit(main())
