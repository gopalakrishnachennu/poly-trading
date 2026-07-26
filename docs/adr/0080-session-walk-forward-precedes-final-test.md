# ADR 0080: Session walk-forward validation precedes the final test

- Status: Accepted
- Date: 2026-07-26

## Decision

BTC-hourly intraday models are selected only through expanding-window,
whole-session validation. The final chronological sessions are reserved before
selection and opened once after the objective, horizon and threshold are frozen.
Order-book, spot and flow joins use event timestamps no later than the
decision's configured source cutoff.

## Consequences

Millions of correlated rows cannot masquerade as millions of independent
markets, and repeatedly tuning against a losing final report cannot manufacture
a promotable result. A green development result may still fail the final fold;
that failure remains visible and requires genuinely later evidence, not another
search over the same test period.
