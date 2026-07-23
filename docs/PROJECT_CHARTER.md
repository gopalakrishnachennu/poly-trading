# Project Charter

## Objective

Build a low-latency, event-driven platform that observes hourly prediction
markets, reconstructs authoritative state, evaluates strategies under explicit
failure scenarios, and permits execution only through independent risk and
signing controls.

The system seeks risk-adjusted net profit. It does not guarantee trades,
principal preservation, or hourly returns. Under strict capital protection it
must frequently choose `NO_TRADE`.

## Operating principles

1. Correctness before latency.
2. Determinism before prediction.
3. Reconciliation before new exposure.
4. Confirmed assets before expected assets.
5. Measured requirements before distributed infrastructure.
6. Replay and failure testing before live execution.

## Initial milestone

Phase 0/1 delivers a recorder foundation:

- canonical financial types;
- versioned event envelopes;
- durable checksummed journaling;
- deterministic decode/replay primitives;
- truncated-tail recovery;
- formal capital-reservation invariants;
- CI-enforced formatting, linting, and tests.

It explicitly excludes order submission, credentials, strategy models,
databases, Kubernetes, streaming brokers, and production deployment.

