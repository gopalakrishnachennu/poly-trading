#![forbid(unsafe_code)]

use crate::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, VecDeque},
    env,
    fs::{create_dir_all, read_dir, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use terminal_projection::AssetProjection;

const MICROS_PER_UNIT: i64 = 1_000_000;
const MAX_TRADES: usize = 500;
const MAX_JOURNAL_BYTES: u64 = 512 * 1024 * 1024;
const PAPER_WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const PAPER_POLICY_PATH_ENV: &str = "POLY_PAPER_POLICY_PATH";
const MAX_POLICY_BYTES: u64 = 64 * 1024;
const REPORT_CACHE_TTL_MS: i64 = 15_000;
static PREFLIGHT_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Immutable, operator-supplied economics for a paper campaign. Amounts stay
/// decimal strings at the configuration boundary so no binary floating point
/// can enter a financial decision.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PaperMarketPolicy {
    schema_version: u16,
    policy_id: String,
    issued_at_ms: i64,
    expires_at_ms: i64,
    campaign_duration_ms: i64,
    assets: BTreeMap<String, PaperAssetPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // Names mirror the audited policy schema.
struct PaperAssetPolicy {
    fee_micros: String,
    slippage_micros: String,
    minimum_locked_edge_micros: String,
    maximum_pair_quantity_micros: String,
}

#[derive(Clone, Debug)]
struct ValidatedPolicy {
    policy_id: String,
    digest: String,
    campaign_duration_ms: i64,
    assets: BTreeMap<String, ValidatedAssetPolicy>,
}

#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_field_names)] // Keep unit names explicit at the financial boundary.
struct ValidatedAssetPolicy {
    fee_micros: i64,
    slippage_micros: i64,
    minimum_locked_edge_micros: i64,
    maximum_pair_quantity_micros: i64,
}

impl PaperMarketPolicy {
    fn validate(self, now_ms: i64) -> Result<ValidatedPolicy, String> {
        if self.schema_version != 1 {
            return Err("unsupported paper market policy schema".into());
        }
        if self.policy_id.trim().is_empty() || self.policy_id.len() > 128 {
            return Err("paper market policy ID is invalid".into());
        }
        if self.issued_at_ms < 0 || self.expires_at_ms <= self.issued_at_ms {
            return Err("paper market policy validity interval is invalid".into());
        }
        if !(60 * 60 * 1_000..=31 * PAPER_WEEK_MS).contains(&self.campaign_duration_ms) {
            return Err("paper market policy campaign duration is outside approved bounds".into());
        }
        if now_ms < self.issued_at_ms || now_ms >= self.expires_at_ms {
            return Err("paper market policy is not currently valid".into());
        }
        if self.assets.is_empty() || self.assets.len() > 8 {
            return Err("paper market policy must permit one to eight assets".into());
        }
        let canonical = serde_json::to_vec(&self)
            .map_err(|error| format!("paper market policy canonicalization failed: {error}"))?;
        let mut assets = BTreeMap::new();
        for (raw_asset, economics) in self.assets {
            let asset = normalized_asset(&raw_asset)?;
            if assets.contains_key(&asset) {
                return Err("paper market policy has duplicate asset".into());
            }
            let fee_micros = parse_amount(&economics.fee_micros)?;
            let slippage_micros = parse_amount(&economics.slippage_micros)?;
            let minimum_locked_edge_micros = parse_amount(&economics.minimum_locked_edge_micros)?;
            let maximum_pair_quantity_micros =
                parse_amount(&economics.maximum_pair_quantity_micros)?;
            if minimum_locked_edge_micros >= MICROS_PER_UNIT {
                return Err(
                    "paper market policy minimum edge must be below one payout unit".into(),
                );
            }
            if maximum_pair_quantity_micros <= 0 {
                return Err("paper market policy maximum pair quantity must be positive".into());
            }
            assets.insert(
                asset,
                ValidatedAssetPolicy {
                    fee_micros,
                    slippage_micros,
                    minimum_locked_edge_micros,
                    maximum_pair_quantity_micros,
                },
            );
        }
        Ok(ValidatedPolicy {
            policy_id: self.policy_id,
            digest: to_hex(blake3::hash(&canonical).as_bytes()),
            campaign_duration_ms: self.campaign_duration_ms,
            assets,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartPaperRequest {
    pub principal_micros: String,
    pub backup_micros: String,
    pub contracts: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PaperStatus {
    pub paper_only: bool,
    pub active: bool,
    pub session_id: Option<String>,
    pub started_at_ms: Option<i64>,
    pub stopped_at_ms: Option<i64>,
    pub deadline_at_ms: Option<i64>,
    pub principal_micros: String,
    pub backup_micros: String,
    pub available_cash_micros: String,
    pub reserved_micros: String,
    pub realized_pnl_micros: String,
    pub locked_pnl_micros: String,
    pub unrealized_pnl_micros: String,
    pub max_drawdown_micros: String,
    pub cvar_micros: String,
    pub hedge_failures: u64,
    pub fill_rate_bps: u64,
    pub data_coverage_bps: u64,
    pub events_recorded: u64,
    pub decisions_recorded: u64,
    pub checkpoints: u64,
    pub last_checkpoint_ms: Option<i64>,
    pub replay_digest: String,
    pub journal_path: Option<String>,
    pub last_error: Option<String>,
    pub policy_status: String,
    pub policy_id: Option<String>,
    pub policy_digest: Option<String>,
    pub runtime_config_id: Option<String>,
    pub runtime_config_digest: Option<String>,
    pub contracts: Vec<ContractStatus>,
    pub trades: Vec<PaperTrade>,
    pub daily_rollups: Vec<DailyRollup>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PaperReport {
    pub paper_only: bool,
    pub campaign_id: Option<String>,
    pub replay_verified: bool,
    pub journal_records: u64,
    pub final_digest: String,
    pub net_pnl_micros: String,
    pub data_coverage_bps: u64,
    pub verified_at_ms: i64,
    pub gates: BTreeMap<String, bool>,
    pub reason: String,
}

/// Read-only readiness result shown before an operator can start a new paper
/// campaign.  It is deliberately separate from market readiness: a healthy
/// public feed must never silently create a recorder or mutate paper capital.
#[derive(Clone, Debug, Serialize)]
pub struct PaperPreflight {
    pub eligible: bool,
    pub checked_at_ms: i64,
    pub gates: BTreeMap<String, bool>,
    pub policy_id: Option<String>,
    pub policy_digest: Option<String>,
    pub journal_directory: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContractStatus {
    pub asset: String,
    pub active: bool,
    pub observations: u64,
    pub no_trade: u64,
    pub fills: u64,
    pub realized_pnl_micros: String,
    pub last_decision: String,
    pub last_reason: String,
    pub last_event_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PaperTrade {
    pub trade_id: String,
    pub asset: String,
    pub condition_id: String,
    pub state: String,
    pub quantity_micros: String,
    pub up_price_micros: String,
    pub down_price_micros: String,
    pub fee_micros: String,
    pub slippage_micros: String,
    pub cost_micros: String,
    pub locked_pnl_micros: String,
    pub decision_at_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DailyRollup {
    pub day_utc: String,
    pub events: u64,
    pub decisions: u64,
    pub fills: u64,
    pub no_trade: u64,
    pub realized_pnl_micros: String,
    pub fees_micros: String,
    pub drawdown_micros: String,
}

#[derive(Debug)]
pub struct PaperController {
    state: PaperState,
    report_cache: Mutex<Option<PaperReport>>,
}

#[derive(Debug)]
struct PaperState {
    active: bool,
    session_id: Option<String>,
    started_at_ms: Option<i64>,
    stopped_at_ms: Option<i64>,
    campaign_duration_ms: i64,
    principal: i64,
    backup: i64,
    cash: i64,
    reserved: i64,
    realized: i64,
    locked: i64,
    unrealized: i64,
    max_drawdown: i64,
    hedge_failures: u64,
    observations: u64,
    decisions: u64,
    fills: u64,
    events: u64,
    checkpoints: u64,
    last_checkpoint_ms: Option<i64>,
    journal_path: Option<PathBuf>,
    journal_digest: [u8; 32],
    last_error: Option<String>,
    policy_status: String,
    policy_id: Option<String>,
    policy_digest: Option<String>,
    campaign_policy: Option<ValidatedPolicy>,
    runtime_config_id: Option<String>,
    runtime_config_digest: Option<String>,
    contracts: BTreeMap<String, ContractState>,
    trades: VecDeque<PaperTrade>,
    daily: BTreeMap<String, RollupState>,
    last_condition: BTreeMap<String, String>,
    last_trade_condition: BTreeMap<String, String>,
}

#[derive(Debug)]
struct ContractState {
    active: bool,
    observations: u64,
    no_trade: u64,
    fills: u64,
    realized: i64,
    last_decision: String,
    last_reason: String,
    last_event_ms: Option<i64>,
}

#[derive(Debug, Default)]
struct RollupState {
    events: u64,
    decisions: u64,
    fills: u64,
    no_trade: u64,
    realized: i64,
    fees: i64,
    drawdown: i64,
}

impl Default for PaperController {
    fn default() -> Self {
        Self {
            state: PaperState {
                active: false,
                session_id: None,
                started_at_ms: None,
                stopped_at_ms: None,
                campaign_duration_ms: PAPER_WEEK_MS,
                principal: 0,
                backup: 0,
                cash: 0,
                reserved: 0,
                realized: 0,
                locked: 0,
                unrealized: 0,
                max_drawdown: 0,
                hedge_failures: 0,
                observations: 0,
                decisions: 0,
                fills: 0,
                events: 0,
                checkpoints: 0,
                last_checkpoint_ms: None,
                journal_path: None,
                journal_digest: [0; 32],
                last_error: None,
                policy_status: "UNCONFIGURED".into(),
                policy_id: None,
                policy_digest: None,
                campaign_policy: None,
                runtime_config_id: None,
                runtime_config_digest: None,
                contracts: BTreeMap::new(),
                trades: VecDeque::new(),
                daily: BTreeMap::new(),
                last_condition: BTreeMap::new(),
                last_trade_condition: BTreeMap::new(),
            },
            report_cache: Mutex::new(None),
        }
    }
}

impl PaperController {
    fn load_current_policy(now_ms: i64) -> Result<ValidatedPolicy, String> {
        let path = env::var(PAPER_POLICY_PATH_ENV)
            .map_err(|_| format!("{PAPER_POLICY_PATH_ENV} must name a validated paper policy"))?;
        let path = PathBuf::from(path);
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("paper market policy metadata failed: {error}"))?;
        if !metadata.is_file() || metadata.len() > MAX_POLICY_BYTES {
            return Err("paper market policy path is not a bounded regular file".into());
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("paper market policy read failed: {error}"))?;
        let policy: PaperMarketPolicy = serde_json::from_slice(&bytes)
            .map_err(|error| format!("paper market policy JSON invalid: {error}"))?;
        policy.validate(now_ms)
    }

    fn bind_recovered_policy(&mut self, now_ms: i64) {
        let (Some(policy_id), Some(policy_digest)) = (
            self.state.policy_id.as_deref(),
            self.state.policy_digest.as_deref(),
        ) else {
            self.state.policy_status = "LEGACY_UNBOUND_OBSERVATION_ONLY".into();
            self.state.last_error = Some(
                "recovered campaign has no immutable policy binding; simulated pairs disabled"
                    .into(),
            );
            return;
        };
        match Self::load_current_policy(now_ms) {
            Ok(policy) if policy.policy_id == policy_id && policy.digest == policy_digest => {
                self.state.campaign_policy = Some(policy);
                self.state.policy_status = "BOUND".into();
            }
            Ok(_) => {
                self.state.policy_status = "POLICY_MISMATCH_OBSERVATION_ONLY".into();
                self.state.last_error = Some(
                    "configured policy does not match recovered campaign binding; simulated pairs disabled"
                        .into(),
                );
            }
            Err(error) => {
                self.state.policy_status = "POLICY_UNAVAILABLE_OBSERVATION_ONLY".into();
                self.state.last_error = Some(format!(
                    "recovered campaign policy unavailable: {error}; simulated pairs disabled"
                ));
            }
        }
    }

    /// Rebuilds the most recent journaled paper campaign before accepting any
    /// new observations. A malformed or discontinuous journal is an error;
    /// recovery must never silently discard campaign evidence.
    #[allow(clippy::too_many_lines)]
    pub fn recover() -> Result<Self, String> {
        let Some(path) = latest_journal_path()? else {
            return Ok(Self::default());
        };
        let contents = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        if contents.is_empty() {
            return Err("paper journal is empty".into());
        }
        let mut controller = Self::default();
        let mut expected_sequence = 1_u64;
        let mut saw_start = false;
        for line in contents.lines() {
            let envelope: serde_json::Value = serde_json::from_str(line)
                .map_err(|error| format!("paper journal JSON invalid: {error}"))?;
            let record = envelope
                .get("record")
                .cloned()
                .ok_or("paper journal record missing")?;
            let digest = envelope
                .get("record_digest")
                .and_then(serde_json::Value::as_str)
                .ok_or("paper journal digest missing")?;
            let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
            if to_hex(blake3::hash(&bytes).as_bytes()) != digest {
                return Err("paper journal digest mismatch".into());
            }
            let sequence = record
                .get("sequence")
                .and_then(serde_json::Value::as_u64)
                .ok_or("paper journal sequence missing")?;
            if sequence != expected_sequence {
                return Err("paper journal sequence discontinuity".into());
            }
            expected_sequence = expected_sequence.saturating_add(1);
            controller.state.events = sequence;
            controller.state.journal_digest = *blake3::hash(&bytes).as_bytes();
            let event_time = record
                .get("event_time_ms")
                .and_then(serde_json::Value::as_i64)
                .ok_or("paper journal event timestamp missing")?;
            let kind = record
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .ok_or("paper journal kind missing")?;
            let payload = record
                .get("payload")
                .ok_or("paper journal payload missing")?;
            let record_runtime_config_id = record
                .get("runtime_config_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            let record_runtime_config_digest = record
                .get("runtime_config_digest")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            match kind {
                "paper_start" => {
                    if saw_start {
                        return Err("paper journal contains duplicate start".into());
                    }
                    saw_start = true;
                    controller.state.active = true;
                    controller.state.session_id = record
                        .get("campaign_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    controller.state.started_at_ms = Some(event_time);
                    controller.state.principal = payload_i64(payload, "principal_micros")?;
                    controller.state.backup = payload_i64(payload, "backup_micros")?;
                    controller.state.campaign_duration_ms = payload
                        .get("campaign_duration_ms")
                        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
                        .unwrap_or(PAPER_WEEK_MS);
                    controller.state.cash = controller.state.principal;
                    controller.state.policy_id = payload
                        .get("policy_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    controller.state.policy_digest = payload
                        .get("policy_digest")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    controller
                        .state
                        .runtime_config_id
                        .clone_from(&record_runtime_config_id);
                    controller
                        .state
                        .runtime_config_digest
                        .clone_from(&record_runtime_config_digest);
                }
                "observation" => {
                    let asset = payload_asset(payload)?;
                    let contract = controller.contract_mut(&asset);
                    contract.observations = contract.observations.saturating_add(1);
                    contract.last_event_ms = Some(event_time);
                    controller.state.observations = controller.state.observations.saturating_add(1);
                    controller
                        .state
                        .last_condition
                        .insert(asset, payload_string(payload, "condition_id")?.to_owned());
                }
                "PAIR_BUY" | "NO_TRADE" => {
                    let asset = payload_asset(payload)?;
                    let reason = payload_string(payload, "reason")?.to_owned();
                    let contract = controller.contract_mut(&asset);
                    contract.last_decision = kind.into();
                    contract.last_reason = reason;
                    contract.last_event_ms = Some(event_time);
                    if kind == "NO_TRADE" {
                        contract.no_trade = contract.no_trade.saturating_add(1);
                    }
                    controller.state.decisions = controller.state.decisions.saturating_add(1);
                }
                "filled_pair" => {
                    let trade: PaperTrade = serde_json::from_value(payload.clone())
                        .map_err(|error| format!("paper trade recovery invalid: {error}"))?;
                    let asset = trade.asset.clone();
                    controller.contract_mut(&asset).fills =
                        controller.contract_mut(&asset).fills.saturating_add(1);
                    controller
                        .state
                        .last_trade_condition
                        .insert(asset, trade.condition_id.clone());
                    controller.state.trades.push_back(trade);
                    controller.state.fills = controller.state.fills.saturating_add(1);
                }
                "conservation_checkpoint" => {
                    controller.state.cash = payload_i64(payload, "cash_micros")?;
                    controller.state.reserved = payload_i64(payload, "reserved_micros")?;
                    controller.state.locked = payload_i64(payload, "locked_pnl_micros")?;
                    controller.state.realized = payload_i64(payload, "realized_pnl_micros")?;
                }
                "checkpoint" => {
                    controller.state.checkpoints = controller.state.checkpoints.saturating_add(1);
                    controller.state.last_checkpoint_ms = Some(event_time);
                }
                "paper_stop" | "paper_week_complete" => {
                    controller.state.active = false;
                    controller.state.stopped_at_ms = Some(event_time);
                }
                "reservation_acquired"
                | "reservation_released"
                | "not_submitted"
                | "hour_rollover" => {}
                _ => return Err(format!("paper journal kind unsupported: {kind}")),
            }
            if kind != "paper_start"
                && (record_runtime_config_id != controller.state.runtime_config_id
                    || record_runtime_config_digest != controller.state.runtime_config_digest)
            {
                return Err("paper journal runtime configuration binding changed".into());
            }
        }
        if !saw_start || controller.state.session_id.is_none() {
            return Err("paper journal lacks campaign start identity".into());
        }
        controller.state.journal_path = Some(path);
        controller.bind_recovered_policy(now_ms().unwrap_or(i64::MAX));
        // Recovery is evidence recovery, not consent to continue capturing or
        // simulated execution.  A restarted process has no proof that the
        // operator still intends this campaign to run, so it must remain
        // stopped until a new explicit start after preflight.
        if controller.state.active {
            controller.state.active = false;
            controller.state.stopped_at_ms = None;
            controller.state.last_error = Some(
                "recovered active campaign is suspended; run preflight and explicitly start a new campaign"
                    .into(),
            );
        }
        if controller.state.started_at_ms.is_some_and(|started| {
            now_ms().unwrap_or(i64::MAX).saturating_sub(started)
                >= controller.state.campaign_duration_ms
        }) {
            controller.state.active = false;
            controller.state.stopped_at_ms = controller.state.stopped_at_ms.or(controller
                .state
                .started_at_ms
                .map(|started| started.saturating_add(controller.state.campaign_duration_ms)));
        }
        Ok(controller)
    }

    #[must_use]
    pub fn preflight(&self, now_ms: i64, runtime_configuration_bound: bool) -> PaperPreflight {
        let mut gates = BTreeMap::new();
        gates.insert("paper_only".into(), true);
        gates.insert("clock_valid".into(), now_ms >= 0);
        gates.insert(
            "runtime_configuration_bound".into(),
            runtime_configuration_bound,
        );
        gates.insert("no_active_campaign".into(), !self.state.active);
        let journal_directory = journal_directory();
        let journal_ready = validate_journal_directory(&journal_directory).is_ok();
        gates.insert("journal_directory_writable".into(), journal_ready);
        let policy = if now_ms >= 0 {
            Self::load_current_policy(now_ms).ok()
        } else {
            None
        };
        gates.insert("paper_policy_bound_and_current".into(), policy.is_some());
        gates.insert(
            "btc_eth_permitted".into(),
            policy.as_ref().is_some_and(|value| {
                value.assets.contains_key("BTC") && value.assets.contains_key("ETH")
            }),
        );
        let eligible = gates.values().all(|value| *value);
        let reason = if eligible {
            "preflight passed; an explicit operator start is required".into()
        } else {
            "preflight incomplete; campaign start remains blocked".into()
        };
        PaperPreflight {
            eligible,
            checked_at_ms: now_ms.max(0),
            gates,
            policy_id: policy.as_ref().map(|value| value.policy_id.clone()),
            policy_digest: policy.as_ref().map(|value| value.digest.clone()),
            journal_directory: journal_directory.display().to_string(),
            reason,
        }
    }

    fn contract_mut(&mut self, asset: &str) -> &mut ContractState {
        self.state
            .contracts
            .entry(asset.to_owned())
            .or_insert_with(|| ContractState {
                active: true,
                observations: 0,
                no_trade: 0,
                fills: 0,
                realized: 0,
                last_decision: "WAITING_DATA".into(),
                last_reason: "recovered; awaiting next observation".into(),
                last_event_ms: None,
            })
    }

    pub fn start(
        &mut self,
        request: StartPaperRequest,
        now_ms: i64,
        runtime_config_id: &str,
        runtime_config_digest: &str,
    ) -> Result<PaperStatus, String> {
        if now_ms < 0 {
            return Err("clock timestamp invalid".into());
        }
        if self.state.active {
            return Err("an active paper campaign must be stopped before another can start".into());
        }
        if runtime_config_id.trim().is_empty() || runtime_config_digest.len() != 64 {
            return Err("runtime configuration binding is invalid".into());
        }
        let preflight = self.preflight(now_ms, true);
        if !preflight.eligible {
            return Err("paper campaign preflight is incomplete".into());
        }
        let policy = Self::load_current_policy(now_ms)?;
        let principal = parse_amount(&request.principal_micros)?;
        let backup = parse_amount(&request.backup_micros)?;
        if principal <= 0 {
            return Err("principal must be positive".into());
        }
        if request.contracts.is_empty() || request.contracts.len() > 8 {
            return Err("one to eight contracts required".into());
        }
        let mut contracts = BTreeMap::new();
        for raw in request.contracts {
            let asset = normalized_asset(&raw)?;
            if !policy.assets.contains_key(&asset) {
                return Err(format!("paper policy does not permit contract {asset}"));
            }
            if contracts.contains_key(&asset) {
                return Err(format!(
                    "paper contract {asset} was requested more than once"
                ));
            }
            contracts.insert(
                asset,
                ContractState {
                    active: true,
                    observations: 0,
                    no_trade: 0,
                    fills: 0,
                    realized: 0,
                    last_decision: "WAITING_DATA".into(),
                    last_reason: "session started; awaiting validated feed".into(),
                    last_event_ms: None,
                },
            );
        }
        let session_id = format!("paper-{now_ms}");
        let journal_path = journal_path(&session_id)?;
        self.state = PaperState {
            active: true,
            session_id: Some(session_id),
            started_at_ms: Some(now_ms),
            stopped_at_ms: None,
            campaign_duration_ms: policy.campaign_duration_ms,
            principal,
            backup,
            cash: principal,
            reserved: 0,
            realized: 0,
            locked: 0,
            unrealized: 0,
            max_drawdown: 0,
            hedge_failures: 0,
            observations: 0,
            decisions: 0,
            fills: 0,
            events: 0,
            checkpoints: 0,
            last_checkpoint_ms: None,
            journal_path: Some(journal_path),
            journal_digest: [0; 32],
            last_error: None,
            policy_status: "BOUND".into(),
            policy_id: Some(policy.policy_id.clone()),
            policy_digest: Some(policy.digest.clone()),
            campaign_policy: Some(policy.clone()),
            runtime_config_id: Some(runtime_config_id.to_owned()),
            runtime_config_digest: Some(runtime_config_digest.to_owned()),
            contracts,
            trades: VecDeque::new(),
            daily: BTreeMap::new(),
            last_condition: BTreeMap::new(),
            last_trade_condition: BTreeMap::new(),
        };
        self.record(
            now_ms,
            "operator",
            "paper_start",
            &json!({"principal_micros": principal, "backup_micros": backup, "campaign_duration_ms": policy.campaign_duration_ms, "contracts": self.state.contracts.keys().collect::<Vec<_>>(), "policy_id": policy.policy_id, "policy_digest": policy.digest, "runtime_config_id": runtime_config_id, "runtime_config_digest": runtime_config_digest}),
        )?;
        Ok(self.status())
    }

    pub fn stop(&mut self, now_ms: i64) -> PaperStatus {
        self.state.active = false;
        self.state.stopped_at_ms = Some(now_ms.max(0));
        let _ = self.record(
            now_ms.max(0),
            "operator",
            "paper_stop",
            &json!({"reason": "operator_stop"}),
        );
        self.status()
    }

    #[allow(clippy::too_many_lines)]
    pub fn observe(&mut self, assets: &[AssetProjection], now_ms: i64) {
        if !self.state.active {
            return;
        }
        if self.state.started_at_ms.is_some_and(|started| {
            now_ms.saturating_sub(started) >= self.state.campaign_duration_ms
        }) {
            self.state.active = false;
            self.state.stopped_at_ms = Some(now_ms);
            let _ = self.record(
                now_ms,
                "operator",
                "paper_week_complete",
                &json!({"reason": "seven_day_deadline"}),
            );
            return;
        }
        for asset in assets {
            if !self.state.contracts.contains_key(&asset.asset) {
                continue;
            }
            self.state.observations = self.state.observations.saturating_add(1);
            let day = day_key(now_ms);
            self.state.daily.entry(day.clone()).or_default().events = self
                .state
                .daily
                .get(&day)
                .map_or(0, |r| r.events)
                .saturating_add(1);
            let _ = self.record(now_ms, "market", "observation", &json!({"asset": asset.asset, "condition_id": asset.condition_id, "reference_price_micros": asset.reference_price_micros, "target_price_micros": asset.target_price_micros, "up_best_bid_micros": asset.up_book.best_bid_micros, "up_best_ask_micros": asset.up_book.best_ask_micros, "down_best_bid_micros": asset.down_book.best_bid_micros, "down_best_ask_micros": asset.down_book.best_ask_micros, "feed_age_ms": asset.feed.age_ms}));
            if self
                .state
                .last_condition
                .get(&asset.asset)
                .is_some_and(|previous| previous != &asset.condition_id)
            {
                let previous_condition = self.state.last_condition.get(&asset.asset).cloned();
                let mut settled = 0_i64;
                for trade in &mut self.state.trades {
                    if trade.asset == asset.asset
                        && previous_condition.as_deref() == Some(trade.condition_id.as_str())
                        && trade.state == "FILLED_PAIR_LOCKED"
                    {
                        let quantity = parse_i64(&trade.quantity_micros).unwrap_or(0);
                        let locked = parse_i64(&trade.locked_pnl_micros).unwrap_or(0);
                        self.state.cash = self.state.cash.saturating_add(quantity);
                        self.state.realized = self.state.realized.saturating_add(locked);
                        self.state.locked = self.state.locked.saturating_sub(locked);
                        settled = settled.saturating_add(locked);
                        trade.state = "SETTLED_CONFIRMED".into();
                    }
                }
                if let Some(contract) = self.state.contracts.get_mut(&asset.asset) {
                    contract.realized = contract.realized.saturating_add(settled);
                }
                let rollup = self.state.daily.entry(day.clone()).or_default();
                rollup.realized = rollup.realized.saturating_add(settled);
                let _ = self.record(now_ms, "settlement", "hour_rollover", &json!({"asset": asset.asset, "previous_condition": previous_condition, "settled_pnl_micros": settled, "outcome": "complete_pair_payout"}));
            }
            self.state
                .last_condition
                .insert(asset.asset.clone(), asset.condition_id.clone());
            let up = parse_i64(&asset.up_book.best_ask_micros);
            let down = parse_i64(&asset.down_book.best_ask_micros);
            let qty = parse_i64(&asset.pair.executable_quantity_micros).unwrap_or(0);
            let pair_cost = up
                .and_then(|u| down.and_then(|d| u.checked_add(d)))
                .unwrap_or(MICROS_PER_UNIT);
            let affordable = self.state.cash.saturating_sub(self.state.reserved).max(0);
            let economics = self
                .state
                .campaign_policy
                .as_ref()
                .and_then(|policy| policy.assets.get(&asset.asset))
                .copied();
            let (fee, slippage, net, executable, decision, reason) = if let Some(economics) =
                economics
            {
                let net = pair_cost
                    .saturating_add(economics.fee_micros)
                    .saturating_add(economics.slippage_micros);
                let affordable_quantity = affordable
                    .checked_mul(MICROS_PER_UNIT)
                    .and_then(|value| value.checked_div(net.max(1)))
                    .unwrap_or(0);
                let executable = qty
                    .min(affordable_quantity)
                    .min(economics.maximum_pair_quantity_micros);
                let decision = if up.is_some()
                    && down.is_some()
                    && executable > 0
                    && net < MICROS_PER_UNIT.saturating_sub(economics.minimum_locked_edge_micros)
                {
                    "PAIR_BUY"
                } else {
                    "NO_TRADE"
                };
                let reason = if decision == "PAIR_BUY" {
                    "policy-bound executable pair below payout after configured costs"
                } else {
                    "no policy-qualified locked edge or executable feed quantity"
                };
                (
                    Some(economics.fee_micros),
                    Some(economics.slippage_micros),
                    Some(net),
                    executable,
                    decision,
                    reason,
                )
            } else {
                (
                    None,
                    None,
                    None,
                    0,
                    "NO_TRADE",
                    "campaign policy unavailable or unbound; observation only",
                )
            };
            self.state.decisions = self.state.decisions.saturating_add(1);
            if let Some(contract) = self.state.contracts.get_mut(&asset.asset) {
                contract.observations = contract.observations.saturating_add(1);
                contract.last_decision = decision.into();
                contract.last_reason = reason.into();
                contract.last_event_ms = Some(now_ms);
                if decision == "NO_TRADE" {
                    contract.no_trade = contract.no_trade.saturating_add(1);
                }
            }
            {
                let rollup = self.state.daily.entry(day.clone()).or_default();
                rollup.decisions = rollup.decisions.saturating_add(1);
                if decision == "NO_TRADE" {
                    rollup.no_trade = rollup.no_trade.saturating_add(1);
                }
            }
            let _ = self.record(now_ms, "decision", decision, &json!({"asset": asset.asset, "condition_id": asset.condition_id, "reason": reason, "pair_cost_micros": pair_cost, "fee_micros": fee, "slippage_micros": slippage, "net_cost_micros": net, "quantity_micros": executable, "policy_id": self.state.policy_id, "policy_digest": self.state.policy_digest}));
            if decision == "NO_TRADE" {
                let _ = self.record(
                    now_ms,
                    "execution",
                    "not_submitted",
                    &json!({"asset": asset.asset, "reason": reason}),
                );
            }
            if decision == "PAIR_BUY"
                && self.state.last_trade_condition.get(&asset.asset) != Some(&asset.condition_id)
            {
                let quantity = executable;
                let cost = net
                    .unwrap_or(i64::MAX)
                    .saturating_mul(quantity)
                    .checked_div(MICROS_PER_UNIT)
                    .unwrap_or(i64::MAX);
                let payout = MICROS_PER_UNIT
                    .saturating_mul(quantity)
                    .checked_div(MICROS_PER_UNIT)
                    .unwrap_or(0);
                let locked = payout.saturating_sub(cost);
                if cost <= affordable {
                    self.state.reserved = self.state.reserved.saturating_add(cost);
                    let _ = self.record(
                        now_ms,
                        "ledger",
                        "reservation_acquired",
                        &json!({"asset": asset.asset, "amount_micros": cost}),
                    );
                    self.state.cash = self.state.cash.saturating_sub(cost);
                    self.state.reserved = self.state.reserved.saturating_sub(cost);
                    let _ = self.record(now_ms, "ledger", "reservation_released", &json!({"asset": asset.asset, "amount_micros": cost, "reason": "simulated_pair_matched"}));
                    self.state.locked = self.state.locked.saturating_add(locked);
                    self.state.fills = self.state.fills.saturating_add(1);
                    self.state
                        .last_trade_condition
                        .insert(asset.asset.clone(), asset.condition_id.clone());
                    let trade = PaperTrade {
                        trade_id: format!(
                            "{}-{}",
                            self.state.session_id.as_deref().unwrap_or("paper"),
                            self.state.fills
                        ),
                        asset: asset.asset.clone(),
                        condition_id: asset.condition_id.clone(),
                        state: "FILLED_PAIR_LOCKED".into(),
                        quantity_micros: quantity.to_string(),
                        up_price_micros: up.unwrap_or(0).to_string(),
                        down_price_micros: down.unwrap_or(0).to_string(),
                        fee_micros: fee.unwrap_or(0).to_string(),
                        slippage_micros: slippage.unwrap_or(0).to_string(),
                        cost_micros: cost.to_string(),
                        locked_pnl_micros: locked.to_string(),
                        decision_at_ms: now_ms,
                    };
                    if let Some(contract) = self.state.contracts.get_mut(&asset.asset) {
                        contract.fills = contract.fills.saturating_add(1);
                    }
                    let rollup = self.state.daily.entry(day.clone()).or_default();
                    rollup.fills = rollup.fills.saturating_add(1);
                    rollup.fees = rollup.fees.saturating_add(fee.unwrap_or(0));
                    self.state.trades.push_back(trade.clone());
                    while self.state.trades.len() > MAX_TRADES {
                        self.state.trades.pop_front();
                    }
                    let _ = self.record(
                        now_ms,
                        "execution",
                        "filled_pair",
                        &serde_json::to_value(&trade).unwrap_or_default(),
                    );
                }
            }
            let _ = self.record(now_ms, "ledger", "conservation_checkpoint", &json!({"cash_micros": self.state.cash, "reserved_micros": self.state.reserved, "locked_pnl_micros": self.state.locked, "realized_pnl_micros": self.state.realized}));
        }
        self.state.checkpoints = self.state.checkpoints.saturating_add(1);
        self.state.last_checkpoint_ms = Some(now_ms);
        let _ = self.record(
            now_ms,
            "checkpoint",
            "checkpoint",
            &json!({"sequence": self.state.events, "trades": self.state.fills}),
        );
    }

    #[must_use]
    pub fn report(&self, verified_at_ms: i64) -> PaperReport {
        let mut cache = self
            .report_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(report) = cache.as_ref() {
            let cache_age_ms = verified_at_ms.saturating_sub(report.verified_at_ms);
            if (0..=REPORT_CACHE_TTL_MS).contains(&cache_age_ms) {
                return report.clone();
            }
        }
        let mut replay_verified = true;
        let mut records = 0_u64;
        if let Some(path) = &self.state.journal_path {
            match std::fs::read_to_string(path) {
                Ok(contents) => {
                    for line in contents.lines() {
                        let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
                        let Ok(value) = parsed else {
                            replay_verified = false;
                            break;
                        };
                        let Some(record) = value.get("record").cloned() else {
                            replay_verified = false;
                            break;
                        };
                        let Some(expected) = value
                            .get("record_digest")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                        else {
                            replay_verified = false;
                            break;
                        };
                        let Ok(bytes) = serde_json::to_vec(&record) else {
                            replay_verified = false;
                            break;
                        };
                        if to_hex(blake3::hash(&bytes).as_bytes()) != expected {
                            replay_verified = false;
                            break;
                        }
                        records = records.saturating_add(1);
                    }
                }
                Err(_) => replay_verified = false,
            }
        } else {
            replay_verified = false;
        }
        let mut gates = BTreeMap::new();
        gates.insert("paper_only".into(), true);
        gates.insert("journal_integrity".into(), replay_verified);
        gates.insert("data_present".into(), self.state.observations > 0);
        gates.insert(
            "capital_conservation".into(),
            self.state.cash >= 0 && self.state.reserved >= 0,
        );
        let all_gates = gates.values().all(|value| *value);
        let report = PaperReport {
            paper_only: true,
            campaign_id: self.state.session_id.clone(),
            replay_verified,
            journal_records: records,
            final_digest: to_hex(&self.state.journal_digest),
            net_pnl_micros: self
                .state
                .realized
                .saturating_add(self.state.locked)
                .saturating_add(self.state.unrealized)
                .to_string(),
            data_coverage_bps: if self.state.observations > 0 {
                10_000
            } else {
                0
            },
            verified_at_ms,
            gates,
            reason: if all_gates {
                "paper campaign evidence replay verified".into()
            } else {
                "campaign gates incomplete; no promotion authority".into()
            },
        };
        *cache = Some(report.clone());
        report
    }

    #[must_use]
    pub fn journal_path(&self) -> Option<PathBuf> {
        self.state.journal_path.clone()
    }

    #[must_use]
    pub fn status(&self) -> PaperStatus {
        let data_coverage = if self.state.observations == 0 {
            0
        } else {
            10_000
        };
        PaperStatus {
            paper_only: true,
            active: self.state.active,
            session_id: self.state.session_id.clone(),
            started_at_ms: self.state.started_at_ms,
            stopped_at_ms: self.state.stopped_at_ms,
            deadline_at_ms: self
                .state
                .started_at_ms
                .map(|started| started.saturating_add(self.state.campaign_duration_ms)),
            principal_micros: self.state.principal.to_string(),
            backup_micros: self.state.backup.to_string(),
            available_cash_micros: self.state.cash.to_string(),
            reserved_micros: self.state.reserved.to_string(),
            realized_pnl_micros: self.state.realized.to_string(),
            locked_pnl_micros: self.state.locked.to_string(),
            unrealized_pnl_micros: self.state.unrealized.to_string(),
            max_drawdown_micros: self.state.max_drawdown.to_string(),
            cvar_micros: "0".into(),
            hedge_failures: self.state.hedge_failures,
            fill_rate_bps: if self.state.decisions == 0 {
                0
            } else {
                self.state.fills.saturating_mul(10_000) / self.state.decisions
            },
            data_coverage_bps: data_coverage,
            events_recorded: self.state.events,
            decisions_recorded: self.state.decisions,
            checkpoints: self.state.checkpoints,
            last_checkpoint_ms: self.state.last_checkpoint_ms,
            replay_digest: to_hex(&self.state.journal_digest),
            journal_path: self
                .state
                .journal_path
                .as_ref()
                .map(|p| p.display().to_string()),
            last_error: self.state.last_error.clone(),
            policy_status: self.state.policy_status.clone(),
            policy_id: self.state.policy_id.clone(),
            policy_digest: self.state.policy_digest.clone(),
            runtime_config_id: self.state.runtime_config_id.clone(),
            runtime_config_digest: self.state.runtime_config_digest.clone(),
            contracts: self
                .state
                .contracts
                .iter()
                .map(|(asset, c)| ContractStatus {
                    asset: asset.clone(),
                    active: c.active,
                    observations: c.observations,
                    no_trade: c.no_trade,
                    fills: c.fills,
                    realized_pnl_micros: c.realized.to_string(),
                    last_decision: c.last_decision.clone(),
                    last_reason: c.last_reason.clone(),
                    last_event_ms: c.last_event_ms,
                })
                .collect(),
            trades: self.state.trades.iter().cloned().collect(),
            daily_rollups: self
                .state
                .daily
                .iter()
                .map(|(day, r)| DailyRollup {
                    day_utc: day.clone(),
                    events: r.events,
                    decisions: r.decisions,
                    fills: r.fills,
                    no_trade: r.no_trade,
                    realized_pnl_micros: r.realized.to_string(),
                    fees_micros: r.fees.to_string(),
                    drawdown_micros: r.drawdown.to_string(),
                })
                .collect(),
        }
    }

    fn record(
        &mut self,
        event_time_ms: i64,
        stream: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<(), String> {
        self.state.events = self.state.events.saturating_add(1);
        let body = json!({"schema_version": 1, "campaign_id": self.state.session_id, "runtime_config_id": self.state.runtime_config_id, "runtime_config_digest": self.state.runtime_config_digest, "stream": stream, "sequence": self.state.events, "event_time_ms": event_time_ms, "recorded_time_ms": event_time_ms, "kind": kind, "payload": payload});
        let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
        let digest = blake3::hash(&bytes);
        self.state.journal_digest = *digest.as_bytes();
        if let Some(path) = &self.state.journal_path {
            let length = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if length > MAX_JOURNAL_BYTES {
                return Err("paper journal size bound exceeded".into());
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| e.to_string())?;
            let line = json!({"record": body, "record_digest": to_hex(digest.as_bytes())});
            serde_json::to_writer(&mut file, &line).map_err(|e| e.to_string())?;
            file.write_all(b"\n").map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn parse_amount(raw: &str) -> Result<i64, String> {
    let value = raw
        .parse::<i64>()
        .map_err(|_| "amount must be a signed integer in micros".to_owned())?;
    if value < 0 {
        Err("amount cannot be negative".into())
    } else {
        Ok(value)
    }
}
fn parse_i64(raw: &str) -> Option<i64> {
    raw.parse().ok()
}
fn payload_i64(payload: &serde_json::Value, key: &str) -> Result<i64, String> {
    payload
        .get(key)
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
        .ok_or_else(|| format!("paper journal {key} missing or invalid"))
}
fn payload_string<'a>(payload: &'a serde_json::Value, key: &str) -> Result<&'a str, String> {
    payload
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("paper journal {key} missing or invalid"))
}
fn payload_asset(payload: &serde_json::Value) -> Result<String, String> {
    normalized_asset(payload_string(payload, "asset")?)
}
fn normalized_asset(raw: &str) -> Result<String, String> {
    let asset = raw.trim().to_ascii_uppercase();
    if !(2..=10).contains(&asset.len())
        || !asset
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err("paper asset identifier invalid".into());
    }
    Ok(asset)
}
fn day_key(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map_or_else(|| "invalid".into(), |d| d.format("%Y-%m-%d").to_string())
}
fn journal_path(session_id: &str) -> Result<PathBuf, String> {
    let dir = journal_directory();
    validate_journal_directory(&dir)?;
    let path = dir.join(format!("{session_id}.jsonl"));
    if path.exists() {
        return Err("paper campaign journal path already exists".into());
    }
    Ok(path)
}

fn journal_directory() -> PathBuf {
    PathBuf::from(
        env::var("POLY_PAPER_JOURNAL_DIR").unwrap_or_else(|_| "var/paper-campaign".into()),
    )
}

fn validate_journal_directory(dir: &PathBuf) -> Result<(), String> {
    create_dir_all(dir)
        .map_err(|error| format!("paper journal directory create failed: {error}"))?;
    let metadata = std::fs::symlink_metadata(dir)
        .map_err(|error| format!("paper journal directory metadata failed: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("paper journal directory is not a regular directory".into());
    }
    let probe_sequence = PREFLIGHT_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let probe = dir.join(format!(
        ".preflight-{}-{probe_sequence}",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
        .map_err(|error| format!("paper journal directory is not writable: {error}"))?;
    file.sync_data()
        .map_err(|error| format!("paper journal directory sync failed: {error}"))?;
    std::fs::remove_file(&probe)
        .map_err(|error| format!("paper journal probe cleanup failed: {error}"))?;
    Ok(())
}
fn latest_journal_path() -> Result<Option<PathBuf>, String> {
    let dir = PathBuf::from(
        env::var("POLY_PAPER_JOURNAL_DIR").unwrap_or_else(|_| "var/paper-campaign".into()),
    );
    if !dir.exists() {
        return Ok(None);
    }
    let mut candidates = read_dir(dir)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let valid_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("paper-")
                        && path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
                });
            valid_name.then_some(path)
        })
        .collect::<Vec<_>>();
    candidates.sort();
    Ok(candidates.pop())
}
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        PaperAssetPolicy, PaperController, PaperMarketPolicy, StartPaperRequest, MICROS_PER_UNIT,
        REPORT_CACHE_TTL_MS,
    };
    use std::collections::BTreeMap;

    fn policy(asset: &str) -> PaperMarketPolicy {
        PaperMarketPolicy {
            schema_version: 1,
            policy_id: "paper-test-v1".into(),
            issued_at_ms: 1,
            expires_at_ms: 10_000,
            campaign_duration_ms: 7 * 24 * 60 * 60 * 1_000,
            assets: BTreeMap::from([(
                asset.into(),
                PaperAssetPolicy {
                    fee_micros: "1000".into(),
                    slippage_micros: "500".into(),
                    minimum_locked_edge_micros: "1000".into(),
                    maximum_pair_quantity_micros: "100000000".into(),
                },
            )]),
        }
    }

    #[test]
    fn policy_accepts_configured_asset_without_an_embedded_universe() {
        let validated = policy("DOGE").validate(2).expect("valid policy");
        assert!(validated.assets.contains_key("DOGE"));
        assert_eq!(validated.assets["DOGE"].fee_micros, 1_000);
        assert!(!validated.digest.is_empty());
    }

    #[test]
    fn policy_rejects_non_conservative_edge_boundary() {
        let mut invalid = policy("BTC");
        invalid
            .assets
            .get_mut("BTC")
            .expect("asset")
            .minimum_locked_edge_micros = MICROS_PER_UNIT.to_string();
        assert!(invalid.validate(2).is_err());
    }

    #[test]
    fn policy_rejects_expired_policy() {
        assert!(policy("BTC").validate(10_000).is_err());
    }

    #[test]
    fn active_campaign_cannot_be_replaced_without_a_stop() {
        let mut controller = PaperController::default();
        controller.state.active = true;
        let request = StartPaperRequest {
            principal_micros: "1000000".into(),
            backup_micros: "0".into(),
            contracts: vec!["BTC".into()],
        };
        assert!(controller
            .start(request, 2, "runtime", &"0".repeat(64))
            .is_err());
    }

    #[test]
    fn replay_report_is_reused_only_within_the_bounded_audit_window() {
        let controller = PaperController::default();
        let first = controller.report(10);
        let cached = controller.report(11);
        let renewed = controller.report(10 + REPORT_CACHE_TTL_MS + 1);
        assert_eq!(first.verified_at_ms, 10);
        assert_eq!(cached.verified_at_ms, 10);
        assert_eq!(renewed.verified_at_ms, 10 + REPORT_CACHE_TTL_MS + 1);
    }
}
