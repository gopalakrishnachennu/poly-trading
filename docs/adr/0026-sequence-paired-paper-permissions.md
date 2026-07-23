# ADR 0026: Sequence Paired Paper Permissions After Full Capital Staging

## Status

Accepted for Phase 2.10.

## Decision

The `paired-placement-policy` runtime owns Phase 2.9 staging state and issues
only inert paper permissions. A permit binds the complete digest-valid stage,
candidate economics, reservation identity, leg role, normal exchange-mode
sequence, and a validity interval no longer than one second and no later than
the stage freshness boundary or original candidate expiry.

The first selected leg may receive one permission. The complementary hedge leg
cannot receive a permission until monotonic paper lifecycle evidence records the
first leg fully matched. This ordering is explicit about temporary one-leg risk;
it does not claim atomic pair execution.

Expiry changes an unsubmitted authorization to `EXPIRED` but never releases
capital. Submitted, delayed, live, partially matched, unknown, fully matched,
or hedge-active state retains both reservations. Safe abort delegates to the
owned Phase 2.9 paired abort only when both legs prove zero possible fill. There
is no one-leg release path.

Commands are append-and-sync before mutation, content-idempotent, strictly
replayable, digest-stable, and prefix-checkpointed. Subject substitution,
lifecycle or source-history regression, child failure, and durable corruption
halt the complete owner.

## Consequences

Phase 2.10 can prove policy eligibility and sequencing but does not execute an
order. A later composed paper gateway must consume permissions exactly and feed
authoritative lifecycle observations back without relaxing reservation rules.
No signer, credential, network, wallet, or authenticated venue adapter exists.
