#![forbid(unsafe_code)]

//! Journal-first single-writer orchestration of the complete offline paper path.
//!
//! This runtime has no strategy, credential, signer, network client, wallet
//! action, automatic retry, or live order-submission capability.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurablePaperRuntime,
    RuntimeCheckpoint, RuntimeRecovery, StorageError,
};

use accounting_ledger::{
    AccountingLedger, ApplyOutcome as LedgerOutcome, LedgerCommand, ReservationStatus,
};
use order_intent_policy::{IntentPolicyEngine, PolicyCommand, PolicyCommandId, PolicyDecision};
use paper_execution::{ExchangeEvent, ExecutionCommand, ExecutionDecision, PaperExecutionEngine};
use portfolio_risk::{
    DecisionStatus as RiskStatus, OrderExposure, OrderSide, PortfolioRiskEngine, RiskCommand,
    RiskDecision, RiskOrderId,
};
use serde::{Deserialize, Serialize};
use settlement_reconciliation::{
    ApplyOutcome as ReconciliationOutcome, ReconcilerConfig, ReconciliationCommand,
    ReconciliationCommandId, SettlementReconciler, Side as SettlementSide, TradeIntent,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MICROS_PER_UNIT: i128 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PipelineCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultPoint {
    BeforeRisk,
    BeforeExecution,
    BeforeHandoff,
    IntegrityHalt,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PipelineCommand {
    Accounting {
        command_id: PipelineCommandId,
        command: Box<LedgerCommand>,
        recorded_at_ns: i64,
    },
    Reconciliation {
        command_id: PipelineCommandId,
        command: Box<ReconciliationCommand>,
        recorded_at_ns: i64,
    },
    Risk {
        command_id: PipelineCommandId,
        command: Box<RiskCommand>,
        recorded_at_ns: i64,
    },
    Policy {
        command_id: PipelineCommandId,
        command: Box<PolicyCommand>,
        recorded_at_ns: i64,
    },
    Execution {
        command_id: PipelineCommandId,
        command: Box<ExecutionCommand>,
        recorded_at_ns: i64,
    },
    RegisterHandoff {
        command_id: PipelineCommandId,
        order_id: RiskOrderId,
        handoff_index: usize,
        recorded_at_ns: i64,
    },
    InjectFault {
        command_id: PipelineCommandId,
        fault: FaultPoint,
        recorded_at_ns: i64,
    },
}

impl PipelineCommand {
    #[must_use]
    pub const fn command_id(&self) -> PipelineCommandId {
        match self {
            Self::Accounting { command_id, .. }
            | Self::Reconciliation { command_id, .. }
            | Self::Risk { command_id, .. }
            | Self::Policy { command_id, .. }
            | Self::Execution { command_id, .. }
            | Self::RegisterHandoff { command_id, .. }
            | Self::InjectFault { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Accounting { recorded_at_ns, .. }
            | Self::Reconciliation { recorded_at_ns, .. }
            | Self::Risk { recorded_at_ns, .. }
            | Self::Policy { recorded_at_ns, .. }
            | Self::Execution { recorded_at_ns, .. }
            | Self::RegisterHandoff { recorded_at_ns, .. }
            | Self::InjectFault { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: PipelineCommand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStage {
    Accounting,
    Reconciliation,
    Risk,
    Policy,
    Execution,
    Handoff,
    Fault,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineDetail {
    Applied,
    Duplicate,
    Risk(Box<RiskDecision>),
    Policy(Box<PolicyDecision>),
    Execution(Box<ExecutionDecision>),
    HandoffRegistered,
    FaultArmed(FaultPoint),
    FaultTriggered(FaultPoint),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineOutcome {
    pub command_id: PipelineCommandId,
    pub stage: PipelineStage,
    pub detail: PipelineDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeSnapshot {
    pub accepted_commands: u64,
    pub ledger_digest: [u8; 32],
    pub reconciliation_digest: [u8; 32],
    pub risk_digest: [u8; 32],
    pub policy_digest: [u8; 32],
    pub execution_digest: [u8; 32],
    pub reserved_order_count: usize,
    pub registered_handoff_count: usize,
    pub pending_handoff_count: usize,
    pub armed_fault: Option<FaultPoint>,
    pub last_recorded_at_ns: Option<i64>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error)]
pub enum Error {
    #[error("paper runtime configuration is invalid")]
    Config,
    #[error("pipeline command timestamp or nested timestamp is invalid")]
    Timestamp,
    #[error("pipeline command exceeds its canonical bound")]
    CommandBound,
    #[error("pipeline command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported pipeline command version: {0}")]
    Version(u16),
    #[error("pipeline command id was reused for different content")]
    IdempotencyConflict,
    #[error("pipeline clock regressed")]
    ClockRegression,
    #[error("cross-component provenance or ordering is invalid")]
    Boundary,
    #[error("capital reservation does not exactly back the approved candidate")]
    Reservation,
    #[error("reconciliation handoff is missing, changed, or already registered")]
    Handoff,
    #[error("deterministic integrity fault was injected")]
    InjectedIntegrity,
    #[error("accounting failure: {0}")]
    Accounting(String),
    #[error("reconciliation failure: {0}")]
    Reconciliation(String),
    #[error("risk failure: {0}")]
    Risk(String),
    #[error("policy failure: {0}")]
    Policy(String),
    #[error("execution failure: {0}")]
    Execution(String),
    #[error("pipeline arithmetic overflow")]
    Overflow,
    #[error("paper runtime is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct PaperTradingRuntime {
    config: ReconcilerConfig,
    ledger: AccountingLedger,
    reconciler: SettlementReconciler,
    risk: PortfolioRiskEngine,
    policy: IntentPolicyEngine,
    execution: PaperExecutionEngine,
    approved_candidates: BTreeMap<RiskOrderId, (OrderExposure, [u8; 32])>,
    reserved_orders: BTreeSet<RiskOrderId>,
    registered_handoffs: BTreeSet<[u8; 32]>,
    pending_handoffs: BTreeMap<accounting_ledger::CommandId, (RiskOrderId, TradeIntent)>,
    processed: BTreeMap<PipelineCommandId, ([u8; 32], PipelineOutcome)>,
    armed_fault: Option<FaultPoint>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl PaperTradingRuntime {
    /// Creates one empty single-writer paper pipeline.
    ///
    /// # Errors
    ///
    /// Rejects an invalid reconciliation configuration.
    pub fn new(config: ReconcilerConfig) -> Result<Self, Error> {
        let reconciler = SettlementReconciler::new(config.clone()).map_err(|_| Error::Config)?;
        Ok(Self {
            config,
            ledger: AccountingLedger::default(),
            reconciler,
            risk: PortfolioRiskEngine::default(),
            policy: IntentPolicyEngine::default(),
            execution: PaperExecutionEngine::default(),
            approved_candidates: BTreeMap::new(),
            reserved_orders: BTreeSet::new(),
            registered_handoffs: BTreeSet::new(),
            pending_handoffs: BTreeMap::new(),
            processed: BTreeMap::new(),
            armed_fault: None,
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one transactionally composed pipeline command.
    ///
    /// # Errors
    ///
    /// Returns canonical or absorbing child/cross-component integrity failures.
    pub fn apply(&mut self, command: &PipelineCommand) -> Result<PipelineOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        let bytes = encode_command(command)?;
        let content = *blake3::hash(&bytes).as_bytes();
        let id = command.command_id();
        if let Some((existing, outcome)) = self.processed.get(&id) {
            if *existing == content {
                return Ok(outcome.clone());
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
        let outcome = match candidate.apply_fresh(command) {
            Ok(value) => value,
            Err(error) => return self.install_halt(error),
        };
        candidate.accepted_commands = match candidate.accepted_commands.checked_add(1) {
            Some(value) => value,
            None => return self.install_halt(Error::Overflow),
        };
        candidate.last_recorded_at_ns = Some(command.recorded_at_ns());
        candidate.processed.insert(id, (content, outcome.clone()));
        *self = candidate;
        Ok(outcome)
    }

    #[must_use]
    pub const fn ledger(&self) -> &AccountingLedger {
        &self.ledger
    }

    #[must_use]
    pub const fn reconciler(&self) -> &SettlementReconciler {
        &self.reconciler
    }

    #[must_use]
    pub const fn risk(&self) -> &PortfolioRiskEngine {
        &self.risk
    }

    #[must_use]
    pub const fn policy(&self) -> &IntentPolicyEngine {
        &self.policy
    }

    #[must_use]
    pub const fn execution(&self) -> &PaperExecutionEngine {
        &self.execution
    }

    #[must_use]
    pub fn snapshot(&self) -> RuntimeSnapshot {
        let ledger = self.ledger.snapshot();
        let reconciliation = self.reconciler.snapshot();
        let risk = self.risk.snapshot();
        let policy = self.policy.snapshot();
        let execution = self.execution.snapshot();
        RuntimeSnapshot {
            accepted_commands: self.accepted_commands,
            ledger_digest: ledger.digest,
            reconciliation_digest: reconciliation.digest,
            risk_digest: risk.digest,
            policy_digest: policy.digest,
            execution_digest: execution.digest,
            reserved_order_count: self.reserved_orders.len(),
            registered_handoff_count: self.registered_handoffs.len(),
            pending_handoff_count: self.pending_handoffs.len(),
            armed_fault: self.armed_fault,
            last_recorded_at_ns: self.last_recorded_at_ns,
            halted: self.halted.is_some(),
            halt_reason: self.halted.clone(),
            digest: self.state_digest(),
        }
    }

    #[must_use]
    pub const fn is_halted(&self) -> bool {
        self.halted.is_some()
    }

    fn apply_fresh(&mut self, command: &PipelineCommand) -> Result<PipelineOutcome, Error> {
        match command {
            PipelineCommand::Accounting {
                command_id,
                command,
                ..
            } => self.apply_accounting(*command_id, command),
            PipelineCommand::Reconciliation {
                command_id,
                command,
                ..
            } => self.apply_reconciliation(*command_id, command),
            PipelineCommand::Risk {
                command_id,
                command,
                ..
            } => self.apply_risk(*command_id, command),
            PipelineCommand::Policy {
                command_id,
                command,
                ..
            } => self.apply_policy(*command_id, command),
            PipelineCommand::Execution {
                command_id,
                command,
                ..
            } => self.apply_execution(*command_id, command),
            PipelineCommand::RegisterHandoff {
                command_id,
                order_id,
                handoff_index,
                recorded_at_ns,
            } => self.register_handoff(*command_id, *order_id, *handoff_index, *recorded_at_ns),
            PipelineCommand::InjectFault {
                command_id, fault, ..
            } => {
                if *fault == FaultPoint::IntegrityHalt {
                    return Err(Error::InjectedIntegrity);
                }
                self.armed_fault = Some(*fault);
                Ok(outcome(
                    *command_id,
                    PipelineStage::Fault,
                    PipelineDetail::FaultArmed(*fault),
                ))
            }
        }
    }

    fn apply_accounting(
        &mut self,
        id: PipelineCommandId,
        command: &LedgerCommand,
    ) -> Result<PipelineOutcome, Error> {
        self.validate_accounting_boundary(command)?;
        let result = self
            .ledger
            .apply(command)
            .map_err(|error| Error::Accounting(error.to_string()))?;
        refresh_reservation_tracking(&mut self.reserved_orders, &self.ledger, command);
        if let LedgerCommand::ConfirmBuy { command_id, .. }
        | LedgerCommand::ConfirmSell { command_id, .. } = command
        {
            self.pending_handoffs.remove(command_id);
        }
        Ok(outcome(
            id,
            PipelineStage::Accounting,
            match result {
                LedgerOutcome::Applied => PipelineDetail::Applied,
                LedgerOutcome::Duplicate => PipelineDetail::Duplicate,
            },
        ))
    }

    fn apply_reconciliation(
        &mut self,
        id: PipelineCommandId,
        command: &ReconciliationCommand,
    ) -> Result<PipelineOutcome, Error> {
        match command {
            ReconciliationCommand::RegisterIntent { .. } => return Err(Error::Boundary),
            ReconciliationCommand::Reconcile { frame, .. } => {
                let actual = self.reconciler.capture_frame(
                    &self.ledger,
                    frame.chain.clone(),
                    frame.evaluated_at_ns,
                );
                if actual != *frame {
                    return Err(Error::Boundary);
                }
            }
            ReconciliationCommand::ObserveTrade { .. } => {}
        }
        let result = self
            .reconciler
            .apply(command)
            .map_err(|error| Error::Reconciliation(error.to_string()))?;
        Ok(outcome(
            id,
            PipelineStage::Reconciliation,
            match result {
                ReconciliationOutcome::Applied => PipelineDetail::Applied,
                ReconciliationOutcome::Duplicate => PipelineDetail::Duplicate,
            },
        ))
    }

    fn apply_risk(
        &mut self,
        id: PipelineCommandId,
        command: &RiskCommand,
    ) -> Result<PipelineOutcome, Error> {
        if self.consume_fault(FaultPoint::BeforeRisk) {
            return Ok(outcome(
                id,
                PipelineStage::Risk,
                PipelineDetail::FaultTriggered(FaultPoint::BeforeRisk),
            ));
        }
        let RiskCommand::Evaluate { request, .. } = command;
        if request.reconciliation != self.reconciler.risk_gate()
            || request.ledger != self.ledger.risk_view()
            || !request.additional_candidates.is_empty()
        {
            return Err(Error::Boundary);
        }
        let candidate = request.candidate.clone();
        let decision = self
            .risk
            .apply(command)
            .map_err(|error| Error::Risk(error.to_string()))?;
        if decision.status == RiskStatus::Approve {
            self.approved_candidates
                .insert(candidate.order_id, (candidate, decision.decision_digest));
        }
        Ok(outcome(
            id,
            PipelineStage::Risk,
            PipelineDetail::Risk(Box::new(decision)),
        ))
    }

    fn apply_policy(
        &mut self,
        id: PipelineCommandId,
        command: &PolicyCommand,
    ) -> Result<PipelineOutcome, Error> {
        if let PolicyCommand::AuthorizePlacement { request, .. } = command {
            if self.risk.snapshot().last_decision.as_ref() != Some(&request.approval)
                || !self.reserved_orders.contains(&request.order.order_id)
            {
                return Err(Error::Boundary);
            }
        }
        let decision = self
            .policy
            .apply(command)
            .map_err(|error| Error::Policy(error.to_string()))?;
        Ok(outcome(
            id,
            PipelineStage::Policy,
            PipelineDetail::Policy(Box::new(decision)),
        ))
    }

    fn apply_execution(
        &mut self,
        id: PipelineCommandId,
        command: &ExecutionCommand,
    ) -> Result<PipelineOutcome, Error> {
        if self.consume_fault(FaultPoint::BeforeExecution) {
            return Ok(outcome(
                id,
                PipelineStage::Execution,
                PipelineDetail::FaultTriggered(FaultPoint::BeforeExecution),
            ));
        }
        if let ExecutionCommand::Submit { placement, .. } = command {
            if !self.reserved_orders.contains(&placement.order.order_id) {
                return Err(Error::Reservation);
            }
        }
        match command {
            ExecutionCommand::Submit {
                policy_decision, ..
            }
            | ExecutionCommand::RequestCancel {
                policy_decision, ..
            } => {
                if self.policy.snapshot().last_decision.as_ref() != Some(policy_decision) {
                    return Err(Error::Boundary);
                }
            }
            ExecutionCommand::Observe { .. } => {}
        }
        let decision = self
            .execution
            .apply(command)
            .map_err(|error| Error::Execution(error.to_string()))?;
        self.synchronize_policy(command, &decision)?;
        Ok(outcome(
            id,
            PipelineStage::Execution,
            PipelineDetail::Execution(Box::new(decision)),
        ))
    }

    fn synchronize_policy(
        &mut self,
        command: &ExecutionCommand,
        decision: &ExecutionDecision,
    ) -> Result<(), Error> {
        let ExecutionCommand::Observe {
            observation,
            recorded_at_ns,
            ..
        } = command
        else {
            return Ok(());
        };
        let Some(order_id) = decision.order_id else {
            return Ok(());
        };
        let policy_id = derived_policy_id(command.command_id(), decision.decision_digest);
        let sync = match (&observation.event, decision.state.as_ref()) {
            (
                ExchangeEvent::Delayed {
                    release_at_ns,
                    uncancellable_until_ns,
                },
                _,
            ) => Some(PolicyCommand::MarkDelayed {
                command_id: policy_id,
                order_id,
                release_at_ns: *release_at_ns,
                uncancellable_until_ns: *uncancellable_until_ns,
                recorded_at_ns: *recorded_at_ns,
            }),
            (ExchangeEvent::Live, _) => Some(PolicyCommand::MarkLive {
                command_id: policy_id,
                order_id,
                recorded_at_ns: *recorded_at_ns,
            }),
            (_, Some(state)) if state.is_terminal() => Some(PolicyCommand::MarkTerminal {
                command_id: policy_id,
                order_id,
                recorded_at_ns: *recorded_at_ns,
            }),
            _ => None,
        };
        if let Some(sync) = sync {
            self.policy
                .apply(&sync)
                .map_err(|error| Error::Policy(error.to_string()))?;
        }
        Ok(())
    }

    fn register_handoff(
        &mut self,
        id: PipelineCommandId,
        order_id: RiskOrderId,
        index: usize,
        at: i64,
    ) -> Result<PipelineOutcome, Error> {
        if self.consume_fault(FaultPoint::BeforeHandoff) {
            return Ok(outcome(
                id,
                PipelineStage::Handoff,
                PipelineDetail::FaultTriggered(FaultPoint::BeforeHandoff),
            ));
        }
        let handoff = self
            .execution
            .order(order_id)
            .and_then(|order| order.handoffs.get(index))
            .cloned()
            .ok_or(Error::Handoff)?;
        let digest = *blake3::hash(
            &serde_json::to_vec(&handoff).map_err(|error| Error::Json(error.to_string()))?,
        )
        .as_bytes();
        if !self.registered_handoffs.insert(digest) {
            return Err(Error::Handoff);
        }
        let command = ReconciliationCommand::RegisterIntent {
            command_id: ReconciliationCommandId(digest),
            intent: handoff.intent.clone(),
            recorded_at_ns: at,
        };
        self.reconciler
            .apply(&command)
            .map_err(|error| Error::Reconciliation(error.to_string()))?;
        if self
            .pending_handoffs
            .insert(handoff.intent.ledger_command_id, (order_id, handoff.intent))
            .is_some()
        {
            return Err(Error::Handoff);
        }
        Ok(outcome(
            id,
            PipelineStage::Handoff,
            PipelineDetail::HandoffRegistered,
        ))
    }

    fn validate_accounting_boundary(&self, command: &LedgerCommand) -> Result<(), Error> {
        match command {
            LedgerCommand::ReserveCollateral {
                reservation_id,
                amount_micros,
                ..
            } => {
                let order_id = RiskOrderId(reservation_id.0);
                let Some((candidate, _)) = self.approved_candidates.get(&order_id) else {
                    return Err(Error::Reservation);
                };
                if candidate.side != OrderSide::Buy || full_buy_cost(candidate)? != *amount_micros {
                    return Err(Error::Reservation);
                }
            }
            LedgerCommand::ReserveToken {
                reservation_id,
                token,
                quantity_micros,
                ..
            } => {
                let order_id = RiskOrderId(reservation_id.0);
                let Some((candidate, _)) = self.approved_candidates.get(&order_id) else {
                    return Err(Error::Reservation);
                };
                if candidate.side != OrderSide::Sell
                    || candidate.token != *token
                    || candidate.quantity_micros != *quantity_micros
                {
                    return Err(Error::Reservation);
                }
            }
            LedgerCommand::ConfirmBuy {
                command_id,
                reservation_id,
                token,
                quantity_micros,
                consideration_micros,
                fee_micros,
                ..
            } => {
                let Some((order_id, intent)) = self.pending_handoffs.get(command_id) else {
                    return Err(Error::Handoff);
                };
                if *order_id != RiskOrderId(reservation_id.0)
                    || intent.side != SettlementSide::Buy
                    || intent.token != *token
                    || intent.quantity_micros != *quantity_micros
                    || intent.consideration_micros != *consideration_micros
                    || intent.fee_micros != *fee_micros
                {
                    return Err(Error::Handoff);
                }
            }
            LedgerCommand::ConfirmSell {
                command_id,
                reservation_id,
                quantity_micros,
                gross_proceeds_micros,
                fee_micros,
                ..
            } => {
                let Some((order_id, intent)) = self.pending_handoffs.get(command_id) else {
                    return Err(Error::Handoff);
                };
                if *order_id != RiskOrderId(reservation_id.0)
                    || intent.side != SettlementSide::Sell
                    || intent.quantity_micros != *quantity_micros
                    || intent.consideration_micros != *gross_proceeds_micros
                    || intent.fee_micros != *fee_micros
                {
                    return Err(Error::Handoff);
                }
            }
            LedgerCommand::ReleaseReservation { reservation_id, .. } => {
                let order_id = RiskOrderId(reservation_id.0);
                if self
                    .pending_handoffs
                    .values()
                    .any(|(pending_order, _)| *pending_order == order_id)
                {
                    return Err(Error::Handoff);
                }
                if let Some(order) = self.execution.order(order_id) {
                    if !matches!(
                        order.state,
                        paper_execution::OrderState::Canceled
                            | paper_execution::OrderState::Rejected { .. }
                    ) {
                        return Err(Error::Reservation);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn consume_fault(&mut self, expected: FaultPoint) -> bool {
        if self.armed_fault == Some(expected) {
            self.armed_fault = None;
            true
        } else {
            false
        }
    }

    fn install_halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"paper-trading-runtime-state-v1");
        hash_json(&mut hasher, &self.config.chain_id);
        hash_json(&mut hasher, &self.config.wallet);
        hash_json(&mut hasher, &self.config.confirmation_grace_ns);
        hash_json(&mut hasher, &self.config.max_intents);
        hash_json(&mut hasher, &self.config.max_tokens);
        for digest in [
            self.ledger.snapshot().digest,
            self.reconciler.snapshot().digest,
            self.risk.snapshot().digest,
            self.policy.snapshot().digest,
            self.execution.snapshot().digest,
        ] {
            hasher.update(&digest);
        }
        for (order_id, (candidate, decision_digest)) in &self.approved_candidates {
            hasher.update(&order_id.0);
            hash_json(&mut hasher, candidate);
            hasher.update(decision_digest);
        }
        hash_json(&mut hasher, &self.reserved_orders);
        hash_json(&mut hasher, &self.registered_handoffs);
        for (command_id, (order_id, intent)) in &self.pending_handoffs {
            hasher.update(&command_id.0);
            hasher.update(&order_id.0);
            hash_json(&mut hasher, intent);
        }
        hash_json(&mut hasher, &self.armed_fault);
        hash_json(&mut hasher, &self.accepted_commands);
        hash_json(&mut hasher, &self.last_recorded_at_ns);
        for (id, (content, result)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_json(&mut hasher, result);
        }
        hash_json(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }
}

fn refresh_reservation_tracking(
    values: &mut BTreeSet<RiskOrderId>,
    ledger: &AccountingLedger,
    command: &LedgerCommand,
) {
    let reservation_id = match command {
        LedgerCommand::ReserveCollateral { reservation_id, .. }
        | LedgerCommand::ReserveToken { reservation_id, .. }
        | LedgerCommand::ReleaseReservation { reservation_id, .. }
        | LedgerCommand::ConfirmBuy { reservation_id, .. }
        | LedgerCommand::ConfirmSell { reservation_id, .. } => Some(*reservation_id),
        _ => None,
    };
    let Some(reservation_id) = reservation_id else {
        return;
    };
    let order_id = RiskOrderId(reservation_id.0);
    if ledger
        .reservation(reservation_id)
        .is_some_and(|reservation| reservation.status == ReservationStatus::Active)
    {
        values.insert(order_id);
    } else {
        values.remove(&order_id);
    }
}

fn full_buy_cost(order: &OrderExposure) -> Result<i128, Error> {
    let product = i128::from(order.limit_price_micros)
        .checked_mul(order.quantity_micros)
        .ok_or(Error::Overflow)?;
    product
        .checked_add(MICROS_PER_UNIT - 1)
        .ok_or(Error::Overflow)?
        .checked_div(MICROS_PER_UNIT)
        .and_then(|value| value.checked_add(order.max_fee_micros))
        .ok_or(Error::Overflow)
}

fn derived_policy_id(
    execution_id: paper_execution::ExecutionCommandId,
    decision: [u8; 32],
) -> PolicyCommandId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paper-runtime-policy-sync-v1");
    hasher.update(&execution_id.0);
    hasher.update(&decision);
    PolicyCommandId(*hasher.finalize().as_bytes())
}

fn outcome(id: PipelineCommandId, stage: PipelineStage, detail: PipelineDetail) -> PipelineOutcome {
    let mut value = PipelineOutcome {
        command_id: id,
        stage,
        detail,
        outcome_digest: [0; 32],
    };
    value.outcome_digest = outcome_digest(&value);
    value
}

fn outcome_digest(outcome: &PipelineOutcome) -> [u8; 32] {
    let mut value = outcome.clone();
    value.outcome_digest = [0; 32];
    *blake3::hash(&serde_json::to_vec(&value).expect("pipeline outcome serializes")).as_bytes()
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("pipeline state serializes");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

fn validate_command(command: &PipelineCommand) -> Result<(), Error> {
    if command.recorded_at_ns() < 0 {
        return Err(Error::Timestamp);
    }
    let nested = match command {
        PipelineCommand::Accounting { command, .. } => Some(command.recorded_at_ns()),
        PipelineCommand::Reconciliation { command, .. } => Some(command.recorded_at_ns()),
        PipelineCommand::Risk { command, .. } => Some(command.recorded_at_ns()),
        PipelineCommand::Policy { command, .. } => Some(command.recorded_at_ns()),
        PipelineCommand::Execution { command, .. } => Some(command.recorded_at_ns()),
        PipelineCommand::RegisterHandoff { .. } | PipelineCommand::InjectFault { .. } => None,
    };
    if nested.is_some_and(|value| value != command.recorded_at_ns()) {
        return Err(Error::Timestamp);
    }
    Ok(())
}

/// Encodes one bounded canonical pipeline command.
///
/// # Errors
///
/// Rejects timestamp mismatch and commands beyond the wire bound.
pub fn encode_command(command: &PipelineCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one exact bounded canonical pipeline command.
///
/// # Errors
///
/// Rejects malformed, trailing, oversized, unsupported, or invalid input.
pub fn decode_command(bytes: &[u8]) -> Result<PipelineCommand, Error> {
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
