#![forbid(unsafe_code)]

//! Deterministic fail-closed supervision across independent Polymarket and
//! settlement-reference feeds.
//!
//! This crate owns no network connection and reads no system clock. Callers
//! provide immutable feed snapshots and explicit evaluation time. `READY`
//! means only that observed data satisfies health and time-integrity budgets;
//! it does not authorize a strategy or order.

use live_market_state::{ActorMode, ActorSnapshot};
use reference_market_data::{ReferenceHealth, ReferenceSnapshot, ReferenceSymbol};
use std::time::Duration;
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct SupervisorConfig {
    pub market_stale_after: Duration,
    pub reference_stale_after: Duration,
    pub max_cross_feed_receive_skew: Duration,
    pub max_source_event_lag: Duration,
    pub max_source_future_skew: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            market_stale_after: Duration::from_secs(5),
            reference_stale_after: Duration::from_secs(5),
            max_cross_feed_receive_skew: Duration::from_secs(2),
            max_source_event_lag: Duration::from_secs(10),
            max_source_future_skew: Duration::from_millis(500),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SupervisorMode {
    Starting = 1,
    Ready = 2,
    MarketUnavailable = 3,
    ReferenceUnavailable = 4,
    MarketStale = 5,
    ReferenceStale = 6,
    CrossFeedSkew = 7,
    SourceEventLag = 8,
    SourceEventFuture = 9,
    Halted = 10,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorSnapshot {
    pub mode: SupervisorMode,
    pub ready: bool,
    pub evaluated_at_ns: Option<i64>,
    pub market_epoch: u64,
    pub market_sequence: Option<u64>,
    pub market_digest: [u8; 32],
    pub market_state_digest: [u8; 32],
    pub reference_epoch: u64,
    pub reference_sequence: Option<u64>,
    pub reference_digest: [u8; 32],
    pub reference_state_digest: [u8; 32],
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorObservation {
    pub now_ns: i64,
    pub market: ActorSnapshot,
    pub reference: ReferenceSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FeedMarker {
    epoch: u64,
    sequence: Option<u64>,
    digest: [u8; 32],
    state_digest: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct CrossFeedSupervisor {
    market_stale_ns: i64,
    reference_stale_ns: i64,
    max_cross_feed_skew_ns: i64,
    max_source_lag_ns: i64,
    max_source_future_ns: i64,
    mode: SupervisorMode,
    last_now_ns: Option<i64>,
    market_marker: Option<FeedMarker>,
    reference_marker: Option<FeedMarker>,
    halt_reason: Option<String>,
}

impl CrossFeedSupervisor {
    /// Creates a deterministic supervisor with positive representable budgets.
    ///
    /// # Errors
    ///
    /// Returns [`SupervisorError::InvalidConfig`] for zero or overflowing
    /// duration values.
    pub fn new(config: SupervisorConfig) -> Result<Self, SupervisorError> {
        Ok(Self {
            market_stale_ns: duration_ns(config.market_stale_after)?,
            reference_stale_ns: duration_ns(config.reference_stale_after)?,
            max_cross_feed_skew_ns: duration_ns(config.max_cross_feed_receive_skew)?,
            max_source_lag_ns: duration_ns(config.max_source_event_lag)?,
            max_source_future_ns: duration_ns(config.max_source_future_skew)?,
            mode: SupervisorMode::Starting,
            last_now_ns: None,
            market_marker: None,
            reference_marker: None,
            halt_reason: None,
        })
    }

    /// Evaluates one pair of immutable feed snapshots at caller-supplied time.
    ///
    /// Recoverable health, staleness, and source-time failures produce a
    /// non-ready snapshot. Local clock regression, receive timestamps from the
    /// future, or feed snapshot history regression permanently halt the core.
    ///
    /// # Errors
    ///
    /// Returns [`SupervisorError`] on permanent integrity failures or if the
    /// supervisor was already halted.
    pub fn evaluate(
        &mut self,
        observation: &SupervisorObservation,
    ) -> Result<SupervisorSnapshot, SupervisorError> {
        if let Some(reason) = &self.halt_reason {
            return Err(SupervisorError::Halted(reason.clone()));
        }
        if observation.now_ns < 0 {
            return self.halt(SupervisorError::InvalidNow(observation.now_ns));
        }
        if self
            .last_now_ns
            .is_some_and(|previous| observation.now_ns < previous)
        {
            return self.halt(SupervisorError::ClockRegression {
                previous: self.last_now_ns.unwrap_or(observation.now_ns),
                actual: observation.now_ns,
            });
        }

        let next_market = FeedMarker {
            epoch: observation.market.epoch,
            sequence: observation.market.last_sequence,
            digest: market_marker_digest(&observation.market),
            state_digest: observation.market.digest,
        };
        let next_reference = FeedMarker {
            epoch: observation.reference.epoch,
            sequence: observation.reference.last_sequence,
            digest: reference_marker_digest(&observation.reference),
            state_digest: observation.reference.digest,
        };
        if let Some(previous) = self.market_marker {
            if let Err(error) = validate_marker("market", previous, next_market) {
                return self.halt(error);
            }
        }
        if let Some(previous) = self.reference_marker {
            if let Err(error) = validate_marker("reference", previous, next_reference) {
                return self.halt(error);
            }
        }

        if let Err(error) = validate_no_future_receive(observation) {
            return self.halt(error);
        }
        if observation.reference.health == ReferenceHealth::Ready {
            if let Err(error) = validate_reference_aggregate_timing(&observation.reference) {
                return self.halt(error);
            }
        }
        let mode = match self.evaluate_mode(observation) {
            Ok(mode) => mode,
            Err(error) => return self.halt(error),
        };
        self.mode = mode;
        self.last_now_ns = Some(observation.now_ns);
        self.market_marker = Some(next_market);
        self.reference_marker = Some(next_reference);
        Ok(self.snapshot())
    }

    fn evaluate_mode(
        &self,
        observation: &SupervisorObservation,
    ) -> Result<SupervisorMode, SupervisorError> {
        if observation.market.mode != ActorMode::Ready || !observation.market.ready {
            return Ok(SupervisorMode::MarketUnavailable);
        }
        if observation.reference.health != ReferenceHealth::Ready {
            return Ok(SupervisorMode::ReferenceUnavailable);
        }
        let market_received = observation
            .market
            .last_market_received_ns
            .ok_or(SupervisorError::MissingTimestamp("market receive"))?;
        let market_event = observation
            .market
            .last_market_event_ns
            .ok_or(SupervisorError::MissingTimestamp("market event"))?;
        let reference_received = observation
            .reference
            .last_reference_received_ns
            .ok_or(SupervisorError::MissingTimestamp("reference receive"))?;

        if age(observation.now_ns, market_received)? > self.market_stale_ns {
            return Ok(SupervisorMode::MarketStale);
        }

        let mut source_events = vec![market_event];
        for symbol in ReferenceSymbol::ALL {
            let timing = observation
                .reference
                .symbols
                .get(&symbol)
                .copied()
                .ok_or(SupervisorError::MissingReferenceSymbol(symbol))?;
            let oldest_receive =
                timing
                    .oldest_required_received_ns()
                    .ok_or(SupervisorError::MissingTimestamp(
                        "reference component receive",
                    ))?;
            if age(observation.now_ns, oldest_receive)? > self.reference_stale_ns {
                return Ok(SupervisorMode::ReferenceStale);
            }
            source_events.push(
                timing
                    .candle_event_ns
                    .ok_or(SupervisorError::MissingTimestamp("reference candle event"))?,
            );
            source_events.push(timing.aggregate_trade_event_ns.ok_or(
                SupervisorError::MissingTimestamp("reference aggregate-trade event"),
            )?);
        }
        let receive_skew = market_received.abs_diff(reference_received);
        if receive_skew > u64::try_from(self.max_cross_feed_skew_ns).unwrap_or(u64::MAX) {
            return Ok(SupervisorMode::CrossFeedSkew);
        }
        for event_ns in source_events {
            if event_ns > observation.now_ns {
                if event_ns.abs_diff(observation.now_ns)
                    > u64::try_from(self.max_source_future_ns).unwrap_or(u64::MAX)
                {
                    return Ok(SupervisorMode::SourceEventFuture);
                }
            } else if age(observation.now_ns, event_ns)? > self.max_source_lag_ns {
                return Ok(SupervisorMode::SourceEventLag);
            }
        }
        Ok(SupervisorMode::Ready)
    }

    fn halt(&mut self, error: SupervisorError) -> Result<SupervisorSnapshot, SupervisorError> {
        self.mode = SupervisorMode::Halted;
        self.halt_reason = Some(error.to_string());
        Err(error)
    }

    #[must_use]
    pub fn snapshot(&self) -> SupervisorSnapshot {
        let market = self.market_marker.unwrap_or(FeedMarker {
            epoch: 0,
            sequence: None,
            digest: [0; 32],
            state_digest: [0; 32],
        });
        let reference = self.reference_marker.unwrap_or(FeedMarker {
            epoch: 0,
            sequence: None,
            digest: [0; 32],
            state_digest: [0; 32],
        });
        let digest = state_digest(
            self.mode,
            self.last_now_ns,
            market,
            reference,
            self.halt_reason.as_deref(),
        );
        SupervisorSnapshot {
            mode: self.mode,
            ready: self.mode == SupervisorMode::Ready,
            evaluated_at_ns: self.last_now_ns,
            market_epoch: market.epoch,
            market_sequence: market.sequence,
            market_digest: market.digest,
            market_state_digest: market.state_digest,
            reference_epoch: reference.epoch,
            reference_sequence: reference.sequence,
            reference_digest: reference.digest,
            reference_state_digest: reference.state_digest,
            halt_reason: self.halt_reason.clone(),
            digest,
        }
    }
}

/// Replays observations through the same deterministic supervisor core.
///
/// # Errors
///
/// Returns [`SupervisorError`] at the first permanent integrity failure.
pub fn replay(
    config: SupervisorConfig,
    observations: &[SupervisorObservation],
) -> Result<SupervisorSnapshot, SupervisorError> {
    let mut supervisor = CrossFeedSupervisor::new(config)?;
    for observation in observations {
        supervisor.evaluate(observation)?;
    }
    Ok(supervisor.snapshot())
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SupervisorError {
    #[error("supervisor duration configuration is zero or too large")]
    InvalidConfig,
    #[error("invalid evaluation time: {0}")]
    InvalidNow(i64),
    #[error("local evaluation clock regressed from {previous} to {actual}")]
    ClockRegression { previous: i64, actual: i64 },
    #[error("{feed} feed snapshot epoch or sequence regressed")]
    FeedRegression { feed: &'static str },
    #[error("{feed} feed digest changed without sequence advancement")]
    DigestChangedWithoutSequence { feed: &'static str },
    #[error("required timestamp is missing: {0}")]
    MissingTimestamp(&'static str),
    #[error("reference snapshot is missing {0:?}")]
    MissingReferenceSymbol(ReferenceSymbol),
    #[error("a feed receive timestamp is later than evaluation time")]
    ReceiveTimeFuture,
    #[error("feed snapshot aggregate timing is internally inconsistent")]
    SnapshotTimingMismatch,
    #[error("timestamp subtraction overflow")]
    TimestampOverflow,
    #[error("supervisor is permanently halted: {0}")]
    Halted(String),
}

fn duration_ns(value: Duration) -> Result<i64, SupervisorError> {
    let nanos = i64::try_from(value.as_nanos()).map_err(|_| SupervisorError::InvalidConfig)?;
    if nanos == 0 {
        Err(SupervisorError::InvalidConfig)
    } else {
        Ok(nanos)
    }
}

fn age(now_ns: i64, timestamp_ns: i64) -> Result<i64, SupervisorError> {
    now_ns
        .checked_sub(timestamp_ns)
        .ok_or(SupervisorError::TimestampOverflow)
}

fn validate_marker(
    feed: &'static str,
    previous: FeedMarker,
    next: FeedMarker,
) -> Result<(), SupervisorError> {
    if next.epoch < previous.epoch
        || matches!((previous.sequence, next.sequence), (Some(_), None))
        || matches!((previous.sequence, next.sequence), (Some(left), Some(right)) if right < left)
    {
        return Err(SupervisorError::FeedRegression { feed });
    }
    if previous.sequence == next.sequence && previous.digest != next.digest {
        return Err(SupervisorError::DigestChangedWithoutSequence { feed });
    }
    Ok(())
}

fn validate_no_future_receive(observation: &SupervisorObservation) -> Result<(), SupervisorError> {
    let mut received_times = Vec::new();
    if let Some(value) = observation.market.last_market_received_ns {
        received_times.push(value);
    }
    if let Some(value) = observation.reference.last_reference_received_ns {
        received_times.push(value);
    }
    for timing in observation.reference.symbols.values() {
        received_times.extend(
            [
                timing.candle_received_ns,
                timing.aggregate_trade_received_ns,
                timing.book_ticker_received_ns,
            ]
            .into_iter()
            .flatten(),
        );
    }
    if received_times
        .into_iter()
        .any(|received| received < 0 || received > observation.now_ns)
    {
        Err(SupervisorError::ReceiveTimeFuture)
    } else {
        Ok(())
    }
}

fn validate_reference_aggregate_timing(
    snapshot: &ReferenceSnapshot,
) -> Result<(), SupervisorError> {
    let expected = snapshot
        .last_reference_received_ns
        .ok_or(SupervisorError::MissingTimestamp("reference receive"))?;
    let mut latest = i64::MIN;
    for symbol in ReferenceSymbol::ALL {
        let timing = snapshot
            .symbols
            .get(&symbol)
            .ok_or(SupervisorError::MissingReferenceSymbol(symbol))?;
        for received in [
            timing.candle_received_ns,
            timing.aggregate_trade_received_ns,
            timing.book_ticker_received_ns,
        ] {
            latest = latest.max(received.ok_or(SupervisorError::MissingTimestamp(
                "reference component receive",
            ))?);
        }
    }
    if latest == expected {
        Ok(())
    } else {
        Err(SupervisorError::SnapshotTimingMismatch)
    }
}

fn market_marker_digest(snapshot: &ActorSnapshot) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"MARKET_SUPERVISION_MARKER_V1");
    hasher.update(&snapshot.epoch.to_le_bytes());
    hasher.update(&snapshot.last_sequence.unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(&snapshot.digest);
    hasher.update(
        &snapshot
            .last_market_event_ns
            .unwrap_or(i64::MIN)
            .to_le_bytes(),
    );
    hasher.update(
        &snapshot
            .last_market_received_ns
            .unwrap_or(i64::MIN)
            .to_le_bytes(),
    );
    *hasher.finalize().as_bytes()
}

fn reference_marker_digest(snapshot: &ReferenceSnapshot) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"REFERENCE_SUPERVISION_MARKER_V1");
    hasher.update(&snapshot.epoch.to_le_bytes());
    hasher.update(&snapshot.last_sequence.unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(&snapshot.digest);
    hasher.update(
        &snapshot
            .last_reference_received_ns
            .unwrap_or(i64::MIN)
            .to_le_bytes(),
    );
    for symbol in ReferenceSymbol::ALL {
        hasher.update(&[symbol as u8]);
        if let Some(timing) = snapshot.symbols.get(&symbol) {
            for timestamp in [
                timing.candle_event_ns,
                timing.candle_received_ns,
                timing.aggregate_trade_event_ns,
                timing.aggregate_trade_received_ns,
                timing.book_ticker_received_ns,
            ] {
                hasher.update(&timestamp.unwrap_or(i64::MIN).to_le_bytes());
            }
        } else {
            for _ in 0..5 {
                hasher.update(&i64::MIN.to_le_bytes());
            }
        }
    }
    *hasher.finalize().as_bytes()
}

fn state_digest(
    mode: SupervisorMode,
    now_ns: Option<i64>,
    market: FeedMarker,
    reference: FeedMarker,
    halt_reason: Option<&str>,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"CROSS_FEED_SUPERVISOR_V1");
    hasher.update(&[mode as u8]);
    hasher.update(&now_ns.unwrap_or(i64::MIN).to_le_bytes());
    for marker in [market, reference] {
        hasher.update(&marker.epoch.to_le_bytes());
        hasher.update(&marker.sequence.unwrap_or(u64::MAX).to_le_bytes());
        hasher.update(&marker.digest);
        hasher.update(&marker.state_digest);
    }
    if let Some(reason) = halt_reason {
        hasher.update(
            &u64::try_from(reason.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        hasher.update(reason.as_bytes());
    } else {
        hasher.update(&0_u64.to_le_bytes());
    }
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reference_market_data::{ReferenceSymbol, ReferenceSymbolSnapshot};
    use std::collections::BTreeMap;

    const SECOND: i64 = 1_000_000_000;

    fn config() -> SupervisorConfig {
        SupervisorConfig {
            market_stale_after: Duration::from_secs(5),
            reference_stale_after: Duration::from_secs(5),
            max_cross_feed_receive_skew: Duration::from_secs(2),
            max_source_event_lag: Duration::from_secs(10),
            max_source_future_skew: Duration::from_millis(500),
        }
    }

    fn observation(
        now_ns: i64,
        market_received_ns: i64,
        reference_received_ns: i64,
        sequence: u64,
    ) -> SupervisorObservation {
        let digest_byte = u8::try_from(sequence).unwrap_or(u8::MAX);
        let market = ActorSnapshot {
            mode: ActorMode::Ready,
            ready: true,
            epoch: 1,
            last_sequence: Some(sequence),
            book_count: 4,
            digest: [digest_byte; 32],
            last_market_event_ns: Some(market_received_ns),
            last_market_received_ns: Some(market_received_ns),
            halt_reason: None,
        };
        let timing = ReferenceSymbolSnapshot {
            candle_event_ns: Some(reference_received_ns),
            candle_received_ns: Some(reference_received_ns),
            aggregate_trade_event_ns: Some(reference_received_ns),
            aggregate_trade_received_ns: Some(reference_received_ns),
            book_ticker_received_ns: Some(reference_received_ns),
        };
        let symbols = ReferenceSymbol::ALL
            .into_iter()
            .map(|symbol| (symbol, timing))
            .collect::<BTreeMap<_, _>>();
        let reference = ReferenceSnapshot {
            health: ReferenceHealth::Ready,
            epoch: 1,
            last_sequence: Some(sequence),
            digest: [digest_byte.saturating_add(100); 32],
            last_reference_received_ns: Some(reference_received_ns),
            symbols,
        };
        SupervisorObservation {
            now_ns,
            market,
            reference,
        }
    }

    #[test]
    fn ready_includes_exact_boundaries_and_stale_recovers() {
        let mut supervisor = CrossFeedSupervisor::new(config()).expect("supervisor");
        let first = observation(10 * SECOND, 5 * SECOND, 5 * SECOND, 1);
        assert_eq!(
            supervisor.evaluate(&first).expect("boundary").mode,
            SupervisorMode::Ready
        );

        let stale = observation(11 * SECOND, 5 * SECOND, 5 * SECOND, 1);
        assert_eq!(
            supervisor.evaluate(&stale).expect("stale").mode,
            SupervisorMode::MarketStale
        );
        let recovered = observation(11 * SECOND, 11 * SECOND, 11 * SECOND, 2);
        assert_eq!(
            supervisor.evaluate(&recovered).expect("recovered").mode,
            SupervisorMode::Ready
        );

        let allowed_future = 11 * SECOND + 500_000_000;
        let mut boundary = observation(11 * SECOND, 11 * SECOND, 11 * SECOND, 3);
        boundary.market.last_market_event_ns = Some(allowed_future);
        assert_eq!(
            supervisor
                .evaluate(&boundary)
                .expect("future boundary")
                .mode,
            SupervisorMode::Ready
        );
    }

    #[test]
    fn unavailable_stale_skew_lag_and_future_are_distinct() {
        let mut unavailable = observation(10 * SECOND, 10 * SECOND, 10 * SECOND, 1);
        unavailable.market.mode = ActorMode::CollectingSnapshots;
        unavailable.market.ready = false;
        let mut supervisor = CrossFeedSupervisor::new(config()).expect("supervisor");
        assert_eq!(
            supervisor.evaluate(&unavailable).expect("unavailable").mode,
            SupervisorMode::MarketUnavailable
        );

        let mut reference_unavailable = observation(10 * SECOND, 10 * SECOND, 10 * SECOND, 2);
        reference_unavailable.reference.health = ReferenceHealth::Collecting;
        assert_eq!(
            supervisor
                .evaluate(&reference_unavailable)
                .expect("reference unavailable")
                .mode,
            SupervisorMode::ReferenceUnavailable
        );

        let reference_stale = observation(20 * SECOND, 20 * SECOND, 14 * SECOND, 3);
        assert_eq!(
            supervisor
                .evaluate(&reference_stale)
                .expect("reference stale")
                .mode,
            SupervisorMode::ReferenceStale
        );

        let skewed = observation(21 * SECOND, 21 * SECOND, 18 * SECOND, 4);
        assert_eq!(
            supervisor.evaluate(&skewed).expect("skewed").mode,
            SupervisorMode::CrossFeedSkew
        );

        let mut lagged = observation(30 * SECOND, 30 * SECOND, 30 * SECOND, 5);
        lagged
            .reference
            .symbols
            .get_mut(&ReferenceSymbol::BtcUsdt)
            .expect("BTC timing")
            .candle_event_ns = Some(19 * SECOND);
        assert_eq!(
            supervisor.evaluate(&lagged).expect("lagged").mode,
            SupervisorMode::SourceEventLag
        );

        let mut future = observation(40 * SECOND, 40 * SECOND, 40 * SECOND, 6);
        future.market.last_market_event_ns = Some(40 * SECOND + SECOND);
        assert_eq!(
            supervisor.evaluate(&future).expect("future").mode,
            SupervisorMode::SourceEventFuture
        );
    }

    #[test]
    fn receive_future_clock_regression_and_feed_regression_halt() {
        let mut receive_future = CrossFeedSupervisor::new(config()).expect("supervisor");
        assert_eq!(
            receive_future.evaluate(&observation(10 * SECOND, 11 * SECOND, 10 * SECOND, 1)),
            Err(SupervisorError::ReceiveTimeFuture)
        );
        assert_eq!(receive_future.snapshot().mode, SupervisorMode::Halted);

        let mut unavailable_future = CrossFeedSupervisor::new(config()).expect("supervisor");
        let mut unavailable = observation(10 * SECOND, 11 * SECOND, 10 * SECOND, 1);
        unavailable.market.mode = ActorMode::CollectingSnapshots;
        unavailable.market.ready = false;
        assert_eq!(
            unavailable_future.evaluate(&unavailable),
            Err(SupervisorError::ReceiveTimeFuture)
        );

        let mut inconsistent = CrossFeedSupervisor::new(config()).expect("supervisor");
        let mut bad_aggregate = observation(10 * SECOND, 10 * SECOND, 10 * SECOND, 1);
        bad_aggregate.reference.last_reference_received_ns = Some(9 * SECOND);
        assert_eq!(
            inconsistent.evaluate(&bad_aggregate),
            Err(SupervisorError::SnapshotTimingMismatch)
        );

        let mut clock = CrossFeedSupervisor::new(config()).expect("supervisor");
        clock
            .evaluate(&observation(10 * SECOND, 10 * SECOND, 10 * SECOND, 1))
            .expect("first");
        assert!(matches!(
            clock.evaluate(&observation(9 * SECOND, 9 * SECOND, 9 * SECOND, 2)),
            Err(SupervisorError::ClockRegression { .. })
        ));
        assert!(matches!(
            clock.evaluate(&observation(11 * SECOND, 11 * SECOND, 11 * SECOND, 3)),
            Err(SupervisorError::Halted(_))
        ));

        let mut sequence = CrossFeedSupervisor::new(config()).expect("supervisor");
        sequence
            .evaluate(&observation(10 * SECOND, 10 * SECOND, 10 * SECOND, 2))
            .expect("first");
        assert!(matches!(
            sequence.evaluate(&observation(11 * SECOND, 11 * SECOND, 11 * SECOND, 1)),
            Err(SupervisorError::FeedRegression { .. })
        ));
    }

    #[test]
    fn digest_equivocation_halts_without_advancing_marker() {
        let mut supervisor = CrossFeedSupervisor::new(config()).expect("supervisor");
        let first = observation(10 * SECOND, 10 * SECOND, 10 * SECOND, 1);
        supervisor.evaluate(&first).expect("first");
        let before = supervisor.snapshot();
        let mut equivocation = first;
        equivocation.now_ns = 11 * SECOND;
        equivocation.market.digest = [99; 32];
        assert_eq!(
            supervisor.evaluate(&equivocation),
            Err(SupervisorError::DigestChangedWithoutSequence { feed: "market" })
        );
        let halted = supervisor.snapshot();
        assert_eq!(halted.market_digest, before.market_digest);
        assert_eq!(halted.market_sequence, before.market_sequence);
        assert_eq!(halted.mode, SupervisorMode::Halted);
    }

    #[test]
    fn online_and_replay_digests_match() {
        let observations = vec![
            observation(10 * SECOND, 10 * SECOND, 10 * SECOND, 1),
            observation(11 * SECOND, 11 * SECOND, 11 * SECOND, 2),
            observation(12 * SECOND, 12 * SECOND, 12 * SECOND, 3),
        ];
        let mut online = CrossFeedSupervisor::new(config()).expect("supervisor");
        for item in &observations {
            online.evaluate(item).expect("online");
        }
        let replayed = replay(config(), &observations).expect("replay");
        assert_eq!(online.snapshot(), replayed);
        assert_eq!(online.snapshot().digest, replayed.digest);
    }

    #[test]
    fn zero_or_unrepresentable_budgets_are_rejected() {
        let mut invalid = config();
        invalid.market_stale_after = Duration::ZERO;
        assert_eq!(
            CrossFeedSupervisor::new(invalid).err(),
            Some(SupervisorError::InvalidConfig)
        );
        invalid = config();
        invalid.max_source_event_lag = Duration::MAX;
        assert_eq!(
            CrossFeedSupervisor::new(invalid).err(),
            Some(SupervisorError::InvalidConfig)
        );
    }
}
