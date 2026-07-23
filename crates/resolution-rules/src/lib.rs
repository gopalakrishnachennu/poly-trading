#![forbid(unsafe_code)]

//! Deterministic binding between immutable Polymarket hourly rules and the
//! exact finalized Binance candle used by those rules.
//!
//! This crate computes resolution evidence; it does not assert that Polymarket
//! has proposed, confirmed, or paid a resolution. On-chain resolution remains
//! a separate future source of truth.

use common_types::QuotePriceMicros;
use public_market_data::{Asset, MarketIdentity, BTC_HOURLY, ETH_HOURLY};
use reference_market_data::{
    CandleData, CandleInterval, FinalizedCandle, InProgressCandle, ReferenceSymbol,
};
use std::error::Error;
use std::fmt::{Display, Formatter};

const ONE_HOUR_MS: i64 = 3_600_000;
const EVIDENCE_MAGIC: &[u8; 8] = b"POLYRES1";
const EVIDENCE_VERSION: u16 = 1;
const EVIDENCE_FIXED_BYTES: usize = 96;
const CHECKSUM_BYTES: usize = 32;
const MAX_ID_BYTES: usize = 4 * 1024;

const BTC_SOURCE: &str = "https://www.binance.com/en/trade/BTC_USDT";
const ETH_SOURCE: &str = "https://www.binance.com/en/trade/ETH_USDT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseComparator {
    GreaterThanOrEqual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Outcome {
    Up = 1,
    Down = 2,
}

impl Outcome {
    fn from_byte(value: u8) -> Result<Self, ResolutionError> {
        match value {
            1 => Ok(Self::Up),
            2 => Ok(Self::Down),
            _ => Err(ResolutionError::InvalidEvidence("outcome")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolutionContract {
    pub condition_id: String,
    pub question_id: String,
    pub up_token_id: String,
    pub down_token_id: String,
    pub rules_fingerprint: [u8; 32],
    pub symbol: ReferenceSymbol,
    pub candle_open_time_ms: i64,
    pub candle_close_time_ms: i64,
    pub comparator: CloseComparator,
}

impl ResolutionContract {
    /// Binds one validated market identity to its exact oracle contract.
    ///
    /// # Errors
    ///
    /// Returns [`ResolutionError`] if series, asset, source URL, rule language,
    /// UTC candle alignment, or identity fields are inconsistent.
    pub fn bind(market: &MarketIdentity) -> Result<Self, ResolutionError> {
        let (symbol, expected_series_id, expected_series_slug, expected_source) = match market.asset
        {
            Asset::Bitcoin => (
                ReferenceSymbol::BtcUsdt,
                BTC_HOURLY.id,
                BTC_HOURLY.slug,
                BTC_SOURCE,
            ),
            Asset::Ethereum => (
                ReferenceSymbol::EthUsdt,
                ETH_HOURLY.id,
                ETH_HOURLY.slug,
                ETH_SOURCE,
            ),
        };
        if market.series_id != expected_series_id || market.series_slug != expected_series_slug {
            return Err(ResolutionError::SeriesMismatch);
        }
        if market.resolution_source != expected_source {
            return Err(ResolutionError::ResolutionSourceMismatch);
        }
        validate_rule_text(&market.description, symbol)?;
        if market.start_time_ms < 0
            || market.start_time_ms % ONE_HOUR_MS != 0
            || market.end_time_ms.checked_sub(market.start_time_ms) != Some(ONE_HOUR_MS)
        {
            return Err(ResolutionError::CandleWindowMismatch);
        }
        if market.condition_id.is_empty()
            || market.question_id.is_empty()
            || market.up_token_id.is_empty()
            || market.down_token_id.is_empty()
            || market.up_token_id == market.down_token_id
        {
            return Err(ResolutionError::IdentityMismatch);
        }
        let candle_close_time_ms = market
            .end_time_ms
            .checked_sub(1)
            .ok_or(ResolutionError::CandleWindowMismatch)?;
        Ok(Self {
            condition_id: market.condition_id.clone(),
            question_id: market.question_id.clone(),
            up_token_id: market.up_token_id.clone(),
            down_token_id: market.down_token_id.clone(),
            rules_fingerprint: market.rules_fingerprint,
            symbol,
            candle_open_time_ms: market.start_time_ms,
            candle_close_time_ms,
            comparator: CloseComparator::GreaterThanOrEqual,
        })
    }

    fn validate_candle(&self, candle: CandleData) -> Result<(), ResolutionError> {
        if candle.symbol != self.symbol
            || candle.interval != CandleInterval::OneHourUtc
            || candle.open_time_ms != self.candle_open_time_ms
            || candle.close_time_ms != self.candle_close_time_ms
        {
            return Err(ResolutionError::CandleWindowMismatch);
        }
        Ok(())
    }

    #[must_use]
    pub fn outcome(&self, open: QuotePriceMicros, close: QuotePriceMicros) -> Outcome {
        match self.comparator {
            CloseComparator::GreaterThanOrEqual if close >= open => Outcome::Up,
            CloseComparator::GreaterThanOrEqual => Outcome::Down,
        }
    }

    #[must_use]
    pub fn winning_token(&self, outcome: Outcome) -> &str {
        match outcome {
            Outcome::Up => &self.up_token_id,
            Outcome::Down => &self.down_token_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndicativeAssessment {
    pub outcome_if_closed_now: Outcome,
    pub open: QuotePriceMicros,
    pub current_close: QuotePriceMicros,
    pub candle_open_time_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolutionEvidence {
    pub condition_id: String,
    pub question_id: String,
    pub winning_token_id: String,
    pub rules_fingerprint: [u8; 32],
    pub symbol: ReferenceSymbol,
    pub outcome: Outcome,
    pub candle_open_time_ms: i64,
    pub candle_close_time_ms: i64,
    pub open: QuotePriceMicros,
    pub close: QuotePriceMicros,
}

impl ResolutionEvidence {
    /// Encodes immutable oracle evidence with an explicit schema and BLAKE3
    /// checksum. The encoding never uses Rust memory layout.
    ///
    /// # Errors
    ///
    /// Returns [`ResolutionError`] if a bounded identifier is too large.
    pub fn encode(&self) -> Result<Vec<u8>, ResolutionError> {
        self.validate_outcome()?;
        validate_id_lengths(
            &self.condition_id,
            &self.question_id,
            &self.winning_token_id,
        )?;
        let condition_len = u16::try_from(self.condition_id.len())
            .map_err(|_| ResolutionError::EvidenceTooLarge)?;
        let token_len = u16::try_from(self.winning_token_id.len())
            .map_err(|_| ResolutionError::EvidenceTooLarge)?;
        let question_len =
            u16::try_from(self.question_id.len()).map_err(|_| ResolutionError::EvidenceTooLarge)?;
        let body_len = EVIDENCE_FIXED_BYTES
            .checked_add(self.condition_id.len())
            .and_then(|value| value.checked_add(self.winning_token_id.len()))
            .and_then(|value| value.checked_add(self.question_id.len()))
            .ok_or(ResolutionError::EvidenceTooLarge)?;
        let mut bytes = Vec::with_capacity(body_len + CHECKSUM_BYTES);
        bytes.extend_from_slice(EVIDENCE_MAGIC);
        bytes.extend_from_slice(&EVIDENCE_VERSION.to_le_bytes());
        bytes.push(self.symbol as u8);
        bytes.push(self.outcome as u8);
        bytes.extend_from_slice(&self.candle_open_time_ms.to_le_bytes());
        bytes.extend_from_slice(&self.candle_close_time_ms.to_le_bytes());
        bytes.extend_from_slice(&self.open.as_micros().to_le_bytes());
        bytes.extend_from_slice(&self.close.as_micros().to_le_bytes());
        bytes.extend_from_slice(&condition_len.to_le_bytes());
        bytes.extend_from_slice(&token_len.to_le_bytes());
        bytes.extend_from_slice(&question_len.to_le_bytes());
        bytes.extend_from_slice(&[0_u8; 14]);
        bytes.extend_from_slice(&self.rules_fingerprint);
        bytes.extend_from_slice(self.condition_id.as_bytes());
        bytes.extend_from_slice(self.winning_token_id.as_bytes());
        bytes.extend_from_slice(self.question_id.as_bytes());
        let checksum = blake3::hash(&bytes);
        bytes.extend_from_slice(checksum.as_bytes());
        Ok(bytes)
    }

    /// Decodes and verifies one exact oracle-evidence record.
    ///
    /// # Errors
    ///
    /// Returns [`ResolutionError`] for truncation, checksum, version, reserved
    /// bytes, identifiers, fixed-point values, or trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, ResolutionError> {
        if bytes.len() < EVIDENCE_FIXED_BYTES + CHECKSUM_BYTES {
            return Err(ResolutionError::InvalidEvidence("truncated"));
        }
        let checksum_offset = bytes
            .len()
            .checked_sub(CHECKSUM_BYTES)
            .ok_or(ResolutionError::InvalidEvidence("truncated"))?;
        let (body, checksum) = bytes.split_at(checksum_offset);
        if blake3::hash(body).as_bytes() != checksum {
            return Err(ResolutionError::EvidenceChecksum);
        }
        if body.get(0..8) != Some(EVIDENCE_MAGIC) {
            return Err(ResolutionError::InvalidEvidence("magic"));
        }
        if read_u16(body, 8)? != EVIDENCE_VERSION {
            return Err(ResolutionError::InvalidEvidence("version"));
        }
        let symbol = match *body
            .get(10)
            .ok_or(ResolutionError::InvalidEvidence("symbol"))?
        {
            1 => ReferenceSymbol::BtcUsdt,
            2 => ReferenceSymbol::EthUsdt,
            _ => return Err(ResolutionError::InvalidEvidence("symbol")),
        };
        let outcome = Outcome::from_byte(
            *body
                .get(11)
                .ok_or(ResolutionError::InvalidEvidence("outcome"))?,
        )?;
        if body.get(50..64) != Some(&[0_u8; 14]) {
            return Err(ResolutionError::InvalidEvidence("reserved bytes"));
        }
        let condition_len = usize::from(read_u16(body, 44)?);
        let token_len = usize::from(read_u16(body, 46)?);
        let question_len = usize::from(read_u16(body, 48)?);
        let condition_end = EVIDENCE_FIXED_BYTES
            .checked_add(condition_len)
            .ok_or(ResolutionError::EvidenceTooLarge)?;
        let token_end = condition_end
            .checked_add(token_len)
            .ok_or(ResolutionError::EvidenceTooLarge)?;
        let question_end = token_end
            .checked_add(question_len)
            .ok_or(ResolutionError::EvidenceTooLarge)?;
        if question_end != body.len() {
            return Err(ResolutionError::InvalidEvidence("length"));
        }
        let condition_id = decode_string(body, EVIDENCE_FIXED_BYTES, condition_end)?;
        let winning_token_id = decode_string(body, condition_end, token_end)?;
        let question_id = decode_string(body, token_end, question_end)?;
        validate_id_lengths(&condition_id, &question_id, &winning_token_id)?;
        let evidence = Self {
            condition_id,
            question_id,
            winning_token_id,
            rules_fingerprint: body
                .get(64..96)
                .ok_or(ResolutionError::InvalidEvidence("rules fingerprint"))?
                .try_into()
                .map_err(|_| ResolutionError::InvalidEvidence("rules fingerprint"))?,
            symbol,
            outcome,
            candle_open_time_ms: read_i64(body, 12)?,
            candle_close_time_ms: read_i64(body, 20)?,
            open: QuotePriceMicros::new(read_i64(body, 28)?)
                .map_err(|_| ResolutionError::InvalidEvidence("open price"))?,
            close: QuotePriceMicros::new(read_i64(body, 36)?)
                .map_err(|_| ResolutionError::InvalidEvidence("close price"))?,
        };
        if evidence.candle_close_time_ms <= evidence.candle_open_time_ms {
            return Err(ResolutionError::InvalidEvidence("candle window"));
        }
        evidence.validate_outcome()?;
        Ok(evidence)
    }

    /// Verifies that decoded evidence belongs to one immutable contract.
    ///
    /// # Errors
    ///
    /// Returns [`ResolutionError::EvidenceContractMismatch`] when any market,
    /// rule, oracle, window, outcome, or winning-token field disagrees.
    pub fn validate_against(&self, contract: &ResolutionContract) -> Result<(), ResolutionError> {
        self.validate_outcome()?;
        if self.condition_id != contract.condition_id
            || self.question_id != contract.question_id
            || self.rules_fingerprint != contract.rules_fingerprint
            || self.symbol != contract.symbol
            || self.candle_open_time_ms != contract.candle_open_time_ms
            || self.candle_close_time_ms != contract.candle_close_time_ms
            || self.outcome != contract.outcome(self.open, self.close)
            || self.winning_token_id != contract.winning_token(self.outcome)
        {
            return Err(ResolutionError::EvidenceContractMismatch);
        }
        Ok(())
    }

    fn validate_outcome(&self) -> Result<(), ResolutionError> {
        let expected = if self.close >= self.open {
            Outcome::Up
        } else {
            Outcome::Down
        };
        if self.outcome == expected {
            Ok(())
        } else {
            Err(ResolutionError::InvalidEvidence("outcome comparator"))
        }
    }

    /// Returns a stable digest of the explicit evidence encoding.
    ///
    /// # Errors
    ///
    /// Returns [`ResolutionError`] when bounded identifiers cannot be encoded.
    pub fn digest(&self) -> Result<[u8; 32], ResolutionError> {
        Ok(*blake3::hash(&self.encode()?).as_bytes())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleState {
    contract: ResolutionContract,
    final_evidence: Option<ResolutionEvidence>,
}

impl OracleState {
    #[must_use]
    pub const fn new(contract: ResolutionContract) -> Self {
        Self {
            contract,
            final_evidence: None,
        }
    }

    #[must_use]
    pub const fn contract(&self) -> &ResolutionContract {
        &self.contract
    }

    #[must_use]
    pub const fn final_evidence(&self) -> Option<&ResolutionEvidence> {
        self.final_evidence.as_ref()
    }

    /// Computes a clearly non-final, close-if-now indication.
    ///
    /// # Errors
    ///
    /// Returns [`ResolutionError`] when the candle is not the contract's exact
    /// symbol and time window.
    pub fn assess(
        &self,
        candle: InProgressCandle,
    ) -> Result<IndicativeAssessment, ResolutionError> {
        self.contract.validate_candle(candle.0)?;
        Ok(IndicativeAssessment {
            outcome_if_closed_now: self.contract.outcome(candle.0.open, candle.0.close),
            open: candle.0.open,
            current_close: candle.0.close,
            candle_open_time_ms: candle.0.open_time_ms,
        })
    }

    /// Produces immutable evidence from the exact finalized candle.
    ///
    /// Reapplying identical evidence is idempotent. A conflicting finalized
    /// candle fails without changing the existing evidence.
    ///
    /// # Errors
    ///
    /// Returns [`ResolutionError`] for a candle mismatch or conflicting final
    /// evidence.
    pub fn finalize(
        &mut self,
        candle: FinalizedCandle,
    ) -> Result<&ResolutionEvidence, ResolutionError> {
        self.contract.validate_candle(candle.0)?;
        let outcome = self.contract.outcome(candle.0.open, candle.0.close);
        let evidence = ResolutionEvidence {
            condition_id: self.contract.condition_id.clone(),
            question_id: self.contract.question_id.clone(),
            winning_token_id: self.contract.winning_token(outcome).to_owned(),
            rules_fingerprint: self.contract.rules_fingerprint,
            symbol: self.contract.symbol,
            outcome,
            candle_open_time_ms: candle.0.open_time_ms,
            candle_close_time_ms: candle.0.close_time_ms,
            open: candle.0.open,
            close: candle.0.close,
        };
        if self.final_evidence.is_some() {
            if self.final_evidence.as_ref() != Some(&evidence) {
                return Err(ResolutionError::ConflictingFinalEvidence);
            }
            return self
                .final_evidence
                .as_ref()
                .ok_or(ResolutionError::InvalidEvidence("missing final evidence"));
        }
        self.final_evidence = Some(evidence);
        self.final_evidence
            .as_ref()
            .ok_or(ResolutionError::InvalidEvidence("missing final evidence"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolutionError {
    SeriesMismatch,
    ResolutionSourceMismatch,
    UnsupportedRuleLanguage,
    CandleWindowMismatch,
    IdentityMismatch,
    ConflictingFinalEvidence,
    EvidenceTooLarge,
    EvidenceChecksum,
    EvidenceContractMismatch,
    InvalidEvidence(&'static str),
}

impl Display for ResolutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SeriesMismatch => formatter.write_str("hourly series and asset do not match"),
            Self::ResolutionSourceMismatch => {
                formatter.write_str("resolution source is not the exact Binance spot pair")
            }
            Self::UnsupportedRuleLanguage => {
                formatter.write_str("hourly resolution language is unsupported or changed")
            }
            Self::CandleWindowMismatch => {
                formatter.write_str("candle symbol or time window does not match market")
            }
            Self::IdentityMismatch => formatter.write_str("market identity is inconsistent"),
            Self::ConflictingFinalEvidence => {
                formatter.write_str("finalized oracle evidence conflicts with prior evidence")
            }
            Self::EvidenceTooLarge => formatter.write_str("resolution evidence is too large"),
            Self::EvidenceChecksum => formatter.write_str("resolution evidence checksum mismatch"),
            Self::EvidenceContractMismatch => {
                formatter.write_str("resolution evidence does not match immutable contract")
            }
            Self::InvalidEvidence(field) => {
                write!(formatter, "invalid resolution evidence: {field}")
            }
        }
    }
}

impl Error for ResolutionError {}

fn validate_rule_text(text: &str, symbol: ReferenceSymbol) -> Result<(), ResolutionError> {
    let normalized = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let pair = match symbol {
        ReferenceSymbol::BtcUsdt => "btc/usdt 1 hour candle",
        ReferenceSymbol::EthUsdt => "eth/usdt 1 hour candle",
    };
    let required = [
        "close price is greater than or equal to the open price",
        "otherwise, this market will resolve to \"down\"",
        "once the data for that candle is finalized",
        "not according to other exchanges or trading pairs",
        pair,
    ];
    if required.iter().all(|clause| normalized.contains(clause)) {
        Ok(())
    } else {
        Err(ResolutionError::UnsupportedRuleLanguage)
    }
}

fn validate_id_lengths(
    condition: &str,
    question: &str,
    token: &str,
) -> Result<(), ResolutionError> {
    if condition.is_empty()
        || question.is_empty()
        || token.is_empty()
        || condition.len() > MAX_ID_BYTES
        || question.len() > MAX_ID_BYTES
        || token.len() > MAX_ID_BYTES
    {
        Err(ResolutionError::EvidenceTooLarge)
    } else {
        Ok(())
    }
}

fn decode_string(bytes: &[u8], start: usize, end: usize) -> Result<String, ResolutionError> {
    std::str::from_utf8(
        bytes
            .get(start..end)
            .ok_or(ResolutionError::InvalidEvidence("string bounds"))?,
    )
    .map(str::to_owned)
    .map_err(|_| ResolutionError::InvalidEvidence("string UTF-8"))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ResolutionError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(ResolutionError::InvalidEvidence("truncated"))?
            .try_into()
            .map_err(|_| ResolutionError::InvalidEvidence("truncated"))?,
    ))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, ResolutionError> {
    Ok(i64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(ResolutionError::InvalidEvidence("truncated"))?
            .try_into()
            .map_err(|_| ResolutionError::InvalidEvidence("truncated"))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_types::ReferenceQuantityE8;

    fn description(pair: &str) -> String {
        format!(
            "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the {pair} 1 hour candle that begins on the time and date specified in the title. Otherwise, this market will resolve to \"Down\". The close and open displayed for the relevant candle will be used once the data for that candle is finalized. This market is about the Binance pair, not according to other exchanges or trading pairs."
        )
    }

    fn market(asset: Asset) -> MarketIdentity {
        let (series, source, pair) = match asset {
            Asset::Bitcoin => (BTC_HOURLY, BTC_SOURCE, "BTC/USDT"),
            Asset::Ethereum => (ETH_HOURLY, ETH_SOURCE, "ETH/USDT"),
        };
        MarketIdentity {
            asset,
            event_id: "event".to_owned(),
            market_id: "market".to_owned(),
            condition_id: format!("0x{}", "a".repeat(64)),
            question_id: format!("0x{}", "b".repeat(64)),
            event_slug: "event".to_owned(),
            market_slug: "market".to_owned(),
            series_id: series.id.to_owned(),
            series_slug: series.slug.to_owned(),
            title: "Up or Down".to_owned(),
            start_time_ms: 3_600_000,
            end_time_ms: 7_200_000,
            resolution_source: source.to_owned(),
            description: description(pair),
            up_token_id: "11".to_owned(),
            down_token_id: "22".to_owned(),
            rules_fingerprint: [7; 32],
        }
    }

    fn candle(symbol: ReferenceSymbol, close: i64) -> CandleData {
        CandleData {
            symbol,
            interval: CandleInterval::OneHourUtc,
            open_time_ms: 3_600_000,
            close_time_ms: 7_199_999,
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

    #[test]
    fn binds_exact_btc_and_eth_rules() {
        let btc = ResolutionContract::bind(&market(Asset::Bitcoin)).expect("BTC contract");
        assert_eq!(btc.symbol, ReferenceSymbol::BtcUsdt);
        let eth = ResolutionContract::bind(&market(Asset::Ethereum)).expect("ETH contract");
        assert_eq!(eth.symbol, ReferenceSymbol::EthUsdt);
    }

    #[test]
    fn equality_resolves_up_and_finalized_only_creates_evidence() {
        let contract = ResolutionContract::bind(&market(Asset::Bitcoin)).expect("contract");
        let mut state = OracleState::new(contract);
        let indicative = state
            .assess(InProgressCandle(candle(
                ReferenceSymbol::BtcUsdt,
                99_000_000,
            )))
            .expect("assessment");
        assert_eq!(indicative.outcome_if_closed_now, Outcome::Down);
        assert!(state.final_evidence().is_none());
        let final_evidence = state
            .finalize(FinalizedCandle(candle(
                ReferenceSymbol::BtcUsdt,
                100_000_000,
            )))
            .expect("finalize");
        assert_eq!(final_evidence.outcome, Outcome::Up);
        assert_eq!(final_evidence.winning_token_id, "11");
    }

    #[test]
    fn rejects_source_rule_symbol_and_window_mismatches() {
        let mut wrong_source = market(Asset::Bitcoin);
        wrong_source.resolution_source = ETH_SOURCE.to_owned();
        assert_eq!(
            ResolutionContract::bind(&wrong_source),
            Err(ResolutionError::ResolutionSourceMismatch)
        );
        let mut changed_rule = market(Asset::Bitcoin);
        changed_rule.description = "close greater than open".to_owned();
        assert_eq!(
            ResolutionContract::bind(&changed_rule),
            Err(ResolutionError::UnsupportedRuleLanguage)
        );
        let contract = ResolutionContract::bind(&market(Asset::Bitcoin)).expect("contract");
        let state = OracleState::new(contract);
        assert_eq!(
            state.assess(InProgressCandle(candle(
                ReferenceSymbol::EthUsdt,
                100_000_000
            ))),
            Err(ResolutionError::CandleWindowMismatch)
        );
    }

    #[test]
    fn finalization_is_idempotent_and_conflicts_are_transactional() {
        let contract = ResolutionContract::bind(&market(Asset::Bitcoin)).expect("contract");
        let mut state = OracleState::new(contract);
        let first = FinalizedCandle(candle(ReferenceSymbol::BtcUsdt, 101_000_000));
        state.finalize(first).expect("first");
        let before = state.clone();
        state.finalize(first).expect("idempotent");
        assert_eq!(state, before);
        assert_eq!(
            state.finalize(FinalizedCandle(candle(
                ReferenceSymbol::BtcUsdt,
                99_000_000
            ))),
            Err(ResolutionError::ConflictingFinalEvidence)
        );
        assert_eq!(state, before);
    }

    #[test]
    fn evidence_round_trip_checksum_and_digest_are_strict() {
        let contract = ResolutionContract::bind(&market(Asset::Bitcoin)).expect("contract");
        let mut state = OracleState::new(contract);
        let evidence = state
            .finalize(FinalizedCandle(candle(
                ReferenceSymbol::BtcUsdt,
                101_000_000,
            )))
            .expect("evidence")
            .clone();
        let encoded = evidence.encode().expect("encode");
        let decoded = ResolutionEvidence::decode(&encoded).expect("decode");
        assert_eq!(decoded, evidence);
        decoded
            .validate_against(state.contract())
            .expect("contract validation");
        assert_eq!(
            evidence.digest().expect("digest"),
            evidence.digest().expect("digest")
        );
        let mut corrupted = encoded;
        corrupted[30] ^= 1;
        assert_eq!(
            ResolutionEvidence::decode(&corrupted),
            Err(ResolutionError::EvidenceChecksum)
        );

        let mut wrong_contract = state.contract().clone();
        wrong_contract.rules_fingerprint = [8; 32];
        assert_eq!(
            evidence.validate_against(&wrong_contract),
            Err(ResolutionError::EvidenceContractMismatch)
        );

        let mut inconsistent = evidence;
        inconsistent.outcome = Outcome::Down;
        assert_eq!(
            inconsistent.encode(),
            Err(ResolutionError::InvalidEvidence("outcome comparator"))
        );
    }
}
