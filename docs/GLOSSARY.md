# Glossary

**Confirmed inventory** — inventory whose settlement has reached the configured
finality requirement and is visible in the authoritative balance source.

**Expected inventory** — inventory implied by local intent or pending exchange
activity; never automatically spendable.

**Locked profit** — profit whose required legs and settlement state satisfy the
configured conservative recognition policy.

**Market actor** — the single writer that owns all mutable state for one market.

**Capital floor** — the minimum permitted conservative terminal wealth.

**Reservation** — exclusive allocation of confirmed spendable collateral or
inventory to a proposed or open action.

**Event time** — timestamp associated with the event by its source.

**Receive time** — timestamp at which this system accepted the event.

**Reconciliation halt** — state that forbids new exposure because authoritative
sources disagree without a known bounded explanation.

**Journal** — append-only durable sequence of canonical event envelopes used for
recovery, replay, and audit.

**Synchronization epoch** — one public-feed connection lifetime. Books from a
prior epoch are invalid until new authoritative snapshots arrive.

**Replay equivalence** — identical durable input produces identical typed state
and an identical explicitly encoded state digest.

**Backpressure halt** — capture stops because bounded live-state ingress is full
or closed. The journaled event is retained; it is never silently dropped.

**Ready** — a synchronized, fresh, fully authoritative state permitted to feed
future read-only calculations. It does not itself authorize trading.

**Journal segment** — one bounded append-only journal file in a contiguous
sequence. Segment boundaries do not reset recorder event sequence.

**Replay checkpoint** — a checksummed optimization containing complete replay
state at one durable sequence. It is usable only after matching the journal
prefix digest.

**Resolution contract** — immutable binding of a market's identifiers, rules
fingerprint, series, source, comparator, Binance symbol, and exact candle window.

**Indicative assessment** — non-final answer to “what would win if the current
open candle closed now?” It is never settlement evidence.

**Resolution evidence** — checksummed deterministic outcome calculation from an
exact finalized reference candle and immutable resolution contract. It does not
mean the exchange or blockchain has confirmed resolution.

**Cross-feed readiness** — deterministic eligibility result requiring every
configured independent feed to be ready, fresh, temporally coherent, and
history-consistent. It is not permission to trade.

**Digest equivocation** — a feed snapshot's authoritative or timing state changes
without its durable sequence advancing. This is a permanent integrity halt.
