#![forbid(unsafe_code)]

//! Deterministic offline deployment change-control and manual-handoff simulation.
//!
//! No type in this crate carries credentials or executable control-plane authority.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, ChangeControlCheckpoint,
    ChangeControlRecovery, ChangeControlStorageError, DurableChangeControl,
};
pub use report::{read_report, write_report_create_new, ChangeControlReportFileError};

use deployment_adapter_certification::{AdapterCertificationReport, CertificationStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_WINDOWS_HARD: usize = 128;
const MAX_STEPS_HARD: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ChangeCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeControlPolicy {
    pub maximum_windows: usize,
    pub maximum_steps: usize,
    pub maximum_certificate_age_ns: i64,
    pub maximum_plan_age_ns: i64,
    pub maximum_approval_age_ns: i64,
    pub maximum_permission_lifetime_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceWindow {
    pub window_id: [u8; 32],
    pub starts_at_ns: i64,
    pub ends_at_ns: i64,
    pub window_digest: [u8; 32],
}

impl MaintenanceWindow {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.window_digest = maintenance_window_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.window_digest == maintenance_window_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeAction {
    ApplyConfiguration,
    StartService,
    ShiftTraffic,
    VerifyHealth,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeStep {
    pub step_id: [u8; 32],
    pub index: u32,
    pub region: String,
    pub action: ChangeAction,
    pub subject_digest: [u8; 32],
    pub step_digest: [u8; 32],
}

impl ChangeStep {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.step_digest = change_step_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.step_digest == change_step_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmergencyTrigger {
    CapitalFloorBreach,
    ReconciliationFailure,
    RegionalHealthFailure,
    ControlPlaneUnknown,
    OperatorAbort,
    DeadlineExceeded,
}

impl EmergencyTrigger {
    pub const ALL: [Self; 6] = [
        Self::CapitalFloorBreach,
        Self::ReconciliationFailure,
        Self::RegionalHealthFailure,
        Self::ControlPlaneUnknown,
        Self::OperatorAbort,
        Self::DeadlineExceeded,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmergencyRollbackPolicy {
    pub rollback_package_digest: [u8; 32],
    pub rollback_runbook_digest: [u8; 32],
    pub triggers: Vec<EmergencyTrigger>,
    pub maximum_rollback_permission_lifetime_ns: i64,
    pub policy_digest: [u8; 32],
}

impl EmergencyRollbackPolicy {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.triggers.sort();
        self.policy_digest = emergency_policy_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.policy_digest == emergency_policy_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangePlan {
    pub plan_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub certificate: AdapterCertificationReport,
    pub windows: Vec<MaintenanceWindow>,
    pub steps: Vec<ChangeStep>,
    pub emergency_policy: EmergencyRollbackPolicy,
    pub control_policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl ChangePlan {
    #[must_use]
    pub fn sealed(mut self, policy: &ChangeControlPolicy) -> Self {
        self.control_policy_digest = digest_json(b"deployment-change-control-policy-v1", policy);
        self.plan_digest = change_plan_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &ChangeControlPolicy) -> bool {
        self.control_policy_digest == digest_json(b"deployment-change-control-policy-v1", policy)
            && self.plan_digest == change_plan_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRole {
    Release,
    Risk,
}

impl ApprovalRole {
    const ALL: [Self; 2] = [Self::Release, Self::Risk];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeApproval {
    pub approval_id: [u8; 32],
    pub plan_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub role: ApprovalRole,
    pub operator_id: [u8; 32],
    pub decision: ApprovalDecision,
    pub decided_at_ns: i64,
    pub valid_until_ns: i64,
    pub approval_digest: [u8; 32],
}

impl ChangeApproval {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.approval_digest = approval_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.approval_digest == approval_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Change,
    Rollback,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ManualPermission {
    pub permission_id: [u8; 32],
    pub plan_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub certificate_digest: [u8; 32],
    pub kind: PermissionKind,
    pub step_index: u32,
    pub step_digest: [u8; 32],
    pub issued_at_ns: i64,
    pub valid_until_ns: i64,
    pub manual_operator_execution_required: bool,
    pub credential_material_created: bool,
    pub authentication_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub rollback_execution_authority_granted: bool,
    pub traffic_authority_granted: bool,
    pub cloud_control_authority_granted: bool,
    pub permission_digest: [u8; 32],
}

impl ManualPermission {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.permission_digest == permission_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeMode {
    Registered,
    Approved,
    Active,
    Paused,
    RollbackRequired,
    Completed,
    Aborted,
    RolledBack,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReportStatus {
    SimulatedHandoffsCompleted,
    SimulatedAborted,
    SimulatedRolledBack,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ChangeControlReport {
    pub report_id: [u8; 32],
    pub plan_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub certificate_digest: [u8; 32],
    pub finalized_at_ns: i64,
    pub status: ChangeReportStatus,
    pub consumed_change_steps: Vec<u32>,
    pub rolled_back_steps: Vec<u32>,
    pub emergency_trigger: Option<EmergencyTrigger>,
    pub invalidated_permission_count: usize,
    pub manual_operator_execution_required: bool,
    pub credential_material_created: bool,
    pub authentication_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub rollback_execution_authority_granted: bool,
    pub traffic_authority_granted: bool,
    pub cloud_control_authority_granted: bool,
    pub live_trading_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl ChangeControlReport {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest == change_report_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ChangeCommand {
    Register {
        command_id: ChangeCommandId,
        plan: Box<ChangePlan>,
        recorded_at_ns: i64,
    },
    RecordApproval {
        command_id: ChangeCommandId,
        approval: ChangeApproval,
        recorded_at_ns: i64,
    },
    IssuePermission {
        command_id: ChangeCommandId,
        plan_id: [u8; 32],
        permission_id: [u8; 32],
        valid_until_ns: i64,
        recorded_at_ns: i64,
    },
    ConsumePermission {
        command_id: ChangeCommandId,
        plan_id: [u8; 32],
        permission_id: [u8; 32],
        permission_digest: [u8; 32],
        operator_handoff_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    Pause {
        command_id: ChangeCommandId,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        reason_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    Resume {
        command_id: ChangeCommandId,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        recorded_at_ns: i64,
    },
    Abort {
        command_id: ChangeCommandId,
        plan_id: [u8; 32],
        operator_id: [u8; 32],
        reason_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    SignalEmergency {
        command_id: ChangeCommandId,
        plan_id: [u8; 32],
        trigger: EmergencyTrigger,
        evidence_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: ChangeCommandId,
        plan_id: [u8; 32],
        report_id: [u8; 32],
        recorded_at_ns: i64,
    },
}

impl ChangeCommand {
    #[must_use]
    pub const fn command_id(&self) -> ChangeCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::RecordApproval { command_id, .. }
            | Self::IssuePermission { command_id, .. }
            | Self::ConsumePermission { command_id, .. }
            | Self::Pause { command_id, .. }
            | Self::Resume { command_id, .. }
            | Self::Abort { command_id, .. }
            | Self::SignalEmergency { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::RecordApproval { recorded_at_ns, .. }
            | Self::IssuePermission { recorded_at_ns, .. }
            | Self::ConsumePermission { recorded_at_ns, .. }
            | Self::Pause { recorded_at_ns, .. }
            | Self::Resume { recorded_at_ns, .. }
            | Self::Abort { recorded_at_ns, .. }
            | Self::SignalEmergency { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ChangeDetail {
    Registered,
    ApprovalRecorded(ApprovalRole),
    PermissionIssued(Box<ManualPermission>),
    PermissionConsumed {
        kind: PermissionKind,
        step_index: u32,
    },
    Paused,
    Resumed,
    Aborted,
    RollbackRequired(EmergencyTrigger),
    Finalized(Box<ChangeControlReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeOutcome {
    pub command_id: ChangeCommandId,
    pub detail: ChangeDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeSnapshot {
    pub accepted_commands: u64,
    pub plan_id: Option<[u8; 32]>,
    pub mode: Option<ChangeMode>,
    pub consumed_change_steps: Vec<u32>,
    pub rolled_back_steps: Vec<u32>,
    pub active_permission: Option<ManualPermission>,
    pub last_report: Option<ChangeControlReport>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("deployment change-control configuration is invalid")]
    Config,
    #[error("deployment change-control timestamp is invalid or regressed")]
    Timestamp,
    #[error("deployment change-control command exceeds its canonical bound")]
    CommandBound,
    #[error("deployment change-control JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported deployment change-control version: {0}")]
    Version(u16),
    #[error("change-control command id was reused for different content")]
    IdempotencyConflict,
    #[error("change-control plan or certificate is invalid")]
    Plan,
    #[error("change-control approval is invalid")]
    Approval,
    #[error("change-control permission is invalid, expired, used or out of order")]
    Permission,
    #[error("change-control lifecycle gate is closed")]
    GateClosed,
    #[error("change-control emergency signal is invalid")]
    Emergency,
    #[error("change-control report lifecycle is invalid")]
    Report,
    #[error("change-control is already finalized")]
    Finalized,
    #[error("change-control arithmetic overflow")]
    Overflow,
    #[error("deployment change control is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct DeploymentChangeControl {
    policy: ChangeControlPolicy,
    plan: Option<ChangePlan>,
    mode: Option<ChangeMode>,
    approvals: BTreeMap<ApprovalRole, ChangeApproval>,
    approval_ids: BTreeSet<[u8; 32]>,
    permissions: BTreeMap<[u8; 32], ManualPermission>,
    active_permission: Option<ManualPermission>,
    consumed_permissions: BTreeSet<[u8; 32]>,
    invalidated_permissions: BTreeSet<[u8; 32]>,
    consumed_change_steps: Vec<u32>,
    rolled_back_steps: Vec<u32>,
    emergency_trigger: Option<EmergencyTrigger>,
    report: Option<ChangeControlReport>,
    processed: BTreeMap<ChangeCommandId, ([u8; 32], ChangeOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl DeploymentChangeControl {
    /// Creates one empty offline change-control owner.
    ///
    /// # Errors
    ///
    /// Rejects invalid limits or time bounds.
    pub fn new(policy: ChangeControlPolicy) -> Result<Self, Error> {
        validate_control_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            mode: None,
            approvals: BTreeMap::new(),
            approval_ids: BTreeSet::new(),
            permissions: BTreeMap::new(),
            active_permission: None,
            consumed_permissions: BTreeSet::new(),
            invalidated_permissions: BTreeSet::new(),
            consumed_change_steps: Vec::new(),
            rolled_back_steps: Vec::new(),
            emergency_trigger: None,
            report: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic change-control command.
    ///
    /// # Errors
    ///
    /// Integrity, chronology, approval, permission or lifecycle failures halt.
    pub fn apply(&mut self, command: &ChangeCommand) -> Result<ChangeOutcome, Error> {
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
        let mut outcome = ChangeOutcome {
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

    fn apply_fresh(&mut self, command: &ChangeCommand) -> Result<ChangeDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalized);
        }
        match command {
            ChangeCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some() {
                    return Err(Error::Plan);
                }
                validate_plan(plan, &self.policy, *recorded_at_ns)?;
                self.plan = Some((**plan).clone());
                self.mode = Some(ChangeMode::Registered);
                Ok(ChangeDetail::Registered)
            }
            ChangeCommand::RecordApproval {
                approval,
                recorded_at_ns,
                ..
            } => self.record_approval(approval, *recorded_at_ns),
            ChangeCommand::IssuePermission {
                plan_id,
                permission_id,
                valid_until_ns,
                recorded_at_ns,
                ..
            } => self.issue_permission(*plan_id, *permission_id, *valid_until_ns, *recorded_at_ns),
            ChangeCommand::ConsumePermission {
                plan_id,
                permission_id,
                permission_digest,
                operator_handoff_digest,
                recorded_at_ns,
                ..
            } => self.consume_permission(
                *plan_id,
                *permission_id,
                *permission_digest,
                *operator_handoff_digest,
                *recorded_at_ns,
            ),
            ChangeCommand::Pause {
                plan_id,
                operator_id,
                reason_digest,
                ..
            } => self.pause(*plan_id, *operator_id, *reason_digest),
            ChangeCommand::Resume {
                plan_id,
                operator_id,
                recorded_at_ns,
                ..
            } => self.resume(*plan_id, *operator_id, *recorded_at_ns),
            ChangeCommand::Abort {
                plan_id,
                operator_id,
                reason_digest,
                ..
            } => self.abort(*plan_id, *operator_id, *reason_digest),
            ChangeCommand::SignalEmergency {
                plan_id,
                trigger,
                evidence_digest,
                ..
            } => self.signal_emergency(*plan_id, *trigger, *evidence_digest),
            ChangeCommand::Finalize {
                plan_id,
                report_id,
                recorded_at_ns,
                ..
            } => self.finalize(*plan_id, *report_id, *recorded_at_ns),
        }
    }

    fn record_approval(
        &mut self,
        approval: &ChangeApproval,
        at: i64,
    ) -> Result<ChangeDetail, Error> {
        let plan = self.plan.as_ref().ok_or(Error::Plan)?;
        if self.mode != Some(ChangeMode::Registered)
            || approval.approval_id == [0; 32]
            || self.approval_ids.contains(&approval.approval_id)
            || self.approvals.contains_key(&approval.role)
            || approval.plan_id != plan.plan_id
            || approval.plan_digest != plan.plan_digest
            || approval.operator_id == [0; 32]
            || approval.decided_at_ns != at
            || approval.valid_until_ns <= at
            || approval.valid_until_ns > plan.expires_at_ns
            || approval.valid_until_ns - at > self.policy.maximum_approval_age_ns
            || !approval.verify_digest()
        {
            return Err(Error::Approval);
        }
        self.approval_ids.insert(approval.approval_id);
        self.approvals.insert(approval.role, approval.clone());
        if approval.decision == ApprovalDecision::Reject {
            self.mode = Some(ChangeMode::Aborted);
        } else if self.approvals.len() == ApprovalRole::ALL.len() {
            let operators = self
                .approvals
                .values()
                .map(|item| item.operator_id)
                .collect::<BTreeSet<_>>();
            if operators.len() != ApprovalRole::ALL.len()
                || self
                    .approvals
                    .values()
                    .any(|item| item.decision != ApprovalDecision::Approve)
            {
                return Err(Error::Approval);
            }
            self.mode = Some(ChangeMode::Approved);
        }
        Ok(ChangeDetail::ApprovalRecorded(approval.role))
    }

    fn issue_permission(
        &mut self,
        plan_id: [u8; 32],
        permission_id: [u8; 32],
        valid_until: i64,
        at: i64,
    ) -> Result<ChangeDetail, Error> {
        let plan = self.require_plan(plan_id)?.clone();
        if permission_id == [0; 32]
            || self.permissions.contains_key(&permission_id)
            || self.active_permission.is_some()
        {
            return Err(Error::Permission);
        }
        let (kind, step) = if self.mode == Some(ChangeMode::RollbackRequired) {
            let expected = self
                .consumed_change_steps
                .iter()
                .rev()
                .nth(self.rolled_back_steps.len())
                .copied()
                .ok_or(Error::Permission)?;
            let step = plan
                .steps
                .get(usize::try_from(expected).map_err(|_| Error::Overflow)?)
                .ok_or(Error::Permission)?;
            if valid_until <= at
                || valid_until - at
                    > plan
                        .emergency_policy
                        .maximum_rollback_permission_lifetime_ns
            {
                return Err(Error::Permission);
            }
            (PermissionKind::Rollback, step)
        } else {
            if !matches!(self.mode, Some(ChangeMode::Approved | ChangeMode::Active))
                || !plan_current(&plan, at)
                || !inside_window(&plan.windows, at)
            {
                return Err(Error::GateClosed);
            }
            self.require_current_dual_approval(at)?;
            let index = self.consumed_change_steps.len();
            let step = plan.steps.get(index).ok_or(Error::Permission)?;
            let window_end = active_window_end(&plan.windows, at).ok_or(Error::GateClosed)?;
            if valid_until <= at
                || valid_until > window_end
                || valid_until > plan.expires_at_ns
                || valid_until - at > self.policy.maximum_permission_lifetime_ns
            {
                return Err(Error::Permission);
            }
            (PermissionKind::Change, step)
        };
        let mut permission = ManualPermission {
            permission_id,
            plan_id,
            plan_digest: plan.plan_digest,
            certificate_digest: plan.certificate.report_digest,
            kind,
            step_index: step.index,
            step_digest: step.step_digest,
            issued_at_ns: at,
            valid_until_ns: valid_until,
            manual_operator_execution_required: true,
            credential_material_created: false,
            authentication_authority_granted: false,
            deployment_authority_granted: false,
            rollback_execution_authority_granted: false,
            traffic_authority_granted: false,
            cloud_control_authority_granted: false,
            permission_digest: [0; 32],
        };
        permission.permission_digest = permission_digest(&permission);
        self.permissions.insert(permission_id, permission.clone());
        self.active_permission = Some(permission.clone());
        Ok(ChangeDetail::PermissionIssued(Box::new(permission)))
    }

    fn consume_permission(
        &mut self,
        plan_id: [u8; 32],
        permission_id: [u8; 32],
        expected_digest: [u8; 32],
        handoff: [u8; 32],
        at: i64,
    ) -> Result<ChangeDetail, Error> {
        let plan = self.require_plan(plan_id)?.clone();
        let permission = self
            .active_permission
            .as_ref()
            .ok_or(Error::Permission)?
            .clone();
        if permission.permission_id != permission_id
            || permission.permission_digest != expected_digest
            || !permission.verify_digest()
            || handoff == [0; 32]
            || at > permission.valid_until_ns
            || self.consumed_permissions.contains(&permission_id)
            || self.invalidated_permissions.contains(&permission_id)
        {
            return Err(Error::Permission);
        }
        self.consumed_permissions.insert(permission_id);
        self.active_permission = None;
        match permission.kind {
            PermissionKind::Change => {
                let expected =
                    u32::try_from(self.consumed_change_steps.len()).map_err(|_| Error::Overflow)?;
                if permission.step_index != expected {
                    return Err(Error::Permission);
                }
                self.consumed_change_steps.push(expected);
                self.mode = if self.consumed_change_steps.len() == plan.steps.len() {
                    Some(ChangeMode::Completed)
                } else {
                    Some(ChangeMode::Active)
                };
            }
            PermissionKind::Rollback => {
                let expected = self
                    .consumed_change_steps
                    .iter()
                    .rev()
                    .nth(self.rolled_back_steps.len())
                    .copied()
                    .ok_or(Error::Permission)?;
                if permission.step_index != expected {
                    return Err(Error::Permission);
                }
                self.rolled_back_steps.push(expected);
                if self.rolled_back_steps.len() == self.consumed_change_steps.len() {
                    self.mode = Some(ChangeMode::RolledBack);
                }
            }
        }
        Ok(ChangeDetail::PermissionConsumed {
            kind: permission.kind,
            step_index: permission.step_index,
        })
    }

    fn pause(
        &mut self,
        plan_id: [u8; 32],
        operator: [u8; 32],
        reason: [u8; 32],
    ) -> Result<ChangeDetail, Error> {
        self.require_plan(plan_id)?;
        if !matches!(self.mode, Some(ChangeMode::Approved | ChangeMode::Active))
            || operator == [0; 32]
            || reason == [0; 32]
        {
            return Err(Error::GateClosed);
        }
        self.invalidate_active();
        self.mode = Some(ChangeMode::Paused);
        Ok(ChangeDetail::Paused)
    }

    fn resume(
        &mut self,
        plan_id: [u8; 32],
        operator: [u8; 32],
        at: i64,
    ) -> Result<ChangeDetail, Error> {
        let plan = self.require_plan(plan_id)?;
        if self.mode != Some(ChangeMode::Paused)
            || operator == [0; 32]
            || !plan_current(plan, at)
            || !inside_window(&plan.windows, at)
        {
            return Err(Error::GateClosed);
        }
        self.require_current_dual_approval(at)?;
        self.mode = if self.consumed_change_steps.is_empty() {
            Some(ChangeMode::Approved)
        } else {
            Some(ChangeMode::Active)
        };
        Ok(ChangeDetail::Resumed)
    }

    fn abort(
        &mut self,
        plan_id: [u8; 32],
        operator: [u8; 32],
        reason: [u8; 32],
    ) -> Result<ChangeDetail, Error> {
        self.require_plan(plan_id)?;
        if operator == [0; 32]
            || reason == [0; 32]
            || !matches!(
                self.mode,
                Some(
                    ChangeMode::Registered
                        | ChangeMode::Approved
                        | ChangeMode::Active
                        | ChangeMode::Paused
                )
            )
        {
            return Err(Error::GateClosed);
        }
        self.invalidate_active();
        if self.consumed_change_steps.is_empty() {
            self.mode = Some(ChangeMode::Aborted);
            Ok(ChangeDetail::Aborted)
        } else {
            self.latch_rollback(EmergencyTrigger::OperatorAbort)
        }
    }

    fn signal_emergency(
        &mut self,
        plan_id: [u8; 32],
        trigger: EmergencyTrigger,
        evidence: [u8; 32],
    ) -> Result<ChangeDetail, Error> {
        let plan = self.require_plan(plan_id)?;
        if evidence == [0; 32]
            || !plan.emergency_policy.triggers.contains(&trigger)
            || !matches!(
                self.mode,
                Some(ChangeMode::Approved | ChangeMode::Active | ChangeMode::Paused)
            )
        {
            return Err(Error::Emergency);
        }
        self.invalidate_active();
        if self.consumed_change_steps.is_empty() {
            self.mode = Some(ChangeMode::Aborted);
            Ok(ChangeDetail::Aborted)
        } else {
            self.latch_rollback(trigger)
        }
    }

    fn latch_rollback(&mut self, trigger: EmergencyTrigger) -> Result<ChangeDetail, Error> {
        if self.emergency_trigger.is_none() {
            self.emergency_trigger = Some(trigger);
        }
        self.mode = Some(ChangeMode::RollbackRequired);
        Ok(ChangeDetail::RollbackRequired(
            self.emergency_trigger.ok_or(Error::Emergency)?,
        ))
    }

    fn finalize(
        &mut self,
        plan_id: [u8; 32],
        report_id: [u8; 32],
        at: i64,
    ) -> Result<ChangeDetail, Error> {
        let plan = self.require_plan(plan_id)?;
        if report_id == [0; 32] || self.active_permission.is_some() {
            return Err(Error::Report);
        }
        let status = match self.mode {
            Some(ChangeMode::Completed) => ChangeReportStatus::SimulatedHandoffsCompleted,
            Some(ChangeMode::Aborted) => ChangeReportStatus::SimulatedAborted,
            Some(ChangeMode::RolledBack) => ChangeReportStatus::SimulatedRolledBack,
            _ => return Err(Error::Report),
        };
        let mut report = ChangeControlReport {
            report_id,
            plan_id,
            plan_digest: plan.plan_digest,
            certificate_digest: plan.certificate.report_digest,
            finalized_at_ns: at,
            status,
            consumed_change_steps: self.consumed_change_steps.clone(),
            rolled_back_steps: self.rolled_back_steps.clone(),
            emergency_trigger: self.emergency_trigger,
            invalidated_permission_count: self.invalidated_permissions.len(),
            manual_operator_execution_required: true,
            credential_material_created: false,
            authentication_authority_granted: false,
            deployment_authority_granted: false,
            rollback_execution_authority_granted: false,
            traffic_authority_granted: false,
            cloud_control_authority_granted: false,
            live_trading_authority_granted: false,
            report_digest: [0; 32],
        };
        report.report_digest = change_report_digest(&report);
        self.report = Some(report.clone());
        Ok(ChangeDetail::Finalized(Box::new(report)))
    }

    fn require_current_dual_approval(&self, at: i64) -> Result<(), Error> {
        if self.approvals.len() != ApprovalRole::ALL.len()
            || self.approvals.values().any(|approval| {
                approval.decision != ApprovalDecision::Approve
                    || at < approval.decided_at_ns
                    || at > approval.valid_until_ns
                    || at - approval.decided_at_ns > self.policy.maximum_approval_age_ns
            })
            || self
                .approvals
                .values()
                .map(|approval| approval.operator_id)
                .collect::<BTreeSet<_>>()
                .len()
                != ApprovalRole::ALL.len()
        {
            Err(Error::Approval)
        } else {
            Ok(())
        }
    }

    fn require_plan(&self, plan_id: [u8; 32]) -> Result<&ChangePlan, Error> {
        let plan = self.plan.as_ref().ok_or(Error::Plan)?;
        if plan.plan_id == plan_id {
            Ok(plan)
        } else {
            Err(Error::Plan)
        }
    }

    fn invalidate_active(&mut self) {
        if let Some(permission) = self.active_permission.take() {
            self.invalidated_permissions
                .insert(permission.permission_id);
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ChangeSnapshot {
        ChangeSnapshot {
            accepted_commands: self.accepted_commands,
            plan_id: self.plan.as_ref().map(|plan| plan.plan_id),
            mode: self.mode,
            consumed_change_steps: self.consumed_change_steps.clone(),
            rolled_back_steps: self.rolled_back_steps.clone(),
            active_permission: self.active_permission.clone(),
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
        hasher.update(b"deployment-change-control-state-v1");
        hash_json(&mut hasher, &self.policy);
        hash_json(&mut hasher, &self.plan);
        hash_json(&mut hasher, &self.mode);
        hash_json(&mut hasher, &self.approvals);
        hash_json(&mut hasher, &self.approval_ids);
        for (permission_id, permission) in &self.permissions {
            hasher.update(permission_id);
            hash_json(&mut hasher, permission);
        }
        hash_json(&mut hasher, &self.active_permission);
        hash_json(&mut hasher, &self.consumed_permissions);
        hash_json(&mut hasher, &self.invalidated_permissions);
        hash_json(&mut hasher, &self.consumed_change_steps);
        hash_json(&mut hasher, &self.rolled_back_steps);
        hash_json(&mut hasher, &self.emergency_trigger);
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

fn validate_control_policy(policy: &ChangeControlPolicy) -> Result<(), Error> {
    if policy.maximum_windows == 0
        || policy.maximum_windows > MAX_WINDOWS_HARD
        || policy.maximum_steps == 0
        || policy.maximum_steps > MAX_STEPS_HARD
        || policy.maximum_certificate_age_ns <= 0
        || policy.maximum_plan_age_ns <= 0
        || policy.maximum_approval_age_ns <= 0
        || policy.maximum_permission_lifetime_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn validate_plan(plan: &ChangePlan, policy: &ChangeControlPolicy, at: i64) -> Result<(), Error> {
    let certificate = &plan.certificate;
    if plan.plan_id == [0; 32]
        || plan.created_at_ns != at
        || plan.expires_at_ns <= at
        || plan.expires_at_ns - at > policy.maximum_plan_age_ns
        || !plan.verify_digest(policy)
        || !valid_certificate(certificate, at, policy.maximum_certificate_age_ns)
        || plan.windows.is_empty()
        || plan.windows.len() > policy.maximum_windows
        || plan.steps.is_empty()
        || plan.steps.len() > policy.maximum_steps
        || !valid_windows(&plan.windows, at, plan.expires_at_ns)
        || !valid_steps(&plan.steps, &certificate.regions)
        || !valid_emergency_policy(&plan.emergency_policy, certificate, policy)
    {
        Err(Error::Plan)
    } else {
        Ok(())
    }
}

fn valid_certificate(report: &AdapterCertificationReport, at: i64, max_age: i64) -> bool {
    report.verify_digest()
        && report.status == CertificationStatus::Certified
        && report.reasons.is_empty()
        && report.finalized_at_ns <= at
        && at
            .checked_sub(report.finalized_at_ns)
            .is_some_and(|age| age <= max_age)
        && report.regions.len() >= 2
        && report.regions.windows(2).all(|pair| pair[0] < pair[1])
        && report.preflight_report_digest != [0; 32]
        && report.rollback_package_digest != [0; 32]
        && report.manual_operator_execution_required
        && !report.credential_material_created
        && !report.authentication_authority_granted
        && !report.deployment_authority_granted
        && !report.rollback_execution_authority_granted
        && !report.traffic_authority_granted
        && !report.cloud_control_authority_granted
        && !report.live_trading_authority_granted
}

fn valid_windows(windows: &[MaintenanceWindow], created: i64, expires: i64) -> bool {
    windows.iter().all(|window| {
        window.window_id != [0; 32]
            && window.starts_at_ns >= created
            && window.ends_at_ns > window.starts_at_ns
            && window.ends_at_ns <= expires
            && window.verify_digest()
    }) && windows
        .windows(2)
        .all(|pair| pair[0].ends_at_ns <= pair[1].starts_at_ns)
        && windows
            .iter()
            .map(|window| window.window_id)
            .collect::<BTreeSet<_>>()
            .len()
            == windows.len()
}

fn valid_steps(steps: &[ChangeStep], regions: &[String]) -> bool {
    steps.iter().enumerate().all(|(index, step)| {
        step.step_id != [0; 32]
            && step.index == u32::try_from(index).unwrap_or(u32::MAX)
            && regions.contains(&step.region)
            && step.subject_digest != [0; 32]
            && step.verify_digest()
    }) && steps
        .iter()
        .map(|step| step.step_id)
        .collect::<BTreeSet<_>>()
        .len()
        == steps.len()
}

fn valid_emergency_policy(
    emergency: &EmergencyRollbackPolicy,
    certificate: &AdapterCertificationReport,
    policy: &ChangeControlPolicy,
) -> bool {
    emergency.verify_digest()
        && emergency.rollback_package_digest == certificate.rollback_package_digest
        && emergency.rollback_runbook_digest != [0; 32]
        && emergency.triggers == EmergencyTrigger::ALL
        && emergency.maximum_rollback_permission_lifetime_ns > 0
        && emergency.maximum_rollback_permission_lifetime_ns
            <= policy.maximum_permission_lifetime_ns
}

fn plan_current(plan: &ChangePlan, at: i64) -> bool {
    at >= plan.created_at_ns && at <= plan.expires_at_ns
}

fn inside_window(windows: &[MaintenanceWindow], at: i64) -> bool {
    active_window_end(windows, at).is_some()
}

fn active_window_end(windows: &[MaintenanceWindow], at: i64) -> Option<i64> {
    windows
        .iter()
        .find(|window| at >= window.starts_at_ns && at < window.ends_at_ns)
        .map(|window| window.ends_at_ns)
}

fn maintenance_window_digest(value: &MaintenanceWindow) -> [u8; 32] {
    let mut clone = value.clone();
    clone.window_digest = [0; 32];
    digest_json(b"deployment-change-window-v1", &clone)
}

fn change_step_digest(value: &ChangeStep) -> [u8; 32] {
    let mut clone = value.clone();
    clone.step_digest = [0; 32];
    digest_json(b"deployment-change-step-v1", &clone)
}

fn emergency_policy_digest(value: &EmergencyRollbackPolicy) -> [u8; 32] {
    let mut clone = value.clone();
    clone.policy_digest = [0; 32];
    digest_json(b"deployment-emergency-policy-v1", &clone)
}

fn change_plan_digest(value: &ChangePlan) -> [u8; 32] {
    let mut clone = value.clone();
    clone.plan_digest = [0; 32];
    digest_json(b"deployment-change-plan-v1", &clone)
}

fn approval_digest(value: &ChangeApproval) -> [u8; 32] {
    let mut clone = value.clone();
    clone.approval_digest = [0; 32];
    digest_json(b"deployment-change-approval-v1", &clone)
}

fn permission_digest(value: &ManualPermission) -> [u8; 32] {
    let mut clone = value.clone();
    clone.permission_digest = [0; 32];
    digest_json(b"deployment-manual-permission-v1", &clone)
}

fn change_report_digest(value: &ChangeControlReport) -> [u8; 32] {
    let mut clone = value.clone();
    clone.report_digest = [0; 32];
    digest_json(b"deployment-change-report-v1", &clone)
}

fn outcome_digest(value: &ChangeOutcome) -> [u8; 32] {
    let mut clone = value.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"deployment-change-outcome-v1", &clone)
}

fn digest_json<T: Serialize + ?Sized>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize + ?Sized>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable change-control state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: ChangeCommand,
}

/// Encodes one bounded, versioned change-control command.
///
/// # Errors
///
/// Rejects serialization failure or a command exceeding the canonical bound.
pub fn encode_command(command: &ChangeCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one complete, bounded, versioned change-control command.
///
/// # Errors
///
/// Rejects oversized, malformed, trailing or unsupported-version input.
pub fn decode_command(bytes: &[u8]) -> Result<ChangeCommand, Error> {
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
