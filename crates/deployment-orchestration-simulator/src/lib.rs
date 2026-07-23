#![forbid(unsafe_code)]

//! Deterministic offline regional deployment and rollback orchestration.
//!
//! No command in this crate contacts or authorizes a real control plane.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableOrchestrator,
    OrchestrationCheckpoint, OrchestrationRecovery, OrchestrationStorageError,
};
pub use report::{read_report, write_report_create_new, OrchestrationReportFileError};

use deployment_preflight::{DeploymentPreflightReport, PreflightStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_WAVES_HARD: usize = 128;
const MAX_REGIONS_HARD: usize = 128;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct OrchestrationCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationPolicy {
    pub maximum_waves: usize,
    pub maximum_regions: usize,
    pub maximum_preflight_age_ns: i64,
    pub maximum_plan_age_ns: i64,
    pub maximum_health_age_ns: i64,
    pub maximum_wave_duration_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentWave {
    pub wave_id: [u8; 32],
    pub regions: Vec<String>,
    pub minimum_observation_ns: i64,
    pub maximum_duration_ns: i64,
    pub wave_digest: [u8; 32],
}

impl DeploymentWave {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.regions.sort();
        self.wave_digest = wave_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.wave_digest == wave_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationPlan {
    pub plan_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub preflight: DeploymentPreflightReport,
    pub waves: Vec<DeploymentWave>,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl OrchestrationPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &OrchestrationPolicy) -> Self {
        self.policy_digest = digest_json(b"deployment-orchestration-policy-v1", policy);
        self.plan_digest = plan_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &OrchestrationPolicy) -> bool {
        self.policy_digest == digest_json(b"deployment-orchestration-policy-v1", policy)
            && self.plan_digest == plan_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RegionHealth {
    pub region: String,
    pub package_applied: bool,
    pub service_healthy: bool,
    pub risk_healthy: bool,
    pub reconciliation_healthy: bool,
    pub capital_floor_preserved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionalHealthFrame {
    pub sequence: u64,
    pub observed_at_ns: i64,
    pub valid_until_ns: i64,
    pub regions: Vec<RegionHealth>,
    pub source_digest: [u8; 32],
    pub frame_digest: [u8; 32],
}

impl RegionalHealthFrame {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.regions
            .sort_by(|left, right| left.region.cmp(&right.region));
        self.frame_digest = health_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.frame_digest == health_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackObservation {
    pub observation_id: [u8; 32],
    pub plan_id: [u8; 32],
    pub region: String,
    pub rollback_package_digest: [u8; 32],
    pub baseline_restored: bool,
    pub observed_at_ns: i64,
    pub source_digest: [u8; 32],
    pub observation_digest: [u8; 32],
}

impl RollbackObservation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.observation_digest = rollback_observation_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.observation_digest == rollback_observation_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationMode {
    Registered,
    Running,
    Paused,
    Recovering,
    RollbackRequired,
    RolledBack,
    Completed,
    Aborted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackTrigger {
    ReconciliationFailure,
    CapitalFloorBreach,
    WaveTimeout,
    PlanTimeout,
    OperatorAbort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    HealthDegraded,
    HealthStale,
    Operator,
    RestartRecovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationReportStatus {
    SimulatedCompleted,
    SimulatedRolledBack,
    SimulatedAborted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct OrchestrationReport {
    pub report_id: [u8; 32],
    pub plan_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub preflight_report_digest: [u8; 32],
    pub rollback_package_digest: [u8; 32],
    pub finalized_at_ns: i64,
    pub status: OrchestrationReportStatus,
    pub completed_wave_count: usize,
    pub activated_regions: Vec<String>,
    pub rolled_back_regions: Vec<String>,
    pub rollback_trigger: Option<RollbackTrigger>,
    pub pause_count: u64,
    pub restart_count: u64,
    pub recovery_epoch: u64,
    pub manual_operator_execution_required: bool,
    pub credential_material_created: bool,
    pub deployment_authority_granted: bool,
    pub rollback_execution_authority_granted: bool,
    pub cloud_control_authority_granted: bool,
    pub live_trading_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl OrchestrationReport {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest == orchestration_report_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum OrchestrationCommand {
    Register {
        command_id: OrchestrationCommandId,
        plan: Box<OrchestrationPlan>,
        recorded_at_ns: i64,
    },
    ObserveHealth {
        command_id: OrchestrationCommandId,
        plan_id: [u8; 32],
        frame: RegionalHealthFrame,
        recorded_at_ns: i64,
    },
    Start {
        command_id: OrchestrationCommandId,
        plan_id: [u8; 32],
        recorded_at_ns: i64,
    },
    Advance {
        command_id: OrchestrationCommandId,
        plan_id: [u8; 32],
        recorded_at_ns: i64,
    },
    OperatorPause {
        command_id: OrchestrationCommandId,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        reason_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    OperatorResume {
        command_id: OrchestrationCommandId,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        recorded_at_ns: i64,
    },
    OperatorAbort {
        command_id: OrchestrationCommandId,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        reason_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    Tick {
        command_id: OrchestrationCommandId,
        plan_id: [u8; 32],
        recorded_at_ns: i64,
    },
    Restart {
        command_id: OrchestrationCommandId,
        plan_id: [u8; 32],
        recorded_at_ns: i64,
    },
    Recover {
        command_id: OrchestrationCommandId,
        plan_id: [u8; 32],
        recovery_epoch: u64,
        evidence_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    ObserveRollback {
        command_id: OrchestrationCommandId,
        observation: RollbackObservation,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: OrchestrationCommandId,
        plan_id: [u8; 32],
        report_id: [u8; 32],
        recorded_at_ns: i64,
    },
}

impl OrchestrationCommand {
    #[must_use]
    pub const fn command_id(&self) -> OrchestrationCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::ObserveHealth { command_id, .. }
            | Self::Start { command_id, .. }
            | Self::Advance { command_id, .. }
            | Self::OperatorPause { command_id, .. }
            | Self::OperatorResume { command_id, .. }
            | Self::OperatorAbort { command_id, .. }
            | Self::Tick { command_id, .. }
            | Self::Restart { command_id, .. }
            | Self::Recover { command_id, .. }
            | Self::ObserveRollback { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::ObserveHealth { recorded_at_ns, .. }
            | Self::Start { recorded_at_ns, .. }
            | Self::Advance { recorded_at_ns, .. }
            | Self::OperatorPause { recorded_at_ns, .. }
            | Self::OperatorResume { recorded_at_ns, .. }
            | Self::OperatorAbort { recorded_at_ns, .. }
            | Self::Tick { recorded_at_ns, .. }
            | Self::Restart { recorded_at_ns, .. }
            | Self::Recover { recorded_at_ns, .. }
            | Self::ObserveRollback { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum OrchestrationDetail {
    Registered,
    HealthObserved,
    Started { wave_index: usize },
    Advanced { wave_index: usize },
    Paused(PauseReason),
    Resumed,
    Aborted,
    RollbackRequired(RollbackTrigger),
    Restarted,
    Recovered(OrchestrationMode),
    RollbackConverged { region: String, complete: bool },
    Finalized(Box<OrchestrationReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationOutcome {
    pub command_id: OrchestrationCommandId,
    pub detail: OrchestrationDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrchestrationSnapshot {
    pub accepted_commands: u64,
    pub plan_id: Option<[u8; 32]>,
    pub mode: Option<OrchestrationMode>,
    pub wave_index: Option<usize>,
    pub activated_regions: Vec<String>,
    pub rolled_back_regions: Vec<String>,
    pub rollback_trigger: Option<RollbackTrigger>,
    pub last_report: Option<OrchestrationReport>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("deployment-orchestration configuration is invalid")]
    Config,
    #[error("deployment-orchestration timestamp is invalid or regressed")]
    Timestamp,
    #[error("deployment-orchestration command exceeds its canonical bound")]
    CommandBound,
    #[error("deployment-orchestration JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported deployment-orchestration version: {0}")]
    Version(u16),
    #[error("orchestration command id was reused for different content")]
    IdempotencyConflict,
    #[error("orchestration plan or preflight subject is invalid")]
    Plan,
    #[error("regional health frame is invalid, stale or mismatched")]
    Health,
    #[error("orchestration lifecycle gate is closed")]
    GateClosed,
    #[error("rollback observation is invalid, out of order or mismatched")]
    Rollback,
    #[error("restart recovery evidence or epoch is invalid")]
    Recovery,
    #[error("orchestration report lifecycle is invalid")]
    Report,
    #[error("orchestration is already finalized")]
    Finalized,
    #[error("deployment-orchestration arithmetic overflow")]
    Overflow,
    #[error("deployment orchestration is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct DeploymentOrchestrator {
    policy: OrchestrationPolicy,
    plan: Option<OrchestrationPlan>,
    mode: Option<OrchestrationMode>,
    wave_index: Option<usize>,
    activated_regions: Vec<String>,
    rolled_back_regions: Vec<String>,
    rollback_observation_ids: BTreeSet<[u8; 32]>,
    last_health: Option<RegionalHealthFrame>,
    last_health_sequence: Option<u64>,
    wave_started_at_ns: Option<i64>,
    rollback_trigger: Option<RollbackTrigger>,
    pause_count: u64,
    restart_count: u64,
    recovery_epoch: u64,
    pre_restart_mode: Option<OrchestrationMode>,
    report: Option<OrchestrationReport>,
    processed: BTreeMap<OrchestrationCommandId, ([u8; 32], OrchestrationOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl DeploymentOrchestrator {
    /// Creates one empty offline orchestration owner.
    ///
    /// # Errors
    ///
    /// Rejects invalid wave, region, age, or duration bounds.
    pub fn new(policy: OrchestrationPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            mode: None,
            wave_index: None,
            activated_regions: Vec::new(),
            rolled_back_regions: Vec::new(),
            rollback_observation_ids: BTreeSet::new(),
            last_health: None,
            last_health_sequence: None,
            wave_started_at_ns: None,
            rollback_trigger: None,
            pause_count: 0,
            restart_count: 0,
            recovery_epoch: 0,
            pre_restart_mode: None,
            report: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic orchestration command.
    ///
    /// # Errors
    ///
    /// Integrity, chronology, lifecycle, recovery, or arithmetic failures halt.
    pub fn apply(&mut self, command: &OrchestrationCommand) -> Result<OrchestrationOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        if command.recorded_at_ns() < 0
            || self
                .last_recorded_at_ns
                .is_some_and(|last| command.recorded_at_ns() < last)
        {
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
        let mut next = self.clone();
        let detail = match next.apply_fresh(command) {
            Ok(detail) => detail,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = next
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        let mut outcome = OrchestrationOutcome {
            command_id: id,
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = outcome_digest(&outcome);
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.processed.insert(id, (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    fn apply_fresh(
        &mut self,
        command: &OrchestrationCommand,
    ) -> Result<OrchestrationDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalized);
        }
        match command {
            OrchestrationCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some() {
                    return Err(Error::Plan);
                }
                validate_plan(plan, &self.policy, *recorded_at_ns)?;
                self.plan = Some((**plan).clone());
                self.mode = Some(OrchestrationMode::Registered);
                Ok(OrchestrationDetail::Registered)
            }
            OrchestrationCommand::ObserveHealth {
                plan_id,
                frame,
                recorded_at_ns,
                ..
            } => self.observe_health(*plan_id, frame, *recorded_at_ns),
            OrchestrationCommand::Start {
                plan_id,
                recorded_at_ns,
                ..
            } => self.start(*plan_id, *recorded_at_ns),
            OrchestrationCommand::Advance {
                plan_id,
                recorded_at_ns,
                ..
            } => self.advance(*plan_id, *recorded_at_ns),
            OrchestrationCommand::OperatorPause {
                plan_id,
                operator_id,
                reason_digest,
                ..
            } => self.operator_pause(*plan_id, *operator_id, *reason_digest),
            OrchestrationCommand::OperatorResume {
                plan_id,
                operator_id,
                recorded_at_ns,
                ..
            } => self.resume(*plan_id, *operator_id, *recorded_at_ns),
            OrchestrationCommand::OperatorAbort {
                plan_id,
                operator_id,
                reason_digest,
                ..
            } => self.abort(*plan_id, *operator_id, *reason_digest),
            OrchestrationCommand::Tick {
                plan_id,
                recorded_at_ns,
                ..
            } => self.tick(*plan_id, *recorded_at_ns),
            OrchestrationCommand::Restart { plan_id, .. } => self.restart(*plan_id),
            OrchestrationCommand::Recover {
                plan_id,
                recovery_epoch,
                evidence_digest,
                recorded_at_ns,
                ..
            } => self.recover(*plan_id, *recovery_epoch, *evidence_digest, *recorded_at_ns),
            OrchestrationCommand::ObserveRollback {
                observation,
                recorded_at_ns,
                ..
            } => self.observe_rollback(observation, *recorded_at_ns),
            OrchestrationCommand::Finalize {
                plan_id,
                report_id,
                recorded_at_ns,
                ..
            } => self.finalize(*plan_id, *report_id, *recorded_at_ns),
        }
    }

    fn observe_health(
        &mut self,
        plan_id: [u8; 32],
        frame: &RegionalHealthFrame,
        at: i64,
    ) -> Result<OrchestrationDetail, Error> {
        let plan = self.require_plan(plan_id)?;
        validate_health_frame(frame, plan, self.last_health_sequence, at)?;
        self.last_health = Some(frame.clone());
        self.last_health_sequence = Some(frame.sequence);
        if self.mode == Some(OrchestrationMode::Running) {
            if let Some(trigger) = severe_trigger(frame, &self.activated_regions) {
                return self.latch_rollback(trigger);
            }
            if !regions_healthy(frame, &self.activated_regions) {
                self.mode = Some(OrchestrationMode::Paused);
                self.pause_count = self.pause_count.checked_add(1).ok_or(Error::Overflow)?;
                return Ok(OrchestrationDetail::Paused(PauseReason::HealthDegraded));
            }
        }
        Ok(OrchestrationDetail::HealthObserved)
    }

    fn start(&mut self, plan_id: [u8; 32], at: i64) -> Result<OrchestrationDetail, Error> {
        let plan = self.require_plan(plan_id)?;
        if self.mode != Some(OrchestrationMode::Registered) || !plan_current(plan, at) {
            return Err(Error::GateClosed);
        }
        let first_regions = plan.waves.first().ok_or(Error::Plan)?.regions.clone();
        self.require_current_health(&first_regions, at)?;
        self.activated_regions.extend(first_regions);
        self.wave_index = Some(0);
        self.wave_started_at_ns = Some(at);
        self.mode = Some(OrchestrationMode::Running);
        Ok(OrchestrationDetail::Started { wave_index: 0 })
    }

    fn advance(&mut self, plan_id: [u8; 32], at: i64) -> Result<OrchestrationDetail, Error> {
        let plan = self.require_plan(plan_id)?.clone();
        if self.mode != Some(OrchestrationMode::Running) {
            return Err(Error::GateClosed);
        }
        if !plan_current(&plan, at) {
            return self.latch_rollback(RollbackTrigger::PlanTimeout);
        }
        let index = self.wave_index.ok_or(Error::GateClosed)?;
        let wave = plan.waves.get(index).ok_or(Error::Plan)?;
        let started = self.wave_started_at_ns.ok_or(Error::GateClosed)?;
        let elapsed = at.checked_sub(started).ok_or(Error::Overflow)?;
        if elapsed > wave.maximum_duration_ns {
            return self.latch_rollback(RollbackTrigger::WaveTimeout);
        }
        if elapsed < wave.minimum_observation_ns {
            return Err(Error::GateClosed);
        }
        self.require_current_health(&self.activated_regions.clone(), at)?;
        let next = index.checked_add(1).ok_or(Error::Overflow)?;
        if let Some(next_wave) = plan.waves.get(next) {
            self.require_current_health(&next_wave.regions, at)?;
            self.activated_regions.extend(next_wave.regions.clone());
            self.wave_index = Some(next);
            self.wave_started_at_ns = Some(at);
            Ok(OrchestrationDetail::Advanced { wave_index: next })
        } else {
            self.mode = Some(OrchestrationMode::Completed);
            Ok(OrchestrationDetail::Advanced { wave_index: index })
        }
    }

    fn operator_pause(
        &mut self,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        reason_digest: [u8; 32],
    ) -> Result<OrchestrationDetail, Error> {
        self.require_plan(plan_id)?;
        if self.mode != Some(OrchestrationMode::Running)
            || operator_id == [0; 32]
            || reason_digest == [0; 32]
        {
            return Err(Error::GateClosed);
        }
        self.mode = Some(OrchestrationMode::Paused);
        self.pause_count = self.pause_count.checked_add(1).ok_or(Error::Overflow)?;
        Ok(OrchestrationDetail::Paused(PauseReason::Operator))
    }

    fn resume(
        &mut self,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        at: i64,
    ) -> Result<OrchestrationDetail, Error> {
        let plan = self.require_plan(plan_id)?;
        if self.mode != Some(OrchestrationMode::Paused)
            || operator_id == [0; 32]
            || !plan_current(plan, at)
        {
            return Err(Error::GateClosed);
        }
        self.require_current_health(&self.activated_regions.clone(), at)?;
        self.mode = Some(OrchestrationMode::Running);
        Ok(OrchestrationDetail::Resumed)
    }

    fn abort(
        &mut self,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        reason_digest: [u8; 32],
    ) -> Result<OrchestrationDetail, Error> {
        self.require_plan(plan_id)?;
        if operator_id == [0; 32] || reason_digest == [0; 32] {
            return Err(Error::GateClosed);
        }
        match self.mode {
            Some(OrchestrationMode::Registered) => {
                self.mode = Some(OrchestrationMode::Aborted);
                Ok(OrchestrationDetail::Aborted)
            }
            Some(OrchestrationMode::Running | OrchestrationMode::Paused) => {
                self.latch_rollback(RollbackTrigger::OperatorAbort)
            }
            _ => Err(Error::GateClosed),
        }
    }

    fn tick(&mut self, plan_id: [u8; 32], at: i64) -> Result<OrchestrationDetail, Error> {
        let plan = self.require_plan(plan_id)?.clone();
        if self.mode != Some(OrchestrationMode::Running) {
            return Err(Error::GateClosed);
        }
        if at > plan.expires_at_ns || at > plan.preflight.package_expires_at_ns {
            return self.latch_rollback(RollbackTrigger::PlanTimeout);
        }
        let wave = plan
            .waves
            .get(self.wave_index.ok_or(Error::GateClosed)?)
            .ok_or(Error::Plan)?;
        if at - self.wave_started_at_ns.ok_or(Error::GateClosed)? > wave.maximum_duration_ns {
            return self.latch_rollback(RollbackTrigger::WaveTimeout);
        }
        if self.current_health(at).is_err() {
            self.mode = Some(OrchestrationMode::Paused);
            self.pause_count = self.pause_count.checked_add(1).ok_or(Error::Overflow)?;
            return Ok(OrchestrationDetail::Paused(PauseReason::HealthStale));
        }
        Ok(OrchestrationDetail::HealthObserved)
    }

    fn restart(&mut self, plan_id: [u8; 32]) -> Result<OrchestrationDetail, Error> {
        self.require_plan(plan_id)?;
        let mode = self.mode.ok_or(Error::GateClosed)?;
        if !matches!(
            mode,
            OrchestrationMode::Running
                | OrchestrationMode::Paused
                | OrchestrationMode::RollbackRequired
        ) {
            return Err(Error::GateClosed);
        }
        self.pre_restart_mode = Some(mode);
        self.mode = Some(OrchestrationMode::Recovering);
        self.restart_count = self.restart_count.checked_add(1).ok_or(Error::Overflow)?;
        Ok(OrchestrationDetail::Restarted)
    }

    fn recover(
        &mut self,
        plan_id: [u8; 32],
        epoch: u64,
        evidence: [u8; 32],
        at: i64,
    ) -> Result<OrchestrationDetail, Error> {
        self.require_plan(plan_id)?;
        if self.mode != Some(OrchestrationMode::Recovering)
            || epoch <= self.recovery_epoch
            || evidence == [0; 32]
        {
            return Err(Error::Recovery);
        }
        let target = self.pre_restart_mode.ok_or(Error::Recovery)?;
        let next = if target == OrchestrationMode::RollbackRequired {
            OrchestrationMode::RollbackRequired
        } else {
            self.require_current_health(&self.activated_regions.clone(), at)?;
            self.pause_count = self.pause_count.checked_add(1).ok_or(Error::Overflow)?;
            OrchestrationMode::Paused
        };
        self.recovery_epoch = epoch;
        self.pre_restart_mode = None;
        self.mode = Some(next);
        Ok(OrchestrationDetail::Recovered(next))
    }

    fn observe_rollback(
        &mut self,
        observation: &RollbackObservation,
        at: i64,
    ) -> Result<OrchestrationDetail, Error> {
        let plan = self.plan.as_ref().ok_or(Error::Plan)?;
        let expected = self
            .activated_regions
            .iter()
            .rev()
            .nth(self.rolled_back_regions.len())
            .ok_or(Error::Rollback)?;
        if self.mode != Some(OrchestrationMode::RollbackRequired)
            || observation.observation_id == [0; 32]
            || self
                .rollback_observation_ids
                .contains(&observation.observation_id)
            || observation.plan_id != plan.plan_id
            || observation.region != *expected
            || observation.rollback_package_digest != plan.preflight.rollback_package_digest
            || !observation.baseline_restored
            || observation.observed_at_ns != at
            || observation.source_digest == [0; 32]
            || !observation.verify_digest()
        {
            return Err(Error::Rollback);
        }
        self.rollback_observation_ids
            .insert(observation.observation_id);
        self.rolled_back_regions.push(observation.region.clone());
        let complete = self.rolled_back_regions.len() == self.activated_regions.len();
        if complete {
            self.mode = Some(OrchestrationMode::RolledBack);
        }
        Ok(OrchestrationDetail::RollbackConverged {
            region: observation.region.clone(),
            complete,
        })
    }

    fn finalize(
        &mut self,
        plan_id: [u8; 32],
        report_id: [u8; 32],
        at: i64,
    ) -> Result<OrchestrationDetail, Error> {
        let plan = self.require_plan(plan_id)?;
        if report_id == [0; 32] {
            return Err(Error::Report);
        }
        let status = match self.mode {
            Some(OrchestrationMode::Completed) => OrchestrationReportStatus::SimulatedCompleted,
            Some(OrchestrationMode::RolledBack) => OrchestrationReportStatus::SimulatedRolledBack,
            Some(OrchestrationMode::Aborted) => OrchestrationReportStatus::SimulatedAborted,
            _ => return Err(Error::Report),
        };
        let mut report = OrchestrationReport {
            report_id,
            plan_id,
            plan_digest: plan.plan_digest,
            preflight_report_digest: plan.preflight.report_digest,
            rollback_package_digest: plan.preflight.rollback_package_digest,
            finalized_at_ns: at,
            status,
            completed_wave_count: if status == OrchestrationReportStatus::SimulatedCompleted {
                plan.waves.len()
            } else {
                self.wave_index.map_or(0, |index| index + 1)
            },
            activated_regions: self.activated_regions.clone(),
            rolled_back_regions: self.rolled_back_regions.clone(),
            rollback_trigger: self.rollback_trigger,
            pause_count: self.pause_count,
            restart_count: self.restart_count,
            recovery_epoch: self.recovery_epoch,
            manual_operator_execution_required: true,
            credential_material_created: false,
            deployment_authority_granted: false,
            rollback_execution_authority_granted: false,
            cloud_control_authority_granted: false,
            live_trading_authority_granted: false,
            report_digest: [0; 32],
        };
        report.report_digest = orchestration_report_digest(&report);
        self.report = Some(report.clone());
        Ok(OrchestrationDetail::Finalized(Box::new(report)))
    }

    fn latch_rollback(&mut self, trigger: RollbackTrigger) -> Result<OrchestrationDetail, Error> {
        if self.rollback_trigger.is_none() {
            self.rollback_trigger = Some(trigger);
        }
        self.mode = Some(OrchestrationMode::RollbackRequired);
        Ok(OrchestrationDetail::RollbackRequired(
            self.rollback_trigger.ok_or(Error::Rollback)?,
        ))
    }

    fn require_plan(&self, plan_id: [u8; 32]) -> Result<&OrchestrationPlan, Error> {
        let plan = self.plan.as_ref().ok_or(Error::Plan)?;
        if plan.plan_id == plan_id {
            Ok(plan)
        } else {
            Err(Error::Plan)
        }
    }

    fn current_health(&self, at: i64) -> Result<&RegionalHealthFrame, Error> {
        let frame = self.last_health.as_ref().ok_or(Error::Health)?;
        if at < frame.observed_at_ns
            || at > frame.valid_until_ns
            || at - frame.observed_at_ns > self.policy.maximum_health_age_ns
        {
            Err(Error::Health)
        } else {
            Ok(frame)
        }
    }

    fn require_current_health(&self, regions: &[String], at: i64) -> Result<(), Error> {
        let frame = self.current_health(at)?;
        if regions_healthy(frame, regions) {
            Ok(())
        } else {
            Err(Error::Health)
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> OrchestrationSnapshot {
        OrchestrationSnapshot {
            accepted_commands: self.accepted_commands,
            plan_id: self.plan.as_ref().map(|item| item.plan_id),
            mode: self.mode,
            wave_index: self.wave_index,
            activated_regions: self.activated_regions.clone(),
            rolled_back_regions: self.rolled_back_regions.clone(),
            rollback_trigger: self.rollback_trigger,
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
        hasher.update(b"deployment-orchestration-state-v1");
        hash_json(&mut hasher, &self.policy);
        hash_json(&mut hasher, &self.plan);
        hash_json(&mut hasher, &self.mode);
        hash_json(&mut hasher, &self.wave_index);
        hash_json(&mut hasher, &self.activated_regions);
        hash_json(&mut hasher, &self.rolled_back_regions);
        hash_json(&mut hasher, &self.rollback_observation_ids);
        hash_json(&mut hasher, &self.last_health);
        hash_json(&mut hasher, &self.last_health_sequence);
        hash_json(&mut hasher, &self.wave_started_at_ns);
        hash_json(&mut hasher, &self.rollback_trigger);
        hash_json(&mut hasher, &self.pause_count);
        hash_json(&mut hasher, &self.restart_count);
        hash_json(&mut hasher, &self.recovery_epoch);
        hash_json(&mut hasher, &self.pre_restart_mode);
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

fn validate_policy(policy: &OrchestrationPolicy) -> Result<(), Error> {
    if policy.maximum_waves == 0
        || policy.maximum_waves > MAX_WAVES_HARD
        || policy.maximum_regions == 0
        || policy.maximum_regions > MAX_REGIONS_HARD
        || policy.maximum_preflight_age_ns <= 0
        || policy.maximum_plan_age_ns <= 0
        || policy.maximum_health_age_ns <= 0
        || policy.maximum_wave_duration_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn validate_plan(
    plan: &OrchestrationPlan,
    policy: &OrchestrationPolicy,
    at: i64,
) -> Result<(), Error> {
    let report = &plan.preflight;
    if plan.plan_id == [0; 32]
        || plan.created_at_ns != at
        || plan.expires_at_ns <= at
        || plan.expires_at_ns - at > policy.maximum_plan_age_ns
        || plan.expires_at_ns > report.package_expires_at_ns
        || plan.waves.is_empty()
        || plan.waves.len() > policy.maximum_waves
        || !plan.verify_digest(policy)
        || !report.verify_digest()
        || report.status != PreflightStatus::ReadyForManualDeployment
        || !report.reasons.is_empty()
        || !report.manual_operator_execution_required
        || report.credential_material_created
        || report.signing_authority_granted
        || report.deployment_authority_granted
        || report.rollback_execution_authority_granted
        || report.cloud_control_authority_granted
        || report.live_trading_authority_granted
        || report.evaluated_at_ns > at
        || at - report.evaluated_at_ns > policy.maximum_preflight_age_ns
        || report.package_expires_at_ns < at
        || report.regions.is_empty()
        || report.regions.len() > policy.maximum_regions
        || !report.regions.windows(2).all(|pair| pair[0] < pair[1])
        || report.rollback_package_digest == [0; 32]
    {
        return Err(Error::Plan);
    }
    let mut covered = BTreeSet::new();
    let mut wave_ids = BTreeSet::new();
    for wave in &plan.waves {
        if wave.wave_id == [0; 32]
            || !wave_ids.insert(wave.wave_id)
            || wave.regions.is_empty()
            || !wave.regions.windows(2).all(|pair| pair[0] < pair[1])
            || wave.minimum_observation_ns <= 0
            || wave.maximum_duration_ns < wave.minimum_observation_ns
            || wave.maximum_duration_ns > policy.maximum_wave_duration_ns
            || !wave.verify_digest()
        {
            return Err(Error::Plan);
        }
        for region in &wave.regions {
            if !covered.insert(region.clone()) {
                return Err(Error::Plan);
            }
        }
    }
    if covered.into_iter().collect::<Vec<_>>() != report.regions {
        return Err(Error::Plan);
    }
    Ok(())
}

fn validate_health_frame(
    frame: &RegionalHealthFrame,
    plan: &OrchestrationPlan,
    last_sequence: Option<u64>,
    at: i64,
) -> Result<(), Error> {
    let expected_sequence = match last_sequence {
        Some(value) => value.checked_add(1).ok_or(Error::Overflow)?,
        None => 0,
    };
    if frame.sequence != expected_sequence
        || frame.observed_at_ns != at
        || frame.valid_until_ns < at
        || frame.source_digest == [0; 32]
        || !frame.verify_digest()
        || !frame
            .regions
            .windows(2)
            .all(|pair| pair[0].region < pair[1].region)
        || frame
            .regions
            .iter()
            .map(|item| &item.region)
            .ne(plan.preflight.regions.iter())
    {
        Err(Error::Health)
    } else {
        Ok(())
    }
}

fn regions_healthy(frame: &RegionalHealthFrame, regions: &[String]) -> bool {
    regions.iter().all(|region| {
        frame
            .regions
            .iter()
            .find(|item| &item.region == region)
            .is_some_and(|item| {
                item.package_applied
                    && item.service_healthy
                    && item.risk_healthy
                    && item.reconciliation_healthy
                    && item.capital_floor_preserved
            })
    })
}

fn severe_trigger(frame: &RegionalHealthFrame, regions: &[String]) -> Option<RollbackTrigger> {
    for region in regions {
        if let Some(item) = frame.regions.iter().find(|item| &item.region == region) {
            if !item.capital_floor_preserved {
                return Some(RollbackTrigger::CapitalFloorBreach);
            }
            if !item.reconciliation_healthy {
                return Some(RollbackTrigger::ReconciliationFailure);
            }
        }
    }
    None
}

fn plan_current(plan: &OrchestrationPlan, at: i64) -> bool {
    at <= plan.expires_at_ns && at <= plan.preflight.package_expires_at_ns
}

fn wave_digest(value: &DeploymentWave) -> [u8; 32] {
    let mut clone = value.clone();
    clone.wave_digest = [0; 32];
    digest_json(b"deployment-wave-v1", &clone)
}

fn health_digest(value: &RegionalHealthFrame) -> [u8; 32] {
    let mut clone = value.clone();
    clone.frame_digest = [0; 32];
    digest_json(b"deployment-regional-health-v1", &clone)
}

fn rollback_observation_digest(value: &RollbackObservation) -> [u8; 32] {
    let mut clone = value.clone();
    clone.observation_digest = [0; 32];
    digest_json(b"deployment-rollback-observation-v1", &clone)
}

fn plan_digest(value: &OrchestrationPlan) -> [u8; 32] {
    let mut clone = value.clone();
    clone.plan_digest = [0; 32];
    digest_json(b"deployment-orchestration-plan-v1", &clone)
}

fn orchestration_report_digest(value: &OrchestrationReport) -> [u8; 32] {
    let mut clone = value.clone();
    clone.report_digest = [0; 32];
    digest_json(b"deployment-orchestration-report-v1", &clone)
}

fn outcome_digest(value: &OrchestrationOutcome) -> [u8; 32] {
    let mut clone = value.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"deployment-orchestration-outcome-v1", &clone)
}

fn digest_json<T: Serialize + ?Sized>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize + ?Sized>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable orchestration state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: OrchestrationCommand,
}

/// Encodes one bounded, versioned orchestration command.
///
/// # Errors
///
/// Rejects serialization failure or a command exceeding the canonical bound.
pub fn encode_command(command: &OrchestrationCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one complete, bounded, versioned orchestration command.
///
/// # Errors
///
/// Rejects oversized, malformed, trailing, or unsupported-version input.
pub fn decode_command(bytes: &[u8]) -> Result<OrchestrationCommand, Error> {
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
