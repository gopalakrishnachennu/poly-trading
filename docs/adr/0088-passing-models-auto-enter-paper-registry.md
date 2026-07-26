# ADR 0088: Passing models automatically enter the paper registry

- Status: Accepted
- Date: 2026-07-26
- Supersedes: the manual-review requirement in ADR 0087

## Decision

An operator-configured rolling cycle may automatically register a model for
paper-only evaluation when the accepted trainer reports
`PAPER_CANDIDATE_ONLY`, confirms that the frozen final test was evaluated and
the model checksum matches. Registration grants paper-simulation eligibility
only.

## Consequences

No manual step is needed to add a genuinely passing model to the paper roster.
Registration still cannot start live trading, sign, submit, access a wallet, or
bypass the independent Paper Week risk/accounting path. Rejected models remain
`NO_TRADE`.
