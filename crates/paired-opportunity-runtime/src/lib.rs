#![forbid(unsafe_code)]

//! Offline single-writer composition of complete-set detection, proposal
//! validation, and combined two-candidate portfolio risk.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurablePairedRuntime,
    PairedCheckpoint, PairedRecovery, StorageError,
};

use accounting_ledger::LedgerRiskView;
use complete_set_arbitrage::{
    ArbitrageCommand, ArbitrageDecision, ArbitrageEngine, ArbitrageStatus,
};
use portfolio_risk::{
    BinaryMarketRisk, DecisionStatus as RiskStatus, PortfolioRiskEngine, RiskCommand,
    RiskCommandId, RiskDecision, RiskLimits, RiskRequest, ShockProfile,
};
use serde::{Deserialize, Serialize};
use settlement_reconciliation::ReconciliationRiskGate;
use std::collections::{BTreeMap, BTreeSet};
use strategy_proposal::{
    ProposalCommand, ProposalCommandId, ProposalDecision, ProposalEngine, ProposalStatus,
};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PairedCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairRiskFrame {
    pub reconciliation: ReconciliationRiskGate,
    pub ledger: LedgerRiskView,
    pub markets: Vec<BinaryMarketRisk>,
    pub open_orders: Vec<portfolio_risk::OrderExposure>,
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
pub enum PairedCommand {
    Evaluate {
        command_id: PairedCommandId,
        arbitrage_command: Box<ArbitrageCommand>,
        risk_frame: Box<PairRiskFrame>,
        recorded_at_ns: i64,
    },
}

impl PairedCommand {
    #[must_use]
    pub const fn command_id(&self) -> PairedCommandId {
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
pub enum PairedStatus {
    RiskEligible,
    NoTrade,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedReason {
    CombinedRiskApproved,
    DetectorNoOpportunity,
    CombinedRiskNoTrade,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedDecision {
    pub command_id: PairedCommandId,
    pub status: PairedStatus,
    pub reason: PairedReason,
    pub arbitrage_decision: ArbitrageDecision,
    pub proposal_decisions: Vec<ProposalDecision>,
    pub risk_decision: Option<RiskDecision>,
    pub decision_digest: [u8; 32],
}

impl PairedDecision {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == decision_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairedSnapshot {
    pub accepted_commands: u64,
    pub risk_eligible_count: u64,
    pub no_trade_count: u64,
    pub arbitrage_digest: [u8; 32],
    pub proposal_digest: [u8; 32],
    pub risk_digest: [u8; 32],
    pub last_decision: Option<PairedDecision>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("paired command timestamp is invalid")]
    Timestamp,
    #[error("paired command exceeds its canonical bound")]
    CommandBound,
    #[error("paired command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported paired command version: {0}")]
    Version(u16),
    #[error("paired command id was reused for different content")]
    IdempotencyConflict,
    #[error("paired runtime clock regressed")]
    ClockRegression,
    #[error("paired child boundary was substituted or became inconsistent")]
    Boundary,
    #[error("arbitrage child failed: {0}")]
    Arbitrage(String),
    #[error("proposal child failed: {0}")]
    Proposal(String),
    #[error("risk child failed: {0}")]
    Risk(String),
    #[error("paired runtime counter overflow")]
    Overflow,
    #[error("paired runtime is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug, Default)]
pub struct PairedRuntime {
    arbitrage: ArbitrageEngine,
    proposal: ProposalEngine,
    risk: PortfolioRiskEngine,
    consumed_arbitrage_decisions: BTreeSet<[u8; 32]>,
    processed: BTreeMap<PairedCommandId, ([u8; 32], PairedDecision)>,
    accepted_commands: u64,
    risk_eligible_count: u64,
    no_trade_count: u64,
    last_recorded_at_ns: Option<i64>,
    last_decision: Option<PairedDecision>,
    halted: Option<String>,
}

impl PairedRuntime {
    /// Applies one paired evaluation through all three owned children.
    ///
    /// # Errors
    ///
    /// Returns absorbing child, boundary, history, arithmetic, or durable errors.
    pub fn apply(&mut self, command: &PairedCommand) -> Result<PairedDecision, Error> {
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
        let mut next = self.clone();
        let decision = match next.evaluate_fresh(command) {
            Ok(value) => value,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = match next.accepted_commands.checked_add(1) {
            Some(value) => value,
            None => return self.halt(Error::Overflow),
        };
        match decision.status {
            PairedStatus::RiskEligible => {
                next.risk_eligible_count = match next.risk_eligible_count.checked_add(1) {
                    Some(value) => value,
                    None => return self.halt(Error::Overflow),
                };
            }
            PairedStatus::NoTrade => {
                next.no_trade_count = match next.no_trade_count.checked_add(1) {
                    Some(value) => value,
                    None => return self.halt(Error::Overflow),
                };
            }
        }
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.last_decision = Some(decision.clone());
        next.processed.insert(id, (content, decision.clone()));
        *self = next;
        Ok(decision)
    }

    fn evaluate_fresh(&mut self, command: &PairedCommand) -> Result<PairedDecision, Error> {
        let PairedCommand::Evaluate {
            command_id,
            arbitrage_command,
            risk_frame,
            recorded_at_ns,
        } = command;
        let arbitrage_decision = self
            .arbitrage
            .apply(arbitrage_command)
            .map_err(|error| Error::Arbitrage(error.to_string()))?;
        if !self
            .consumed_arbitrage_decisions
            .insert(arbitrage_decision.decision_digest)
        {
            return Err(Error::Boundary);
        }
        if arbitrage_decision.status != ArbitrageStatus::Opportunity {
            return Ok(make_decision(
                *command_id,
                PairedStatus::NoTrade,
                PairedReason::DetectorNoOpportunity,
                arbitrage_decision,
                Vec::new(),
                None,
            ));
        }
        let plan = arbitrage_decision.plan.as_ref().ok_or(Error::Boundary)?;
        if !plan.verify_digest() || plan.intents.len() != 2 {
            return Err(Error::Boundary);
        }
        let ArbitrageCommand::Evaluate { request, .. } = arbitrage_command.as_ref();
        if request.context.context_digest != arbitrage_decision.context_digest
            || risk_frame.evaluated_at_ns != *recorded_at_ns
        {
            return Err(Error::Boundary);
        }
        let mut proposal_decisions = Vec::with_capacity(2);
        for (index, intent) in plan.intents.iter().enumerate() {
            let proposal_command = ProposalCommand::Evaluate {
                command_id: derived_proposal_command_id(*command_id, index, plan.plan_digest),
                context: request.context.clone(),
                intent: Box::new(intent.clone()),
                recorded_at_ns: intent.evaluated_at_ns,
            };
            let decision = self
                .proposal
                .apply(&proposal_command)
                .map_err(|error| Error::Proposal(error.to_string()))?;
            if decision.status != ProposalStatus::Candidate || decision.candidate.is_none() {
                return Err(Error::Boundary);
            }
            proposal_decisions.push(decision);
        }
        let first = proposal_decisions[0]
            .candidate
            .clone()
            .ok_or(Error::Boundary)?;
        let second = proposal_decisions[1]
            .candidate
            .clone()
            .ok_or(Error::Boundary)?;
        let risk_request = RiskRequest {
            reconciliation: risk_frame.reconciliation.clone(),
            ledger: risk_frame.ledger.clone(),
            markets: risk_frame.markets.clone(),
            open_orders: risk_frame.open_orders.clone(),
            candidate: first,
            additional_candidates: vec![second],
            shocks: risk_frame.shocks.clone(),
            limits: risk_frame.limits.clone(),
            evaluated_at_ns: risk_frame.evaluated_at_ns,
        };
        let risk_command = RiskCommand::Evaluate {
            command_id: derived_risk_command_id(*command_id, plan.plan_digest),
            request: risk_request,
            recorded_at_ns: risk_frame.evaluated_at_ns,
        };
        let risk_decision = self
            .risk
            .apply(&risk_command)
            .map_err(|error| Error::Risk(error.to_string()))?;
        let (status, reason) = if risk_decision.status == RiskStatus::Approve {
            (
                PairedStatus::RiskEligible,
                PairedReason::CombinedRiskApproved,
            )
        } else {
            (PairedStatus::NoTrade, PairedReason::CombinedRiskNoTrade)
        };
        Ok(make_decision(
            *command_id,
            status,
            reason,
            arbitrage_decision,
            proposal_decisions,
            Some(risk_decision),
        ))
    }

    #[must_use]
    pub fn snapshot(&self) -> PairedSnapshot {
        PairedSnapshot {
            accepted_commands: self.accepted_commands,
            risk_eligible_count: self.risk_eligible_count,
            no_trade_count: self.no_trade_count,
            arbitrage_digest: self.arbitrage.snapshot().digest,
            proposal_digest: self.proposal.snapshot().digest,
            risk_digest: self.risk.snapshot().digest,
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

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"paired-opportunity-runtime-state-v1");
        for digest in [
            self.arbitrage.snapshot().digest,
            self.proposal.snapshot().digest,
            self.risk.snapshot().digest,
        ] {
            hasher.update(&digest);
        }
        for digest in &self.consumed_arbitrage_decisions {
            hasher.update(digest);
        }
        hash_into(&mut hasher, &self.accepted_commands);
        hash_into(&mut hasher, &self.risk_eligible_count);
        hash_into(&mut hasher, &self.no_trade_count);
        hash_into(&mut hasher, &self.last_recorded_at_ns);
        hash_into(&mut hasher, &self.last_decision);
        for (id, (content, decision)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_into(&mut hasher, decision);
        }
        hash_into(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }

    fn halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }
}

fn make_decision(
    command_id: PairedCommandId,
    status: PairedStatus,
    reason: PairedReason,
    arbitrage_decision: ArbitrageDecision,
    proposal_decisions: Vec<ProposalDecision>,
    risk_decision: Option<RiskDecision>,
) -> PairedDecision {
    let mut value = PairedDecision {
        command_id,
        status,
        reason,
        arbitrage_decision,
        proposal_decisions,
        risk_decision,
        decision_digest: [0; 32],
    };
    value.decision_digest = decision_digest(&value);
    value
}

fn derived_proposal_command_id(
    command_id: PairedCommandId,
    index: usize,
    plan_digest: [u8; 32],
) -> ProposalCommandId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-runtime-proposal-command-v1");
    hasher.update(&command_id.0);
    hasher.update(&plan_digest);
    hash_into(&mut hasher, &index);
    ProposalCommandId(*hasher.finalize().as_bytes())
}

fn derived_risk_command_id(command_id: PairedCommandId, plan_digest: [u8; 32]) -> RiskCommandId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-runtime-risk-command-v1");
    hasher.update(&command_id.0);
    hasher.update(&plan_digest);
    RiskCommandId(*hasher.finalize().as_bytes())
}

fn decision_digest(value: &PairedDecision) -> [u8; 32] {
    let mut decision = value.clone();
    decision.decision_digest = [0; 32];
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-opportunity-decision-v1");
    hash_into(&mut hasher, &decision);
    *hasher.finalize().as_bytes()
}

fn hash_into<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("accepted paired state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: PairedCommand,
}

fn validate_command(command: &PairedCommand) -> Result<(), Error> {
    let PairedCommand::Evaluate {
        arbitrage_command,
        risk_frame,
        recorded_at_ns,
        ..
    } = command;
    if *recorded_at_ns < 0
        || risk_frame.evaluated_at_ns != *recorded_at_ns
        || arbitrage_command.recorded_at_ns() != *recorded_at_ns
    {
        return Err(Error::Timestamp);
    }
    Ok(())
}

/// Encodes one bounded canonical paired command.
///
/// # Errors
///
/// Rejects invalid timestamps and oversized commands.
pub fn encode_command(command: &PairedCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one exact bounded paired command.
///
/// # Errors
///
/// Rejects malformed, trailing, oversized, unsupported, or invalid input.
pub fn decode_command(bytes: &[u8]) -> Result<PairedCommand, Error> {
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
