# Paper Trade History Export

This runbook produces the consolidated strategy-attributed paper-trade CSV.
The result is historical evidence only and contains no real-money trades.

## Build and verify core journals

```bash
cargo build -p terminal-projection --bin audit_paper_trade_journals
target/debug/audit_paper_trade_journals \
  var/research-export/trade-history-core-audit.json \
  var/paper-campaign/paper-*.jsonl
```

The audit streams every journal, checks its complete newline-terminated tail,
BLAKE3 record digests, contiguous sequence, stable campaign identifier, and the
exact relationship between each `PAIR_BUY` decision and `filled_pair` event.

## Export all paper history

```bash
python3 scripts/export_paper_trades_with_strategies.py
```

The immutable revision and its manifest are written beneath:

```text
var/research-export/paper-trade-history/<revision>/
```

The current content-addressed revision is referenced by:

```text
var/research-export/paper-trade-history/latest.json
```

## Authority classes

- `CORE_PAPER_PAIR`: verified fixed-point core paper fills.
- `LEGACY_DIRECTIONAL_SETTLEMENT`: retired directional simulator evidence.
- `LEGACY_INTRADAY_ROUND_TRIP`: retired intraday simulator evidence whose
  strategy attribution is bound to the recorded simulator contract checksum.

Do not add these independent simulator books together as one portfolio. No row
represents a real-money trade.
