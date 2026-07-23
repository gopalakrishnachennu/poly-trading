#![forbid(unsafe_code)]

//! Deterministic read-only integration, fault injection, and hourly soak core.

use common_types::{PriceMicros, QuantityMicros, QuotePriceMicros, ReferenceQuantityE8};
use feed_supervisor::{
    CrossFeedSupervisor, SupervisorConfig, SupervisorError, SupervisorObservation,
};
use live_market_state::{ActorMode, ActorSnapshot};
use market_recorder::EventJournal;
use market_session::{
    capture_sources, CoordinationFrame, CoordinatorSnapshot, SessionKey, SessionPhase,
    SessionSourceState, TokenBookView,
};
use order_book_replay::ReplayState;
use public_market_data::{Asset, MarketIdentity, BTC_HOURLY, ETH_HOURLY};
use reference_market_data::{
    CandleData, CandleInterval, FinalizedCandle, InProgressCandle, ReferenceHealth,
    ReferenceReplayState, ReferenceSnapshot, ReferenceSymbol, ReferenceSymbolSnapshot,
};
use session_runtime::{DurableCommand, DurableCoordinator, RuntimeError};
use std::collections::BTreeMap;
use thiserror::Error;

const HOUR_MS: i64 = 3_600_000;
const MAX_HOURS: u16 = 168;
const MAX_TICKS_PER_HOUR: u16 = 3_600;
const MAX_FAULTS: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrationTick {
    pub now_ns: i64,
    pub market: ActorSnapshot,
    pub reference: ReferenceSnapshot,
    pub sessions: BTreeMap<SessionKey, SessionSourceState>,
}

impl IntegrationTick {
    /// Captures exact source state from the production replay cores.
    #[must_use]
    pub fn capture<'a>(
        now_ns: i64,
        identities: impl IntoIterator<Item = &'a MarketIdentity>,
        market: ActorSnapshot,
        books: &ReplayState,
        reference: &ReferenceReplayState,
    ) -> Self {
        Self {
            now_ns,
            market,
            reference: reference.snapshot(),
            sessions: capture_sources(identities, books, reference),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Fault {
    MarketUnavailable,
    ReferenceUnavailable,
    CrossFeedReceiveSkew { nanos: i64 },
    SourceEventFuture { nanos: i64 },
    FutureMarketReceive { nanos: i64 },
    RepeatMarketSequenceWithDigestChange,
    RemoveUpBook(SessionKey),
    RemoveOracleCandle(SessionKey),
    SupervisorProvenanceMismatch,
    RemoveSession(SessionKey),
}

impl Fault {
    const fn is_pre_supervision(&self) -> bool {
        matches!(
            self,
            Self::MarketUnavailable
                | Self::ReferenceUnavailable
                | Self::CrossFeedReceiveSkew { .. }
                | Self::SourceEventFuture { .. }
                | Self::FutureMarketReceive { .. }
                | Self::RepeatMarketSequenceWithDigestChange
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FaultScript {
    scheduled: BTreeMap<u64, Vec<Fault>>,
}

impl FaultScript {
    /// Creates a bounded ordered script.
    ///
    /// # Errors
    ///
    /// Rejects too many faults, non-positive offsets, or invalid skew bounds.
    pub fn new(entries: impl IntoIterator<Item = (u64, Fault)>) -> Result<Self, IntegrationError> {
        let mut scheduled: BTreeMap<u64, Vec<Fault>> = BTreeMap::new();
        let mut count = 0_usize;
        for (tick, fault) in entries {
            validate_fault(&fault)?;
            count = count.checked_add(1).ok_or(IntegrationError::InvalidFault)?;
            if count > MAX_FAULTS {
                return Err(IntegrationError::FaultBound);
            }
            scheduled.entry(tick).or_default().push(fault);
        }
        Ok(Self { scheduled })
    }

    fn at(&self, tick: u64) -> &[Fault] {
        self.scheduled.get(&tick).map_or(&[], Vec::as_slice)
    }
}

fn validate_fault(fault: &Fault) -> Result<(), IntegrationError> {
    match fault {
        Fault::CrossFeedReceiveSkew { nanos }
        | Fault::SourceEventFuture { nanos }
        | Fault::FutureMarketReceive { nanos }
            if *nanos <= 0 =>
        {
            Err(IntegrationError::InvalidFault)
        }
        _ => Ok(()),
    }
}

#[derive(Debug)]
pub struct IntegrationEngine<J> {
    durable: DurableCoordinator<J>,
    supervisor: CrossFeedSupervisor,
    faults: FaultScript,
    tick_index: u64,
}

impl<J: EventJournal> IntegrationEngine<J> {
    /// Creates the one-owner integration core.
    ///
    /// # Errors
    ///
    /// Rejects invalid supervisor configuration.
    pub fn new(
        durable: DurableCoordinator<J>,
        supervisor: SupervisorConfig,
        faults: FaultScript,
    ) -> Result<Self, IntegrationError> {
        Ok(Self {
            durable,
            supervisor: CrossFeedSupervisor::new(supervisor)?,
            faults,
            tick_index: 0,
        })
    }

    /// Durably registers one discovered identity.
    ///
    /// # Errors
    ///
    /// Propagates durable journal or coordinator failure.
    pub fn register(
        &mut self,
        identity: MarketIdentity,
        recorded_at_ns: i64,
    ) -> Result<CoordinatorSnapshot, IntegrationError> {
        Ok(self.durable.apply(DurableCommand::Register {
            identity,
            recorded_at_ns,
        })?)
    }

    /// Supervises, constructs, faults, and durably applies one exact tick.
    ///
    /// # Errors
    ///
    /// Integrity feed failures or durable coordination failures terminate the
    /// caller's run. Recoverable supervisor modes produce degraded snapshots.
    pub fn step(
        &mut self,
        mut tick: IntegrationTick,
    ) -> Result<CoordinatorSnapshot, IntegrationError> {
        let faults = self.faults.at(self.tick_index);
        for fault in faults.iter().filter(|fault| fault.is_pre_supervision()) {
            apply_pre_fault(&mut tick, fault)?;
        }
        let supervision = self.supervisor.evaluate(&SupervisorObservation {
            now_ns: tick.now_ns,
            market: tick.market.clone(),
            reference: tick.reference.clone(),
        })?;
        let mut frame = CoordinationFrame {
            now_ns: tick.now_ns,
            market: tick.market,
            reference: tick.reference,
            supervision,
            sessions: tick.sessions,
        };
        for fault in faults.iter().filter(|fault| !fault.is_pre_supervision()) {
            apply_post_fault(&mut frame, fault);
        }
        let snapshot = self.durable.apply(DurableCommand::Coordinate(frame))?;
        self.tick_index = self
            .tick_index
            .checked_add(1)
            .ok_or(IntegrationError::TickOverflow)?;
        Ok(snapshot)
    }

    #[must_use]
    pub const fn tick_index(&self) -> u64 {
        self.tick_index
    }

    #[must_use]
    pub fn snapshot(&self) -> CoordinatorSnapshot {
        self.durable.snapshot()
    }

    #[must_use]
    pub const fn last_sequence(&self) -> Option<u64> {
        self.durable.last_sequence()
    }

    #[must_use]
    pub fn into_durable(self) -> DurableCoordinator<J> {
        self.durable
    }
}

fn apply_pre_fault(tick: &mut IntegrationTick, fault: &Fault) -> Result<(), IntegrationError> {
    match fault {
        Fault::MarketUnavailable => {
            tick.market.mode = ActorMode::Stale;
            tick.market.ready = false;
        }
        Fault::ReferenceUnavailable => tick.reference.health = ReferenceHealth::Disconnected,
        Fault::CrossFeedReceiveSkew { nanos } => {
            shift_reference_receive(&mut tick.reference, -*nanos)?;
        }
        Fault::SourceEventFuture { nanos } => {
            tick.market.last_market_event_ns = Some(
                tick.now_ns
                    .checked_add(*nanos)
                    .ok_or(IntegrationError::TimestampOverflow)?,
            );
        }
        Fault::FutureMarketReceive { nanos } => {
            tick.market.last_market_received_ns = Some(
                tick.now_ns
                    .checked_add(*nanos)
                    .ok_or(IntegrationError::TimestampOverflow)?,
            );
        }
        Fault::RepeatMarketSequenceWithDigestChange => {
            tick.market.last_sequence = tick
                .market
                .last_sequence
                .and_then(|value| value.checked_sub(1));
            tick.market.digest[0] ^= 1;
        }
        _ => return Err(IntegrationError::FaultLayer),
    }
    Ok(())
}

fn shift_reference_receive(
    reference: &mut ReferenceSnapshot,
    delta: i64,
) -> Result<(), IntegrationError> {
    reference.last_reference_received_ns = reference
        .last_reference_received_ns
        .map(|value| {
            value
                .checked_add(delta)
                .ok_or(IntegrationError::TimestampOverflow)
        })
        .transpose()?;
    for timing in reference.symbols.values_mut() {
        timing.candle_received_ns = shift_optional(timing.candle_received_ns, delta)?;
        timing.aggregate_trade_received_ns =
            shift_optional(timing.aggregate_trade_received_ns, delta)?;
        timing.book_ticker_received_ns = shift_optional(timing.book_ticker_received_ns, delta)?;
    }
    Ok(())
}

fn shift_optional(value: Option<i64>, delta: i64) -> Result<Option<i64>, IntegrationError> {
    value
        .map(|value| {
            value
                .checked_add(delta)
                .ok_or(IntegrationError::TimestampOverflow)
        })
        .transpose()
}

fn apply_post_fault(frame: &mut CoordinationFrame, fault: &Fault) {
    match fault {
        Fault::RemoveUpBook(key) => {
            if let Some(source) = frame.sessions.get_mut(key) {
                source.up_book = None;
            }
        }
        Fault::RemoveOracleCandle(key) => {
            if let Some(source) = frame.sessions.get_mut(key) {
                source.in_progress = None;
            }
        }
        Fault::SupervisorProvenanceMismatch => frame.market.digest[0] ^= 1,
        Fault::RemoveSession(key) => {
            frame.sessions.remove(key);
        }
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SoakPlan {
    pub start_time_ms: i64,
    pub hours: u16,
    pub ticks_per_hour: u16,
}

impl SoakPlan {
    /// Validates bounded aligned soak parameters.
    ///
    /// # Errors
    ///
    /// Rejects negative or unaligned starts, zero/excessive bounds, and tick
    /// counts that do not divide an hour exactly.
    pub fn validate(self) -> Result<Self, IntegrationError> {
        if self.start_time_ms < 0
            || self.start_time_ms % HOUR_MS != 0
            || self.hours == 0
            || self.hours > MAX_HOURS
            || self.ticks_per_hour == 0
            || self.ticks_per_hour > MAX_TICKS_PER_HOUR
            || HOUR_MS % i64::from(self.ticks_per_hour) != 0
        {
            return Err(IntegrationError::InvalidPlan);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SoakReport {
    pub ticks: u64,
    pub ready_session_observations: u64,
    pub degraded_session_observations: u64,
    pub finalized_sessions: usize,
    pub generated_sessions: usize,
    pub last_sequence: Option<u64>,
    pub coordinator_digest: [u8; 32],
}

/// Runs a deterministic multi-hour BTC/ETH integration soak.
///
/// # Errors
///
/// Returns on invalid bounds, arithmetic overflow, feed integrity failure, or
/// durable coordination failure.
pub fn run_soak<J: EventJournal>(
    durable: DurableCoordinator<J>,
    plan: SoakPlan,
    faults: FaultScript,
) -> Result<(SoakReport, DurableCoordinator<J>), IntegrationError> {
    let plan = plan.validate()?;
    let identities = generate_identities(plan)?;
    let mut engine = IntegrationEngine::new(durable, SupervisorConfig::default(), faults)?;
    for identity in &identities {
        engine.register(identity.clone(), 0)?;
    }
    let total_intervals = u64::from(plan.hours)
        .checked_mul(u64::from(plan.ticks_per_hour))
        .ok_or(IntegrationError::TickOverflow)?;
    let step_ms = HOUR_MS / i64::from(plan.ticks_per_hour);
    let mut ready = 0_u64;
    let mut degraded = 0_u64;
    for tick_index in 0..=total_intervals {
        let elapsed_ms = i64::try_from(tick_index)
            .map_err(|_| IntegrationError::TickOverflow)?
            .checked_mul(step_ms)
            .ok_or(IntegrationError::TimestampOverflow)?;
        let now_ms = plan
            .start_time_ms
            .checked_add(elapsed_ms)
            .ok_or(IntegrationError::TimestampOverflow)?;
        let snapshot = engine.step(synthetic_tick(now_ms, tick_index, &identities)?)?;
        for session in snapshot.sessions.values() {
            match session.phase {
                SessionPhase::ActiveReady => {
                    ready = ready.checked_add(1).ok_or(IntegrationError::TickOverflow)?;
                }
                SessionPhase::ActiveDegraded => {
                    degraded = degraded
                        .checked_add(1)
                        .ok_or(IntegrationError::TickOverflow)?;
                }
                _ => {}
            }
        }
    }
    let snapshot = engine.snapshot();
    let finalized_sessions = snapshot
        .sessions
        .values()
        .filter(|session| session.phase == SessionPhase::Finalized)
        .count();
    let generated_sessions = identities.len();
    if finalized_sessions != generated_sessions || !snapshot.current.is_empty() {
        return Err(IntegrationError::SoakIncomplete);
    }
    let report = SoakReport {
        ticks: total_intervals
            .checked_add(1)
            .ok_or(IntegrationError::TickOverflow)?,
        ready_session_observations: ready,
        degraded_session_observations: degraded,
        finalized_sessions,
        generated_sessions,
        last_sequence: engine.last_sequence(),
        coordinator_digest: snapshot.digest,
    };
    Ok((report, engine.into_durable()))
}

fn generate_identities(plan: SoakPlan) -> Result<Vec<MarketIdentity>, IntegrationError> {
    let mut identities = Vec::with_capacity(usize::from(plan.hours) * 2);
    for hour in 0..plan.hours {
        let start = plan
            .start_time_ms
            .checked_add(i64::from(hour) * HOUR_MS)
            .ok_or(IntegrationError::TimestampOverflow)?;
        for asset in [Asset::Bitcoin, Asset::Ethereum] {
            identities.push(synthetic_identity(asset, start, hour));
        }
    }
    Ok(identities)
}

fn synthetic_identity(asset: Asset, start_time_ms: i64, ordinal: u16) -> MarketIdentity {
    let (series, source, pair, asset_number) = match asset {
        Asset::Bitcoin => (
            BTC_HOURLY,
            "https://www.binance.com/en/trade/BTC_USDT",
            "BTC/USDT",
            1_u64,
        ),
        Asset::Ethereum => (
            ETH_HOURLY,
            "https://www.binance.com/en/trade/ETH_USDT",
            "ETH/USDT",
            2_u64,
        ),
    };
    let seed = format!("{}:{start_time_ms}:{ordinal}", asset.as_str());
    let condition_id = format!("0x{}", hex(blake3::hash(seed.as_bytes()).as_bytes()));
    let question_seed = format!("question:{seed}");
    let question_id = format!(
        "0x{}",
        hex(blake3::hash(question_seed.as_bytes()).as_bytes())
    );
    let token_base = u64::from(ordinal) * 10 + asset_number * 2;
    let description = format!(
        "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the {pair} 1 hour candle that begins on the time and date specified in the title. Otherwise, this market will resolve to \"Down\". The close and open displayed for the relevant candle will be used once the data for that candle is finalized. This market is about the Binance pair, not according to other exchanges or trading pairs."
    );
    MarketIdentity {
        asset,
        event_id: format!("{}-{start_time_ms}", asset.as_str()),
        market_id: format!("{}-{start_time_ms}-market", asset.as_str()),
        condition_id,
        question_id,
        event_slug: format!("{}-{start_time_ms}", asset.as_str().to_ascii_lowercase()),
        market_slug: format!(
            "{}-{start_time_ms}-market",
            asset.as_str().to_ascii_lowercase()
        ),
        series_id: series.id.to_owned(),
        series_slug: series.slug.to_owned(),
        title: format!("{} Up or Down", asset.as_str()),
        start_time_ms,
        end_time_ms: start_time_ms + HOUR_MS,
        resolution_source: source.to_owned(),
        description,
        up_token_id: (token_base + 1).to_string(),
        down_token_id: (token_base + 2).to_string(),
        rules_fingerprint: *blake3::hash(seed.as_bytes()).as_bytes(),
    }
}

fn synthetic_tick(
    now_ms: i64,
    sequence: u64,
    identities: &[MarketIdentity],
) -> Result<IntegrationTick, IntegrationError> {
    let evaluation_ns = now_ms
        .checked_mul(1_000_000)
        .ok_or(IntegrationError::TimestampOverflow)?;
    let market_digest = tagged_digest(b"SOAK_MARKET", sequence);
    let reference_digest = tagged_digest(b"SOAK_REFERENCE", sequence);
    let timing = ReferenceSymbolSnapshot {
        candle_event_ns: Some(evaluation_ns),
        candle_received_ns: Some(evaluation_ns),
        aggregate_trade_event_ns: Some(evaluation_ns),
        aggregate_trade_received_ns: Some(evaluation_ns),
        book_ticker_received_ns: Some(evaluation_ns),
    };
    let mut sessions = BTreeMap::new();
    for identity in identities {
        let active = identity.start_time_ms <= now_ms && now_ms < identity.end_time_ms;
        let ended = now_ms >= identity.end_time_ms;
        let candle = synthetic_candle(identity, now_ms)?;
        sessions.insert(
            SessionKey::from(identity),
            SessionSourceState {
                up_book: Some(synthetic_book()),
                down_book: Some(synthetic_book()),
                in_progress: active.then_some(InProgressCandle(candle)),
                finalized: ended.then_some(FinalizedCandle(candle)),
            },
        );
    }
    Ok(IntegrationTick {
        now_ns: evaluation_ns,
        market: ActorSnapshot {
            mode: ActorMode::Ready,
            ready: true,
            epoch: 1,
            last_sequence: Some(sequence),
            book_count: identities.len() * 2,
            digest: market_digest,
            last_market_event_ns: Some(evaluation_ns),
            last_market_received_ns: Some(evaluation_ns),
            halt_reason: None,
        },
        reference: ReferenceSnapshot {
            health: ReferenceHealth::Ready,
            epoch: 1,
            last_sequence: Some(sequence),
            digest: reference_digest,
            last_reference_received_ns: Some(evaluation_ns),
            symbols: BTreeMap::from([
                (ReferenceSymbol::BtcUsdt, timing),
                (ReferenceSymbol::EthUsdt, timing),
            ]),
        },
        sessions,
    })
}

fn synthetic_book() -> TokenBookView {
    TokenBookView {
        authoritative: true,
        best_bid: Some((
            PriceMicros::new(490_000).expect("constant valid price"),
            QuantityMicros::new(10_000_000).expect("constant valid quantity"),
        )),
        best_ask: Some((
            PriceMicros::new(510_000).expect("constant valid price"),
            QuantityMicros::new(10_000_000).expect("constant valid quantity"),
        )),
    }
}

fn synthetic_candle(
    identity: &MarketIdentity,
    now_ms: i64,
) -> Result<CandleData, IntegrationError> {
    let symbol = match identity.asset {
        Asset::Bitcoin => ReferenceSymbol::BtcUsdt,
        Asset::Ethereum => ReferenceSymbol::EthUsdt,
    };
    let base = match identity.asset {
        Asset::Bitcoin => 60_000_000_000_i64,
        Asset::Ethereum => 3_000_000_000_i64,
    };
    let hour_ordinal = identity.start_time_ms / HOUR_MS;
    let direction = if hour_ordinal % 2 == 0 { 1 } else { -1 };
    let elapsed = now_ms
        .saturating_sub(identity.start_time_ms)
        .clamp(0, HOUR_MS);
    let movement = direction * (elapsed / 1_000);
    let close = base
        .checked_add(movement)
        .ok_or(IntegrationError::TimestampOverflow)?;
    Ok(CandleData {
        symbol,
        interval: CandleInterval::OneHourUtc,
        open_time_ms: identity.start_time_ms,
        close_time_ms: identity.end_time_ms - 1,
        first_trade_id: 1,
        last_trade_id: 2,
        open: QuotePriceMicros::new(base).map_err(|_| IntegrationError::Financial)?,
        high: QuotePriceMicros::new(base + 10_000_000).map_err(|_| IntegrationError::Financial)?,
        low: QuotePriceMicros::new(base - 10_000_000).map_err(|_| IntegrationError::Financial)?,
        close: QuotePriceMicros::new(close).map_err(|_| IntegrationError::Financial)?,
        base_volume: ReferenceQuantityE8::new(100_000_000)
            .map_err(|_| IntegrationError::Financial)?,
        quote_volume: ReferenceQuantityE8::new(
            base.checked_mul(100)
                .ok_or(IntegrationError::TimestampOverflow)?,
        )
        .map_err(|_| IntegrationError::Financial)?,
        trade_count: 2,
    })
}

fn tagged_digest(tag: &[u8], sequence: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tag);
    hasher.update(&sequence.to_le_bytes());
    *hasher.finalize().as_bytes()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

#[derive(Debug, Error)]
pub enum IntegrationError {
    #[error("integration soak plan is invalid")]
    InvalidPlan,
    #[error("integration fault is invalid")]
    InvalidFault,
    #[error("integration fault count exceeds its bound")]
    FaultBound,
    #[error("integration fault was applied at the wrong layer")]
    FaultLayer,
    #[error("integration timestamp arithmetic overflow")]
    TimestampOverflow,
    #[error("integration tick counter overflow")]
    TickOverflow,
    #[error("integration synthetic fixed-point value is invalid")]
    Financial,
    #[error("integration soak ended without finalizing every session")]
    SoakIncomplete,
    #[error("cross-feed supervision failed: {0}")]
    Supervisor(#[from] SupervisorError),
    #[error("durable session runtime failed: {0}")]
    Runtime(#[from] RuntimeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_recorder::{SegmentConfig, SegmentedJournalWriter};
    use session_runtime::{recover_segmented, RecoveryState};
    use tempfile::tempdir;

    fn durable(directory: &std::path::Path) -> DurableCoordinator<SegmentedJournalWriter> {
        let writer = SegmentedJournalWriter::open(
            directory,
            SegmentConfig {
                max_segment_bytes: 32 * 1024,
                max_segment_records: 5,
            },
        )
        .expect("writer");
        DurableCoordinator::new(
            writer,
            RecoveryState {
                coordinator: market_session::MarketSessionCoordinator::default(),
                last_sequence: None,
            },
        )
        .expect("durable")
    }

    fn plan() -> SoakPlan {
        SoakPlan {
            start_time_ms: HOUR_MS,
            hours: 3,
            ticks_per_hour: 4,
        }
    }

    #[test]
    fn multi_hour_btc_eth_soak_rolls_and_finalizes_every_session() {
        let temp = tempdir().expect("temp");
        let (report, mut durable) = run_soak(
            durable(&temp.path().join("journal")),
            plan(),
            FaultScript::default(),
        )
        .expect("soak");
        durable.sync().expect("sync");
        assert_eq!(report.ticks, 13);
        assert_eq!(report.generated_sessions, 6);
        assert_eq!(report.finalized_sessions, 6);
        assert_eq!(report.ready_session_observations, 24);
        assert_eq!(report.degraded_session_observations, 0);
        assert_eq!(report.last_sequence, Some(18));
    }

    #[test]
    fn recoverable_feed_and_session_faults_degrade_then_recover() {
        let temp = tempdir().expect("temp");
        let identities = generate_identities(plan()).expect("identities");
        let btc = SessionKey::from(
            identities
                .iter()
                .find(|identity| identity.asset == Asset::Bitcoin)
                .expect("BTC"),
        );
        let script = FaultScript::new([
            (1, Fault::MarketUnavailable),
            (2, Fault::RemoveUpBook(btc)),
            (
                3,
                Fault::CrossFeedReceiveSkew {
                    nanos: 3_000_000_000,
                },
            ),
        ])
        .expect("script");
        let (report, _) = run_soak(durable(&temp.path().join("journal")), plan(), script)
            .expect("recoverable soak");
        assert_eq!(report.finalized_sessions, 6);
        assert_eq!(report.degraded_session_observations, 5);
        assert_eq!(report.ready_session_observations, 19);
    }

    #[test]
    fn identical_plans_produce_identical_reports_and_restart_digest() {
        let first = tempdir().expect("first");
        let second = tempdir().expect("second");
        let first_path = first.path().join("journal");
        let second_path = second.path().join("journal");
        let (first_report, first_durable) =
            run_soak(durable(&first_path), plan(), FaultScript::default()).expect("first soak");
        drop(first_durable);
        let (second_report, second_durable) =
            run_soak(durable(&second_path), plan(), FaultScript::default()).expect("second soak");
        drop(second_durable);
        assert_eq!(first_report, second_report);
        let recovered = recover_segmented(&first_path, None).expect("recover");
        assert_eq!(
            recovered.coordinator.snapshot().digest,
            first_report.coordinator_digest
        );
        assert_eq!(recovered.last_sequence, first_report.last_sequence);
    }

    #[test]
    fn feed_equivocation_and_post_supervision_provenance_faults_halt() {
        let first = tempdir().expect("first");
        let equivocation =
            FaultScript::new([(1, Fault::RepeatMarketSequenceWithDigestChange)]).expect("script");
        assert!(matches!(
            run_soak(durable(&first.path().join("journal")), plan(), equivocation),
            Err(IntegrationError::Supervisor(_))
        ));

        let second = tempdir().expect("second");
        let provenance =
            FaultScript::new([(1, Fault::SupervisorProvenanceMismatch)]).expect("script");
        assert!(matches!(
            run_soak(durable(&second.path().join("journal")), plan(), provenance),
            Err(IntegrationError::Runtime(_))
        ));
    }

    #[test]
    fn bounds_and_fault_validation_fail_closed() {
        assert!(matches!(
            SoakPlan {
                start_time_ms: 1,
                hours: 0,
                ticks_per_hour: 7,
            }
            .validate(),
            Err(IntegrationError::InvalidPlan)
        ));
        assert!(matches!(
            FaultScript::new([(0, Fault::CrossFeedReceiveSkew { nanos: 0 })]),
            Err(IntegrationError::InvalidFault)
        ));
        let too_many = (0..=MAX_FAULTS)
            .map(|tick| (u64::try_from(tick).expect("tick"), Fault::MarketUnavailable));
        assert!(matches!(
            FaultScript::new(too_many),
            Err(IntegrationError::FaultBound)
        ));
    }

    #[test]
    fn production_capture_adapter_never_invents_books_or_candles() {
        let identity = synthetic_identity(Asset::Bitcoin, HOUR_MS, 0);
        let tick = IntegrationTick::capture(
            HOUR_MS * 1_000_000,
            std::iter::once(&identity),
            ActorSnapshot {
                mode: ActorMode::Starting,
                ready: false,
                epoch: 0,
                last_sequence: None,
                book_count: 0,
                digest: [0; 32],
                last_market_event_ns: None,
                last_market_received_ns: None,
                halt_reason: None,
            },
            &ReplayState::default(),
            &ReferenceReplayState::default(),
        );
        let source = &tick.sessions[&SessionKey::from(&identity)];
        assert!(source.up_book.is_none());
        assert!(source.down_book.is_none());
        assert!(source.in_progress.is_none());
        assert!(source.finalized.is_none());
    }
}
