#![forbid(unsafe_code)]

use chrono::DateTime;
use common_types::{parse_price_micros, parse_quantity_micros, parse_quote_price_micros};
use public_market_data::{Asset, MarketIdentity};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const SCHEMA_VERSION: u16 = 1;
pub const REQUIRED_ASSETS: [&str; 2] = ["BTC", "ETH"];
// Public CLOB books can legitimately contain many thousands of levels during
// active hourly sessions. The HTTP response is independently bounded at the
// gateway and every level is still validated before sorting, while only the
// best projected levels are retained below.  This is deliberately a parser
// safety bound, rather than a liquidity assumption: rejecting a deep but valid
// venue book makes the whole terminal flap into an avoidable NO_TRADE state.
const MAX_BOOK_LEVELS: usize = 50_000;
const PROJECTED_BOOK_LEVELS: usize = 10;
const MAX_REASON_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionMode {
    Discovering,
    Ready,
    Stale,
    Halted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelProjection {
    pub price_micros: String,
    pub quantity_micros: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookProjection {
    pub token_id: String,
    pub condition_id: String,
    pub source_timestamp_ms: i64,
    pub received_at_ms: i64,
    pub hash: String,
    pub tick_size_micros: String,
    pub bids: Vec<LevelProjection>,
    pub asks: Vec<LevelProjection>,
    pub best_bid_micros: String,
    pub best_ask_micros: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairObservation {
    pub buy_pair_cost_micros: String,
    pub raw_gap_micros: String,
    pub executable_quantity_micros: String,
    pub observation: String,
    pub decision: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedProjection {
    pub market_identity: String,
    pub up_book: String,
    pub down_book: String,
    pub reference: String,
    pub age_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetProjection {
    pub asset: String,
    pub symbol: String,
    pub title: String,
    pub event_slug: String,
    pub condition_id: String,
    pub market_id: String,
    pub rules_fingerprint: String,
    pub resolution_source: String,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub reference_price_micros: String,
    pub target_price_micros: String,
    pub reference_received_at_ms: i64,
    pub up_book: BookProjection,
    pub down_book: BookProjection,
    pub pair: PairObservation,
    pub feed: FeedProjection,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct TerminalSnapshot {
    pub schema_version: u16,
    pub sequence: u64,
    pub generated_at_ms: i64,
    pub last_success_at_ms: Option<i64>,
    pub mode: ProjectionMode,
    pub no_trade: bool,
    pub reason: String,
    pub assets: Vec<AssetProjection>,
    pub credentials_present: bool,
    pub authenticated_transport_present: bool,
    pub order_submission_present: bool,
    pub financial_authority_present: bool,
    pub snapshot_digest: String,
}

impl TerminalSnapshot {
    fn sealed(mut self) -> Self {
        self.snapshot_digest.clear();
        let body = serde_json::to_vec(&self).expect("bounded projection serialization");
        self.snapshot_digest = hex(blake3::hash(&body).as_bytes());
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        let mut copy = self.clone();
        let expected = copy.snapshot_digest.clone();
        copy.snapshot_digest.clear();
        serde_json::to_vec(&copy).is_ok_and(|body| expected == hex(blake3::hash(&body).as_bytes()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawLevel {
    pub price: String,
    pub size: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawBook {
    pub condition_id: String,
    pub token_id: String,
    pub timestamp: String,
    pub hash: String,
    pub tick_size: String,
    pub bids: Vec<RawLevel>,
    pub asks: Vec<RawLevel>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawReference {
    pub symbol: String,
    pub price: String,
    pub candle_open_time_ms: i64,
    pub candle_close_time_ms: i64,
    pub candle_open: String,
    pub received_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectionPolicy {
    pub maximum_book_age_ms: i64,
    pub maximum_reference_age_ms: i64,
    pub maximum_cross_book_skew_ms: i64,
    pub maximum_projection_age_ms: i64,
}

impl Default for ProjectionPolicy {
    fn default() -> Self {
        Self {
            maximum_book_age_ms: 5_000,
            maximum_reference_age_ms: 5_000,
            maximum_cross_book_skew_ms: 2_000,
            maximum_projection_age_ms: 5_000,
        }
    }
}

impl ProjectionPolicy {
    fn valid(self) -> bool {
        self.maximum_book_age_ms > 0
            && self.maximum_reference_age_ms > 0
            && self.maximum_cross_book_skew_ms >= 0
            && self.maximum_projection_age_ms > 0
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProjectionError {
    #[error("projection policy invalid")]
    Policy,
    #[error("projection timestamp invalid")]
    Timestamp,
    #[error("current hourly market missing or duplicated for {0}")]
    MarketCardinality(&'static str),
    #[error("market identity changed within one projection")]
    MarketIdentity,
    #[error("book identity mismatch")]
    BookIdentity,
    #[error("book timestamp invalid")]
    BookTimestamp,
    #[error("book stale")]
    BookStale,
    #[error("book level count invalid")]
    BookLevelCount,
    #[error("book fixed-point value invalid")]
    BookValue,
    #[error("book is one-sided or crossed")]
    BookShape,
    #[error("complementary book timestamps exceed skew")]
    BookSkew,
    #[error("reference identity or hour mismatch")]
    ReferenceIdentity,
    #[error("reference stale")]
    ReferenceStale,
    #[error("reference fixed-point value invalid")]
    ReferenceValue,
    #[error("projection asset set invalid")]
    AssetSet,
    #[error("projection sequence exhausted")]
    Sequence,
    #[error("projection clock regressed")]
    ClockRegression,
    #[error("projection arithmetic overflow")]
    Arithmetic,
    #[error("projection halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct ProjectionState {
    policy: ProjectionPolicy,
    sequence: u64,
    last_at_ms: Option<i64>,
    last_success_at_ms: Option<i64>,
    snapshot: TerminalSnapshot,
    halted: Option<String>,
}

impl ProjectionState {
    /// Creates an empty, non-authorizing projection owner.
    /// # Errors
    /// Rejects nonpositive freshness limits.
    pub fn new(policy: ProjectionPolicy, now_ms: i64) -> Result<Self, ProjectionError> {
        if !policy.valid() || now_ms < 0 {
            return Err(ProjectionError::Policy);
        }
        let snapshot = unavailable_snapshot(
            0,
            now_ms,
            None,
            ProjectionMode::Discovering,
            "awaiting validated public data",
        );
        Ok(Self {
            policy,
            sequence: 0,
            last_at_ms: Some(now_ms),
            last_success_at_ms: None,
            snapshot,
            halted: None,
        })
    }

    /// Publishes an atomic two-asset refresh.
    /// # Errors
    /// Rejects incomplete, stale, regressed or authority-bearing data.
    pub fn publish_ready(
        &mut self,
        mut assets: Vec<AssetProjection>,
        now_ms: i64,
    ) -> Result<(), ProjectionError> {
        self.preflight_time(now_ms)?;
        validate_asset_set(&assets, now_ms)?;
        assets.sort_by(|left, right| left.asset.cmp(&right.asset));
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ProjectionError::Sequence)?;
        self.last_at_ms = Some(now_ms);
        self.last_success_at_ms = Some(now_ms);
        self.snapshot = TerminalSnapshot {
            schema_version: SCHEMA_VERSION,
            sequence: self.sequence,
            generated_at_ms: now_ms,
            last_success_at_ms: self.last_success_at_ms,
            mode: ProjectionMode::Ready,
            no_trade: true,
            reason: "public projection ready; execution authority absent".to_owned(),
            assets,
            credentials_present: false,
            authenticated_transport_present: false,
            order_submission_present: false,
            financial_authority_present: false,
            snapshot_digest: String::new(),
        }
        .sealed();
        Ok(())
    }

    /// Publishes an attributable unavailable state and clears asset authority.
    /// # Errors
    /// Rejects clock regression, overlong reasons and sequence exhaustion.
    pub fn publish_unavailable(
        &mut self,
        now_ms: i64,
        reason: impl Into<String>,
    ) -> Result<(), ProjectionError> {
        self.preflight_time(now_ms)?;
        let reason = reason.into();
        if reason.is_empty() || reason.len() > MAX_REASON_BYTES {
            return Err(ProjectionError::AssetSet);
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ProjectionError::Sequence)?;
        self.last_at_ms = Some(now_ms);
        let mode = if self.last_success_at_ms.is_some() {
            ProjectionMode::Stale
        } else {
            ProjectionMode::Discovering
        };
        self.snapshot = unavailable_snapshot(
            self.sequence,
            now_ms,
            self.last_success_at_ms,
            mode,
            &reason,
        );
        Ok(())
    }

    /// Re-evaluates projection freshness.
    /// # Errors
    /// Rejects clock regression and sequence exhaustion.
    pub fn evaluate_freshness(&mut self, now_ms: i64) -> Result<(), ProjectionError> {
        self.preflight_time(now_ms)?;
        if self.snapshot.mode == ProjectionMode::Ready
            && now_ms
                .checked_sub(self.snapshot.generated_at_ms)
                .ok_or(ProjectionError::ClockRegression)?
                > self.policy.maximum_projection_age_ms
        {
            self.publish_unavailable(now_ms, "projection freshness budget exceeded")?;
        }
        Ok(())
    }

    #[must_use]
    pub const fn snapshot(&self) -> &TerminalSnapshot {
        &self.snapshot
    }

    fn preflight_time(&mut self, now_ms: i64) -> Result<(), ProjectionError> {
        if let Some(reason) = &self.halted {
            return Err(ProjectionError::Halted(reason.clone()));
        }
        if now_ms < 0 || self.last_at_ms.is_some_and(|last| now_ms < last) {
            let error = ProjectionError::ClockRegression;
            self.halted = Some(error.to_string());
            self.sequence = self.sequence.saturating_add(1);
            self.snapshot = unavailable_snapshot(
                self.sequence,
                now_ms.max(0),
                self.last_success_at_ms,
                ProjectionMode::Halted,
                &error.to_string(),
            );
            return Err(error);
        }
        Ok(())
    }
}

/// Selects exactly one active validated market for BTC and ETH.
/// # Errors
/// Rejects missing or duplicate active identities.
pub fn select_current_markets(
    markets: &[MarketIdentity],
    now_ms: i64,
) -> Result<BTreeMap<Asset, MarketIdentity>, ProjectionError> {
    if now_ms < 0 {
        return Err(ProjectionError::Timestamp);
    }
    let mut selected = BTreeMap::new();
    for asset in [Asset::Bitcoin, Asset::Ethereum] {
        let current: Vec<_> = markets
            .iter()
            .filter(|market| {
                market.asset == asset
                    && market.start_time_ms <= now_ms
                    && now_ms < market.end_time_ms
            })
            .collect();
        if current.len() != 1 {
            return Err(ProjectionError::MarketCardinality(asset.as_str()));
        }
        selected.insert(asset, current[0].clone());
    }
    Ok(selected)
}

/// Validates one exact public CLOB book.
/// # Errors
/// Rejects identity substitution, staleness, malformed fixed point, bounds and crossed books.
pub fn normalize_book(
    raw: RawBook,
    expected_condition: &str,
    expected_token: &str,
    received_at_ms: i64,
    policy: ProjectionPolicy,
) -> Result<BookProjection, ProjectionError> {
    if !policy.valid() {
        return Err(ProjectionError::Policy);
    }
    if raw.condition_id != expected_condition
        || raw.token_id != expected_token
        || raw.hash.is_empty()
    {
        return Err(ProjectionError::BookIdentity);
    }
    if raw.bids.is_empty()
        || raw.asks.is_empty()
        || raw.bids.len() > MAX_BOOK_LEVELS
        || raw.asks.len() > MAX_BOOK_LEVELS
    {
        return Err(ProjectionError::BookLevelCount);
    }
    let source_timestamp_ms = parse_book_timestamp(&raw.timestamp)?;
    if source_timestamp_ms < 0 || source_timestamp_ms > received_at_ms {
        return Err(ProjectionError::BookTimestamp);
    }
    if received_at_ms - source_timestamp_ms > policy.maximum_book_age_ms {
        return Err(ProjectionError::BookStale);
    }
    let tick = parse_price_micros(&raw.tick_size)
        .map_err(|_| ProjectionError::BookValue)?
        .as_micros();
    if tick <= 0 {
        return Err(ProjectionError::BookValue);
    }
    let mut bids = normalize_levels(raw.bids)?;
    let mut asks = normalize_levels(raw.asks)?;
    bids.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.cmp(&left.1)));
    asks.sort_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(&left.1)));
    if bids[0].0 >= asks[0].0 {
        return Err(ProjectionError::BookShape);
    }
    let best_bid_micros = bids[0].0.to_string();
    let best_ask_micros = asks[0].0.to_string();
    Ok(BookProjection {
        token_id: raw.token_id,
        condition_id: raw.condition_id,
        source_timestamp_ms,
        received_at_ms,
        hash: raw.hash,
        tick_size_micros: tick.to_string(),
        bids: project_levels(bids),
        asks: project_levels(asks),
        best_bid_micros,
        best_ask_micros,
    })
}

/// Composes an all-or-nothing exact hourly asset projection.
/// # Errors
/// Rejects complementary mismatch, skew, stale reference and arithmetic failure.
pub fn compose_asset(
    identity: &MarketIdentity,
    up_book: BookProjection,
    down_book: BookProjection,
    reference: &RawReference,
    now_ms: i64,
    policy: ProjectionPolicy,
) -> Result<AssetProjection, ProjectionError> {
    if up_book.condition_id != identity.condition_id
        || down_book.condition_id != identity.condition_id
        || up_book.token_id != identity.up_token_id
        || down_book.token_id != identity.down_token_id
    {
        return Err(ProjectionError::MarketIdentity);
    }
    let skew = (up_book.source_timestamp_ms - down_book.source_timestamp_ms).abs();
    if skew > policy.maximum_cross_book_skew_ms {
        return Err(ProjectionError::BookSkew);
    }
    let symbol = match identity.asset {
        Asset::Bitcoin => "BTCUSDT",
        Asset::Ethereum => "ETHUSDT",
    };
    if reference.symbol != symbol
        || reference.candle_open_time_ms != identity.start_time_ms
        || reference.candle_close_time_ms != identity.end_time_ms - 1
    {
        return Err(ProjectionError::ReferenceIdentity);
    }
    if reference.received_at_ms > now_ms
        || now_ms - reference.received_at_ms > policy.maximum_reference_age_ms
    {
        return Err(ProjectionError::ReferenceStale);
    }
    let reference_price = parse_quote_price_micros(&reference.price)
        .map_err(|_| ProjectionError::ReferenceValue)?
        .as_micros();
    let target = parse_quote_price_micros(&reference.candle_open)
        .map_err(|_| ProjectionError::ReferenceValue)?
        .as_micros();
    let up_ask = parse_i64(&up_book.best_ask_micros)?;
    let down_ask = parse_i64(&down_book.best_ask_micros)?;
    let cost = up_ask
        .checked_add(down_ask)
        .ok_or(ProjectionError::Arithmetic)?;
    let gap = 1_000_000_i64
        .checked_sub(cost)
        .ok_or(ProjectionError::Arithmetic)?;
    let up_quantity = parse_i64(&up_book.asks[0].quantity_micros)?;
    let down_quantity = parse_i64(&down_book.asks[0].quantity_micros)?;
    let quantity = up_quantity.min(down_quantity);
    let observation = if gap > 0 {
        "raw_pair_below_one"
    } else {
        "no_raw_pair_edge"
    };
    let age = now_ms
        - [
            up_book.received_at_ms,
            down_book.received_at_ms,
            reference.received_at_ms,
        ]
        .into_iter()
        .min()
        .ok_or(ProjectionError::Timestamp)?;
    Ok(AssetProjection {
        asset: identity.asset.as_str().to_owned(),
        symbol: symbol.to_owned(),
        title: identity.title.clone(),
        event_slug: identity.event_slug.clone(),
        condition_id: identity.condition_id.clone(),
        market_id: identity.market_id.clone(),
        rules_fingerprint: hex(&identity.rules_fingerprint),
        resolution_source: identity.resolution_source.clone(),
        start_time_ms: identity.start_time_ms,
        end_time_ms: identity.end_time_ms,
        reference_price_micros: reference_price.to_string(),
        target_price_micros: target.to_string(),
        reference_received_at_ms: reference.received_at_ms,
        up_book,
        down_book,
        pair: PairObservation {
            buy_pair_cost_micros: cost.to_string(),
            raw_gap_micros: gap.to_string(),
            executable_quantity_micros: quantity.to_string(),
            observation: observation.to_owned(),
            decision: "no_trade".to_owned(),
            reason: "fees, risk, reservations and execution authority are not projected".to_owned(),
        },
        feed: FeedProjection {
            market_identity: "ready".to_owned(),
            up_book: "ready".to_owned(),
            down_book: "ready".to_owned(),
            reference: "ready".to_owned(),
            age_ms: age,
        },
    })
}

fn normalize_levels(raw: Vec<RawLevel>) -> Result<Vec<(i64, i64)>, ProjectionError> {
    let mut seen = BTreeSet::new();
    let mut levels = Vec::with_capacity(raw.len());
    for level in raw {
        let price = parse_price_micros(&level.price)
            .map_err(|_| ProjectionError::BookValue)?
            .as_micros();
        let size = parse_quantity_micros(&level.size)
            .map_err(|_| ProjectionError::BookValue)?
            .as_micros();
        if price <= 0 || price >= 1_000_000 || size <= 0 || !seen.insert(price) {
            return Err(ProjectionError::BookValue);
        }
        levels.push((price, size));
    }
    Ok(levels)
}

fn project_levels(levels: Vec<(i64, i64)>) -> Vec<LevelProjection> {
    levels
        .into_iter()
        .take(PROJECTED_BOOK_LEVELS)
        .map(|(price, quantity)| LevelProjection {
            price_micros: price.to_string(),
            quantity_micros: quantity.to_string(),
        })
        .collect()
}

fn parse_i64(value: &str) -> Result<i64, ProjectionError> {
    value
        .parse::<i64>()
        .map_err(|_| ProjectionError::Arithmetic)
}

fn parse_book_timestamp(value: &str) -> Result<i64, ProjectionError> {
    value
        .parse::<i64>()
        .or_else(|_| {
            DateTime::parse_from_rfc3339(value).map(|timestamp| timestamp.timestamp_millis())
        })
        .map_err(|_| ProjectionError::BookTimestamp)
}

fn validate_asset_set(assets: &[AssetProjection], now_ms: i64) -> Result<(), ProjectionError> {
    let names: BTreeSet<_> = assets.iter().map(|asset| asset.asset.as_str()).collect();
    let required: BTreeSet<_> = REQUIRED_ASSETS.into_iter().collect();
    if assets.len() != REQUIRED_ASSETS.len()
        || names != required
        || assets.iter().any(|asset| {
            now_ms < asset.start_time_ms
                || now_ms >= asset.end_time_ms
                || asset.pair.decision != "no_trade"
        })
    {
        return Err(ProjectionError::AssetSet);
    }
    Ok(())
}

fn unavailable_snapshot(
    sequence: u64,
    now_ms: i64,
    last_success_at_ms: Option<i64>,
    mode: ProjectionMode,
    reason: &str,
) -> TerminalSnapshot {
    TerminalSnapshot {
        schema_version: SCHEMA_VERSION,
        sequence,
        generated_at_ms: now_ms,
        last_success_at_ms,
        mode,
        no_trade: true,
        reason: reason.to_owned(),
        assets: Vec::new(),
        credentials_present: false,
        authenticated_transport_present: false,
        order_submission_present: false,
        financial_authority_present: false,
        snapshot_digest: String::new(),
    }
    .sealed()
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

#[cfg(test)]
mod tests;
