# Phase 1.11 Specification: Shadow Operations and Stress Gates

## Objective

Add deterministic operational health, watchdog, resource-budget, lifecycle,
metrics, stress-profile, and runbook controls around the read-only shadow
system without introducing external infrastructure or live trading authority.

## Requirements

1. The operational core receives caller-supplied samples and reads no clock,
   process, filesystem, or network state directly.
2. Runtime/coordinator halt, clock regression, sequence regression, impossible
   queue depth, future progress time, missing progress, and watchdog expiry are
   permanent integrity halts.
3. RSS, open-file, journal-size, ingress-watermark, and tick-latency budget
   excesses are explicit recoverable degradation modes.
4. Exact budget and watchdog boundaries are accepted.
5. Drain prevents return to ready, stop requires drain or an already terminal
   runtime, and halt is absorbing.
6. Immutable snapshots expose mode, reason, resource gauges, counters,
   sequence, evaluation/progress time, and a stable digest.
7. OpenMetrics rendering is deterministic, bounded, label-safe, and contains
   no dynamic user or market identifiers.
8. Stress profiles are bounded presets for smoke, one day, and seven days.
   Stress counting journals are explicitly non-durable and never usable by the
   production runtime.
9. Seven-day stress must finalize every generated BTC/ETH session and produce
   deterministic record, byte, sync, readiness, and digest results.
10. Operator runbooks define startup, readiness, degradation, integrity halt,
    checkpoint recovery, disk pressure, graceful drain, and network-gate steps.

## Acceptance criteria

- Tests cover all lifecycle modes, exact boundaries, recovery, absorbing halt,
  clock/sequence/watchdog failures, metrics stability, profile bounds, and a
  complete seven-day stress run.
- CLI runs named profiles and prints a stable machine-readable summary.
- A formal model verifies readiness, degradation recovery, drain/stop ordering,
  and absorbing halt.
- Formatting, denied-warning Clippy, all tests, and all TLC models pass.

## Exclusions

Prometheus/Grafana deployment, Kubernetes, cloud services, external paging,
live networking, strategies, authentication, orders, wallets, and signing.
