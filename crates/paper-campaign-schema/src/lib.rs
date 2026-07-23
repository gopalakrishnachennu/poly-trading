#![forbid(unsafe_code)]

//! Bounded, versioned records for a paper-trading campaign.
//!
//! This crate deliberately contains no strategy or venue client.  It defines
//! the durable evidence contract used by a recorder: every input, decision,
//! simulated execution, ledger projection, health transition and checkpoint
//! can be replayed and tied to a campaign digest.

use event_schema::{EventEnvelope, EventSource};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SCHEMA_VERSION: u16 = 1;
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_ID_BYTES: usize = 512;
pub const MAX_TEXT_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Market,
    Reference,
    Decision,
    Execution,
    Ledger,
    Health,
    Checkpoint,
    Resolution,
    Operator,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookLevel {
    pub price_micros: i64,
    pub quantity_micros: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketRecord {
    pub venue: String,
    pub market_id: String,
    pub asset: String,
    pub sequence: u64,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
    pub target_price_micros: Option<i64>,
    pub source_payload_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceRecord {
    pub venue: String,
    pub symbol: String,
    pub sequence: u64,
    pub price_micros: i64,
    pub bid_micros: Option<i64>,
    pub ask_micros: Option<i64>,
    pub source_payload_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecord {
    pub decision_id: [u8; 32],
    pub market_id: String,
    pub action: String,
    pub reason: String,
    pub no_trade: bool,
    pub expected_edge_micros: i64,
    pub risk_floor_micros: i64,
    pub model_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecord {
    pub order_id: [u8; 32],
    pub market_id: String,
    pub state: String,
    pub side: String,
    pub requested_quantity_micros: i64,
    pub matched_quantity_micros: i64,
    pub price_micros: i64,
    pub fee_micros: i64,
    pub exchange_event_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRecord {
    pub command_id: [u8; 32],
    pub cash_micros: i64,
    pub reserved_micros: i64,
    pub up_inventory_micros: i64,
    pub down_inventory_micros: i64,
    pub realized_pnl_micros: i64,
    pub locked_pnl_micros: i64,
    pub unrealized_pnl_micros: i64,
    pub ledger_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthRecord {
    pub component: String,
    pub state: String,
    pub observed_latency_ns: u64,
    pub age_ns: u64,
    pub input_sequence: Option<u64>,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointRecord {
    pub last_sequence: u64,
    pub state_digest: [u8; 32],
    pub journal_digest: [u8; 32],
    pub record_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionRecord {
    pub market_id: String,
    pub outcome: String,
    pub resolved_at_ns: i64,
    pub rules_digest: [u8; 32],
    pub payout_up_micros: i64,
    pub payout_down_micros: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorRecord {
    pub operator: String,
    pub action: String,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum CampaignRecord {
    Market(MarketRecord),
    Reference(ReferenceRecord),
    Decision(DecisionRecord),
    Execution(ExecutionRecord),
    Ledger(LedgerRecord),
    Health(HealthRecord),
    Checkpoint(CheckpointRecord),
    Resolution(ResolutionRecord),
    Operator(OperatorRecord),
}

impl CampaignRecord {
    #[must_use]
    pub const fn kind(&self) -> RecordKind {
        match self {
            Self::Market(_) => RecordKind::Market,
            Self::Reference(_) => RecordKind::Reference,
            Self::Decision(_) => RecordKind::Decision,
            Self::Execution(_) => RecordKind::Execution,
            Self::Ledger(_) => RecordKind::Ledger,
            Self::Health(_) => RecordKind::Health,
            Self::Checkpoint(_) => RecordKind::Checkpoint,
            Self::Resolution(_) => RecordKind::Resolution,
            Self::Operator(_) => RecordKind::Operator,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordEnvelope {
    pub schema_version: u16,
    pub campaign_id: String,
    pub stream: String,
    pub sequence: u64,
    pub event_time_ns: i64,
    pub recorded_time_ns: i64,
    pub record: CampaignRecord,
    pub record_digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RecordError {
    #[error("identifier or text field exceeds its bound")]
    FieldTooLarge,
    #[error("timestamps must be non-negative and recorded time cannot precede event time")]
    Timestamp,
    #[error("numeric value is outside the non-negative bounded range")]
    Numeric,
    #[error("record JSON exceeds the bounded size")]
    TooLarge,
    #[error("record JSON is invalid: {0}")]
    Json(String),
    #[error("record schema version is unsupported: {0}")]
    Version(u16),
    #[error("record digest does not match payload")]
    Digest,
    #[error("event envelope conversion failed: {0}")]
    Envelope(String),
}

impl RecordEnvelope {
    /// Creates a bounded, digest-sealed campaign record.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid identifiers, timestamps, record fields or
    /// serialization failures.
    pub fn new(
        campaign_id: String,
        stream: String,
        sequence: u64,
        event_time_ns: i64,
        recorded_time_ns: i64,
        record: CampaignRecord,
    ) -> Result<Self, RecordError> {
        if campaign_id.is_empty()
            || campaign_id.len() > MAX_ID_BYTES
            || stream.is_empty()
            || stream.len() > MAX_ID_BYTES
        {
            return Err(RecordError::FieldTooLarge);
        }
        if event_time_ns < 0 || recorded_time_ns < event_time_ns {
            return Err(RecordError::Timestamp);
        }
        validate_record(&record)?;
        let mut envelope = Self {
            schema_version: SCHEMA_VERSION,
            campaign_id,
            stream,
            sequence,
            event_time_ns,
            recorded_time_ns,
            record,
            record_digest: [0; 32],
        };
        envelope.record_digest = envelope.payload_digest()?;
        Ok(envelope)
    }

    fn payload_digest(&self) -> Result<[u8; 32], RecordError> {
        let bytes =
            serde_json::to_vec(&self.record).map_err(|e| RecordError::Json(e.to_string()))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(RecordError::TooLarge);
        }
        Ok(*blake3::hash(&bytes).as_bytes())
    }

    /// Encodes this envelope after revalidating its version and payload digest.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported versions, tampering, oversized records
    /// or serialization failures.
    pub fn encode(&self) -> Result<Vec<u8>, RecordError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(RecordError::Version(self.schema_version));
        }
        if self.payload_digest()? != self.record_digest {
            return Err(RecordError::Digest);
        }
        let bytes = serde_json::to_vec(self).map_err(|e| RecordError::Json(e.to_string()))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(RecordError::TooLarge);
        }
        Ok(bytes)
    }

    /// Decodes and validates a bounded, digest-sealed envelope.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, oversized, tampered or invalid records.
    pub fn decode(bytes: &[u8]) -> Result<Self, RecordError> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(RecordError::TooLarge);
        }
        let value: Self =
            serde_json::from_slice(bytes).map_err(|e| RecordError::Json(e.to_string()))?;
        if value.schema_version != SCHEMA_VERSION {
            return Err(RecordError::Version(value.schema_version));
        }
        if value.payload_digest()? != value.record_digest {
            return Err(RecordError::Digest);
        }
        if value.event_time_ns < 0 || value.recorded_time_ns < value.event_time_ns {
            return Err(RecordError::Timestamp);
        }
        if value.campaign_id.is_empty()
            || value.campaign_id.len() > MAX_ID_BYTES
            || value.stream.is_empty()
            || value.stream.len() > MAX_ID_BYTES
        {
            return Err(RecordError::FieldTooLarge);
        }
        validate_record(&value.record)?;
        Ok(value)
    }

    /// Converts this paper-campaign record into the canonical event envelope.
    ///
    /// # Errors
    ///
    /// Returns an error when encoding or canonical event construction fails.
    pub fn to_event_envelope(&self) -> Result<EventEnvelope, RecordError> {
        EventEnvelope::new(
            EventSource::System,
            self.sequence,
            self.event_time_ns,
            self.recorded_time_ns,
            self.campaign_id.clone(),
            self.encode()?,
        )
        .map_err(|e| RecordError::Envelope(e.to_string()))
    }
}

fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT_BYTES
}

fn non_negative(value: i64) -> bool {
    value >= 0
}

fn validate_record(record: &CampaignRecord) -> Result<(), RecordError> {
    let valid = match record {
        CampaignRecord::Market(value) => {
            bounded(&value.venue)
                && bounded(&value.market_id)
                && bounded(&value.asset)
                && value.bids.iter().chain(value.asks.iter()).all(|level| {
                    non_negative(level.price_micros) && non_negative(level.quantity_micros)
                })
                && value.target_price_micros.is_none_or(non_negative)
        }
        CampaignRecord::Reference(value) => {
            bounded(&value.venue)
                && bounded(&value.symbol)
                && non_negative(value.price_micros)
                && value.bid_micros.is_none_or(non_negative)
                && value.ask_micros.is_none_or(non_negative)
        }
        CampaignRecord::Decision(value) => {
            bounded(&value.market_id) && bounded(&value.action) && bounded(&value.reason)
        }
        CampaignRecord::Execution(value) => {
            bounded(&value.market_id)
                && bounded(&value.state)
                && bounded(&value.side)
                && non_negative(value.requested_quantity_micros)
                && non_negative(value.matched_quantity_micros)
                && non_negative(value.price_micros)
                && non_negative(value.fee_micros)
        }
        CampaignRecord::Ledger(value) => [
            value.cash_micros,
            value.reserved_micros,
            value.up_inventory_micros,
            value.down_inventory_micros,
        ]
        .iter()
        .all(|value| non_negative(*value)),
        CampaignRecord::Health(value) => {
            bounded(&value.component) && bounded(&value.state) && bounded(&value.detail)
        }
        CampaignRecord::Checkpoint(_) => true,
        CampaignRecord::Resolution(value) => {
            bounded(&value.market_id)
                && bounded(&value.outcome)
                && value.resolved_at_ns >= 0
                && non_negative(value.payout_up_micros)
                && non_negative(value.payout_down_micros)
        }
        CampaignRecord::Operator(value) => {
            bounded(&value.operator) && bounded(&value.action) && bounded(&value.detail)
        }
    };
    if valid {
        Ok(())
    } else {
        Err(RecordError::Numeric)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_and_digest() {
        let record = CampaignRecord::Health(HealthRecord {
            component: "feed".into(),
            state: "healthy".into(),
            observed_latency_ns: 10,
            age_ns: 20,
            input_sequence: Some(4),
            detail: "ok".into(),
        });
        let envelope =
            RecordEnvelope::new("week-1".into(), "health".into(), 4, 100, 110, record).unwrap();
        let bytes = envelope.encode().unwrap();
        assert_eq!(RecordEnvelope::decode(&bytes).unwrap(), envelope);
        assert_eq!(envelope.to_event_envelope().unwrap().sequence, 4);
    }
    #[test]
    fn rejects_tampering_and_clock_regression() {
        let record = CampaignRecord::Operator(OperatorRecord {
            operator: "sim".into(),
            action: "start".into(),
            detail: "paper".into(),
        });
        assert_eq!(
            RecordEnvelope::new("x".into(), "audit".into(), 0, 2, 1, record).unwrap_err(),
            RecordError::Timestamp
        );
        let mut envelope = RecordEnvelope::new(
            "x".into(),
            "audit".into(),
            0,
            2,
            2,
            CampaignRecord::Operator(OperatorRecord {
                operator: "sim".into(),
                action: "start".into(),
                detail: "paper".into(),
            }),
        )
        .unwrap();
        envelope.record_digest = [9; 32];
        assert_eq!(envelope.encode().unwrap_err(), RecordError::Digest);
    }
}
