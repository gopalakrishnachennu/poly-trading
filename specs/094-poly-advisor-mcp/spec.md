# Specification 094: Poly Advisor MCP (Read-Only Research Cockpit)

## Objective

Give Claude, ChatGPT, and Cursor a laptop-local MCP that can read live paper
state, historical journals, 1-second snapshots, and a durable suggestion /
pattern library — so an operator can discover profit patterns and draft spec
changes without ever letting an LLM place, size, or cancel a trade.

## Problem

The window paper book already journals every decision and exposes a rich status
API, but that truth is hard for an external model to consume safely. Operators
want live + historical analysis, what-if replays, and a pattern library. Wiring
an LLM into the bet path would break determinism, auditability, and the
platform's financial invariants.

## Scope

- Local TypeScript MCP server (`advisor/`) over stdio.
- Local snapshot writer that polls the paper API every second and stores
  compressed feature rollups under `var/poly-advisor/` (gitignored).
- Tools covering: data, analytics/what-if, patterns, suggestions, specs, ops,
  reports, and guardrails (see acceptance criteria).
- Resources for status, features, scoreboard, policy, prompts, patterns, and
  latest suggestion.
- Claude Desktop / ChatGPT connection docs and run scripts.
- Hard exclusion of any place/cancel/sign/live-authority tool.

## Exclusions

- Live trading, wallets, credentials, order signing, or Paper Week capital.
- Auto-applying config or stake changes into `btc-window-paper`.
- Calling Anthropic/OpenAI APIs from the MCP itself (the host model calls tools).
- Replacing Spec 093 sizing inside the engine.

## Data roots

| Path | Role |
|---|---|
| `var/btc-window-paper/` | Engine journal + status (source of truth) |
| `http://127.0.0.1:3000/api/btc-window-paper` | Live enriched API (default) |
| `var/poly-advisor/snapshots/` | 1s status snapshots |
| `var/poly-advisor/features/` | LLM-sized rollups |
| `var/poly-advisor/tape/` | Decision ring buffer |
| `var/poly-advisor/suggestions/` | Advisor run store |
| `var/poly-advisor/patterns/` | Pattern library |
| `var/poly-advisor/reports/` | Exported day briefs |
| `var/poly-advisor/prompts/` | System prompt copies |

Money fields remain integer micros / bps. Display may format dollars as text;
calculations must not use binary floating point for P&L math beyond integer
division of micros.

## Acceptance criteria

1. MCP starts over stdio and lists all catalogued tools (data through guardrails).
2. Live tools return paper status, capital, open book, scoreboard, diagnostics,
   recent closes/decisions, and health without mutating the engine.
3. Historical tools can read today's journal slice and stored snapshots/features.
4. Analytics tools can rank kill/keep, find death patterns, compare days when
   archives exist, audit stake tiers, and run what-if disable / cap-tier /
   min-sample replays on day closes using integer micros.
5. Patterns and suggestions can be saved, listed, accepted, rejected, and
   status-transitioned on local disk only.
6. Spec tools can emit a draft delta and validate proposals against paper-only
   guardrails and the allowed action menu.
7. Ops/report tools produce overview, halt diagnosis, disk usage, daily
   briefing, trade explain, and exportable markdown/JSON reports.
8. Guardrail tools confirm paper-only and that no live-order tools exist.
9. Snapshot daemon writes at ~1 Hz and updates `features/latest.json` plus a
   live tape buffer.
10. Documentation shows how to connect Claude Desktop and ChatGPT without
    granting order authority.
11. Unit tests cover micros math, what-if filtering, proposal validation, and
    pattern/suggestion store round-trips.
