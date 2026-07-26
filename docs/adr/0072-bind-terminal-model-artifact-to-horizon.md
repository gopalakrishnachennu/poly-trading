# ADR 0072: Bind the terminal model artifact to the BTC-hourly horizon

- Status: Accepted
- Date: 2026-07-25

## Decision

Profit Engine reports are published to a horizon-specific immutable consumer
path such as `var/profit-engine/1h/latest-report.json`. The terminal model API
accepts only an artifact explicitly marked `window: "1h"`; a root-level report
is not an hourly model authority.

## Consequences

A concurrent short-window experiment cannot replace the reported hourly model
evidence. If the hourly artifact is absent or invalid, the terminal states that
fact rather than silently displaying a different horizon. This is a research
display boundary only and grants no strategy, paper-execution, or live-trading
authority.
