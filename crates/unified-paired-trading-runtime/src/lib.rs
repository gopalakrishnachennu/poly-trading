#![forbid(unsafe_code)]

//! Unified deterministic owner for the complete offline paired-trading path.
//!
//! This crate has no credential, signer, authenticated transport, RPC, wallet,
//! relayer, live order, live transaction, or automatic retry capability.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableUnifiedRuntime,
    StorageError, UnifiedCheckpoint, UnifiedRecovery,
};

use accounting_ledger::{CommandId as LedgerCommandId, LockId};
use complete_set_arbitrage::{ArbitrageCommand, ArbitrageCommandId, ArbitrageRequest};
use ctf_transaction_runtime::{
    ConversionId, ConversionObservation, ConversionRequest, CtfCommand, CtfCommandId, CtfDetail,
    CtfOutcome, CtfTransactionRuntime,
};
use order_intent_policy::ExchangeModeObservation;
use paired_capital_staging::PairStageId;
use paired_opportunity_runtime::{PairRiskFrame, PairedCommand, PairedCommandId};
use paired_paper_execution::{
    PairedExecutionCommand, PairedExecutionCommandId, PairedExecutionStatus,
};
use paired_placement_policy::{
    PairedPolicyCommand, PairedPolicyCommandId, PairedPolicyDecision, PairedPolicyStatus,
};
use paired_settlement_runtime::{
    PairedSettlementCommand, PairedSettlementCommandId, PairedSettlementDetail,
};
use paper_execution::{ExchangeEvent, ExchangeObservation, MatchFill, RetryClass, UnknownReason};
use portfolio_risk::{BinaryMarketRisk, OrderExposure, RiskLimits, ShockProfile};
use serde::{Deserialize, Serialize};
use settlement_reconciliation::{FinalizedChainSnapshot, ReconcilerConfig, TradeObservation};
use std::collections::BTreeMap;
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct UnifiedCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationRiskInputs {
    pub markets: Vec<BinaryMarketRisk>,
    pub open_orders: Vec<OrderExposure>,
    pub shocks: Vec<ShockProfile>,
    pub limits: RiskLimits,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum UnifiedOrderEvent {
    Delayed {
        release_at_ns: i64,
        uncancellable_until_ns: i64,
    },
    Acknowledged,
    Live,
    Match {
        fill_id: String,
        quantity_micros: i128,
        consideration_micros: i128,
        fee_micros: i128,
        cumulative_quantity_micros: i128,
        cumulative_consideration_micros: i128,
        cumulative_fee_micros: i128,
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
pub struct UnifiedOrderObservation {
    pub source_sequence: u64,
    pub exchange_order_id: Option<String>,
    pub event: UnifiedOrderEvent,
    pub event_time_ns: i64,
    pub received_time_ns: i64,
}

/// Deterministic fault points. A fault never installs a partial child state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnifiedFault {
    AfterAuthorizationBeforeSubmission,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum UnifiedCommand {
    Fund {
        command_id: UnifiedCommandId,
        amount_micros: i128,
        recorded_at_ns: i64,
    },
    Reconcile {
        command_id: UnifiedCommandId,
        chain: FinalizedChainSnapshot,
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    },
    EvaluateAndStage {
        command_id: UnifiedCommandId,
        request: Box<ArbitrageRequest>,
        risk: Box<EvaluationRiskInputs>,
        recorded_at_ns: i64,
    },
    ObserveMode {
        command_id: UnifiedCommandId,
        observation: ExchangeModeObservation,
        recorded_at_ns: i64,
    },
    AuthorizeAndSubmitFirst {
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        max_mode_age_ns: i64,
        valid_until_ns: i64,
        local_submission_id: String,
        recorded_at_ns: i64,
    },
    AuthorizeAndSubmitHedge {
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        max_mode_age_ns: i64,
        valid_until_ns: i64,
        local_submission_id: String,
        recorded_at_ns: i64,
    },
    RequestCancel {
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        recorded_at_ns: i64,
    },
    ObserveOrder {
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        observation: Box<UnifiedOrderObservation>,
        recorded_at_ns: i64,
    },
    ExpirePermissions {
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        recorded_at_ns: i64,
    },
    AbortUnfilled {
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        recorded_at_ns: i64,
    },
    RegisterHandoff {
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        handoff_index: usize,
        recorded_at_ns: i64,
    },
    ObserveTrade {
        command_id: UnifiedCommandId,
        observation: TradeObservation,
        recorded_at_ns: i64,
    },
    PostConfirmed {
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        handoff_index: usize,
        recorded_at_ns: i64,
    },
    LockCompletePair {
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        lock_id: LockId,
        quantity_micros: i128,
        recorded_at_ns: i64,
    },
    FinalizeStage {
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        recorded_at_ns: i64,
    },
    RequestConversion {
        command_id: UnifiedCommandId,
        conversion_id: ConversionId,
        request: ConversionRequest,
        recorded_at_ns: i64,
    },
    ObserveConversion {
        command_id: UnifiedCommandId,
        observation: ConversionObservation,
        recorded_at_ns: i64,
    },
    InjectFault {
        command_id: UnifiedCommandId,
        fault: UnifiedFault,
        recorded_at_ns: i64,
    },
}

impl UnifiedCommand {
    #[must_use]
    pub const fn command_id(&self) -> UnifiedCommandId {
        match self {
            Self::Fund { command_id, .. }
            | Self::Reconcile { command_id, .. }
            | Self::EvaluateAndStage { command_id, .. }
            | Self::ObserveMode { command_id, .. }
            | Self::AuthorizeAndSubmitFirst { command_id, .. }
            | Self::AuthorizeAndSubmitHedge { command_id, .. }
            | Self::RequestCancel { command_id, .. }
            | Self::ObserveOrder { command_id, .. }
            | Self::ExpirePermissions { command_id, .. }
            | Self::AbortUnfilled { command_id, .. }
            | Self::RegisterHandoff { command_id, .. }
            | Self::ObserveTrade { command_id, .. }
            | Self::PostConfirmed { command_id, .. }
            | Self::LockCompletePair { command_id, .. }
            | Self::FinalizeStage { command_id, .. }
            | Self::RequestConversion { command_id, .. }
            | Self::ObserveConversion { command_id, .. }
            | Self::InjectFault { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Fund { recorded_at_ns, .. }
            | Self::Reconcile { recorded_at_ns, .. }
            | Self::EvaluateAndStage { recorded_at_ns, .. }
            | Self::ObserveMode { recorded_at_ns, .. }
            | Self::AuthorizeAndSubmitFirst { recorded_at_ns, .. }
            | Self::AuthorizeAndSubmitHedge { recorded_at_ns, .. }
            | Self::RequestCancel { recorded_at_ns, .. }
            | Self::ObserveOrder { recorded_at_ns, .. }
            | Self::ExpirePermissions { recorded_at_ns, .. }
            | Self::AbortUnfilled { recorded_at_ns, .. }
            | Self::RegisterHandoff { recorded_at_ns, .. }
            | Self::ObserveTrade { recorded_at_ns, .. }
            | Self::PostConfirmed { recorded_at_ns, .. }
            | Self::LockCompletePair { recorded_at_ns, .. }
            | Self::FinalizeStage { recorded_at_ns, .. }
            | Self::RequestConversion { recorded_at_ns, .. }
            | Self::ObserveConversion { recorded_at_ns, .. }
            | Self::InjectFault { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum UnifiedDetail {
    Funded,
    Reconciled {
        current: bool,
    },
    Staged {
        stage_id: PairStageId,
    },
    NoTrade,
    ModeObserved,
    Submitted {
        stage_id: PairStageId,
        leg_index: u8,
    },
    AuthorizationDenied {
        stage_id: PairStageId,
    },
    CancelProcessed {
        stage_id: PairStageId,
        leg_index: u8,
    },
    OrderObserved {
        handoff_created: bool,
    },
    PermissionsExpired,
    StageAborted,
    StageAbortDenied,
    HandoffRegistered {
        ledger_command_id: LedgerCommandId,
    },
    TradeObserved,
    ConfirmedPosted {
        ledger_command_id: LedgerCommandId,
    },
    PairLocked,
    StageFinalized,
    Conversion(CtfDetail),
    FaultArmed(UnifiedFault),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnifiedOutcome {
    pub command_id: UnifiedCommandId,
    pub detail: UnifiedDetail,
    pub child_outcomes: Vec<CtfOutcome>,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnifiedSnapshot {
    pub accepted_commands: u64,
    pub ctf_digest: [u8; 32],
    pub cash_available_micros: i128,
    pub cash_reserved_micros: i128,
    pub pending_conversion_count: usize,
    pub confirmed_conversion_count: usize,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("unified runtime configuration is invalid")]
    Config,
    #[error("unified command timestamp is invalid or regressed")]
    Timestamp,
    #[error("unified command exceeds its canonical bound")]
    CommandBound,
    #[error("unified command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported unified command version: {0}")]
    Version(u16),
    #[error("unified command id was reused for different content")]
    IdempotencyConflict,
    #[error("unified boundary could not derive an authentic child subject")]
    Boundary,
    #[error("unified identifier is invalid")]
    Identifier,
    #[error("deterministic unified fault injected")]
    InjectedFault,
    #[error("CTF child failed: {0}")]
    Child(String),
    #[error("unified arithmetic or counter overflow")]
    Overflow,
    #[error("unified runtime is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct UnifiedPairedTradingRuntime {
    config: ReconcilerConfig,
    ctf: CtfTransactionRuntime,
    processed: BTreeMap<UnifiedCommandId, ([u8; 32], UnifiedOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    armed_fault: Option<UnifiedFault>,
    halted: Option<String>,
}

impl UnifiedPairedTradingRuntime {
    /// Creates an empty offline unified owner.
    ///
    /// # Errors
    ///
    /// Rejects invalid reconciliation configuration.
    pub fn new(config: ReconcilerConfig) -> Result<Self, Error> {
        let ctf = CtfTransactionRuntime::new(config.clone()).map_err(|_| Error::Config)?;
        Ok(Self {
            config,
            ctf,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            armed_fault: None,
            halted: None,
        })
    }

    /// Applies one top-level command transactionally.
    ///
    /// # Errors
    ///
    /// Returns absorbing identity, boundary, child, durability, or arithmetic failures.
    pub fn apply(&mut self, command: &UnifiedCommand) -> Result<UnifiedOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        if command.recorded_at_ns() < 0 {
            return self.halt(Error::Timestamp);
        }
        let encoded = encode_command(command)?;
        let content = *blake3::hash(&encoded).as_bytes();
        let id = command.command_id();
        if let Some((existing, outcome)) = self.processed.get(&id) {
            if *existing == content {
                return Ok(outcome.clone());
            }
            return self.halt(Error::IdempotencyConflict);
        }
        if self
            .last_recorded_at_ns
            .is_some_and(|previous| command.recorded_at_ns() < previous)
        {
            return self.halt(Error::Timestamp);
        }
        let mut next = self.clone();
        let (detail, child_outcomes) = match next.apply_fresh(command) {
            Ok(value) => value,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = next
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        let mut outcome = UnifiedOutcome {
            command_id: id,
            detail,
            child_outcomes,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = outcome_digest(&outcome);
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.processed.insert(id, (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn apply_fresh(
        &mut self,
        command: &UnifiedCommand,
    ) -> Result<(UnifiedDetail, Vec<CtfOutcome>), Error> {
        let id = command.command_id();
        let at = command.recorded_at_ns();
        match command {
            UnifiedCommand::Fund { amount_micros, .. } => {
                let outcome = self.apply_policy(
                    id,
                    0,
                    PairedPolicyCommand::Fund {
                        command_id: policy_id(id, 0),
                        amount_micros: *amount_micros,
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                Ok((UnifiedDetail::Funded, vec![outcome]))
            }
            UnifiedCommand::Reconcile {
                chain,
                evaluated_at_ns,
                ..
            } => {
                let outcome = self.apply_settlement(
                    id,
                    0,
                    PairedSettlementCommand::Reconcile {
                        command_id: settlement_id(id, 0),
                        chain: chain.clone(),
                        evaluated_at_ns: *evaluated_at_ns,
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                Ok((
                    UnifiedDetail::Reconciled {
                        current: self.ctf.parent().reconciliation_is_current(),
                    },
                    vec![outcome],
                ))
            }
            UnifiedCommand::EvaluateAndStage { request, risk, .. } => {
                let settlement = self.ctf.parent();
                let frame = PairRiskFrame {
                    reconciliation: settlement.reconciler().risk_gate(),
                    ledger: settlement.execution().policy().staging().ledger_risk_view(),
                    markets: risk.markets.clone(),
                    open_orders: risk.open_orders.clone(),
                    shocks: risk.shocks.clone(),
                    limits: risk.limits.clone(),
                    evaluated_at_ns: at,
                };
                let paired = PairedCommand::Evaluate {
                    command_id: paired_id(id, 0),
                    arbitrage_command: Box::new(ArbitrageCommand::Evaluate {
                        command_id: arbitrage_id(id, 0),
                        request: request.as_ref().clone(),
                        recorded_at_ns: at,
                    }),
                    risk_frame: Box::new(frame),
                    recorded_at_ns: at,
                };
                let outcome = self.apply_policy(
                    id,
                    0,
                    PairedPolicyCommand::Stage {
                        command_id: policy_id(id, 0),
                        paired_command: Box::new(paired),
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                let decision = policy_decision(&outcome)?;
                let detail = if decision.status == PairedPolicyStatus::Accepted {
                    UnifiedDetail::Staged {
                        stage_id: decision.stage_id.ok_or(Error::Boundary)?,
                    }
                } else {
                    UnifiedDetail::NoTrade
                };
                Ok((detail, vec![outcome]))
            }
            UnifiedCommand::ObserveMode { observation, .. } => {
                let outcome = self.apply_policy(
                    id,
                    0,
                    PairedPolicyCommand::ObserveMode {
                        command_id: policy_id(id, 0),
                        observation: observation.clone(),
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                Ok((UnifiedDetail::ModeObserved, vec![outcome]))
            }
            UnifiedCommand::AuthorizeAndSubmitFirst {
                stage_id,
                leg_index,
                max_mode_age_ns,
                valid_until_ns,
                local_submission_id,
                ..
            } => self.authorize_and_submit(
                id,
                *stage_id,
                Some(*leg_index),
                *max_mode_age_ns,
                *valid_until_ns,
                local_submission_id,
                at,
            ),
            UnifiedCommand::AuthorizeAndSubmitHedge {
                stage_id,
                max_mode_age_ns,
                valid_until_ns,
                local_submission_id,
                ..
            } => self.authorize_and_submit(
                id,
                *stage_id,
                None,
                *max_mode_age_ns,
                *valid_until_ns,
                local_submission_id,
                at,
            ),
            UnifiedCommand::RequestCancel {
                stage_id,
                leg_index,
                ..
            } => {
                let outcome = self.apply_execution(
                    id,
                    0,
                    PairedExecutionCommand::RequestCancel {
                        command_id: execution_id(id, 0),
                        stage_id: *stage_id,
                        leg_index: *leg_index,
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                Ok((
                    UnifiedDetail::CancelProcessed {
                        stage_id: *stage_id,
                        leg_index: *leg_index,
                    },
                    vec![outcome],
                ))
            }
            UnifiedCommand::ObserveOrder {
                stage_id,
                leg_index,
                observation,
                ..
            } => {
                let exchange_observation =
                    self.derive_exchange_observation(id, *stage_id, *leg_index, observation)?;
                let outcome = self.apply_execution(
                    id,
                    0,
                    PairedExecutionCommand::Observe {
                        command_id: execution_id(id, 0),
                        stage_id: *stage_id,
                        leg_index: *leg_index,
                        observation: Box::new(exchange_observation),
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                let created = execution_decision(&outcome)?.new_handoff.is_some();
                Ok((
                    UnifiedDetail::OrderObserved {
                        handoff_created: created,
                    },
                    vec![outcome],
                ))
            }
            UnifiedCommand::ExpirePermissions { stage_id, .. } => {
                let outcome = self.apply_policy(
                    id,
                    0,
                    PairedPolicyCommand::Expire {
                        command_id: policy_id(id, 0),
                        stage_id: *stage_id,
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                Ok((UnifiedDetail::PermissionsExpired, vec![outcome]))
            }
            UnifiedCommand::AbortUnfilled { stage_id, .. } => {
                let outcome = self.apply_policy(
                    id,
                    0,
                    PairedPolicyCommand::AbortSafe {
                        command_id: policy_id(id, 0),
                        stage_id: *stage_id,
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                let detail = if policy_decision(&outcome)?.status == PairedPolicyStatus::Accepted {
                    UnifiedDetail::StageAborted
                } else {
                    UnifiedDetail::StageAbortDenied
                };
                Ok((detail, vec![outcome]))
            }
            UnifiedCommand::RegisterHandoff {
                stage_id,
                leg_index,
                handoff_index,
                ..
            } => {
                let ledger_id = self.handoff_ledger_id(*stage_id, *leg_index, *handoff_index)?;
                let outcome = self.apply_settlement(
                    id,
                    0,
                    PairedSettlementCommand::RegisterHandoff {
                        command_id: settlement_id(id, 0),
                        stage_id: *stage_id,
                        leg_index: *leg_index,
                        handoff_index: *handoff_index,
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                Ok((
                    UnifiedDetail::HandoffRegistered {
                        ledger_command_id: ledger_id,
                    },
                    vec![outcome],
                ))
            }
            UnifiedCommand::ObserveTrade { observation, .. } => {
                let outcome = self.apply_settlement(
                    id,
                    0,
                    PairedSettlementCommand::ObserveTrade {
                        command_id: settlement_id(id, 0),
                        observation: observation.clone(),
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                Ok((UnifiedDetail::TradeObserved, vec![outcome]))
            }
            UnifiedCommand::PostConfirmed {
                stage_id,
                leg_index,
                handoff_index,
                ..
            } => {
                let ledger_id = self.handoff_ledger_id(*stage_id, *leg_index, *handoff_index)?;
                let outcome = self.apply_settlement(
                    id,
                    0,
                    PairedSettlementCommand::PostConfirmed {
                        command_id: settlement_id(id, 0),
                        ledger_command_id: ledger_id,
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                Ok((
                    UnifiedDetail::ConfirmedPosted {
                        ledger_command_id: ledger_id,
                    },
                    vec![outcome],
                ))
            }
            UnifiedCommand::LockCompletePair {
                stage_id,
                lock_id,
                quantity_micros,
                ..
            } => {
                let outcome = self.apply_settlement(
                    id,
                    0,
                    PairedSettlementCommand::LockCompletePair {
                        command_id: settlement_id(id, 0),
                        stage_id: *stage_id,
                        lock_id: *lock_id,
                        quantity_micros: *quantity_micros,
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                Ok((UnifiedDetail::PairLocked, vec![outcome]))
            }
            UnifiedCommand::FinalizeStage { stage_id, .. } => {
                let outcome = self.apply_settlement(
                    id,
                    0,
                    PairedSettlementCommand::FinalizeStage {
                        command_id: settlement_id(id, 0),
                        stage_id: *stage_id,
                        recorded_at_ns: at,
                    },
                    at,
                )?;
                Ok((UnifiedDetail::StageFinalized, vec![outcome]))
            }
            UnifiedCommand::RequestConversion {
                conversion_id,
                request,
                ..
            } => {
                let outcome = self.apply_ctf(&CtfCommand::Request {
                    command_id: ctf_id(id, 0),
                    conversion_id: *conversion_id,
                    request: request.clone(),
                    recorded_at_ns: at,
                })?;
                Ok((
                    UnifiedDetail::Conversion(outcome.detail.clone()),
                    vec![outcome],
                ))
            }
            UnifiedCommand::ObserveConversion { observation, .. } => {
                let outcome = self.apply_ctf(&CtfCommand::Observe {
                    command_id: ctf_id(id, 0),
                    observation: observation.clone(),
                    recorded_at_ns: at,
                })?;
                Ok((
                    UnifiedDetail::Conversion(outcome.detail.clone()),
                    vec![outcome],
                ))
            }
            UnifiedCommand::InjectFault { fault, .. } => {
                if self.armed_fault.replace(*fault).is_some() {
                    return Err(Error::Boundary);
                }
                Ok((UnifiedDetail::FaultArmed(*fault), Vec::new()))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn authorize_and_submit(
        &mut self,
        id: UnifiedCommandId,
        stage_id: PairStageId,
        first_leg: Option<u8>,
        max_mode_age_ns: i64,
        valid_until_ns: i64,
        local_submission_id: &str,
        at: i64,
    ) -> Result<(UnifiedDetail, Vec<CtfOutcome>), Error> {
        validate_text(local_submission_id)?;
        let policy_command = if let Some(leg_index) = first_leg {
            PairedPolicyCommand::AuthorizeFirst {
                command_id: policy_id(id, 0),
                stage_id,
                leg_index,
                max_mode_age_ns,
                valid_until_ns,
                recorded_at_ns: at,
            }
        } else {
            PairedPolicyCommand::AuthorizeHedge {
                command_id: policy_id(id, 0),
                stage_id,
                max_mode_age_ns,
                valid_until_ns,
                recorded_at_ns: at,
            }
        };
        let authorization = self.apply_policy(id, 0, policy_command, at)?;
        let policy = policy_decision(&authorization)?;
        if policy.status != PairedPolicyStatus::Accepted {
            return Ok((
                UnifiedDetail::AuthorizationDenied { stage_id },
                vec![authorization],
            ));
        }
        let permit = policy.permit.clone().ok_or(Error::Boundary)?;
        if self.armed_fault == Some(UnifiedFault::AfterAuthorizationBeforeSubmission) {
            return Err(Error::InjectedFault);
        }
        let submission = self.apply_execution(
            id,
            1,
            PairedExecutionCommand::Submit {
                command_id: execution_id(id, 1),
                permit: Box::new(permit.clone()),
                local_submission_id: local_submission_id.to_owned(),
                recorded_at_ns: at,
            },
            at,
        )?;
        if execution_decision(&submission)?.status != PairedExecutionStatus::Applied {
            return Err(Error::Boundary);
        }
        Ok((
            UnifiedDetail::Submitted {
                stage_id,
                leg_index: permit.leg_index,
            },
            vec![authorization, submission],
        ))
    }

    fn apply_policy(
        &mut self,
        id: UnifiedCommandId,
        step: u8,
        command: PairedPolicyCommand,
        at: i64,
    ) -> Result<CtfOutcome, Error> {
        self.apply_execution(
            id,
            step,
            PairedExecutionCommand::Policy {
                command_id: execution_id(id, step),
                command: Box::new(command),
                recorded_at_ns: at,
            },
            at,
        )
    }

    fn apply_execution(
        &mut self,
        id: UnifiedCommandId,
        step: u8,
        command: PairedExecutionCommand,
        at: i64,
    ) -> Result<CtfOutcome, Error> {
        self.apply_settlement(
            id,
            step,
            PairedSettlementCommand::Execution {
                command_id: settlement_id(id, step),
                command: Box::new(command),
                recorded_at_ns: at,
            },
            at,
        )
    }

    fn apply_settlement(
        &mut self,
        id: UnifiedCommandId,
        step: u8,
        command: PairedSettlementCommand,
        at: i64,
    ) -> Result<CtfOutcome, Error> {
        self.apply_ctf(&CtfCommand::Parent {
            command_id: ctf_id(id, step),
            command: Box::new(command),
            recorded_at_ns: at,
        })
    }

    fn apply_ctf(&mut self, command: &CtfCommand) -> Result<CtfOutcome, Error> {
        self.ctf
            .apply(command)
            .map_err(|error| Error::Child(error.to_string()))
    }

    fn handoff_ledger_id(
        &self,
        stage_id: PairStageId,
        leg_index: u8,
        handoff_index: usize,
    ) -> Result<LedgerCommandId, Error> {
        self.ctf
            .parent()
            .execution()
            .order(stage_id, leg_index)
            .and_then(|order| order.handoffs.get(handoff_index))
            .map(|handoff| handoff.intent.ledger_command_id)
            .ok_or(Error::Boundary)
    }

    fn derive_exchange_observation(
        &self,
        command_id: UnifiedCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        observation: &UnifiedOrderObservation,
    ) -> Result<ExchangeObservation, Error> {
        let order_id = self
            .ctf
            .parent()
            .execution()
            .order(stage_id, leg_index)
            .map(|order| order.permit.order.order_id)
            .ok_or(Error::Boundary)?;
        let event = match &observation.event {
            UnifiedOrderEvent::Delayed {
                release_at_ns,
                uncancellable_until_ns,
            } => ExchangeEvent::Delayed {
                release_at_ns: *release_at_ns,
                uncancellable_until_ns: *uncancellable_until_ns,
            },
            UnifiedOrderEvent::Acknowledged => ExchangeEvent::Acknowledged,
            UnifiedOrderEvent::Live => ExchangeEvent::Live,
            UnifiedOrderEvent::Match {
                fill_id,
                quantity_micros,
                consideration_micros,
                fee_micros,
                cumulative_quantity_micros,
                cumulative_consideration_micros,
                cumulative_fee_micros,
                fully_matched,
            } => ExchangeEvent::Match {
                fill: MatchFill {
                    fill_id: fill_id.clone(),
                    quantity_micros: *quantity_micros,
                    consideration_micros: *consideration_micros,
                    fee_micros: *fee_micros,
                    cumulative_quantity_micros: *cumulative_quantity_micros,
                    cumulative_consideration_micros: *cumulative_consideration_micros,
                    cumulative_fee_micros: *cumulative_fee_micros,
                    ledger_command_id: LedgerCommandId(derive(command_id, b"fill-ledger", 0)),
                },
                fully_matched: *fully_matched,
            },
            UnifiedOrderEvent::CancelAccepted => ExchangeEvent::CancelAccepted,
            UnifiedOrderEvent::CancelRejected => ExchangeEvent::CancelRejected,
            UnifiedOrderEvent::Rejected { class, code } => ExchangeEvent::Rejected {
                class: *class,
                code: code.clone(),
            },
            UnifiedOrderEvent::Unknown { reason } => ExchangeEvent::Unknown { reason: *reason },
        };
        Ok(ExchangeObservation {
            order_id,
            source_sequence: observation.source_sequence,
            exchange_order_id: observation.exchange_order_id.clone(),
            event,
            event_time_ns: observation.event_time_ns,
            received_time_ns: observation.received_time_ns,
        })
    }

    #[must_use]
    pub const fn ctf(&self) -> &CtfTransactionRuntime {
        &self.ctf
    }

    #[must_use]
    pub fn snapshot(&self) -> UnifiedSnapshot {
        let ctf = self.ctf.snapshot();
        let ledger = self
            .ctf
            .parent()
            .execution()
            .policy()
            .staging()
            .ledger_risk_view();
        UnifiedSnapshot {
            accepted_commands: self.accepted_commands,
            ctf_digest: ctf.digest,
            cash_available_micros: ledger.cash_available_micros,
            cash_reserved_micros: ledger.cash_reserved_micros,
            pending_conversion_count: ctf.pending_count + ctf.retrying_count,
            confirmed_conversion_count: ctf.confirmed_count,
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
        hasher.update(b"unified-paired-trading-runtime-state-v1");
        hash_json(&mut hasher, &self.config.chain_id);
        hash_json(&mut hasher, &self.config.wallet);
        hash_json(&mut hasher, &self.config.confirmation_grace_ns);
        hash_json(&mut hasher, &self.config.max_intents);
        hash_json(&mut hasher, &self.config.max_tokens);
        hasher.update(&self.ctf.snapshot().digest);
        for (id, (content, outcome)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_json(&mut hasher, outcome);
        }
        hash_json(&mut hasher, &self.accepted_commands);
        hash_json(&mut hasher, &self.last_recorded_at_ns);
        hash_json(&mut hasher, &self.armed_fault);
        hash_json(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }

    fn halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }
}

fn policy_decision(outcome: &CtfOutcome) -> Result<&PairedPolicyDecision, Error> {
    let CtfDetail::Parent(settlement) = &outcome.detail else {
        return Err(Error::Boundary);
    };
    let PairedSettlementDetail::Execution(execution) = &settlement.detail else {
        return Err(Error::Boundary);
    };
    execution.policy_decision.as_ref().ok_or(Error::Boundary)
}

fn execution_decision(
    outcome: &CtfOutcome,
) -> Result<&paired_paper_execution::PairedExecutionDecision, Error> {
    let CtfDetail::Parent(settlement) = &outcome.detail else {
        return Err(Error::Boundary);
    };
    let PairedSettlementDetail::Execution(execution) = &settlement.detail else {
        return Err(Error::Boundary);
    };
    Ok(execution)
}

fn validate_text(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
        Err(Error::Identifier)
    } else {
        Ok(())
    }
}

fn derive(id: UnifiedCommandId, domain: &[u8], step: u8) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"unified-paired-trading-child-v1");
    hasher.update(&id.0);
    hasher.update(domain);
    hasher.update(&[step]);
    *hasher.finalize().as_bytes()
}

fn ctf_id(id: UnifiedCommandId, step: u8) -> CtfCommandId {
    CtfCommandId(derive(id, b"ctf", step))
}

fn settlement_id(id: UnifiedCommandId, step: u8) -> PairedSettlementCommandId {
    PairedSettlementCommandId(derive(id, b"settlement", step))
}

fn execution_id(id: UnifiedCommandId, step: u8) -> PairedExecutionCommandId {
    PairedExecutionCommandId(derive(id, b"execution", step))
}

fn policy_id(id: UnifiedCommandId, step: u8) -> PairedPolicyCommandId {
    PairedPolicyCommandId(derive(id, b"policy", step))
}

fn paired_id(id: UnifiedCommandId, step: u8) -> PairedCommandId {
    PairedCommandId(derive(id, b"paired", step))
}

fn arbitrage_id(id: UnifiedCommandId, step: u8) -> ArbitrageCommandId {
    ArbitrageCommandId(derive(id, b"arbitrage", step))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: UnifiedCommand,
}

/// Encodes a bounded, versioned command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &UnifiedCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes a bounded, versioned command.
///
/// # Errors
///
/// Rejects malformed, oversized, trailing, or unsupported input.
pub fn decode_command(bytes: &[u8]) -> Result<UnifiedCommand, Error> {
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
    Ok(wire.command)
}

fn outcome_digest(outcome: &UnifiedOutcome) -> [u8; 32] {
    let mut clone = outcome.clone();
    clone.outcome_digest = [0; 32];
    *blake3::hash(&serde_json::to_vec(&clone).expect("serializable outcome")).as_bytes()
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[cfg(test)]
mod tests;
