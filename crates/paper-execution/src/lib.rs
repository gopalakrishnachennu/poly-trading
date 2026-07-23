#![forbid(unsafe_code)]

//! Deterministic simulated/replayed execution and order lifecycle.
//!
//! This crate cannot sign, authenticate, connect to an exchange, or submit a
//! real order. All observations are explicit caller-supplied paper events.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableExecutionEngine,
    ExecutionCheckpoint, ExecutionRecovery, StorageError,
};

use accounting_ledger::CommandId as LedgerCommandId;
use order_intent_policy::{
    cancel_request_digest, placement_request_digest, CancelRequest, PlacementRequest, PolicyAction,
    PolicyDecision, PolicyReason, PolicyStatus,
};
use portfolio_risk::{OrderSide, RiskOrderId};
use serde::{Deserialize, Serialize};
use settlement_reconciliation::{IntentId, Side, TradeIntent};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 256 * 1024;
const MAX_TEXT_BYTES: usize = 512;
const MICROS_PER_UNIT: i128 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ExecutionCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    SubmitTimeout,
    CancelTimeout,
    TransportLost,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryClass {
    Permanent,
    Restart,
    RateLimit,
    BalanceOrAllowance,
    DelayedCheck,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchFill {
    pub fill_id: String,
    pub quantity_micros: i128,
    pub consideration_micros: i128,
    pub fee_micros: i128,
    pub cumulative_quantity_micros: i128,
    pub cumulative_consideration_micros: i128,
    pub cumulative_fee_micros: i128,
    pub ledger_command_id: LedgerCommandId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExchangeEvent {
    Delayed {
        release_at_ns: i64,
        uncancellable_until_ns: i64,
    },
    Acknowledged,
    Live,
    Match {
        fill: MatchFill,
        fully_matched: bool,
    },
    CancelAccepted,
    CancelRejected,
    Rejected {
        class: RetryClass,
        code: String,
    },
    Unknown {
        reason: UnknownReason,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeObservation {
    pub order_id: RiskOrderId,
    pub source_sequence: u64,
    pub exchange_order_id: Option<String>,
    pub event: ExchangeEvent,
    pub event_time_ns: i64,
    pub received_time_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExecutionCommand {
    Submit {
        command_id: ExecutionCommandId,
        policy_decision: PolicyDecision,
        placement: Box<PlacementRequest>,
        local_submission_id: String,
        recorded_at_ns: i64,
    },
    RequestCancel {
        command_id: ExecutionCommandId,
        policy_decision: PolicyDecision,
        request: CancelRequest,
        recorded_at_ns: i64,
    },
    Observe {
        command_id: ExecutionCommandId,
        observation: Box<ExchangeObservation>,
        recorded_at_ns: i64,
    },
}

impl ExecutionCommand {
    #[must_use]
    pub const fn command_id(&self) -> ExecutionCommandId {
        match self {
            Self::Submit { command_id, .. }
            | Self::RequestCancel { command_id, .. }
            | Self::Observe { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Submit { recorded_at_ns, .. }
            | Self::RequestCancel { recorded_at_ns, .. }
            | Self::Observe { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: ExecutionCommand,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveState {
    Submitted,
    Delayed {
        release_at_ns: i64,
        uncancellable_until_ns: i64,
    },
    Acknowledged,
    Live,
    PartiallyMatched,
    Unknown {
        reason: UnknownReason,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderState {
    Active(ActiveState),
    CancelPending { resume: ActiveState },
    FullyMatched,
    Canceled,
    Rejected { class: RetryClass, code: String },
}

impl OrderState {
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::FullyMatched | Self::Canceled | Self::Rejected { .. }
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationHandoff {
    pub intent: TradeIntent,
    pub source_sequence: u64,
    pub placement_policy_digest: [u8; 32],
    pub source_observation_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaperOrder {
    pub placement: PlacementRequest,
    pub local_submission_id: String,
    pub placement_policy_digest: [u8; 32],
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Applied,
    Denied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionReason {
    SubmissionAccepted,
    CancelRequested,
    ObservationApplied,
    PolicyDigestInvalid,
    PolicyNotPermitted,
    PolicySubjectMismatch,
    PolicyOrderMismatch,
    AuthorizationExpired,
    AuthorizationNotYetValid,
    PolicyAlreadyUsed,
    OrderAlreadyExists,
    OrderUnknown,
    OrderTerminal,
    CancelAlreadyPending,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionDecision {
    pub command_id: ExecutionCommandId,
    pub status: ExecutionStatus,
    pub reason: ExecutionReason,
    pub order_id: Option<RiskOrderId>,
    pub state: Option<OrderState>,
    pub new_handoff: Option<ReconciliationHandoff>,
    pub decision_digest: [u8; 32],
}

impl ExecutionDecision {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == decision_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionSnapshot {
    pub accepted_commands: u64,
    pub order_count: usize,
    pub used_placement_permits: usize,
    pub used_cancel_permits: usize,
    pub handoff_count: usize,
    pub last_recorded_at_ns: Option<i64>,
    pub last_decision: Option<ExecutionDecision>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("execution command timestamp is invalid")]
    Timestamp,
    #[error("execution command exceeds its canonical bound")]
    CommandBound,
    #[error("execution command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported execution command version: {0}")]
    Version(u16),
    #[error("execution command id was reused for different content")]
    IdempotencyConflict,
    #[error("execution command clock regressed")]
    ClockRegression,
    #[error("paper source history regressed or equivocated")]
    SourceHistory,
    #[error("paper observation changed immutable exchange order identity")]
    ExchangeOrderIdentity,
    #[error("paper order lifecycle transition is impossible")]
    LifecycleTransition,
    #[error("paper fill facts are invalid or inconsistent")]
    FillInvariant,
    #[error("paper identifier is invalid")]
    Identifier,
    #[error("paper arithmetic overflow")]
    Overflow,
    #[error("paper execution engine is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug, Default)]
pub struct PaperExecutionEngine {
    processed: BTreeMap<ExecutionCommandId, ([u8; 32], ExecutionDecision)>,
    orders: BTreeMap<RiskOrderId, PaperOrder>,
    used_placement_permits: BTreeSet<[u8; 32]>,
    used_cancel_permits: BTreeSet<[u8; 32]>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    last_decision: Option<ExecutionDecision>,
    halted: Option<String>,
}

impl PaperExecutionEngine {
    /// Applies one deterministic paper-execution command.
    ///
    /// # Errors
    ///
    /// Returns canonical validation or absorbing integrity failures.
    pub fn apply(&mut self, command: &ExecutionCommand) -> Result<ExecutionDecision, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        let bytes = encode_command(command)?;
        let content = *blake3::hash(&bytes).as_bytes();
        let id = command.command_id();
        if let Some((existing, decision)) = self.processed.get(&id) {
            if *existing == content {
                return Ok(decision.clone());
            }
            return self.install_halt(Error::IdempotencyConflict);
        }
        if self
            .last_recorded_at_ns
            .is_some_and(|previous| command.recorded_at_ns() < previous)
        {
            return self.install_halt(Error::ClockRegression);
        }
        let mut candidate = self.clone();
        let decision = match candidate.apply_fresh(command) {
            Ok(value) => value,
            Err(error) => return self.install_halt(error),
        };
        candidate.accepted_commands = match candidate.accepted_commands.checked_add(1) {
            Some(value) => value,
            None => return self.install_halt(Error::Overflow),
        };
        candidate.last_recorded_at_ns = Some(command.recorded_at_ns());
        candidate.last_decision = Some(decision.clone());
        candidate.processed.insert(id, (content, decision.clone()));
        *self = candidate;
        Ok(decision)
    }

    #[must_use]
    pub fn order(&self, id: RiskOrderId) -> Option<&PaperOrder> {
        self.orders.get(&id)
    }

    #[must_use]
    pub fn snapshot(&self) -> ExecutionSnapshot {
        ExecutionSnapshot {
            accepted_commands: self.accepted_commands,
            order_count: self.orders.len(),
            used_placement_permits: self.used_placement_permits.len(),
            used_cancel_permits: self.used_cancel_permits.len(),
            handoff_count: self.orders.values().map(|order| order.handoffs.len()).sum(),
            last_recorded_at_ns: self.last_recorded_at_ns,
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

    fn apply_fresh(&mut self, command: &ExecutionCommand) -> Result<ExecutionDecision, Error> {
        match command {
            ExecutionCommand::Submit {
                command_id,
                policy_decision,
                placement,
                local_submission_id,
                recorded_at_ns,
            } => self.submit(
                *command_id,
                policy_decision,
                placement,
                local_submission_id,
                *recorded_at_ns,
            ),
            ExecutionCommand::RequestCancel {
                command_id,
                policy_decision,
                request,
                ..
            } => self.request_cancel(*command_id, policy_decision, request),
            ExecutionCommand::Observe {
                command_id,
                observation,
                ..
            } => self.observe(*command_id, observation),
        }
    }

    fn submit(
        &mut self,
        id: ExecutionCommandId,
        policy: &PolicyDecision,
        placement: &PlacementRequest,
        local_submission_id: &str,
        at: i64,
    ) -> Result<ExecutionDecision, Error> {
        let order_id = placement.order.order_id;
        let denial = if !policy.verify_digest() {
            Some(ExecutionReason::PolicyDigestInvalid)
        } else if policy.status != PolicyStatus::Permit
            || policy.action != PolicyAction::Place
            || policy.reason != PolicyReason::PlacementPermitted
        {
            Some(ExecutionReason::PolicyNotPermitted)
        } else if policy.subject_digest != Some(placement_request_digest(placement)) {
            Some(ExecutionReason::PolicySubjectMismatch)
        } else if policy.order_id != Some(order_id) {
            Some(ExecutionReason::PolicyOrderMismatch)
        } else if at < placement.evaluated_at_ns {
            Some(ExecutionReason::AuthorizationNotYetValid)
        } else if at >= placement.authorization_expires_at_ns {
            Some(ExecutionReason::AuthorizationExpired)
        } else if self
            .used_placement_permits
            .contains(&policy.decision_digest)
        {
            Some(ExecutionReason::PolicyAlreadyUsed)
        } else if self.orders.contains_key(&order_id) {
            Some(ExecutionReason::OrderAlreadyExists)
        } else {
            None
        };
        if let Some(reason) = denial {
            return Ok(make_decision(
                id,
                ExecutionStatus::Denied,
                reason,
                Some(order_id),
                None,
                None,
            ));
        }
        validate_text(local_submission_id)?;
        self.used_placement_permits.insert(policy.decision_digest);
        let state = OrderState::Active(ActiveState::Submitted);
        self.orders.insert(
            order_id,
            PaperOrder {
                placement: placement.clone(),
                local_submission_id: local_submission_id.to_owned(),
                placement_policy_digest: policy.decision_digest,
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
        Ok(make_decision(
            id,
            ExecutionStatus::Applied,
            ExecutionReason::SubmissionAccepted,
            Some(order_id),
            Some(state),
            None,
        ))
    }

    fn request_cancel(
        &mut self,
        id: ExecutionCommandId,
        policy: &PolicyDecision,
        request: &CancelRequest,
    ) -> Result<ExecutionDecision, Error> {
        let denial = if !policy.verify_digest() {
            Some(ExecutionReason::PolicyDigestInvalid)
        } else if policy.status != PolicyStatus::Permit
            || policy.action != PolicyAction::Cancel
            || policy.reason != PolicyReason::CancelPermitted
        {
            Some(ExecutionReason::PolicyNotPermitted)
        } else if policy.subject_digest != Some(cancel_request_digest(request)) {
            Some(ExecutionReason::PolicySubjectMismatch)
        } else if policy.order_id != Some(request.order_id) {
            Some(ExecutionReason::PolicyOrderMismatch)
        } else if self.used_cancel_permits.contains(&policy.decision_digest) {
            Some(ExecutionReason::PolicyAlreadyUsed)
        } else {
            match self.orders.get(&request.order_id) {
                None => Some(ExecutionReason::OrderUnknown),
                Some(order) if order.state.is_terminal() => Some(ExecutionReason::OrderTerminal),
                Some(order) if matches!(order.state, OrderState::CancelPending { .. }) => {
                    Some(ExecutionReason::CancelAlreadyPending)
                }
                Some(_) => None,
            }
        };
        if let Some(reason) = denial {
            return Ok(make_decision(
                id,
                ExecutionStatus::Denied,
                reason,
                Some(request.order_id),
                self.orders
                    .get(&request.order_id)
                    .map(|order| order.state.clone()),
                None,
            ));
        }
        let order = self
            .orders
            .get_mut(&request.order_id)
            .ok_or(Error::LifecycleTransition)?;
        let OrderState::Active(resume) = &order.state else {
            return Err(Error::LifecycleTransition);
        };
        order.state = OrderState::CancelPending {
            resume: resume.clone(),
        };
        self.used_cancel_permits.insert(policy.decision_digest);
        Ok(make_decision(
            id,
            ExecutionStatus::Applied,
            ExecutionReason::CancelRequested,
            Some(request.order_id),
            Some(order.state.clone()),
            None,
        ))
    }

    fn observe(
        &mut self,
        id: ExecutionCommandId,
        observation: &ExchangeObservation,
    ) -> Result<ExecutionDecision, Error> {
        validate_observation(observation)?;
        let order = self
            .orders
            .get_mut(&observation.order_id)
            .ok_or(Error::LifecycleTransition)?;
        validate_source_history(order, observation)?;
        bind_exchange_order_id(order, observation.exchange_order_id.as_deref())?;
        let observation_digest = *blake3::hash(
            &serde_json::to_vec(observation).map_err(|error| Error::Json(error.to_string()))?,
        )
        .as_bytes();
        let handoff = apply_observation(order, observation, observation_digest)?;
        order.last_source_sequence = Some(observation.source_sequence);
        order.last_event_time_ns = Some(observation.event_time_ns);
        order.last_received_time_ns = Some(observation.received_time_ns);
        Ok(make_decision(
            id,
            ExecutionStatus::Applied,
            ExecutionReason::ObservationApplied,
            Some(observation.order_id),
            Some(order.state.clone()),
            handoff,
        ))
    }

    fn install_halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"paper-execution-state-v1");
        for (id, order) in &self.orders {
            hasher.update(&id.0);
            hash_json(&mut hasher, order);
        }
        hash_json(&mut hasher, &self.used_placement_permits);
        hash_json(&mut hasher, &self.used_cancel_permits);
        hash_json(&mut hasher, &self.accepted_commands);
        hash_json(&mut hasher, &self.last_recorded_at_ns);
        hash_json(&mut hasher, &self.last_decision);
        for (id, (content, decision)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_json(&mut hasher, decision);
        }
        hash_json(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }
}

fn validate_source_history(
    order: &PaperOrder,
    observation: &ExchangeObservation,
) -> Result<(), Error> {
    if order
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

fn bind_exchange_order_id(order: &mut PaperOrder, observed: Option<&str>) -> Result<(), Error> {
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
    order: &mut PaperOrder,
    observation: &ExchangeObservation,
    observation_digest: [u8; 32],
) -> Result<Option<ReconciliationHandoff>, Error> {
    match &observation.event {
        ExchangeEvent::Delayed {
            release_at_ns,
            uncancellable_until_ns,
        } => {
            require_active_transition(&order.state, |state| {
                matches!(state, ActiveState::Submitted | ActiveState::Unknown { .. })
            })?;
            if *release_at_ns <= observation.event_time_ns
                || *uncancellable_until_ns < *release_at_ns
            {
                return Err(Error::LifecycleTransition);
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
            require_active_transition(&order.state, |state| {
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
            require_active_transition(&order.state, |state| {
                matches!(
                    state,
                    ActiveState::Submitted
                        | ActiveState::Delayed { .. }
                        | ActiveState::Acknowledged
                        | ActiveState::Unknown { .. }
                )
            })?;
            if active_state(&order.state).is_some_and(|state| matches!(state, ActiveState::Delayed { release_at_ns, .. } if observation.event_time_ns < *release_at_ns)) { return Err(Error::LifecycleTransition); }
            set_active(order, ActiveState::Live)?;
            Ok(None)
        }
        ExchangeEvent::Match {
            fill,
            fully_matched,
        } => apply_match(order, observation, fill, *fully_matched, observation_digest).map(Some),
        ExchangeEvent::CancelAccepted => {
            if !matches!(order.state, OrderState::CancelPending { .. }) {
                return Err(Error::LifecycleTransition);
            }
            order.state = OrderState::Canceled;
            Ok(None)
        }
        ExchangeEvent::CancelRejected => {
            let OrderState::CancelPending { resume } = &order.state else {
                return Err(Error::LifecycleTransition);
            };
            order.state = OrderState::Active(resume.clone());
            Ok(None)
        }
        ExchangeEvent::Rejected { class, code } => {
            validate_text(code)?;
            if order.state.is_terminal() {
                return Err(Error::LifecycleTransition);
            }
            order.state = OrderState::Rejected {
                class: *class,
                code: code.clone(),
            };
            Ok(None)
        }
        ExchangeEvent::Unknown { reason } => {
            if order.state.is_terminal() {
                return Err(Error::LifecycleTransition);
            }
            match &mut order.state {
                OrderState::Active(_) => {
                    order.state = OrderState::Active(ActiveState::Unknown { reason: *reason });
                }
                OrderState::CancelPending { resume } => {
                    *resume = ActiveState::Unknown { reason: *reason }
                }
                _ => return Err(Error::LifecycleTransition),
            }
            Ok(None)
        }
    }
}

fn apply_match(
    order: &mut PaperOrder,
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
    if active_state(&order.state).is_some_and(|state| matches!(state, ActiveState::Delayed { release_at_ns, .. } if observation.event_time_ns < *release_at_ns)) { return Err(Error::LifecycleTransition); }
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
        token: order.placement.order.token.clone(),
        side: match order.placement.order.side {
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
        placement_policy_digest: order.placement_policy_digest,
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

fn validate_fill(order: &PaperOrder, fill: &MatchFill, fully: bool) -> Result<(), Error> {
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
        || fill.cumulative_quantity_micros > order.placement.order.quantity_micros
        || fill.cumulative_fee_micros > order.placement.order.max_fee_micros
        || fully != (fill.cumulative_quantity_micros == order.placement.order.quantity_micros)
    {
        return Err(Error::FillInvariant);
    }
    let limit_value = i128::from(order.placement.order.limit_price_micros)
        .checked_mul(fill.quantity_micros)
        .ok_or(Error::Overflow)?;
    match order.placement.order.side {
        OrderSide::Buy if fill.consideration_micros > div_ceil(limit_value, MICROS_PER_UNIT)? => {
            Err(Error::FillInvariant)
        }
        OrderSide::Sell if fill.consideration_micros < limit_value / MICROS_PER_UNIT => {
            Err(Error::FillInvariant)
        }
        _ => Ok(()),
    }
}

fn active_state(state: &OrderState) -> Option<&ActiveState> {
    match state {
        OrderState::Active(value) | OrderState::CancelPending { resume: value } => Some(value),
        _ => None,
    }
}

fn require_active_transition(
    state: &OrderState,
    allowed: impl FnOnce(&ActiveState) -> bool,
) -> Result<(), Error> {
    match active_state(state) {
        Some(value) if allowed(value) => Ok(()),
        _ => Err(Error::LifecycleTransition),
    }
}

fn set_active(order: &mut PaperOrder, state: ActiveState) -> Result<(), Error> {
    match &mut order.state {
        OrderState::Active(_) => order.state = OrderState::Active(state),
        OrderState::CancelPending { resume } => *resume = state,
        _ => return Err(Error::LifecycleTransition),
    }
    Ok(())
}

fn require_exchange_id(order: &PaperOrder) -> Result<(), Error> {
    if order.exchange_order_id.is_none() {
        Err(Error::ExchangeOrderIdentity)
    } else {
        Ok(())
    }
}

fn intent_id(fill_id: &str, order_id: &str) -> IntentId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paper-execution-handoff-v1");
    hasher.update(&(fill_id.len() as u64).to_le_bytes());
    hasher.update(fill_id.as_bytes());
    hasher.update(&(order_id.len() as u64).to_le_bytes());
    hasher.update(order_id.as_bytes());
    IntentId(*hasher.finalize().as_bytes())
}

fn make_decision(
    id: ExecutionCommandId,
    status: ExecutionStatus,
    reason: ExecutionReason,
    order_id: Option<RiskOrderId>,
    state: Option<OrderState>,
    handoff: Option<ReconciliationHandoff>,
) -> ExecutionDecision {
    let mut value = ExecutionDecision {
        command_id: id,
        status,
        reason,
        order_id,
        state,
        new_handoff: handoff,
        decision_digest: [0; 32],
    };
    value.decision_digest = decision_digest(&value);
    value
}

fn decision_digest(decision: &ExecutionDecision) -> [u8; 32] {
    let mut value = decision.clone();
    value.decision_digest = [0; 32];
    *blake3::hash(&serde_json::to_vec(&value).expect("execution decision serializes")).as_bytes()
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

fn validate_observation(value: &ExchangeObservation) -> Result<(), Error> {
    if value.event_time_ns < 0 || value.received_time_ns < value.event_time_ns {
        return Err(Error::Timestamp);
    }
    if let Some(id) = &value.exchange_order_id {
        validate_text(id)?;
    }
    Ok(())
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("internal execution state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

fn validate_command(command: &ExecutionCommand) -> Result<(), Error> {
    if command.recorded_at_ns() < 0 {
        return Err(Error::Timestamp);
    }
    match command {
        ExecutionCommand::Submit { recorded_at_ns, .. } if *recorded_at_ns < 0 => {
            Err(Error::Timestamp)
        }
        ExecutionCommand::RequestCancel {
            request,
            recorded_at_ns,
            ..
        } if request.evaluated_at_ns != *recorded_at_ns => Err(Error::Timestamp),
        ExecutionCommand::Observe {
            observation,
            recorded_at_ns,
            ..
        } if observation.received_time_ns > *recorded_at_ns => Err(Error::Timestamp),
        _ => Ok(()),
    }
}

/// Encodes one bounded canonical paper-execution command.
///
/// # Errors
///
/// Rejects invalid timestamps and commands beyond the wire bound.
pub fn encode_command(command: &ExecutionCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one exact bounded paper-execution command.
///
/// # Errors
///
/// Rejects malformed, trailing, oversized, unsupported, or invalid input.
pub fn decode_command(bytes: &[u8]) -> Result<ExecutionCommand, Error> {
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
