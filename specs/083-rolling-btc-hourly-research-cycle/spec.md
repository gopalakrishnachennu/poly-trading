# Phase 5.17 Specification: Rolling BTC-Hourly Research Cycle

## Objective

Run the accepted fixed-point BTC-hourly research pipeline repeatedly over
newly completed local sessions without allowing mutable capture files,
concurrent jobs, failed models or mutable convenience reports to become
strategy authority.

## Scope

- Freeze newline-complete prefixes of append-only book and reference captures
  into checksum-addressed source blobs.
- Bind every cycle to exact source, historical evidence and policy digests.
- Normalize only completed atomic UP/DOWN sessions.
- Merge, rebuild and evaluate the path classifier and path net-value challenger.
- Publish create-new cycle evidence and one atomic, non-authorizing latest
  research pointer.
- Support one-shot and policy-cadenced operation under an exclusive process
  lock.

## Exclusions

- Final-test bypass, live promotion, parameter mutation, live orders,
  credentials, signing or wallet access. Automatic paper-only registration is
  governed separately by Phase 5.18.

## Acceptance criteria

- An active partial JSON line is never copied into a frozen source.
- Source truncation/replacement, checksum collision, excessive source count or
  size, concurrent execution, step failure, and malformed verdict fail closed.
- Repeated identical completed evidence resolves to the same cycle and does not
  retrain.
- A rejected model publishes `NO_TRADE`.
- With explicit automatic-admission policy, a passing model publishes
  `PAPER_AUTO_ADMITTED`; it grants paper-simulation eligibility but no live
  authority.
