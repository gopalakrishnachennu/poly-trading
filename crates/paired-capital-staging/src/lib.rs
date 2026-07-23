#![forbid(unsafe_code)]

//! Offline all-or-neither capital staging for exact paired opportunities.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableCapitalStaging,
    StagingCheckpoint, StagingRecovery, StorageError,
};

use accounting_ledger::{
    AccountingLedger, ApplyOutcome, CommandId as LedgerCommandId, LedgerCommand,
    LedgerReconciliationView, LedgerRiskView, ReservationAsset, ReservationId, ReservationStatus,
};
use paired_opportunity_runtime::{PairedCommand, PairedDecision, PairedRuntime, PairedStatus};
use portfolio_risk::{order_exposure_digest, OrderExposure, OrderSide, RiskOrderId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MICROS_PER_UNIT: i128 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct StagingCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PairStageId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairStageStatus {
    FullyReserved,
    Aborted,
}

/// Deterministic offline fault points used to prove transactional staging.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagingFault {
    BeforeSecondReservation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairStageRecord {
    pub stage_id: PairStageId,
    pub paired_decision_digest: [u8; 32],
    pub candidates: [OrderExposure; 2],
    pub candidate_expires_at_ns: [i64; 2],
    pub order_ids: [RiskOrderId; 2],
    pub candidate_digests: [[u8; 32]; 2],
    pub reservation_ids: [ReservationId; 2],
    pub status: PairStageStatus,
    pub staged_at_ns: i64,
    pub aborted_at_ns: Option<i64>,
    pub staged_ledger_digest: [u8; 32],
    pub terminal_ledger_digest: Option<[u8; 32]>,
    pub record_digest: [u8; 32],
}

impl PairStageRecord {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.record_digest == record_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum StagingCommand {
    Fund {
        command_id: StagingCommandId,
        amount_micros: i128,
        recorded_at_ns: i64,
    },
    Stage {
        command_id: StagingCommandId,
        paired_command: Box<PairedCommand>,
        recorded_at_ns: i64,
    },
    Abort {
        command_id: StagingCommandId,
        stage_id: PairStageId,
        recorded_at_ns: i64,
    },
    InjectFault {
        command_id: StagingCommandId,
        fault: StagingFault,
        recorded_at_ns: i64,
    },
}

impl StagingCommand {
    #[must_use]
    pub const fn command_id(&self) -> StagingCommandId {
        match self {
            Self::Fund { command_id, .. }
            | Self::Stage { command_id, .. }
            | Self::Abort { command_id, .. }
            | Self::InjectFault { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Fund { recorded_at_ns, .. }
            | Self::Stage { recorded_at_ns, .. }
            | Self::Abort { recorded_at_ns, .. }
            | Self::InjectFault { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum StagingDetail {
    Funded,
    PairNoTrade {
        paired_decision: Box<PairedDecision>,
    },
    FullyReserved {
        record: PairStageRecord,
    },
    Aborted {
        record: PairStageRecord,
    },
    FaultArmed {
        fault: StagingFault,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagingDecision {
    pub command_id: StagingCommandId,
    pub detail: StagingDetail,
    pub decision_digest: [u8; 32],
}

impl StagingDecision {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == decision_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagingSnapshot {
    pub accepted_commands: u64,
    pub ledger: LedgerRiskView,
    pub paired_digest: [u8; 32],
    pub stages: BTreeMap<PairStageId, PairStageRecord>,
    pub active_stage_count: usize,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("staging command timestamp is invalid")]
    Timestamp,
    #[error("staging command exceeds its canonical bound")]
    CommandBound,
    #[error("staging command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported staging command version: {0}")]
    Version(u16),
    #[error("staging command id was reused for different content")]
    IdempotencyConflict,
    #[error("staging clock regressed")]
    ClockRegression,
    #[error("paired or ledger provenance was substituted")]
    Boundary,
    #[error("pair stage already exists")]
    StageExists,
    #[error("pair stage does not exist or is not active")]
    StageInactive,
    #[error("accounting child failed: {0}")]
    Accounting(String),
    #[error("paired child failed: {0}")]
    Paired(String),
    #[error("staging arithmetic or counter overflow")]
    Overflow,
    #[error("capital staging is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug, Default)]
pub struct CapitalStagingRuntime {
    paired: PairedRuntime,
    ledger: AccountingLedger,
    stages: BTreeMap<PairStageId, PairStageRecord>,
    processed: BTreeMap<StagingCommandId, ([u8; 32], StagingDecision)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    last_decision: Option<StagingDecision>,
    halted: Option<String>,
    armed_fault: Option<StagingFault>,
}

impl CapitalStagingRuntime {
    /// Applies one funding, paired staging, or paired abort command atomically.
    ///
    /// # Errors
    ///
    /// Returns absorbing boundary, child, history, arithmetic, or durable errors.
    pub fn apply(&mut self, command: &StagingCommand) -> Result<StagingDecision, Error> {
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
        let detail = match next.apply_fresh(command) {
            Ok(value) => value,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = match next.accepted_commands.checked_add(1) {
            Some(value) => value,
            None => return self.halt(Error::Overflow),
        };
        let mut decision = StagingDecision {
            command_id: id,
            detail,
            decision_digest: [0; 32],
        };
        decision.decision_digest = decision_digest(&decision);
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.last_decision = Some(decision.clone());
        next.processed.insert(id, (content, decision.clone()));
        *self = next;
        Ok(decision)
    }

    fn apply_fresh(&mut self, command: &StagingCommand) -> Result<StagingDetail, Error> {
        match command {
            StagingCommand::Fund {
                command_id,
                amount_micros,
                recorded_at_ns,
            } => {
                if self.active_stage_count() != 0 {
                    return Err(Error::Boundary);
                }
                self.ledger_apply(&LedgerCommand::FundCollateral {
                    command_id: LedgerCommandId(command_id.0),
                    amount_micros: *amount_micros,
                    recorded_at_ns: *recorded_at_ns,
                })?;
                Ok(StagingDetail::Funded)
            }
            StagingCommand::Stage {
                paired_command,
                recorded_at_ns,
                ..
            } => self.stage(paired_command, *recorded_at_ns),
            StagingCommand::Abort {
                stage_id,
                recorded_at_ns,
                ..
            } => self.abort(*stage_id, *recorded_at_ns),
            StagingCommand::InjectFault { fault, .. } => {
                if self.armed_fault.replace(*fault).is_some() {
                    return Err(Error::Boundary);
                }
                Ok(StagingDetail::FaultArmed { fault: *fault })
            }
        }
    }

    fn stage(&mut self, command: &PairedCommand, at: i64) -> Result<StagingDetail, Error> {
        let PairedCommand::Evaluate { risk_frame, .. } = command;
        if risk_frame.ledger != self.ledger.risk_view() {
            return Err(Error::Boundary);
        }
        let paired_decision = self
            .paired
            .apply(command)
            .map_err(|error| Error::Paired(error.to_string()))?;
        if paired_decision.status != PairedStatus::RiskEligible {
            return Ok(StagingDetail::PairNoTrade {
                paired_decision: Box::new(paired_decision),
            });
        }
        let candidates = exact_candidates(&paired_decision)?;
        let plan = paired_decision
            .arbitrage_decision
            .plan
            .as_ref()
            .filter(|plan| plan.verify_digest())
            .ok_or(Error::Boundary)?;
        let candidate_expires_at_ns =
            [plan.intents[0].expires_at_ns, plan.intents[1].expires_at_ns];
        let stage_id = derive_stage_id(paired_decision.decision_digest);
        if self.stages.contains_key(&stage_id) {
            return Err(Error::StageExists);
        }
        let reservation_ids = [
            ReservationId(candidates[0].order_id.0),
            ReservationId(candidates[1].order_id.0),
        ];
        for (index, candidate) in candidates.iter().enumerate() {
            if index == 1 && self.armed_fault == Some(StagingFault::BeforeSecondReservation) {
                self.armed_fault = None;
                return Err(Error::Boundary);
            }
            self.reserve(candidate, reservation_ids[index], stage_id, index, at)?;
        }
        for (index, candidate) in candidates.iter().enumerate() {
            let reservation = self
                .ledger
                .reservation(reservation_ids[index])
                .ok_or(Error::Boundary)?;
            if reservation.status != ReservationStatus::Active
                || reservation.remaining_micros != reservation_amount(candidate)?
                || reservation.asset != reservation_asset(candidate)
            {
                return Err(Error::Boundary);
            }
        }
        let mut record = PairStageRecord {
            stage_id,
            paired_decision_digest: paired_decision.decision_digest,
            candidates: candidates.clone(),
            candidate_expires_at_ns,
            order_ids: [candidates[0].order_id, candidates[1].order_id],
            candidate_digests: [
                order_exposure_digest(&candidates[0]),
                order_exposure_digest(&candidates[1]),
            ],
            reservation_ids,
            status: PairStageStatus::FullyReserved,
            staged_at_ns: at,
            aborted_at_ns: None,
            staged_ledger_digest: self.ledger.snapshot().digest,
            terminal_ledger_digest: None,
            record_digest: [0; 32],
        };
        record.record_digest = record_digest(&record);
        self.stages.insert(stage_id, record.clone());
        Ok(StagingDetail::FullyReserved { record })
    }

    fn abort(&mut self, stage_id: PairStageId, at: i64) -> Result<StagingDetail, Error> {
        let current = self
            .stages
            .get(&stage_id)
            .cloned()
            .filter(|value| value.status == PairStageStatus::FullyReserved)
            .ok_or(Error::StageInactive)?;
        for (index, reservation_id) in current.reservation_ids.iter().enumerate() {
            self.ledger_apply(&LedgerCommand::ReleaseReservation {
                command_id: derived_ledger_id(stage_id, index + 2),
                reservation_id: *reservation_id,
                recorded_at_ns: at,
            })?;
        }
        let mut record = current;
        record.status = PairStageStatus::Aborted;
        record.aborted_at_ns = Some(at);
        record.terminal_ledger_digest = Some(self.ledger.snapshot().digest);
        record.record_digest = [0; 32];
        record.record_digest = record_digest(&record);
        self.stages.insert(stage_id, record.clone());
        Ok(StagingDetail::Aborted { record })
    }

    fn reserve(
        &mut self,
        candidate: &OrderExposure,
        reservation_id: ReservationId,
        stage_id: PairStageId,
        index: usize,
        at: i64,
    ) -> Result<(), Error> {
        let command_id = derived_ledger_id(stage_id, index);
        let command = match candidate.side {
            OrderSide::Buy => LedgerCommand::ReserveCollateral {
                command_id,
                reservation_id,
                amount_micros: reservation_amount(candidate)?,
                recorded_at_ns: at,
            },
            OrderSide::Sell => LedgerCommand::ReserveToken {
                command_id,
                reservation_id,
                token: candidate.token.clone(),
                quantity_micros: candidate.quantity_micros,
                recorded_at_ns: at,
            },
        };
        self.ledger_apply(&command)
    }

    fn ledger_apply(&mut self, command: &LedgerCommand) -> Result<(), Error> {
        match self.ledger.apply(command) {
            Ok(ApplyOutcome::Applied | ApplyOutcome::Duplicate) => Ok(()),
            Err(error) => Err(Error::Accounting(error.to_string())),
        }
    }

    #[must_use]
    pub fn ledger_risk_view(&self) -> LedgerRiskView {
        self.ledger.risk_view()
    }

    /// Returns the authoritative nested-ledger view needed by a composing
    /// settlement owner.
    #[doc(hidden)]
    #[must_use]
    pub fn settlement_reconciliation_view(
        &self,
        command_ids: &BTreeSet<LedgerCommandId>,
    ) -> LedgerReconciliationView {
        self.ledger.reconciliation_view(command_ids)
    }

    /// Applies an already-authorized settlement accounting batch atomically.
    ///
    /// This is composition plumbing for the Phase 2.12/2.13 owners. It
    /// deliberately excludes funding and arbitrary postings.
    ///
    /// # Errors
    ///
    /// Rejects unsupported commands or any accounting invariant failure.
    #[doc(hidden)]
    pub fn settlement_apply_batch(&mut self, commands: &[LedgerCommand]) -> Result<(), Error> {
        let mut next = self.clone();
        for command in commands {
            if !matches!(
                command,
                LedgerCommand::ReserveCollateral { .. }
                    | LedgerCommand::ReserveToken { .. }
                    | LedgerCommand::ConfirmBuy { .. }
                    | LedgerCommand::ConfirmSell { .. }
                    | LedgerCommand::ReleaseReservation { .. }
                    | LedgerCommand::LockPair { .. }
                    | LedgerCommand::ConfirmMerge { .. }
                    | LedgerCommand::ConfirmSplit { .. }
                    | LedgerCommand::ConfirmRedemption { .. }
            ) {
                return Err(Error::Boundary);
            }
            next.ledger_apply(command)?;
        }
        *self = next;
        Ok(())
    }

    #[must_use]
    pub fn stage_record(&self, id: PairStageId) -> Option<&PairStageRecord> {
        self.stages.get(&id)
    }

    #[must_use]
    pub fn reservation(&self, id: ReservationId) -> Option<&accounting_ledger::Reservation> {
        self.ledger.reservation(id)
    }

    #[doc(hidden)]
    #[must_use]
    pub fn conversion_pair_lock(
        &self,
        id: accounting_ledger::LockId,
    ) -> Option<&accounting_ledger::PairLock> {
        self.ledger.pair_lock(id)
    }

    #[must_use]
    pub fn snapshot(&self) -> StagingSnapshot {
        StagingSnapshot {
            accepted_commands: self.accepted_commands,
            ledger: self.ledger.risk_view(),
            paired_digest: self.paired.snapshot().digest,
            stages: self.stages.clone(),
            active_stage_count: self.active_stage_count(),
            halted: self.halted.is_some(),
            halt_reason: self.halted.clone(),
            digest: self.state_digest(),
        }
    }

    #[must_use]
    pub const fn is_halted(&self) -> bool {
        self.halted.is_some()
    }

    fn active_stage_count(&self) -> usize {
        self.stages
            .values()
            .filter(|value| value.status == PairStageStatus::FullyReserved)
            .count()
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"paired-capital-staging-state-v1");
        hasher.update(&self.ledger.snapshot().digest);
        hasher.update(&self.paired.snapshot().digest);
        for (id, stage) in &self.stages {
            hasher.update(&id.0);
            hash_into(&mut hasher, stage);
        }
        hash_into(&mut hasher, &self.accepted_commands);
        hash_into(&mut hasher, &self.last_recorded_at_ns);
        hash_into(&mut hasher, &self.last_decision);
        for (id, (content, decision)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_into(&mut hasher, decision);
        }
        hash_into(&mut hasher, &self.halted);
        hash_into(&mut hasher, &self.armed_fault);
        *hasher.finalize().as_bytes()
    }

    fn halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }
}

fn exact_candidates(decision: &PairedDecision) -> Result<[OrderExposure; 2], Error> {
    if !decision.verify_digest() || decision.proposal_decisions.len() != 2 {
        return Err(Error::Boundary);
    }
    let first = decision.proposal_decisions[0]
        .candidate
        .clone()
        .ok_or(Error::Boundary)?;
    let second = decision.proposal_decisions[1]
        .candidate
        .clone()
        .ok_or(Error::Boundary)?;
    if first.order_id == second.order_id {
        return Err(Error::Boundary);
    }
    Ok([first, second])
}

fn reservation_amount(candidate: &OrderExposure) -> Result<i128, Error> {
    match candidate.side {
        OrderSide::Buy => {
            let product = i128::from(candidate.limit_price_micros)
                .checked_mul(candidate.quantity_micros)
                .ok_or(Error::Overflow)?;
            product
                .checked_add(MICROS_PER_UNIT - 1)
                .map(|value| value / MICROS_PER_UNIT)
                .and_then(|value| value.checked_add(candidate.max_fee_micros))
                .ok_or(Error::Overflow)
        }
        OrderSide::Sell => Ok(candidate.quantity_micros),
    }
}

fn reservation_asset(candidate: &OrderExposure) -> ReservationAsset {
    match candidate.side {
        OrderSide::Buy => ReservationAsset::Collateral,
        OrderSide::Sell => ReservationAsset::Token(candidate.token.clone()),
    }
}

fn derive_stage_id(digest: [u8; 32]) -> PairStageId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-capital-stage-id-v1");
    hasher.update(&digest);
    PairStageId(*hasher.finalize().as_bytes())
}

fn derived_ledger_id(stage_id: PairStageId, index: usize) -> LedgerCommandId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-capital-ledger-command-v1");
    hasher.update(&stage_id.0);
    hash_into(&mut hasher, &index);
    LedgerCommandId(*hasher.finalize().as_bytes())
}

fn record_digest(value: &PairStageRecord) -> [u8; 32] {
    let mut record = value.clone();
    record.record_digest = [0; 32];
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-capital-stage-record-v1");
    hash_into(&mut hasher, &record);
    *hasher.finalize().as_bytes()
}

fn decision_digest(value: &StagingDecision) -> [u8; 32] {
    let mut decision = value.clone();
    decision.decision_digest = [0; 32];
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-capital-decision-v1");
    hash_into(&mut hasher, &decision);
    *hasher.finalize().as_bytes()
}

fn hash_into<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("accepted staging state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: StagingCommand,
}

fn validate_command(command: &StagingCommand) -> Result<(), Error> {
    if command.recorded_at_ns() < 0 {
        return Err(Error::Timestamp);
    }
    if let StagingCommand::Stage {
        paired_command,
        recorded_at_ns,
        ..
    } = command
    {
        if paired_command.recorded_at_ns() != *recorded_at_ns {
            return Err(Error::Timestamp);
        }
    }
    Ok(())
}

/// Encodes one bounded, versioned staging command.
///
/// # Errors
///
/// Rejects invalid timestamps, oversized commands, and serialization failures.
pub fn encode_command(command: &StagingCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one exact bounded staging command.
///
/// # Errors
///
/// Rejects malformed, trailing, oversized, or unsupported wire data.
pub fn decode_command(bytes: &[u8]) -> Result<StagingCommand, Error> {
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
