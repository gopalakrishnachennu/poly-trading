#![forbid(unsafe_code)]

//! Deterministic offline canary-rollout and abort-controller simulation.
//!
//! This crate cannot deploy, route traffic, allocate capital, execute rollback,
//! authenticate, sign, access RPC or wallet state, or submit a live order or
//! transaction. Every action and report is simulation evidence only.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableRolloutSimulator,
    RolloutCheckpoint, RolloutRecovery, RolloutStorageError,
};
pub use report::{read_rollout_report, write_rollout_report_create_new, RolloutReportFileError};

use promotion_governance::{CanaryEligibilityRecord, CanaryStatus, RollbackCriteria};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 4 * 1024 * 1024;
const MAX_WINDOWS_HARD: usize = 512;
const MAX_STAGES_HARD: usize = 64;
const BASIS_POINTS: u16 = 10_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct RolloutCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulatorPolicy {
    pub maximum_windows: usize,
    pub maximum_stages: usize,
    pub maximum_health_age_ns: i64,
    pub maximum_plan_age_ns: i64,
    pub maximum_target_bps: u16,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceWindow {
    pub window_id: [u8; 32],
    pub start_ns: i64,
    pub end_ns: i64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolloutStage {
    pub stage_id: [u8; 32],
    pub target_bps: u16,
    pub minimum_observation_ns: i64,
    pub maximum_stage_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolloutPlan {
    pub plan_id: [u8; 32],
    pub created_at_ns: i64,
    pub scheduled_start_ns: i64,
    pub scheduled_end_ns: i64,
    pub eligibility: CanaryEligibilityRecord,
    pub rollback: RollbackCriteria,
    pub windows: Vec<MaintenanceWindow>,
    pub stages: Vec<RolloutStage>,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl RolloutPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &SimulatorPolicy) -> Self {
        self.policy_digest = digest_json(b"canary-rollout-policy-v1", policy);
        self.plan_digest = rollout_plan_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &SimulatorPolicy) -> bool {
        self.policy_digest == digest_json(b"canary-rollout-policy-v1", policy)
            && self.plan_digest == rollout_plan_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct HealthFrame {
    pub sequence: u64,
    pub observed_at_ns: i64,
    pub valid_until_ns: i64,
    pub strategy_healthy: bool,
    pub risk_healthy: bool,
    pub market_feed_healthy: bool,
    pub user_feed_healthy: bool,
    pub reconciliation_healthy: bool,
    pub capital_floor_preserved: bool,
    pub unreconciled_age_ns: i64,
    pub unknown_state_age_ns: i64,
    pub session_loss_micros: i128,
    pub consecutive_faults: u64,
    pub source_digest: [u8; 32],
    pub frame_digest: [u8; 32],
}

impl HealthFrame {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.frame_digest = health_frame_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.frame_digest == health_frame_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutMode {
    Registered,
    Running,
    Paused,
    Recovering,
    Completed,
    Aborted,
    RollbackRequired,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    Operator,
    StackUnhealthy,
    HealthStale,
    OutsideMaintenanceWindow,
    RestartRecovery,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackTrigger {
    CapitalFloorBreach,
    ReconciliationTimeout,
    UnknownStateTimeout,
    SessionLossLimit,
    ConsecutiveFaultLimit,
    StageTimeout,
    PlanTimeout,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RolloutCommand {
    RegisterPlan {
        command_id: RolloutCommandId,
        plan: Box<RolloutPlan>,
        recorded_at_ns: i64,
    },
    ObserveHealth {
        command_id: RolloutCommandId,
        plan_id: [u8; 32],
        frame: HealthFrame,
        recorded_at_ns: i64,
    },
    Start {
        command_id: RolloutCommandId,
        plan_id: [u8; 32],
        recorded_at_ns: i64,
    },
    Advance {
        command_id: RolloutCommandId,
        plan_id: [u8; 32],
        recorded_at_ns: i64,
    },
    Tick {
        command_id: RolloutCommandId,
        plan_id: [u8; 32],
        recorded_at_ns: i64,
    },
    OperatorPause {
        command_id: RolloutCommandId,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        reason_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    OperatorResume {
        command_id: RolloutCommandId,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        recorded_at_ns: i64,
    },
    OperatorAbort {
        command_id: RolloutCommandId,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        reason_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    Restart {
        command_id: RolloutCommandId,
        plan_id: [u8; 32],
        restart_id: [u8; 32],
        recorded_at_ns: i64,
    },
    Recover {
        command_id: RolloutCommandId,
        plan_id: [u8; 32],
        recovery_epoch: u64,
        evidence_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: RolloutCommandId,
        plan_id: [u8; 32],
        report_id: [u8; 32],
        recorded_at_ns: i64,
    },
}

impl RolloutCommand {
    #[must_use]
    pub const fn command_id(&self) -> RolloutCommandId {
        match self {
            Self::RegisterPlan { command_id, .. }
            | Self::ObserveHealth { command_id, .. }
            | Self::Start { command_id, .. }
            | Self::Advance { command_id, .. }
            | Self::Tick { command_id, .. }
            | Self::OperatorPause { command_id, .. }
            | Self::OperatorResume { command_id, .. }
            | Self::OperatorAbort { command_id, .. }
            | Self::Restart { command_id, .. }
            | Self::Recover { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::RegisterPlan { recorded_at_ns, .. }
            | Self::ObserveHealth { recorded_at_ns, .. }
            | Self::Start { recorded_at_ns, .. }
            | Self::Advance { recorded_at_ns, .. }
            | Self::Tick { recorded_at_ns, .. }
            | Self::OperatorPause { recorded_at_ns, .. }
            | Self::OperatorResume { recorded_at_ns, .. }
            | Self::OperatorAbort { recorded_at_ns, .. }
            | Self::Restart { recorded_at_ns, .. }
            | Self::Recover { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutReportStatus {
    SimulatedCompleted,
    OperatorAborted,
    RollbackRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RolloutReport {
    pub report_id: [u8; 32],
    pub plan_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub eligibility_record_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
    pub rollback_digest: [u8; 32],
    pub finalized_at_ns: i64,
    pub status: RolloutReportStatus,
    pub completed_stage_count: usize,
    pub final_stage_index: Option<usize>,
    pub final_target_bps: u16,
    pub health_frame_count: u64,
    pub pause_count: u64,
    pub restart_count: u64,
    pub recovery_epoch: u64,
    pub rollback_trigger: Option<RollbackTrigger>,
    pub abort_operator_id: Option<[u8; 32]>,
    pub operator_execution_required: bool,
    pub rollout_execution_authority_granted: bool,
    pub rollback_execution_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub credential_authority_granted: bool,
    pub live_trading_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl RolloutReport {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest == rollout_report_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum RolloutDetail {
    PlanRegistered,
    HealthObserved {
        healthy: bool,
        rollback_trigger: Option<RollbackTrigger>,
    },
    Started {
        stage_index: usize,
        target_bps: u16,
    },
    Advanced {
        stage_index: usize,
        target_bps: u16,
    },
    Completed,
    Paused(PauseReason),
    Resumed,
    Aborted,
    Restarted,
    Recovered {
        recovery_epoch: u64,
    },
    TickApplied,
    RollbackLatched(RollbackTrigger),
    Finalized(Box<RolloutReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolloutOutcome {
    pub command_id: RolloutCommandId,
    pub detail: RolloutDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RolloutSnapshot {
    pub accepted_commands: u64,
    pub plan_id: Option<[u8; 32]>,
    pub mode: Option<RolloutMode>,
    pub pause_reason: Option<PauseReason>,
    pub stage_index: Option<usize>,
    pub latest_health: Option<HealthFrame>,
    pub rollback_trigger: Option<RollbackTrigger>,
    pub recovery_epoch: u64,
    pub last_report: Option<RolloutReport>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("canary-rollout simulator configuration is invalid")]
    Config,
    #[error("canary-rollout timestamp is invalid or regressed")]
    Timestamp,
    #[error("canary-rollout command exceeds its canonical bound")]
    CommandBound,
    #[error("canary-rollout command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported canary-rollout command version: {0}")]
    Version(u16),
    #[error("canary-rollout command id was reused for different content")]
    IdempotencyConflict,
    #[error("canary-rollout plan identity, digest, or bounds are invalid")]
    Plan,
    #[error("Phase 2.18 eligibility record is invalid or unavailable")]
    Eligibility,
    #[error("rollback-criteria binding is invalid")]
    Rollback,
    #[error("health frame identity, sequence, time, or digest is invalid")]
    Health,
    #[error("canary-rollout lifecycle transition is invalid")]
    Lifecycle,
    #[error("current health or maintenance-window gate is closed")]
    GateClosed,
    #[error("operator accountability fields are invalid")]
    Operator,
    #[error("restart recovery evidence or epoch is invalid")]
    Recovery,
    #[error("rollout report identity or terminal state is invalid")]
    Report,
    #[error("canary-rollout arithmetic or counter overflow")]
    Overflow,
    #[error("canary-rollout simulator is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct CanaryRolloutSimulator {
    policy: SimulatorPolicy,
    plan: Option<RolloutPlan>,
    mode: Option<RolloutMode>,
    pause_reason: Option<PauseReason>,
    stage_index: Option<usize>,
    started_at_ns: Option<i64>,
    stage_started_at_ns: Option<i64>,
    latest_health: Option<HealthFrame>,
    health_frame_count: u64,
    pause_count: u64,
    restart_count: u64,
    restart_at_ns: Option<i64>,
    recovery_epoch: u64,
    rollback_trigger: Option<RollbackTrigger>,
    abort_operator_id: Option<[u8; 32]>,
    report: Option<RolloutReport>,
    processed: BTreeMap<RolloutCommandId, ([u8; 32], RolloutOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl CanaryRolloutSimulator {
    /// Creates an empty offline rollout simulator.
    ///
    /// # Errors
    ///
    /// Rejects invalid bounds or target limits.
    pub fn new(policy: SimulatorPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            mode: None,
            pause_reason: None,
            stage_index: None,
            started_at_ns: None,
            stage_started_at_ns: None,
            latest_health: None,
            health_frame_count: 0,
            pause_count: 0,
            restart_count: 0,
            restart_at_ns: None,
            recovery_epoch: 0,
            rollback_trigger: None,
            abort_operator_id: None,
            report: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic simulated rollout command transactionally.
    ///
    /// # Errors
    ///
    /// Identity, chronology, lifecycle, integrity, or arithmetic failures halt.
    pub fn apply(&mut self, command: &RolloutCommand) -> Result<RolloutOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        if command.recorded_at_ns() < 0
            || self
                .last_recorded_at_ns
                .is_some_and(|previous| command.recorded_at_ns() < previous)
        {
            return self.halt(Error::Timestamp);
        }
        let encoded = encode_command(command)?;
        let content = *blake3::hash(&encoded).as_bytes();
        let command_id = command.command_id();
        if let Some((existing, outcome)) = self.processed.get(&command_id) {
            if *existing == content {
                return Ok(outcome.clone());
            }
            return self.halt(Error::IdempotencyConflict);
        }
        let mut next = self.clone();
        let detail = match next.apply_fresh(command) {
            Ok(detail) => detail,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = next
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        let mut outcome = RolloutOutcome {
            command_id,
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = rollout_outcome_digest(&outcome);
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.processed
            .insert(command_id, (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn apply_fresh(&mut self, command: &RolloutCommand) -> Result<RolloutDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Lifecycle);
        }
        match command {
            RolloutCommand::RegisterPlan {
                plan,
                recorded_at_ns,
                ..
            } => self.register_plan(plan, *recorded_at_ns),
            RolloutCommand::ObserveHealth {
                plan_id,
                frame,
                recorded_at_ns,
                ..
            } => self.observe_health(*plan_id, frame, *recorded_at_ns),
            RolloutCommand::Start {
                plan_id,
                recorded_at_ns,
                ..
            } => self.start(*plan_id, *recorded_at_ns),
            RolloutCommand::Advance {
                plan_id,
                recorded_at_ns,
                ..
            } => self.advance(*plan_id, *recorded_at_ns),
            RolloutCommand::Tick {
                plan_id,
                recorded_at_ns,
                ..
            } => self.tick(*plan_id, *recorded_at_ns),
            RolloutCommand::OperatorPause {
                plan_id,
                operator_id,
                reason_digest,
                ..
            } => self.operator_pause(*plan_id, *operator_id, *reason_digest),
            RolloutCommand::OperatorResume {
                plan_id,
                operator_id,
                recorded_at_ns,
                ..
            } => self.operator_resume(*plan_id, *operator_id, *recorded_at_ns),
            RolloutCommand::OperatorAbort {
                plan_id,
                operator_id,
                reason_digest,
                ..
            } => self.operator_abort(*plan_id, *operator_id, *reason_digest),
            RolloutCommand::Restart {
                plan_id,
                restart_id,
                recorded_at_ns,
                ..
            } => self.restart(*plan_id, *restart_id, *recorded_at_ns),
            RolloutCommand::Recover {
                plan_id,
                recovery_epoch,
                evidence_digest,
                recorded_at_ns,
                ..
            } => self.recover(*plan_id, *recovery_epoch, *evidence_digest, *recorded_at_ns),
            RolloutCommand::Finalize {
                plan_id,
                report_id,
                recorded_at_ns,
                ..
            } => self.finalize(*plan_id, *report_id, *recorded_at_ns),
        }
    }

    fn register_plan(&mut self, plan: &RolloutPlan, at: i64) -> Result<RolloutDetail, Error> {
        if self.plan.is_some() {
            return Err(Error::Plan);
        }
        validate_plan(plan, &self.policy, at)?;
        self.plan = Some(plan.clone());
        self.mode = Some(RolloutMode::Registered);
        Ok(RolloutDetail::PlanRegistered)
    }

    fn observe_health(
        &mut self,
        plan_id: [u8; 32],
        frame: &HealthFrame,
        at: i64,
    ) -> Result<RolloutDetail, Error> {
        self.require_plan(plan_id)?;
        if self.is_terminal() {
            return Err(Error::Lifecycle);
        }
        let expected = match self.latest_health.as_ref() {
            Some(previous) => previous.sequence.checked_add(1).ok_or(Error::Overflow)?,
            None => 1,
        };
        if frame.sequence != expected
            || frame.observed_at_ns != at
            || frame.observed_at_ns < 0
            || frame.valid_until_ns < at
            || frame.valid_until_ns - at > self.policy.maximum_health_age_ns
            || frame.source_digest == [0; 32]
            || frame.unreconciled_age_ns < 0
            || frame.unknown_state_age_ns < 0
            || frame.session_loss_micros < 0
            || !frame.verify_digest()
        {
            return Err(Error::Health);
        }
        self.health_frame_count = self
            .health_frame_count
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        self.latest_health = Some(frame.clone());
        let trigger = self.severe_trigger(frame);
        if let Some(trigger) = trigger {
            self.latch_rollback(trigger);
        } else if !stack_healthy(frame) && self.mode == Some(RolloutMode::Running) {
            self.pause(PauseReason::StackUnhealthy)?;
        }
        Ok(RolloutDetail::HealthObserved {
            healthy: stack_healthy(frame),
            rollback_trigger: trigger,
        })
    }

    fn start(&mut self, plan_id: [u8; 32], at: i64) -> Result<RolloutDetail, Error> {
        self.require_mode(plan_id, RolloutMode::Registered)?;
        self.require_gate(at)?;
        let plan = self.plan.as_ref().ok_or(Error::Plan)?;
        let stage = plan.stages.first().ok_or(Error::Plan)?;
        self.mode = Some(RolloutMode::Running);
        self.stage_index = Some(0);
        self.started_at_ns = Some(at);
        self.stage_started_at_ns = Some(at);
        Ok(RolloutDetail::Started {
            stage_index: 0,
            target_bps: stage.target_bps,
        })
    }

    fn advance(&mut self, plan_id: [u8; 32], at: i64) -> Result<RolloutDetail, Error> {
        self.require_mode(plan_id, RolloutMode::Running)?;
        if let Some(trigger) = self.timeout_trigger(at)? {
            self.latch_rollback(trigger);
            return Ok(RolloutDetail::RollbackLatched(trigger));
        }
        self.require_gate(at)?;
        let plan = self.plan.as_ref().ok_or(Error::Plan)?;
        let index = self.stage_index.ok_or(Error::Lifecycle)?;
        let stage = plan.stages.get(index).ok_or(Error::Lifecycle)?;
        let stage_start = self.stage_started_at_ns.ok_or(Error::Lifecycle)?;
        if at - stage_start < stage.minimum_observation_ns {
            return Err(Error::GateClosed);
        }
        if index + 1 == plan.stages.len() {
            self.mode = Some(RolloutMode::Completed);
            return Ok(RolloutDetail::Completed);
        }
        let next_index = index.checked_add(1).ok_or(Error::Overflow)?;
        let target_bps = plan
            .stages
            .get(next_index)
            .ok_or(Error::Lifecycle)?
            .target_bps;
        self.stage_index = Some(next_index);
        self.stage_started_at_ns = Some(at);
        Ok(RolloutDetail::Advanced {
            stage_index: next_index,
            target_bps,
        })
    }

    fn tick(&mut self, plan_id: [u8; 32], at: i64) -> Result<RolloutDetail, Error> {
        self.require_plan(plan_id)?;
        if self.is_terminal() {
            return Err(Error::Lifecycle);
        }
        if let Some(trigger) = self.timeout_trigger(at)? {
            self.latch_rollback(trigger);
            return Ok(RolloutDetail::RollbackLatched(trigger));
        }
        if self.mode == Some(RolloutMode::Running) {
            let health_current = self
                .latest_health
                .as_ref()
                .is_some_and(|frame| frame.observed_at_ns <= at && at <= frame.valid_until_ns);
            if !health_current {
                self.pause(PauseReason::HealthStale)?;
                return Ok(RolloutDetail::Paused(PauseReason::HealthStale));
            }
            if !self.within_window(at)? {
                self.pause(PauseReason::OutsideMaintenanceWindow)?;
                return Ok(RolloutDetail::Paused(PauseReason::OutsideMaintenanceWindow));
            }
        }
        Ok(RolloutDetail::TickApplied)
    }

    fn operator_pause(
        &mut self,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        reason_digest: [u8; 32],
    ) -> Result<RolloutDetail, Error> {
        self.require_mode(plan_id, RolloutMode::Running)?;
        validate_operator(operator_id, reason_digest)?;
        self.pause(PauseReason::Operator)?;
        Ok(RolloutDetail::Paused(PauseReason::Operator))
    }

    fn operator_resume(
        &mut self,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        at: i64,
    ) -> Result<RolloutDetail, Error> {
        self.require_mode(plan_id, RolloutMode::Paused)?;
        if operator_id == [0; 32] {
            return Err(Error::Operator);
        }
        if let Some(trigger) = self.timeout_trigger(at)? {
            self.latch_rollback(trigger);
            return Ok(RolloutDetail::RollbackLatched(trigger));
        }
        self.require_gate(at)?;
        self.mode = Some(RolloutMode::Running);
        self.pause_reason = None;
        Ok(RolloutDetail::Resumed)
    }

    fn operator_abort(
        &mut self,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        reason_digest: [u8; 32],
    ) -> Result<RolloutDetail, Error> {
        self.require_plan(plan_id)?;
        if self.is_terminal() {
            return Err(Error::Lifecycle);
        }
        validate_operator(operator_id, reason_digest)?;
        self.mode = Some(RolloutMode::Aborted);
        self.pause_reason = None;
        self.abort_operator_id = Some(operator_id);
        Ok(RolloutDetail::Aborted)
    }

    fn restart(
        &mut self,
        plan_id: [u8; 32],
        restart_id: [u8; 32],
        at: i64,
    ) -> Result<RolloutDetail, Error> {
        self.require_plan(plan_id)?;
        if self.is_terminal()
            || self.mode == Some(RolloutMode::Recovering)
            || restart_id == [0; 32]
            || self.stage_index.is_none()
        {
            return Err(Error::Lifecycle);
        }
        self.restart_count = self.restart_count.checked_add(1).ok_or(Error::Overflow)?;
        self.restart_at_ns = Some(at);
        self.mode = Some(RolloutMode::Recovering);
        self.pause_reason = None;
        Ok(RolloutDetail::Restarted)
    }

    fn recover(
        &mut self,
        plan_id: [u8; 32],
        recovery_epoch: u64,
        evidence_digest: [u8; 32],
        at: i64,
    ) -> Result<RolloutDetail, Error> {
        self.require_mode(plan_id, RolloutMode::Recovering)?;
        let expected = self.recovery_epoch.checked_add(1).ok_or(Error::Overflow)?;
        let restart_at = self.restart_at_ns.ok_or(Error::Recovery)?;
        let health = self.latest_health.as_ref().ok_or(Error::Recovery)?;
        if recovery_epoch != expected
            || evidence_digest == [0; 32]
            || health.observed_at_ns < restart_at
            || !self.health_gate_at(health, at)
        {
            return Err(Error::Recovery);
        }
        if let Some(trigger) = self.timeout_trigger(at)? {
            self.latch_rollback(trigger);
            return Ok(RolloutDetail::RollbackLatched(trigger));
        }
        self.recovery_epoch = recovery_epoch;
        self.restart_at_ns = None;
        self.mode = Some(RolloutMode::Paused);
        self.pause_reason = Some(PauseReason::RestartRecovery);
        self.pause_count = self.pause_count.checked_add(1).ok_or(Error::Overflow)?;
        Ok(RolloutDetail::Recovered { recovery_epoch })
    }

    fn finalize(
        &mut self,
        plan_id: [u8; 32],
        report_id: [u8; 32],
        at: i64,
    ) -> Result<RolloutDetail, Error> {
        self.require_plan(plan_id)?;
        let mode = self.mode.ok_or(Error::Lifecycle)?;
        let status = match mode {
            RolloutMode::Completed => RolloutReportStatus::SimulatedCompleted,
            RolloutMode::Aborted => RolloutReportStatus::OperatorAborted,
            RolloutMode::RollbackRequired => RolloutReportStatus::RollbackRequired,
            _ => return Err(Error::Report),
        };
        if report_id == [0; 32] {
            return Err(Error::Report);
        }
        let plan = self.plan.as_ref().ok_or(Error::Plan)?;
        let completed_stage_count = match status {
            RolloutReportStatus::SimulatedCompleted => plan.stages.len(),
            _ => self.stage_index.unwrap_or(0),
        };
        let final_target_bps = self
            .stage_index
            .and_then(|index| plan.stages.get(index))
            .map_or(0, |stage| stage.target_bps);
        let mut report = RolloutReport {
            report_id,
            plan_id,
            plan_digest: plan.plan_digest,
            eligibility_record_digest: plan.eligibility.record_digest,
            artifacts_digest: plan.eligibility.artifacts_digest,
            rollback_digest: plan.rollback.criteria_digest,
            finalized_at_ns: at,
            status,
            completed_stage_count,
            final_stage_index: self.stage_index,
            final_target_bps,
            health_frame_count: self.health_frame_count,
            pause_count: self.pause_count,
            restart_count: self.restart_count,
            recovery_epoch: self.recovery_epoch,
            rollback_trigger: self.rollback_trigger,
            abort_operator_id: self.abort_operator_id,
            operator_execution_required: true,
            rollout_execution_authority_granted: false,
            rollback_execution_authority_granted: false,
            deployment_authority_granted: false,
            credential_authority_granted: false,
            live_trading_authority_granted: false,
            report_digest: [0; 32],
        };
        report.report_digest = rollout_report_digest(&report);
        self.report = Some(report.clone());
        Ok(RolloutDetail::Finalized(Box::new(report)))
    }

    fn require_plan(&self, plan_id: [u8; 32]) -> Result<(), Error> {
        if self.plan.as_ref().map(|plan| plan.plan_id) == Some(plan_id) {
            Ok(())
        } else {
            Err(Error::Plan)
        }
    }

    fn require_mode(&self, plan_id: [u8; 32], mode: RolloutMode) -> Result<(), Error> {
        self.require_plan(plan_id)?;
        if self.mode == Some(mode) {
            Ok(())
        } else {
            Err(Error::Lifecycle)
        }
    }

    fn require_gate(&self, at: i64) -> Result<(), Error> {
        let plan = self.plan.as_ref().ok_or(Error::Plan)?;
        let health = self.latest_health.as_ref().ok_or(Error::GateClosed)?;
        if self.rollback_trigger.is_some()
            || at < plan.scheduled_start_ns
            || at > plan.scheduled_end_ns
            || at > plan.eligibility.valid_until_ns
            || !self.within_window(at)?
            || !self.health_gate_at(health, at)
        {
            Err(Error::GateClosed)
        } else {
            Ok(())
        }
    }

    fn health_gate_at(&self, health: &HealthFrame, at: i64) -> bool {
        health.observed_at_ns <= at
            && at <= health.valid_until_ns
            && stack_healthy(health)
            && self.severe_trigger(health).is_none()
    }

    fn within_window(&self, at: i64) -> Result<bool, Error> {
        let plan = self.plan.as_ref().ok_or(Error::Plan)?;
        Ok(plan
            .windows
            .iter()
            .any(|window| window.start_ns <= at && at < window.end_ns))
    }

    fn severe_trigger(&self, frame: &HealthFrame) -> Option<RollbackTrigger> {
        let rollback = &self.plan.as_ref()?.rollback;
        if !frame.capital_floor_preserved {
            Some(RollbackTrigger::CapitalFloorBreach)
        } else if frame.unreconciled_age_ns > rollback.maximum_unreconciled_ns {
            Some(RollbackTrigger::ReconciliationTimeout)
        } else if frame.unknown_state_age_ns > rollback.maximum_unknown_state_ns {
            Some(RollbackTrigger::UnknownStateTimeout)
        } else if frame.session_loss_micros > rollback.maximum_session_loss_micros {
            Some(RollbackTrigger::SessionLossLimit)
        } else if frame.consecutive_faults >= rollback.maximum_consecutive_faults {
            Some(RollbackTrigger::ConsecutiveFaultLimit)
        } else {
            None
        }
    }

    fn timeout_trigger(&self, at: i64) -> Result<Option<RollbackTrigger>, Error> {
        let plan = self.plan.as_ref().ok_or(Error::Plan)?;
        if at > plan.scheduled_end_ns
            || self
                .started_at_ns
                .is_some_and(|started| at - started > plan.rollback.maximum_canary_duration_ns)
        {
            return Ok(Some(RollbackTrigger::PlanTimeout));
        }
        if let (Some(index), Some(started)) = (self.stage_index, self.stage_started_at_ns) {
            let stage = plan.stages.get(index).ok_or(Error::Lifecycle)?;
            if at - started > stage.maximum_stage_ns {
                return Ok(Some(RollbackTrigger::StageTimeout));
            }
        }
        Ok(None)
    }

    fn pause(&mut self, reason: PauseReason) -> Result<(), Error> {
        self.mode = Some(RolloutMode::Paused);
        self.pause_reason = Some(reason);
        self.pause_count = self.pause_count.checked_add(1).ok_or(Error::Overflow)?;
        Ok(())
    }

    fn latch_rollback(&mut self, trigger: RollbackTrigger) {
        if self.rollback_trigger.is_none() {
            self.rollback_trigger = Some(trigger);
        }
        self.mode = Some(RolloutMode::RollbackRequired);
        self.pause_reason = None;
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self.mode,
            Some(RolloutMode::Completed | RolloutMode::Aborted | RolloutMode::RollbackRequired)
        )
    }

    #[must_use]
    pub fn snapshot(&self) -> RolloutSnapshot {
        RolloutSnapshot {
            accepted_commands: self.accepted_commands,
            plan_id: self.plan.as_ref().map(|plan| plan.plan_id),
            mode: self.mode,
            pause_reason: self.pause_reason,
            stage_index: self.stage_index,
            latest_health: self.latest_health.clone(),
            rollback_trigger: self.rollback_trigger,
            recovery_epoch: self.recovery_epoch,
            last_report: self.report.clone(),
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
        hasher.update(b"canary-rollout-simulator-state-v1");
        hash_json(&mut hasher, &self.policy);
        hash_json(&mut hasher, &self.plan);
        hash_json(&mut hasher, &self.mode);
        hash_json(&mut hasher, &self.pause_reason);
        hash_json(&mut hasher, &self.stage_index);
        hash_json(&mut hasher, &self.started_at_ns);
        hash_json(&mut hasher, &self.stage_started_at_ns);
        hash_json(&mut hasher, &self.latest_health);
        hash_json(&mut hasher, &self.health_frame_count);
        hash_json(&mut hasher, &self.pause_count);
        hash_json(&mut hasher, &self.restart_count);
        hash_json(&mut hasher, &self.restart_at_ns);
        hash_json(&mut hasher, &self.recovery_epoch);
        hash_json(&mut hasher, &self.rollback_trigger);
        hash_json(&mut hasher, &self.abort_operator_id);
        hash_json(&mut hasher, &self.report);
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

fn validate_policy(policy: &SimulatorPolicy) -> Result<(), Error> {
    if policy.maximum_windows == 0
        || policy.maximum_windows > MAX_WINDOWS_HARD
        || policy.maximum_stages == 0
        || policy.maximum_stages > MAX_STAGES_HARD
        || policy.maximum_health_age_ns <= 0
        || policy.maximum_plan_age_ns <= 0
        || policy.maximum_target_bps == 0
        || policy.maximum_target_bps > BASIS_POINTS
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn validate_plan(plan: &RolloutPlan, policy: &SimulatorPolicy, at: i64) -> Result<(), Error> {
    if plan.plan_id == [0; 32]
        || plan.created_at_ns != at
        || plan.scheduled_start_ns < plan.created_at_ns
        || plan.scheduled_end_ns <= plan.scheduled_start_ns
        || plan.scheduled_end_ns - plan.created_at_ns > policy.maximum_plan_age_ns
        || plan.windows.is_empty()
        || plan.windows.len() > policy.maximum_windows
        || plan.stages.is_empty()
        || plan.stages.len() > policy.maximum_stages
        || !plan.verify_digest(policy)
    {
        return Err(Error::Plan);
    }
    validate_eligibility(&plan.eligibility, plan.created_at_ns)?;
    if !plan.rollback.verify_digest()
        || plan.rollback.criteria_id == [0; 32]
        || plan.rollback.rollback_target_digest == [0; 32]
        || plan.rollback.maximum_canary_duration_ns <= 0
        || plan.rollback.maximum_unreconciled_ns <= 0
        || plan.rollback.maximum_unknown_state_ns <= 0
        || plan.rollback.maximum_session_loss_micros < 0
        || plan.rollback.maximum_consecutive_faults == 0
        || !plan.rollback.require_capital_floor_halt
        || !plan.rollback.require_reconciliation_halt
        || plan.rollback.criteria_digest != plan.eligibility.rollback_digest
        || plan.scheduled_end_ns - plan.scheduled_start_ns
            > plan.rollback.maximum_canary_duration_ns
    {
        return Err(Error::Rollback);
    }
    if plan.scheduled_end_ns > plan.eligibility.valid_until_ns {
        return Err(Error::Eligibility);
    }
    validate_windows(plan)?;
    validate_stages(plan, policy)?;
    Ok(())
}

fn validate_eligibility(record: &CanaryEligibilityRecord, at: i64) -> Result<(), Error> {
    let bound_digests = [
        record.record_id,
        record.candidate_id,
        record.candidate_digest,
        record.evidence_set_digest,
        record.baseline_digest,
        record.artifacts_digest,
        record.rollback_digest,
        record.policy_digest,
    ];
    if bound_digests.contains(&[0; 32])
        || !record.verify_digest()
        || record.status != CanaryStatus::CanaryEligible
        || !record.reasons.is_empty()
        || !record.dual_control_complete
        || !record.operator_execution_required
        || !record.rollback_required_on_threshold
        || record.canary_execution_authority_granted
        || record.promotion_authority_granted
        || record.deployment_authority_granted
        || record.credential_authority_granted
        || record.live_trading_authority_granted
        || record.evaluated_at_ns > at
        || at > record.valid_until_ns
    {
        Err(Error::Eligibility)
    } else {
        Ok(())
    }
}

fn validate_windows(plan: &RolloutPlan) -> Result<(), Error> {
    let ids: BTreeSet<_> = plan.windows.iter().map(|window| window.window_id).collect();
    let ordered = plan
        .windows
        .windows(2)
        .all(|pair| pair[0].end_ns <= pair[1].start_ns);
    if !ordered
        || ids.len() != plan.windows.len()
        || plan.windows.iter().any(|window| {
            window.window_id == [0; 32]
                || window.start_ns < plan.scheduled_start_ns
                || window.end_ns > plan.scheduled_end_ns
                || window.end_ns <= window.start_ns
        })
    {
        Err(Error::Plan)
    } else {
        Ok(())
    }
}

fn validate_stages(plan: &RolloutPlan, policy: &SimulatorPolicy) -> Result<(), Error> {
    let ids: BTreeSet<_> = plan.stages.iter().map(|stage| stage.stage_id).collect();
    let strictly_increasing = plan
        .stages
        .windows(2)
        .all(|pair| pair[0].target_bps < pair[1].target_bps);
    let mut minimum_total = 0_i64;
    for stage in &plan.stages {
        if stage.stage_id == [0; 32]
            || stage.target_bps == 0
            || stage.target_bps > policy.maximum_target_bps
            || stage.minimum_observation_ns <= 0
            || stage.maximum_stage_ns < stage.minimum_observation_ns
            || stage.maximum_stage_ns > plan.rollback.maximum_canary_duration_ns
        {
            return Err(Error::Plan);
        }
        minimum_total = minimum_total
            .checked_add(stage.minimum_observation_ns)
            .ok_or(Error::Overflow)?;
    }
    if !strictly_increasing
        || ids.len() != plan.stages.len()
        || minimum_total > plan.scheduled_end_ns - plan.scheduled_start_ns
    {
        Err(Error::Plan)
    } else {
        Ok(())
    }
}

fn validate_operator(operator_id: [u8; 32], reason_digest: [u8; 32]) -> Result<(), Error> {
    if operator_id == [0; 32] || reason_digest == [0; 32] {
        Err(Error::Operator)
    } else {
        Ok(())
    }
}

fn stack_healthy(frame: &HealthFrame) -> bool {
    frame.strategy_healthy
        && frame.risk_healthy
        && frame.market_feed_healthy
        && frame.user_feed_healthy
        && frame.reconciliation_healthy
        && frame.capital_floor_preserved
}

fn rollout_plan_digest(value: &RolloutPlan) -> [u8; 32] {
    let mut clone = value.clone();
    clone.plan_digest = [0; 32];
    digest_json(b"canary-rollout-plan-v1", &clone)
}

fn health_frame_digest(value: &HealthFrame) -> [u8; 32] {
    let mut clone = value.clone();
    clone.frame_digest = [0; 32];
    digest_json(b"canary-rollout-health-v1", &clone)
}

fn rollout_report_digest(value: &RolloutReport) -> [u8; 32] {
    let mut clone = value.clone();
    clone.report_digest = [0; 32];
    digest_json(b"canary-rollout-report-v1", &clone)
}

fn rollout_outcome_digest(value: &RolloutOutcome) -> [u8; 32] {
    let mut clone = value.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"canary-rollout-outcome-v1", &clone)
}

fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable rollout state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: RolloutCommand,
}

/// Encodes one bounded versioned rollout command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &RolloutCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one bounded versioned rollout command.
///
/// # Errors
///
/// Rejects malformed, oversized, trailing, or unsupported input.
pub fn decode_command(bytes: &[u8]) -> Result<RolloutCommand, Error> {
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

#[cfg(test)]
mod tests;
