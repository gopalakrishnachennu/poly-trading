# ADR 0067: Paper campaigns never autostart or autoresume

## Status

Accepted

## Context

Public-feed discovery is intentionally always available, but a paper campaign
changes local financial simulation state and creates an audit journal. Starting
or resuming it as a side effect of process boot, configuration load, gateway
recovery, or healthy market data violates operator intent and creates confusing
evidence boundaries.

## Decision

Gateway boot performs observation and configuration validation only. A recovered
active campaign is suspended and does not record, decide, reserve, or simulate
orders. A new paper campaign needs an explicit operator request after a
read-only preflight passes all of these gates:

- valid monotonic local clock;
- immutable runtime configuration bound at process start;
- current immutable paper policy permitting BTC and ETH;
- no active campaign;
- non-symlink journal directory that supports create, sync and cleanup.

The browser receives the preflight result but cannot alter it. The gateway
reruns preflight immediately before accepting a start request.

## Consequences

Restarts are safe and do not silently grow a campaign journal. A stopped or
suspended journal remains immutable evidence and can be exported. Public market
observation remains credentialless and independent of paper-campaign consent.
