# Profit Engine — Implementation Notes

Multi-phase implementation of the calibrated LightGBM plan.  Nothing in this
package changes existing live paper trading or dashboard behavior.  All code
is research infrastructure; execution wiring is a separate deliberate step
that only happens after out-of-sample paper validation.

## Phase mapping (from the plan)

| Plan section | This package |
|---|---|
| §4 Data collection | `collectors/binance_spot.py`, existing Polymarket capture |
| §5 Feature set | `features/one_second.py` |
| §6 Model architecture | `models/market_baseline.py`, `models/logistic.py`, `models/calibration.py` (LightGBM slot: see below) |
| §7 Polymarket trading rule | `labels/cost_aware.py` (labels), model output → rule application (later) |
| §10 Backtesting | `evaluation/walk_forward.py`, `evaluation/metrics.py` |
| §11 Phase 3 paper | `paper_shadow_runner.py` (later — after models validate) |

## What's built now vs later

### Built in this pass (Phase 1 foundation)

1. **`collectors/binance_spot.py`** — Binance BTC/ETH spot price REST poller.
   Every 1s writes normalised events (`t_recv_ms`, `t_event_ms`, `symbol`,
   `bid`, `ask`, `mid`, `bid_sz`, `ask_sz`, `seq`) to
   `var/research-binance/{symbol}-YYYY-MM-DD.jsonl`.  Rate limit safe.
   Can be upgraded to WebSocket later with same schema.
2. **`features/one_second.py`** — reads captured Polymarket `research-1h/*.jsonl`
   and Binance `research-binance/*.jsonl`; produces per-second feature rows
   with all §5 core features (OBI, microprice, spread, cross-exchange lead,
   time-to-expiry, distance-to-threshold z-score).
3. **`labels/cost_aware.py`** — for every decision frame, compute cost-aware
   label per the plan: `y=1` iff terminal outcome pays after taker fee,
   slippage estimate, safety margin.  Not just "did YES win?"
4. **`models/market_baseline.py`** — the "trust the executable ask" baseline
   that every model must beat.
5. **`models/logistic.py`** — pure-Python logistic regression trained by
   L2-regularised gradient descent.  No external deps.  Serves as the
   second required baseline.
6. **`models/calibration.py`** — Platt scaling (small-sample) and isotonic
   regression (large-sample) implementations, pure Python.
7. **`evaluation/walk_forward.py`** — time-split trainer/validator/tester
   with rolling windows, no random shuffle, no leakage (session-independent
   splits per ADR 0068 for BTC 1h).
8. **`evaluation/metrics.py`** — Brier score, log loss, reliability curve,
   expected calibration error, per-confidence-bucket precision.

### Deferred (needs pip installs, larger scope, or more captured data)

- **LightGBM model** (`models/lightgbm_model.py`) — requires `pip install
  lightgbm`.  Placeholder interface defined so training/eval scripts can
  swap it in when installed.
- **Temporal CNN / DeepLOB** — Phase 4 optional ensemble.  Needs PyTorch.
- **Hawkes intensities** — Phase 4 feature generator.  Manual implementation
  or `tick` package.
- **Coinbase + Binance perp collectors** — same schema as spot, one file
  each.  Straightforward extension.
- **WebSocket upgrades for all collectors** — moves latency from ~1s to
  ~100ms.  Requires `pip install websockets`.
- **Timescale/ClickHouse storage** — current design uses JSONL + Parquet
  (via `pyarrow`).  Real DB is a Phase 3 concern.
- **Shadow-mode API integration** — read-only account, no orders — Phase 4.

## Data dependencies

Baseline (pure Python, no pip):
  logistic + calibration + walk-forward + metrics.  Trainable on existing
  Polymarket captures today.

Optional upgrades:
```
pip install lightgbm scikit-learn numpy pandas pyarrow websockets
```

## How to run (once collectors have captured for a few days)

```
# 1. Start the Binance collector alongside the existing supervisor
python3 scripts/profit_engine/collectors/binance_spot.py

# 2. Build per-second features from captured data (run periodically)
python3 -m scripts.profit_engine.features.one_second \
    --polymarket var/research-15m/*.jsonl \
    --binance var/research-binance/*.jsonl \
    --out var/features/features-15m.parquet

# 3. Build cost-aware labels aligned to those features
python3 -m scripts.profit_engine.labels.cost_aware \
    --features var/features/features-15m.parquet \
    --out var/features/labels-15m.parquet

# 4. Walk-forward-train + calibrate + evaluate the baseline
python3 -m scripts.profit_engine.evaluation.walk_forward \
    --features var/features/features-15m.parquet \
    --labels var/features/labels-15m.parquet \
    --model logistic \
    --report var/profit-engine/logistic-15m-report.json
```

## Non-goals

- **No live execution.**  This package does not open positions, does not sign
  orders, does not connect to any authenticated endpoint.
- **No changes to the running trader or dashboard.**  The current paper
  intraday tracker keeps collecting evidence in parallel.
- **No claim of profitability until walk-forward on 100+ sessions across
  mixed regimes shows a positive risk-adjusted edge after fees.**  That is
  the honest gate before this package is allowed to influence anything.
