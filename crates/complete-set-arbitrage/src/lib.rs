#![forbid(unsafe_code)]

//! Deterministic, offline complete-set arbitrage detection.
//!
//! A detected plan is not locked profit and has no capital, risk, signing,
//! split/merge, network, or execution authority.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, ArbitrageCheckpoint,
    ArbitrageRecovery, DurableArbitrageEngine, StorageError,
};

use portfolio_risk::OrderSide;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use strategy_proposal::{
    ProposalId, ProposalIntent, ProposalLevel, StrategyClass, StrategyContext,
};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 256 * 1024;
const MICROS_PER_UNIT: i128 = 1_000_000;
const BPS_DENOMINATOR: i128 = 10_000;
const MAX_QUANTITY_MICROS: i64 = 1_000_000_000_000;
const MAX_COST_MICROS: i128 = 1_000_000_000_000;
const MAX_FEE_MICROS: i128 = 1_000_000_000;
const MAX_ROI_BPS: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ArbitrageCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ArbitrageEvaluationId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArbitrageDirection {
    BuyPair,
    SellPair,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArbitrageConstraints {
    pub min_quantity_micros: i64,
    pub max_quantity_micros: i64,
    pub partial_fill_micros: i64,
    pub up_max_fee_micros: i128,
    pub down_max_fee_micros: i128,
    pub conversion_max_cost_micros: i128,
    pub min_net_profit_micros: i128,
    pub min_roi_bps: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArbitrageRequest {
    pub evaluation_id: ArbitrageEvaluationId,
    pub direction: ArbitrageDirection,
    pub context: Box<StrategyContext>,
    pub constraints: ArbitrageConstraints,
    pub evaluated_at_ns: i64,
    pub expires_at_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ArbitrageCommand {
    Evaluate {
        command_id: ArbitrageCommandId,
        request: ArbitrageRequest,
        recorded_at_ns: i64,
    },
}

impl ArbitrageCommand {
    #[must_use]
    pub const fn command_id(&self) -> ArbitrageCommandId {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArbitrageStatus {
    Opportunity,
    NoOpportunity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArbitrageReason {
    ExecutableProfit,
    SourceNotReady,
    ContextExpired,
    RequestExpired,
    InvalidConstraints,
    InsufficientLiquidity,
    NonPositiveProfit,
    ProfitBelowMinimum,
    RoiBelowMinimum,
    UndefinedRoi,
    EvaluationAlreadyUsed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArbitragePlan {
    pub direction: ArbitrageDirection,
    pub quantity_micros: i64,
    pub up_price_micros: i64,
    pub down_price_micros: i64,
    pub up_value_micros: i128,
    pub down_value_micros: i128,
    pub fee_budget_micros: i128,
    pub conversion_cost_micros: i128,
    pub deployed_capital_micros: i128,
    pub net_profit_micros: i128,
    pub roi_bps: u64,
    pub intents: [ProposalIntent; 2],
    pub plan_digest: [u8; 32],
}

impl ArbitragePlan {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.plan_digest == plan_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArbitrageDecision {
    pub command_id: ArbitrageCommandId,
    pub evaluation_id: ArbitrageEvaluationId,
    pub status: ArbitrageStatus,
    pub reason: ArbitrageReason,
    pub context_digest: [u8; 32],
    pub request_digest: [u8; 32],
    pub plan: Option<ArbitragePlan>,
    pub decision_digest: [u8; 32],
}

impl ArbitrageDecision {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == decision_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbitrageSnapshot {
    pub accepted_commands: u64,
    pub evaluation_count: usize,
    pub opportunity_count: u64,
    pub no_opportunity_count: u64,
    pub last_context_captured_at_ns: Option<i64>,
    pub last_context_digest: Option<[u8; 32]>,
    pub last_decision: Option<ArbitrageDecision>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("arbitrage command timestamp is invalid")]
    Timestamp,
    #[error("arbitrage command exceeds its canonical bound")]
    CommandBound,
    #[error("arbitrage command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported arbitrage command version: {0}")]
    Version(u16),
    #[error("arbitrage command id was reused for different content")]
    IdempotencyConflict,
    #[error("arbitrage command clock regressed")]
    ClockRegression,
    #[error("arbitrage context checksum is invalid")]
    ContextDigest,
    #[error("arbitrage context history regressed or equivocated")]
    ContextHistory,
    #[error("arbitrage arithmetic or counter overflow")]
    Overflow,
    #[error("arbitrage engine is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug, Default)]
pub struct ArbitrageEngine {
    processed: BTreeMap<ArbitrageCommandId, ([u8; 32], ArbitrageDecision)>,
    used_evaluations: BTreeSet<ArbitrageEvaluationId>,
    accepted_commands: u64,
    opportunity_count: u64,
    no_opportunity_count: u64,
    last_recorded_at_ns: Option<i64>,
    last_context_captured_at_ns: Option<i64>,
    last_context_digest: Option<[u8; 32]>,
    last_decision: Option<ArbitrageDecision>,
    halted: Option<String>,
}

impl ArbitrageEngine {
    /// Applies one complete-set evaluation transactionally.
    ///
    /// # Errors
    ///
    /// Returns absorbing integrity, history, canonical, or arithmetic failures.
    pub fn apply(&mut self, command: &ArbitrageCommand) -> Result<ArbitrageDecision, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        let encoded = encode_command(command)?;
        let content = *blake3::hash(&encoded).as_bytes();
        let id = command.command_id();
        if let Some((existing, decision)) = self.processed.get(&id) {
            if *existing == content {
                return Ok(decision.clone());
            }
            return self.halt(Error::IdempotencyConflict);
        }
        if self
            .last_recorded_at_ns
            .is_some_and(|previous| command.recorded_at_ns() < previous)
        {
            return self.halt(Error::ClockRegression);
        }
        let ArbitrageCommand::Evaluate { request, .. } = command;
        if !request.context.verify_digest() {
            return self.halt(Error::ContextDigest);
        }
        if self
            .last_context_captured_at_ns
            .is_some_and(|previous| request.context.captured_at_ns < previous)
            || (self.last_context_captured_at_ns == Some(request.context.captured_at_ns)
                && self
                    .last_context_digest
                    .is_some_and(|digest| digest != request.context.context_digest))
        {
            return self.halt(Error::ContextHistory);
        }
        let mut next = self.clone();
        let decision = match next.evaluate(id, request) {
            Ok(decision) => decision,
            Err(Error::Overflow) => return self.halt(Error::Overflow),
            Err(error) => return Err(error),
        };
        next.accepted_commands = match next.accepted_commands.checked_add(1) {
            Some(value) => value,
            None => return self.halt(Error::Overflow),
        };
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.last_context_captured_at_ns = Some(request.context.captured_at_ns);
        next.last_context_digest = Some(request.context.context_digest);
        next.last_decision = Some(decision.clone());
        next.processed.insert(id, (content, decision.clone()));
        *self = next;
        Ok(decision)
    }

    #[must_use]
    pub fn snapshot(&self) -> ArbitrageSnapshot {
        ArbitrageSnapshot {
            accepted_commands: self.accepted_commands,
            evaluation_count: self.used_evaluations.len(),
            opportunity_count: self.opportunity_count,
            no_opportunity_count: self.no_opportunity_count,
            last_context_captured_at_ns: self.last_context_captured_at_ns,
            last_context_digest: self.last_context_digest,
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

    fn evaluate(
        &mut self,
        command_id: ArbitrageCommandId,
        request: &ArbitrageRequest,
    ) -> Result<ArbitrageDecision, Error> {
        let request_digest = hash_serialized(b"complete-set-request-v1", request);
        let mut reason = if !self.used_evaluations.insert(request.evaluation_id) {
            ArbitrageReason::EvaluationAlreadyUsed
        } else if !request.context.current
            || !request.context.active_ready
            || !books_ready(&request.context)
        {
            ArbitrageReason::SourceNotReady
        } else if request.evaluated_at_ns > request.context.valid_until_ns {
            ArbitrageReason::ContextExpired
        } else if request.evaluated_at_ns > request.expires_at_ns {
            ArbitrageReason::RequestExpired
        } else if !valid_constraints(&request.constraints) {
            ArbitrageReason::InvalidConstraints
        } else {
            ArbitrageReason::ExecutableProfit
        };
        let mut plan = None;
        if reason == ArbitrageReason::ExecutableProfit {
            match calculate_plan(request)? {
                Calculation::Plan(value) => plan = Some(*value),
                Calculation::Reject(value) => reason = value,
            }
        }
        let status = if plan.is_some() {
            self.opportunity_count = self
                .opportunity_count
                .checked_add(1)
                .ok_or(Error::Overflow)?;
            ArbitrageStatus::Opportunity
        } else {
            self.no_opportunity_count = self
                .no_opportunity_count
                .checked_add(1)
                .ok_or(Error::Overflow)?;
            ArbitrageStatus::NoOpportunity
        };
        let mut decision = ArbitrageDecision {
            command_id,
            evaluation_id: request.evaluation_id,
            status,
            reason,
            context_digest: request.context.context_digest,
            request_digest,
            plan,
            decision_digest: [0; 32],
        };
        decision.decision_digest = decision_digest(&decision);
        Ok(decision)
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"complete-set-arbitrage-state-v1");
        for id in &self.used_evaluations {
            hasher.update(&id.0);
        }
        for (id, (content, decision)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_into(&mut hasher, decision);
        }
        hash_into(&mut hasher, &self.accepted_commands);
        hash_into(&mut hasher, &self.opportunity_count);
        hash_into(&mut hasher, &self.no_opportunity_count);
        hash_into(&mut hasher, &self.last_recorded_at_ns);
        hash_into(&mut hasher, &self.last_context_captured_at_ns);
        hash_into(&mut hasher, &self.last_context_digest);
        hash_into(&mut hasher, &self.last_decision);
        hash_into(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }

    fn halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }
}

enum Calculation {
    Plan(Box<ArbitragePlan>),
    Reject(ArbitrageReason),
}

fn calculate_plan(request: &ArbitrageRequest) -> Result<Calculation, Error> {
    let (up, down, side) = selected_levels(request).ok_or(Error::Overflow)?;
    let quantity = request
        .constraints
        .max_quantity_micros
        .min(up.quantity_micros)
        .min(down.quantity_micros);
    if quantity < request.constraints.min_quantity_micros
        || request.constraints.partial_fill_micros > quantity
    {
        return Ok(Calculation::Reject(ArbitrageReason::InsufficientLiquidity));
    }
    let up_value = leg_value(up.price_micros, quantity, side)?;
    let down_value = leg_value(down.price_micros, quantity, side)?;
    let fees = request
        .constraints
        .up_max_fee_micros
        .checked_add(request.constraints.down_max_fee_micros)
        .ok_or(Error::Overflow)?;
    let conversion = request.constraints.conversion_max_cost_micros;
    let (deployed, profit) = match request.direction {
        ArbitrageDirection::BuyPair => {
            let deployed = up_value
                .checked_add(down_value)
                .and_then(|value| value.checked_add(fees))
                .and_then(|value| value.checked_add(conversion))
                .ok_or(Error::Overflow)?;
            (deployed, i128::from(quantity).checked_sub(deployed))
        }
        ArbitrageDirection::SellPair => {
            let deployed = i128::from(quantity)
                .checked_add(conversion)
                .ok_or(Error::Overflow)?;
            let profit = up_value
                .checked_add(down_value)
                .and_then(|value| value.checked_sub(fees))
                .and_then(|value| value.checked_sub(deployed));
            (deployed, profit)
        }
    };
    let profit = profit.ok_or(Error::Overflow)?;
    if profit <= 0 {
        return Ok(Calculation::Reject(ArbitrageReason::NonPositiveProfit));
    }
    if profit < request.constraints.min_net_profit_micros {
        return Ok(Calculation::Reject(ArbitrageReason::ProfitBelowMinimum));
    }
    if deployed <= 0 {
        return Ok(Calculation::Reject(ArbitrageReason::UndefinedRoi));
    }
    let roi = profit
        .checked_mul(BPS_DENOMINATOR)
        .ok_or(Error::Overflow)?
        .checked_div(deployed)
        .ok_or(Error::Overflow)?;
    let roi_bps = u64::try_from(roi).map_err(|_| Error::Overflow)?;
    if roi_bps < request.constraints.min_roi_bps {
        return Ok(Calculation::Reject(ArbitrageReason::RoiBelowMinimum));
    }
    let expires_at_ns = request.expires_at_ns.min(request.context.valid_until_ns);
    let intents = [
        proposal_intent(
            request,
            true,
            side,
            quantity,
            up.price_micros,
            expires_at_ns,
        ),
        proposal_intent(
            request,
            false,
            side,
            quantity,
            down.price_micros,
            expires_at_ns,
        ),
    ];
    let mut plan = ArbitragePlan {
        direction: request.direction,
        quantity_micros: quantity,
        up_price_micros: up.price_micros,
        down_price_micros: down.price_micros,
        up_value_micros: up_value,
        down_value_micros: down_value,
        fee_budget_micros: fees,
        conversion_cost_micros: conversion,
        deployed_capital_micros: deployed,
        net_profit_micros: profit,
        roi_bps,
        intents,
        plan_digest: [0; 32],
    };
    plan.plan_digest = plan_digest(&plan);
    Ok(Calculation::Plan(Box::new(plan)))
}

fn proposal_intent(
    request: &ArbitrageRequest,
    up: bool,
    side: OrderSide,
    quantity: i64,
    price: i64,
    expires_at_ns: i64,
) -> ProposalIntent {
    let fee = if up {
        request.constraints.up_max_fee_micros
    } else {
        request.constraints.down_max_fee_micros
    };
    ProposalIntent {
        proposal_id: derived_proposal_id(request, up),
        strategy: StrategyClass::CompleteSetArbitrage,
        token: if up {
            request.context.up_token.clone()
        } else {
            request.context.down_token.clone()
        },
        side,
        quantity_micros: i128::from(quantity),
        partial_fill_micros: i128::from(request.constraints.partial_fill_micros),
        limit_price_micros: price,
        max_fee_micros: fee,
        evaluated_at_ns: request.evaluated_at_ns,
        expires_at_ns,
    }
}

fn selected_levels(
    request: &ArbitrageRequest,
) -> Option<(ProposalLevel, ProposalLevel, OrderSide)> {
    let up = request.context.up_book?;
    let down = request.context.down_book?;
    match request.direction {
        ArbitrageDirection::BuyPair => Some((up.best_ask?, down.best_ask?, OrderSide::Buy)),
        ArbitrageDirection::SellPair => Some((up.best_bid?, down.best_bid?, OrderSide::Sell)),
    }
}

fn leg_value(price: i64, quantity: i64, side: OrderSide) -> Result<i128, Error> {
    let product = i128::from(price)
        .checked_mul(i128::from(quantity))
        .ok_or(Error::Overflow)?;
    match side {
        OrderSide::Buy => product
            .checked_add(MICROS_PER_UNIT - 1)
            .map(|value| value / MICROS_PER_UNIT)
            .ok_or(Error::Overflow),
        OrderSide::Sell => Ok(product / MICROS_PER_UNIT),
    }
}

fn books_ready(context: &StrategyContext) -> bool {
    [context.up_book, context.down_book].iter().all(|book| {
        book.is_some_and(|value| {
            value.authoritative && value.best_bid.is_some() && value.best_ask.is_some()
        })
    })
}

fn valid_constraints(value: &ArbitrageConstraints) -> bool {
    value.min_quantity_micros > 0
        && value.max_quantity_micros >= value.min_quantity_micros
        && value.max_quantity_micros <= MAX_QUANTITY_MICROS
        && value.partial_fill_micros > 0
        && value.partial_fill_micros < value.min_quantity_micros
        && (0..=MAX_FEE_MICROS).contains(&value.up_max_fee_micros)
        && (0..=MAX_FEE_MICROS).contains(&value.down_max_fee_micros)
        && (0..=MAX_COST_MICROS).contains(&value.conversion_max_cost_micros)
        && (1..=MAX_COST_MICROS).contains(&value.min_net_profit_micros)
        && value.min_roi_bps <= MAX_ROI_BPS
}

fn derived_proposal_id(request: &ArbitrageRequest, up: bool) -> ProposalId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"complete-set-proposal-v1");
    hasher.update(&request.evaluation_id.0);
    hasher.update(&request.context.context_digest);
    hasher.update(&[u8::from(up)]);
    hash_into(&mut hasher, &request.direction);
    hash_into(&mut hasher, &request.constraints);
    ProposalId(*hasher.finalize().as_bytes())
}

fn plan_digest(plan: &ArbitragePlan) -> [u8; 32] {
    let mut value = plan.clone();
    value.plan_digest = [0; 32];
    hash_serialized(b"complete-set-plan-v1", &value)
}

fn decision_digest(decision: &ArbitrageDecision) -> [u8; 32] {
    let mut value = decision.clone();
    value.decision_digest = [0; 32];
    hash_serialized(b"complete-set-decision-v1", &value)
}

fn hash_serialized<T: Serialize>(tag: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tag);
    hash_into(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_into<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("accepted arbitrage state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: ArbitrageCommand,
}

fn validate_command(command: &ArbitrageCommand) -> Result<(), Error> {
    let ArbitrageCommand::Evaluate {
        request,
        recorded_at_ns,
        ..
    } = command;
    if *recorded_at_ns < 0
        || request.evaluated_at_ns != *recorded_at_ns
        || request.context.captured_at_ns > *recorded_at_ns
    {
        return Err(Error::Timestamp);
    }
    Ok(())
}

/// Encodes one bounded canonical arbitrage command.
///
/// # Errors
///
/// Rejects invalid timestamps and oversized commands.
pub fn encode_command(command: &ArbitrageCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one exact bounded arbitrage command.
///
/// # Errors
///
/// Rejects malformed, trailing, oversized, unsupported, or invalid input.
pub fn decode_command(bytes: &[u8]) -> Result<ArbitrageCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let wire = WireCommand::deserialize(&mut deserializer)
        .map_err(|error| Error::Json(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| Error::Json(error.to_string()))?;
    if wire.version != WIRE_VERSION {
        return Err(Error::Version(wire.version));
    }
    validate_command(&wire.command)?;
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
