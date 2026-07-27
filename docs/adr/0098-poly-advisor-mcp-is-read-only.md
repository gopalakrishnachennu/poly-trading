# ADR 0098: Poly Advisor MCP Is Read-Only

## Status

Accepted

## Context

Operators want Claude or ChatGPT to analyze the BTC/ETH window paper book every
few minutes, propose profit patterns, and help draft specs. Putting the model in
the every-second decision path would make sizing non-deterministic, unauditable,
and expensive, and would violate the platform rule that strategy code never
signs or submits orders.

## Decision

- Ship a laptop-local **Poly Advisor MCP** that exposes live paper data,
  journals, 1-second snapshots, analytics, what-ifs, patterns, suggestions,
  spec drafting, ops, and reports.
- The MCP is **read-only with respect to trading authority**: it may write only
  to `var/poly-advisor/` (snapshots, suggestions, patterns, reports).
- It must not expose place/cancel/sign tools, mutate engine config, or hold
  wallet material.
- Local capture runs at 1 Hz; LLM analysis is on-demand or scheduled by the
  host — never required every second.
- Accepted suggestions become new specs and code through the normal human-gated
  workflow; the engine remains the only place that proposes paper trades.

## Consequences

Claude/ChatGPT become a research cockpit over durable paper truth. Profit
improvements still require encoded specs (for example tightening entries or
capping tiers when expectancy is negative). Operators must keep API keys in
local host config, not in Git.
