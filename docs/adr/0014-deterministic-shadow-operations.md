# ADR 0014: Keep Shadow Operational Decisions Deterministic

## Status

Accepted

## Context

The read-only pipeline can be mathematically correct while still becoming
unsafe through a stalled event loop, disk growth, file-descriptor exhaustion,
memory pressure, queue saturation, slow ticks, invalid process sequencing, or
misleading monitoring. Direct system calls inside the safety core would make
those decisions difficult to replay and test.

## Decision

Add `shadow-ops` as a pure operational state machine. Platform adapters supply
explicit time, runtime state, progress, queue, RSS, file, journal, and latency
samples. Exact resource limits are accepted; ordinary budget excess produces a
recoverable degraded mode. Clock/sequence regression, impossible queue state,
future or missing progress, watchdog expiry, and runtime/coordinator halt are
permanent integrity failures.

Drain is explicit and prevents return to ready. Stop requires drain unless the
underlying runtime is already closed or shut down. Metrics are rendered in a
stable identifier-free OpenMetrics form.

Named stress profiles use a non-durable counting journal only for deterministic
capacity measurement. This journal can never replace the production segmented
journal or its recovery tests.

## Consequences

- Operational decisions can be replayed from recorded samples.
- Resource pressure is visible without being confused with data integrity loss.
- Watchdog and lifecycle failures are testable without sleeping or signals.
- Prometheus, Grafana, systemd, and Kubernetes remain optional adapters rather
  than hot-path dependencies.
