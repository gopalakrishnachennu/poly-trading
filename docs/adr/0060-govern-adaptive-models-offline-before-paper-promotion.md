# ADR 0060: Govern Adaptive Models Offline Before Paper Promotion

- Status: Accepted
- Date: 2026-07-22

## Decision

Adaptive models are research artifacts, not execution authorities. A model must
bind immutable training-data, feature-schema, configuration and code digests;
be evaluated on chronological unseen evidence; and pass independent adversarial
and deterministic policy gates before it can become a paper champion.

The governance boundary emits only a promotion decision and a `NO_TRADE`
fallback. It cannot reserve capital, approve risk, create an order, sign,
submit, access a wallet, mutate a model, or promote itself. Online learning and
automatic promotion are excluded.

## Consequences

Research can compare specialized agents or models without allowing correlated,
overfit, stale, leaking or self-approved outputs to affect execution. A paper
week is evidence collection, not proof of alpha; promotion requires unseen
walk-forward evidence and remains paper-only.
