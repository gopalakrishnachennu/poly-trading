#![forbid(unsafe_code)]

//! Paper-only paired placement policy over transactionally staged capital.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurablePairedPolicy,
    PairedPolicyCheckpoint, PairedPolicyRecovery, StorageError,
};

use accounting_ledger::ReservationStatus;
use order_intent_policy::{ExchangeMode, ExchangeModeObservation};
use paired_capital_staging::{
    CapitalStagingRuntime, PairStageId, PairStageRecord, PairStageStatus, StagingCommand,
    StagingCommandId, StagingDetail,
};
use paired_opportunity_runtime::PairedCommand;
use portfolio_risk::{order_exposure_digest, OrderExposure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_PERMISSION_NS: i64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PairedPolicyCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PairPermitId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegRole {
    First,
    Hedge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegState {
    NotAuthorized,
    Authorized,
    Expired,
    Submitted,
    Delayed,
    Live,
    PartiallyMatched,
    Unknown,
    FullyMatched,
    PartiallyMatchedTerminal,
    NoFillTerminal,
}

impl LegState {
    const fn possible_fill(self) -> bool {
        matches!(
            self,
            Self::Submitted
                | Self::Delayed
                | Self::Live
                | Self::PartiallyMatched
                | Self::Unknown
                | Self::FullyMatched
                | Self::PartiallyMatchedTerminal
        )
    }

    const fn safe_terminal(self) -> bool {
        matches!(
            self,
            Self::NotAuthorized | Self::Expired | Self::NoFillTerminal
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairPermit {
    pub permit_id: PairPermitId,
    pub stage_id: PairStageId,
    pub stage_record_digest: [u8; 32],
    pub leg_index: u8,
    pub role: LegRole,
    pub order: OrderExposure,
    pub candidate_digest: [u8; 32],
    pub reservation_id: accounting_ledger::ReservationId,
    pub mode_sequence: u64,
    pub valid_from_ns: i64,
    pub valid_until_ns: i64,
    pub permit_digest: [u8; 32],
}

impl PairPermit {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.permit_digest == permit_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairPolicyRecord {
    pub stage_id: PairStageId,
    pub stage_record_digest: [u8; 32],
    pub first_leg: Option<u8>,
    pub legs: [LegState; 2],
    pub permits: [Option<PairPermit>; 2],
    pub source_sequences: [Option<u64>; 2],
    pub source_observed_at_ns: [Option<i64>; 2],
    pub ever_partially_matched: [bool; 2],
    pub aborted: bool,
    pub record_digest: [u8; 32],
}

impl PairPolicyRecord {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.record_digest == policy_record_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PairedPolicyCommand {
    Fund {
        command_id: PairedPolicyCommandId,
        amount_micros: i128,
        recorded_at_ns: i64,
    },
    Stage {
        command_id: PairedPolicyCommandId,
        paired_command: Box<PairedCommand>,
        recorded_at_ns: i64,
    },
    ObserveMode {
        command_id: PairedPolicyCommandId,
        observation: ExchangeModeObservation,
        recorded_at_ns: i64,
    },
    AuthorizeFirst {
        command_id: PairedPolicyCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        max_mode_age_ns: i64,
        valid_until_ns: i64,
        recorded_at_ns: i64,
    },
    AuthorizeHedge {
        command_id: PairedPolicyCommandId,
        stage_id: PairStageId,
        max_mode_age_ns: i64,
        valid_until_ns: i64,
        recorded_at_ns: i64,
    },
    ObserveLeg {
        command_id: PairedPolicyCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        permit_id: PairPermitId,
        state: LegState,
        source_sequence: u64,
        observed_at_ns: i64,
        recorded_at_ns: i64,
    },
    Expire {
        command_id: PairedPolicyCommandId,
        stage_id: PairStageId,
        recorded_at_ns: i64,
    },
    AbortSafe {
        command_id: PairedPolicyCommandId,
        stage_id: PairStageId,
        recorded_at_ns: i64,
    },
}

impl PairedPolicyCommand {
    #[must_use]
    pub const fn command_id(&self) -> PairedPolicyCommandId {
        match self {
            Self::Fund { command_id, .. }
            | Self::Stage { command_id, .. }
            | Self::ObserveMode { command_id, .. }
            | Self::AuthorizeFirst { command_id, .. }
            | Self::AuthorizeHedge { command_id, .. }
            | Self::ObserveLeg { command_id, .. }
            | Self::Expire { command_id, .. }
            | Self::AbortSafe { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Fund { recorded_at_ns, .. }
            | Self::Stage { recorded_at_ns, .. }
            | Self::ObserveMode { recorded_at_ns, .. }
            | Self::AuthorizeFirst { recorded_at_ns, .. }
            | Self::AuthorizeHedge { recorded_at_ns, .. }
            | Self::ObserveLeg { recorded_at_ns, .. }
            | Self::Expire { recorded_at_ns, .. }
            | Self::AbortSafe { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedPolicyStatus {
    Accepted,
    Denied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedPolicyReason {
    Funded,
    Staged,
    PairNoTrade,
    ModeAccepted,
    PermissionIssued,
    StageUnknown,
    StageInactive,
    ModeUnavailable,
    ModeStale,
    StageStale,
    InvalidValidity,
    InvalidLeg,
    FirstAlreadySelected,
    HedgeNotReady,
    LegAlreadyAuthorized,
    LifecycleAccepted,
    LifecycleDenied,
    PermissionExpired,
    NothingExpired,
    UnsafeAbort,
    Aborted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedPolicyDecision {
    pub command_id: PairedPolicyCommandId,
    pub status: PairedPolicyStatus,
    pub reason: PairedPolicyReason,
    pub stage_id: Option<PairStageId>,
    pub permit: Option<PairPermit>,
    pub decision_digest: [u8; 32],
}

impl PairedPolicyDecision {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == decision_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairedPolicySnapshot {
    pub accepted_commands: u64,
    pub mode: ExchangeMode,
    pub mode_sequence: Option<u64>,
    pub staging_digest: [u8; 32],
    pub pair_records: BTreeMap<PairStageId, PairPolicyRecord>,
    pub reserved_cash_micros: i128,
    pub reserved_tokens: Vec<accounting_ledger::ConfirmedTokenBalance>,
    pub active_reservation_count: usize,
    pub possible_exposure_legs: usize,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("paired policy command timestamp is invalid")]
    Timestamp,
    #[error("paired policy command exceeds its canonical bound")]
    CommandBound,
    #[error("paired policy command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported paired policy command version: {0}")]
    Version(u16),
    #[error("paired policy command id was reused for different content")]
    IdempotencyConflict,
    #[error("paired policy clock regressed")]
    ClockRegression,
    #[error("exchange-mode history regressed or equivocated")]
    ModeHistory,
    #[error("paired policy child or subject boundary was substituted")]
    Boundary,
    #[error("paired policy arithmetic or counter overflow")]
    Overflow,
    #[error("paired staging child failed: {0}")]
    Staging(String),
    #[error("paired policy is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug, Default)]
pub struct PairedPlacementPolicy {
    staging: CapitalStagingRuntime,
    mode: Option<ExchangeModeObservation>,
    records: BTreeMap<PairStageId, PairPolicyRecord>,
    processed: BTreeMap<PairedPolicyCommandId, ([u8; 32], PairedPolicyDecision)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    last_decision: Option<PairedPolicyDecision>,
    halted: Option<String>,
}

impl PairedPlacementPolicy {
    /// Applies one paper-only paired policy command atomically.
    ///
    /// # Errors
    ///
    /// Returns absorbing history, boundary, child, arithmetic, or codec errors.
    pub fn apply(&mut self, command: &PairedPolicyCommand) -> Result<PairedPolicyDecision, Error> {
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
        let mut decision = match next.apply_fresh(command) {
            Ok(value) => value,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = match next.accepted_commands.checked_add(1) {
            Some(value) => value,
            None => return self.halt(Error::Overflow),
        };
        decision.decision_digest = decision_digest(&decision);
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.last_decision = Some(decision.clone());
        next.processed.insert(id, (content, decision.clone()));
        *self = next;
        Ok(decision)
    }

    #[allow(clippy::too_many_lines)]
    fn apply_fresh(
        &mut self,
        command: &PairedPolicyCommand,
    ) -> Result<PairedPolicyDecision, Error> {
        match command {
            PairedPolicyCommand::Fund {
                command_id,
                amount_micros,
                recorded_at_ns,
            } => {
                self.staging_apply(&StagingCommand::Fund {
                    command_id: derived_staging_id(*command_id, 0),
                    amount_micros: *amount_micros,
                    recorded_at_ns: *recorded_at_ns,
                })?;
                Ok(decision(
                    *command_id,
                    PairedPolicyStatus::Accepted,
                    PairedPolicyReason::Funded,
                    None,
                    None,
                ))
            }
            PairedPolicyCommand::Stage {
                command_id,
                paired_command,
                recorded_at_ns,
            } => {
                let staged = self.staging_apply(&StagingCommand::Stage {
                    command_id: derived_staging_id(*command_id, 0),
                    paired_command: paired_command.clone(),
                    recorded_at_ns: *recorded_at_ns,
                })?;
                match staged.detail {
                    StagingDetail::FullyReserved { record } => {
                        self.validate_stage(&record)?;
                        let stage_id = record.stage_id;
                        if self.records.contains_key(&stage_id) {
                            return Err(Error::Boundary);
                        }
                        self.records.insert(stage_id, new_policy_record(&record));
                        Ok(decision(
                            *command_id,
                            PairedPolicyStatus::Accepted,
                            PairedPolicyReason::Staged,
                            Some(stage_id),
                            None,
                        ))
                    }
                    StagingDetail::PairNoTrade { .. } => Ok(decision(
                        *command_id,
                        PairedPolicyStatus::Denied,
                        PairedPolicyReason::PairNoTrade,
                        None,
                        None,
                    )),
                    _ => Err(Error::Boundary),
                }
            }
            PairedPolicyCommand::ObserveMode {
                command_id,
                observation,
                ..
            } => self.observe_mode(*command_id, observation),
            PairedPolicyCommand::AuthorizeFirst {
                command_id,
                stage_id,
                leg_index,
                max_mode_age_ns,
                valid_until_ns,
                recorded_at_ns,
            } => self.authorize(
                *command_id,
                *stage_id,
                *leg_index,
                LegRole::First,
                *max_mode_age_ns,
                *valid_until_ns,
                *recorded_at_ns,
            ),
            PairedPolicyCommand::AuthorizeHedge {
                command_id,
                stage_id,
                max_mode_age_ns,
                valid_until_ns,
                recorded_at_ns,
            } => {
                let record = self.records.get(stage_id).ok_or(Error::Boundary)?;
                let first = record.first_leg.ok_or(Error::Boundary)?;
                let hedge = 1_u8.checked_sub(first).ok_or(Error::Boundary)?;
                self.authorize(
                    *command_id,
                    *stage_id,
                    hedge,
                    LegRole::Hedge,
                    *max_mode_age_ns,
                    *valid_until_ns,
                    *recorded_at_ns,
                )
            }
            PairedPolicyCommand::ObserveLeg {
                command_id,
                stage_id,
                leg_index,
                permit_id,
                state,
                source_sequence,
                observed_at_ns,
                recorded_at_ns,
            } => self.observe_leg(
                *command_id,
                *stage_id,
                *leg_index,
                *permit_id,
                *state,
                *source_sequence,
                *observed_at_ns,
                *recorded_at_ns,
            ),
            PairedPolicyCommand::Expire {
                command_id,
                stage_id,
                recorded_at_ns,
            } => self.expire(*command_id, *stage_id, *recorded_at_ns),
            PairedPolicyCommand::AbortSafe {
                command_id,
                stage_id,
                recorded_at_ns,
            } => self.abort_safe(*command_id, *stage_id, *recorded_at_ns),
        }
    }

    fn observe_mode(
        &mut self,
        command_id: PairedPolicyCommandId,
        observation: &ExchangeModeObservation,
    ) -> Result<PairedPolicyDecision, Error> {
        if observation.observed_at_ns < 0 || observation.valid_until_ns < observation.observed_at_ns
        {
            return Err(Error::Timestamp);
        }
        if let Some(previous) = &self.mode {
            if observation.sequence < previous.sequence
                || (observation.sequence == previous.sequence && observation != previous)
                || observation.observed_at_ns < previous.observed_at_ns
            {
                return Err(Error::ModeHistory);
            }
        }
        self.mode = Some(observation.clone());
        Ok(decision(
            command_id,
            PairedPolicyStatus::Accepted,
            PairedPolicyReason::ModeAccepted,
            None,
            None,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn authorize(
        &mut self,
        command_id: PairedPolicyCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        role: LegRole,
        max_mode_age_ns: i64,
        valid_until_ns: i64,
        at: i64,
    ) -> Result<PairedPolicyDecision, Error> {
        let denial = self.authorization_denial(
            stage_id,
            leg_index,
            role,
            max_mode_age_ns,
            valid_until_ns,
            at,
        )?;
        if let Some(reason) = denial {
            return Ok(decision(
                command_id,
                PairedPolicyStatus::Denied,
                reason,
                Some(stage_id),
                None,
            ));
        }
        let stage = self
            .staging
            .stage_record(stage_id)
            .cloned()
            .ok_or(Error::Boundary)?;
        self.validate_stage(&stage)?;
        let index = usize::from(leg_index);
        let mode = self.mode.as_ref().ok_or(Error::Boundary)?;
        let mut permit = PairPermit {
            permit_id: derive_permit_id(stage_id, leg_index, role),
            stage_id,
            stage_record_digest: stage.record_digest,
            leg_index,
            role,
            order: stage.candidates[index].clone(),
            candidate_digest: stage.candidate_digests[index],
            reservation_id: stage.reservation_ids[index],
            mode_sequence: mode.sequence,
            valid_from_ns: at,
            valid_until_ns,
            permit_digest: [0; 32],
        };
        permit.permit_digest = permit_digest(&permit);
        let record = self.records.get_mut(&stage_id).ok_or(Error::Boundary)?;
        if role == LegRole::First {
            record.first_leg = Some(leg_index);
        }
        record.legs[index] = LegState::Authorized;
        record.permits[index] = Some(permit.clone());
        refresh_record(record);
        Ok(decision(
            command_id,
            PairedPolicyStatus::Accepted,
            PairedPolicyReason::PermissionIssued,
            Some(stage_id),
            Some(permit),
        ))
    }

    fn authorization_denial(
        &self,
        stage_id: PairStageId,
        leg_index: u8,
        role: LegRole,
        max_mode_age_ns: i64,
        valid_until_ns: i64,
        at: i64,
    ) -> Result<Option<PairedPolicyReason>, Error> {
        if leg_index > 1 {
            return Ok(Some(PairedPolicyReason::InvalidLeg));
        }
        let Some(stage) = self.staging.stage_record(stage_id) else {
            return Ok(Some(PairedPolicyReason::StageUnknown));
        };
        self.validate_stage(stage)?;
        if stage.status != PairStageStatus::FullyReserved {
            return Ok(Some(PairedPolicyReason::StageInactive));
        }
        let record = self.records.get(&stage_id).ok_or(Error::Boundary)?;
        if record.aborted {
            return Ok(Some(PairedPolicyReason::StageInactive));
        }
        if at
            > stage
                .staged_at_ns
                .checked_add(MAX_PERMISSION_NS)
                .ok_or(Error::Overflow)?
        {
            return Ok(Some(PairedPolicyReason::StageStale));
        }
        let candidate_expires_at_ns = stage.candidate_expires_at_ns[usize::from(leg_index)];
        if at >= candidate_expires_at_ns {
            return Ok(Some(PairedPolicyReason::StageStale));
        }
        if max_mode_age_ns < 0
            || valid_until_ns <= at
            || valid_until_ns - at > MAX_PERMISSION_NS
            || valid_until_ns
                > stage
                    .staged_at_ns
                    .checked_add(MAX_PERMISSION_NS)
                    .ok_or(Error::Overflow)?
            || valid_until_ns > candidate_expires_at_ns
        {
            return Ok(Some(PairedPolicyReason::InvalidValidity));
        }
        let Some(mode) = &self.mode else {
            return Ok(Some(PairedPolicyReason::ModeUnavailable));
        };
        if mode.mode != ExchangeMode::Normal {
            return Ok(Some(PairedPolicyReason::ModeUnavailable));
        }
        if at < mode.observed_at_ns
            || at > mode.valid_until_ns
            || at - mode.observed_at_ns > max_mode_age_ns
        {
            return Ok(Some(PairedPolicyReason::ModeStale));
        }
        let index = usize::from(leg_index);
        if record.legs[index] != LegState::NotAuthorized {
            return Ok(Some(PairedPolicyReason::LegAlreadyAuthorized));
        }
        match role {
            LegRole::First if record.first_leg.is_some() => {
                Ok(Some(PairedPolicyReason::FirstAlreadySelected))
            }
            LegRole::Hedge => {
                let Some(first) = record.first_leg else {
                    return Ok(Some(PairedPolicyReason::HedgeNotReady));
                };
                if first == leg_index || record.legs[usize::from(first)] != LegState::FullyMatched {
                    Ok(Some(PairedPolicyReason::HedgeNotReady))
                } else {
                    Ok(None)
                }
            }
            LegRole::First => Ok(None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn observe_leg(
        &mut self,
        command_id: PairedPolicyCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        permit_id: PairPermitId,
        state: LegState,
        source_sequence: u64,
        observed_at_ns: i64,
        at: i64,
    ) -> Result<PairedPolicyDecision, Error> {
        if leg_index > 1 || observed_at_ns < 0 || observed_at_ns > at {
            return Err(Error::Timestamp);
        }
        let index = usize::from(leg_index);
        let record = self.records.get_mut(&stage_id).ok_or(Error::Boundary)?;
        let permit = record.permits[index].as_ref().ok_or(Error::Boundary)?;
        if !permit.verify_digest() || permit.permit_id != permit_id {
            return Err(Error::Boundary);
        }
        if record.source_sequences[index].is_some_and(|previous| source_sequence <= previous) {
            return Err(Error::Boundary);
        }
        if record.source_observed_at_ns[index].is_some_and(|previous| observed_at_ns < previous) {
            return Err(Error::Boundary);
        }
        let current = record.legs[index];
        if !valid_transition(current, state, record.ever_partially_matched[index]) {
            return Err(Error::Boundary);
        }
        record.legs[index] = state;
        record.source_sequences[index] = Some(source_sequence);
        record.source_observed_at_ns[index] = Some(observed_at_ns);
        if state == LegState::PartiallyMatched {
            record.ever_partially_matched[index] = true;
        }
        refresh_record(record);
        Ok(decision(
            command_id,
            PairedPolicyStatus::Accepted,
            PairedPolicyReason::LifecycleAccepted,
            Some(stage_id),
            None,
        ))
    }

    fn expire(
        &mut self,
        command_id: PairedPolicyCommandId,
        stage_id: PairStageId,
        at: i64,
    ) -> Result<PairedPolicyDecision, Error> {
        let record = self.records.get_mut(&stage_id).ok_or(Error::Boundary)?;
        let mut expired = false;
        for index in 0..2 {
            if record.legs[index] == LegState::Authorized
                && record.permits[index]
                    .as_ref()
                    .is_some_and(|permit| at >= permit.valid_until_ns)
            {
                record.legs[index] = LegState::Expired;
                expired = true;
            }
        }
        refresh_record(record);
        Ok(decision(
            command_id,
            PairedPolicyStatus::Accepted,
            if expired {
                PairedPolicyReason::PermissionExpired
            } else {
                PairedPolicyReason::NothingExpired
            },
            Some(stage_id),
            None,
        ))
    }

    fn abort_safe(
        &mut self,
        command_id: PairedPolicyCommandId,
        stage_id: PairStageId,
        at: i64,
    ) -> Result<PairedPolicyDecision, Error> {
        let safe = self.records.get(&stage_id).is_some_and(|record| {
            !record.aborted && record.legs.iter().all(|state| state.safe_terminal())
        });
        if !safe {
            return Ok(decision(
                command_id,
                PairedPolicyStatus::Denied,
                PairedPolicyReason::UnsafeAbort,
                Some(stage_id),
                None,
            ));
        }
        let result = self.staging_apply(&StagingCommand::Abort {
            command_id: derived_staging_id(command_id, 1),
            stage_id,
            recorded_at_ns: at,
        })?;
        if !matches!(result.detail, StagingDetail::Aborted { .. }) {
            return Err(Error::Boundary);
        }
        let record = self.records.get_mut(&stage_id).ok_or(Error::Boundary)?;
        record.aborted = true;
        refresh_record(record);
        Ok(decision(
            command_id,
            PairedPolicyStatus::Accepted,
            PairedPolicyReason::Aborted,
            Some(stage_id),
            None,
        ))
    }

    fn validate_stage(&self, stage: &PairStageRecord) -> Result<(), Error> {
        if !stage.verify_digest()
            || stage.candidates[0].order_id != stage.order_ids[0]
            || stage.candidates[1].order_id != stage.order_ids[1]
            || order_exposure_digest(&stage.candidates[0]) != stage.candidate_digests[0]
            || order_exposure_digest(&stage.candidates[1]) != stage.candidate_digests[1]
        {
            return Err(Error::Boundary);
        }
        for index in 0..2 {
            let reservation = self
                .staging
                .reservation(stage.reservation_ids[index])
                .ok_or(Error::Boundary)?;
            if stage.status == PairStageStatus::FullyReserved
                && reservation.status != ReservationStatus::Active
            {
                return Err(Error::Boundary);
            }
        }
        Ok(())
    }

    fn staging_apply(
        &mut self,
        command: &StagingCommand,
    ) -> Result<paired_capital_staging::StagingDecision, Error> {
        self.staging
            .apply(command)
            .map_err(|error| Error::Staging(error.to_string()))
    }

    #[must_use]
    pub fn record(&self, stage_id: PairStageId) -> Option<&PairPolicyRecord> {
        self.records.get(&stage_id)
    }

    #[must_use]
    pub fn staging(&self) -> &CapitalStagingRuntime {
        &self.staging
    }

    #[doc(hidden)]
    pub fn settlement_apply_batch(
        &mut self,
        commands: &[accounting_ledger::LedgerCommand],
    ) -> Result<(), Error> {
        self.staging
            .settlement_apply_batch(commands)
            .map_err(|error| Error::Staging(error.to_string()))
    }

    #[must_use]
    pub fn snapshot(&self) -> PairedPolicySnapshot {
        let ledger = self.staging.ledger_risk_view();
        PairedPolicySnapshot {
            accepted_commands: self.accepted_commands,
            mode: self
                .mode
                .as_ref()
                .map_or(ExchangeMode::Unknown, |value| value.mode),
            mode_sequence: self.mode.as_ref().map(|value| value.sequence),
            staging_digest: self.staging.snapshot().digest,
            pair_records: self.records.clone(),
            reserved_cash_micros: ledger.cash_reserved_micros,
            reserved_tokens: ledger.reserved_tokens,
            active_reservation_count: self
                .records
                .values()
                .filter(|record| !record.aborted)
                .map(|_| 2_usize)
                .sum(),
            possible_exposure_legs: self
                .records
                .values()
                .flat_map(|record| record.legs)
                .filter(|state| state.possible_fill())
                .count(),
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
        hasher.update(b"paired-placement-policy-state-v1");
        hasher.update(&self.staging.snapshot().digest);
        hash_into(&mut hasher, &self.mode);
        for (id, record) in &self.records {
            hasher.update(&id.0);
            hash_into(&mut hasher, record);
        }
        for (id, (content, decision)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_into(&mut hasher, decision);
        }
        hash_into(&mut hasher, &self.accepted_commands);
        hash_into(&mut hasher, &self.last_recorded_at_ns);
        hash_into(&mut hasher, &self.last_decision);
        hash_into(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }

    fn halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }
}

fn valid_transition(current: LegState, next: LegState, ever_partial: bool) -> bool {
    match current {
        LegState::Authorized => matches!(
            next,
            LegState::Submitted | LegState::Unknown | LegState::NoFillTerminal
        ),
        LegState::Submitted => matches!(
            next,
            LegState::Delayed
                | LegState::Live
                | LegState::PartiallyMatched
                | LegState::Unknown
                | LegState::FullyMatched
                | LegState::NoFillTerminal
        ),
        LegState::Delayed | LegState::Live => matches!(
            next,
            LegState::Live
                | LegState::PartiallyMatched
                | LegState::Unknown
                | LegState::FullyMatched
                | LegState::NoFillTerminal
        ),
        LegState::PartiallyMatched => matches!(
            next,
            LegState::PartiallyMatched
                | LegState::Unknown
                | LegState::FullyMatched
                | LegState::PartiallyMatchedTerminal
        ),
        LegState::Unknown => {
            matches!(
                next,
                LegState::Submitted
                    | LegState::Delayed
                    | LegState::Live
                    | LegState::PartiallyMatched
                    | LegState::Unknown
                    | LegState::FullyMatched
                    | LegState::NoFillTerminal
                    | LegState::PartiallyMatchedTerminal
            ) && !(ever_partial && next == LegState::NoFillTerminal)
                && (!matches!(next, LegState::PartiallyMatchedTerminal) || ever_partial)
        }
        LegState::NotAuthorized
        | LegState::Expired
        | LegState::FullyMatched
        | LegState::PartiallyMatchedTerminal
        | LegState::NoFillTerminal => false,
    }
}

fn new_policy_record(stage: &PairStageRecord) -> PairPolicyRecord {
    let mut record = PairPolicyRecord {
        stage_id: stage.stage_id,
        stage_record_digest: stage.record_digest,
        first_leg: None,
        legs: [LegState::NotAuthorized; 2],
        permits: [None, None],
        source_sequences: [None, None],
        source_observed_at_ns: [None, None],
        ever_partially_matched: [false; 2],
        aborted: false,
        record_digest: [0; 32],
    };
    refresh_record(&mut record);
    record
}

fn refresh_record(record: &mut PairPolicyRecord) {
    record.record_digest = [0; 32];
    record.record_digest = policy_record_digest(record);
}

fn decision(
    command_id: PairedPolicyCommandId,
    status: PairedPolicyStatus,
    reason: PairedPolicyReason,
    stage_id: Option<PairStageId>,
    permit: Option<PairPermit>,
) -> PairedPolicyDecision {
    PairedPolicyDecision {
        command_id,
        status,
        reason,
        stage_id,
        permit,
        decision_digest: [0; 32],
    }
}

fn derived_staging_id(command_id: PairedPolicyCommandId, discriminator: u8) -> StagingCommandId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-policy-staging-command-v1");
    hasher.update(&command_id.0);
    hasher.update(&[discriminator]);
    StagingCommandId(*hasher.finalize().as_bytes())
}

fn derive_permit_id(stage_id: PairStageId, leg_index: u8, role: LegRole) -> PairPermitId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-paper-permit-id-v1");
    hasher.update(&stage_id.0);
    hasher.update(&[leg_index]);
    hash_into(&mut hasher, &role);
    PairPermitId(*hasher.finalize().as_bytes())
}

fn permit_digest(value: &PairPermit) -> [u8; 32] {
    let mut copy = value.clone();
    copy.permit_digest = [0; 32];
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-paper-permit-v1");
    hash_into(&mut hasher, &copy);
    *hasher.finalize().as_bytes()
}

fn policy_record_digest(value: &PairPolicyRecord) -> [u8; 32] {
    let mut copy = value.clone();
    copy.record_digest = [0; 32];
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-policy-record-v1");
    hash_into(&mut hasher, &copy);
    *hasher.finalize().as_bytes()
}

fn decision_digest(value: &PairedPolicyDecision) -> [u8; 32] {
    let mut copy = value.clone();
    copy.decision_digest = [0; 32];
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-policy-decision-v1");
    hash_into(&mut hasher, &copy);
    *hasher.finalize().as_bytes()
}

fn hash_into<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("accepted paired policy state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: PairedPolicyCommand,
}

fn validate_command(command: &PairedPolicyCommand) -> Result<(), Error> {
    if command.recorded_at_ns() < 0 {
        return Err(Error::Timestamp);
    }
    match command {
        PairedPolicyCommand::Stage {
            paired_command,
            recorded_at_ns,
            ..
        } if paired_command.recorded_at_ns() != *recorded_at_ns => Err(Error::Timestamp),
        PairedPolicyCommand::ObserveMode {
            observation,
            recorded_at_ns,
            ..
        } if observation.observed_at_ns > *recorded_at_ns => Err(Error::Timestamp),
        PairedPolicyCommand::ObserveLeg {
            observed_at_ns,
            recorded_at_ns,
            ..
        } if observed_at_ns > recorded_at_ns => Err(Error::Timestamp),
        _ => Ok(()),
    }
}

/// Encodes one bounded, versioned paired policy command.
///
/// # Errors
///
/// Rejects invalid timestamps, oversized commands, and serialization failures.
pub fn encode_command(command: &PairedPolicyCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one exact bounded paired policy command.
///
/// # Errors
///
/// Rejects malformed, trailing, oversized, or unsupported wire data.
pub fn decode_command(bytes: &[u8]) -> Result<PairedPolicyCommand, Error> {
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
