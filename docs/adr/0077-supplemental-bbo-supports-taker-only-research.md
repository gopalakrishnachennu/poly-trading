# ADR 0077: Supplemental BBO supports taker-only research

- Status: Accepted
- Date: 2026-07-26

## Decision

The revision-pinned Hugging Face BBO archive may be normalized into one-level
fixed-point books after official identity verification. It can support only
immediate-taker entry/exit research when the requested quantity fits inside
the displayed best level. It cannot support passive queue-position claims or
full-depth slippage claims.

## Consequences

The platform gains hundreds of independent BTC-hourly path observations at
bounded storage cost while preserving an honest capability boundary. Full-L2
PMXT or locally captured depth remains necessary for queue-aware and deeper
size research. No model or execution authority follows from normalization.
