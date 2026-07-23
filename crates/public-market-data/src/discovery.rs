//! Bounded Gamma keyset discovery for configured hourly series.

use crate::domain::{
    parse_time_ms, validate_candidate, DiscoveryWindow, HourlySeries, IdentityError,
    MarketCandidate, MarketIdentity, HOURLY_SERIES,
};
use chrono::{SecondsFormat, TimeZone, Utc};
use futures_util::StreamExt;
use reqwest::{Client, Url};
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;
use thiserror::Error;

pub const DEFAULT_GAMMA_KEYSET_ENDPOINT: &str = "https://gamma-api.polymarket.com/events/keyset";
const ONE_HOUR_MS: i64 = 3_600_000;

#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    pub endpoint: String,
    pub request_timeout: Duration,
    pub page_size: u16,
    pub max_pages: u16,
    pub max_markets: usize,
    pub max_response_bytes: usize,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_GAMMA_KEYSET_ENDPOINT.to_owned(),
            request_timeout: Duration::from_secs(10),
            page_size: 50,
            max_pages: 8,
            max_markets: 128,
            max_response_bytes: 4 * 1024 * 1024,
        }
    }
}

impl DiscoveryConfig {
    fn validate(&self) -> Result<(), DiscoveryError> {
        if self.page_size == 0 || self.page_size > 500 {
            return Err(DiscoveryError::InvalidConfig(
                "page_size must be in 1..=500",
            ));
        }
        if self.max_pages == 0 || self.max_markets == 0 || self.max_response_bytes == 0 {
            return Err(DiscoveryError::InvalidConfig(
                "max_pages, max_markets, and max_response_bytes must be positive",
            ));
        }
        Url::parse(&self.endpoint).map_err(DiscoveryError::InvalidEndpoint)?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("invalid discovery configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("invalid discovery endpoint: {0}")]
    InvalidEndpoint(url::ParseError),
    #[error("failed to build HTTP client: {0}")]
    ClientBuild(reqwest::Error),
    #[error("discovery request failed: {0}")]
    Request(reqwest::Error),
    #[error("discovery returned HTTP status {0}")]
    HttpStatus(reqwest::StatusCode),
    #[error("discovery response exceeds {limit} bytes")]
    ResponseTooLarge { limit: usize },
    #[error("invalid discovery JSON: {0}")]
    Json(serde_json::Error),
    #[error("invalid market identity: {0}")]
    Identity(#[from] IdentityError),
    #[error("discovery cursor repeated: {0}")]
    RepeatedCursor(String),
    #[error("discovery exceeded the configured page limit")]
    PageLimit,
    #[error("discovery exceeded the configured market limit")]
    MarketLimit,
    #[error("timestamp is outside chrono's supported range: {0}")]
    TimestampRange(i64),
}

#[derive(Debug)]
pub struct GammaDiscoveryClient {
    config: DiscoveryConfig,
    client: Client,
    series: Vec<HourlySeries>,
}

impl GammaDiscoveryClient {
    /// Builds a read-only discovery client for BTC and ETH hourly series.
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryError`] for invalid bounds, endpoint, or HTTP client
    /// configuration.
    pub fn new(config: DiscoveryConfig) -> Result<Self, DiscoveryError> {
        config.validate()?;
        let client = Client::builder()
            .timeout(config.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("poly-trading-public-recorder/0.1")
            .build()
            .map_err(DiscoveryError::ClientBuild)?;
        Ok(Self {
            config,
            client,
            series: HOURLY_SERIES.to_vec(),
        })
    }

    /// Discovers validated hourly markets overlapping the supplied UTC window.
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryError`] on transport, bounds, pagination, schema, or
    /// market-identity failures.
    pub async fn discover(
        &self,
        window: DiscoveryWindow,
    ) -> Result<Vec<MarketIdentity>, DiscoveryError> {
        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut markets = Vec::new();

        for _ in 0..self.config.max_pages {
            let url = self.build_url(window, cursor.as_deref())?;
            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(DiscoveryError::Request)?;
            if !response.status().is_success() {
                return Err(DiscoveryError::HttpStatus(response.status()));
            }
            let body = read_bounded(response, self.config.max_response_bytes).await?;
            let page: GammaPage = serde_json::from_slice(&body).map_err(DiscoveryError::Json)?;
            markets.extend(normalize_page(page.events, window, &self.series)?);
            if markets.len() > self.config.max_markets {
                return Err(DiscoveryError::MarketLimit);
            }

            let Some(next) = page.next_cursor.filter(|value| !value.is_empty()) else {
                markets.sort_by(|left, right| {
                    left.start_time_ms
                        .cmp(&right.start_time_ms)
                        .then(left.asset.cmp(&right.asset))
                });
                markets.dedup_by(|left, right| left.condition_id == right.condition_id);
                return Ok(markets);
            };
            if !seen_cursors.insert(next.clone()) {
                return Err(DiscoveryError::RepeatedCursor(next));
            }
            cursor = Some(next);
        }
        Err(DiscoveryError::PageLimit)
    }

    fn build_url(
        &self,
        window: DiscoveryWindow,
        cursor: Option<&str>,
    ) -> Result<Url, DiscoveryError> {
        let mut url = Url::parse(&self.config.endpoint).map_err(DiscoveryError::InvalidEndpoint)?;
        // Gamma's event `startDate` is creation time for these recurring
        // markets. The authoritative hourly start is `market.eventStartTime`,
        // so keyset discovery is bounded by resolution time and exact overlap
        // is enforced again during normalization.
        let earliest_end = format_timestamp(window.start_ms)?;
        let latest_end_ms = window
            .end_ms
            .checked_add(ONE_HOUR_MS)
            .ok_or(DiscoveryError::TimestampRange(window.end_ms))?;
        let latest_end = format_timestamp(latest_end_ms)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", &self.config.page_size.to_string());
            query.append_pair("closed", "false");
            query.append_pair("order", "endDate");
            query.append_pair("ascending", "true");
            query.append_pair("end_date_min", &earliest_end);
            query.append_pair("end_date_max", &latest_end);
            for series in &self.series {
                query.append_pair("series_id", series.id);
            }
            if let Some(cursor) = cursor {
                query.append_pair("after_cursor", cursor);
            }
        }
        Ok(url)
    }
}

async fn read_bounded(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, DiscoveryError> {
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(limit).unwrap_or(u64::MAX))
    {
        return Err(DiscoveryError::ResponseTooLarge { limit });
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(DiscoveryError::Request)?;
        let new_length = body
            .len()
            .checked_add(chunk.len())
            .ok_or(DiscoveryError::ResponseTooLarge { limit })?;
        if new_length > limit {
            return Err(DiscoveryError::ResponseTooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn format_timestamp(timestamp_ms: i64) -> Result<String, DiscoveryError> {
    Utc.timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or(DiscoveryError::TimestampRange(timestamp_ms))
}

fn normalize_page(
    events: Vec<GammaEvent>,
    window: DiscoveryWindow,
    configured: &[HourlySeries],
) -> Result<Vec<MarketIdentity>, DiscoveryError> {
    let mut identities = Vec::new();
    for event in events {
        if event.active != Some(true) || event.closed == Some(true) {
            continue;
        }
        for market in event.markets {
            if market.active != Some(true)
                || market.closed == Some(true)
                || market.accepting_orders != Some(true)
                || market.enable_order_book != Some(true)
            {
                continue;
            }
            let start_time = event
                .start_time
                .as_deref()
                .or(market.event_start_time.as_deref())
                .ok_or(IdentityError::Missing("start_time"))?;
            let end_time = market
                .end_date
                .as_deref()
                .or(event.end_date.as_deref())
                .ok_or(IdentityError::Missing("end_time"))?;
            let start_ms = parse_time_ms("start_time", start_time)?;
            let end_ms = parse_time_ms("end_time", end_time)?;
            if !window.overlaps(start_ms, end_ms) {
                continue;
            }

            identities.push(validate_candidate(
                MarketCandidate {
                    event_id: event.id.clone(),
                    event_slug: required(event.slug.as_ref(), "event_slug")?,
                    market_id: market.id,
                    condition_id: market.condition_id,
                    question_id: required(market.question_id.as_ref(), "question_id")?,
                    market_slug: required(market.slug.as_ref(), "market_slug")?,
                    series: event
                        .series
                        .iter()
                        .filter_map(|series| Some((series.id.clone(), series.slug.clone()?)))
                        .collect(),
                    series_slug: required(event.series_slug.as_ref(), "series_slug")?,
                    title: required(event.title.as_ref(), "title")?,
                    start_time: start_time.to_owned(),
                    end_time: end_time.to_owned(),
                    resolution_source: market
                        .resolution_source
                        .clone()
                        .or_else(|| event.resolution_source.clone())
                        .ok_or(IdentityError::Missing("resolution_source"))?,
                    description: market
                        .description
                        .clone()
                        .or_else(|| event.description.clone())
                        .ok_or(IdentityError::Missing("description"))?,
                    outcomes_json: required(market.outcomes.as_ref(), "outcomes")?,
                    token_ids_json: required(market.clob_token_ids.as_ref(), "clob_token_ids")?,
                },
                configured,
            )?);
        }
    }
    Ok(identities)
}

fn required(value: Option<&String>, field: &'static str) -> Result<String, IdentityError> {
    value.cloned().ok_or(IdentityError::Missing(field))
}

#[derive(Debug, Deserialize)]
struct GammaPage {
    #[serde(default)]
    events: Vec<GammaEvent>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GammaEvent {
    id: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    resolution_source: Option<String>,
    #[serde(default)]
    end_date: Option<String>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    active: Option<bool>,
    #[serde(default)]
    closed: Option<bool>,
    #[serde(default)]
    series_slug: Option<String>,
    #[serde(default)]
    series: Vec<GammaSeries>,
    #[serde(default)]
    markets: Vec<GammaMarket>,
}

#[derive(Debug, Deserialize)]
struct GammaSeries {
    id: String,
    #[serde(default)]
    slug: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GammaMarket {
    id: String,
    condition_id: String,
    #[serde(default, rename = "questionID")]
    question_id: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    resolution_source: Option<String>,
    #[serde(default)]
    end_date: Option<String>,
    #[serde(default)]
    event_start_time: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    outcomes: Option<String>,
    #[serde(default)]
    active: Option<bool>,
    #[serde(default)]
    closed: Option<bool>,
    #[serde(default)]
    enable_order_book: Option<bool>,
    #[serde(default)]
    clob_token_ids: Option<String>,
    #[serde(default)]
    accepting_orders: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        format!(
            r#"{{
                "events": [{{
                    "id": "e1",
                    "slug": "bitcoin-up-or-down-july-16-2026-10pm-et",
                    "title": "Bitcoin Up or Down - July 16, 10PM ET",
                    "description": "Final Binance BTC one-hour candle.",
                    "resolutionSource": "https://example.test/btc",
                    "endDate": "2026-07-17T03:00:00Z",
                    "startTime": "2026-07-17T02:00:00Z",
                    "active": true,
                    "closed": false,
                    "seriesSlug": "{}",
                    "series": [{{"id":"{}","slug":"{}"}}],
                    "markets": [{{
                        "id": "m1",
                        "conditionId": "0x{}",
                        "questionID": "0x{}",
                        "slug": "bitcoin-up-or-down-july-16-2026-10pm-et",
                        "endDate": "2026-07-17T03:00:00Z",
                        "eventStartTime": "2026-07-17T02:00:00Z",
                        "outcomes": "[\"Up\",\"Down\"]",
                        "active": true,
                        "closed": false,
                        "enableOrderBook": true,
                        "acceptingOrders": true,
                        "clobTokenIds": "[\"11\",\"22\"]"
                    }}]
                }}],
                "next_cursor": null
            }}"#,
            crate::BTC_HOURLY.slug,
            crate::BTC_HOURLY.id,
            crate::BTC_HOURLY.slug,
            "a".repeat(64),
            "b".repeat(64),
        )
    }

    #[test]
    fn normalizes_valid_hourly_fixture() {
        let page: GammaPage = serde_json::from_str(&fixture()).expect("fixture");
        let window = DiscoveryWindow::new(1_784_253_600_000, 1_784_264_400_000).expect("window");
        let markets = normalize_page(page.events, window, &HOURLY_SERIES).expect("normalize");
        assert_eq!(markets.len(), 1);
        assert_eq!(markets[0].up_token_id, "11");
        assert_eq!(markets[0].down_token_id, "22");
    }

    #[test]
    fn skips_inactive_market() {
        let json = fixture().replace("\"acceptingOrders\": true", "\"acceptingOrders\": false");
        let page: GammaPage = serde_json::from_str(&json).expect("fixture");
        let window = DiscoveryWindow::new(i64::MIN + 1, i64::MAX).expect("window");
        assert!(normalize_page(page.events, window, &HOURLY_SERIES)
            .expect("normalize")
            .is_empty());
    }

    #[test]
    fn repeated_cursor_is_detectable() {
        let mut seen = HashSet::new();
        assert!(seen.insert("cursor".to_owned()));
        assert!(!seen.insert("cursor".to_owned()));
    }

    #[test]
    fn subscription_query_repeats_series_ids() {
        let client = GammaDiscoveryClient::new(DiscoveryConfig::default()).expect("client");
        let window = DiscoveryWindow::new(1_784_253_600_000, 1_784_264_400_000).expect("window");
        let url = client.build_url(window, Some("next")).expect("url");
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(
            pairs.iter().filter(|(key, _)| key == "series_id").count(),
            2
        );
        assert!(pairs
            .iter()
            .any(|(key, value)| key == "after_cursor" && value == "next"));
        assert!(pairs
            .iter()
            .any(|(key, value)| { key == "order" && value == "endDate" }));
        assert!(pairs.iter().any(|(key, _)| key == "end_date_min"));
        assert!(pairs.iter().any(|(key, _)| key == "end_date_max"));
        assert!(!pairs.iter().any(|(key, _)| key == "start_time_min"));
    }

    #[test]
    fn rejects_invalid_discovery_bounds() {
        let invalid_page_size = DiscoveryConfig {
            page_size: 501,
            ..DiscoveryConfig::default()
        };
        assert!(matches!(
            GammaDiscoveryClient::new(invalid_page_size),
            Err(DiscoveryError::InvalidConfig(_))
        ));

        let invalid_page_limit = DiscoveryConfig {
            max_pages: 0,
            ..DiscoveryConfig::default()
        };
        assert!(matches!(
            GammaDiscoveryClient::new(invalid_page_limit),
            Err(DiscoveryError::InvalidConfig(_))
        ));

        let invalid_body_limit = DiscoveryConfig {
            max_response_bytes: 0,
            ..DiscoveryConfig::default()
        };
        assert!(matches!(
            GammaDiscoveryClient::new(invalid_body_limit),
            Err(DiscoveryError::InvalidConfig(_))
        ));
    }
}
