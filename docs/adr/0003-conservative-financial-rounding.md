# ADR-0003: Conservative fixed-point financial rounding

- Status: Accepted
- Date: 2026-07-16

## Context

Binary floating point and favorable rounding can understate collateral or
overstate proceeds, violating capital-floor controls.

## Decision

Represent price and quantity in millionths. Required collateral rounds upward;
proceeds and conservative asset value round downward. All arithmetic is checked.

## Consequences

Call sites must choose an economically meaningful rounding direction. Overflow
and invalid ranges are explicit errors rather than wrapping or clamping.

