#!/usr/bin/env python3
"""Compatibility-neutral entry point for BTC per-second window exports."""

try:
    from scripts.export_btc_5m_per_second import main
except ModuleNotFoundError:
    from export_btc_5m_per_second import main


if __name__ == "__main__":
    raise SystemExit(main())
