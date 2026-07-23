# Phase 4.6 — Terminal Telemetry Resilience

## Objective

Keep the read-only terminal responsive while preserving a clear, fail-closed
distinction between live market eligibility, paper status, and replay-audit
availability.

## Acceptance criteria

1. Fast paper status polling never rereads the complete paper journal.
2. Full replay-integrity verification is bounded and cached, so concurrent
   dashboard tabs do not repeatedly block the paper recorder.
3. A dashboard loading or audit failure is labelled as reconciliation/audit
   state and never presented as an idle campaign or a market trading signal.
4. Any market-feed failure remains `NO_TRADE`; telemetry optimisation cannot
   make stale market data eligible.
