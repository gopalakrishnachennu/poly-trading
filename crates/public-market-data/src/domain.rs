//! Validated market identity and discovery-window types.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use thiserror::Error;

const ONE_HOUR_MS: i64 = 3_600_000;
const MAX_SHORT_TEXT: usize = 512;
const MAX_RULES_TEXT: usize = 128 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Asset {
    Bitcoin,
    Ethereum,
}

impl Asset {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bitcoin => "BTC",
            Self::Ethereum => "ETH",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HourlySeries {
    pub id: &'static str,
    pub slug: &'static str,
    pub asset: Asset,
}

pub const BTC_HOURLY: HourlySeries = HourlySeries {
    id: "10114",
    slug: "btc-up-or-down-hourly",
    asset: Asset::Bitcoin,
};

pub const ETH_HOURLY: HourlySeries = HourlySeries {
    id: "10117",
    slug: "eth-up-or-down-hourly",
    asset: Asset::Ethereum,
};

pub const HOURLY_SERIES: [HourlySeries; 2] = [BTC_HOURLY, ETH_HOURLY];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiscoveryWindow {
    pub start_ms: i64,
    pub end_ms: i64,
}

impl DiscoveryWindow {
    /// Creates a non-empty UTC millisecond window.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidWindow`] unless `start_ms < end_ms`.
    pub fn new(start_ms: i64, end_ms: i64) -> Result<Self, IdentityError> {
        if start_ms >= end_ms {
            return Err(IdentityError::InvalidWindow { start_ms, end_ms });
        }
        Ok(Self { start_ms, end_ms })
    }

    #[must_use]
    pub const fn overlaps(self, start_ms: i64, end_ms: i64) -> bool {
        start_ms < self.end_ms && end_ms > self.start_ms
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarketIdentity {
    pub asset: Asset,
    pub event_id: String,
    pub market_id: String,
    pub condition_id: String,
    pub question_id: String,
    pub event_slug: String,
    pub market_slug: String,
    pub series_id: String,
    pub series_slug: String,
    pub title: String,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub resolution_source: String,
    pub description: String,
    pub up_token_id: String,
    pub down_token_id: String,
    pub rules_fingerprint: [u8; 32],
}

impl MarketIdentity {
    #[must_use]
    pub fn token_ids(&self) -> [&str; 2] {
        [&self.up_token_id, &self.down_token_id]
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum IdentityError {
    #[error("invalid discovery window: {start_ms}..{end_ms}")]
    InvalidWindow { start_ms: i64, end_ms: i64 },
    #[error("missing field: {0}")]
    Missing(&'static str),
    #[error("field {field} is too large: {length} bytes")]
    FieldTooLarge { field: &'static str, length: usize },
    #[error("invalid RFC3339 timestamp in {field}: {value}")]
    InvalidTimestamp { field: &'static str, value: String },
    #[error("event is not exactly one hour: {start_ms}..{end_ms}")]
    NotHourly { start_ms: i64, end_ms: i64 },
    #[error("invalid 32-byte hexadecimal identifier in {field}: {value}")]
    InvalidHexId { field: &'static str, value: String },
    #[error("invalid decimal token identifier: {0}")]
    InvalidTokenId(String),
    #[error("invalid outcomes JSON: {0}")]
    InvalidOutcomes(String),
    #[error("market must contain exactly one Up and one Down outcome")]
    NotUpDown,
    #[error("market must contain exactly two token identifiers")]
    InvalidTokenCount,
    #[error("series identity mismatch for slug: {0}")]
    SeriesMismatch(String),
}

#[derive(Clone, Debug)]
pub(crate) struct MarketCandidate {
    pub event_id: String,
    pub event_slug: String,
    pub market_id: String,
    pub condition_id: String,
    pub question_id: String,
    pub market_slug: String,
    pub series: Vec<(String, String)>,
    pub series_slug: String,
    pub title: String,
    pub start_time: String,
    pub end_time: String,
    pub resolution_source: String,
    pub description: String,
    pub outcomes_json: String,
    pub token_ids_json: String,
}

pub(crate) fn validate_candidate(
    candidate: MarketCandidate,
    configured: &[HourlySeries],
) -> Result<MarketIdentity, IdentityError> {
    validate_short("event_id", &candidate.event_id)?;
    validate_short("event_slug", &candidate.event_slug)?;
    validate_short("market_id", &candidate.market_id)?;
    validate_short("market_slug", &candidate.market_slug)?;
    validate_short("title", &candidate.title)?;
    validate_rules("description", &candidate.description)?;
    validate_rules("resolution_source", &candidate.resolution_source)?;
    validate_hex_id("condition_id", &candidate.condition_id)?;
    validate_hex_id("question_id", &candidate.question_id)?;

    let series = configured
        .iter()
        .find(|series| series.slug == candidate.series_slug)
        .ok_or_else(|| IdentityError::SeriesMismatch(candidate.series_slug.clone()))?;
    let series_relation_matches = candidate
        .series
        .iter()
        .any(|(id, slug)| id == series.id && slug == series.slug);
    if !series_relation_matches {
        return Err(IdentityError::SeriesMismatch(candidate.series_slug));
    }

    let start_time_ms = parse_time_ms("start_time", &candidate.start_time)?;
    let end_time_ms = parse_time_ms("end_time", &candidate.end_time)?;
    if end_time_ms.checked_sub(start_time_ms) != Some(ONE_HOUR_MS) {
        return Err(IdentityError::NotHourly {
            start_ms: start_time_ms,
            end_ms: end_time_ms,
        });
    }

    let outcomes: Vec<String> = serde_json::from_str(&candidate.outcomes_json)
        .map_err(|error| IdentityError::InvalidOutcomes(error.to_string()))?;
    let token_ids: Vec<String> = serde_json::from_str(&candidate.token_ids_json)
        .map_err(|_| IdentityError::InvalidTokenCount)?;
    if token_ids.len() != 2 {
        return Err(IdentityError::InvalidTokenCount);
    }
    for token in &token_ids {
        validate_token_id(token)?;
    }

    let positions: HashMap<String, usize> = outcomes
        .iter()
        .enumerate()
        .map(|(index, outcome)| (outcome.to_ascii_lowercase(), index))
        .collect();
    if outcomes.len() != 2 || positions.len() != 2 {
        return Err(IdentityError::NotUpDown);
    }
    let up_index = positions
        .get("up")
        .copied()
        .ok_or(IdentityError::NotUpDown)?;
    let down_index = positions
        .get("down")
        .copied()
        .ok_or(IdentityError::NotUpDown)?;

    let rules_fingerprint = fingerprint(&[
        &candidate.condition_id,
        &candidate.question_id,
        &candidate.series_slug,
        &candidate.start_time,
        &candidate.end_time,
        &candidate.resolution_source,
        &candidate.description,
        &candidate.outcomes_json,
        &candidate.token_ids_json,
    ]);

    Ok(MarketIdentity {
        asset: series.asset,
        event_id: candidate.event_id,
        market_id: candidate.market_id,
        condition_id: candidate.condition_id,
        question_id: candidate.question_id,
        event_slug: candidate.event_slug,
        market_slug: candidate.market_slug,
        series_id: series.id.to_owned(),
        series_slug: series.slug.to_owned(),
        title: candidate.title,
        start_time_ms,
        end_time_ms,
        resolution_source: candidate.resolution_source,
        description: candidate.description,
        up_token_id: token_ids[up_index].clone(),
        down_token_id: token_ids[down_index].clone(),
        rules_fingerprint,
    })
}

pub(crate) fn parse_time_ms(field: &'static str, value: &str) -> Result<i64, IdentityError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc).timestamp_millis())
        .map_err(|_| IdentityError::InvalidTimestamp {
            field,
            value: value.to_owned(),
        })
}

fn validate_short(field: &'static str, value: &str) -> Result<(), IdentityError> {
    if value.is_empty() {
        return Err(IdentityError::Missing(field));
    }
    if value.len() > MAX_SHORT_TEXT {
        return Err(IdentityError::FieldTooLarge {
            field,
            length: value.len(),
        });
    }
    Ok(())
}

fn validate_rules(field: &'static str, value: &str) -> Result<(), IdentityError> {
    if value.is_empty() {
        return Err(IdentityError::Missing(field));
    }
    if value.len() > MAX_RULES_TEXT {
        return Err(IdentityError::FieldTooLarge {
            field,
            length: value.len(),
        });
    }
    Ok(())
}

pub(crate) fn validate_hex_id(field: &'static str, value: &str) -> Result<(), IdentityError> {
    let valid = value.len() == 66
        && value.starts_with("0x")
        && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit());
    if valid {
        Ok(())
    } else {
        Err(IdentityError::InvalidHexId {
            field,
            value: value.to_owned(),
        })
    }
}

pub(crate) fn validate_token_id(value: &str) -> Result<(), IdentityError> {
    let valid =
        !value.is_empty() && value.len() <= 78 && value.bytes().all(|byte| byte.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(IdentityError::InvalidTokenId(value.to_owned()))
    }
}

fn fingerprint(fields: &[&str]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for field in fields {
        let length = u64::try_from(field.len()).expect("bounded field length fits u64");
        hasher.update(&length.to_le_bytes());
        hasher.update(field.as_bytes());
    }
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> MarketCandidate {
        MarketCandidate {
            event_id: "event-1".to_owned(),
            event_slug: "bitcoin-up-or-down-july-16-2026-10pm-et".to_owned(),
            market_id: "market-1".to_owned(),
            condition_id: format!("0x{}", "a".repeat(64)),
            question_id: format!("0x{}", "b".repeat(64)),
            market_slug: "bitcoin-up-or-down-july-16-2026-10pm-et".to_owned(),
            series: vec![(BTC_HOURLY.id.to_owned(), BTC_HOURLY.slug.to_owned())],
            series_slug: BTC_HOURLY.slug.to_owned(),
            title: "Bitcoin Up or Down".to_owned(),
            start_time: "2026-07-17T02:00:00Z".to_owned(),
            end_time: "2026-07-17T03:00:00Z".to_owned(),
            resolution_source: "https://example.test/btc".to_owned(),
            description: "Use the final one-hour candle.".to_owned(),
            outcomes_json: "[\"Down\",\"Up\"]".to_owned(),
            token_ids_json: "[\"11\",\"22\"]".to_owned(),
        }
    }

    #[test]
    fn maps_tokens_by_outcome_not_position_assumption() {
        let identity = validate_candidate(candidate(), &HOURLY_SERIES).expect("valid");
        assert_eq!(identity.up_token_id, "22");
        assert_eq!(identity.down_token_id, "11");
        assert_eq!(identity.asset, Asset::Bitcoin);
    }

    #[test]
    fn rejects_series_relation_mismatch() {
        let mut input = candidate();
        input.series = vec![(ETH_HOURLY.id.to_owned(), ETH_HOURLY.slug.to_owned())];
        assert!(matches!(
            validate_candidate(input, &HOURLY_SERIES),
            Err(IdentityError::SeriesMismatch(_))
        ));
    }

    #[test]
    fn rejects_non_hourly_or_non_binary_market() {
        let mut duration = candidate();
        duration.end_time = "2026-07-17T03:00:01Z".to_owned();
        assert!(matches!(
            validate_candidate(duration, &HOURLY_SERIES),
            Err(IdentityError::NotHourly { .. })
        ));

        let mut outcomes = candidate();
        outcomes.outcomes_json = "[\"Yes\",\"No\"]".to_owned();
        assert_eq!(
            validate_candidate(outcomes, &HOURLY_SERIES),
            Err(IdentityError::NotUpDown)
        );
    }

    #[test]
    fn rejects_invalid_identifiers_and_token_counts() {
        let mut condition = candidate();
        condition.condition_id = "0x12".to_owned();
        assert!(matches!(
            validate_candidate(condition, &HOURLY_SERIES),
            Err(IdentityError::InvalidHexId {
                field: "condition_id",
                ..
            })
        ));

        let mut question = candidate();
        question.question_id = format!("0x{}", "g".repeat(64));
        assert!(matches!(
            validate_candidate(question, &HOURLY_SERIES),
            Err(IdentityError::InvalidHexId {
                field: "question_id",
                ..
            })
        ));

        let mut count = candidate();
        count.token_ids_json = r#"["11"]"#.to_owned();
        assert!(matches!(
            validate_candidate(count, &HOURLY_SERIES),
            Err(IdentityError::InvalidTokenCount)
        ));

        let mut token = candidate();
        token.token_ids_json = r#"["11","not-decimal"]"#.to_owned();
        assert!(matches!(
            validate_candidate(token, &HOURLY_SERIES),
            Err(IdentityError::InvalidTokenId(_))
        ));
    }

    #[test]
    fn rejects_invalid_or_missing_rules_data() {
        let mut timestamp = candidate();
        timestamp.start_time = "July 17".to_owned();
        assert!(matches!(
            validate_candidate(timestamp, &HOURLY_SERIES),
            Err(IdentityError::InvalidTimestamp {
                field: "start_time",
                ..
            })
        ));

        let mut rules = candidate();
        rules.description.clear();
        assert!(matches!(
            validate_candidate(rules, &HOURLY_SERIES),
            Err(IdentityError::Missing("description"))
        ));

        let mut source = candidate();
        source.resolution_source.clear();
        assert!(matches!(
            validate_candidate(source, &HOURLY_SERIES),
            Err(IdentityError::Missing("resolution_source"))
        ));
    }

    #[test]
    fn fingerprint_changes_with_rules() {
        let first = validate_candidate(candidate(), &HOURLY_SERIES).expect("first");
        let mut changed = candidate();
        changed.description.push_str(" Corrected.");
        let second = validate_candidate(changed, &HOURLY_SERIES).expect("second");
        assert_ne!(first.rules_fingerprint, second.rules_fingerprint);
    }
}
