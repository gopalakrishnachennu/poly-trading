# ADR 0025: Stage Paired Capital Transactionally

## Status

Accepted for Phase 2.9.

## Decision

The `paired-capital-staging` runtime is the single writer for one owned Phase
2.8 paired evaluator and one owned Phase 2.0 accounting ledger. A caller cannot
present a detached approval or a claimed balance: the paired risk frame must
equal the runtime's current ledger risk view before evaluation.

Only an authentic, digest-valid, two-candidate `RISK_ELIGIBLE` result can reach
staging. Buy reservations cover conservative full consideration plus maximum
fee; sell reservations cover exact confirmed token quantity. Both ledger
commands execute against a cloned runtime. The clone is installed only after
both active reservations have the exact expected identity, asset, and amount.
Thus a second-leg failure cannot expose a first-leg reservation.

An abort command targets a fully reserved pair and releases both reservations
inside the same transactional clone. There is no one-leg release command.
Funding is prohibited while a pair is active, and boundary, child, arithmetic,
identity, or durability failure halts the owner.

Commands are bounded and canonical. The durable owner appends and device-syncs
before installing state, supports strict replay and prefix checkpoints, and
includes deterministic second-reservation fault injection for atomicity tests.

## Consequences

A stage record proves that both exact legs were capital-backed together at one
ledger digest. It remains inert: it grants no placement eligibility, execution
sequencing, signature, authenticated transport, split/merge, or order authority.
