# Read-Only Shadow Operations Runbook

## Safety boundary

This runbook operates only the public, unauthenticated shadow stack. It must not
be supplied with wallet keys, API credentials, signing access, or order routes.
`READY` means data and operational gates are healthy; it does not authorize a
trade.

## Startup gate

1. Confirm the host clock is synchronized and monotonic-wall-clock checks pass.
2. Confirm journal and checkpoint targets are dedicated paths with sufficient
   free space and are not symbolic links.
3. Run `cargo test --workspace --locked` and all TLC models for the release.
4. Run `cargo run -p shadow-ops -- stress smoke`.
5. Recover the latest session journal with its checkpoint and compare the
   reported digest with the previous clean shutdown record.
6. Start public market capture, reference capture, replay actors, supervision,
   session runtime, then operational supervision in that order.
7. Enable shadow readiness only after every layer reports healthy provenance.

## Readiness interpretation

- `Ready`: runtime and coordinator are healthy, progress is within watchdog,
  and every configured resource is at or below its limit.
- `Degraded`: one recoverable resource budget is exceeded. New downstream
  exposure remains forbidden; investigate the reason code.
- `Draining`: shutdown is in progress. The process cannot return to ready.
- `Stopped`: drain completed or the underlying runtime terminated cleanly.
- `Halted`: integrity failure. Restart only after durable diagnosis and replay.

## Degradation response

### RSS budget

Capture a process memory profile, verify queue and session counts, and compare
with the latest stress profile. Drain if usage continues growing. Do not raise
the limit until the growth cause is understood.

### Open-file budget

Inspect journal segments, sockets, and descriptors. Look for unclosed epochs or
checkpoint files. Drain before approaching the host hard limit.

### Journal-size budget

Stop new shadow capture, synchronize active segments, verify a checkpoint, and
archive only closed segments. Never delete the active or unverified recovery
prefix.

### Ingress watermark

Treat sustained depth as consumer lag. Verify tick latency and disk sync time.
Do not add an unbounded queue. Drain if the configured capacity could be hit.

### Tick-latency budget

Compare CPU saturation, disk latency, and frame size. Readiness may recover on
the next healthy sample, but repeated violations require drain and profiling.

## Integrity-halt response

1. Freeze the process and preserve journals, checkpoints, logs, configuration,
   and the last metrics snapshot.
2. Do not truncate, skip, rewrite, or hand-edit a corrupted record.
3. Identify the first failing sequence and reproduce it with strict replay.
4. Compare local intent, feed journal, replay digest, supervisor provenance,
   session digest, and checkpoint prefix.
5. If a crash left an incomplete tail, use only the explicit recorder recovery
   procedure and retain a forensic copy first.
6. Re-run replay and the relevant fault test before returning to shadow mode.

## Graceful drain and stop

1. Request `begin_drain`; verify mode becomes `Draining`.
2. Stop accepting new integration ticks.
3. Let queued frames complete without exceeding the watchdog policy selected
   for draining.
4. Synchronize journals and write a create-new checkpoint.
5. Strictly recover the journal using that checkpoint and compare digests.
6. Close feed epochs and runtime channels.
7. Mark stopped and record sequence, digest, segment count, and resource gauges.

## Stress gates

Run the named profiles before a release:

```text
cargo run -p shadow-ops -- stress smoke
cargo run -p shadow-ops -- stress day
cargo run -p shadow-ops -- stress seven-day
```

The counting journal measures deterministic encoded volume and sync boundaries;
it is intentionally non-durable. Segmented-journal restart tests remain a
separate mandatory gate.

## External-network gate

The Binance Phase 1.5 smoke must pass from the intended, legally eligible
deployment network before enabling live reference capture. Record endpoint,
region, timestamp, TLS result, WebSocket subscription result, first complete
BTC/ETH feed set, disconnect behavior, and journal/replay digest. Do not route
around geographic or provider restrictions.

The development-network smoke passed on 2026-07-17. See
[`../evidence/phase-1.5-binance-live-smoke-2026-07-17.md`](../evidence/phase-1.5-binance-live-smoke-2026-07-17.md).
Repeat this gate for every intended deployment region; evidence from one network
does not establish legal or technical eligibility elsewhere.
