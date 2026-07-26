# Phase 5.0 Specification: BTC Hourly Strategy Governance

## Objective

Prevent an unvalidated, fixed-threshold intraday paper challenger from being
mistaken for the profit engine. The managed research stack must focus its
automated model-evaluation cycle on BTC one-hour evidence and must not start
the losing challenger automatically.

## Scope

- Retire automatic startup of `live_paper_intraday.py`; preserve its historical
  reports and JSONL observations read-only. A direct invocation must also deny
  new simulated entries unless two exact deployment acknowledgements are set.
- Change the managed Profit Engine cadence to BTC one-hour research data.
- Surface a `RETIRED_UNVALIDATED` status in the terminal for the historical
  fixed-threshold challenger.
- Make the terminal explain that a model report is research evidence, not a
  strategy approval, and that `NO_MODEL_BEATS_BASELINE` means no model is
  eligible for paper promotion.

## Exclusions

- Model promotion, order placement, credentials, wallets, signing, real-money
  execution, changing historical records, deleting existing data, or claiming
  a profitable strategy.

## Acceptance criteria

- A new managed stack start does not launch the fixed-threshold intraday
  process unless an explicit deployment configuration enables it.
- A stale pre-retirement supervised worker is not allowed to resume entries
  after restart using only its former enable flag.
- The default recurring profit-engine command targets the one-hour dataset.
- The UI distinguishes historical challenger P&L from an approved strategy and
  displays the research verdict without implying approval.
- Existing historical paper observations remain readable and no state file is
  altered by the status change.
