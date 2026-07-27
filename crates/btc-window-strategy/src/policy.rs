//! Versioned window policy for paper setups (Spec 096 / ADR 0100).
//!
//! Phase 4: all eight window setups are digest-bound. Missing active policy →
//! compiled defaults identical to pre-policy hardcodes. Older Phase-2 documents
//! that only contain `5m_late_continuation` deserialize with defaults for the
//! remaining setups; digest mismatch after expansion rewrites `active.json`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{create_dir_all, read_to_string, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Plugin type registered for GUI discovery.
pub const PLUGIN_TYPE_WINDOW_SETUP: &str = "window_setup";

/// Setup id for the original Phase 2 slice (kept for exports / tests).
pub const LATE_CONTINUATION_SETUP_ID: &str = "5m_late_continuation";

/// Shared entry gates for one window setup (integer micros / seconds only).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct SetupGateParams {
    pub enabled: bool,
    pub entry_elapsed_lo_secs: i64,
    pub entry_elapsed_hi_secs: i64,
    /// Second window (both 0 = unused). Used by sequential hedge seed.
    #[serde(default)]
    pub entry_elapsed_lo2_secs: i64,
    #[serde(default)]
    pub entry_elapsed_hi2_secs: i64,
    pub min_confidence_micros: i64,
    /// 0 = no upper confidence band (min-only).
    #[serde(default)]
    pub max_confidence_micros: i64,
    /// Min confidence when second entry window matches. 0 = use `min_confidence_micros`.
    #[serde(default)]
    pub min_confidence_window2_micros: i64,
    pub max_ask_override_micros: i64,
    #[serde(default)]
    pub min_persistence_secs: i64,
    /// If > 0: require `persistence_secs` >= elapsed − offset.
    #[serde(default)]
    pub persistence_since_offset_secs: i64,
    #[serde(default)]
    pub require_checkpoint_favourite: bool,
    #[serde(default)]
    pub require_positive_mom_med: bool,
    #[serde(default)]
    pub require_non_negative_mom: bool,
    #[serde(default)]
    pub require_mom_sign_agreement: bool,
    /// 0 = disabled.
    #[serde(default)]
    pub min_confidence_delta_60s_micros: i64,
    #[serde(default)]
    pub require_reference_agrees: bool,
    #[serde(default)]
    pub force_shadow_book: bool,
    /// If > 0 and ask above this, demote book to shadow.
    #[serde(default)]
    pub shadow_ask_threshold_micros: i64,
}

impl SetupGateParams {
    /// Pre-policy hardcodes for `5m_late_continuation` (alias for tests / CLI).
    #[must_use]
    pub const fn compiled_defaults() -> Self {
        Self::late_continuation()
    }

    #[must_use]
    pub fn validate(&self) -> Vec<String> {
        self.validate_named(LATE_CONTINUATION_SETUP_ID, 300)
    }

    fn validate_named(&self, setup_id: &str, max_elapsed: i64) -> Vec<String> {
        let mut errors = Vec::new();
        let tag = |msg: &str| format!("{setup_id}: {msg}");
        if self.entry_elapsed_hi_secs < self.entry_elapsed_lo_secs {
            errors.push(tag(
                "entry_elapsed_hi_secs must be >= entry_elapsed_lo_secs",
            ));
        }
        if self.entry_elapsed_lo_secs < 0 || self.entry_elapsed_hi_secs > max_elapsed {
            errors.push(tag(&format!(
                "entry window must be within 0..={max_elapsed}"
            )));
        }
        if self.entry_elapsed_lo2_secs != 0 || self.entry_elapsed_hi2_secs != 0 {
            if self.entry_elapsed_hi2_secs < self.entry_elapsed_lo2_secs {
                errors.push(tag(
                    "entry_elapsed_hi2_secs must be >= entry_elapsed_lo2_secs",
                ));
            }
            if self.entry_elapsed_lo2_secs < 0 || self.entry_elapsed_hi2_secs > max_elapsed {
                errors.push(tag(&format!(
                    "second entry window must be within 0..={max_elapsed}"
                )));
            }
        }
        if self.min_confidence_micros < 0 || self.min_confidence_micros > 1_000_000 {
            errors.push(tag("min_confidence_micros must be within 0..=1_000_000"));
        }
        if self.max_confidence_micros != 0 {
            if self.max_confidence_micros > 1_000_000 {
                errors.push(tag("max_confidence_micros must be <= 1_000_000"));
            }
            if self.max_confidence_micros < self.min_confidence_micros {
                errors.push(tag(
                    "max_confidence_micros must be >= min_confidence_micros",
                ));
            }
        }
        if self.max_ask_override_micros < 0 || self.max_ask_override_micros > 1_000_000 {
            errors.push(tag("max_ask_override_micros must be within 0..=1_000_000"));
        }
        errors
    }

    /// Pre-policy hardcodes for `5m_late_continuation`.
    #[must_use]
    pub const fn late_continuation() -> Self {
        Self {
            enabled: true,
            entry_elapsed_lo_secs: 230,
            entry_elapsed_hi_secs: 245,
            entry_elapsed_lo2_secs: 0,
            entry_elapsed_hi2_secs: 0,
            min_confidence_micros: 600_000,
            max_confidence_micros: 820_000,
            min_confidence_window2_micros: 0,
            max_ask_override_micros: 820_000,
            min_persistence_secs: 0,
            persistence_since_offset_secs: 0,
            require_checkpoint_favourite: false,
            require_positive_mom_med: true,
            require_non_negative_mom: false,
            require_mom_sign_agreement: false,
            min_confidence_delta_60s_micros: 0,
            require_reference_agrees: false,
            force_shadow_book: false,
            shadow_ask_threshold_micros: 0,
        }
    }

    #[must_use]
    pub const fn mid_window() -> Self {
        Self {
            enabled: true,
            entry_elapsed_lo_secs: 135,
            entry_elapsed_hi_secs: 180,
            entry_elapsed_lo2_secs: 0,
            entry_elapsed_hi2_secs: 0,
            min_confidence_micros: 650_000,
            max_confidence_micros: 0,
            min_confidence_window2_micros: 0,
            max_ask_override_micros: 820_000,
            min_persistence_secs: 20,
            persistence_since_offset_secs: 0,
            require_checkpoint_favourite: false,
            require_positive_mom_med: false,
            require_non_negative_mom: true,
            require_mom_sign_agreement: false,
            min_confidence_delta_60s_micros: 0,
            require_reference_agrees: false,
            force_shadow_book: false,
            shadow_ask_threshold_micros: 0,
        }
    }

    #[must_use]
    pub const fn high_confidence() -> Self {
        Self {
            enabled: true,
            entry_elapsed_lo_secs: 150,
            entry_elapsed_hi_secs: 165,
            entry_elapsed_lo2_secs: 0,
            entry_elapsed_hi2_secs: 0,
            min_confidence_micros: 750_000,
            max_confidence_micros: 0,
            min_confidence_window2_micros: 0,
            max_ask_override_micros: 860_000,
            min_persistence_secs: 0,
            persistence_since_offset_secs: 120,
            require_checkpoint_favourite: false,
            require_positive_mom_med: false,
            require_non_negative_mom: true,
            require_mom_sign_agreement: false,
            min_confidence_delta_60s_micros: 0,
            require_reference_agrees: false,
            force_shadow_book: true,
            shadow_ask_threshold_micros: 0,
        }
    }

    #[must_use]
    pub const fn fifteen_persistent() -> Self {
        Self {
            enabled: true,
            entry_elapsed_lo_secs: 220,
            entry_elapsed_hi_secs: 260,
            entry_elapsed_lo2_secs: 0,
            entry_elapsed_hi2_secs: 0,
            min_confidence_micros: 600_000,
            max_confidence_micros: 0,
            min_confidence_window2_micros: 0,
            max_ask_override_micros: 840_000,
            min_persistence_secs: 30,
            persistence_since_offset_secs: 0,
            require_checkpoint_favourite: true,
            require_positive_mom_med: false,
            require_non_negative_mom: false,
            require_mom_sign_agreement: false,
            min_confidence_delta_60s_micros: 0,
            require_reference_agrees: false,
            force_shadow_book: false,
            shadow_ask_threshold_micros: 0,
        }
    }

    #[must_use]
    pub const fn fifteen_acceleration() -> Self {
        Self {
            enabled: true,
            entry_elapsed_lo_secs: 400,
            entry_elapsed_hi_secs: 440,
            entry_elapsed_lo2_secs: 0,
            entry_elapsed_hi2_secs: 0,
            min_confidence_micros: 600_000,
            max_confidence_micros: 0,
            min_confidence_window2_micros: 0,
            max_ask_override_micros: 840_000,
            min_persistence_secs: 60,
            persistence_since_offset_secs: 0,
            require_checkpoint_favourite: false,
            require_positive_mom_med: false,
            require_non_negative_mom: false,
            require_mom_sign_agreement: false,
            min_confidence_delta_60s_micros: 50_000,
            require_reference_agrees: true,
            force_shadow_book: false,
            shadow_ask_threshold_micros: 0,
        }
    }

    #[must_use]
    pub const fn fifteen_continuation() -> Self {
        Self {
            enabled: true,
            entry_elapsed_lo_secs: 640,
            entry_elapsed_hi_secs: 680,
            entry_elapsed_lo2_secs: 0,
            entry_elapsed_hi2_secs: 0,
            min_confidence_micros: 650_000,
            max_confidence_micros: 0,
            min_confidence_window2_micros: 0,
            max_ask_override_micros: 860_000,
            min_persistence_secs: 0,
            persistence_since_offset_secs: 600,
            require_checkpoint_favourite: false,
            require_positive_mom_med: false,
            require_non_negative_mom: false,
            require_mom_sign_agreement: false,
            min_confidence_delta_60s_micros: 20_000,
            require_reference_agrees: false,
            force_shadow_book: false,
            shadow_ask_threshold_micros: 800_000,
        }
    }

    #[must_use]
    pub const fn one_hour_minute50() -> Self {
        Self {
            enabled: true,
            entry_elapsed_lo_secs: 2_940,
            entry_elapsed_hi_secs: 3_060,
            entry_elapsed_lo2_secs: 0,
            entry_elapsed_hi2_secs: 0,
            min_confidence_micros: 600_000,
            max_confidence_micros: 0,
            min_confidence_window2_micros: 0,
            max_ask_override_micros: 880_000,
            min_persistence_secs: 300,
            persistence_since_offset_secs: 0,
            require_checkpoint_favourite: false,
            require_positive_mom_med: false,
            require_non_negative_mom: false,
            require_mom_sign_agreement: true,
            min_confidence_delta_60s_micros: 0,
            require_reference_agrees: false,
            force_shadow_book: true,
            shadow_ask_threshold_micros: 0,
        }
    }

    #[must_use]
    pub const fn one_hour_seq_hedge() -> Self {
        Self {
            enabled: true,
            entry_elapsed_lo_secs: 880,
            entry_elapsed_hi_secs: 920,
            entry_elapsed_lo2_secs: 2_380,
            entry_elapsed_hi2_secs: 2_420,
            min_confidence_micros: 600_000,
            max_confidence_micros: 0,
            min_confidence_window2_micros: 700_000,
            max_ask_override_micros: 880_000,
            min_persistence_secs: 0,
            persistence_since_offset_secs: 0,
            require_checkpoint_favourite: false,
            require_positive_mom_med: false,
            require_non_negative_mom: false,
            require_mom_sign_agreement: false,
            min_confidence_delta_60s_micros: 0,
            require_reference_agrees: false,
            force_shadow_book: true,
            shadow_ask_threshold_micros: 0,
        }
    }
}

/// Alias kept for older call sites / tests.
pub type LateContinuationParams = SetupGateParams;

/// All paper setup gates. Missing keys in older files fill via `#[serde(default)]`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupPolicyBag {
    #[serde(
        rename = "5m_late_continuation",
        default = "SetupGateParams::late_continuation"
    )]
    pub late_continuation: SetupGateParams,
    #[serde(
        rename = "5m_confirmed_mid_window_favourite",
        default = "SetupGateParams::mid_window"
    )]
    pub mid_window: SetupGateParams,
    #[serde(
        rename = "5m_high_confidence_confirmation",
        default = "SetupGateParams::high_confidence"
    )]
    pub high_confidence: SetupGateParams,
    #[serde(
        rename = "15m_four_minute_persistent_favourite",
        default = "SetupGateParams::fifteen_persistent"
    )]
    pub fifteen_persistent: SetupGateParams,
    #[serde(
        rename = "15m_seven_minute_acceleration",
        default = "SetupGateParams::fifteen_acceleration"
    )]
    pub fifteen_acceleration: SetupGateParams,
    #[serde(
        rename = "15m_eleven_minute_continuation",
        default = "SetupGateParams::fifteen_continuation"
    )]
    pub fifteen_continuation: SetupGateParams,
    #[serde(
        rename = "1h_minute50_confirmed_favourite",
        default = "SetupGateParams::one_hour_minute50"
    )]
    pub one_hour_minute50: SetupGateParams,
    #[serde(
        rename = "1h_experimental_sequential_hedge_seed",
        default = "SetupGateParams::one_hour_seq_hedge"
    )]
    pub one_hour_seq_hedge: SetupGateParams,
}

impl SetupPolicyBag {
    #[must_use]
    pub const fn compiled_defaults() -> Self {
        Self {
            late_continuation: SetupGateParams::late_continuation(),
            mid_window: SetupGateParams::mid_window(),
            high_confidence: SetupGateParams::high_confidence(),
            fifteen_persistent: SetupGateParams::fifteen_persistent(),
            fifteen_acceleration: SetupGateParams::fifteen_acceleration(),
            fifteen_continuation: SetupGateParams::fifteen_continuation(),
            one_hour_minute50: SetupGateParams::one_hour_minute50(),
            one_hour_seq_hedge: SetupGateParams::one_hour_seq_hedge(),
        }
    }

    #[must_use]
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        errors.extend(
            self.late_continuation
                .validate_named("5m_late_continuation", 300),
        );
        errors.extend(
            self.mid_window
                .validate_named("5m_confirmed_mid_window_favourite", 300),
        );
        errors.extend(
            self.high_confidence
                .validate_named("5m_high_confidence_confirmation", 300),
        );
        errors.extend(
            self.fifteen_persistent
                .validate_named("15m_four_minute_persistent_favourite", 900),
        );
        errors.extend(
            self.fifteen_acceleration
                .validate_named("15m_seven_minute_acceleration", 900),
        );
        errors.extend(
            self.fifteen_continuation
                .validate_named("15m_eleven_minute_continuation", 900),
        );
        errors.extend(
            self.one_hour_minute50
                .validate_named("1h_minute50_confirmed_favourite", 3_600),
        );
        errors.extend(
            self.one_hour_seq_hedge
                .validate_named("1h_experimental_sequential_hedge_seed", 3_600),
        );
        errors
    }

    #[must_use]
    pub fn get(&self, setup_id: &str) -> Option<&SetupGateParams> {
        match setup_id {
            "5m_late_continuation" => Some(&self.late_continuation),
            "5m_confirmed_mid_window_favourite" => Some(&self.mid_window),
            "5m_high_confidence_confirmation" => Some(&self.high_confidence),
            "15m_four_minute_persistent_favourite" => Some(&self.fifteen_persistent),
            "15m_seven_minute_acceleration" => Some(&self.fifteen_acceleration),
            "15m_eleven_minute_continuation" => Some(&self.fifteen_continuation),
            "1h_minute50_confirmed_favourite" => Some(&self.one_hour_minute50),
            "1h_experimental_sequential_hedge_seed" => Some(&self.one_hour_seq_hedge),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, setup_id: &str) -> Option<&mut SetupGateParams> {
        match setup_id {
            "5m_late_continuation" => Some(&mut self.late_continuation),
            "5m_confirmed_mid_window_favourite" => Some(&mut self.mid_window),
            "5m_high_confidence_confirmation" => Some(&mut self.high_confidence),
            "15m_four_minute_persistent_favourite" => Some(&mut self.fifteen_persistent),
            "15m_seven_minute_acceleration" => Some(&mut self.fifteen_acceleration),
            "15m_eleven_minute_continuation" => Some(&mut self.fifteen_continuation),
            "1h_minute50_confirmed_favourite" => Some(&mut self.one_hour_minute50),
            "1h_experimental_sequential_hedge_seed" => Some(&mut self.one_hour_seq_hedge),
            _ => None,
        }
    }
}

/// One immutable policy version document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowPolicyDocument {
    pub policy_id: String,
    pub version: u64,
    pub paper_only: bool,
    pub live_trading: bool,
    pub change_reason: String,
    pub setups: SetupPolicyBag,
}

impl WindowPolicyDocument {
    #[must_use]
    pub fn compiled_defaults() -> Self {
        Self {
            policy_id: "window_paper_policy".into(),
            version: 0,
            paper_only: true,
            live_trading: false,
            change_reason: "compiled_defaults".into(),
            setups: SetupPolicyBag::compiled_defaults(),
        }
    }

    #[must_use]
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if !self.paper_only {
            errors.push("paper_only must be true (Spec 096)".into());
        }
        if self.live_trading {
            errors.push("live_trading must be false (ADR 0100)".into());
        }
        errors.extend(self.setups.validate());
        errors
    }

    #[must_use]
    pub fn digest_hex(&self) -> String {
        let payload = serde_json::to_vec(self).unwrap_or_default();
        blake3::hash(&payload).to_hex().to_string()
    }
}

/// Runtime-resolved policy plus digest identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivePolicy {
    pub document: WindowPolicyDocument,
    pub digest_hex: String,
    pub source: PolicySource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicySource {
    CompiledDefaults,
    ActiveFile,
}

impl ActivePolicy {
    #[must_use]
    pub fn compiled_defaults() -> Self {
        let document = WindowPolicyDocument::compiled_defaults();
        let digest_hex = document.digest_hex();
        Self {
            document,
            digest_hex,
            source: PolicySource::CompiledDefaults,
        }
    }

    #[must_use]
    pub fn late_continuation(&self) -> &SetupGateParams {
        &self.document.setups.late_continuation
    }

    #[must_use]
    pub fn setup(&self, setup_id: &str) -> Option<&SetupGateParams> {
        self.document.setups.get(setup_id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivePointer {
    pub version: u64,
    pub digest: String,
    pub path: String,
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("policy validation failed: {0}")]
    Validation(String),
    #[error("corrupt policy at {path}: {message}")]
    Corrupt { path: String, message: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

fn param(name: &str, typ: &str, default: &Value, description: &str, extra: &Value) -> Value {
    let mut obj = json!({
        "name": name,
        "type": typ,
        "default": default,
        "description": description,
        "reload": "next_signal",
    });
    if let Some(map) = extra.as_object() {
        for (k, v) in map {
            obj[k] = v.clone();
        }
    }
    obj
}

fn gate_schema(
    setup_id: &str,
    title: &str,
    description: &str,
    defaults: &SetupGateParams,
) -> Value {
    let empty = json!({});
    let secs = json!({"unit":"seconds"});
    let micros = json!({"unit":"micros"});
    let conf = json!({"unit":"micros","minimum":0,"maximum":1_000_000,"risk_warning":"Lowering this admits weaker signals."});
    let conf_band = json!({"unit":"micros","minimum":0,"maximum":1_000_000});
    let ask = json!({"unit":"micros","minimum":0,"maximum":1_000_000,"risk_warning":"Raising ask caps increases adverse selection risk."});
    json!({
        "plugin_type": PLUGIN_TYPE_WINDOW_SETUP,
        "setup_id": setup_id,
        "version": "2",
        "title": title,
        "description": description,
        "paper_only": true,
        "parameters": [
            param("enabled", "boolean", &json!(defaults.enabled), "When false, this setup never proposes entries.", &empty),
            param("entry_elapsed_lo_secs", "integer", &json!(defaults.entry_elapsed_lo_secs), "Earliest elapsed second for entry.", &secs),
            param("entry_elapsed_hi_secs", "integer", &json!(defaults.entry_elapsed_hi_secs), "Latest elapsed second for entry.", &secs),
            param("entry_elapsed_lo2_secs", "integer", &json!(defaults.entry_elapsed_lo2_secs), "Optional second window start (0=unused).", &secs),
            param("entry_elapsed_hi2_secs", "integer", &json!(defaults.entry_elapsed_hi2_secs), "Optional second window end (0=unused).", &secs),
            param("min_confidence_micros", "integer", &json!(defaults.min_confidence_micros), "Minimum favourite confidence.", &conf),
            param("max_confidence_micros", "integer", &json!(defaults.max_confidence_micros), "Max confidence (0=no upper band).", &conf_band),
            param("min_confidence_window2_micros", "integer", &json!(defaults.min_confidence_window2_micros), "Min confidence in second window (0=use primary).", &micros),
            param("max_ask_override_micros", "integer", &json!(defaults.max_ask_override_micros), "Hard ask cap.", &ask),
            param("min_persistence_secs", "integer", &json!(defaults.min_persistence_secs), "Minimum favourite persistence.", &secs),
            param("persistence_since_offset_secs", "integer", &json!(defaults.persistence_since_offset_secs), "Require persistence >= elapsed − offset (0=off).", &secs),
            param("require_checkpoint_favourite", "boolean", &json!(defaults.require_checkpoint_favourite), "Require checkpoint favourite match.", &empty),
            param("require_positive_mom_med", "boolean", &json!(defaults.require_positive_mom_med), "Require medium momentum > 0.", &empty),
            param("require_non_negative_mom", "boolean", &json!(defaults.require_non_negative_mom), "Require non-negative medium/long momentum.", &empty),
            param("require_mom_sign_agreement", "boolean", &json!(defaults.require_mom_sign_agreement), "Require medium/long momentum sign agreement.", &empty),
            param("min_confidence_delta_60s_micros", "integer", &json!(defaults.min_confidence_delta_60s_micros), "Minimum 60s confidence acceleration (0=off).", &micros),
            param("require_reference_agrees", "boolean", &json!(defaults.require_reference_agrees), "Require reference agrees with favourite.", &empty),
            param("force_shadow_book", "boolean", &json!(defaults.force_shadow_book), "Always attribute to SHADOW_EXPERIMENT.", &empty),
            param("shadow_ask_threshold_micros", "integer", &json!(defaults.shadow_ask_threshold_micros), "Demote to shadow when ask above this (0=off).", &micros),
        ]
    })
}

/// JSON Schema-ish descriptor for Strategy Builder.
#[must_use]
pub fn late_continuation_parameter_schema() -> Value {
    gate_schema(
        LATE_CONTINUATION_SETUP_ID,
        "5m late continuation",
        "Late-window favourite continuation on 5m markets.",
        &SetupGateParams::late_continuation(),
    )
}

#[must_use]
pub fn list_plugins() -> Value {
    let bag = SetupPolicyBag::compiled_defaults();
    json!([
        gate_schema(
            "5m_late_continuation",
            "5m late continuation",
            "Late-window favourite continuation.",
            &bag.late_continuation
        ),
        gate_schema(
            "5m_confirmed_mid_window_favourite",
            "5m mid-window favourite",
            "Confirmed mid-window favourite.",
            &bag.mid_window
        ),
        gate_schema(
            "5m_high_confidence_confirmation",
            "5m high confidence",
            "High-confidence confirmation (shadow).",
            &bag.high_confidence
        ),
        gate_schema(
            "15m_four_minute_persistent_favourite",
            "15m persistent favourite",
            "Four-minute persistent favourite.",
            &bag.fifteen_persistent
        ),
        gate_schema(
            "15m_seven_minute_acceleration",
            "15m acceleration",
            "Seven-minute acceleration.",
            &bag.fifteen_acceleration
        ),
        gate_schema(
            "15m_eleven_minute_continuation",
            "15m continuation",
            "Eleven-minute continuation.",
            &bag.fifteen_continuation
        ),
        gate_schema(
            "1h_minute50_confirmed_favourite",
            "1h minute-50 favourite",
            "Minute-50 confirmed favourite (shadow).",
            &bag.one_hour_minute50
        ),
        gate_schema(
            "1h_experimental_sequential_hedge_seed",
            "1h sequential hedge seed",
            "Experimental sequential hedge seed (shadow).",
            &bag.one_hour_seq_hedge
        ),
    ])
}

/// On-disk policy store under `var/strategy-control/`.
#[derive(Clone, Debug)]
pub struct PolicyStore {
    root: PathBuf,
}

impl PolicyStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    fn active_path(&self) -> PathBuf {
        self.root.join("active.json")
    }

    fn audit_path(&self) -> PathBuf {
        self.root.join("audit.jsonl")
    }

    /// Load active policy. Missing active → compiled defaults (Ok).
    /// Corrupt active/version → Err (fail closed).
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::Corrupt`] when the active pointer or version file
    /// is unreadable, invalid JSON, fails validation, or cannot be migrated.
    pub fn load_active(&self) -> Result<ActivePolicy, PolicyError> {
        let active_path = self.active_path();
        if !active_path.exists() {
            return Ok(ActivePolicy::compiled_defaults());
        }
        let raw = read_to_string(&active_path).map_err(|e| PolicyError::Corrupt {
            path: active_path.display().to_string(),
            message: e.to_string(),
        })?;
        let pointer: ActivePointer =
            serde_json::from_str(&raw).map_err(|e| PolicyError::Corrupt {
                path: active_path.display().to_string(),
                message: e.to_string(),
            })?;
        let version_path = if Path::new(&pointer.path).is_absolute() {
            PathBuf::from(&pointer.path)
        } else {
            self.root.join(&pointer.path)
        };
        let doc_raw = read_to_string(&version_path).map_err(|e| PolicyError::Corrupt {
            path: version_path.display().to_string(),
            message: e.to_string(),
        })?;
        let document: WindowPolicyDocument =
            serde_json::from_str(&doc_raw).map_err(|e| PolicyError::Corrupt {
                path: version_path.display().to_string(),
                message: e.to_string(),
            })?;
        let errors = document.validate();
        if !errors.is_empty() {
            return Err(PolicyError::Corrupt {
                path: version_path.display().to_string(),
                message: errors.join("; "),
            });
        }
        let digest_hex = document.digest_hex();
        if digest_hex != pointer.digest {
            // Phase-4 expansion: older Phase-2 files gain defaulted setups; rewrite pointer.
            let migrated = ActivePointer {
                version: pointer.version,
                digest: digest_hex.clone(),
                path: pointer.path.clone(),
            };
            atomic_write(&self.active_path(), &serde_json::to_vec_pretty(&migrated)?)?;
            append_audit(
                &self.audit_path(),
                &json!({
                    "event": "migrate_digest",
                    "version": migrated.version,
                    "old_digest": pointer.digest,
                    "digest": migrated.digest,
                }),
            )?;
        }
        Ok(ActivePolicy {
            document,
            digest_hex,
            source: PolicySource::ActiveFile,
        })
    }

    /// Persist a new immutable version (does not activate).
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::Validation`] when the document is illegal, or
    /// [`PolicyError::Io`] / [`PolicyError::Json`] on write failures.
    pub fn save_version(
        &self,
        mut document: WindowPolicyDocument,
    ) -> Result<ActivePointer, PolicyError> {
        let errors = document.validate();
        if !errors.is_empty() {
            return Err(PolicyError::Validation(errors.join("; ")));
        }
        create_dir_all(self.versions_dir())?;
        if document.version == 0 {
            document.version = next_version(&self.versions_dir())?;
        }
        let digest = document.digest_hex();
        let rel = format!("versions/v{:04}.json", document.version);
        let path = self.root.join(&rel);
        let body = serde_json::to_vec_pretty(&document)?;
        atomic_write(&path, &body)?;
        let pointer = ActivePointer {
            version: document.version,
            digest,
            path: rel,
        };
        append_audit(
            &self.audit_path(),
            &json!({
                "event": "save_version",
                "version": pointer.version,
                "digest": pointer.digest,
                "change_reason": document.change_reason,
            }),
        )?;
        Ok(pointer)
    }

    /// Activate an existing version path/digest after re-validation.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::Validation`] or [`PolicyError::Corrupt`] when the
    /// version cannot be loaded or is illegal; [`PolicyError::Io`] on write.
    pub fn activate(&self, version: u64) -> Result<ActivePolicy, PolicyError> {
        let rel = format!("versions/v{version:04}.json");
        let path = self.root.join(&rel);
        let doc_raw = read_to_string(&path).map_err(|e| PolicyError::Corrupt {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        let document: WindowPolicyDocument = serde_json::from_str(&doc_raw)?;
        let errors = document.validate();
        if !errors.is_empty() {
            return Err(PolicyError::Validation(errors.join("; ")));
        }
        let digest = document.digest_hex();
        let pointer = ActivePointer {
            version: document.version,
            digest: digest.clone(),
            path: rel,
        };
        create_dir_all(&self.root)?;
        atomic_write(&self.active_path(), &serde_json::to_vec_pretty(&pointer)?)?;
        append_audit(
            &self.audit_path(),
            &json!({
                "event": "activate",
                "version": pointer.version,
                "digest": pointer.digest,
            }),
        )?;
        Ok(ActivePolicy {
            document,
            digest_hex: digest,
            source: PolicySource::ActiveFile,
        })
    }
}

fn next_version(versions_dir: &Path) -> Result<u64, PolicyError> {
    let mut max = 0_u64;
    if versions_dir.exists() {
        for entry in std::fs::read_dir(versions_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(num) = name
                .strip_prefix('v')
                .and_then(|s| s.strip_suffix(".json"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                max = max.max(num);
            }
        }
    }
    Ok(max.saturating_add(1))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), PolicyError> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn append_audit(path: &Path, event: &Value) -> Result<(), PolicyError> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(event)?)?;
    Ok(())
}
