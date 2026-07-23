#![forbid(unsafe_code)]

//! Deterministic scenario-based portfolio risk and capital-floor authority.
//!
//! An approval is an audit fact only. This crate has no signing, wallet,
//! authenticated exchange, order-submission, or cancellation capability.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableRiskEngine,
    RiskCheckpoint, RiskRecovery, StorageError,
};

use accounting_ledger::{ConfirmedTokenBalance, LedgerRiskView, TokenKey};
use serde::{Deserialize, Serialize};
use settlement_reconciliation::ReconciliationRiskGate;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 2;
const MIN_WIRE_VERSION: u16 = 1;
const MICROS_PER_UNIT: i128 = 1_000_000;
const BPS_DENOMINATOR: i128 = 10_000;
const MAX_TEXT_BYTES: usize = 512;
const MAX_COMMAND_BYTES: usize = 512 * 1024;
const HARD_MAX_ORDERS: usize = 12;
const HARD_MAX_CANDIDATES: usize = 2;
const HARD_MAX_MARKETS: usize = 12;
const HARD_MAX_SHOCKS: usize = 32;
const HARD_MAX_SCENARIOS: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct RiskCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct RiskOrderId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryMarketRisk {
    pub condition_id: String,
    pub up: TokenKey,
    pub down: TokenKey,
    pub shock_group: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderExposure {
    pub order_id: RiskOrderId,
    pub token: TokenKey,
    pub side: OrderSide,
    pub quantity_micros: i128,
    pub partial_fill_micros: i128,
    pub limit_price_micros: i64,
    pub max_fee_micros: i128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupMultiplier {
    pub shock_group: String,
    pub multiplier_bps: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShockProfile {
    pub shock_id: String,
    pub group_multipliers: Vec<GroupMultiplier>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskLimits {
    pub capital_floor_micros: i128,
    pub operational_reserve_micros: i128,
    pub pending_settlement_reserve_micros: i128,
    pub max_gross_exposure_micros: i128,
    pub max_condition_exposure_micros: i128,
    pub max_group_exposure_micros: i128,
    pub reserved_cash_haircut_bps: u16,
    pub available_token_haircut_bps: u16,
    pub reserved_token_haircut_bps: u16,
    pub locked_token_haircut_bps: u16,
    pub max_reconciliation_age_ns: i64,
    pub max_open_orders: usize,
    pub max_scenarios: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskRequest {
    pub reconciliation: ReconciliationRiskGate,
    pub ledger: LedgerRiskView,
    pub markets: Vec<BinaryMarketRisk>,
    pub open_orders: Vec<OrderExposure>,
    pub candidate: OrderExposure,
    #[serde(default)]
    pub additional_candidates: Vec<OrderExposure>,
    pub shocks: Vec<ShockProfile>,
    pub limits: RiskLimits,
    pub evaluated_at_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RiskCommand {
    Evaluate {
        command_id: RiskCommandId,
        request: RiskRequest,
        recorded_at_ns: i64,
    },
}

impl RiskCommand {
    #[must_use]
    pub const fn command_id(&self) -> RiskCommandId {
        match self {
            Self::Evaluate { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Evaluate { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: RiskCommand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Approve,
    NoTrade,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReason {
    AllLimitsSatisfied,
    ReconciliationNotReady,
    ReconciliationStale,
    ProvenanceMismatch,
    LedgerHalted,
    InvalidFrame,
    ReservationMismatch,
    CandidateCapacity,
    ScenarioBudget,
    CapitalFloor,
    GrossExposure,
    ConditionExposure,
    GroupExposure,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioWitness {
    pub fill_quantities_micros: Vec<i128>,
    pub outcome_bits: Vec<u8>,
    pub shock_id: String,
    pub terminal_wealth_micros: i128,
    pub gross_exposure_micros: i128,
    pub max_condition_exposure_micros: i128,
    pub max_group_exposure_micros: i128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskDecision {
    pub command_id: RiskCommandId,
    pub evaluated_at_ns: i64,
    pub status: DecisionStatus,
    pub reason: DecisionReason,
    pub minimum_terminal_wealth_micros: Option<i128>,
    pub scenario_count: u64,
    pub witness: Option<ScenarioWitness>,
    pub reconciliation_digest: [u8; 32],
    pub ledger_digest: [u8; 32],
    pub candidate_order_digest: [u8; 32],
    pub decision_digest: [u8; 32],
}

impl RiskDecision {
    /// Verifies that this immutable decision has not been altered.
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == decision_digest(self)
    }
}

/// Produces the stable Phase 2.2 fingerprint of an exact candidate order.
#[must_use]
pub fn order_exposure_digest(order: &OrderExposure) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"portfolio-risk-order-exposure-v1");
    hash_json(&mut hasher, order);
    *hasher.finalize().as_bytes()
}

/// Produces a stable fingerprint for the complete candidate set.
///
/// A one-order set deliberately preserves the Phase 2.2 single-order digest so
/// existing placement policy remains compatible. Multi-order decisions use a
/// distinct domain and therefore cannot authorize either constituent order.
#[must_use]
pub fn candidate_set_digest(request: &RiskRequest) -> [u8; 32] {
    if request.additional_candidates.is_empty() {
        return order_exposure_digest(&request.candidate);
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"portfolio-risk-candidate-set-v1");
    hash_json(&mut hasher, &request.candidate);
    for candidate in &request.additional_candidates {
        hash_json(&mut hasher, candidate);
    }
    *hasher.finalize().as_bytes()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RiskSnapshot {
    pub accepted_commands: u64,
    pub last_evaluated_at_ns: Option<i64>,
    pub last_reconciled_at_ns: Option<i64>,
    pub last_reconciliation_digest: Option<[u8; 32]>,
    pub last_decision: Option<RiskDecision>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("risk command timestamp is invalid")]
    Timestamp,
    #[error("risk command exceeds its canonical bound")]
    CommandBound,
    #[error("risk command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported risk command version: {0}")]
    Version(u16),
    #[error("risk command id was reused for different content")]
    IdempotencyConflict,
    #[error("reconciliation history regressed or equivocated")]
    ReconciliationHistory,
    #[error("risk arithmetic overflow")]
    Overflow,
    #[error("risk engine is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug, Default)]
pub struct PortfolioRiskEngine {
    processed: BTreeMap<RiskCommandId, ([u8; 32], RiskDecision)>,
    accepted_commands: u64,
    last_evaluated_at_ns: Option<i64>,
    last_reconciled_at_ns: Option<i64>,
    last_reconciliation_digest: Option<[u8; 32]>,
    last_decision: Option<RiskDecision>,
    halted: Option<String>,
}

impl PortfolioRiskEngine {
    /// Evaluates and records one candidate. Normal risk failures produce a
    /// durable `NO_TRADE`; history/idempotency failures halt.
    ///
    /// # Errors
    ///
    /// Returns codec, arithmetic, history, idempotency, or halted errors.
    pub fn apply(&mut self, command: &RiskCommand) -> Result<RiskDecision, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        let bytes = encode_command(command)?;
        let digest = *blake3::hash(&bytes).as_bytes();
        let id = command.command_id();
        if let Some((existing, decision)) = self.processed.get(&id) {
            if *existing == digest {
                return Ok(decision.clone());
            }
            return self.install_halt(Error::IdempotencyConflict);
        }
        let RiskCommand::Evaluate { request, .. } = command;
        if let Some(previous_at) = self.last_evaluated_at_ns {
            if request.evaluated_at_ns < previous_at {
                return self.install_halt(Error::ReconciliationHistory);
            }
        }
        if let (Some(previous_at), Some(observed_at)) = (
            self.last_reconciled_at_ns,
            request.reconciliation.evaluated_at_ns,
        ) {
            if observed_at < previous_at
                || (observed_at == previous_at
                    && self.last_reconciliation_digest
                        != Some(request.reconciliation.reconciliation_digest))
            {
                return self.install_halt(Error::ReconciliationHistory);
            }
        }
        let mut decision = match evaluate(id, request) {
            Ok(value) => value,
            Err(error) => return self.install_halt(error),
        };
        decision.decision_digest = decision_digest(&decision);
        let mut candidate = self.clone();
        candidate.processed.insert(id, (digest, decision.clone()));
        candidate.accepted_commands = match candidate.accepted_commands.checked_add(1) {
            Some(value) => value,
            None => return self.install_halt(Error::Overflow),
        };
        candidate.last_evaluated_at_ns = Some(request.evaluated_at_ns);
        if request.reconciliation.evaluated_at_ns.is_some() {
            candidate.last_reconciled_at_ns = request.reconciliation.evaluated_at_ns;
        }
        candidate.last_reconciliation_digest = Some(request.reconciliation.reconciliation_digest);
        candidate.last_decision = Some(decision.clone());
        *self = candidate;
        Ok(decision)
    }

    #[must_use]
    pub fn snapshot(&self) -> RiskSnapshot {
        RiskSnapshot {
            accepted_commands: self.accepted_commands,
            last_evaluated_at_ns: self.last_evaluated_at_ns,
            last_reconciled_at_ns: self.last_reconciled_at_ns,
            last_reconciliation_digest: self.last_reconciliation_digest,
            last_decision: self.last_decision.clone(),
            halted: self.halted.is_some(),
            halt_reason: self.halted.clone(),
            digest: self.state_digest(),
        }
    }

    #[must_use]
    pub const fn is_halted(&self) -> bool {
        self.halted.is_some()
    }

    fn install_halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"portfolio-risk-state-v1");
        hasher.update(&self.accepted_commands.to_le_bytes());
        hash_json(&mut hasher, &self.last_evaluated_at_ns);
        hash_json(&mut hasher, &self.last_reconciled_at_ns);
        hash_json(&mut hasher, &self.last_reconciliation_digest);
        hash_json(&mut hasher, &self.last_decision);
        for (id, (digest, decision)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(digest);
            hash_json(&mut hasher, decision);
        }
        hash_json(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }
}

#[derive(Clone)]
struct Holdings {
    available_cash: i128,
    reserved_cash: i128,
    available: BTreeMap<TokenKey, i128>,
    reserved: BTreeMap<TokenKey, i128>,
    locked: BTreeMap<TokenKey, i128>,
}

struct Prepared<'a> {
    request: &'a RiskRequest,
    groups: Vec<String>,
    fill_options: Vec<Vec<i128>>,
    fill_combinations: u64,
    outcome_combinations: u64,
    scenario_count: u64,
    holdings: Holdings,
}

fn evaluate(id: RiskCommandId, request: &RiskRequest) -> Result<RiskDecision, Error> {
    let base = |reason| no_trade(id, request, reason, 0, None, None);
    if !valid_limits(&request.limits) || request.evaluated_at_ns < 0 {
        return Ok(base(DecisionReason::InvalidFrame));
    }
    if !request.reconciliation.ready {
        return Ok(base(DecisionReason::ReconciliationNotReady));
    }
    let Some(reconciled_at) = request.reconciliation.evaluated_at_ns else {
        return Ok(base(DecisionReason::ReconciliationNotReady));
    };
    let Some(reconciled_ledger) = request.reconciliation.ledger_digest else {
        return Ok(base(DecisionReason::ReconciliationNotReady));
    };
    if request.evaluated_at_ns < reconciled_at {
        return Ok(base(DecisionReason::InvalidFrame));
    }
    let age = request
        .evaluated_at_ns
        .checked_sub(reconciled_at)
        .ok_or(Error::Overflow)?;
    if age > request.limits.max_reconciliation_age_ns {
        return Ok(base(DecisionReason::ReconciliationStale));
    }
    if reconciled_ledger != request.ledger.ledger_digest {
        return Ok(base(DecisionReason::ProvenanceMismatch));
    }
    if request.ledger.halted {
        return Ok(base(DecisionReason::LedgerHalted));
    }
    let prepared = match prepare(request)? {
        Ok(value) => value,
        Err(reason) => return Ok(base(reason)),
    };
    enumerate(id, &prepared)
}

fn prepare(request: &RiskRequest) -> Result<Result<Prepared<'_>, DecisionReason>, Error> {
    if !valid_limits(&request.limits)
        || request.evaluated_at_ns < 0
        || request.markets.is_empty()
        || request.markets.len() > HARD_MAX_MARKETS
        || request.open_orders.len() > request.limits.max_open_orders
        || request.open_orders.len() > HARD_MAX_ORDERS
        || request.additional_candidates.len() + 1 > HARD_MAX_CANDIDATES
        || request.open_orders.len() + request.additional_candidates.len() + 1 > HARD_MAX_ORDERS
        || request.shocks.is_empty()
        || request.shocks.len() > HARD_MAX_SHOCKS
    {
        return Ok(Err(DecisionReason::InvalidFrame));
    }
    let Some((market_by_token, groups)) = market_index(&request.markets) else {
        return Ok(Err(DecisionReason::InvalidFrame));
    };
    if !valid_shocks(&request.shocks, &groups) {
        return Ok(Err(DecisionReason::InvalidFrame));
    }
    let Some(available) = balances(&request.ledger.available_tokens, &market_by_token) else {
        return Ok(Err(DecisionReason::InvalidFrame));
    };
    let Some(reserved) = balances(&request.ledger.reserved_tokens, &market_by_token) else {
        return Ok(Err(DecisionReason::InvalidFrame));
    };
    let Some(locked) = balances(&request.ledger.locked_tokens, &market_by_token) else {
        return Ok(Err(DecisionReason::InvalidFrame));
    };
    if request.ledger.cash_available_micros < 0 || request.ledger.cash_reserved_micros < 0 {
        return Ok(Err(DecisionReason::InvalidFrame));
    }
    let mut order_ids = BTreeSet::new();
    let mut reserved_buy = 0_i128;
    let mut reserved_sell = BTreeMap::new();
    let mut fill_options = Vec::new();
    for order in &request.open_orders {
        if !valid_order(order, &market_by_token)
            || !order_fill_safe(order)?
            || !order_ids.insert(order.order_id)
        {
            return Ok(Err(DecisionReason::InvalidFrame));
        }
        match order.side {
            OrderSide::Buy => {
                reserved_buy = reserved_buy
                    .checked_add(full_buy_cost(order)?)
                    .ok_or(Error::Overflow)?;
            }
            OrderSide::Sell => {
                let total = reserved_sell.entry(order.token.clone()).or_insert(0_i128);
                *total = total
                    .checked_add(order.quantity_micros)
                    .ok_or(Error::Overflow)?;
            }
        }
        fill_options.push(options(order));
    }
    if reserved_buy != request.ledger.cash_reserved_micros || reserved_sell != reserved {
        return Ok(Err(DecisionReason::ReservationMismatch));
    }
    for candidate in std::iter::once(&request.candidate).chain(&request.additional_candidates) {
        if !valid_order(candidate, &market_by_token) || !order_ids.insert(candidate.order_id) {
            return Ok(Err(DecisionReason::InvalidFrame));
        }
        fill_options.push(options(candidate));
    }
    if !candidates_have_capacity(request, &available)? {
        return Ok(Err(DecisionReason::CandidateCapacity));
    }
    let fill_combinations = product_count(fill_options.iter().map(Vec::len))?;
    let outcome_combinations = 1_u64
        .checked_shl(u32::try_from(request.markets.len()).map_err(|_| Error::Overflow)?)
        .ok_or(Error::Overflow)?;
    let scenario_count = fill_combinations
        .checked_mul(outcome_combinations)
        .and_then(|value| {
            u64::try_from(request.shocks.len())
                .ok()
                .and_then(|count| value.checked_mul(count))
        })
        .ok_or(Error::Overflow)?;
    if scenario_count > request.limits.max_scenarios || scenario_count > HARD_MAX_SCENARIOS {
        return Ok(Err(DecisionReason::ScenarioBudget));
    }
    Ok(Ok(Prepared {
        request,
        groups: groups.into_iter().collect(),
        fill_options,
        fill_combinations,
        outcome_combinations,
        scenario_count,
        holdings: Holdings {
            available_cash: request.ledger.cash_available_micros,
            reserved_cash: request.ledger.cash_reserved_micros,
            available,
            reserved,
            locked,
        },
    }))
}

fn candidates_have_capacity(
    request: &RiskRequest,
    available: &BTreeMap<TokenKey, i128>,
) -> Result<bool, Error> {
    let mut buy_cost = 0_i128;
    let mut sell_quantity = BTreeMap::<TokenKey, i128>::new();
    for candidate in std::iter::once(&request.candidate).chain(&request.additional_candidates) {
        if !order_fill_safe(candidate)? {
            return Ok(false);
        }
        match candidate.side {
            OrderSide::Buy => {
                buy_cost = buy_cost
                    .checked_add(full_buy_cost(candidate)?)
                    .ok_or(Error::Overflow)?;
            }
            OrderSide::Sell => {
                let value = sell_quantity.entry(candidate.token.clone()).or_default();
                *value = value
                    .checked_add(candidate.quantity_micros)
                    .ok_or(Error::Overflow)?;
            }
        }
    }
    Ok(buy_cost <= request.ledger.cash_available_micros
        && sell_quantity
            .into_iter()
            .all(|(token, quantity)| available.get(&token).copied().unwrap_or(0) >= quantity))
}

fn enumerate(id: RiskCommandId, prepared: &Prepared<'_>) -> Result<RiskDecision, Error> {
    let mut minimum: Option<ScenarioWitness> = None;
    let mut max_gross: Option<ScenarioWitness> = None;
    let mut max_condition: Option<ScenarioWitness> = None;
    let mut max_group: Option<ScenarioWitness> = None;
    for fill_index in 0..prepared.fill_combinations {
        let fills = decode_fills(fill_index, &prepared.fill_options)?;
        let mut holdings = prepared.holdings.clone();
        for (index, fill) in fills.iter().copied().enumerate() {
            let (order, reserved_source) = scenario_order(prepared.request, index);
            apply_fill(&mut holdings, order, fill, reserved_source)?;
        }
        let (gross, condition, group) = exposure(&holdings, prepared.request, &prepared.groups)?;
        for outcome in 0..prepared.outcome_combinations {
            let bits = outcome_bits(outcome, prepared.request.markets.len());
            for shock in &prepared.request.shocks {
                let wealth = terminal_wealth(&holdings, prepared.request, &bits, shock)?;
                let witness = ScenarioWitness {
                    fill_quantities_micros: fills.clone(),
                    outcome_bits: bits.clone(),
                    shock_id: shock.shock_id.clone(),
                    terminal_wealth_micros: wealth,
                    gross_exposure_micros: gross,
                    max_condition_exposure_micros: condition,
                    max_group_exposure_micros: group,
                };
                replace_min(&mut minimum, &witness, |value| value.terminal_wealth_micros);
                replace_max(&mut max_gross, &witness, |value| {
                    value.gross_exposure_micros
                });
                replace_max(&mut max_condition, &witness, |value| {
                    value.max_condition_exposure_micros
                });
                replace_max(&mut max_group, &witness, |value| {
                    value.max_group_exposure_micros
                });
            }
        }
    }
    let minimum = minimum.ok_or(Error::Overflow)?;
    let limits = &prepared.request.limits;
    let (status, reason, witness) = if minimum.terminal_wealth_micros < limits.capital_floor_micros
    {
        (
            DecisionStatus::NoTrade,
            DecisionReason::CapitalFloor,
            minimum.clone(),
        )
    } else if max_gross
        .as_ref()
        .is_some_and(|value| value.gross_exposure_micros > limits.max_gross_exposure_micros)
    {
        (
            DecisionStatus::NoTrade,
            DecisionReason::GrossExposure,
            max_gross.ok_or(Error::Overflow)?,
        )
    } else if max_condition.as_ref().is_some_and(|value| {
        value.max_condition_exposure_micros > limits.max_condition_exposure_micros
    }) {
        (
            DecisionStatus::NoTrade,
            DecisionReason::ConditionExposure,
            max_condition.ok_or(Error::Overflow)?,
        )
    } else if max_group
        .as_ref()
        .is_some_and(|value| value.max_group_exposure_micros > limits.max_group_exposure_micros)
    {
        (
            DecisionStatus::NoTrade,
            DecisionReason::GroupExposure,
            max_group.ok_or(Error::Overflow)?,
        )
    } else {
        (
            DecisionStatus::Approve,
            DecisionReason::AllLimitsSatisfied,
            minimum.clone(),
        )
    };
    let mut decision = RiskDecision {
        command_id: id,
        evaluated_at_ns: prepared.request.evaluated_at_ns,
        status,
        reason,
        minimum_terminal_wealth_micros: Some(minimum.terminal_wealth_micros),
        scenario_count: prepared.scenario_count,
        witness: Some(witness),
        reconciliation_digest: prepared.request.reconciliation.reconciliation_digest,
        ledger_digest: prepared.request.ledger.ledger_digest,
        candidate_order_digest: candidate_set_digest(prepared.request),
        decision_digest: [0; 32],
    };
    decision.decision_digest = decision_digest(&decision);
    Ok(decision)
}

fn scenario_order(request: &RiskRequest, index: usize) -> (&OrderExposure, bool) {
    if index < request.open_orders.len() {
        return (&request.open_orders[index], true);
    }
    let candidate_index = index - request.open_orders.len();
    if candidate_index == 0 {
        (&request.candidate, false)
    } else {
        (&request.additional_candidates[candidate_index - 1], false)
    }
}

fn apply_fill(
    holdings: &mut Holdings,
    order: &OrderExposure,
    fill: i128,
    reserved_source: bool,
) -> Result<(), Error> {
    if fill == 0 {
        return Ok(());
    }
    let fee = partial_fee(order.max_fee_micros, fill, order.quantity_micros)?;
    match order.side {
        OrderSide::Buy => {
            let cost = buy_cost(order.limit_price_micros, fill)?
                .checked_add(fee)
                .ok_or(Error::Overflow)?;
            let cash = if reserved_source {
                &mut holdings.reserved_cash
            } else {
                &mut holdings.available_cash
            };
            *cash = cash.checked_sub(cost).ok_or(Error::Overflow)?;
            if *cash < 0 {
                return Err(Error::Overflow);
            }
            add_balance(&mut holdings.available, &order.token, fill)?;
        }
        OrderSide::Sell => {
            let tokens = if reserved_source {
                &mut holdings.reserved
            } else {
                &mut holdings.available
            };
            subtract_balance(tokens, &order.token, fill)?;
            let proceeds = sale_proceeds(order.limit_price_micros, fill)?
                .checked_sub(fee)
                .ok_or(Error::Overflow)?;
            if proceeds < 0 {
                return Err(Error::Overflow);
            }
            holdings.available_cash = holdings
                .available_cash
                .checked_add(proceeds)
                .ok_or(Error::Overflow)?;
        }
    }
    Ok(())
}

fn terminal_wealth(
    holdings: &Holdings,
    request: &RiskRequest,
    outcomes: &[u8],
    shock: &ShockProfile,
) -> Result<i128, Error> {
    let reserved_cash = haircut(
        holdings.reserved_cash,
        request.limits.reserved_cash_haircut_bps,
    )?;
    let mut wealth = holdings
        .available_cash
        .checked_add(reserved_cash)
        .ok_or(Error::Overflow)?;
    for (index, market) in request.markets.iter().enumerate() {
        let winner = if outcomes[index] == 0 {
            &market.up
        } else {
            &market.down
        };
        let multiplier = shock_multiplier(shock, &market.shock_group);
        for (balances, base_haircut) in [
            (
                &holdings.available,
                request.limits.available_token_haircut_bps,
            ),
            (
                &holdings.reserved,
                request.limits.reserved_token_haircut_bps,
            ),
            (&holdings.locked, request.limits.locked_token_haircut_bps),
        ] {
            let quantity = balances.get(winner).copied().unwrap_or(0);
            let value = haircut(haircut(quantity, base_haircut)?, multiplier)?;
            wealth = wealth.checked_add(value).ok_or(Error::Overflow)?;
        }
    }
    wealth
        .checked_sub(request.limits.operational_reserve_micros)
        .and_then(|value| value.checked_sub(request.limits.pending_settlement_reserve_micros))
        .ok_or(Error::Overflow)
}

fn exposure(
    holdings: &Holdings,
    request: &RiskRequest,
    groups: &[String],
) -> Result<(i128, i128, i128), Error> {
    let mut gross = 0_i128;
    let mut max_condition = 0_i128;
    let mut group_exposure: BTreeMap<&str, i128> =
        groups.iter().map(|group| (group.as_str(), 0)).collect();
    for market in &request.markets {
        let up = total_token(holdings, &market.up)?;
        let down = total_token(holdings, &market.down)?;
        gross = gross
            .checked_add(up)
            .and_then(|value| value.checked_add(down))
            .ok_or(Error::Overflow)?;
        let directional = up.checked_sub(down).ok_or(Error::Overflow)?.abs();
        max_condition = max_condition.max(directional);
        let total = group_exposure
            .get_mut(market.shock_group.as_str())
            .ok_or(Error::Overflow)?;
        *total = total.checked_add(directional).ok_or(Error::Overflow)?;
    }
    let max_group = group_exposure.values().copied().max().unwrap_or(0);
    Ok((gross, max_condition, max_group))
}

fn total_token(holdings: &Holdings, token: &TokenKey) -> Result<i128, Error> {
    holdings
        .available
        .get(token)
        .copied()
        .unwrap_or(0)
        .checked_add(holdings.reserved.get(token).copied().unwrap_or(0))
        .and_then(|value| value.checked_add(holdings.locked.get(token).copied().unwrap_or(0)))
        .ok_or(Error::Overflow)
}

fn market_index(
    markets: &[BinaryMarketRisk],
) -> Option<(BTreeMap<TokenKey, usize>, BTreeSet<String>)> {
    let mut by_token = BTreeMap::new();
    let mut conditions = BTreeSet::new();
    let mut groups = BTreeSet::new();
    for (index, market) in markets.iter().enumerate() {
        if !valid_text(&market.condition_id)
            || !valid_text(&market.shock_group)
            || !valid_text(&market.up.token_id)
            || !valid_text(&market.down.token_id)
            || market.up == market.down
            || market.up.condition_id != market.condition_id
            || market.down.condition_id != market.condition_id
            || !conditions.insert(market.condition_id.clone())
            || by_token.insert(market.up.clone(), index).is_some()
            || by_token.insert(market.down.clone(), index).is_some()
        {
            return None;
        }
        groups.insert(market.shock_group.clone());
    }
    Some((by_token, groups))
}

fn valid_limits(limits: &RiskLimits) -> bool {
    limits.capital_floor_micros >= 0
        && limits.operational_reserve_micros >= 0
        && limits.pending_settlement_reserve_micros >= 0
        && limits.max_gross_exposure_micros >= 0
        && limits.max_condition_exposure_micros >= 0
        && limits.max_group_exposure_micros >= 0
        && limits.max_reconciliation_age_ns >= 0
        && limits.max_open_orders <= HARD_MAX_ORDERS
        && limits.max_scenarios > 0
        && limits.max_scenarios <= HARD_MAX_SCENARIOS
        && [
            limits.reserved_cash_haircut_bps,
            limits.available_token_haircut_bps,
            limits.reserved_token_haircut_bps,
            limits.locked_token_haircut_bps,
        ]
        .into_iter()
        .all(|value| value <= 10_000)
}

fn valid_shocks(shocks: &[ShockProfile], groups: &BTreeSet<String>) -> bool {
    let mut ids = BTreeSet::new();
    shocks.iter().all(|shock| {
        if !valid_text(&shock.shock_id) || !ids.insert(&shock.shock_id) {
            return false;
        }
        let mut previous: Option<&str> = None;
        shock.group_multipliers.iter().all(|entry| {
            let valid = valid_text(&entry.shock_group)
                && groups.contains(&entry.shock_group)
                && entry.multiplier_bps <= 10_000
                && previous.is_none_or(|value| value < entry.shock_group.as_str());
            previous = Some(&entry.shock_group);
            valid
        })
    })
}

fn valid_order(order: &OrderExposure, markets: &BTreeMap<TokenKey, usize>) -> bool {
    markets.contains_key(&order.token)
        && order.quantity_micros > 0
        && order.partial_fill_micros >= 0
        && order.partial_fill_micros < order.quantity_micros
        && (0..=1_000_000).contains(&order.limit_price_micros)
        && order.max_fee_micros >= 0
}

fn order_fill_safe(order: &OrderExposure) -> Result<bool, Error> {
    if order.side == OrderSide::Buy {
        return Ok(true);
    }
    for fill in options(order).into_iter().filter(|fill| *fill > 0) {
        if partial_fee(order.max_fee_micros, fill, order.quantity_micros)?
            > sale_proceeds(order.limit_price_micros, fill)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn balances(
    values: &[ConfirmedTokenBalance],
    markets: &BTreeMap<TokenKey, usize>,
) -> Option<BTreeMap<TokenKey, i128>> {
    let mut result = BTreeMap::new();
    let mut previous: Option<&TokenKey> = None;
    for value in values {
        if value.balance_micros <= 0
            || !markets.contains_key(&value.token)
            || previous.is_some_and(|token| token >= &value.token)
            || result
                .insert(value.token.clone(), value.balance_micros)
                .is_some()
        {
            return None;
        }
        previous = Some(&value.token);
    }
    Some(result)
}

fn options(order: &OrderExposure) -> Vec<i128> {
    let mut result = vec![0];
    if order.partial_fill_micros > 0 {
        result.push(order.partial_fill_micros);
    }
    result.push(order.quantity_micros);
    result
}

fn product_count(mut values: impl Iterator<Item = usize>) -> Result<u64, Error> {
    values.try_fold(1_u64, |total, value| {
        total
            .checked_mul(u64::try_from(value).map_err(|_| Error::Overflow)?)
            .ok_or(Error::Overflow)
    })
}

fn decode_fills(mut index: u64, options: &[Vec<i128>]) -> Result<Vec<i128>, Error> {
    let mut result = Vec::with_capacity(options.len());
    for values in options {
        let radix = u64::try_from(values.len()).map_err(|_| Error::Overflow)?;
        let selected = usize::try_from(index % radix).map_err(|_| Error::Overflow)?;
        result.push(values[selected]);
        index /= radix;
    }
    Ok(result)
}

fn outcome_bits(value: u64, count: usize) -> Vec<u8> {
    (0..count)
        .map(|index| u8::from((value & (1_u64 << index)) != 0))
        .collect()
}

fn full_buy_cost(order: &OrderExposure) -> Result<i128, Error> {
    buy_cost(order.limit_price_micros, order.quantity_micros)?
        .checked_add(order.max_fee_micros)
        .ok_or(Error::Overflow)
}

fn buy_cost(price: i64, quantity: i128) -> Result<i128, Error> {
    let product = i128::from(price)
        .checked_mul(quantity)
        .ok_or(Error::Overflow)?;
    div_ceil(product, MICROS_PER_UNIT)
}

fn sale_proceeds(price: i64, quantity: i128) -> Result<i128, Error> {
    i128::from(price)
        .checked_mul(quantity)
        .ok_or(Error::Overflow)
        .map(|value| value / MICROS_PER_UNIT)
}

fn partial_fee(fee: i128, filled: i128, total: i128) -> Result<i128, Error> {
    div_ceil(fee.checked_mul(filled).ok_or(Error::Overflow)?, total)
}

fn div_ceil(value: i128, divisor: i128) -> Result<i128, Error> {
    value
        .checked_add(divisor.checked_sub(1).ok_or(Error::Overflow)?)
        .ok_or(Error::Overflow)
        .map(|adjusted| adjusted / divisor)
}

fn haircut(value: i128, bps: u16) -> Result<i128, Error> {
    value
        .checked_mul(i128::from(bps))
        .ok_or(Error::Overflow)
        .map(|product| product / BPS_DENOMINATOR)
}

fn shock_multiplier(shock: &ShockProfile, group: &str) -> u16 {
    shock
        .group_multipliers
        .binary_search_by(|entry| entry.shock_group.as_str().cmp(group))
        .ok()
        .map_or(10_000, |index| {
            shock.group_multipliers[index].multiplier_bps
        })
}

fn add_balance(
    balances: &mut BTreeMap<TokenKey, i128>,
    token: &TokenKey,
    quantity: i128,
) -> Result<(), Error> {
    let balance = balances.entry(token.clone()).or_default();
    *balance = balance.checked_add(quantity).ok_or(Error::Overflow)?;
    Ok(())
}

fn subtract_balance(
    balances: &mut BTreeMap<TokenKey, i128>,
    token: &TokenKey,
    quantity: i128,
) -> Result<(), Error> {
    let balance = balances.get_mut(token).ok_or(Error::Overflow)?;
    *balance = balance.checked_sub(quantity).ok_or(Error::Overflow)?;
    if *balance < 0 {
        return Err(Error::Overflow);
    }
    if *balance == 0 {
        balances.remove(token);
    }
    Ok(())
}

fn replace_min(
    current: &mut Option<ScenarioWitness>,
    candidate: &ScenarioWitness,
    value: impl Fn(&ScenarioWitness) -> i128,
) {
    if current
        .as_ref()
        .is_none_or(|existing| value(candidate) < value(existing))
    {
        *current = Some(candidate.clone());
    }
}

fn replace_max(
    current: &mut Option<ScenarioWitness>,
    candidate: &ScenarioWitness,
    value: impl Fn(&ScenarioWitness) -> i128,
) {
    if current
        .as_ref()
        .is_none_or(|existing| value(candidate) > value(existing))
    {
        *current = Some(candidate.clone());
    }
}

fn no_trade(
    id: RiskCommandId,
    request: &RiskRequest,
    reason: DecisionReason,
    scenarios: u64,
    minimum: Option<i128>,
    witness: Option<ScenarioWitness>,
) -> RiskDecision {
    let mut decision = RiskDecision {
        command_id: id,
        evaluated_at_ns: request.evaluated_at_ns,
        status: DecisionStatus::NoTrade,
        reason,
        minimum_terminal_wealth_micros: minimum,
        scenario_count: scenarios,
        witness,
        reconciliation_digest: request.reconciliation.reconciliation_digest,
        ledger_digest: request.ledger.ledger_digest,
        candidate_order_digest: candidate_set_digest(request),
        decision_digest: [0; 32],
    };
    decision.decision_digest = decision_digest(&decision);
    decision
}

fn decision_digest(decision: &RiskDecision) -> [u8; 32] {
    let mut value = decision.clone();
    value.decision_digest = [0; 32];
    *blake3::hash(&serde_json::to_vec(&value).expect("risk decision serializes")).as_bytes()
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT_BYTES
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("internal portfolio risk state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

fn validate_command(command: &RiskCommand) -> Result<(), Error> {
    let RiskCommand::Evaluate {
        request,
        recorded_at_ns,
        ..
    } = command;
    if *recorded_at_ns < 0
        || request.evaluated_at_ns < 0
        || *recorded_at_ns < request.evaluated_at_ns
    {
        return Err(Error::Timestamp);
    }
    Ok(())
}

/// Encodes one exact bounded versioned evaluation command.
///
/// # Errors
///
/// Rejects invalid timestamps, serialization failure, or oversized payloads.
pub fn encode_command(command: &RiskCommand) -> Result<Vec<u8>, Error> {
    validate_command(command)?;
    let bytes = serde_json::to_vec(&WireCommand {
        version: WIRE_VERSION,
        command: command.clone(),
    })
    .map_err(|error| Error::Json(error.to_string()))?;
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    Ok(bytes)
}

/// Decodes and revalidates one exact risk command.
///
/// # Errors
///
/// Rejects malformed JSON, trailing data, versions, bounds, and timestamps.
pub fn decode_command(bytes: &[u8]) -> Result<RiskCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let wire: WireCommand =
        serde_json::from_slice(bytes).map_err(|error| Error::Json(error.to_string()))?;
    if !(MIN_WIRE_VERSION..=WIRE_VERSION).contains(&wire.version) {
        return Err(Error::Version(wire.version));
    }
    validate_command(&wire.command)?;
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
