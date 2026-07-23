#![forbid(unsafe_code)]

//! Deterministic paired paper execution composed with Phase 2.10 policy.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurablePairedExecution,
    PairedExecutionCheckpoint, PairedExecutionRecovery, StorageError,
};

use accounting_ledger::{CommandId as LedgerCommandId, ReservationStatus};
use paired_capital_staging::PairStageId;
use paired_placement_policy::{
    LegState, PairPermit, PairPermitId, PairedPlacementPolicy, PairedPolicyCommand,
    PairedPolicyCommandId, PairedPolicyDecision, PairedPolicyStatus,
};
use paper_execution::{
    ActiveState, ExchangeEvent, ExchangeObservation, MatchFill, OrderState, ReconciliationHandoff,
};
use portfolio_risk::OrderSide;
use serde::{Deserialize, Serialize};
use settlement_reconciliation::{IntentId, Side, TradeIntent};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 512;
const MICROS_PER_UNIT: i128 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PairedExecutionCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedPaperOrder {
    pub permit: PairPermit,
    pub local_submission_id: String,
    pub exchange_order_id: Option<String>,
    pub state: OrderState,
    pub last_source_sequence: Option<u64>,
    pub last_event_time_ns: Option<i64>,
    pub last_received_time_ns: Option<i64>,
    pub cumulative_quantity_micros: i128,
    pub cumulative_consideration_micros: i128,
    pub cumulative_fee_micros: i128,
    pub fills: BTreeMap<String, MatchFill>,
    pub handoffs: Vec<ReconciliationHandoff>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PairedExecutionCommand {
    Policy {
        command_id: PairedExecutionCommandId,
        command: Box<PairedPolicyCommand>,
        recorded_at_ns: i64,
    },
    Submit {
        command_id: PairedExecutionCommandId,
        permit: Box<PairPermit>,
        local_submission_id: String,
        recorded_at_ns: i64,
    },
    RequestCancel {
        command_id: PairedExecutionCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        recorded_at_ns: i64,
    },
    Observe {
        command_id: PairedExecutionCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        observation: Box<ExchangeObservation>,
        recorded_at_ns: i64,
    },
}

impl PairedExecutionCommand {
    #[must_use]
    pub const fn command_id(&self) -> PairedExecutionCommandId {
        match self {
            Self::Policy { command_id, .. }
            | Self::Submit { command_id, .. }
            | Self::RequestCancel { command_id, .. }
            | Self::Observe { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Policy { recorded_at_ns, .. }
            | Self::Submit { recorded_at_ns, .. }
            | Self::RequestCancel { recorded_at_ns, .. }
            | Self::Observe { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedExecutionStatus {
    Applied,
    Denied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedExecutionReason {
    PolicyApplied,
    SubmissionAccepted,
    PermitInvalid,
    PermitExpired,
    PermitAlreadyUsed,
    OrderAlreadyExists,
    CancelRequested,
    CancelDenied,
    ObservationApplied,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedExecutionDecision {
    pub command_id: PairedExecutionCommandId,
    pub status: PairedExecutionStatus,
    pub reason: PairedExecutionReason,
    pub stage_id: Option<PairStageId>,
    pub leg_index: Option<u8>,
    pub state: Option<OrderState>,
    pub policy_decision: Option<PairedPolicyDecision>,
    pub new_handoff: Option<ReconciliationHandoff>,
    pub decision_digest: [u8; 32],
}

impl PairedExecutionDecision {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == decision_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairedExecutionSnapshot {
    pub accepted_commands: u64,
    pub policy_digest: [u8; 32],
    pub order_count: usize,
    pub used_permit_count: usize,
    pub handoff_count: usize,
    pub reserved_cash_micros: i128,
    pub reserved_tokens: Vec<accounting_ledger::ConfirmedTokenBalance>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("paired execution command timestamp is invalid")]
    Timestamp,
    #[error("paired execution command exceeds its canonical bound")]
    CommandBound,
    #[error("paired execution command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported paired execution command version: {0}")]
    Version(u16),
    #[error("paired execution command id was reused for different content")]
    IdempotencyConflict,
    #[error("paired execution clock regressed")]
    ClockRegression,
    #[error("caller attempted to bypass composed lifecycle ownership")]
    Boundary,
    #[error("paper source history regressed or equivocated")]
    SourceHistory,
    #[error("paper exchange order identity changed")]
    ExchangeOrderIdentity,
    #[error("paper lifecycle transition is impossible")]
    Lifecycle,
    #[error("paper fill facts violate exact bounds")]
    FillInvariant,
    #[error("paper identifier is invalid")]
    Identifier,
    #[error("paired reservations changed across an exposure transition")]
    Reservation,
    #[error("settlement handoff identity was reused")]
    Handoff,
    #[error("paired execution arithmetic or counter overflow")]
    Overflow,
    #[error("paired policy child failed: {0}")]
    Policy(String),
    #[error("paired execution is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug, Default)]
pub struct PairedPaperExecution {
    policy: PairedPlacementPolicy,
    orders: BTreeMap<(PairStageId, u8), PairedPaperOrder>,
    used_permits: BTreeSet<[u8; 32]>,
    handoff_digests: BTreeSet<[u8; 32]>,
    ledger_command_ids: BTreeSet<LedgerCommandId>,
    processed: BTreeMap<PairedExecutionCommandId, ([u8; 32], PairedExecutionDecision)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    last_decision: Option<PairedExecutionDecision>,
    halted: Option<String>,
}

impl PairedPaperExecution {
    /// Applies one composed paired paper-execution transition atomically.
    ///
    /// # Errors
    ///
    /// Returns absorbing child, boundary, lifecycle, fill, history, or durable errors.
    pub fn apply(
        &mut self,
        command: &PairedExecutionCommand,
    ) -> Result<PairedExecutionDecision, Error> {
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

    fn apply_fresh(
        &mut self,
        command: &PairedExecutionCommand,
    ) -> Result<PairedExecutionDecision, Error> {
        match command {
            PairedExecutionCommand::Policy {
                command_id,
                command,
                recorded_at_ns,
            } => self.apply_policy(*command_id, command, *recorded_at_ns),
            PairedExecutionCommand::Submit {
                command_id,
                permit,
                local_submission_id,
                recorded_at_ns,
            } => self.submit(*command_id, permit, local_submission_id, *recorded_at_ns),
            PairedExecutionCommand::RequestCancel {
                command_id,
                stage_id,
                leg_index,
                recorded_at_ns,
            } => Ok(self.request_cancel(*command_id, *stage_id, *leg_index, *recorded_at_ns)),
            PairedExecutionCommand::Observe {
                command_id,
                stage_id,
                leg_index,
                observation,
                recorded_at_ns,
            } => self.observe(
                *command_id,
                *stage_id,
                *leg_index,
                observation,
                *recorded_at_ns,
            ),
        }
    }

    fn apply_policy(
        &mut self,
        id: PairedExecutionCommandId,
        command: &PairedPolicyCommand,
        at: i64,
    ) -> Result<PairedExecutionDecision, Error> {
        if command.recorded_at_ns() != at
            || matches!(command, PairedPolicyCommand::ObserveLeg { .. })
        {
            return Err(Error::Boundary);
        }
        let policy_decision = self
            .policy
            .apply(command)
            .map_err(|error| Error::Policy(error.to_string()))?;
        Ok(make_decision(
            id,
            if policy_decision.status == PairedPolicyStatus::Accepted {
                PairedExecutionStatus::Applied
            } else {
                PairedExecutionStatus::Denied
            },
            PairedExecutionReason::PolicyApplied,
            policy_decision.stage_id,
            policy_decision
                .permit
                .as_ref()
                .map(|permit| permit.leg_index),
            None,
            Some(policy_decision),
            None,
        ))
    }

    fn submit(
        &mut self,
        id: PairedExecutionCommandId,
        permit: &PairPermit,
        local_submission_id: &str,
        at: i64,
    ) -> Result<PairedExecutionDecision, Error> {
        let key = (permit.stage_id, permit.leg_index);
        let denial = if !permit.verify_digest() || permit.leg_index > 1 {
            Some(PairedExecutionReason::PermitInvalid)
        } else if self.used_permits.contains(&permit.permit_digest) {
            Some(PairedExecutionReason::PermitAlreadyUsed)
        } else if self
            .policy
            .record(permit.stage_id)
            .and_then(|record| record.permits[usize::from(permit.leg_index)].as_ref())
            != Some(permit)
            || self.policy.record(permit.stage_id).is_none_or(|record| {
                record.legs[usize::from(permit.leg_index)] != LegState::Authorized
            })
            || self
                .policy
                .staging()
                .reservation(permit.reservation_id)
                .is_none_or(|reservation| reservation.status != ReservationStatus::Active)
        {
            Some(PairedExecutionReason::PermitInvalid)
        } else if at < permit.valid_from_ns || at >= permit.valid_until_ns {
            Some(PairedExecutionReason::PermitExpired)
        } else if self.orders.contains_key(&key) {
            Some(PairedExecutionReason::OrderAlreadyExists)
        } else {
            None
        };
        if let Some(reason) = denial {
            return Ok(make_decision(
                id,
                PairedExecutionStatus::Denied,
                reason,
                Some(permit.stage_id),
                Some(permit.leg_index),
                self.orders.get(&key).map(|order| order.state.clone()),
                None,
                None,
            ));
        }
        validate_text(local_submission_id)?;
        let before = self.policy.staging().ledger_risk_view();
        self.used_permits.insert(permit.permit_digest);
        let state = OrderState::Active(ActiveState::Submitted);
        self.orders.insert(
            key,
            PairedPaperOrder {
                permit: permit.clone(),
                local_submission_id: local_submission_id.to_owned(),
                exchange_order_id: None,
                state: state.clone(),
                last_source_sequence: None,
                last_event_time_ns: None,
                last_received_time_ns: None,
                cumulative_quantity_micros: 0,
                cumulative_consideration_micros: 0,
                cumulative_fee_micros: 0,
                fills: BTreeMap::new(),
                handoffs: Vec::new(),
            },
        );
        self.sync_policy(id, permit, LegState::Submitted, 0, at, at)?;
        self.require_reservations_unchanged(&before)?;
        Ok(make_decision(
            id,
            PairedExecutionStatus::Applied,
            PairedExecutionReason::SubmissionAccepted,
            Some(permit.stage_id),
            Some(permit.leg_index),
            Some(state),
            None,
            None,
        ))
    }

    fn request_cancel(
        &mut self,
        id: PairedExecutionCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        at: i64,
    ) -> PairedExecutionDecision {
        let key = (stage_id, leg_index);
        let Some(order) = self.orders.get_mut(&key) else {
            return make_decision(
                id,
                PairedExecutionStatus::Denied,
                PairedExecutionReason::CancelDenied,
                Some(stage_id),
                Some(leg_index),
                None,
                None,
                None,
            );
        };
        let OrderState::Active(resume) = &order.state else {
            return make_decision(
                id,
                PairedExecutionStatus::Denied,
                PairedExecutionReason::CancelDenied,
                Some(stage_id),
                Some(leg_index),
                Some(order.state.clone()),
                None,
                None,
            );
        };
        if matches!(resume, ActiveState::Delayed { uncancellable_until_ns, .. } if at < *uncancellable_until_ns)
        {
            return make_decision(
                id,
                PairedExecutionStatus::Denied,
                PairedExecutionReason::CancelDenied,
                Some(stage_id),
                Some(leg_index),
                Some(order.state.clone()),
                None,
                None,
            );
        }
        order.state = OrderState::CancelPending {
            resume: resume.clone(),
        };
        make_decision(
            id,
            PairedExecutionStatus::Applied,
            PairedExecutionReason::CancelRequested,
            Some(stage_id),
            Some(leg_index),
            Some(order.state.clone()),
            None,
            None,
        )
    }

    fn observe(
        &mut self,
        id: PairedExecutionCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        observation: &ExchangeObservation,
        at: i64,
    ) -> Result<PairedExecutionDecision, Error> {
        let key = (stage_id, leg_index);
        let before = self.policy.staging().ledger_risk_view();
        let (permit, state, handoff, policy_state) = {
            let order = self.orders.get_mut(&key).ok_or(Error::Lifecycle)?;
            if observation.order_id != order.permit.order.order_id {
                return Err(Error::Boundary);
            }
            validate_observation(order, observation, at)?;
            bind_exchange_order_id(order, observation.exchange_order_id.as_deref())?;
            let observation_digest = *blake3::hash(
                &serde_json::to_vec(observation).map_err(|error| Error::Json(error.to_string()))?,
            )
            .as_bytes();
            let handoff = apply_observation(order, observation, observation_digest)?;
            order.last_source_sequence = Some(observation.source_sequence);
            order.last_event_time_ns = Some(observation.event_time_ns);
            order.last_received_time_ns = Some(observation.received_time_ns);
            let policy_state = policy_state_for(order, &observation.event);
            (
                order.permit.clone(),
                order.state.clone(),
                handoff,
                policy_state,
            )
        };
        if let Some(value) = &handoff {
            let digest = handoff_digest(value)?;
            if !self.handoff_digests.insert(digest)
                || !self
                    .ledger_command_ids
                    .insert(value.intent.ledger_command_id)
            {
                return Err(Error::Handoff);
            }
        }
        self.sync_policy(
            id,
            &permit,
            policy_state,
            observation.source_sequence,
            observation.event_time_ns,
            at,
        )?;
        self.require_reservations_unchanged(&before)?;
        Ok(make_decision(
            id,
            PairedExecutionStatus::Applied,
            PairedExecutionReason::ObservationApplied,
            Some(stage_id),
            Some(leg_index),
            Some(state),
            None,
            handoff,
        ))
    }

    fn sync_policy(
        &mut self,
        id: PairedExecutionCommandId,
        permit: &PairPermit,
        state: LegState,
        source_sequence: u64,
        observed_at_ns: i64,
        at: i64,
    ) -> Result<(), Error> {
        let command = PairedPolicyCommand::ObserveLeg {
            command_id: derived_policy_id(id, permit.permit_id, state),
            stage_id: permit.stage_id,
            leg_index: permit.leg_index,
            permit_id: permit.permit_id,
            state,
            source_sequence,
            observed_at_ns,
            recorded_at_ns: at,
        };
        let decision = self
            .policy
            .apply(&command)
            .map_err(|error| Error::Policy(error.to_string()))?;
        if decision.status != PairedPolicyStatus::Accepted {
            return Err(Error::Boundary);
        }
        Ok(())
    }

    fn require_reservations_unchanged(
        &self,
        before: &accounting_ledger::LedgerRiskView,
    ) -> Result<(), Error> {
        if self.policy.staging().ledger_risk_view() == *before {
            Ok(())
        } else {
            Err(Error::Reservation)
        }
    }

    #[must_use]
    pub fn policy(&self) -> &PairedPlacementPolicy {
        &self.policy
    }

    #[must_use]
    pub fn order(&self, stage_id: PairStageId, leg_index: u8) -> Option<&PairedPaperOrder> {
        self.orders.get(&(stage_id, leg_index))
    }

    /// Returns an authoritative view of the ledger nested below paired
    /// staging. This is reserved for the Phase 2.12 composing owner.
    #[doc(hidden)]
    #[must_use]
    pub fn settlement_reconciliation_view(
        &self,
        command_ids: &BTreeSet<LedgerCommandId>,
    ) -> accounting_ledger::LedgerReconciliationView {
        self.policy
            .staging()
            .settlement_reconciliation_view(command_ids)
    }

    /// Applies a Phase 2.12-validated accounting batch to the authoritative
    /// nested ledger. The narrow child API cannot fund or create arbitrary
    /// postings; upper composed owners validate every permitted command.
    ///
    /// # Errors
    ///
    /// Returns a child accounting or boundary failure.
    #[doc(hidden)]
    pub fn settlement_apply_batch(
        &mut self,
        commands: &[accounting_ledger::LedgerCommand],
    ) -> Result<(), Error> {
        self.policy
            .settlement_apply_batch(commands)
            .map_err(|error| Error::Policy(error.to_string()))
    }

    #[doc(hidden)]
    #[must_use]
    pub fn conversion_pair_lock(
        &self,
        id: accounting_ledger::LockId,
    ) -> Option<&accounting_ledger::PairLock> {
        self.policy.staging().conversion_pair_lock(id)
    }

    #[must_use]
    pub fn snapshot(&self) -> PairedExecutionSnapshot {
        let ledger = self.policy.staging().ledger_risk_view();
        PairedExecutionSnapshot {
            accepted_commands: self.accepted_commands,
            policy_digest: self.policy.snapshot().digest,
            order_count: self.orders.len(),
            used_permit_count: self.used_permits.len(),
            handoff_count: self.handoff_digests.len(),
            reserved_cash_micros: ledger.cash_reserved_micros,
            reserved_tokens: ledger.reserved_tokens,
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
        hasher.update(b"paired-paper-execution-state-v1");
        hasher.update(&self.policy.snapshot().digest);
        for ((stage_id, leg), order) in &self.orders {
            hasher.update(&stage_id.0);
            hasher.update(&[*leg]);
            hash_into(&mut hasher, order);
        }
        hash_into(&mut hasher, &self.used_permits);
        hash_into(&mut hasher, &self.handoff_digests);
        hash_into(&mut hasher, &self.ledger_command_ids);
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

fn validate_observation(
    order: &PairedPaperOrder,
    observation: &ExchangeObservation,
    at: i64,
) -> Result<(), Error> {
    if observation.source_sequence == 0
        || observation.event_time_ns < 0
        || observation.received_time_ns < observation.event_time_ns
        || observation.received_time_ns > at
        || order
            .last_source_sequence
            .is_some_and(|value| observation.source_sequence <= value)
        || order
            .last_event_time_ns
            .is_some_and(|value| observation.event_time_ns < value)
        || order
            .last_received_time_ns
            .is_some_and(|value| observation.received_time_ns < value)
    {
        return Err(Error::SourceHistory);
    }
    Ok(())
}

fn bind_exchange_order_id(
    order: &mut PairedPaperOrder,
    observed: Option<&str>,
) -> Result<(), Error> {
    if let Some(value) = observed {
        validate_text(value)?;
    }
    match (&order.exchange_order_id, observed) {
        (Some(existing), Some(value)) if existing != value => Err(Error::ExchangeOrderIdentity),
        (None, Some(value)) => {
            order.exchange_order_id = Some(value.to_owned());
            Ok(())
        }
        _ => Ok(()),
    }
}

fn apply_observation(
    order: &mut PairedPaperOrder,
    observation: &ExchangeObservation,
    observation_digest: [u8; 32],
) -> Result<Option<ReconciliationHandoff>, Error> {
    match &observation.event {
        ExchangeEvent::Delayed {
            release_at_ns,
            uncancellable_until_ns,
        } => {
            require_active(&order.state, |state| {
                matches!(state, ActiveState::Submitted | ActiveState::Unknown { .. })
            })?;
            if *release_at_ns <= observation.event_time_ns
                || *uncancellable_until_ns < *release_at_ns
            {
                return Err(Error::Lifecycle);
            }
            set_active(
                order,
                ActiveState::Delayed {
                    release_at_ns: *release_at_ns,
                    uncancellable_until_ns: *uncancellable_until_ns,
                },
            )?;
            Ok(None)
        }
        ExchangeEvent::Acknowledged => {
            require_exchange_id(order)?;
            require_active(&order.state, |state| {
                matches!(
                    state,
                    ActiveState::Submitted
                        | ActiveState::Delayed { .. }
                        | ActiveState::Unknown { .. }
                )
            })?;
            set_active(order, ActiveState::Acknowledged)?;
            Ok(None)
        }
        ExchangeEvent::Live => {
            require_exchange_id(order)?;
            require_active(&order.state, |state| {
                matches!(
                    state,
                    ActiveState::Submitted
                        | ActiveState::Delayed { .. }
                        | ActiveState::Acknowledged
                        | ActiveState::Unknown { .. }
                )
            })?;
            if active_state(&order.state).is_some_and(|state| matches!(state, ActiveState::Delayed { release_at_ns, .. } if observation.event_time_ns < *release_at_ns)) {
                return Err(Error::Lifecycle);
            }
            set_active(order, ActiveState::Live)?;
            Ok(None)
        }
        ExchangeEvent::Match {
            fill,
            fully_matched,
        } => apply_match(order, observation, fill, *fully_matched, observation_digest).map(Some),
        ExchangeEvent::CancelAccepted => {
            if !matches!(order.state, OrderState::CancelPending { .. }) {
                return Err(Error::Lifecycle);
            }
            order.state = OrderState::Canceled;
            Ok(None)
        }
        ExchangeEvent::CancelRejected => {
            let OrderState::CancelPending { resume } = &order.state else {
                return Err(Error::Lifecycle);
            };
            order.state = OrderState::Active(resume.clone());
            Ok(None)
        }
        ExchangeEvent::Rejected { class, code } => {
            validate_text(code)?;
            if order.state.is_terminal() {
                return Err(Error::Lifecycle);
            }
            order.state = OrderState::Rejected {
                class: *class,
                code: code.clone(),
            };
            Ok(None)
        }
        ExchangeEvent::Unknown { reason } => {
            if order.state.is_terminal() {
                return Err(Error::Lifecycle);
            }
            match &mut order.state {
                OrderState::Active(_) => {
                    order.state = OrderState::Active(ActiveState::Unknown { reason: *reason });
                }
                OrderState::CancelPending { resume } => {
                    *resume = ActiveState::Unknown { reason: *reason }
                }
                _ => return Err(Error::Lifecycle),
            }
            Ok(None)
        }
    }
}

fn apply_match(
    order: &mut PairedPaperOrder,
    observation: &ExchangeObservation,
    fill: &MatchFill,
    fully_matched: bool,
    observation_digest: [u8; 32],
) -> Result<ReconciliationHandoff, Error> {
    require_exchange_id(order)?;
    if order.state.is_terminal()
        || order.fills.contains_key(&fill.fill_id)
        || order
            .fills
            .values()
            .any(|value| value.ledger_command_id == fill.ledger_command_id)
    {
        return Err(Error::FillInvariant);
    }
    if active_state(&order.state).is_some_and(|state| matches!(state, ActiveState::Delayed { release_at_ns, .. } if observation.event_time_ns < *release_at_ns)) {
        return Err(Error::Lifecycle);
    }
    validate_fill(order, fill, fully_matched)?;
    order.cumulative_quantity_micros = fill.cumulative_quantity_micros;
    order.cumulative_consideration_micros = fill.cumulative_consideration_micros;
    order.cumulative_fee_micros = fill.cumulative_fee_micros;
    order.fills.insert(fill.fill_id.clone(), fill.clone());
    let exchange_order_id = order
        .exchange_order_id
        .clone()
        .ok_or(Error::ExchangeOrderIdentity)?;
    let intent = TradeIntent {
        intent_id: intent_id(&fill.fill_id, &exchange_order_id),
        trade_id: fill.fill_id.clone(),
        order_id: exchange_order_id,
        token: order.permit.order.token.clone(),
        side: match order.permit.order.side {
            OrderSide::Buy => Side::Buy,
            OrderSide::Sell => Side::Sell,
        },
        quantity_micros: fill.quantity_micros,
        consideration_micros: fill.consideration_micros,
        fee_micros: fill.fee_micros,
        ledger_command_id: fill.ledger_command_id,
    };
    let handoff = ReconciliationHandoff {
        intent,
        source_sequence: observation.source_sequence,
        placement_policy_digest: order.permit.permit_digest,
        source_observation_digest: observation_digest,
    };
    order.handoffs.push(handoff.clone());
    if fully_matched {
        order.state = OrderState::FullyMatched;
    } else {
        set_active(order, ActiveState::PartiallyMatched)?;
    }
    Ok(handoff)
}

fn validate_fill(order: &PairedPaperOrder, fill: &MatchFill, fully: bool) -> Result<(), Error> {
    validate_text(&fill.fill_id)?;
    if fill.quantity_micros <= 0
        || fill.consideration_micros <= 0
        || fill.fee_micros < 0
        || fill.fee_micros > fill.consideration_micros
    {
        return Err(Error::FillInvariant);
    }
    if order
        .cumulative_quantity_micros
        .checked_add(fill.quantity_micros)
        != Some(fill.cumulative_quantity_micros)
        || order
            .cumulative_consideration_micros
            .checked_add(fill.consideration_micros)
            != Some(fill.cumulative_consideration_micros)
        || order.cumulative_fee_micros.checked_add(fill.fee_micros)
            != Some(fill.cumulative_fee_micros)
        || fill.cumulative_quantity_micros > order.permit.order.quantity_micros
        || fill.cumulative_fee_micros > order.permit.order.max_fee_micros
        || fully != (fill.cumulative_quantity_micros == order.permit.order.quantity_micros)
    {
        return Err(Error::FillInvariant);
    }
    let value = i128::from(order.permit.order.limit_price_micros)
        .checked_mul(fill.quantity_micros)
        .ok_or(Error::Overflow)?;
    match order.permit.order.side {
        OrderSide::Buy if fill.consideration_micros > div_ceil(value, MICROS_PER_UNIT)? => {
            Err(Error::FillInvariant)
        }
        OrderSide::Sell if fill.consideration_micros < value / MICROS_PER_UNIT => {
            Err(Error::FillInvariant)
        }
        _ => Ok(()),
    }
}

fn policy_state_for(order: &PairedPaperOrder, event: &ExchangeEvent) -> LegState {
    match event {
        ExchangeEvent::Delayed { .. } => LegState::Delayed,
        ExchangeEvent::Acknowledged | ExchangeEvent::Live => LegState::Live,
        ExchangeEvent::Match {
            fully_matched: true,
            ..
        } => LegState::FullyMatched,
        ExchangeEvent::Match {
            fully_matched: false,
            ..
        } => LegState::PartiallyMatched,
        ExchangeEvent::Unknown { .. } => LegState::Unknown,
        ExchangeEvent::CancelAccepted | ExchangeEvent::Rejected { .. } => {
            if order.cumulative_quantity_micros == 0 {
                LegState::NoFillTerminal
            } else {
                LegState::PartiallyMatchedTerminal
            }
        }
        ExchangeEvent::CancelRejected => match active_state(&order.state) {
            Some(ActiveState::Delayed { .. }) => LegState::Delayed,
            Some(ActiveState::PartiallyMatched) => LegState::PartiallyMatched,
            Some(ActiveState::Unknown { .. }) => LegState::Unknown,
            _ => LegState::Live,
        },
    }
}

fn active_state(state: &OrderState) -> Option<&ActiveState> {
    match state {
        OrderState::Active(value) | OrderState::CancelPending { resume: value } => Some(value),
        _ => None,
    }
}

fn require_active(
    state: &OrderState,
    allowed: impl FnOnce(&ActiveState) -> bool,
) -> Result<(), Error> {
    match active_state(state) {
        Some(value) if allowed(value) => Ok(()),
        _ => Err(Error::Lifecycle),
    }
}

fn set_active(order: &mut PairedPaperOrder, state: ActiveState) -> Result<(), Error> {
    match &mut order.state {
        OrderState::Active(_) => order.state = OrderState::Active(state),
        OrderState::CancelPending { resume } => *resume = state,
        _ => return Err(Error::Lifecycle),
    }
    Ok(())
}

fn require_exchange_id(order: &PairedPaperOrder) -> Result<(), Error> {
    if order.exchange_order_id.is_some() {
        Ok(())
    } else {
        Err(Error::ExchangeOrderIdentity)
    }
}

fn intent_id(fill_id: &str, order_id: &str) -> IntentId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-paper-execution-handoff-v1");
    hasher.update(&(fill_id.len() as u64).to_le_bytes());
    hasher.update(fill_id.as_bytes());
    hasher.update(&(order_id.len() as u64).to_le_bytes());
    hasher.update(order_id.as_bytes());
    IntentId(*hasher.finalize().as_bytes())
}

fn handoff_digest(value: &ReconciliationHandoff) -> Result<[u8; 32], Error> {
    Ok(
        *blake3::hash(&serde_json::to_vec(value).map_err(|error| Error::Json(error.to_string()))?)
            .as_bytes(),
    )
}

fn derived_policy_id(
    id: PairedExecutionCommandId,
    permit_id: PairPermitId,
    state: LegState,
) -> PairedPolicyCommandId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-execution-policy-sync-v1");
    hasher.update(&id.0);
    hasher.update(&permit_id.0);
    hash_into(&mut hasher, &state);
    PairedPolicyCommandId(*hasher.finalize().as_bytes())
}

#[allow(clippy::too_many_arguments)]
fn make_decision(
    command_id: PairedExecutionCommandId,
    status: PairedExecutionStatus,
    reason: PairedExecutionReason,
    stage_id: Option<PairStageId>,
    leg_index: Option<u8>,
    state: Option<OrderState>,
    policy_decision: Option<PairedPolicyDecision>,
    new_handoff: Option<ReconciliationHandoff>,
) -> PairedExecutionDecision {
    PairedExecutionDecision {
        command_id,
        status,
        reason,
        stage_id,
        leg_index,
        state,
        policy_decision,
        new_handoff,
        decision_digest: [0; 32],
    }
}

fn decision_digest(value: &PairedExecutionDecision) -> [u8; 32] {
    let mut copy = value.clone();
    copy.decision_digest = [0; 32];
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-execution-decision-v1");
    hash_into(&mut hasher, &copy);
    *hasher.finalize().as_bytes()
}

fn div_ceil(value: i128, divisor: i128) -> Result<i128, Error> {
    value
        .checked_add(divisor.checked_sub(1).ok_or(Error::Overflow)?)
        .ok_or(Error::Overflow)
        .map(|adjusted| adjusted / divisor)
}

fn validate_text(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES {
        Err(Error::Identifier)
    } else {
        Ok(())
    }
}

fn hash_into<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("accepted paired execution state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: PairedExecutionCommand,
}

fn validate_command(command: &PairedExecutionCommand) -> Result<(), Error> {
    if command.recorded_at_ns() < 0 {
        return Err(Error::Timestamp);
    }
    match command {
        PairedExecutionCommand::Policy {
            command,
            recorded_at_ns,
            ..
        } if command.recorded_at_ns() != *recorded_at_ns => Err(Error::Timestamp),
        PairedExecutionCommand::Observe {
            observation,
            recorded_at_ns,
            ..
        } if observation.received_time_ns > *recorded_at_ns => Err(Error::Timestamp),
        _ => Ok(()),
    }
}

/// Encodes one bounded, versioned paired execution command.
///
/// # Errors
///
/// Rejects invalid timestamps, oversized commands, and serialization failures.
pub fn encode_command(command: &PairedExecutionCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one exact bounded paired execution command.
///
/// # Errors
///
/// Rejects malformed, trailing, oversized, or unsupported wire data.
pub fn decode_command(bytes: &[u8]) -> Result<PairedExecutionCommand, Error> {
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
