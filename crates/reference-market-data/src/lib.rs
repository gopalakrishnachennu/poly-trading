#![forbid(unsafe_code)]

//! Public Binance settlement-reference and predictive market-data boundary.
//!
//! The finalized UTC one-hour candle is settlement-reference data. Aggregate
//! trades and best bid/ask updates are predictive observations only. This
//! crate preserves that distinction in its types, journal payload, health, and
//! replay state. It contains no credentials, strategy, or order capability.

mod payload;
mod replay;
mod websocket;

pub use payload::{
    decode_reference_payload, AggregateTrade, BookTicker, CandleData, CandleInterval,
    DecodedReferencePayload, FinalizedCandle, InProgressCandle, ReferenceEvent, ReferenceEventKind,
    ReferenceSymbol, SOURCE_TIME_UNAVAILABLE_NS,
};
pub use replay::{
    replay_path, ReferenceHealth, ReferenceReplayError, ReferenceReplayState, ReferenceSnapshot,
    ReferenceSymbolSnapshot,
};
pub use websocket::{
    capture_session, capture_session_with_channel, CaptureConfig, CaptureError, CaptureOutcome,
    DEFAULT_REFERENCE_WS_ENDPOINT,
};
