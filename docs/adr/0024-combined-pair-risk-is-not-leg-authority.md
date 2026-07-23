# ADR 0024: Combined pair risk is not single-leg authority

## Status

Accepted for Phase 2.8.

## Decision

The `paired-opportunity-runtime` is the single writer across one owned Phase 2.7
detector, Phase 2.6 proposal engine, and Phase 2.2 portfolio-risk engine. Callers
provide an arbitrage command and a candidate-free risk frame. The runtime derives
the detector decision, both proposal decisions, and both candidates internally.

Portfolio risk now accepts a bounded candidate set of at most two orders. Both
candidates share available cash or token capacity and enter the same Cartesian
product with existing open orders, terminal outcomes, and correlated shocks.
Resting orders remain subject to exact confirmed reservation backing.

A two-candidate decision uses a candidate-set digest domain distinct from the
single-order fingerprint required by placement policy. Therefore a favorable
paired risk decision cannot authorize either constituent leg. The existing
paper runtime also rejects multi-candidate risk requests explicitly.

The paired runtime is append-and-sync before mutation, strictly replayable,
digest-stable, and prefix-checkpointed. Child failures, candidate substitution,
command conflict, arithmetic failure, and durable corruption halt the complete
owner.

## Consequences

The system can prove that both arbitrage legs fit combined portfolio scenarios
without pretending they fill atomically. Capital reservation and safe leg
sequencing require a later phase and a new authority derived from the exact
paired decision; this phase cannot reserve, permit, sign, or submit.
