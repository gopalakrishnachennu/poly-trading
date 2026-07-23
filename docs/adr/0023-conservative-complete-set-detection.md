# ADR 0023: Detect complete-set opportunities without claiming atomicity

## Status

Accepted for Phase 2.7.

## Decision

The `complete-set-arbitrage` crate consumes an immutable Phase 2.6 strategy
context and evaluates either the two best asks for a buy-pair opportunity or the
two best bids for a sell-pair opportunity. Executable quantity is capped by the
smaller selected top-level quantity and the configured maximum.

Buy costs round upward per leg. Sell proceeds round downward per leg. Maximum
fees for both legs and maximum conversion cost are explicit inputs. Net-profit
and ROI thresholds are inclusive and use checked integer arithmetic. A valid
opportunity emits exactly two proposal intents whose identities are derived
from the evaluation, context, direction, and constraints.

An opportunity is not locked profit. The two intents remain independently
subject to Phase 2.6 validation, scenario risk including one-leg and partial
fills, exact capital reservations, intent policy, execution, confirmation, and
reconciliation. The detector cannot split, merge, reserve, approve, sign, or
submit.

Commands are bounded and content-idempotent. Decisions are append-and-sync
before mutation, strictly replayable, digest-stable, and checksummed-prefix
checkpointed. Context equivocation, arithmetic failure, command conflict, or
durable corruption is an absorbing halt.

## Consequences

Top-of-book complete-set economics can now be audited independently of fill and
execution assumptions. Depth sweeping, queue modeling, atomic paired execution,
and live adapters remain outside this phase.
