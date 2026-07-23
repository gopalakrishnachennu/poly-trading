#![forbid(unsafe_code)]

//! Versioned deterministic event envelopes.

use std::error::Error;
use std::fmt::{Display, Formatter};

pub const CURRENT_SCHEMA_VERSION: u16 = 1;
pub const MAX_MARKET_ID_BYTES: usize = 4 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
const FIXED_HEADER_BYTES: usize = 36;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum EventSource {
    Market = 1,
    User = 2,
    ReferencePrice = 3,
    Blockchain = 4,
    System = 5,
}

impl TryFrom<u8> for EventSource {
    type Error = SchemaError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Market),
            2 => Ok(Self::User),
            3 => Ok(Self::ReferencePrice),
            4 => Ok(Self::Blockchain),
            5 => Ok(Self::System),
            _ => Err(SchemaError::UnknownSource(value)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventEnvelope {
    pub schema_version: u16,
    pub source: EventSource,
    pub sequence: u64,
    pub event_time_ns: i64,
    pub received_time_ns: i64,
    pub market_id: String,
    pub payload: Vec<u8>,
}

impl EventEnvelope {
    /// Creates a validated envelope using the current schema version.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError`] when the market identifier is empty or a bounded
    /// field is too large.
    pub fn new(
        source: EventSource,
        sequence: u64,
        event_time_ns: i64,
        received_time_ns: i64,
        market_id: String,
        payload: Vec<u8>,
    ) -> Result<Self, SchemaError> {
        validate_lengths(market_id.len(), payload.len())?;
        if market_id.is_empty() {
            return Err(SchemaError::EmptyMarketId);
        }
        Ok(Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            source,
            sequence,
            event_time_ns,
            received_time_ns,
            market_id,
            payload,
        })
    }

    /// Encodes the envelope using the explicit current wire schema.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError`] for unsupported versions, invalid identifiers,
    /// or bounded-length violations.
    pub fn encode(&self) -> Result<Vec<u8>, SchemaError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(SchemaError::UnsupportedVersion(self.schema_version));
        }
        validate_lengths(self.market_id.len(), self.payload.len())?;
        if self.market_id.is_empty() {
            return Err(SchemaError::EmptyMarketId);
        }

        let capacity = FIXED_HEADER_BYTES
            .checked_add(self.market_id.len())
            .and_then(|value| value.checked_add(self.payload.len()))
            .ok_or(SchemaError::LengthOverflow)?;
        let market_len =
            u32::try_from(self.market_id.len()).map_err(|_| SchemaError::LengthOverflow)?;
        let payload_len =
            u32::try_from(self.payload.len()).map_err(|_| SchemaError::LengthOverflow)?;

        let mut bytes = Vec::with_capacity(capacity);
        bytes.extend_from_slice(&self.schema_version.to_le_bytes());
        bytes.push(self.source as u8);
        bytes.push(0); // Reserved; must remain zero in schema version 1.
        bytes.extend_from_slice(&self.sequence.to_le_bytes());
        bytes.extend_from_slice(&self.event_time_ns.to_le_bytes());
        bytes.extend_from_slice(&self.received_time_ns.to_le_bytes());
        bytes.extend_from_slice(&market_len.to_le_bytes());
        bytes.extend_from_slice(&payload_len.to_le_bytes());
        bytes.extend_from_slice(self.market_id.as_bytes());
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }

    /// Decodes exactly one envelope and rejects trailing bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError`] for malformed, truncated, unsupported, or
    /// oversized input.
    pub fn decode(bytes: &[u8]) -> Result<Self, SchemaError> {
        if bytes.len() < FIXED_HEADER_BYTES {
            return Err(SchemaError::Truncated);
        }

        let schema_version = read_u16(bytes, 0)?;
        if schema_version != CURRENT_SCHEMA_VERSION {
            return Err(SchemaError::UnsupportedVersion(schema_version));
        }
        let source = EventSource::try_from(bytes[2])?;
        if bytes[3] != 0 {
            return Err(SchemaError::ReservedByteSet(bytes[3]));
        }
        let sequence = read_u64(bytes, 4)?;
        let event_time_ns = read_i64(bytes, 12)?;
        let received_time_ns = read_i64(bytes, 20)?;
        let market_len =
            usize::try_from(read_u32(bytes, 28)?).map_err(|_| SchemaError::LengthOverflow)?;
        let payload_len =
            usize::try_from(read_u32(bytes, 32)?).map_err(|_| SchemaError::LengthOverflow)?;
        validate_lengths(market_len, payload_len)?;

        let expected = FIXED_HEADER_BYTES
            .checked_add(market_len)
            .and_then(|value| value.checked_add(payload_len))
            .ok_or(SchemaError::LengthOverflow)?;
        if bytes.len() < expected {
            return Err(SchemaError::Truncated);
        }
        if bytes.len() != expected {
            return Err(SchemaError::TrailingBytes(bytes.len() - expected));
        }

        let market_end = FIXED_HEADER_BYTES + market_len;
        let market_id = std::str::from_utf8(&bytes[FIXED_HEADER_BYTES..market_end])
            .map_err(|_| SchemaError::InvalidMarketIdUtf8)?
            .to_owned();
        if market_id.is_empty() {
            return Err(SchemaError::EmptyMarketId);
        }

        Ok(Self {
            schema_version,
            source,
            sequence,
            event_time_ns,
            received_time_ns,
            market_id,
            payload: bytes[market_end..expected].to_vec(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaError {
    EmptyMarketId,
    MarketIdTooLarge(usize),
    PayloadTooLarge(usize),
    UnsupportedVersion(u16),
    UnknownSource(u8),
    ReservedByteSet(u8),
    InvalidMarketIdUtf8,
    Truncated,
    TrailingBytes(usize),
    LengthOverflow,
}

impl Display for SchemaError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMarketId => formatter.write_str("market identifier cannot be empty"),
            Self::MarketIdTooLarge(length) => {
                write!(formatter, "market identifier is too large: {length}")
            }
            Self::PayloadTooLarge(length) => write!(formatter, "payload is too large: {length}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported schema version: {version}")
            }
            Self::UnknownSource(source) => write!(formatter, "unknown event source: {source}"),
            Self::ReservedByteSet(value) => {
                write!(formatter, "reserved schema byte is non-zero: {value}")
            }
            Self::InvalidMarketIdUtf8 => {
                formatter.write_str("market identifier is not valid UTF-8")
            }
            Self::Truncated => formatter.write_str("event envelope is truncated"),
            Self::TrailingBytes(count) => {
                write!(formatter, "event envelope has {count} trailing bytes")
            }
            Self::LengthOverflow => formatter.write_str("event envelope length overflow"),
        }
    }
}

impl Error for SchemaError {}

fn validate_lengths(market_len: usize, payload_len: usize) -> Result<(), SchemaError> {
    if market_len > MAX_MARKET_ID_BYTES {
        return Err(SchemaError::MarketIdTooLarge(market_len));
    }
    if payload_len > MAX_PAYLOAD_BYTES {
        return Err(SchemaError::PayloadTooLarge(payload_len));
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, SchemaError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(SchemaError::Truncated)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, SchemaError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(SchemaError::Truncated)?;
    Ok(u32::from_le_bytes(
        value.try_into().map_err(|_| SchemaError::Truncated)?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, SchemaError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or(SchemaError::Truncated)?;
    Ok(u64::from_le_bytes(
        value.try_into().map_err(|_| SchemaError::Truncated)?,
    ))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, SchemaError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or(SchemaError::Truncated)?;
    Ok(i64::from_le_bytes(
        value.try_into().map_err(|_| SchemaError::Truncated)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EventEnvelope {
        EventEnvelope::new(
            EventSource::Market,
            42,
            100,
            110,
            "btc-hourly".to_owned(),
            vec![1, 2, 3],
        )
        .expect("valid sample")
    }

    #[test]
    fn encoding_is_deterministic_and_round_trips() {
        let event = sample();
        let first = event.encode().expect("encode");
        let second = event.encode().expect("encode again");

        assert_eq!(first, second);
        assert_eq!(EventEnvelope::decode(&first), Ok(event));
    }

    #[test]
    fn version_one_golden_encoding_is_stable() {
        let expected = vec![
            1, 0, // schema version
            1, 0, // source and reserved byte
            42, 0, 0, 0, 0, 0, 0, 0, // sequence
            100, 0, 0, 0, 0, 0, 0, 0, // event time
            110, 0, 0, 0, 0, 0, 0, 0, // receive time
            10, 0, 0, 0, // market identifier length
            3, 0, 0, 0, // payload length
            b'b', b't', b'c', b'-', b'h', b'o', b'u', b'r', b'l', b'y', // market
            1, 2, 3, // payload
        ];

        assert_eq!(sample().encode().expect("encode"), expected);
    }

    #[test]
    fn rejects_trailing_and_reserved_bytes() {
        let mut trailing = sample().encode().expect("encode");
        trailing.push(0);
        assert_eq!(
            EventEnvelope::decode(&trailing),
            Err(SchemaError::TrailingBytes(1))
        );

        let mut reserved = sample().encode().expect("encode");
        reserved[3] = 7;
        assert_eq!(
            EventEnvelope::decode(&reserved),
            Err(SchemaError::ReservedByteSet(7))
        );
    }

    #[test]
    fn rejects_lengths_before_slicing() {
        let mut encoded = sample().encode().expect("encode");
        encoded[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            EventEnvelope::decode(&encoded),
            Err(SchemaError::PayloadTooLarge(_))
        ));
    }
}
