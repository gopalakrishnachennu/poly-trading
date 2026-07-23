#![forbid(unsafe_code)]

//! Deterministic, read-only coordination of hourly market sessions.
//!
//! The coordinator owns no network connection, clock, credentials, or order
//! path. It binds immutable identities to exact books and oracle windows, then
//! consumes caller-supplied snapshots through a fail-closed lifecycle.

use common_types::{PriceMicros, QuantityMicros};
use feed_supervisor::{SupervisorMode, SupervisorSnapshot};
use live_market_state::ActorSnapshot;
use order_book_replay::{BookKey, ReplayState};
use public_market_data::{Asset, MarketIdentity};
use reference_market_data::{
    FinalizedCandle, InProgressCandle, ReferenceReplayState, ReferenceSnapshot,
};
use resolution_rules::{
    IndicativeAssessment, OracleState, ResolutionContract, ResolutionError, ResolutionEvidence,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionKey {
    pub asset: Asset,
    pub start_time_ms: i64,
}

impl From<&MarketIdentity> for SessionKey {
    fn from(value: &MarketIdentity) -> Self {
        Self {
            asset: value.asset,
            start_time_ms: value.start_time_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenBookView {
    pub authoritative: bool,
    pub best_bid: Option<(PriceMicros, QuantityMicros)>,
    pub best_ask: Option<(PriceMicros, QuantityMicros)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSourceState {
    pub up_book: Option<TokenBookView>,
    pub down_book: Option<TokenBookView>,
    pub in_progress: Option<InProgressCandle>,
    pub finalized: Option<FinalizedCandle>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordinationFrame {
    pub now_ns: i64,
    pub market: ActorSnapshot,
    pub reference: ReferenceSnapshot,
    pub supervision: SupervisorSnapshot,
    pub sessions: BTreeMap<SessionKey, SessionSourceState>,
}

/// Captures only immutable, decision-relevant state for deterministic replay.
#[must_use]
pub fn capture_frame<'a>(
    now_ns: i64,
    identities: impl IntoIterator<Item = &'a MarketIdentity>,
    market: ActorSnapshot,
    books: &ReplayState,
    reference: &ReferenceReplayState,
    supervision: SupervisorSnapshot,
) -> CoordinationFrame {
    let sessions = capture_sources(identities, books, reference);
    CoordinationFrame {
        now_ns,
        market,
        reference: reference.snapshot(),
        supervision,
        sessions,
    }
}

/// Captures exact registered outcome books and oracle candles from immutable
/// replay states without inventing missing data.
#[must_use]
pub fn capture_sources<'a>(
    identities: impl IntoIterator<Item = &'a MarketIdentity>,
    books: &ReplayState,
    reference: &ReferenceReplayState,
) -> BTreeMap<SessionKey, SessionSourceState> {
    let mut sessions = BTreeMap::new();
    for identity in identities {
        let contract = ResolutionContract::bind(identity).ok();
        let source = SessionSourceState {
            up_book: token_book_view(books, &identity.condition_id, &identity.up_token_id),
            down_book: token_book_view(books, &identity.condition_id, &identity.down_token_id),
            in_progress: contract
                .as_ref()
                .and_then(|value| reference.in_progress_candle(value.symbol)),
            finalized: contract.as_ref().and_then(|value| {
                reference.finalized_candle(value.symbol, value.candle_open_time_ms)
            }),
        };
        sessions.insert(SessionKey::from(identity), source);
    }
    sessions
}

fn token_book_view(
    state: &ReplayState,
    condition_id: &str,
    asset_id: &str,
) -> Option<TokenBookView> {
    state
        .books()
        .get(&BookKey {
            condition_id: condition_id.to_owned(),
            asset_id: asset_id.to_owned(),
        })
        .map(|book| TokenBookView {
            authoritative: book.is_authoritative(),
            best_bid: book.best_bid(),
            best_ask: book.best_ask(),
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SessionPhase {
    Upcoming = 1,
    ActiveDegraded = 2,
    ActiveReady = 3,
    AwaitingFinalEvidence = 4,
    Finalized = 5,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ReadinessReason {
    BeforeWindow = 1,
    SupervisorNotReady = 2,
    OutcomeBooksUnavailable = 3,
    OracleCandleUnavailable = 4,
    Ready = 5,
    WindowEnded = 6,
    FinalEvidenceAvailable = 7,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSnapshot {
    pub key: SessionKey,
    pub condition_id: String,
    pub end_time_ms: i64,
    pub phase: SessionPhase,
    pub ready: bool,
    pub reason: ReadinessReason,
    pub indicative: Option<IndicativeAssessment>,
    pub final_evidence: Option<ResolutionEvidence>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordinatorSnapshot {
    pub evaluated_at_ns: Option<i64>,
    pub applied_frame_digest: Option<[u8; 32]>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub current: BTreeMap<Asset, SessionKey>,
    pub next: BTreeMap<Asset, SessionKey>,
    pub sessions: BTreeMap<SessionKey, SessionSnapshot>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug)]
struct SessionRecord {
    identity: MarketIdentity,
    oracle: OracleState,
    phase: SessionPhase,
    reason: ReadinessReason,
    indicative: Option<IndicativeAssessment>,
}

#[derive(Clone, Debug, Default)]
pub struct MarketSessionCoordinator {
    sessions: BTreeMap<SessionKey, SessionRecord>,
    last_now_ns: Option<i64>,
    last_frame_digest: Option<[u8; 32]>,
    halt_reason: Option<String>,
}

impl MarketSessionCoordinator {
    pub fn identities(&self) -> impl Iterator<Item = &MarketIdentity> {
        self.sessions.values().map(|record| &record.identity)
    }

    /// Registers one validated immutable hourly identity transactionally.
    ///
    /// # Errors
    ///
    /// Invalid contracts, slot conflicts, overlapping windows, reused
    /// conditions, or registration after halt permanently halt the coordinator.
    pub fn register(&mut self, identity: MarketIdentity) -> Result<(), CoordinatorError> {
        if let Some(reason) = &self.halt_reason {
            return Err(CoordinatorError::Halted(reason.clone()));
        }
        let key = SessionKey::from(&identity);
        if let Some(existing) = self.sessions.get(&key) {
            if existing.identity == identity {
                return Ok(());
            }
            return self.halt(CoordinatorError::ConflictingSlot(key));
        }
        if self.sessions.values().any(|record| {
            record.identity.condition_id == identity.condition_id
                || (record.identity.asset == identity.asset
                    && identity.start_time_ms < record.identity.end_time_ms
                    && identity.end_time_ms > record.identity.start_time_ms)
        }) {
            return self.halt(CoordinatorError::IdentityConflict);
        }
        let contract = match ResolutionContract::bind(&identity) {
            Ok(value) => value,
            Err(error) => return self.halt(CoordinatorError::Resolution(error)),
        };
        self.sessions.insert(
            key,
            SessionRecord {
                identity,
                oracle: OracleState::new(contract),
                phase: SessionPhase::Upcoming,
                reason: ReadinessReason::BeforeWindow,
                indicative: None,
            },
        );
        Ok(())
    }

    /// Applies one complete coordination frame transactionally.
    ///
    /// # Errors
    ///
    /// Permanent integrity failures halt the coordinator while preserving the
    /// last accepted session state.
    pub fn evaluate(
        &mut self,
        frame: &CoordinationFrame,
    ) -> Result<CoordinatorSnapshot, CoordinatorError> {
        if let Some(reason) = &self.halt_reason {
            return Err(CoordinatorError::Halted(reason.clone()));
        }
        if frame.now_ns < 0 {
            return self.halt(CoordinatorError::InvalidTime(frame.now_ns));
        }
        if self
            .last_now_ns
            .is_some_and(|previous| frame.now_ns < previous)
        {
            return self.halt(CoordinatorError::ClockRegression {
                previous: self.last_now_ns.unwrap_or(frame.now_ns),
                actual: frame.now_ns,
            });
        }
        if let Err(error) = validate_provenance(frame) {
            return self.halt(error);
        }
        let expected_keys: BTreeSet<_> = self.sessions.keys().copied().collect();
        let actual_keys: BTreeSet<_> = frame.sessions.keys().copied().collect();
        if expected_keys != actual_keys {
            return self.halt(CoordinatorError::SessionSetMismatch);
        }

        let mut candidate = self.sessions.clone();
        let now_ms = frame.now_ns / 1_000_000;
        let transition = (|| -> Result<(), CoordinatorError> {
            for (key, record) in &mut candidate {
                let source = frame
                    .sessions
                    .get(key)
                    .ok_or(CoordinatorError::SessionSetMismatch)?;
                if now_ms < record.identity.start_time_ms {
                    set_state(
                        record,
                        SessionPhase::Upcoming,
                        ReadinessReason::BeforeWindow,
                    );
                } else if now_ms < record.identity.end_time_ms {
                    evaluate_active(record, source, &frame.supervision)?;
                } else if let Some(candle) = source.finalized {
                    record.oracle.finalize(candle)?;
                    set_state(
                        record,
                        SessionPhase::Finalized,
                        ReadinessReason::FinalEvidenceAvailable,
                    );
                } else if record.oracle.final_evidence().is_some() {
                    set_state(
                        record,
                        SessionPhase::Finalized,
                        ReadinessReason::FinalEvidenceAvailable,
                    );
                } else {
                    set_state(
                        record,
                        SessionPhase::AwaitingFinalEvidence,
                        ReadinessReason::WindowEnded,
                    );
                }
            }
            Ok(())
        })();
        if let Err(error) = transition {
            return self.halt(error);
        }

        self.sessions = candidate;
        self.last_now_ns = Some(frame.now_ns);
        self.last_frame_digest = Some(coordination_frame_digest(frame));
        Ok(self.snapshot())
    }

    #[must_use]
    pub fn snapshot(&self) -> CoordinatorSnapshot {
        let now_ms = self.last_now_ns.map(|value| value / 1_000_000);
        let mut current = BTreeMap::new();
        let mut next = BTreeMap::new();
        if let Some(now_ms) = now_ms {
            for asset in [Asset::Bitcoin, Asset::Ethereum] {
                if let Some((key, _)) = self.sessions.iter().find(|(_, record)| {
                    record.identity.asset == asset
                        && record.identity.start_time_ms <= now_ms
                        && now_ms < record.identity.end_time_ms
                }) {
                    current.insert(asset, *key);
                }
                if let Some((key, _)) = self
                    .sessions
                    .iter()
                    .filter(|(_, record)| {
                        record.identity.asset == asset && record.identity.start_time_ms > now_ms
                    })
                    .min_by_key(|(_, record)| record.identity.start_time_ms)
                {
                    next.insert(asset, *key);
                }
            }
        }
        let sessions: BTreeMap<_, _> = self
            .sessions
            .iter()
            .map(|(key, record)| (*key, session_snapshot(*key, record)))
            .collect();
        let digest = coordinator_digest(
            self.last_now_ns,
            self.last_frame_digest,
            self.halt_reason.as_deref(),
            &sessions,
        );
        CoordinatorSnapshot {
            evaluated_at_ns: self.last_now_ns,
            applied_frame_digest: self.last_frame_digest,
            halted: self.halt_reason.is_some(),
            halt_reason: self.halt_reason.clone(),
            current,
            next,
            sessions,
            digest,
        }
    }

    fn halt<T>(&mut self, error: CoordinatorError) -> Result<T, CoordinatorError> {
        self.halt_reason = Some(error.to_string());
        Err(error)
    }
}

fn set_state(record: &mut SessionRecord, phase: SessionPhase, reason: ReadinessReason) {
    record.phase = phase;
    record.reason = reason;
    record.indicative = None;
}

fn evaluate_active(
    record: &mut SessionRecord,
    source: &SessionSourceState,
    supervision: &SupervisorSnapshot,
) -> Result<(), CoordinatorError> {
    record.indicative = match source.in_progress {
        Some(candle) => Some(record.oracle.assess(candle)?),
        None => None,
    };
    if supervision.mode != SupervisorMode::Ready || !supervision.ready {
        record.phase = SessionPhase::ActiveDegraded;
        record.reason = ReadinessReason::SupervisorNotReady;
    } else if !matches!(source.up_book, Some(book) if book.authoritative)
        || !matches!(source.down_book, Some(book) if book.authoritative)
    {
        record.phase = SessionPhase::ActiveDegraded;
        record.reason = ReadinessReason::OutcomeBooksUnavailable;
    } else if record.indicative.is_none() {
        record.phase = SessionPhase::ActiveDegraded;
        record.reason = ReadinessReason::OracleCandleUnavailable;
    } else {
        record.phase = SessionPhase::ActiveReady;
        record.reason = ReadinessReason::Ready;
    }
    Ok(())
}

fn validate_provenance(frame: &CoordinationFrame) -> Result<(), CoordinatorError> {
    let supervision = &frame.supervision;
    if supervision.evaluated_at_ns != Some(frame.now_ns)
        || supervision.market_epoch != frame.market.epoch
        || supervision.market_sequence != frame.market.last_sequence
        || supervision.market_state_digest != frame.market.digest
        || supervision.reference_epoch != frame.reference.epoch
        || supervision.reference_sequence != frame.reference.last_sequence
        || supervision.reference_state_digest != frame.reference.digest
    {
        return Err(CoordinatorError::SupervisorProvenanceMismatch);
    }
    Ok(())
}

fn session_snapshot(key: SessionKey, record: &SessionRecord) -> SessionSnapshot {
    let final_evidence = record.oracle.final_evidence().cloned();
    let digest = session_digest(key, record, final_evidence.as_ref());
    SessionSnapshot {
        key,
        condition_id: record.identity.condition_id.clone(),
        end_time_ms: record.identity.end_time_ms,
        phase: record.phase,
        ready: record.phase == SessionPhase::ActiveReady,
        reason: record.reason,
        indicative: record.indicative,
        final_evidence,
        digest,
    }
}

fn session_digest(
    key: SessionKey,
    record: &SessionRecord,
    evidence: Option<&ResolutionEvidence>,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"MARKET_SESSION_V1");
    hasher.update(&[key.asset as u8]);
    hasher.update(&key.start_time_ms.to_le_bytes());
    hasher.update(&record.identity.end_time_ms.to_le_bytes());
    encode_string(&mut hasher, &record.identity.condition_id);
    encode_string(&mut hasher, &record.identity.question_id);
    encode_string(&mut hasher, &record.identity.up_token_id);
    encode_string(&mut hasher, &record.identity.down_token_id);
    hasher.update(&record.identity.rules_fingerprint);
    hasher.update(&[record.phase as u8, record.reason as u8]);
    if let Some(value) = record.indicative {
        hasher.update(&[1, value.outcome_if_closed_now as u8]);
        hasher.update(&value.open.as_micros().to_le_bytes());
        hasher.update(&value.current_close.as_micros().to_le_bytes());
    } else {
        hasher.update(&[0]);
    }
    if let Some(value) = evidence {
        hasher.update(&[1]);
        encode_string(&mut hasher, &value.condition_id);
        encode_string(&mut hasher, &value.question_id);
        encode_string(&mut hasher, &value.winning_token_id);
        hasher.update(&value.rules_fingerprint);
        hasher.update(&[value.symbol as u8, value.outcome as u8]);
        hasher.update(&value.candle_open_time_ms.to_le_bytes());
        hasher.update(&value.candle_close_time_ms.to_le_bytes());
        hasher.update(&value.open.as_micros().to_le_bytes());
        hasher.update(&value.close.as_micros().to_le_bytes());
    } else {
        hasher.update(&[0]);
    }
    *hasher.finalize().as_bytes()
}

/// Produces the stable digest of one exact decision-relevant coordination frame.
#[must_use]
pub fn coordination_frame_digest(frame: &CoordinationFrame) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"COORDINATION_FRAME_V1");
    hasher.update(&frame.now_ns.to_le_bytes());
    hasher.update(&frame.market.digest);
    hasher.update(&frame.reference.digest);
    hasher.update(&frame.supervision.digest);
    for (key, source) in &frame.sessions {
        hasher.update(&[key.asset as u8]);
        hasher.update(&key.start_time_ms.to_le_bytes());
        for book in [&source.up_book, &source.down_book] {
            match book {
                Some(book) => {
                    hasher.update(&[1, u8::from(book.authoritative)]);
                    encode_level(&mut hasher, book.best_bid);
                    encode_level(&mut hasher, book.best_ask);
                }
                None => {
                    hasher.update(&[0]);
                }
            }
        }
        encode_candle_marker(&mut hasher, source.in_progress.map(|value| value.0));
        encode_candle_marker(&mut hasher, source.finalized.map(|value| value.0));
    }
    *hasher.finalize().as_bytes()
}

fn coordinator_digest(
    now_ns: Option<i64>,
    frame_digest: Option<[u8; 32]>,
    halt_reason: Option<&str>,
    sessions: &BTreeMap<SessionKey, SessionSnapshot>,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"MARKET_SESSION_COORDINATOR_V1");
    hasher.update(&now_ns.unwrap_or(i64::MIN).to_le_bytes());
    hasher.update(&frame_digest.unwrap_or([0; 32]));
    if let Some(reason) = halt_reason {
        hasher.update(&[1]);
        encode_string(&mut hasher, reason);
    } else {
        hasher.update(&[0]);
    }
    for snapshot in sessions.values() {
        hasher.update(&snapshot.digest);
    }
    *hasher.finalize().as_bytes()
}

fn encode_level(hasher: &mut blake3::Hasher, level: Option<(PriceMicros, QuantityMicros)>) {
    if let Some((price, quantity)) = level {
        hasher.update(&[1]);
        hasher.update(&price.as_micros().to_le_bytes());
        hasher.update(&quantity.as_micros().to_le_bytes());
    } else {
        hasher.update(&[0]);
    }
}

fn encode_candle_marker(
    hasher: &mut blake3::Hasher,
    candle: Option<reference_market_data::CandleData>,
) {
    if let Some(candle) = candle {
        hasher.update(&[1, candle.symbol as u8]);
        hasher.update(&candle.open_time_ms.to_le_bytes());
        hasher.update(&candle.close_time_ms.to_le_bytes());
        hasher.update(&candle.open.as_micros().to_le_bytes());
        hasher.update(&candle.close.as_micros().to_le_bytes());
    } else {
        hasher.update(&[0]);
    }
}

fn encode_string(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(value.as_bytes());
}

/// Replays registrations and frames through the same production core.
///
/// # Errors
///
/// Returns the first registration or coordination integrity failure.
pub fn replay(
    identities: &[MarketIdentity],
    frames: &[CoordinationFrame],
) -> Result<CoordinatorSnapshot, CoordinatorError> {
    let mut coordinator = MarketSessionCoordinator::default();
    for identity in identities {
        coordinator.register(identity.clone())?;
    }
    for frame in frames {
        coordinator.evaluate(frame)?;
    }
    Ok(coordinator.snapshot())
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CoordinatorError {
    #[error("invalid coordination time: {0}")]
    InvalidTime(i64),
    #[error("coordination clock regressed from {previous} to {actual}")]
    ClockRegression { previous: i64, actual: i64 },
    #[error("conflicting identity for hourly slot {0:?}")]
    ConflictingSlot(SessionKey),
    #[error("condition reuse or overlapping session identity")]
    IdentityConflict,
    #[error("resolution contract error: {0}")]
    Resolution(#[from] ResolutionError),
    #[error("supervisor provenance does not match component state")]
    SupervisorProvenanceMismatch,
    #[error("coordination frame session set does not match registry")]
    SessionSetMismatch,
    #[error("coordinator is permanently halted: {0}")]
    Halted(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_types::{QuotePriceMicros, ReferenceQuantityE8};
    use feed_supervisor::SupervisorMode;
    use live_market_state::ActorMode;
    use public_market_data::{BTC_HOURLY, ETH_HOURLY};
    use reference_market_data::{CandleData, CandleInterval, ReferenceHealth, ReferenceSymbol};

    const HOUR_MS: i64 = 3_600_000;

    fn description(pair: &str) -> String {
        format!(
            "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the {pair} 1 hour candle that begins on the time and date specified in the title. Otherwise, this market will resolve to \"Down\". The close and open displayed for the relevant candle will be used once the data for that candle is finalized. This market is about the Binance pair, not according to other exchanges or trading pairs."
        )
    }

    fn market(asset: Asset, start: i64, suffix: char) -> MarketIdentity {
        let (series, source, pair) = match asset {
            Asset::Bitcoin => (
                BTC_HOURLY,
                "https://www.binance.com/en/trade/BTC_USDT",
                "BTC/USDT",
            ),
            Asset::Ethereum => (
                ETH_HOURLY,
                "https://www.binance.com/en/trade/ETH_USDT",
                "ETH/USDT",
            ),
        };
        MarketIdentity {
            asset,
            event_id: format!("event-{suffix}"),
            market_id: format!("market-{suffix}"),
            condition_id: format!("0x{}", suffix.to_string().repeat(64)),
            question_id: format!("0x{}", "b".repeat(64)),
            event_slug: format!("event-{suffix}"),
            market_slug: format!("market-{suffix}"),
            series_id: series.id.to_owned(),
            series_slug: series.slug.to_owned(),
            title: "Up or Down".to_owned(),
            start_time_ms: start,
            end_time_ms: start + HOUR_MS,
            resolution_source: source.to_owned(),
            description: description(pair),
            up_token_id: format!("1{}", suffix as u32),
            down_token_id: format!("2{}", suffix as u32),
            rules_fingerprint: [suffix as u8; 32],
        }
    }

    fn candle(asset: Asset, start: i64, close: i64) -> CandleData {
        CandleData {
            symbol: match asset {
                Asset::Bitcoin => ReferenceSymbol::BtcUsdt,
                Asset::Ethereum => ReferenceSymbol::EthUsdt,
            },
            interval: CandleInterval::OneHourUtc,
            open_time_ms: start,
            close_time_ms: start + HOUR_MS - 1,
            first_trade_id: 1,
            last_trade_id: 2,
            open: QuotePriceMicros::new(100_000_000).expect("open"),
            high: QuotePriceMicros::new(110_000_000).expect("high"),
            low: QuotePriceMicros::new(90_000_000).expect("low"),
            close: QuotePriceMicros::new(close).expect("close"),
            base_volume: ReferenceQuantityE8::new(100_000_000).expect("volume"),
            quote_volume: ReferenceQuantityE8::new(10_000_000_000).expect("volume"),
            trade_count: 2,
        }
    }

    fn book(authoritative: bool) -> TokenBookView {
        TokenBookView {
            authoritative,
            best_bid: Some((
                PriceMicros::new(400_000).expect("price"),
                QuantityMicros::new(2_000_000).expect("quantity"),
            )),
            best_ask: Some((
                PriceMicros::new(410_000).expect("price"),
                QuantityMicros::new(3_000_000).expect("quantity"),
            )),
        }
    }

    fn frame(now_ms: i64, markets: &[MarketIdentity], ready: bool) -> CoordinationFrame {
        let evaluation_ns = now_ms * 1_000_000;
        let market_digest = [3; 32];
        let reference_digest = [4; 32];
        let market = ActorSnapshot {
            mode: if ready {
                ActorMode::Ready
            } else {
                ActorMode::Stale
            },
            ready,
            epoch: 2,
            last_sequence: Some(20),
            book_count: markets.len() * 2,
            digest: market_digest,
            last_market_event_ns: Some(evaluation_ns),
            last_market_received_ns: Some(evaluation_ns),
            halt_reason: None,
        };
        let reference = ReferenceSnapshot {
            health: if ready {
                ReferenceHealth::Ready
            } else {
                ReferenceHealth::Disconnected
            },
            epoch: 3,
            last_sequence: Some(30),
            digest: reference_digest,
            last_reference_received_ns: Some(evaluation_ns),
            symbols: BTreeMap::new(),
        };
        let supervision = SupervisorSnapshot {
            mode: if ready {
                SupervisorMode::Ready
            } else {
                SupervisorMode::MarketStale
            },
            ready,
            evaluated_at_ns: Some(evaluation_ns),
            market_epoch: market.epoch,
            market_sequence: market.last_sequence,
            market_digest: [5; 32],
            market_state_digest: market_digest,
            reference_epoch: reference.epoch,
            reference_sequence: reference.last_sequence,
            reference_digest: [6; 32],
            reference_state_digest: reference_digest,
            halt_reason: None,
            digest: [7; 32],
        };
        let sessions = markets
            .iter()
            .map(|identity| {
                (
                    SessionKey::from(identity),
                    SessionSourceState {
                        up_book: Some(book(true)),
                        down_book: Some(book(true)),
                        in_progress: Some(InProgressCandle(candle(
                            identity.asset,
                            identity.start_time_ms,
                            101_000_000,
                        ))),
                        finalized: None,
                    },
                )
            })
            .collect();
        CoordinationFrame {
            now_ns: evaluation_ns,
            market,
            reference,
            supervision,
            sessions,
        }
    }

    #[test]
    fn active_session_requires_all_components_and_exact_boundaries() {
        let identity = market(Asset::Bitcoin, HOUR_MS, 'a');
        let mut coordinator = MarketSessionCoordinator::default();
        coordinator.register(identity.clone()).expect("register");

        let before = coordinator
            .evaluate(&frame(HOUR_MS - 1, std::slice::from_ref(&identity), true))
            .expect("before");
        assert_eq!(
            before.sessions[&SessionKey::from(&identity)].phase,
            SessionPhase::Upcoming
        );
        let active = coordinator
            .evaluate(&frame(HOUR_MS, std::slice::from_ref(&identity), true))
            .expect("active");
        assert!(active.sessions[&SessionKey::from(&identity)].ready);
        assert_eq!(active.current[&Asset::Bitcoin], SessionKey::from(&identity));
        let ended = coordinator
            .evaluate(&frame(
                identity.end_time_ms,
                std::slice::from_ref(&identity),
                true,
            ))
            .expect("ended");
        assert_eq!(
            ended.sessions[&SessionKey::from(&identity)].phase,
            SessionPhase::AwaitingFinalEvidence
        );
        assert!(!ended.current.contains_key(&Asset::Bitcoin));
    }

    #[test]
    fn active_degradation_reasons_are_explicit_and_recoverable() {
        let identity = market(Asset::Bitcoin, HOUR_MS, 'a');
        let mut coordinator = MarketSessionCoordinator::default();
        coordinator.register(identity.clone()).expect("register");
        let key = SessionKey::from(&identity);

        let degraded = coordinator
            .evaluate(&frame(HOUR_MS, std::slice::from_ref(&identity), false))
            .expect("degraded");
        assert_eq!(
            degraded.sessions[&key].reason,
            ReadinessReason::SupervisorNotReady
        );
        let mut missing_book = frame(HOUR_MS + 1, std::slice::from_ref(&identity), true);
        missing_book
            .sessions
            .get_mut(&key)
            .expect("session")
            .down_book = None;
        assert_eq!(
            coordinator
                .evaluate(&missing_book)
                .expect("missing book")
                .sessions[&key]
                .reason,
            ReadinessReason::OutcomeBooksUnavailable
        );
        let mut missing_candle = frame(HOUR_MS + 2, std::slice::from_ref(&identity), true);
        missing_candle
            .sessions
            .get_mut(&key)
            .expect("session")
            .in_progress = None;
        assert_eq!(
            coordinator
                .evaluate(&missing_candle)
                .expect("missing candle")
                .sessions[&key]
                .reason,
            ReadinessReason::OracleCandleUnavailable
        );
        assert!(
            coordinator
                .evaluate(&frame(HOUR_MS + 3, std::slice::from_ref(&identity), true,))
                .expect("recovered")
                .sessions[&key]
                .ready
        );
    }

    #[test]
    fn rollover_keeps_prior_pending_then_attaches_immutable_evidence() {
        let first = market(Asset::Bitcoin, HOUR_MS, 'a');
        let second = market(Asset::Bitcoin, 2 * HOUR_MS, 'c');
        let markets = [first.clone(), second.clone()];
        let first_key = SessionKey::from(&first);
        let second_key = SessionKey::from(&second);
        let mut coordinator = MarketSessionCoordinator::default();
        coordinator.register(first.clone()).expect("first");
        coordinator.register(second.clone()).expect("second");

        let rolled = coordinator
            .evaluate(&frame(2 * HOUR_MS, &markets, true))
            .expect("rollover");
        assert_eq!(rolled.current[&Asset::Bitcoin], second_key);
        assert_eq!(
            rolled.sessions[&first_key].phase,
            SessionPhase::AwaitingFinalEvidence
        );
        assert!(rolled.sessions[&second_key].ready);

        let mut finalized = frame(2 * HOUR_MS + 1, &markets, true);
        finalized
            .sessions
            .get_mut(&first_key)
            .expect("first source")
            .finalized = Some(FinalizedCandle(candle(Asset::Bitcoin, HOUR_MS, 99_000_000)));
        let complete = coordinator.evaluate(&finalized).expect("finalize");
        assert_eq!(complete.sessions[&first_key].phase, SessionPhase::Finalized);
        assert!(complete.sessions[&first_key].final_evidence.is_some());
        assert_eq!(complete.current[&Asset::Bitcoin], second_key);

        let retained = coordinator
            .evaluate(&frame(2 * HOUR_MS + 2, &markets, true))
            .expect("retained");
        assert!(retained.sessions[&first_key].final_evidence.is_some());
    }

    #[test]
    fn gaps_have_no_current_session_but_preserve_next() {
        let identity = market(Asset::Ethereum, 3 * HOUR_MS, 'd');
        let mut coordinator = MarketSessionCoordinator::default();
        coordinator.register(identity.clone()).expect("register");
        let snapshot = coordinator
            .evaluate(&frame(2 * HOUR_MS, std::slice::from_ref(&identity), true))
            .expect("gap");
        assert!(!snapshot.current.contains_key(&Asset::Ethereum));
        assert_eq!(snapshot.next[&Asset::Ethereum], SessionKey::from(&identity));
    }

    #[test]
    fn identity_conflicts_halt_and_halt_is_absorbing() {
        let identity = market(Asset::Bitcoin, HOUR_MS, 'a');
        let mut conflict = identity.clone();
        conflict.condition_id = format!("0x{}", "e".repeat(64));
        let mut coordinator = MarketSessionCoordinator::default();
        coordinator.register(identity).expect("register");
        assert!(matches!(
            coordinator.register(conflict),
            Err(CoordinatorError::ConflictingSlot(_))
        ));
        assert!(matches!(
            coordinator.register(market(Asset::Ethereum, HOUR_MS, 'd')),
            Err(CoordinatorError::Halted(_))
        ));
        assert!(coordinator.snapshot().halted);
    }

    #[test]
    fn provenance_and_clock_failures_are_transactional() {
        let identity = market(Asset::Bitcoin, HOUR_MS, 'a');
        let mut coordinator = MarketSessionCoordinator::default();
        coordinator.register(identity.clone()).expect("register");
        coordinator
            .evaluate(&frame(HOUR_MS, std::slice::from_ref(&identity), true))
            .expect("ready");
        let accepted_session =
            coordinator.snapshot().sessions[&SessionKey::from(&identity)].clone();
        let mut invalid = frame(HOUR_MS + 1, std::slice::from_ref(&identity), true);
        invalid.supervision.market_state_digest = [99; 32];
        assert_eq!(
            coordinator.evaluate(&invalid),
            Err(CoordinatorError::SupervisorProvenanceMismatch)
        );
        assert_eq!(
            coordinator.snapshot().sessions[&SessionKey::from(&identity)],
            accepted_session
        );

        let mut clock = MarketSessionCoordinator::default();
        clock.register(identity.clone()).expect("register");
        clock
            .evaluate(&frame(HOUR_MS + 1, std::slice::from_ref(&identity), true))
            .expect("first");
        assert!(matches!(
            clock.evaluate(&frame(HOUR_MS, std::slice::from_ref(&identity), true)),
            Err(CoordinatorError::ClockRegression { .. })
        ));
    }

    #[test]
    fn wrong_oracle_window_halts_without_installing_assessment() {
        let identity = market(Asset::Bitcoin, HOUR_MS, 'a');
        let key = SessionKey::from(&identity);
        let mut coordinator = MarketSessionCoordinator::default();
        coordinator.register(identity.clone()).expect("register");
        let mut invalid = frame(HOUR_MS, std::slice::from_ref(&identity), true);
        invalid.sessions.get_mut(&key).expect("session").in_progress = Some(InProgressCandle(
            candle(Asset::Bitcoin, 2 * HOUR_MS, 101_000_000),
        ));
        assert_eq!(
            coordinator.evaluate(&invalid),
            Err(CoordinatorError::Resolution(
                ResolutionError::CandleWindowMismatch
            ))
        );
        assert!(coordinator.snapshot().halted);
        assert!(coordinator.snapshot().sessions[&key].indicative.is_none());
    }

    #[test]
    fn online_and_replay_digests_match() {
        let identity = market(Asset::Bitcoin, HOUR_MS, 'a');
        let frames = vec![
            frame(HOUR_MS - 1, std::slice::from_ref(&identity), true),
            frame(HOUR_MS, std::slice::from_ref(&identity), true),
        ];
        let mut online = MarketSessionCoordinator::default();
        online.register(identity.clone()).expect("register");
        for value in &frames {
            online.evaluate(value).expect("evaluate");
        }
        let replayed = replay(std::slice::from_ref(&identity), &frames).expect("replay");
        assert_eq!(online.snapshot(), replayed);
        assert_eq!(online.snapshot().digest, replayed.digest);
    }

    #[test]
    fn capture_frame_uses_exact_registered_keys_without_inventing_state() {
        let identity = market(Asset::Bitcoin, HOUR_MS, 'a');
        let actor = frame(HOUR_MS, std::slice::from_ref(&identity), true);
        let captured = capture_frame(
            actor.now_ns,
            std::iter::once(&identity),
            actor.market,
            &ReplayState::default(),
            &ReferenceReplayState::default(),
            actor.supervision,
        );
        let source = &captured.sessions[&SessionKey::from(&identity)];
        assert!(source.up_book.is_none());
        assert!(source.down_book.is_none());
        assert!(source.in_progress.is_none());
        assert!(source.finalized.is_none());
    }
}
