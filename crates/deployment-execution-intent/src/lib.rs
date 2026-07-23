#![forbid(unsafe_code)]

//! Deterministic, credentialless deployment execution-intent certification.
//!
//! This crate can only model manual handoffs. It contains no signer, credential,
//! authenticated transport, control-plane client, or submission path.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableExecutionIntent,
    ExecutionCheckpoint, ExecutionRecovery, ExecutionStorageError,
};
pub use report::{read_report, write_report_create_new, ExecutionReportFileError};

use production_change_readiness::{
    ProductionChangeSubject, ProductionReadinessRecord, ReadinessStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ExecutionCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIntentPolicy {
    pub maximum_readiness_age_ns: i64,
    pub maximum_plan_age_ns: i64,
    pub maximum_intent_lifetime_ns: i64,
    pub maximum_steps: usize,
    pub maximum_regions: usize,
    pub maximum_resources: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentOperation {
    ReadCurrentState,
    ServerSideDryRun,
    ApplyConfiguration,
    RestartService,
    ShiftTraffic,
    VerifyHealth,
    ExecuteRollback,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct PrivilegeCeiling {
    pub allowed_operations: Vec<DeploymentOperation>,
    pub allowed_regions: Vec<String>,
    pub allowed_resource_digests: Vec<[u8; 32]>,
    pub wildcard_access: bool,
    pub secret_read: bool,
    pub cluster_admin: bool,
    pub arbitrary_exec: bool,
    pub privilege_escalation: bool,
    pub cross_region_mutation: bool,
    pub credential_loading: bool,
    pub ceiling_digest: [u8; 32],
}

impl PrivilegeCeiling {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.allowed_operations.sort();
        self.allowed_regions.sort();
        self.allowed_resource_digests.sort_unstable();
        self.ceiling_digest = digest_without(b"execution-privilege-ceiling-v1", &self, |v| {
            v.ceiling_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.ceiling_digest
            == digest_without(b"execution-privilege-ceiling-v1", self, |v| {
                v.ceiling_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct IsolatedExecutorContract {
    pub executor_binary_digest: [u8; 32],
    pub executor_schema_digest: [u8; 32],
    pub audit_policy_digest: [u8; 32],
    pub subject_digest: [u8; 32],
    pub privilege_ceiling: PrivilegeCeiling,
    pub credential_loading: bool,
    pub signature_production: bool,
    pub authenticated_transport: bool,
    pub external_submission: bool,
    pub contract_digest: [u8; 32],
}

impl IsolatedExecutorContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.contract_digest = digest_without(b"isolated-executor-contract-v1", &self, |v| {
            v.contract_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest
            == digest_without(b"isolated-executor-contract-v1", self, |v| {
                v.contract_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionStep {
    pub index: u32,
    pub region: String,
    pub operation: DeploymentOperation,
    pub resource_digest: [u8; 32],
    pub step_digest: [u8; 32],
}

impl ExecutionStep {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.step_digest = digest_without(b"deployment-execution-step-v1", &self, |v| {
            v.step_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.step_digest
            == digest_without(b"deployment-execution-step-v1", self, |v| {
                v.step_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIntentPlan {
    pub plan_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub readiness_valid_until_ns: i64,
    pub readiness_record: ProductionReadinessRecord,
    pub subject: ProductionChangeSubject,
    pub executor_contract: IsolatedExecutorContract,
    pub steps: Vec<ExecutionStep>,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl ExecutionIntentPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &ExecutionIntentPolicy) -> Self {
        self.policy_digest = digest_json(b"execution-intent-policy-v1", policy);
        self.plan_digest = digest_without(b"execution-intent-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &ExecutionIntentPolicy) -> bool {
        self.policy_digest == digest_json(b"execution-intent-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"execution-intent-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DryRunDisposition {
    ManualHandoffOnly,
    Deny,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DryRunCase {
    PermittedBaseline,
    WrongSubject,
    WrongRegion,
    ForbiddenOperation,
    WildcardResource,
    CredentialRequest,
    SignatureRequest,
    AuthenticatedTransport,
    ExpiredIntent,
    ReplayIntent,
}

impl DryRunCase {
    pub const ALL: [Self; 10] = [
        Self::PermittedBaseline,
        Self::WrongSubject,
        Self::WrongRegion,
        Self::ForbiddenOperation,
        Self::WildcardResource,
        Self::CredentialRequest,
        Self::SignatureRequest,
        Self::AuthenticatedTransport,
        Self::ExpiredIntent,
        Self::ReplayIntent,
    ];

    #[must_use]
    pub const fn expected(self) -> DryRunDisposition {
        match self {
            Self::PermittedBaseline => DryRunDisposition::ManualHandoffOnly,
            _ => DryRunDisposition::Deny,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ExecutorDryRunEvidence {
    pub sequence: u8,
    pub case: DryRunCase,
    pub expected: DryRunDisposition,
    pub observed: DryRunDisposition,
    pub observed_at_ns: i64,
    pub observation_digest: [u8; 32],
    pub credential_loaded: bool,
    pub signature_produced: bool,
    pub authenticated_request_sent: bool,
    pub external_mutation_observed: bool,
    pub evidence_digest: [u8; 32],
}

impl ExecutorDryRunEvidence {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.evidence_digest = digest_without(b"executor-dry-run-evidence-v1", &self, |v| {
            v.evidence_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evidence_digest
            == digest_without(b"executor-dry-run-evidence-v1", self, |v| {
                v.evidence_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ManualExecutionIntent {
    pub intent_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub readiness_record_digest: [u8; 32],
    pub contract_digest: [u8; 32],
    pub step: ExecutionStep,
    pub issued_at_ns: i64,
    pub expires_at_ns: i64,
    pub one_use: bool,
    pub manual_operator_required: bool,
    pub credential_material_created: bool,
    pub signature_authority_granted: bool,
    pub authenticated_transport_granted: bool,
    pub deployment_authority_granted: bool,
    pub intent_digest: [u8; 32],
}

impl ManualExecutionIntent {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.intent_digest
            == digest_without(b"manual-execution-intent-v1", self, |v| {
                v.intent_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionReportStatus {
    SimulatedHandoffsCompleted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ExecutionCertificationReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub readiness_record_digest: [u8; 32],
    pub subject_digest: [u8; 32],
    pub contract_digest: [u8; 32],
    pub dry_run_chain_digest: [u8; 32],
    pub completed_step_count: usize,
    pub finalized_at_ns: i64,
    pub status: ExecutionReportStatus,
    pub manual_execution_still_required: bool,
    pub credential_material_created: bool,
    pub signature_authority_granted: bool,
    pub authenticated_transport_granted: bool,
    pub deployment_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl ExecutionCertificationReport {
    /// Seals non-executable evidence for deterministic downstream fixtures.
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"execution-certification-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"execution-certification-report-v1", self, |v| {
                v.report_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExecutionCommand {
    Register {
        command_id: ExecutionCommandId,
        plan: Box<ExecutionIntentPlan>,
        recorded_at_ns: i64,
    },
    RecordDryRun {
        command_id: ExecutionCommandId,
        evidence: ExecutorDryRunEvidence,
        recorded_at_ns: i64,
    },
    Certify {
        command_id: ExecutionCommandId,
        plan_id: [u8; 32],
        certified_at_ns: i64,
        recorded_at_ns: i64,
    },
    IssueIntent {
        command_id: ExecutionCommandId,
        intent_id: [u8; 32],
        issued_at_ns: i64,
        requested_expires_at_ns: i64,
        recorded_at_ns: i64,
    },
    ConsumeIntent {
        command_id: ExecutionCommandId,
        intent: Box<ManualExecutionIntent>,
        operator_handoff_digest: [u8; 32],
        consumed_at_ns: i64,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: ExecutionCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl ExecutionCommand {
    #[must_use]
    pub const fn command_id(&self) -> ExecutionCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::RecordDryRun { command_id, .. }
            | Self::Certify { command_id, .. }
            | Self::IssueIntent { command_id, .. }
            | Self::ConsumeIntent { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::RecordDryRun { recorded_at_ns, .. }
            | Self::Certify { recorded_at_ns, .. }
            | Self::IssueIntent { recorded_at_ns, .. }
            | Self::ConsumeIntent { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ExecutionDetail {
    Registered,
    DryRunRecorded(DryRunCase),
    Certified,
    IntentIssued(Box<ManualExecutionIntent>),
    IntentConsumed(u32),
    Finalized(Box<ExecutionCertificationReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionOutcome {
    pub command_id: ExecutionCommandId,
    pub detail: ExecutionDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionSnapshot {
    pub accepted_commands: u64,
    pub plan_id: Option<[u8; 32]>,
    pub dry_run_count: usize,
    pub certified: bool,
    pub completed_step_count: usize,
    pub active_intent: Option<ManualExecutionIntent>,
    pub last_report: Option<ExecutionCertificationReport>,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("execution-intent configuration is invalid")]
    Config,
    #[error("execution-intent timestamp is invalid or regressed")]
    Timestamp,
    #[error("execution-intent command exceeds its bound")]
    CommandBound,
    #[error("execution-intent JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported execution-intent command version: {0}")]
    Version(u16),
    #[error("execution-intent command id conflict")]
    IdempotencyConflict,
    #[error("readiness record or subject is invalid, stale, substituted, or authority-bearing")]
    Readiness,
    #[error("executor contract or privilege ceiling is invalid")]
    Contract,
    #[error("execution plan or step is invalid")]
    Plan,
    #[error("executor dry-run evidence is invalid or incomplete")]
    DryRun,
    #[error("execution intent is invalid, expired, replayed, or out of order")]
    Intent,
    #[error("execution-intent finalization is invalid")]
    Finalize,
    #[error("execution-intent arithmetic overflow")]
    Overflow,
    #[error("execution-intent owner is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct DeploymentExecutionIntent {
    policy: ExecutionIntentPolicy,
    plan: Option<ExecutionIntentPlan>,
    dry_runs: Vec<ExecutorDryRunEvidence>,
    dry_run_chain_digest: [u8; 32],
    certified: bool,
    completed_steps: usize,
    active_intent: Option<ManualExecutionIntent>,
    consumed_intents: BTreeSet<[u8; 32]>,
    report: Option<ExecutionCertificationReport>,
    processed: BTreeMap<ExecutionCommandId, ([u8; 32], ExecutionOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl DeploymentExecutionIntent {
    /// Creates an empty single-writer execution-intent owner.
    ///
    /// # Errors
    ///
    /// Rejects zero, excessive, or inconsistent policy limits.
    pub fn new(policy: ExecutionIntentPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            dry_runs: Vec::new(),
            dry_run_chain_digest: [0; 32],
            certified: false,
            completed_steps: 0,
            active_intent: None,
            consumed_intents: BTreeSet::new(),
            report: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic command transactionally.
    ///
    /// # Errors
    ///
    /// Integrity, chronology, privilege, lifecycle, or idempotency failures halt.
    pub fn apply(&mut self, command: &ExecutionCommand) -> Result<ExecutionOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        if command.recorded_at_ns() < 0
            || self
                .last_recorded_at_ns
                .is_some_and(|v| command.recorded_at_ns() < v)
        {
            return self.halt(Error::Timestamp);
        }
        let encoded = encode_command(command)?;
        let content = *blake3::hash(&encoded).as_bytes();
        if let Some((prior, outcome)) = self.processed.get(&command.command_id()) {
            if *prior == content {
                return Ok(outcome.clone());
            }
            return self.halt(Error::IdempotencyConflict);
        }
        let mut next = self.clone();
        let detail = match next.transition(command) {
            Ok(v) => v,
            Err(e) => return self.halt(e),
        };
        next.accepted_commands = next
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        let mut outcome = ExecutionOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"execution-intent-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &ExecutionCommand) -> Result<ExecutionDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            ExecutionCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some() || !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(ExecutionDetail::Registered)
            }
            ExecutionCommand::RecordDryRun {
                evidence,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::DryRun)?;
                let expected_index = self.dry_runs.len();
                if self.certified
                    || expected_index >= DryRunCase::ALL.len()
                    || evidence.sequence as usize != expected_index
                    || evidence.case != DryRunCase::ALL[expected_index]
                    || evidence.expected != evidence.case.expected()
                    || evidence.observed != evidence.expected
                    || evidence.observed_at_ns > *recorded_at_ns
                    || evidence.observed_at_ns < plan.created_at_ns
                    || evidence.observation_digest == [0; 32]
                    || !evidence.verify_digest()
                    || evidence.credential_loaded
                    || evidence.signature_produced
                    || evidence.authenticated_request_sent
                    || evidence.external_mutation_observed
                {
                    return Err(Error::DryRun);
                }
                self.dry_run_chain_digest =
                    chain_digest(self.dry_run_chain_digest, evidence.evidence_digest);
                self.dry_runs.push(evidence.clone());
                Ok(ExecutionDetail::DryRunRecorded(evidence.case))
            }
            ExecutionCommand::Certify {
                plan_id,
                certified_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::DryRun)?;
                if self.certified
                    || *plan_id != plan.plan_id
                    || self.dry_runs.len() != DryRunCase::ALL.len()
                    || *certified_at_ns > *recorded_at_ns
                    || !current(plan, &self.policy, *certified_at_ns)
                {
                    return Err(Error::DryRun);
                }
                self.certified = true;
                Ok(ExecutionDetail::Certified)
            }
            ExecutionCommand::IssueIntent {
                intent_id,
                issued_at_ns,
                requested_expires_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Intent)?;
                if !self.certified
                    || self.active_intent.is_some()
                    || *intent_id == [0; 32]
                    || self.consumed_intents.contains(intent_id)
                    || self.completed_steps >= plan.steps.len()
                    || *issued_at_ns > *recorded_at_ns
                    || *requested_expires_at_ns <= *issued_at_ns
                    || !current(plan, &self.policy, *issued_at_ns)
                {
                    return Err(Error::Intent);
                }
                let maximum = issued_at_ns
                    .checked_add(self.policy.maximum_intent_lifetime_ns)
                    .ok_or(Error::Overflow)?;
                if *requested_expires_at_ns > maximum
                    || *requested_expires_at_ns > plan.expires_at_ns
                    || *requested_expires_at_ns > plan.readiness_valid_until_ns
                {
                    return Err(Error::Intent);
                }
                let step = plan.steps[self.completed_steps].clone();
                let mut intent = ManualExecutionIntent {
                    intent_id: *intent_id,
                    plan_digest: plan.plan_digest,
                    readiness_record_digest: plan.readiness_record.record_digest,
                    contract_digest: plan.executor_contract.contract_digest,
                    step,
                    issued_at_ns: *issued_at_ns,
                    expires_at_ns: *requested_expires_at_ns,
                    one_use: true,
                    manual_operator_required: true,
                    credential_material_created: false,
                    signature_authority_granted: false,
                    authenticated_transport_granted: false,
                    deployment_authority_granted: false,
                    intent_digest: [0; 32],
                };
                intent.intent_digest =
                    digest_without(b"manual-execution-intent-v1", &intent, |v| {
                        v.intent_digest = [0; 32];
                    });
                self.active_intent = Some(intent.clone());
                Ok(ExecutionDetail::IntentIssued(Box::new(intent)))
            }
            ExecutionCommand::ConsumeIntent {
                intent,
                operator_handoff_digest,
                consumed_at_ns,
                recorded_at_ns,
                ..
            } => {
                let active = self.active_intent.as_ref().ok_or(Error::Intent)?;
                if *operator_handoff_digest == [0; 32]
                    || !intent.verify_digest()
                    || **intent != *active
                    || self.consumed_intents.contains(&intent.intent_id)
                    || *consumed_at_ns > *recorded_at_ns
                    || *consumed_at_ns < intent.issued_at_ns
                    || *consumed_at_ns > intent.expires_at_ns
                    || !safe_intent(intent)
                {
                    return Err(Error::Intent);
                }
                self.consumed_intents.insert(intent.intent_id);
                self.completed_steps =
                    self.completed_steps.checked_add(1).ok_or(Error::Overflow)?;
                self.active_intent = None;
                Ok(ExecutionDetail::IntentConsumed(intent.step.index))
            }
            ExecutionCommand::Finalize {
                report_id,
                finalized_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                if *report_id == [0; 32]
                    || !self.certified
                    || self.active_intent.is_some()
                    || self.completed_steps != plan.steps.len()
                    || *finalized_at_ns > *recorded_at_ns
                {
                    return Err(Error::Finalize);
                }
                let mut report = ExecutionCertificationReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    readiness_record_digest: plan.readiness_record.record_digest,
                    subject_digest: plan.subject.subject_digest,
                    contract_digest: plan.executor_contract.contract_digest,
                    dry_run_chain_digest: self.dry_run_chain_digest,
                    completed_step_count: self.completed_steps,
                    finalized_at_ns: *finalized_at_ns,
                    status: ExecutionReportStatus::SimulatedHandoffsCompleted,
                    manual_execution_still_required: true,
                    credential_material_created: false,
                    signature_authority_granted: false,
                    authenticated_transport_granted: false,
                    deployment_authority_granted: false,
                    report_digest: [0; 32],
                };
                report.report_digest =
                    digest_without(b"execution-certification-report-v1", &report, |v| {
                        v.report_digest = [0; 32];
                    });
                self.report = Some(report.clone());
                Ok(ExecutionDetail::Finalized(Box::new(report)))
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ExecutionSnapshot {
        ExecutionSnapshot {
            accepted_commands: self.accepted_commands,
            plan_id: self.plan.as_ref().map(|v| v.plan_id),
            dry_run_count: self.dry_runs.len(),
            certified: self.certified,
            completed_step_count: self.completed_steps,
            active_intent: self.active_intent.clone(),
            last_report: self.report.clone(),
            halted: self.halted.is_some(),
            digest: self.state_digest(),
        }
    }
    #[must_use]
    pub const fn is_halted(&self) -> bool {
        self.halted.is_some()
    }
    fn halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }
    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"deployment-execution-intent-state-v1");
        hash_value(
            &mut hasher,
            &(
                &self.policy,
                &self.plan,
                &self.dry_runs,
                self.dry_run_chain_digest,
                self.certified,
                self.completed_steps,
                &self.active_intent,
                &self.consumed_intents,
                &self.report,
            ),
        );
        for (id, (content, outcome)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_value(&mut hasher, outcome);
        }
        hash_value(
            &mut hasher,
            &(
                self.accepted_commands,
                self.last_recorded_at_ns,
                &self.halted,
            ),
        );
        *hasher.finalize().as_bytes()
    }
}

fn validate_policy(policy: &ExecutionIntentPolicy) -> Result<(), Error> {
    if policy.maximum_readiness_age_ns <= 0
        || policy.maximum_plan_age_ns <= 0
        || policy.maximum_intent_lifetime_ns <= 0
        || policy.maximum_steps == 0
        || policy.maximum_steps > MAX_ITEMS
        || policy.maximum_regions == 0
        || policy.maximum_regions > MAX_ITEMS
        || policy.maximum_resources == 0
        || policy.maximum_resources > MAX_ITEMS
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn valid_plan(plan: &ExecutionIntentPlan, policy: &ExecutionIntentPolicy, now: i64) -> bool {
    let record = &plan.readiness_record;
    let contract = &plan.executor_contract;
    let expected_readiness_expiry = record
        .finalized_at_ns
        .checked_add(policy.maximum_readiness_age_ns);
    plan.plan_id != [0; 32]
        && plan.verify_digest(policy)
        && record.verify_digest()
        && record.status == ReadinessStatus::ProductionChangeReady
        && record.reasons.is_empty()
        && record.operator_execution_required
        && !record.credential_material_created
        && !record.authentication_authority_granted
        && !record.deployment_authority_granted
        && !record.rollback_execution_authority_granted
        && !record.traffic_authority_granted
        && !record.cloud_control_authority_granted
        && !record.live_trading_authority_granted
        && plan.subject.verify_digest()
        && record.subject_digest == plan.subject.subject_digest
        && contract.verify_digest()
        && contract.privilege_ceiling.verify_digest()
        && contract.subject_digest == plan.subject.subject_digest
        && contract.executor_binary_digest != [0; 32]
        && contract.executor_schema_digest != [0; 32]
        && contract.audit_policy_digest != [0; 32]
        && !contract.credential_loading
        && !contract.signature_production
        && !contract.authenticated_transport
        && !contract.external_submission
        && valid_ceiling(&contract.privilege_ceiling, &plan.subject, policy)
        && !plan.steps.is_empty()
        && plan.steps.len() <= policy.maximum_steps
        && plan
            .steps
            .iter()
            .enumerate()
            .all(|(i, step)| valid_step(step, i, &contract.privilege_ceiling))
        && plan.created_at_ns >= record.finalized_at_ns
        && plan.created_at_ns <= now
        && plan.expires_at_ns > plan.created_at_ns
        && expected_readiness_expiry == Some(plan.readiness_valid_until_ns)
        && plan.readiness_valid_until_ns >= plan.expires_at_ns
        && current(plan, policy, now)
}

fn valid_ceiling(
    ceiling: &PrivilegeCeiling,
    subject: &ProductionChangeSubject,
    policy: &ExecutionIntentPolicy,
) -> bool {
    let subject_resources: BTreeSet<_> = [
        subject.release_digest,
        subject.binary_digest,
        subject.configuration_digest,
        subject.infrastructure_digest,
        subject.observability_digest,
    ]
    .into_iter()
    .chain(subject.plan_digests.iter().copied())
    .chain(subject.certificate_digests.iter().copied())
    .chain(subject.preflight_report_digests.iter().copied())
    .chain(subject.rollback_package_digests.iter().copied())
    .collect();
    !ceiling.allowed_operations.is_empty()
        && ceiling.allowed_operations.len() <= MAX_ITEMS
        && canonical(&ceiling.allowed_operations)
        && !ceiling.allowed_regions.is_empty()
        && ceiling.allowed_regions.len() <= policy.maximum_regions
        && canonical(&ceiling.allowed_regions)
        && ceiling
            .allowed_regions
            .iter()
            .all(|v| !v.is_empty() && v.len() <= 128 && !v.contains('*'))
        && !ceiling.allowed_resource_digests.is_empty()
        && ceiling.allowed_resource_digests.len() <= policy.maximum_resources
        && canonical(&ceiling.allowed_resource_digests)
        && ceiling
            .allowed_resource_digests
            .iter()
            .all(|v| *v != [0; 32] && subject_resources.contains(v))
        && !ceiling.wildcard_access
        && !ceiling.secret_read
        && !ceiling.cluster_admin
        && !ceiling.arbitrary_exec
        && !ceiling.privilege_escalation
        && !ceiling.cross_region_mutation
        && !ceiling.credential_loading
}

fn valid_step(step: &ExecutionStep, index: usize, ceiling: &PrivilegeCeiling) -> bool {
    step.index as usize == index
        && step.verify_digest()
        && ceiling.allowed_regions.binary_search(&step.region).is_ok()
        && ceiling
            .allowed_operations
            .binary_search(&step.operation)
            .is_ok()
        && ceiling
            .allowed_resource_digests
            .binary_search(&step.resource_digest)
            .is_ok()
}

fn current(plan: &ExecutionIntentPlan, policy: &ExecutionIntentPolicy, at: i64) -> bool {
    at >= plan.created_at_ns
        && at <= plan.expires_at_ns
        && at <= plan.readiness_valid_until_ns
        && at
            .checked_sub(plan.created_at_ns)
            .is_some_and(|v| v <= policy.maximum_plan_age_ns)
        && at
            .checked_sub(plan.readiness_record.finalized_at_ns)
            .is_some_and(|v| v <= policy.maximum_readiness_age_ns)
}

fn safe_intent(value: &ManualExecutionIntent) -> bool {
    value.one_use
        && value.manual_operator_required
        && !value.credential_material_created
        && !value.signature_authority_granted
        && !value.authenticated_transport_granted
        && !value.deployment_authority_granted
}

fn canonical<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|v| v[0] < v[1])
}
fn chain_digest(previous: [u8; 32], current: [u8; 32]) -> [u8; 32] {
    digest_json(b"executor-dry-run-chain-v1", &(previous, current))
}

fn digest_without<T: Clone + Serialize>(
    domain: &[u8],
    value: &T,
    clear: impl FnOnce(&mut T),
) -> [u8; 32] {
    let mut clone = value.clone();
    clear(&mut clone);
    digest_json(domain, &clone)
}
fn digest_json<T: Serialize + ?Sized>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_value(&mut hasher, value);
    *hasher.finalize().as_bytes()
}
fn hash_value<T: Serialize + ?Sized>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable execution-intent state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandWire {
    version: u16,
    command: ExecutionCommand,
}

/// Encodes one bounded, versioned canonical command.
///
/// # Errors
///
/// Rejects serialization failures and commands above the hard byte bound.
pub fn encode_command(command: &ExecutionCommand) -> Result<Vec<u8>, Error> {
    let bytes = serde_json::to_vec(&CommandWire {
        version: WIRE_VERSION,
        command: command.clone(),
    })
    .map_err(|e| Error::Json(e.to_string()))?;
    if bytes.len() > MAX_COMMAND_BYTES {
        Err(Error::CommandBound)
    } else {
        Ok(bytes)
    }
}

/// Decodes one bounded, versioned canonical command.
///
/// # Errors
///
/// Rejects oversized, malformed, unsupported, trailing, or noncanonical data.
pub fn decode_command(bytes: &[u8]) -> Result<ExecutionCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let wire = CommandWire::deserialize(&mut de).map_err(|e| Error::Json(e.to_string()))?;
    de.end().map_err(|e| Error::Json(e.to_string()))?;
    if wire.version != WIRE_VERSION {
        return Err(Error::Version(wire.version));
    }
    if serde_json::to_vec(&wire).map_err(|e| Error::Json(e.to_string()))? != bytes {
        return Err(Error::Json("noncanonical command".into()));
    }
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
