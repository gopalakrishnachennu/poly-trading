#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf};
use terminal_projection::ProjectionPolicy;

const CONFIG_PATH_ENV: &str = "POLY_TERMINAL_CONFIG_PATH";
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeConfigurationStatus {
    pub mode: String,
    pub config_id: Option<String>,
    pub digest: Option<String>,
    pub source: String,
    pub restart_required_for_change: bool,
    pub permits_new_paper_campaign: bool,
    pub reason: String,
    pub effective: RuntimeConfigurationView,
}

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeConfigurationView {
    pub sources: Sources,
    pub polling: Polling,
    pub projection: Projection,
    pub client_display: ClientDisplay,
}

#[derive(Clone, Debug)]
pub struct RuntimeConfiguration {
    pub status: RuntimeConfigurationStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeConfigurationDocument {
    schema_version: u16,
    config_id: String,
    issued_at_ms: i64,
    expires_at_ms: i64,
    sources: Sources,
    polling: Polling,
    projection: Projection,
    client_display: ClientDisplay,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // Mirrors the externally reviewed JSON contract.
pub struct Sources {
    pub gamma_keyset_url: String,
    pub clob_book_url: String,
    pub reference_api_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Polling {
    pub http_timeout_ms: u64,
    pub refresh_interval_ms: u64,
    pub discovery_refresh_ms: i64,
    pub discovery_lookback_ms: i64,
    pub discovery_lookahead_ms: i64,
    pub maximum_response_bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // Explicit safety units are part of the contract.
pub struct Projection {
    pub maximum_book_age_ms: i64,
    pub maximum_reference_age_ms: i64,
    pub maximum_cross_book_skew_ms: i64,
    pub maximum_projection_age_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // Explicit milliseconds prevent unit ambiguity.
pub struct ClientDisplay {
    pub poll_interval_ms: u64,
    pub request_timeout_ms: u64,
    pub maximum_client_age_ms: i64,
    pub maximum_future_skew_ms: i64,
}

impl RuntimeConfiguration {
    #[must_use]
    pub fn load(now_ms: i64) -> Self {
        match env::var(CONFIG_PATH_ENV) {
            Ok(raw_path) => Self::load_file(&PathBuf::from(raw_path), now_ms),
            Err(_) => {
                Self::legacy("configuration path is absent; observation-only defaults active")
            }
        }
    }

    fn load_file(path: &PathBuf, now_ms: i64) -> Self {
        let result = (|| -> Result<(RuntimeConfigurationDocument, String), String> {
            let metadata = fs::metadata(path)
                .map_err(|error| format!("configuration metadata failed: {error}"))?;
            if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
                return Err("configuration path is not a bounded regular file".into());
            }
            let bytes =
                fs::read(path).map_err(|error| format!("configuration read failed: {error}"))?;
            let document: RuntimeConfigurationDocument = serde_json::from_slice(&bytes)
                .map_err(|error| format!("configuration JSON invalid: {error}"))?;
            validate(&document, now_ms)?;
            let canonical = serde_json::to_vec(&document)
                .map_err(|error| format!("configuration canonicalization failed: {error}"))?;
            Ok((document, hex(blake3::hash(&canonical).as_bytes())))
        })();
        match result {
            Ok((document, digest)) => Self {
                status: RuntimeConfigurationStatus {
                    mode: "BOUND".into(),
                    config_id: Some(document.config_id),
                    digest: Some(digest),
                    source: path.display().to_string(),
                    restart_required_for_change: true,
                    permits_new_paper_campaign: true,
                    reason: "validated immutable configuration loaded at process start".into(),
                    effective: RuntimeConfigurationView {
                        sources: document.sources,
                        polling: document.polling,
                        projection: document.projection,
                        client_display: document.client_display,
                    },
                },
            },
            Err(error) => Self::invalid(error),
        }
    }

    #[must_use]
    pub fn projection_policy(&self) -> ProjectionPolicy {
        let projection = &self.status.effective.projection;
        ProjectionPolicy {
            maximum_book_age_ms: projection.maximum_book_age_ms,
            maximum_reference_age_ms: projection.maximum_reference_age_ms,
            maximum_cross_book_skew_ms: projection.maximum_cross_book_skew_ms,
            maximum_projection_age_ms: projection.maximum_projection_age_ms,
        }
    }

    #[must_use]
    pub fn permits_new_paper_campaign(&self) -> bool {
        self.status.permits_new_paper_campaign
    }

    fn legacy(reason: &str) -> Self {
        Self {
            status: RuntimeConfigurationStatus {
                mode: "LEGACY_DEFAULTS_OBSERVATION_ONLY".into(),
                config_id: None,
                digest: None,
                source: "compiled safe defaults".into(),
                restart_required_for_change: true,
                permits_new_paper_campaign: false,
                reason: reason.into(),
                effective: safe_defaults(),
            },
        }
    }

    fn invalid(reason: impl AsRef<str>) -> Self {
        let mut configuration =
            Self::legacy(&format!("invalid configuration: {}", reason.as_ref()));
        configuration.status.mode = "INVALID_OBSERVATION_ONLY".into();
        configuration
    }
}

fn safe_defaults() -> RuntimeConfigurationView {
    RuntimeConfigurationView {
        sources: Sources {
            gamma_keyset_url: "https://gamma-api.polymarket.com/events/keyset".into(),
            clob_book_url: "https://clob.polymarket.com/book".into(),
            reference_api_url: "https://data-api.binance.vision/api/v3".into(),
        },
        polling: Polling {
            http_timeout_ms: 5_000,
            refresh_interval_ms: 1_000,
            discovery_refresh_ms: 15_000,
            discovery_lookback_ms: 5 * 60 * 1_000,
            discovery_lookahead_ms: 70 * 60 * 1_000,
            maximum_response_bytes: 2 * 1024 * 1024,
        },
        projection: Projection {
            maximum_book_age_ms: 5_000,
            maximum_reference_age_ms: 5_000,
            maximum_cross_book_skew_ms: 2_000,
            maximum_projection_age_ms: 5_000,
        },
        client_display: ClientDisplay {
            poll_interval_ms: 1_000,
            request_timeout_ms: 3_500,
            maximum_client_age_ms: 7_000,
            maximum_future_skew_ms: 1_500,
        },
    }
}

fn validate(document: &RuntimeConfigurationDocument, now_ms: i64) -> Result<(), String> {
    if document.schema_version != 1
        || document.config_id.trim().is_empty()
        || document.config_id.len() > 128
    {
        return Err("configuration schema or ID is invalid".into());
    }
    if document.issued_at_ms < 0
        || document.expires_at_ms <= document.issued_at_ms
        || now_ms < document.issued_at_ms
        || now_ms >= document.expires_at_ms
    {
        return Err("configuration validity interval is invalid or not current".into());
    }
    for endpoint in [
        &document.sources.gamma_keyset_url,
        &document.sources.clob_book_url,
        &document.sources.reference_api_url,
    ] {
        let parsed =
            reqwest::Url::parse(endpoint).map_err(|_| "configuration endpoint URL is invalid")?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(
                "configuration endpoint must be an HTTPS origin/path without query or fragment"
                    .into(),
            );
        }
    }
    let polling = &document.polling;
    if polling.http_timeout_ms == 0
        || polling.http_timeout_ms > 60_000
        || polling.refresh_interval_ms == 0
        || polling.refresh_interval_ms > 60_000
        || polling.discovery_refresh_ms <= 0
        || polling.discovery_lookback_ms < 0
        || polling.discovery_lookahead_ms <= 0
        || polling.maximum_response_bytes == 0
        || polling.maximum_response_bytes > 8 * 1024 * 1024
    {
        return Err("configuration polling bounds are invalid".into());
    }
    let projection = &document.projection;
    if projection.maximum_book_age_ms <= 0
        || projection.maximum_reference_age_ms <= 0
        || projection.maximum_cross_book_skew_ms < 0
        || projection.maximum_projection_age_ms <= 0
    {
        return Err("configuration projection bounds are invalid".into());
    }
    let client = &document.client_display;
    if client.poll_interval_ms == 0
        || client.request_timeout_ms == 0
        || client.maximum_client_age_ms <= 0
        || client.maximum_future_skew_ms < 0
        || client.request_timeout_ms > 60_000
    {
        return Err("configuration client display bounds are invalid".into());
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push(char::from(TABLE[usize::from(byte >> 4)]));
        rendered.push(char::from(TABLE[usize::from(byte & 0x0f)]));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{safe_defaults, validate, RuntimeConfigurationDocument};

    #[test]
    fn compiled_defaults_remain_within_all_bounds() {
        let defaults = safe_defaults();
        let document = RuntimeConfigurationDocument {
            schema_version: 1,
            config_id: "test".into(),
            issued_at_ms: 0,
            expires_at_ms: i64::MAX,
            sources: defaults.sources,
            polling: defaults.polling,
            projection: defaults.projection,
            client_display: defaults.client_display,
        };
        assert!(validate(&document, 1).is_ok());
    }
}
