#![forbid(unsafe_code)]

//! Deterministic offline governance for production-change readiness.
//!
//! Readiness records are evidence only. This crate cannot authenticate,
//! deploy, route traffic, execute rollback, access a wallet, or trade.

mod durable;
mod record;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableProductionReadiness,
    ReadinessCheckpoint, ReadinessRecovery, ReadinessStorageError,
};
pub use record::{read_record, write_record_create_new, ProductionReadinessRecordFileError};

use deployment_change_campaign::{
    CampaignEvidenceStatus, ChangeCampaignEvidence, RequiredScenario,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_EVIDENCE_HARD: usize = 256;
const BASIS_POINTS_DENOMINATOR: u128 = 10_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ReadinessCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductionReadinessPolicy {
    pub maximum_evidence_records: usize,
    pub minimum_eligible_campaigns: usize,
    pub minimum_manifest_diversity: usize,
    pub minimum_schedule_diversity: usize,
    pub minimum_result_chain_diversity: usize,
    pub minimum_plan_diversity: usize,
    pub minimum_case_count: usize,
    pub minimum_independent_plan_count: usize,
    pub minimum_restart_count: usize,
    pub minimum_approval_set_count: usize,
    pub retention_basis_points: u16,
    pub maximum_evidence_age_ns: i64,
    pub maximum_candidate_age_ns: i64,
    pub maximum_decision_age_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessBaseline {
    pub campaign_count: usize,
    pub case_count: usize,
    pub independent_plan_count: usize,
    pub restart_count: usize,
    pub approval_set_count: usize,
    pub baseline_digest: [u8; 32],
}

impl ReadinessBaseline {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.baseline_digest = baseline_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.baseline_digest == baseline_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductionChangeSubject {
    pub release_digest: [u8; 32],
    pub binary_digest: [u8; 32],
    pub configuration_digest: [u8; 32],
    pub infrastructure_digest: [u8; 32],
    pub observability_digest: [u8; 32],
    pub plan_digests: Vec<[u8; 32]>,
    pub certificate_digests: Vec<[u8; 32]>,
    pub preflight_report_digests: Vec<[u8; 32]>,
    pub rollback_package_digests: Vec<[u8; 32]>,
    pub subject_digest: [u8; 32],
}

impl ProductionChangeSubject {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.plan_digests.sort_unstable();
        self.certificate_digests.sort_unstable();
        self.preflight_report_digests.sort_unstable();
        self.rollback_package_digests.sort_unstable();
        self.subject_digest = subject_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.subject_digest == subject_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductionReadinessCandidate {
    pub candidate_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub evidence: Vec<ChangeCampaignEvidence>,
    pub baseline: ReadinessBaseline,
    pub subject: ProductionChangeSubject,
    pub policy_digest: [u8; 32],
    pub candidate_digest: [u8; 32],
}

impl ProductionReadinessCandidate {
    #[must_use]
    pub fn sealed(mut self, policy: &ProductionReadinessPolicy) -> Self {
        self.evidence.sort_by_key(|item| item.evidence_digest);
        self.policy_digest = digest_json(b"production-readiness-policy-v1", policy);
        self.candidate_digest = candidate_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &ProductionReadinessPolicy) -> bool {
        self.policy_digest == digest_json(b"production-readiness-policy-v1", policy)
            && self.candidate_digest == candidate_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionRole {
    Release,
    Risk,
    Operations,
}

impl DecisionRole {
    const ALL: [Self; 3] = [Self::Release, Self::Risk, Self::Operations];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionValue {
    Approve,
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessDecision {
    pub decision_id: [u8; 32],
    pub candidate_id: [u8; 32],
    pub candidate_digest: [u8; 32],
    pub subject_digest: [u8; 32],
    pub role: DecisionRole,
    pub operator_id: [u8; 32],
    pub value: DecisionValue,
    pub decided_at_ns: i64,
    pub valid_until_ns: i64,
    pub decision_digest: [u8; 32],
}

impl ReadinessDecision {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.decision_digest = decision_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == decision_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    ProductionChangeReady,
    NotReady,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ReadinessReason {
    CandidateExpired,
    DuplicateEvidence,
    DuplicateCampaign,
    EvidenceIneligible,
    EvidenceStale,
    EligibleCampaignFloor,
    ManifestDiversity,
    ScheduleDiversity,
    ResultChainDiversity,
    PlanDiversity,
    CampaignRegression,
    CaseRegression,
    IndependentPlanRegression,
    RestartRegression,
    ApprovalSetRegression,
    DecisionMissing(DecisionRole),
    DecisionRejected(DecisionRole),
    DecisionExpired(DecisionRole),
    OperatorsNotDistinct,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ProductionReadinessRecord {
    pub record_id: [u8; 32],
    pub candidate_id: [u8; 32],
    pub candidate_digest: [u8; 32],
    pub subject_digest: [u8; 32],
    pub finalized_at_ns: i64,
    pub status: ReadinessStatus,
    pub reasons: Vec<ReadinessReason>,
    pub eligible_campaign_count: usize,
    pub case_count: usize,
    pub independent_plan_count: usize,
    pub restart_count: usize,
    pub approval_set_count: usize,
    pub manifest_diversity: usize,
    pub schedule_diversity: usize,
    pub result_chain_diversity: usize,
    pub plan_diversity: usize,
    pub regression_campaign_floor: usize,
    pub regression_case_floor: usize,
    pub regression_independent_plan_floor: usize,
    pub regression_restart_floor: usize,
    pub regression_approval_set_floor: usize,
    pub operator_execution_required: bool,
    pub credential_material_created: bool,
    pub authentication_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub rollback_execution_authority_granted: bool,
    pub traffic_authority_granted: bool,
    pub cloud_control_authority_granted: bool,
    pub live_trading_authority_granted: bool,
    pub record_digest: [u8; 32],
}

impl ProductionReadinessRecord {
    /// Seals a record for deterministic fixture construction and downstream
    /// verification. Sealing grants no deployment authority.
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.record_digest = record_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.record_digest == record_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ReadinessCommand {
    Register {
        command_id: ReadinessCommandId,
        candidate: Box<ProductionReadinessCandidate>,
        recorded_at_ns: i64,
    },
    RecordDecision {
        command_id: ReadinessCommandId,
        decision: ReadinessDecision,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: ReadinessCommandId,
        candidate_id: [u8; 32],
        record_id: [u8; 32],
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl ReadinessCommand {
    #[must_use]
    pub const fn command_id(&self) -> ReadinessCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::RecordDecision { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::RecordDecision { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ReadinessDetail {
    Registered,
    DecisionRecorded(DecisionRole),
    Finalized(Box<ProductionReadinessRecord>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessOutcome {
    pub command_id: ReadinessCommandId,
    pub detail: ReadinessDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadinessSnapshot {
    pub accepted_commands: u64,
    pub candidate_id: Option<[u8; 32]>,
    pub decision_count: usize,
    pub last_record: Option<ProductionReadinessRecord>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("production readiness configuration is invalid")]
    Config,
    #[error("production readiness timestamp is invalid or regressed")]
    Timestamp,
    #[error("production readiness command exceeds its canonical bound")]
    CommandBound,
    #[error("production readiness JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported production readiness command version: {0}")]
    Version(u16),
    #[error("production readiness command id was reused for different content")]
    IdempotencyConflict,
    #[error("production readiness candidate, baseline, or subject is invalid")]
    Candidate,
    #[error("production readiness evidence is invalid, conflicting, or authority-bearing")]
    Evidence,
    #[error("production readiness decision is invalid or substituted")]
    Decision,
    #[error("production readiness record lifecycle is invalid")]
    Record,
    #[error("production readiness is already finalized")]
    Finalized,
    #[error("production readiness arithmetic overflow")]
    Overflow,
    #[error("production readiness is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct ProductionChangeReadiness {
    policy: ProductionReadinessPolicy,
    candidate: Option<ProductionReadinessCandidate>,
    decisions: BTreeMap<DecisionRole, ReadinessDecision>,
    decision_ids: BTreeSet<[u8; 32]>,
    record: Option<ProductionReadinessRecord>,
    processed: BTreeMap<ReadinessCommandId, ([u8; 32], ReadinessOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl ProductionChangeReadiness {
    /// Creates an empty offline readiness owner.
    ///
    /// # Errors
    ///
    /// Rejects invalid evidence, diversity, time, or regression limits.
    pub fn new(policy: ProductionReadinessPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            candidate: None,
            decisions: BTreeMap::new(),
            decision_ids: BTreeSet::new(),
            record: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic readiness command.
    ///
    /// # Errors
    ///
    /// Integrity, chronology, evidence, decision, or lifecycle failures halt.
    pub fn apply(&mut self, command: &ReadinessCommand) -> Result<ReadinessOutcome, Error> {
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
        let mut outcome = ReadinessOutcome {
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

    fn apply_fresh(&mut self, command: &ReadinessCommand) -> Result<ReadinessDetail, Error> {
        if self.record.is_some() {
            return Err(Error::Finalized);
        }
        match command {
            ReadinessCommand::Register {
                candidate,
                recorded_at_ns,
                ..
            } => self.register(candidate, *recorded_at_ns),
            ReadinessCommand::RecordDecision {
                decision,
                recorded_at_ns,
                ..
            } => self.record_decision(decision, *recorded_at_ns),
            ReadinessCommand::Finalize {
                candidate_id,
                record_id,
                evaluated_at_ns,
                recorded_at_ns,
                ..
            } => self.finalize(*candidate_id, *record_id, *evaluated_at_ns, *recorded_at_ns),
        }
    }

    fn register(
        &mut self,
        candidate: &ProductionReadinessCandidate,
        at: i64,
    ) -> Result<ReadinessDetail, Error> {
        if self.candidate.is_some() || candidate.created_at_ns != at {
            return Err(Error::Candidate);
        }
        validate_candidate(candidate, &self.policy)?;
        self.candidate = Some(candidate.clone());
        Ok(ReadinessDetail::Registered)
    }

    fn record_decision(
        &mut self,
        decision: &ReadinessDecision,
        at: i64,
    ) -> Result<ReadinessDetail, Error> {
        let candidate = self.candidate.as_ref().ok_or(Error::Candidate)?;
        if decision.decision_id == [0; 32]
            || self.decision_ids.contains(&decision.decision_id)
            || self.decisions.contains_key(&decision.role)
            || decision.candidate_id != candidate.candidate_id
            || decision.candidate_digest != candidate.candidate_digest
            || decision.subject_digest != candidate.subject.subject_digest
            || decision.operator_id == [0; 32]
            || decision.decided_at_ns != at
            || decision.valid_until_ns <= at
            || decision.valid_until_ns > candidate.expires_at_ns
            || decision.valid_until_ns - at > self.policy.maximum_decision_age_ns
            || !decision.verify_digest()
        {
            return Err(Error::Decision);
        }
        self.decision_ids.insert(decision.decision_id);
        self.decisions.insert(decision.role, decision.clone());
        Ok(ReadinessDetail::DecisionRecorded(decision.role))
    }

    fn finalize(
        &mut self,
        candidate_id: [u8; 32],
        record_id: [u8; 32],
        evaluated_at: i64,
        recorded_at: i64,
    ) -> Result<ReadinessDetail, Error> {
        let candidate = self.candidate.as_ref().ok_or(Error::Candidate)?;
        if candidate_id != candidate.candidate_id
            || record_id == [0; 32]
            || evaluated_at < candidate.created_at_ns
            || evaluated_at > recorded_at
        {
            return Err(Error::Record);
        }
        let aggregate = evaluate_candidate(candidate, &self.policy, &self.decisions, evaluated_at)?;
        let mut record = ProductionReadinessRecord {
            record_id,
            candidate_id,
            candidate_digest: candidate.candidate_digest,
            subject_digest: candidate.subject.subject_digest,
            finalized_at_ns: evaluated_at,
            status: if aggregate.reasons.is_empty() {
                ReadinessStatus::ProductionChangeReady
            } else {
                ReadinessStatus::NotReady
            },
            reasons: aggregate.reasons,
            eligible_campaign_count: aggregate.eligible_campaign_count,
            case_count: aggregate.case_count,
            independent_plan_count: aggregate.independent_plan_count,
            restart_count: aggregate.restart_count,
            approval_set_count: aggregate.approval_set_count,
            manifest_diversity: aggregate.manifest_diversity,
            schedule_diversity: aggregate.schedule_diversity,
            result_chain_diversity: aggregate.result_chain_diversity,
            plan_diversity: aggregate.plan_diversity,
            regression_campaign_floor: aggregate.regression_campaign_floor,
            regression_case_floor: aggregate.regression_case_floor,
            regression_independent_plan_floor: aggregate.regression_independent_plan_floor,
            regression_restart_floor: aggregate.regression_restart_floor,
            regression_approval_set_floor: aggregate.regression_approval_set_floor,
            operator_execution_required: true,
            credential_material_created: false,
            authentication_authority_granted: false,
            deployment_authority_granted: false,
            rollback_execution_authority_granted: false,
            traffic_authority_granted: false,
            cloud_control_authority_granted: false,
            live_trading_authority_granted: false,
            record_digest: [0; 32],
        };
        record.record_digest = record_digest(&record);
        self.record = Some(record.clone());
        Ok(ReadinessDetail::Finalized(Box::new(record)))
    }

    #[must_use]
    pub fn snapshot(&self) -> ReadinessSnapshot {
        ReadinessSnapshot {
            accepted_commands: self.accepted_commands,
            candidate_id: self.candidate.as_ref().map(|item| item.candidate_id),
            decision_count: self.decisions.len(),
            last_record: self.record.clone(),
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
        hasher.update(b"production-change-readiness-state-v1");
        hash_json(&mut hasher, &self.policy);
        hash_json(&mut hasher, &self.candidate);
        hash_json(&mut hasher, &self.decisions);
        hash_json(&mut hasher, &self.decision_ids);
        hash_json(&mut hasher, &self.record);
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

#[derive(Default)]
struct Aggregate {
    reasons: Vec<ReadinessReason>,
    eligible_campaign_count: usize,
    case_count: usize,
    independent_plan_count: usize,
    restart_count: usize,
    approval_set_count: usize,
    manifest_diversity: usize,
    schedule_diversity: usize,
    result_chain_diversity: usize,
    plan_diversity: usize,
    regression_campaign_floor: usize,
    regression_case_floor: usize,
    regression_independent_plan_floor: usize,
    regression_restart_floor: usize,
    regression_approval_set_floor: usize,
}

#[allow(clippy::too_many_lines)]
fn evaluate_candidate(
    candidate: &ProductionReadinessCandidate,
    policy: &ProductionReadinessPolicy,
    decisions: &BTreeMap<DecisionRole, ReadinessDecision>,
    at: i64,
) -> Result<Aggregate, Error> {
    let mut reasons = BTreeSet::new();
    if at > candidate.expires_at_ns {
        reasons.insert(ReadinessReason::CandidateExpired);
    }
    let evidence_ids: BTreeSet<_> = candidate
        .evidence
        .iter()
        .map(|item| item.evidence_id)
        .collect();
    if evidence_ids.len() != candidate.evidence.len() {
        reasons.insert(ReadinessReason::DuplicateEvidence);
    }
    let campaign_ids: BTreeSet<_> = candidate
        .evidence
        .iter()
        .map(|item| item.campaign_id)
        .collect();
    if campaign_ids.len() != candidate.evidence.len() {
        reasons.insert(ReadinessReason::DuplicateCampaign);
    }
    let unique: BTreeMap<_, _> = candidate
        .evidence
        .iter()
        .map(|item| (item.campaign_id, item))
        .collect();
    let mut contributing = Vec::new();
    for evidence in unique.values() {
        let fresh = evidence.evaluated_at_ns <= at
            && at
                .checked_sub(evidence.evaluated_at_ns)
                .is_some_and(|age| age <= policy.maximum_evidence_age_ns);
        if !fresh {
            reasons.insert(ReadinessReason::EvidenceStale);
        }
        if evidence.status != CampaignEvidenceStatus::OperatorReviewEligible
            || !evidence.reasons.is_empty()
        {
            reasons.insert(ReadinessReason::EvidenceIneligible);
        }
        if fresh
            && evidence.status == CampaignEvidenceStatus::OperatorReviewEligible
            && evidence.reasons.is_empty()
        {
            contributing.push(*evidence);
        }
    }
    let eligible_campaign_count = contributing.len();
    let case_count = checked_sum(contributing.iter().map(|item| item.completed_case_count))?;
    let restart_count = checked_sum(
        contributing
            .iter()
            .map(|item| item.restart_reconstruction_count),
    )?;
    let reported_approval_set_count =
        checked_sum(contributing.iter().map(|item| item.approval_set_count))?;
    let manifest_diversity = contributing
        .iter()
        .map(|item| item.manifest_digest)
        .collect::<BTreeSet<_>>()
        .len();
    let schedule_diversity = contributing
        .iter()
        .map(|item| item.case_schedule_digest)
        .collect::<BTreeSet<_>>()
        .len();
    let result_chain_diversity = contributing
        .iter()
        .map(|item| item.case_result_chain_digest)
        .collect::<BTreeSet<_>>()
        .len();
    let plan_diversity = contributing
        .iter()
        .flat_map(|item| item.plan_digests.iter().copied())
        .collect::<BTreeSet<_>>()
        .len();
    let independent_plan_count = plan_diversity;
    let approval_set_count = reported_approval_set_count.min(plan_diversity);
    if eligible_campaign_count < policy.minimum_eligible_campaigns {
        reasons.insert(ReadinessReason::EligibleCampaignFloor);
    }
    if manifest_diversity < policy.minimum_manifest_diversity {
        reasons.insert(ReadinessReason::ManifestDiversity);
    }
    if schedule_diversity < policy.minimum_schedule_diversity {
        reasons.insert(ReadinessReason::ScheduleDiversity);
    }
    if result_chain_diversity < policy.minimum_result_chain_diversity {
        reasons.insert(ReadinessReason::ResultChainDiversity);
    }
    if plan_diversity < policy.minimum_plan_diversity {
        reasons.insert(ReadinessReason::PlanDiversity);
    }
    let baseline = &candidate.baseline;
    let regression_campaign_floor =
        retained_floor(baseline.campaign_count, policy.retention_basis_points)?;
    let regression_case_floor = retained_floor(baseline.case_count, policy.retention_basis_points)?;
    let regression_independent_plan_floor = retained_floor(
        baseline.independent_plan_count,
        policy.retention_basis_points,
    )?;
    let regression_restart_floor =
        retained_floor(baseline.restart_count, policy.retention_basis_points)?;
    let regression_approval_set_floor =
        retained_floor(baseline.approval_set_count, policy.retention_basis_points)?;
    if eligible_campaign_count < policy.minimum_eligible_campaigns
        || eligible_campaign_count < regression_campaign_floor
    {
        reasons.insert(ReadinessReason::CampaignRegression);
    }
    if case_count < policy.minimum_case_count || case_count < regression_case_floor {
        reasons.insert(ReadinessReason::CaseRegression);
    }
    if independent_plan_count < policy.minimum_independent_plan_count
        || independent_plan_count < regression_independent_plan_floor
    {
        reasons.insert(ReadinessReason::IndependentPlanRegression);
    }
    if restart_count < policy.minimum_restart_count || restart_count < regression_restart_floor {
        reasons.insert(ReadinessReason::RestartRegression);
    }
    if approval_set_count < policy.minimum_approval_set_count
        || approval_set_count < regression_approval_set_floor
    {
        reasons.insert(ReadinessReason::ApprovalSetRegression);
    }
    for role in DecisionRole::ALL {
        match decisions.get(&role) {
            None => {
                reasons.insert(ReadinessReason::DecisionMissing(role));
            }
            Some(decision) if decision.value == DecisionValue::Reject => {
                reasons.insert(ReadinessReason::DecisionRejected(role));
            }
            Some(decision)
                if at < decision.decided_at_ns
                    || at > decision.valid_until_ns
                    || at - decision.decided_at_ns > policy.maximum_decision_age_ns =>
            {
                reasons.insert(ReadinessReason::DecisionExpired(role));
            }
            Some(_) => {}
        }
    }
    if decisions
        .values()
        .map(|decision| decision.operator_id)
        .collect::<BTreeSet<_>>()
        .len()
        != DecisionRole::ALL.len()
    {
        reasons.insert(ReadinessReason::OperatorsNotDistinct);
    }
    Ok(Aggregate {
        reasons: reasons.into_iter().collect(),
        eligible_campaign_count,
        case_count,
        independent_plan_count,
        restart_count,
        approval_set_count,
        manifest_diversity,
        schedule_diversity,
        result_chain_diversity,
        plan_diversity,
        regression_campaign_floor,
        regression_case_floor,
        regression_independent_plan_floor,
        regression_restart_floor,
        regression_approval_set_floor,
    })
}

fn validate_policy(policy: &ProductionReadinessPolicy) -> Result<(), Error> {
    let diversity_max = policy
        .minimum_manifest_diversity
        .max(policy.minimum_schedule_diversity)
        .max(policy.minimum_result_chain_diversity);
    if policy.maximum_evidence_records == 0
        || policy.maximum_evidence_records > MAX_EVIDENCE_HARD
        || policy.minimum_eligible_campaigns == 0
        || policy.minimum_eligible_campaigns > policy.maximum_evidence_records
        || diversity_max > policy.maximum_evidence_records
        || policy.minimum_plan_diversity == 0
        || policy.minimum_case_count == 0
        || policy.minimum_independent_plan_count == 0
        || policy.minimum_restart_count == 0
        || policy.minimum_approval_set_count == 0
        || policy.retention_basis_points == 0
        || u128::from(policy.retention_basis_points) > BASIS_POINTS_DENOMINATOR
        || policy.maximum_evidence_age_ns <= 0
        || policy.maximum_candidate_age_ns <= 0
        || policy.maximum_decision_age_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn validate_candidate(
    candidate: &ProductionReadinessCandidate,
    policy: &ProductionReadinessPolicy,
) -> Result<(), Error> {
    if candidate.candidate_id == [0; 32]
        || candidate.created_at_ns < 0
        || candidate.expires_at_ns <= candidate.created_at_ns
        || candidate.expires_at_ns - candidate.created_at_ns > policy.maximum_candidate_age_ns
        || candidate.evidence.is_empty()
        || candidate.evidence.len() > policy.maximum_evidence_records
        || !candidate
            .evidence
            .windows(2)
            .all(|pair| pair[0].evidence_digest <= pair[1].evidence_digest)
        || !candidate.verify_digest(policy)
        || !candidate.baseline.verify_digest()
        || !valid_baseline(&candidate.baseline)
        || !valid_subject(&candidate.subject)
    {
        return Err(Error::Candidate);
    }
    let mut evidence_ids = BTreeMap::new();
    let mut campaign_ids = BTreeMap::new();
    for evidence in &candidate.evidence {
        if !valid_evidence(evidence, candidate.created_at_ns) {
            return Err(Error::Evidence);
        }
        if evidence_ids
            .insert(evidence.evidence_id, evidence.evidence_digest)
            .is_some_and(|existing| existing != evidence.evidence_digest)
            || campaign_ids
                .insert(evidence.campaign_id, evidence.evidence_digest)
                .is_some_and(|existing| existing != evidence.evidence_digest)
        {
            return Err(Error::Evidence);
        }
    }
    if candidate.subject.plan_digests
        != union_subjects(&candidate.evidence, |item| &item.plan_digests)
        || candidate.subject.certificate_digests
            != union_subjects(&candidate.evidence, |item| &item.certificate_digests)
        || candidate.subject.preflight_report_digests
            != union_subjects(&candidate.evidence, |item| &item.preflight_report_digests)
        || candidate.subject.rollback_package_digests
            != union_subjects(&candidate.evidence, |item| &item.rollback_package_digests)
    {
        return Err(Error::Candidate);
    }
    Ok(())
}

fn valid_baseline(baseline: &ReadinessBaseline) -> bool {
    baseline.campaign_count > 0
        && baseline.case_count > 0
        && baseline.independent_plan_count > 0
        && baseline.restart_count > 0
        && baseline.approval_set_count > 0
}

fn valid_subject(subject: &ProductionChangeSubject) -> bool {
    subject.verify_digest()
        && [
            subject.release_digest,
            subject.binary_digest,
            subject.configuration_digest,
            subject.infrastructure_digest,
            subject.observability_digest,
        ]
        .iter()
        .all(|digest| *digest != [0; 32])
        && valid_subject_set(&subject.plan_digests)
        && valid_subject_set(&subject.certificate_digests)
        && valid_subject_set(&subject.preflight_report_digests)
        && valid_subject_set(&subject.rollback_package_digests)
}

fn valid_subject_set(values: &[[u8; 32]]) -> bool {
    !values.is_empty()
        && values.iter().all(|value| *value != [0; 32])
        && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_evidence(evidence: &ChangeCampaignEvidence, candidate_created_at: i64) -> bool {
    evidence.verify_digest()
        && evidence.evidence_id != [0; 32]
        && evidence.campaign_id != [0; 32]
        && evidence.evaluated_at_ns <= candidate_created_at
        && evidence.operator_decision_required
        && !evidence.credential_material_created
        && !evidence.authentication_authority_granted
        && !evidence.deployment_authority_granted
        && !evidence.rollback_execution_authority_granted
        && !evidence.traffic_authority_granted
        && !evidence.cloud_control_authority_granted
        && !evidence.live_trading_authority_granted
        && evidence.completed_case_count <= evidence.case_count
        && evidence.restart_reconstruction_count <= evidence.completed_case_count
        && evidence.independent_plan_count == evidence.plan_digests.len()
        && evidence.approval_set_count <= evidence.independent_plan_count
        && valid_evidence_subjects(evidence)
        && (evidence.status != CampaignEvidenceStatus::OperatorReviewEligible
            || (evidence.reasons.is_empty()
                && evidence.completed_case_count == evidence.case_count
                && evidence.required_scenarios == RequiredScenario::ALL
                && evidence.covered_scenarios == RequiredScenario::ALL))
}

fn valid_evidence_subjects(evidence: &ChangeCampaignEvidence) -> bool {
    valid_subject_set(&evidence.plan_digests)
        && valid_subject_set(&evidence.certificate_digests)
        && valid_subject_set(&evidence.preflight_report_digests)
        && valid_subject_set(&evidence.rollback_package_digests)
}

fn union_subjects(
    evidence: &[ChangeCampaignEvidence],
    select: impl Fn(&ChangeCampaignEvidence) -> &Vec<[u8; 32]>,
) -> Vec<[u8; 32]> {
    evidence
        .iter()
        .flat_map(|item| select(item).iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn checked_sum(mut values: impl Iterator<Item = usize>) -> Result<usize, Error> {
    values.try_fold(0_usize, |total, value| {
        total.checked_add(value).ok_or(Error::Overflow)
    })
}

fn retained_floor(baseline: usize, basis_points: u16) -> Result<usize, Error> {
    let numerator = (baseline as u128)
        .checked_mul(u128::from(basis_points))
        .ok_or(Error::Overflow)?;
    let rounded = numerator
        .checked_add(BASIS_POINTS_DENOMINATOR - 1)
        .ok_or(Error::Overflow)?
        / BASIS_POINTS_DENOMINATOR;
    usize::try_from(rounded).map_err(|_| Error::Overflow)
}

fn baseline_digest(value: &ReadinessBaseline) -> [u8; 32] {
    let mut clone = value.clone();
    clone.baseline_digest = [0; 32];
    digest_json(b"production-readiness-baseline-v1", &clone)
}

fn subject_digest(value: &ProductionChangeSubject) -> [u8; 32] {
    let mut clone = value.clone();
    clone.subject_digest = [0; 32];
    digest_json(b"production-change-subject-v1", &clone)
}

fn candidate_digest(value: &ProductionReadinessCandidate) -> [u8; 32] {
    let mut clone = value.clone();
    clone.candidate_digest = [0; 32];
    digest_json(b"production-readiness-candidate-v1", &clone)
}

fn decision_digest(value: &ReadinessDecision) -> [u8; 32] {
    let mut clone = value.clone();
    clone.decision_digest = [0; 32];
    digest_json(b"production-readiness-decision-v1", &clone)
}

fn record_digest(value: &ProductionReadinessRecord) -> [u8; 32] {
    let mut clone = value.clone();
    clone.record_digest = [0; 32];
    digest_json(b"production-readiness-record-v1", &clone)
}

fn outcome_digest(value: &ReadinessOutcome) -> [u8; 32] {
    let mut clone = value.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"production-readiness-outcome-v1", &clone)
}

fn digest_json<T: Serialize + ?Sized>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize + ?Sized>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable production-readiness state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandWire {
    version: u16,
    command: ReadinessCommand,
}

/// Encodes one bounded versioned readiness command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &ReadinessCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one canonical bounded versioned readiness command.
///
/// # Errors
///
/// Rejects malformed, unsupported, oversized or noncanonical input.
pub fn decode_command(bytes: &[u8]) -> Result<ReadinessCommand, Error> {
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
