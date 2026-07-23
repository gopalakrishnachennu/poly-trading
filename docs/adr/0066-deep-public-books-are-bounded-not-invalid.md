# ADR 0066: Deep public books are bounded, not invalid

## Status

Accepted

## Context

The terminal projection retained only the best ten book levels but rejected a
valid public CLOB book when either side contained more than 5,000 levels. Live
hourly markets can legitimately exceed that count. The resulting availability
failure cleared the complete projection and made an otherwise healthy paper
campaign appear broken.

## Decision

Treat level count as a parser/resource bound, not a trading assumption. The
projection now validates and sorts up to 50,000 distinct levels per side under
the already-bounded HTTP response budget, then retains only the best ten levels
for the terminal projection. Counts above that hard memory/CPU safety bound
remain invalid and fail closed.

## Consequences

Deep but valid books stay observable. A malformed, duplicate, crossed,
one-sided, stale, substituted, or resource-exhausting book still produces
`NO_TRADE`. This change does not change executable quantity, paper fill logic,
risk, or any live capability.
