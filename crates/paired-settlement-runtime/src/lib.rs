#![forbid(unsafe_code)]

//! Deterministic paired settlement and confirmed-accounting composition.
//!
//! This crate is offline. It owns no credential, signer, authenticated client,
//! RPC, wallet action, split/merge transaction, automatic retry, or live order.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurablePairedSettlement,
    PairedSettlementCheckpoint, PairedSettlementRecovery, StorageError,
};

use accounting_ledger::{
    CommandId as LedgerCommandId, LedgerCommand, LedgerReconciliationView, LockId,
    ReservationStatus,
};
use paired_capital_staging::PairStageId;
use paired_opportunity_runtime::PairedCommand;
use paired_paper_execution::{
    PairedExecutionCommand, PairedExecutionDecision, PairedPaperExecution,
};
use paired_placement_policy::PairedPolicyCommand;
use portfolio_risk::OrderSide;
use serde::{Deserialize, Serialize};
use settlement_reconciliation::{
    FinalizedChainSnapshot, ReconcilerConfig, ReconciliationCommand, ReconciliationCommandId,
    ReconciliationFrame, SettlementReconciler, Side, TradeObservation, TradeStatus,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_CONFIRMATION_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PairedSettlementCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredHandoff {
    pub stage_id: PairStageId,
    pub leg_index: u8,
    pub handoff_index: usize,
    pub handoff_digest: [u8; 32],
    pub intent: settlement_reconciliation::TradeIntent,
    pub posted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PairedSettlementCommand {
    Execution {
        command_id: PairedSettlementCommandId,
        command: Box<PairedExecutionCommand>,
        recorded_at_ns: i64,
    },
    RegisterHandoff {
        command_id: PairedSettlementCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        handoff_index: usize,
        recorded_at_ns: i64,
    },
    ObserveTrade {
        command_id: PairedSettlementCommandId,
        observation: TradeObservation,
        recorded_at_ns: i64,
    },
    PostConfirmed {
        command_id: PairedSettlementCommandId,
        ledger_command_id: LedgerCommandId,
        recorded_at_ns: i64,
    },
    Reconcile {
        command_id: PairedSettlementCommandId,
        chain: FinalizedChainSnapshot,
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    },
    LockCompletePair {
        command_id: PairedSettlementCommandId,
        stage_id: PairStageId,
        lock_id: LockId,
        quantity_micros: i128,
        recorded_at_ns: i64,
    },
    FinalizeStage {
        command_id: PairedSettlementCommandId,
        stage_id: PairStageId,
        recorded_at_ns: i64,
    },
}

impl PairedSettlementCommand {
    #[must_use]
    pub const fn command_id(&self) -> PairedSettlementCommandId {
        match self {
            Self::Execution { command_id, .. }
            | Self::RegisterHandoff { command_id, .. }
            | Self::ObserveTrade { command_id, .. }
            | Self::PostConfirmed { command_id, .. }
            | Self::Reconcile { command_id, .. }
            | Self::LockCompletePair { command_id, .. }
            | Self::FinalizeStage { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Execution { recorded_at_ns, .. }
            | Self::RegisterHandoff { recorded_at_ns, .. }
            | Self::ObserveTrade { recorded_at_ns, .. }
            | Self::PostConfirmed { recorded_at_ns, .. }
            | Self::Reconcile { recorded_at_ns, .. }
            | Self::LockCompletePair { recorded_at_ns, .. }
            | Self::FinalizeStage { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedSettlementStage {
    Execution,
    Handoff,
    Settlement,
    Accounting,
    Reconciliation,
    PairLock,
    Finalization,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PairedSettlementDetail {
    Execution(Box<PairedExecutionDecision>),
    HandoffRegistered,
    TradeObserved(TradeStatus),
    ConfirmedFillPosted,
    Reconciled,
    ReconciliationPending,
    CompletePairLocked,
    StageFinalized,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedSettlementOutcome {
    pub command_id: PairedSettlementCommandId,
    pub stage: PairedSettlementStage,
    pub detail: PairedSettlementDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairedSettlementSnapshot {
    pub accepted_commands: u64,
    pub execution_digest: [u8; 32],
    pub reconciliation_digest: [u8; 32],
    pub registered_handoffs: usize,
    pub posted_handoffs: usize,
    pub finalized_stages: usize,
    pub locked_stages: usize,
    pub reconciliation_current: bool,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("paired settlement configuration is invalid")]
    Config,
    #[error("paired settlement timestamp is invalid or regressed")]
    Timestamp,
    #[error("paired settlement command exceeds its canonical bound")]
    CommandBound,
    #[error("paired settlement command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported paired settlement command version: {0}")]
    Version(u16),
    #[error("paired settlement command id was reused for different content")]
    IdempotencyConflict,
    #[error("execution, handoff, stage, or ledger provenance was substituted")]
    Boundary,
    #[error("handoff is missing, changed, duplicated, or not terminal")]
    Handoff,
    #[error("trade is not confirmed or was already posted")]
    Confirmation,
    #[error("authoritative reconciliation is not current")]
    ReconciliationNotCurrent,
    #[error("paired reservations cannot yet be finalized")]
    UnsafeFinalization,
    #[error("complete-pair lock is invalid")]
    PairLock,
    #[error("execution child failed: {0}")]
    Execution(String),
    #[error("reconciliation child failed: {0}")]
    Reconciliation(String),
    #[error("accounting child failed: {0}")]
    Accounting(String),
    #[error("paired settlement arithmetic overflow")]
    Overflow,
    #[error("paired settlement runtime is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct PairedSettlementRuntime {
    config: ReconcilerConfig,
    execution: PairedPaperExecution,
    reconciler: SettlementReconciler,
    handoffs: BTreeMap<LedgerCommandId, RegisteredHandoff>,
    handoff_digests: BTreeSet<[u8; 32]>,
    finalized_stages: BTreeSet<PairStageId>,
    locked_stages: BTreeMap<PairStageId, LockId>,
    processed: BTreeMap<PairedSettlementCommandId, ([u8; 32], PairedSettlementOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl PairedSettlementRuntime {
    /// Creates one empty offline paired settlement owner.
    ///
    /// # Errors
    ///
    /// Rejects invalid reconciliation identity or bounds.
    pub fn new(config: ReconcilerConfig) -> Result<Self, Error> {
        let reconciler = SettlementReconciler::new(config.clone()).map_err(|_| Error::Config)?;
        Ok(Self {
            config,
            execution: PairedPaperExecution::default(),
            reconciler,
            handoffs: BTreeMap::new(),
            handoff_digests: BTreeSet::new(),
            finalized_stages: BTreeSet::new(),
            locked_stages: BTreeMap::new(),
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one composed command atomically.
    ///
    /// # Errors
    ///
    /// Returns absorbing provenance, lifecycle, accounting, reconciliation, or
    /// durable-integrity failures.
    pub fn apply(
        &mut self,
        command: &PairedSettlementCommand,
    ) -> Result<PairedSettlementOutcome, Error> {
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
        let mut result = match next.apply_fresh(command) {
            Ok(value) => value,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = next
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        result.outcome_digest = outcome_digest(&result);
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.processed.insert(id, (content, result.clone()));
        *self = next;
        Ok(result)
    }

    fn apply_fresh(
        &mut self,
        command: &PairedSettlementCommand,
    ) -> Result<PairedSettlementOutcome, Error> {
        match command {
            PairedSettlementCommand::Execution {
                command_id,
                command,
                recorded_at_ns,
            } => self.apply_execution(*command_id, command, *recorded_at_ns),
            PairedSettlementCommand::RegisterHandoff {
                command_id,
                stage_id,
                leg_index,
                handoff_index,
                recorded_at_ns,
            } => self.register_handoff(
                *command_id,
                *stage_id,
                *leg_index,
                *handoff_index,
                *recorded_at_ns,
            ),
            PairedSettlementCommand::ObserveTrade {
                command_id,
                observation,
                recorded_at_ns,
            } => self.observe_trade(*command_id, observation, *recorded_at_ns),
            PairedSettlementCommand::PostConfirmed {
                command_id,
                ledger_command_id,
                recorded_at_ns,
            } => self.post_confirmed(*command_id, *ledger_command_id, *recorded_at_ns),
            PairedSettlementCommand::Reconcile {
                command_id,
                chain,
                evaluated_at_ns,
                recorded_at_ns,
            } => self.reconcile(
                *command_id,
                chain.clone(),
                *evaluated_at_ns,
                *recorded_at_ns,
            ),
            PairedSettlementCommand::LockCompletePair {
                command_id,
                stage_id,
                lock_id,
                quantity_micros,
                recorded_at_ns,
            } => self.lock_complete_pair(
                *command_id,
                *stage_id,
                *lock_id,
                *quantity_micros,
                *recorded_at_ns,
            ),
            PairedSettlementCommand::FinalizeStage {
                command_id,
                stage_id,
                recorded_at_ns,
            } => self.finalize_stage(*command_id, *stage_id, *recorded_at_ns),
        }
    }

    fn apply_execution(
        &mut self,
        id: PairedSettlementCommandId,
        command: &PairedExecutionCommand,
        at: i64,
    ) -> Result<PairedSettlementOutcome, Error> {
        if command.recorded_at_ns() != at {
            return Err(Error::Boundary);
        }
        if let PairedExecutionCommand::Policy {
            command: policy_command,
            ..
        } = command
        {
            if let PairedPolicyCommand::Stage { paired_command, .. } = policy_command.as_ref() {
                let PairedCommand::Evaluate { risk_frame, .. } = paired_command.as_ref();
                if !self.reconciliation_is_current()
                    || risk_frame.reconciliation != self.reconciler.risk_gate()
                    || risk_frame.ledger != self.execution.policy().staging().ledger_risk_view()
                {
                    return Err(Error::Boundary);
                }
            }
        }
        let decision = self
            .execution
            .apply(command)
            .map_err(|error| Error::Execution(error.to_string()))?;
        Ok(outcome(
            id,
            PairedSettlementStage::Execution,
            PairedSettlementDetail::Execution(Box::new(decision)),
        ))
    }

    fn register_handoff(
        &mut self,
        id: PairedSettlementCommandId,
        stage_id: PairStageId,
        leg_index: u8,
        index: usize,
        at: i64,
    ) -> Result<PairedSettlementOutcome, Error> {
        let handoff = self
            .execution
            .order(stage_id, leg_index)
            .and_then(|order| order.handoffs.get(index))
            .cloned()
            .ok_or(Error::Handoff)?;
        let digest = handoff_digest(&handoff)?;
        if self
            .handoffs
            .contains_key(&handoff.intent.ledger_command_id)
            || !self.handoff_digests.insert(digest)
        {
            return Err(Error::Handoff);
        }
        self.reconciler
            .apply(&ReconciliationCommand::RegisterIntent {
                command_id: ReconciliationCommandId(digest),
                intent: handoff.intent.clone(),
                recorded_at_ns: at,
            })
            .map_err(|error| Error::Reconciliation(error.to_string()))?;
        self.handoffs.insert(
            handoff.intent.ledger_command_id,
            RegisteredHandoff {
                stage_id,
                leg_index,
                handoff_index: index,
                handoff_digest: digest,
                intent: handoff.intent,
                posted: false,
            },
        );
        Ok(outcome(
            id,
            PairedSettlementStage::Handoff,
            PairedSettlementDetail::HandoffRegistered,
        ))
    }

    fn observe_trade(
        &mut self,
        id: PairedSettlementCommandId,
        observation: &TradeObservation,
        at: i64,
    ) -> Result<PairedSettlementOutcome, Error> {
        if observation.updated_at_ns > at
            || !self
                .handoffs
                .values()
                .any(|record| record.intent.trade_id == observation.trade_id)
        {
            return Err(Error::Boundary);
        }
        self.reconciler
            .apply(&ReconciliationCommand::ObserveTrade {
                command_id: derived_reconciliation_id(id, b"observe"),
                observation: observation.clone(),
                recorded_at_ns: at,
            })
            .map_err(|error| Error::Reconciliation(error.to_string()))?;
        Ok(outcome(
            id,
            PairedSettlementStage::Settlement,
            PairedSettlementDetail::TradeObserved(observation.status),
        ))
    }

    fn post_confirmed(
        &mut self,
        id: PairedSettlementCommandId,
        ledger_id: LedgerCommandId,
        at: i64,
    ) -> Result<PairedSettlementOutcome, Error> {
        let record = self.handoffs.get(&ledger_id).ok_or(Error::Handoff)?;
        if record.posted {
            return Err(Error::Confirmation);
        }
        let trade = self
            .reconciler
            .trade(&record.intent.trade_id)
            .ok_or(Error::Confirmation)?;
        if trade.status != TradeStatus::Confirmed {
            return Err(Error::Confirmation);
        }
        let confirmation = trade
            .transaction_hash
            .as_ref()
            .filter(|value| !value.is_empty() && value.len() <= MAX_CONFIRMATION_BYTES)
            .ok_or(Error::Confirmation)?
            .clone();
        let order = self
            .execution
            .order(record.stage_id, record.leg_index)
            .ok_or(Error::Boundary)?;
        if order
            .handoffs
            .get(record.handoff_index)
            .map(|handoff| &handoff.intent)
            != Some(&record.intent)
        {
            return Err(Error::Handoff);
        }
        let reservation_id = order.permit.reservation_id;
        let intent = &record.intent;
        let command = match intent.side {
            Side::Buy => LedgerCommand::ConfirmBuy {
                command_id: intent.ledger_command_id,
                reservation_id,
                token: intent.token.clone(),
                quantity_micros: intent.quantity_micros,
                consideration_micros: intent.consideration_micros,
                fee_micros: intent.fee_micros,
                confirmation,
                recorded_at_ns: at,
            },
            Side::Sell => LedgerCommand::ConfirmSell {
                command_id: intent.ledger_command_id,
                reservation_id,
                quantity_micros: intent.quantity_micros,
                gross_proceeds_micros: intent.consideration_micros,
                fee_micros: intent.fee_micros,
                confirmation,
                recorded_at_ns: at,
            },
        };
        self.execution
            .settlement_apply_batch(&[command])
            .map_err(|error| Error::Accounting(error.to_string()))?;
        self.handoffs
            .get_mut(&ledger_id)
            .ok_or(Error::Handoff)?
            .posted = true;
        Ok(outcome(
            id,
            PairedSettlementStage::Accounting,
            PairedSettlementDetail::ConfirmedFillPosted,
        ))
    }

    fn reconcile(
        &mut self,
        id: PairedSettlementCommandId,
        chain: FinalizedChainSnapshot,
        evaluated_at: i64,
        at: i64,
    ) -> Result<PairedSettlementOutcome, Error> {
        if evaluated_at > at {
            return Err(Error::Timestamp);
        }
        let frame = ReconciliationFrame {
            ledger: self.ledger_reconciliation_view(),
            chain,
            evaluated_at_ns: evaluated_at,
        };
        self.reconciler
            .apply(&ReconciliationCommand::Reconcile {
                command_id: derived_reconciliation_id(id, b"frame"),
                frame,
                recorded_at_ns: at,
            })
            .map_err(|error| Error::Reconciliation(error.to_string()))?;
        let detail = if self.reconciler.snapshot().ready {
            PairedSettlementDetail::Reconciled
        } else {
            PairedSettlementDetail::ReconciliationPending
        };
        Ok(outcome(id, PairedSettlementStage::Reconciliation, detail))
    }

    fn lock_complete_pair(
        &mut self,
        id: PairedSettlementCommandId,
        stage_id: PairStageId,
        lock_id: LockId,
        quantity: i128,
        at: i64,
    ) -> Result<PairedSettlementOutcome, Error> {
        if quantity <= 0
            || self.locked_stages.contains_key(&stage_id)
            || !self.reconciliation_is_current()
        {
            return Err(Error::PairLock);
        }
        let stage = self
            .execution
            .policy()
            .staging()
            .stage_record(stage_id)
            .ok_or(Error::PairLock)?;
        if stage
            .candidates
            .iter()
            .any(|candidate| candidate.side != OrderSide::Buy)
            || stage.candidates[0].token.condition_id != stage.candidates[1].token.condition_id
            || stage.candidates[0].token == stage.candidates[1].token
        {
            return Err(Error::PairLock);
        }
        let mut posted = [0_i128; 2];
        for record in self
            .handoffs
            .values()
            .filter(|record| record.stage_id == stage_id)
        {
            if record.posted {
                let slot = posted
                    .get_mut(usize::from(record.leg_index))
                    .ok_or(Error::PairLock)?;
                *slot = slot
                    .checked_add(record.intent.quantity_micros)
                    .ok_or(Error::Overflow)?;
            }
        }
        if posted[0] < quantity || posted[1] < quantity {
            return Err(Error::PairLock);
        }
        self.execution
            .settlement_apply_batch(&[LedgerCommand::LockPair {
                command_id: derived_ledger_id(id, b"lock", 0),
                lock_id,
                up: stage.candidates[0].token.clone(),
                down: stage.candidates[1].token.clone(),
                quantity_micros: quantity,
                recorded_at_ns: at,
            }])
            .map_err(|error| Error::Accounting(error.to_string()))?;
        self.locked_stages.insert(stage_id, lock_id);
        Ok(outcome(
            id,
            PairedSettlementStage::PairLock,
            PairedSettlementDetail::CompletePairLocked,
        ))
    }

    fn finalize_stage(
        &mut self,
        id: PairedSettlementCommandId,
        stage_id: PairStageId,
        at: i64,
    ) -> Result<PairedSettlementOutcome, Error> {
        if self.finalized_stages.contains(&stage_id) || !self.reconciliation_is_current() {
            return Err(Error::ReconciliationNotCurrent);
        }
        let stage = self
            .execution
            .policy()
            .staging()
            .stage_record(stage_id)
            .cloned()
            .ok_or(Error::UnsafeFinalization)?;
        for leg in 0_u8..=1 {
            let order = self
                .execution
                .order(stage_id, leg)
                .ok_or(Error::UnsafeFinalization)?;
            if !order.state.is_terminal() {
                return Err(Error::UnsafeFinalization);
            }
            for handoff in &order.handoffs {
                let record = self
                    .handoffs
                    .get(&handoff.intent.ledger_command_id)
                    .ok_or(Error::UnsafeFinalization)?;
                let trade = self
                    .reconciler
                    .trade(&record.intent.trade_id)
                    .ok_or(Error::UnsafeFinalization)?;
                if !trade.status.is_terminal()
                    || (trade.status == TradeStatus::Confirmed) != record.posted
                {
                    return Err(Error::UnsafeFinalization);
                }
            }
        }
        let mut releases = Vec::new();
        for (index, reservation_id) in stage.reservation_ids.into_iter().enumerate() {
            let reservation = self
                .execution
                .policy()
                .staging()
                .reservation(reservation_id)
                .ok_or(Error::UnsafeFinalization)?;
            if reservation.status == ReservationStatus::Active {
                releases.push(LedgerCommand::ReleaseReservation {
                    command_id: derived_ledger_id(id, b"release", index),
                    reservation_id,
                    recorded_at_ns: at,
                });
            }
        }
        self.execution
            .settlement_apply_batch(&releases)
            .map_err(|error| Error::Accounting(error.to_string()))?;
        for reservation_id in stage.reservation_ids {
            if self
                .execution
                .policy()
                .staging()
                .reservation(reservation_id)
                .is_some_and(|reservation| reservation.status == ReservationStatus::Active)
            {
                return Err(Error::UnsafeFinalization);
            }
        }
        self.finalized_stages.insert(stage_id);
        Ok(outcome(
            id,
            PairedSettlementStage::Finalization,
            PairedSettlementDetail::StageFinalized,
        ))
    }

    #[must_use]
    pub const fn execution(&self) -> &PairedPaperExecution {
        &self.execution
    }

    #[must_use]
    pub const fn reconciler(&self) -> &SettlementReconciler {
        &self.reconciler
    }

    #[must_use]
    pub fn handoff(&self, id: LedgerCommandId) -> Option<&RegisteredHandoff> {
        self.handoffs.get(&id)
    }

    #[must_use]
    pub fn ledger_reconciliation_view(&self) -> LedgerReconciliationView {
        self.execution
            .settlement_reconciliation_view(&self.handoffs.keys().copied().collect())
    }

    #[doc(hidden)]
    pub fn conversion_apply_batch(&mut self, commands: &[LedgerCommand]) -> Result<(), Error> {
        self.execution
            .settlement_apply_batch(commands)
            .map_err(|error| Error::Accounting(error.to_string()))
    }

    #[doc(hidden)]
    #[must_use]
    pub fn conversion_pair_lock(&self, id: LockId) -> Option<&accounting_ledger::PairLock> {
        self.execution.conversion_pair_lock(id)
    }

    #[must_use]
    pub fn reconciliation_is_current(&self) -> bool {
        let snapshot = self.reconciler.snapshot();
        snapshot.ready
            && snapshot.ledger_digest == Some(self.ledger_reconciliation_view().ledger_digest)
    }

    #[must_use]
    pub fn snapshot(&self) -> PairedSettlementSnapshot {
        PairedSettlementSnapshot {
            accepted_commands: self.accepted_commands,
            execution_digest: self.execution.snapshot().digest,
            reconciliation_digest: self.reconciler.snapshot().digest,
            registered_handoffs: self.handoffs.len(),
            posted_handoffs: self.handoffs.values().filter(|value| value.posted).count(),
            finalized_stages: self.finalized_stages.len(),
            locked_stages: self.locked_stages.len(),
            reconciliation_current: self.reconciliation_is_current(),
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
        hasher.update(b"paired-settlement-runtime-state-v1");
        hash_json(&mut hasher, &self.config.chain_id);
        hash_json(&mut hasher, &self.config.wallet);
        hash_json(&mut hasher, &self.config.confirmation_grace_ns);
        hash_json(&mut hasher, &self.config.max_intents);
        hash_json(&mut hasher, &self.config.max_tokens);
        hasher.update(&self.execution.snapshot().digest);
        hasher.update(&self.reconciler.snapshot().digest);
        for (id, handoff) in &self.handoffs {
            hasher.update(&id.0);
            hash_json(&mut hasher, handoff);
        }
        hash_json(&mut hasher, &self.handoff_digests);
        hash_json(&mut hasher, &self.finalized_stages);
        for (stage, lock) in &self.locked_stages {
            hasher.update(&stage.0);
            hasher.update(&lock.0);
        }
        for (id, (content, outcome)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_json(&mut hasher, outcome);
        }
        hash_json(&mut hasher, &self.accepted_commands);
        hash_json(&mut hasher, &self.last_recorded_at_ns);
        hash_json(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }

    fn halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }
}

fn outcome(
    id: PairedSettlementCommandId,
    stage: PairedSettlementStage,
    detail: PairedSettlementDetail,
) -> PairedSettlementOutcome {
    PairedSettlementOutcome {
        command_id: id,
        stage,
        detail,
        outcome_digest: [0; 32],
    }
}

fn outcome_digest(value: &PairedSettlementOutcome) -> [u8; 32] {
    let mut copy = value.clone();
    copy.outcome_digest = [0; 32];
    *blake3::hash(&serde_json::to_vec(&copy).expect("serializable outcome")).as_bytes()
}

fn handoff_digest(value: &paper_execution::ReconciliationHandoff) -> Result<[u8; 32], Error> {
    serde_json::to_vec(value)
        .map(|bytes| *blake3::hash(&bytes).as_bytes())
        .map_err(|error| Error::Json(error.to_string()))
}

fn derived_reconciliation_id(
    id: PairedSettlementCommandId,
    domain: &[u8],
) -> ReconciliationCommandId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-settlement-reconciliation-v1");
    hasher.update(domain);
    hasher.update(&id.0);
    ReconciliationCommandId(*hasher.finalize().as_bytes())
}

fn derived_ledger_id(
    id: PairedSettlementCommandId,
    domain: &[u8],
    index: usize,
) -> LedgerCommandId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"paired-settlement-ledger-v1");
    hasher.update(domain);
    hasher.update(&id.0);
    hasher.update(&index.to_le_bytes());
    LedgerCommandId(*hasher.finalize().as_bytes())
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: PairedSettlementCommand,
}

pub(crate) fn encode_command(command: &PairedSettlementCommand) -> Result<Vec<u8>, Error> {
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

pub(crate) fn decode_command(bytes: &[u8]) -> Result<PairedSettlementCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let wire: WireCommand =
        serde_json::from_slice(bytes).map_err(|error| Error::Json(error.to_string()))?;
    if wire.version != WIRE_VERSION {
        return Err(Error::Version(wire.version));
    }
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
