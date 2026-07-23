#![forbid(unsafe_code)]

//! Deterministic offline campaigns over independent deployment change plans.
//!
//! The crate replays sealed Phase 2.24 commands only. It has no credential,
//! network, control-plane, deployment, rollback-execution or trading path.

mod durable;
mod evidence;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, CampaignCheckpoint,
    CampaignRecovery, CampaignStorageError, DurableChangeCampaign,
};
pub use evidence::{read_evidence, write_evidence_create_new, ChangeCampaignEvidenceFileError};

use deployment_change_control::{
    ApprovalDecision, ApprovalRole, ChangeCommand, ChangeControlPolicy, ChangeDetail,
    ChangeReportStatus, DeploymentChangeControl, EmergencyTrigger, Error as ChangeError,
    PermissionKind,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_CASES_HARD: usize = 64;
const MAX_CHILD_COMMANDS_HARD: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CampaignCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeCampaignPolicy {
    pub minimum_independent_plans: usize,
    pub maximum_cases: usize,
    pub maximum_commands_per_case: usize,
    pub maximum_campaign_age_ns: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredScenario {
    MultiWindowCompletion,
    ApprovalRenewal,
    ApprovalExpiryDenial,
    PauseResume,
    SafeAbort,
    EmergencyRollback,
    RestartRecovery,
}

impl RequiredScenario {
    pub const ALL: [Self; 7] = [
        Self::MultiWindowCompletion,
        Self::ApprovalRenewal,
        Self::ApprovalExpiryDenial,
        Self::PauseResume,
        Self::SafeAbort,
        Self::EmergencyRollback,
        Self::RestartRecovery,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedCaseResult {
    Completed,
    ApprovalExpiryDenied,
    SafeAbort,
    EmergencyRolledBack,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeCampaignCase {
    pub case_id: [u8; 32],
    pub change_policy: ChangeControlPolicy,
    pub commands: Vec<ChangeCommand>,
    pub expected_result: ExpectedCaseResult,
    pub restart_after_commands: Option<usize>,
    pub case_digest: [u8; 32],
}

impl ChangeCampaignCase {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.case_digest = case_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.case_digest == case_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeCampaignManifest {
    pub campaign_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub cases: Vec<ChangeCampaignCase>,
    pub required_scenarios: Vec<RequiredScenario>,
    pub expected_case_schedule_digest: [u8; 32],
    pub campaign_policy_digest: [u8; 32],
    pub manifest_digest: [u8; 32],
}

impl ChangeCampaignManifest {
    #[must_use]
    pub fn sealed(mut self, policy: &ChangeCampaignPolicy) -> Self {
        self.required_scenarios.sort();
        self.expected_case_schedule_digest = case_schedule_digest(&self.cases);
        self.campaign_policy_digest = digest_json(b"deployment-change-campaign-policy-v1", policy);
        self.manifest_digest = manifest_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &ChangeCampaignPolicy) -> bool {
        self.manifest_digest == manifest_digest(self)
            && self.campaign_policy_digest
                == digest_json(b"deployment-change-campaign-policy-v1", policy)
            && self.expected_case_schedule_digest == case_schedule_digest(&self.cases)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeCaseResult {
    pub case_id: [u8; 32],
    pub plan_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub certificate_digest: [u8; 32],
    pub preflight_report_digest: [u8; 32],
    pub rollback_package_digest: [u8; 32],
    pub expected_result: ExpectedCaseResult,
    pub accepted_child_commands: u64,
    pub restart_reconstructed: bool,
    pub child_halted_as_expected: bool,
    pub report_digest: Option<[u8; 32]>,
    pub child_state_digest: [u8; 32],
    pub covered_scenarios: Vec<RequiredScenario>,
    pub result_digest: [u8; 32],
}

impl ChangeCaseResult {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.result_digest == case_result_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignEvidenceStatus {
    OperatorReviewEligible,
    NotEligible,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CampaignEvidenceReason {
    CasesIncomplete,
    CaseScheduleMismatch,
    IndependentPlanFloor,
    ScenarioMissing(RequiredScenario),
    InvalidCaseResult,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ChangeCampaignEvidence {
    pub evidence_id: [u8; 32],
    pub campaign_id: [u8; 32],
    pub manifest_digest: [u8; 32],
    pub evaluated_at_ns: i64,
    pub status: CampaignEvidenceStatus,
    pub reasons: Vec<CampaignEvidenceReason>,
    pub required_scenarios: Vec<RequiredScenario>,
    pub covered_scenarios: Vec<RequiredScenario>,
    pub case_count: usize,
    pub completed_case_count: usize,
    pub independent_plan_count: usize,
    pub case_schedule_digest: [u8; 32],
    pub case_result_chain_digest: [u8; 32],
    pub restart_reconstruction_count: usize,
    pub approval_set_count: usize,
    pub plan_digests: Vec<[u8; 32]>,
    pub certificate_digests: Vec<[u8; 32]>,
    pub preflight_report_digests: Vec<[u8; 32]>,
    pub rollback_package_digests: Vec<[u8; 32]>,
    pub operator_decision_required: bool,
    pub credential_material_created: bool,
    pub authentication_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub rollback_execution_authority_granted: bool,
    pub traffic_authority_granted: bool,
    pub cloud_control_authority_granted: bool,
    pub live_trading_authority_granted: bool,
    pub evidence_digest: [u8; 32],
}

impl ChangeCampaignEvidence {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evidence_digest == evidence_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CampaignCommand {
    Register {
        command_id: CampaignCommandId,
        manifest: Box<ChangeCampaignManifest>,
        recorded_at_ns: i64,
    },
    RunNextCase {
        command_id: CampaignCommandId,
        campaign_id: [u8; 32],
        case_id: [u8; 32],
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: CampaignCommandId,
        campaign_id: [u8; 32],
        evidence_id: [u8; 32],
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl CampaignCommand {
    #[must_use]
    pub const fn command_id(&self) -> CampaignCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::RunNextCase { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::RunNextCase { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CampaignDetail {
    Registered,
    CaseCompleted(Box<ChangeCaseResult>),
    Finalized(Box<ChangeCampaignEvidence>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignOutcome {
    pub command_id: CampaignCommandId,
    pub detail: CampaignDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CampaignSnapshot {
    pub accepted_commands: u64,
    pub campaign_id: Option<[u8; 32]>,
    pub completed_case_count: usize,
    pub covered_scenarios: BTreeSet<RequiredScenario>,
    pub case_schedule_digest: [u8; 32],
    pub case_result_chain_digest: [u8; 32],
    pub last_evidence: Option<ChangeCampaignEvidence>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("change campaign configuration is invalid")]
    Config,
    #[error("change campaign timestamp is invalid or regressed")]
    Timestamp,
    #[error("change campaign command exceeds its canonical bound")]
    CommandBound,
    #[error("change campaign JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported change campaign command version: {0}")]
    Version(u16),
    #[error("change campaign command id was reused for different content")]
    IdempotencyConflict,
    #[error("change campaign manifest is invalid")]
    Manifest,
    #[error("change campaign case is invalid, substituted, or out of order")]
    Case,
    #[error("change campaign child failed unexpectedly: {0}")]
    Child(String),
    #[error("change campaign restart reconstruction diverged")]
    Restart,
    #[error("change campaign evidence lifecycle is invalid")]
    Evidence,
    #[error("change campaign is already finalized")]
    Finalized,
    #[error("change campaign arithmetic overflow")]
    Overflow,
    #[error("change campaign is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct DeploymentChangeCampaign {
    policy: ChangeCampaignPolicy,
    manifest: Option<ChangeCampaignManifest>,
    results: Vec<ChangeCaseResult>,
    covered_scenarios: BTreeSet<RequiredScenario>,
    case_schedule_digest: [u8; 32],
    case_result_chain_digest: [u8; 32],
    approval_plan_ids: BTreeSet<[u8; 32]>,
    evidence: Option<ChangeCampaignEvidence>,
    processed: BTreeMap<CampaignCommandId, ([u8; 32], CampaignOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl DeploymentChangeCampaign {
    /// Creates one empty offline campaign owner.
    ///
    /// # Errors
    ///
    /// Rejects zero, excessive, or inconsistent bounds.
    pub fn new(policy: ChangeCampaignPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            manifest: None,
            results: Vec::new(),
            covered_scenarios: BTreeSet::new(),
            case_schedule_digest: [0; 32],
            case_result_chain_digest: [0; 32],
            approval_plan_ids: BTreeSet::new(),
            evidence: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic campaign command.
    ///
    /// # Errors
    ///
    /// Integrity, chronology, child, restart, or lifecycle failures halt.
    pub fn apply(&mut self, command: &CampaignCommand) -> Result<CampaignOutcome, Error> {
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
        let mut outcome = CampaignOutcome {
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

    fn apply_fresh(&mut self, command: &CampaignCommand) -> Result<CampaignDetail, Error> {
        if self.evidence.is_some() {
            return Err(Error::Finalized);
        }
        match command {
            CampaignCommand::Register {
                manifest,
                recorded_at_ns,
                ..
            } => self.register(manifest, *recorded_at_ns),
            CampaignCommand::RunNextCase {
                campaign_id,
                case_id,
                recorded_at_ns,
                ..
            } => self.run_next_case(*campaign_id, *case_id, *recorded_at_ns),
            CampaignCommand::Finalize {
                campaign_id,
                evidence_id,
                evaluated_at_ns,
                recorded_at_ns,
                ..
            } => self.finalize(
                *campaign_id,
                *evidence_id,
                *evaluated_at_ns,
                *recorded_at_ns,
            ),
        }
    }

    fn register(
        &mut self,
        manifest: &ChangeCampaignManifest,
        at: i64,
    ) -> Result<CampaignDetail, Error> {
        if self.manifest.is_some() || at != manifest.created_at_ns {
            return Err(Error::Manifest);
        }
        validate_manifest(manifest, &self.policy)?;
        self.manifest = Some(manifest.clone());
        Ok(CampaignDetail::Registered)
    }

    fn run_next_case(
        &mut self,
        campaign_id: [u8; 32],
        case_id: [u8; 32],
        at: i64,
    ) -> Result<CampaignDetail, Error> {
        let manifest = self.manifest.as_ref().ok_or(Error::Manifest)?;
        if campaign_id != manifest.campaign_id || at > manifest.expires_at_ns {
            return Err(Error::Case);
        }
        let case = manifest.cases.get(self.results.len()).ok_or(Error::Case)?;
        if case.case_id != case_id
            || case.commands.last().map(ChangeCommand::recorded_at_ns) != Some(at)
        {
            return Err(Error::Case);
        }
        let execution = execute_case(case)?;
        let result = execution.result;
        self.covered_scenarios
            .extend(result.covered_scenarios.iter().copied());
        if execution.has_approval_pair {
            self.approval_plan_ids.insert(result.plan_id);
            if self.approval_plan_ids.len() >= 2 {
                self.covered_scenarios
                    .insert(RequiredScenario::ApprovalRenewal);
            }
        }
        self.case_schedule_digest =
            append_case_schedule(self.case_schedule_digest, case.case_id, case.case_digest);
        self.case_result_chain_digest = append_case_result(
            self.case_result_chain_digest,
            case.case_id,
            result.result_digest,
        );
        self.results.push(result.clone());
        Ok(CampaignDetail::CaseCompleted(Box::new(result)))
    }

    fn finalize(
        &mut self,
        campaign_id: [u8; 32],
        evidence_id: [u8; 32],
        evaluated_at: i64,
        recorded_at: i64,
    ) -> Result<CampaignDetail, Error> {
        let manifest = self.manifest.as_ref().ok_or(Error::Manifest)?;
        if campaign_id != manifest.campaign_id
            || evidence_id == [0; 32]
            || evaluated_at < manifest.created_at_ns
            || self
                .last_recorded_at_ns
                .is_some_and(|last| evaluated_at < last)
            || evaluated_at > recorded_at
        {
            return Err(Error::Evidence);
        }
        let required: BTreeSet<_> = manifest.required_scenarios.iter().copied().collect();
        let mut reasons = BTreeSet::new();
        if self.results.len() != manifest.cases.len() {
            reasons.insert(CampaignEvidenceReason::CasesIncomplete);
        }
        if self.case_schedule_digest != manifest.expected_case_schedule_digest {
            reasons.insert(CampaignEvidenceReason::CaseScheduleMismatch);
        }
        if self.approval_plan_ids.len() < self.policy.minimum_independent_plans {
            reasons.insert(CampaignEvidenceReason::IndependentPlanFloor);
        }
        for scenario in required.difference(&self.covered_scenarios) {
            reasons.insert(CampaignEvidenceReason::ScenarioMissing(*scenario));
        }
        if self.results.iter().any(|result| !result.verify_digest()) {
            reasons.insert(CampaignEvidenceReason::InvalidCaseResult);
        }
        let reasons: Vec<_> = reasons.into_iter().collect();
        let mut evidence = ChangeCampaignEvidence {
            evidence_id,
            campaign_id,
            manifest_digest: manifest.manifest_digest,
            evaluated_at_ns: evaluated_at,
            status: if reasons.is_empty() {
                CampaignEvidenceStatus::OperatorReviewEligible
            } else {
                CampaignEvidenceStatus::NotEligible
            },
            reasons,
            required_scenarios: required.into_iter().collect(),
            covered_scenarios: self.covered_scenarios.iter().copied().collect(),
            case_count: manifest.cases.len(),
            completed_case_count: self.results.len(),
            independent_plan_count: self.approval_plan_ids.len(),
            case_schedule_digest: self.case_schedule_digest,
            case_result_chain_digest: self.case_result_chain_digest,
            restart_reconstruction_count: self
                .results
                .iter()
                .filter(|result| result.restart_reconstructed)
                .count(),
            approval_set_count: self.approval_plan_ids.len(),
            plan_digests: unique_result_subjects(&self.results, |result| result.plan_digest),
            certificate_digests: unique_result_subjects(&self.results, |result| {
                result.certificate_digest
            }),
            preflight_report_digests: unique_result_subjects(&self.results, |result| {
                result.preflight_report_digest
            }),
            rollback_package_digests: unique_result_subjects(&self.results, |result| {
                result.rollback_package_digest
            }),
            operator_decision_required: true,
            credential_material_created: false,
            authentication_authority_granted: false,
            deployment_authority_granted: false,
            rollback_execution_authority_granted: false,
            traffic_authority_granted: false,
            cloud_control_authority_granted: false,
            live_trading_authority_granted: false,
            evidence_digest: [0; 32],
        };
        evidence.evidence_digest = evidence_digest(&evidence);
        self.evidence = Some(evidence.clone());
        Ok(CampaignDetail::Finalized(Box::new(evidence)))
    }

    #[must_use]
    pub fn snapshot(&self) -> CampaignSnapshot {
        CampaignSnapshot {
            accepted_commands: self.accepted_commands,
            campaign_id: self.manifest.as_ref().map(|manifest| manifest.campaign_id),
            completed_case_count: self.results.len(),
            covered_scenarios: self.covered_scenarios.clone(),
            case_schedule_digest: self.case_schedule_digest,
            case_result_chain_digest: self.case_result_chain_digest,
            last_evidence: self.evidence.clone(),
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
        hasher.update(b"deployment-change-campaign-state-v2");
        hash_json(&mut hasher, &self.policy);
        hash_json(&mut hasher, &self.manifest);
        hash_json(&mut hasher, &self.results);
        hash_json(&mut hasher, &self.covered_scenarios);
        hasher.update(&self.case_schedule_digest);
        hasher.update(&self.case_result_chain_digest);
        hash_json(&mut hasher, &self.approval_plan_ids);
        hash_json(&mut hasher, &self.evidence);
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

struct CaseExecution {
    result: ChangeCaseResult,
    has_approval_pair: bool,
}

#[allow(clippy::too_many_lines)]
fn execute_case(case: &ChangeCampaignCase) -> Result<CaseExecution, Error> {
    let subject = case_plan_subject(case)?;
    let mut owner = DeploymentChangeControl::new(case.change_policy.clone())
        .map_err(|error| Error::Child(error.to_string()))?;
    let mut coverage = BTreeSet::new();
    let mut change_windows = BTreeSet::new();
    let mut pause_seen = false;
    let mut approval_operators = BTreeSet::new();
    let mut approval_roles = BTreeSet::new();
    let mut expected_expiry_halt = false;
    for (index, command) in case.commands.iter().enumerate() {
        match owner.apply(command) {
            Ok(outcome) => match &outcome.detail {
                ChangeDetail::ApprovalRecorded(role) => {
                    approval_roles.insert(*role);
                    if let ChangeCommand::RecordApproval { approval, .. } = command {
                        if approval.decision == ApprovalDecision::Approve {
                            approval_operators.insert(approval.operator_id);
                        }
                    }
                }
                ChangeDetail::PermissionIssued(permission)
                    if permission.kind == PermissionKind::Change =>
                {
                    if let Some(window) = subject.windows.iter().find(|window| {
                        permission.issued_at_ns >= window.0 && permission.issued_at_ns < window.1
                    }) {
                        change_windows.insert(window.2);
                    }
                }
                ChangeDetail::Paused => pause_seen = true,
                ChangeDetail::Resumed if pause_seen => {
                    coverage.insert(RequiredScenario::PauseResume);
                }
                _ => {}
            },
            Err(ChangeError::Approval)
                if case.expected_result == ExpectedCaseResult::ApprovalExpiryDenied
                    && index + 1 == case.commands.len()
                    && approval_expired_before(command, &case.commands[..index]) =>
            {
                expected_expiry_halt = owner.is_halted();
                coverage.insert(RequiredScenario::ApprovalExpiryDenial);
            }
            Err(error) => return Err(Error::Child(error.to_string())),
        }
        if case.restart_after_commands == Some(index + 1) {
            let mut recovered = DeploymentChangeControl::new(case.change_policy.clone())
                .map_err(|error| Error::Child(error.to_string()))?;
            for prefix in &case.commands[..=index] {
                recovered
                    .apply(prefix)
                    .map_err(|error| Error::Child(error.to_string()))?;
            }
            if recovered.snapshot().digest != owner.snapshot().digest {
                return Err(Error::Restart);
            }
            owner = recovered;
            coverage.insert(RequiredScenario::RestartRecovery);
        }
    }
    if change_windows.len() >= 2 {
        coverage.insert(RequiredScenario::MultiWindowCompletion);
    }
    let snapshot = owner.snapshot();
    let report_digest = match case.expected_result {
        ExpectedCaseResult::ApprovalExpiryDenied => {
            if !expected_expiry_halt || !snapshot.halted || snapshot.last_report.is_some() {
                return Err(Error::Case);
            }
            None
        }
        expected => {
            let report = snapshot.last_report.as_ref().ok_or(Error::Case)?;
            if !report.verify_digest() || !report_has_zero_authority(report) {
                return Err(Error::Case);
            }
            match expected {
                ExpectedCaseResult::Completed
                    if report.status == ChangeReportStatus::SimulatedHandoffsCompleted => {}
                ExpectedCaseResult::SafeAbort
                    if report.status == ChangeReportStatus::SimulatedAborted
                        && report.consumed_change_steps.is_empty() =>
                {
                    coverage.insert(RequiredScenario::SafeAbort);
                }
                ExpectedCaseResult::EmergencyRolledBack
                    if report.status == ChangeReportStatus::SimulatedRolledBack
                        && !report.consumed_change_steps.is_empty()
                        && report.rolled_back_steps
                            == report
                                .consumed_change_steps
                                .iter()
                                .rev()
                                .copied()
                                .collect::<Vec<_>>()
                        && report
                            .emergency_trigger
                            .is_some_and(|trigger| trigger != EmergencyTrigger::OperatorAbort) =>
                {
                    coverage.insert(RequiredScenario::EmergencyRollback);
                }
                _ => return Err(Error::Case),
            }
            Some(report.report_digest)
        }
    };
    let has_approval_pair = approval_roles.len() == 2
        && approval_roles.contains(&ApprovalRole::Release)
        && approval_roles.contains(&ApprovalRole::Risk)
        && approval_operators.len() == 2;
    let mut result = ChangeCaseResult {
        case_id: case.case_id,
        plan_id: subject.plan_id,
        plan_digest: subject.plan_digest,
        certificate_digest: subject.certificate_digest,
        preflight_report_digest: subject.preflight_report_digest,
        rollback_package_digest: subject.rollback_package_digest,
        expected_result: case.expected_result,
        accepted_child_commands: snapshot.accepted_commands,
        restart_reconstructed: case.restart_after_commands.is_some(),
        child_halted_as_expected: expected_expiry_halt,
        report_digest,
        child_state_digest: snapshot.digest,
        covered_scenarios: coverage.into_iter().collect(),
        result_digest: [0; 32],
    };
    result.result_digest = case_result_digest(&result);
    Ok(CaseExecution {
        result,
        has_approval_pair,
    })
}

fn validate_policy(policy: &ChangeCampaignPolicy) -> Result<(), Error> {
    if policy.minimum_independent_plans < 2
        || policy.maximum_cases < policy.minimum_independent_plans
        || policy.maximum_cases > MAX_CASES_HARD
        || policy.maximum_commands_per_case == 0
        || policy.maximum_commands_per_case > MAX_CHILD_COMMANDS_HARD
        || policy.maximum_campaign_age_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn validate_manifest(
    manifest: &ChangeCampaignManifest,
    policy: &ChangeCampaignPolicy,
) -> Result<(), Error> {
    if manifest.campaign_id == [0; 32]
        || manifest.created_at_ns < 0
        || manifest.expires_at_ns <= manifest.created_at_ns
        || manifest.expires_at_ns - manifest.created_at_ns > policy.maximum_campaign_age_ns
        || manifest.cases.len() < policy.minimum_independent_plans
        || manifest.cases.len() > policy.maximum_cases
        || manifest.required_scenarios != RequiredScenario::ALL
        || !manifest.verify_digest(policy)
    {
        return Err(Error::Manifest);
    }
    let mut case_ids = BTreeSet::new();
    let mut plan_ids = BTreeSet::new();
    let mut plan_digests = BTreeSet::new();
    for case in &manifest.cases {
        let command_ids: BTreeSet<_> = case
            .commands
            .iter()
            .map(ChangeCommand::command_id)
            .collect();
        if case.case_id == [0; 32]
            || !case.verify_digest()
            || case.commands.is_empty()
            || case.commands.len() > policy.maximum_commands_per_case
            || command_ids.len() != case.commands.len()
            || case
                .commands
                .first()
                .is_some_and(|command| command.recorded_at_ns() < manifest.created_at_ns)
            || case
                .commands
                .last()
                .is_some_and(|command| command.recorded_at_ns() > manifest.expires_at_ns)
            || !case
                .commands
                .windows(2)
                .all(|pair| pair[0].recorded_at_ns() <= pair[1].recorded_at_ns())
            || case
                .restart_after_commands
                .is_some_and(|at| at == 0 || at >= case.commands.len())
        {
            return Err(Error::Case);
        }
        let subject = case_plan_subject(case)?;
        if !case_ids.insert(case.case_id)
            || !plan_ids.insert(subject.plan_id)
            || !plan_digests.insert(subject.plan_digest)
            || case
                .commands
                .iter()
                .any(|command| command_plan_id(command) != subject.plan_id)
        {
            return Err(Error::Case);
        }
        execute_case(case)?;
    }
    Ok(())
}

type WindowSubject = (i64, i64, [u8; 32]);

struct CasePlanSubject {
    plan_id: [u8; 32],
    plan_digest: [u8; 32],
    certificate_digest: [u8; 32],
    preflight_report_digest: [u8; 32],
    rollback_package_digest: [u8; 32],
    windows: Vec<WindowSubject>,
}

fn case_plan_subject(case: &ChangeCampaignCase) -> Result<CasePlanSubject, Error> {
    let Some(ChangeCommand::Register { plan, .. }) = case.commands.first() else {
        return Err(Error::Case);
    };
    Ok(CasePlanSubject {
        plan_id: plan.plan_id,
        plan_digest: plan.plan_digest,
        certificate_digest: plan.certificate.report_digest,
        preflight_report_digest: plan.certificate.preflight_report_digest,
        rollback_package_digest: plan.certificate.rollback_package_digest,
        windows: plan
            .windows
            .iter()
            .map(|window| (window.starts_at_ns, window.ends_at_ns, window.window_id))
            .collect(),
    })
}

fn command_plan_id(command: &ChangeCommand) -> [u8; 32] {
    match command {
        ChangeCommand::Register { plan, .. } => plan.plan_id,
        ChangeCommand::RecordApproval { approval, .. } => approval.plan_id,
        ChangeCommand::IssuePermission { plan_id, .. }
        | ChangeCommand::ConsumePermission { plan_id, .. }
        | ChangeCommand::Pause { plan_id, .. }
        | ChangeCommand::Resume { plan_id, .. }
        | ChangeCommand::Abort { plan_id, .. }
        | ChangeCommand::SignalEmergency { plan_id, .. }
        | ChangeCommand::Finalize { plan_id, .. } => *plan_id,
    }
}

fn approval_expired_before(command: &ChangeCommand, prefix: &[ChangeCommand]) -> bool {
    let ChangeCommand::IssuePermission { recorded_at_ns, .. } = command else {
        return false;
    };
    let approvals: Vec<_> = prefix
        .iter()
        .filter_map(|item| match item {
            ChangeCommand::RecordApproval { approval, .. }
                if approval.decision == ApprovalDecision::Approve =>
            {
                Some(approval)
            }
            _ => None,
        })
        .collect();
    approvals.len() == 2
        && approvals
            .iter()
            .any(|approval| *recorded_at_ns > approval.valid_until_ns)
}

fn report_has_zero_authority(report: &deployment_change_control::ChangeControlReport) -> bool {
    report.manual_operator_execution_required
        && !report.credential_material_created
        && !report.authentication_authority_granted
        && !report.deployment_authority_granted
        && !report.rollback_execution_authority_granted
        && !report.traffic_authority_granted
        && !report.cloud_control_authority_granted
        && !report.live_trading_authority_granted
}

fn unique_result_subjects(
    results: &[ChangeCaseResult],
    select: impl Fn(&ChangeCaseResult) -> [u8; 32],
) -> Vec<[u8; 32]> {
    results
        .iter()
        .map(select)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn case_schedule_digest(cases: &[ChangeCampaignCase]) -> [u8; 32] {
    cases.iter().fold([0; 32], |head, case| {
        append_case_schedule(head, case.case_id, case.case_digest)
    })
}

fn append_case_schedule(head: [u8; 32], case_id: [u8; 32], case_digest: [u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"deployment-change-case-schedule-v1");
    hasher.update(&head);
    hasher.update(&case_id);
    hasher.update(&case_digest);
    *hasher.finalize().as_bytes()
}

fn append_case_result(head: [u8; 32], case_id: [u8; 32], result: [u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"deployment-change-case-result-chain-v1");
    hasher.update(&head);
    hasher.update(&case_id);
    hasher.update(&result);
    *hasher.finalize().as_bytes()
}

fn case_digest(value: &ChangeCampaignCase) -> [u8; 32] {
    let mut clone = value.clone();
    clone.case_digest = [0; 32];
    digest_json(b"deployment-change-campaign-case-v1", &clone)
}

fn manifest_digest(value: &ChangeCampaignManifest) -> [u8; 32] {
    let mut clone = value.clone();
    clone.manifest_digest = [0; 32];
    digest_json(b"deployment-change-campaign-manifest-v1", &clone)
}

fn case_result_digest(value: &ChangeCaseResult) -> [u8; 32] {
    let mut clone = value.clone();
    clone.result_digest = [0; 32];
    digest_json(b"deployment-change-campaign-result-v2", &clone)
}

fn evidence_digest(value: &ChangeCampaignEvidence) -> [u8; 32] {
    let mut clone = value.clone();
    clone.evidence_digest = [0; 32];
    digest_json(b"deployment-change-campaign-evidence-v2", &clone)
}

fn outcome_digest(value: &CampaignOutcome) -> [u8; 32] {
    let mut clone = value.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"deployment-change-campaign-outcome-v1", &clone)
}

fn digest_json<T: Serialize + ?Sized>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize + ?Sized>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable change-campaign state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandWire {
    version: u16,
    command: CampaignCommand,
}

/// Encodes one bounded versioned command.
///
/// # Errors
///
/// Rejects JSON or size failures.
pub fn encode_command(command: &CampaignCommand) -> Result<Vec<u8>, Error> {
    let bytes = serde_json::to_vec(&CommandWire {
        version: WIRE_VERSION,
        command: command.clone(),
    })
    .map_err(|error| Error::Json(error.to_string()))?;
    if bytes.len() > MAX_COMMAND_BYTES {
        Err(Error::CommandBound)
    } else {
        Ok(bytes)
    }
}

/// Decodes one canonical bounded versioned command.
///
/// # Errors
///
/// Rejects malformed, unsupported, oversized or noncanonical input.
pub fn decode_command(bytes: &[u8]) -> Result<CampaignCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let wire = CommandWire::deserialize(&mut deserializer)
        .map_err(|error| Error::Json(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| Error::Json(error.to_string()))?;
    if wire.version != WIRE_VERSION {
        return Err(Error::Version(wire.version));
    }
    if serde_json::to_vec(&wire).map_err(|error| Error::Json(error.to_string()))? != bytes {
        return Err(Error::Json("noncanonical command".to_owned()));
    }
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
