#![forbid(unsafe_code)]

//! Read-only discovery and capture boundary for public Polymarket market data.

pub mod discovery;
pub mod domain;
pub mod payload;
pub mod websocket;

pub use discovery::{DiscoveryConfig, DiscoveryError, GammaDiscoveryClient};
pub use domain::{
    Asset, DiscoveryWindow, HourlySeries, MarketIdentity, BTC_HOURLY, ETH_HOURLY, HOURLY_SERIES,
};
pub use payload::{
    decode_public_payload, BestBidAsk, BookLevel, BookSnapshot, DecodedPublicPayload, LastTrade,
    MarketSide, PayloadError, PriceChange, PublicEventKind, PublicMarketEvent, TickSizeChange,
};
pub use websocket::{
    capture_session, capture_session_with_channel, CaptureConfig, CaptureError, CaptureOutcome,
};
