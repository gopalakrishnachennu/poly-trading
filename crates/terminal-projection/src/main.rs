#![forbid(unsafe_code)]

use axum::{
    extract::State,
    http::{header, HeaderValue, Method},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use public_market_data::{
    Asset, DiscoveryConfig, DiscoveryWindow, GammaDiscoveryClient, MarketIdentity,
};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use terminal_projection::{
    compose_asset, normalize_book, select_current_markets, ProjectionPolicy, ProjectionState,
    RawBook, RawLevel, RawReference,
};
use tokio::{net::TcpListener, sync::RwLock, time::interval};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

mod paper;
mod research_export;
mod runtime_config;

const DEFAULT_PORT: u16 = 8_088;
const METRICS_MAX_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
struct AppState {
    projection: Arc<RwLock<ProjectionState>>,
    metrics: Arc<Metrics>,
    paper: Arc<RwLock<paper::PaperController>>,
    configuration: Arc<runtime_config::RuntimeConfiguration>,
    research_export: Arc<RwLock<research_export::ResearchExporter>>,
}

/// Process-local counters intentionally expose only bounded, non-sensitive
/// operational information. No order, credential, or wallet data is emitted.
#[derive(Debug, Default)]
struct Metrics {
    poll_success_total: AtomicU64,
    poll_failure_total: AtomicU64,
    last_success_ms: AtomicI64,
    last_failure_ms: AtomicI64,
}

impl Metrics {
    fn render(&self) -> String {
        let output = format!(
            "# HELP poly_terminal_poll_success_total Successful read-only projection polls.\n# TYPE poly_terminal_poll_success_total counter\npoly_terminal_poll_success_total {}\n# HELP poly_terminal_poll_failure_total Failed read-only projection polls.\n# TYPE poly_terminal_poll_failure_total counter\npoly_terminal_poll_failure_total {}\n# HELP poly_terminal_last_success_timestamp_ms Unix timestamp of the last successful poll.\n# TYPE poly_terminal_last_success_timestamp_ms gauge\npoly_terminal_last_success_timestamp_ms {}\n# HELP poly_terminal_last_failure_timestamp_ms Unix timestamp of the last failed poll.\n# TYPE poly_terminal_last_failure_timestamp_ms gauge\npoly_terminal_last_failure_timestamp_ms {}\n",
            self.poll_success_total.load(Ordering::Relaxed),
            self.poll_failure_total.load(Ordering::Relaxed),
            self.last_success_ms.load(Ordering::Relaxed),
            self.last_failure_ms.load(Ordering::Relaxed),
        );
        // Keep the endpoint bounded even if this format grows in the future.
        output.chars().take(METRICS_MAX_BYTES).collect()
    }
}

#[derive(Debug)]
struct Poller {
    discovery: GammaDiscoveryClient,
    client: Client,
    policy: ProjectionPolicy,
    polling: runtime_config::Polling,
    cached: Option<CachedMarkets>,
    sources: runtime_config::Sources,
}

#[derive(Clone, Debug)]
struct CachedMarkets {
    markets: BTreeMap<Asset, MarketIdentity>,
    discovered_at_ms: i64,
}

#[derive(Debug, Deserialize)]
struct BookResponse {
    market: String,
    asset_id: String,
    timestamp: String,
    hash: String,
    tick_size: String,
    bids: Vec<BookLevelResponse>,
    asks: Vec<BookLevelResponse>,
}

#[derive(Debug, Deserialize)]
struct BookLevelResponse {
    price: String,
    size: String,
}

#[derive(Debug, Deserialize)]
struct TickerResponse {
    symbol: String,
    price: String,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    mode: terminal_projection::ProjectionMode,
    sequence: u64,
    no_trade: bool,
    reason: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let now = now_ms()?;
    let configuration = Arc::new(runtime_config::RuntimeConfiguration::load(now));
    let policy = configuration.projection_policy();
    let projection = Arc::new(RwLock::new(ProjectionState::new(policy, now)?));
    let poller = Poller::new(policy, configuration.status.effective.clone())?;
    let background_projection = projection.clone();
    let metrics = Arc::new(Metrics::default());
    let background_metrics = metrics.clone();
    let paper = Arc::new(RwLock::new(
        paper::PaperController::recover().map_err(std::io::Error::other)?,
    ));
    let background_paper = paper.clone();
    let research_export = Arc::new(RwLock::new(research_export::ResearchExporter::default()));
    let background_configuration = configuration.clone();
    tokio::spawn(async move {
        poll_loop(
            poller,
            background_projection,
            background_metrics,
            background_paper,
            background_configuration
                .status
                .effective
                .polling
                .refresh_interval_ms,
        )
        .await;
    });

    let state = AppState {
        projection,
        metrics,
        paper,
        configuration,
        research_export,
    };
    let cors = CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("http://localhost:3000"),
            HeaderValue::from_static("http://127.0.0.1:3000"),
        ])
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE]);
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/metrics", get(metrics_endpoint))
        .route("/api/v1/terminal/snapshot", get(snapshot))
        .route("/api/v1/configuration", get(configuration_status))
        .route("/api/v1/paper/status", get(paper_status))
        .route("/api/v1/paper/preflight", get(paper_preflight))
        .route("/api/v1/paper/report", get(paper_report))
        .route(
            "/api/v1/research-export/status",
            get(research_export_status),
        )
        .route(
            "/api/v1/research-export/refresh",
            axum::routing::post(refresh_research_export),
        )
        .route("/api/v1/paper/session", axum::routing::post(start_paper))
        .route(
            "/api/v1/paper/session/stop",
            axum::routing::post(stop_paper),
        )
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    let port = env::var("POLY_TERMINAL_PORT")
        .ok()
        .map(|value| value.parse::<u16>())
        .transpose()?
        .unwrap_or(DEFAULT_PORT);
    // Keep localhost as the safe developer default. Containerized deployments
    // must opt in explicitly with POLY_TERMINAL_BIND (for example 0.0.0.0).
    let bind_host = env::var("POLY_TERMINAL_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let address: SocketAddr = format!("{bind_host}:{port}").parse()?;
    let listener = TcpListener::bind(address).await?;
    println!("read-only terminal projection listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn snapshot(State(state): State<AppState>) -> impl IntoResponse {
    let snapshot = state.projection.read().await.snapshot().clone();
    (
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
        ],
        Json(snapshot),
    )
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let snapshot = state.projection.read().await.snapshot().clone();
    Json(HealthResponse {
        mode: snapshot.mode,
        sequence: snapshot.sequence,
        no_trade: snapshot.no_trade,
        reason: snapshot.reason,
    })
}

async fn metrics_endpoint(State(state): State<AppState>) -> impl IntoResponse {
    let body = state.metrics.render();
    (
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
            ),
        ],
        body,
    )
}

async fn paper_status(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.paper.read().await.status())
}

async fn paper_preflight(State(state): State<AppState>) -> impl IntoResponse {
    let now = now_ms().unwrap_or(0);
    Json(
        state
            .paper
            .read()
            .await
            .preflight(now, state.configuration.permits_new_paper_campaign()),
    )
}

async fn paper_report(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.paper.read().await.report(now_ms().unwrap_or(0)))
}

async fn research_export_status(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.research_export.read().await.status())
}

async fn refresh_research_export(State(state): State<AppState>) -> impl IntoResponse {
    let journal_path = state.paper.read().await.journal_path();
    let Some(journal_path) = journal_path else {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "paper campaign journal is unavailable"})),
        )
            .into_response();
    };
    let now = now_ms().unwrap_or(0);
    match state
        .research_export
        .write()
        .await
        .refresh(&journal_path, now)
    {
        Ok(report) => (axum::http::StatusCode::OK, Json(serde_json::json!(report))).into_response(),
        Err(error) => (
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({"error": error})),
        )
            .into_response(),
    }
}

async fn configuration_status(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.configuration.status.clone())
}

async fn start_paper(
    State(state): State<AppState>,
    Json(request): Json<paper::StartPaperRequest>,
) -> impl IntoResponse {
    let now = now_ms().unwrap_or(0);
    if !state.configuration.permits_new_paper_campaign() {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "a current bound runtime configuration is required for a new paper campaign",
                "paper_only": true,
            })),
        )
            .into_response();
    }
    let (Some(config_id), Some(config_digest)) = (
        state.configuration.status.config_id.clone(),
        state.configuration.status.digest.clone(),
    ) else {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "runtime configuration binding is absent", "paper_only": true})),
        )
            .into_response();
    };
    let preflight = state
        .paper
        .read()
        .await
        .preflight(now, state.configuration.permits_new_paper_campaign());
    if !preflight.eligible {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": preflight.reason,
                "preflight": preflight,
                "paper_only": true,
            })),
        )
            .into_response();
    }
    match state
        .paper
        .write()
        .await
        .start(request, now, &config_id, &config_digest)
    {
        Ok(status) => (axum::http::StatusCode::OK, Json(status)).into_response(),
        Err(error) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": error, "paper_only": true})),
        )
            .into_response(),
    }
}

async fn stop_paper(State(state): State<AppState>) -> impl IntoResponse {
    let now = now_ms().unwrap_or(0);
    (
        axum::http::StatusCode::OK,
        Json(state.paper.write().await.stop(now)),
    )
}

impl Poller {
    fn new(
        policy: ProjectionPolicy,
        effective: runtime_config::RuntimeConfigurationView,
    ) -> Result<Self, Box<dyn Error>> {
        let client = Client::builder()
            .timeout(Duration::from_millis(effective.polling.http_timeout_ms))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("poly-terminal-read-only-projection/0.1")
            .build()?;
        let discovery_config = DiscoveryConfig {
            endpoint: effective.sources.gamma_keyset_url.clone(),
            request_timeout: Duration::from_millis(effective.polling.http_timeout_ms),
            ..DiscoveryConfig::default()
        };
        Ok(Self {
            discovery: GammaDiscoveryClient::new(discovery_config)?,
            client,
            policy,
            polling: effective.polling,
            cached: None,
            sources: effective.sources,
        })
    }

    async fn refresh(
        &mut self,
        now_ms: i64,
    ) -> Result<Vec<terminal_projection::AssetProjection>, String> {
        let markets = self.current_markets(now_ms).await?;
        let btc = markets
            .get(&Asset::Bitcoin)
            .ok_or("BTC market unavailable")?;
        let eth = markets
            .get(&Asset::Ethereum)
            .ok_or("ETH market unavailable")?;
        let (btc_result, eth_result) =
            tokio::join!(self.fetch_asset(btc, now_ms), self.fetch_asset(eth, now_ms),);
        Ok(vec![btc_result?, eth_result?])
    }

    async fn current_markets(
        &mut self,
        now_ms: i64,
    ) -> Result<BTreeMap<Asset, MarketIdentity>, String> {
        let needs_discovery = self.cached.as_ref().is_none_or(|cache| {
            now_ms - cache.discovered_at_ms >= self.polling.discovery_refresh_ms
                || cache
                    .markets
                    .values()
                    .any(|market| now_ms >= market.end_time_ms)
        });
        if needs_discovery {
            let window = DiscoveryWindow::new(
                now_ms
                    .checked_sub(self.polling.discovery_lookback_ms)
                    .ok_or("discovery time underflow")?,
                now_ms
                    .checked_add(self.polling.discovery_lookahead_ms)
                    .ok_or("discovery time overflow")?,
            )
            .map_err(|error| error.to_string())?;
            let discovered = self
                .discovery
                .discover(window)
                .await
                .map_err(|error| error.to_string())?;
            let markets =
                select_current_markets(&discovered, now_ms).map_err(|error| error.to_string())?;
            self.cached = Some(CachedMarkets {
                markets,
                discovered_at_ms: now_ms,
            });
        }
        self.cached
            .as_ref()
            .map(|cache| cache.markets.clone())
            .ok_or_else(|| "market cache unavailable".to_owned())
    }

    async fn fetch_asset(
        &self,
        identity: &MarketIdentity,
        _cycle_started_at_ms: i64,
    ) -> Result<terminal_projection::AssetProjection, String> {
        let symbol = match identity.asset {
            Asset::Bitcoin => "BTCUSDT",
            Asset::Ethereum => "ETHUSDT",
        };
        let (up, down, reference) = tokio::join!(
            self.fetch_book(&identity.condition_id, &identity.up_token_id),
            self.fetch_book(&identity.condition_id, &identity.down_token_id),
            self.fetch_reference(symbol, identity.start_time_ms, identity.end_time_ms),
        );
        let reference = reference?;
        let composed_at_ms = now_ms().map_err(|error| error.to_string())?;
        compose_asset(
            identity,
            up?,
            down?,
            &reference,
            composed_at_ms,
            self.policy,
        )
        .map_err(|error| error.to_string())
    }

    async fn fetch_book(
        &self,
        condition_id: &str,
        token_id: &str,
    ) -> Result<terminal_projection::BookProjection, String> {
        let response = self
            .client
            .get(&self.sources.clob_book_url)
            .query(&[("token_id", token_id)])
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let body: BookResponse =
            decode_bounded(response, self.polling.maximum_response_bytes).await?;
        let received_at_ms = now_ms().map_err(|error| error.to_string())?;
        let raw = RawBook {
            condition_id: body.market,
            token_id: body.asset_id,
            timestamp: body.timestamp,
            hash: body.hash,
            tick_size: body.tick_size,
            bids: body
                .bids
                .into_iter()
                .map(|level| RawLevel {
                    price: level.price,
                    size: level.size,
                })
                .collect(),
            asks: body
                .asks
                .into_iter()
                .map(|level| RawLevel {
                    price: level.price,
                    size: level.size,
                })
                .collect(),
        };
        normalize_book(raw, condition_id, token_id, received_at_ms, self.policy)
            .map_err(|error| error.to_string())
    }

    async fn fetch_reference(
        &self,
        symbol: &str,
        start_time_ms: i64,
        end_time_ms: i64,
    ) -> Result<RawReference, String> {
        let base = self.sources.reference_api_url.trim_end_matches('/');
        let ticker_url = format!("{base}/ticker/price");
        let kline_url = format!("{base}/klines");
        let ticker_request = self
            .client
            .get(ticker_url)
            .query(&[("symbol", symbol)])
            .send();
        let start = start_time_ms.to_string();
        let kline_request = self
            .client
            .get(kline_url)
            .query(&[
                ("symbol", symbol),
                ("interval", "1h"),
                ("startTime", &start),
                ("limit", "1"),
            ])
            .send();
        let (ticker_response, kline_response) = tokio::join!(ticker_request, kline_request);
        let ticker: TickerResponse = decode_bounded(
            ticker_response.map_err(|error| error.to_string())?,
            self.polling.maximum_response_bytes,
        )
        .await?;
        let klines: Vec<Vec<Value>> = decode_bounded(
            kline_response.map_err(|error| error.to_string())?,
            self.polling.maximum_response_bytes,
        )
        .await?;
        let received_at_ms = now_ms().map_err(|error| error.to_string())?;
        if ticker.symbol != symbol || klines.len() != 1 || klines[0].len() < 7 {
            return Err("reference identity or kline shape invalid".to_owned());
        }
        let kline = &klines[0];
        let open_time = kline[0].as_i64().ok_or("kline open time invalid")?;
        let open = kline[1]
            .as_str()
            .ok_or("kline open price invalid")?
            .to_owned();
        let close_time = kline[6].as_i64().ok_or("kline close time invalid")?;
        if open_time != start_time_ms || close_time != end_time_ms - 1 {
            return Err("reference kline does not match market hour".to_owned());
        }
        Ok(RawReference {
            symbol: ticker.symbol,
            price: ticker.price,
            candle_open_time_ms: open_time,
            candle_close_time_ms: close_time,
            candle_open: open,
            received_at_ms,
        })
    }
}

async fn poll_loop(
    mut poller: Poller,
    projection: Arc<RwLock<ProjectionState>>,
    metrics: Arc<Metrics>,
    paper: Arc<RwLock<paper::PaperController>>,
    refresh_interval_ms: u64,
) {
    let mut ticks = interval(Duration::from_millis(refresh_interval_ms));
    loop {
        ticks.tick().await;
        let now = match now_ms() {
            Ok(value) => value,
            Err(error) => {
                let _ = projection
                    .write()
                    .await
                    .publish_unavailable(0, error.to_string());
                continue;
            }
        };
        match poller.refresh(now).await {
            Ok(assets) => {
                let published_at_ms = now_ms().unwrap_or(now);
                paper.write().await.observe(&assets, published_at_ms);
                metrics.poll_success_total.fetch_add(1, Ordering::Relaxed);
                metrics
                    .last_success_ms
                    .store(published_at_ms, Ordering::Relaxed);
                let mut owner = projection.write().await;
                if let Err(error) = owner.publish_ready(assets, published_at_ms) {
                    let _ = owner.publish_unavailable(
                        published_at_ms,
                        format!("projection publish rejected: {error}"),
                    );
                }
            }
            Err(error) => {
                metrics.poll_failure_total.fetch_add(1, Ordering::Relaxed);
                metrics.last_failure_ms.store(now, Ordering::Relaxed);
                let _ = projection.write().await.publish_unavailable(now, error);
            }
        }
    }
}

async fn decode_bounded<T: for<'de> Deserialize<'de>>(
    response: Response,
    maximum_response_bytes: usize,
) -> Result<T, String> {
    if !response.status().is_success() {
        return Err(format!("upstream HTTP status {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(maximum_response_bytes).unwrap_or(u64::MAX))
    {
        return Err("upstream response exceeds bound".to_owned());
    }
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    if bytes.len() > maximum_response_bytes {
        return Err("upstream response exceeds bound".to_owned());
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("upstream JSON invalid: {error}"))
}

fn now_ms() -> Result<i64, std::io::Error> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(std::io::Error::other)?;
    i64::try_from(duration.as_millis()).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_are_bounded_and_prometheus_compatible() {
        let metrics = Metrics::default();
        metrics.poll_success_total.store(3, Ordering::Relaxed);
        metrics.last_success_ms.store(42, Ordering::Relaxed);
        let rendered = metrics.render();
        assert!(rendered.len() <= METRICS_MAX_BYTES);
        assert!(rendered.contains("poly_terminal_poll_success_total 3"));
        assert!(rendered.contains("poly_terminal_last_success_timestamp_ms 42"));
    }
}
