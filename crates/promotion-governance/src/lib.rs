#![forbid(unsafe_code)]

//! Deterministic offline promotion governance over Phase 2.17 evidence.
//!
//! This crate can only produce a non-deploying canary-eligibility record. It
//! has no credential, signature, authenticated transport, RPC, wallet, relayer,
//! deployment, rollback-execution, order, transaction, or live authority.

mod durable;
mod evidence;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableGovernance,
    GovernanceCheckpoint, GovernanceRecovery, GovernanceStorageError,
};
pub use evidence::{read_canary_record, write_canary_record_create_new, CanaryRecordFileError};

use serde::{Deserialize, Serialize};
use shadow_session_campaign::{CampaignStatus, OperatorEvidenceBundle};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_BUNDLES_HARD: usize = 512;
const BASIS_POINTS: u64 = 10_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct GovernanceCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernancePolicy {
    pub max_bundles: usize,
    pub minimum_campaigns: usize,
    pub minimum_distinct_manifests: usize,
    pub minimum_distinct_schedules: usize,
    pub minimum_distinct_final_states: usize,
    pub minimum_total_sessions: u64,
    pub minimum_total_steps: u64,
    pub minimum_total_fault_cycles: u64,
    pub minimum_regression_retention_bps: u16,
    pub maximum_bundle_age_ns: i64,
    pub maximum_candidate_age_ns: i64,
    pub maximum_decision_age_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegressionBaseline {
    pub baseline_id: [u8; 32],
    pub campaign_count: u64,
    pub total_sessions: u64,
    pub total_steps: u64,
    pub total_fault_cycles: u64,
    pub baseline_digest: [u8; 32],
}

impl RegressionBaseline {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.baseline_digest = regression_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.baseline_digest == regression_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseArtifacts {
    pub release_id: [u8; 32],
    pub source_digest: [u8; 32],
    pub binary_digest: [u8; 32],
    pub toolchain_digest: [u8; 32],
    pub dependency_lock_digest: [u8; 32],
    pub sbom_digest: [u8; 32],
    pub configuration_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
}

impl ReleaseArtifacts {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.artifacts_digest = artifacts_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.artifacts_digest == artifacts_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackCriteria {
    pub criteria_id: [u8; 32],
    pub rollback_target_digest: [u8; 32],
    pub maximum_canary_duration_ns: i64,
    pub maximum_unreconciled_ns: i64,
    pub maximum_unknown_state_ns: i64,
    pub maximum_session_loss_micros: i128,
    pub maximum_consecutive_faults: u64,
    pub require_capital_floor_halt: bool,
    pub require_reconciliation_halt: bool,
    pub criteria_digest: [u8; 32],
}

impl RollbackCriteria {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.criteria_digest = rollback_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.criteria_digest == rollback_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseCandidateSubmission {
    pub candidate_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub evidence: Vec<OperatorEvidenceBundle>,
    pub baseline: RegressionBaseline,
    pub artifacts: ReleaseArtifacts,
    pub rollback: RollbackCriteria,
    pub policy_digest: [u8; 32],
    pub evidence_set_digest: [u8; 32],
    pub candidate_digest: [u8; 32],
}

impl ReleaseCandidateSubmission {
    #[must_use]
    pub fn sealed(mut self, policy: &GovernancePolicy) -> Self {
        self.evidence.sort_by_key(|bundle| bundle.bundle_digest);
        self.policy_digest = digest_json(b"promotion-governance-policy-v1", policy);
        self.evidence_set_digest = evidence_set_digest(&self.evidence);
        self.candidate_digest = candidate_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &GovernancePolicy) -> bool {
        self.policy_digest == digest_json(b"promotion-governance-policy-v1", policy)
            && self.evidence_set_digest == evidence_set_digest(&self.evidence)
            && self.candidate_digest == candidate_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionRole {
    Risk,
    Release,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionVerdict {
    Approve,
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorDecision {
    pub decision_id: [u8; 32],
    pub operator_id: [u8; 32],
    pub role: DecisionRole,
    pub verdict: DecisionVerdict,
    pub candidate_digest: [u8; 32],
    pub evidence_set_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
    pub rollback_digest: [u8; 32],
    pub decided_at_ns: i64,
    pub valid_until_ns: i64,
    pub decision_digest: [u8; 32],
}

impl OperatorDecision {
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateAggregate {
    pub unique_campaigns: u64,
    pub distinct_manifests: u64,
    pub distinct_schedules: u64,
    pub distinct_final_states: u64,
    pub total_sessions: u64,
    pub total_steps: u64,
    pub total_fault_cycles: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum EligibilityReason {
    DuplicateEvidence,
    CampaignNotEligible([u8; 32]),
    CampaignStale([u8; 32]),
    InsufficientCampaigns,
    InsufficientManifestDiversity,
    InsufficientScheduleDiversity,
    InsufficientFinalStateDiversity,
    InsufficientSessions,
    InsufficientSteps,
    InsufficientFaultCycles,
    RegressionCampaignCount,
    RegressionSessionCount,
    RegressionStepCount,
    RegressionFaultCycles,
    MissingDecision(DecisionRole),
    RejectedDecision(DecisionRole),
    ExpiredDecision(DecisionRole),
    SameOperator,
    CandidateExpired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanaryStatus {
    CanaryEligible,
    NotEligible,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct CanaryEligibilityRecord {
    pub record_id: [u8; 32],
    pub candidate_id: [u8; 32],
    pub candidate_digest: [u8; 32],
    pub evidence_set_digest: [u8; 32],
    pub baseline_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
    pub rollback_digest: [u8; 32],
    pub policy_digest: [u8; 32],
    pub evaluated_at_ns: i64,
    pub valid_until_ns: i64,
    pub status: CanaryStatus,
    pub reasons: Vec<EligibilityReason>,
    pub aggregate: CandidateAggregate,
    pub risk_decision_digest: Option<[u8; 32]>,
    pub release_decision_digest: Option<[u8; 32]>,
    pub dual_control_complete: bool,
    pub operator_execution_required: bool,
    pub rollback_required_on_threshold: bool,
    pub canary_execution_authority_granted: bool,
    pub promotion_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub credential_authority_granted: bool,
    pub live_trading_authority_granted: bool,
    pub record_digest: [u8; 32],
}

impl CanaryEligibilityRecord {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.record_digest == canary_record_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum GovernanceCommand {
    RegisterCandidate {
        command_id: GovernanceCommandId,
        submission: Box<ReleaseCandidateSubmission>,
        recorded_at_ns: i64,
    },
    RecordDecision {
        command_id: GovernanceCommandId,
        candidate_id: [u8; 32],
        decision: Box<OperatorDecision>,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: GovernanceCommandId,
        candidate_id: [u8; 32],
        record_id: [u8; 32],
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl GovernanceCommand {
    #[must_use]
    pub const fn command_id(&self) -> GovernanceCommandId {
        match self {
            Self::RegisterCandidate { command_id, .. }
            | Self::RecordDecision { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::RegisterCandidate { recorded_at_ns, .. }
            | Self::RecordDecision { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum GovernanceDetail {
    CandidateRegistered {
        candidate_digest: [u8; 32],
        aggregate: CandidateAggregate,
        reasons: Vec<EligibilityReason>,
    },
    DecisionRecorded {
        role: DecisionRole,
        verdict: DecisionVerdict,
    },
    Finalized(Box<CanaryEligibilityRecord>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernanceOutcome {
    pub command_id: GovernanceCommandId,
    pub detail: GovernanceDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GovernanceSnapshot {
    pub accepted_commands: u64,
    pub candidate_id: Option<[u8; 32]>,
    pub candidate_digest: Option<[u8; 32]>,
    pub decisions: BTreeMap<DecisionRole, OperatorDecision>,
    pub last_record: Option<CanaryEligibilityRecord>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("promotion-governance configuration is invalid")]
    Config,
    #[error("governance timestamp is invalid or regressed")]
    Timestamp,
    #[error("governance command exceeds its canonical bound")]
    CommandBound,
    #[error("governance command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported governance command version: {0}")]
    Version(u16),
    #[error("governance command id was reused for different content")]
    IdempotencyConflict,
    #[error("release candidate identity or digest is invalid")]
    Candidate,
    #[error("campaign evidence identity, digest, or authority fields are invalid")]
    Evidence,
    #[error("regression baseline is invalid")]
    Baseline,
    #[error("release artifacts are invalid")]
    Artifacts,
    #[error("rollback criteria are invalid")]
    Rollback,
    #[error("operator decision identity, subject, or lifecycle is invalid")]
    Decision,
    #[error("canary record identity is invalid")]
    Record,
    #[error("release candidate is already finalized")]
    Finalized,
    #[error("governance arithmetic or counter overflow")]
    Overflow,
    #[error("promotion governance is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct PromotionGovernance {
    policy: GovernancePolicy,
    submission: Option<ReleaseCandidateSubmission>,
    aggregate: Option<CandidateAggregate>,
    base_reasons: BTreeSet<EligibilityReason>,
    decisions: BTreeMap<DecisionRole, OperatorDecision>,
    decision_ids: BTreeMap<[u8; 32], [u8; 32]>,
    final_record: Option<CanaryEligibilityRecord>,
    processed: BTreeMap<GovernanceCommandId, ([u8; 32], GovernanceOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl PromotionGovernance {
    /// Creates one empty offline governance owner.
    ///
    /// # Errors
    ///
    /// Rejects invalid hard bounds, thresholds, or validity windows.
    pub fn new(policy: GovernancePolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            submission: None,
            aggregate: None,
            base_reasons: BTreeSet::new(),
            decisions: BTreeMap::new(),
            decision_ids: BTreeMap::new(),
            final_record: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic governance command transactionally.
    ///
    /// # Errors
    ///
    /// Integrity, identity, time, lifecycle, or arithmetic failures halt.
    pub fn apply(&mut self, command: &GovernanceCommand) -> Result<GovernanceOutcome, Error> {
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
        let mut outcome = GovernanceOutcome {
            command_id,
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = governance_outcome_digest(&outcome);
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.processed
            .insert(command_id, (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    fn apply_fresh(&mut self, command: &GovernanceCommand) -> Result<GovernanceDetail, Error> {
        if self.final_record.is_some() {
            return Err(Error::Finalized);
        }
        match command {
            GovernanceCommand::RegisterCandidate {
                submission,
                recorded_at_ns,
                ..
            } => self.register_candidate(submission, *recorded_at_ns),
            GovernanceCommand::RecordDecision {
                candidate_id,
                decision,
                recorded_at_ns,
                ..
            } => self.record_decision(*candidate_id, decision, *recorded_at_ns),
            GovernanceCommand::Finalize {
                candidate_id,
                record_id,
                evaluated_at_ns,
                recorded_at_ns,
                ..
            } => self.finalize(*candidate_id, *record_id, *evaluated_at_ns, *recorded_at_ns),
        }
    }

    fn register_candidate(
        &mut self,
        submission: &ReleaseCandidateSubmission,
        recorded_at_ns: i64,
    ) -> Result<GovernanceDetail, Error> {
        if self.submission.is_some() {
            return Err(Error::Candidate);
        }
        validate_submission(submission, &self.policy, recorded_at_ns)?;
        let (aggregate, reasons) = aggregate_candidate(submission, &self.policy)?;
        self.submission = Some(submission.clone());
        self.aggregate = Some(aggregate.clone());
        self.base_reasons.clone_from(&reasons);
        Ok(GovernanceDetail::CandidateRegistered {
            candidate_digest: submission.candidate_digest,
            aggregate,
            reasons: reasons.into_iter().collect(),
        })
    }

    fn record_decision(
        &mut self,
        candidate_id: [u8; 32],
        decision: &OperatorDecision,
        recorded_at_ns: i64,
    ) -> Result<GovernanceDetail, Error> {
        let submission = self.submission.as_ref().ok_or(Error::Candidate)?;
        if candidate_id != submission.candidate_id
            || decision.decision_id == [0; 32]
            || decision.operator_id == [0; 32]
            || !decision.verify_digest()
            || decision.candidate_digest != submission.candidate_digest
            || decision.evidence_set_digest != submission.evidence_set_digest
            || decision.artifacts_digest != submission.artifacts.artifacts_digest
            || decision.rollback_digest != submission.rollback.criteria_digest
            || decision.decided_at_ns != recorded_at_ns
            || decision.decided_at_ns < submission.created_at_ns
            || decision.valid_until_ns < decision.decided_at_ns
            || decision.valid_until_ns > submission.expires_at_ns
            || decision.valid_until_ns - decision.decided_at_ns
                > self.policy.maximum_decision_age_ns
            || self.decisions.contains_key(&decision.role)
        {
            return Err(Error::Decision);
        }
        if let Some(existing) = self.decision_ids.get(&decision.decision_id) {
            if *existing != decision.decision_digest {
                return Err(Error::Decision);
            }
        }
        self.decision_ids
            .insert(decision.decision_id, decision.decision_digest);
        self.decisions.insert(decision.role, decision.clone());
        Ok(GovernanceDetail::DecisionRecorded {
            role: decision.role,
            verdict: decision.verdict,
        })
    }

    fn finalize(
        &mut self,
        candidate_id: [u8; 32],
        record_id: [u8; 32],
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    ) -> Result<GovernanceDetail, Error> {
        let submission = self.submission.as_ref().ok_or(Error::Candidate)?;
        if candidate_id != submission.candidate_id
            || record_id == [0; 32]
            || evaluated_at_ns < submission.created_at_ns
            || evaluated_at_ns > recorded_at_ns
        {
            return Err(Error::Record);
        }
        let mut reasons = self.base_reasons.clone();
        if evaluated_at_ns > submission.expires_at_ns {
            reasons.insert(EligibilityReason::CandidateExpired);
        }
        for role in [DecisionRole::Risk, DecisionRole::Release] {
            match self.decisions.get(&role) {
                None => {
                    reasons.insert(EligibilityReason::MissingDecision(role));
                }
                Some(decision) if decision.verdict == DecisionVerdict::Reject => {
                    reasons.insert(EligibilityReason::RejectedDecision(role));
                }
                Some(decision) if evaluated_at_ns > decision.valid_until_ns => {
                    reasons.insert(EligibilityReason::ExpiredDecision(role));
                }
                Some(_) => {}
            }
        }
        let risk = self.decisions.get(&DecisionRole::Risk);
        let release = self.decisions.get(&DecisionRole::Release);
        if risk
            .zip(release)
            .is_some_and(|(left, right)| left.operator_id == right.operator_id)
        {
            reasons.insert(EligibilityReason::SameOperator);
        }
        let reasons: Vec<_> = reasons.into_iter().collect();
        let dual_control_complete = risk.zip(release).is_some_and(|(left, right)| {
            left.operator_id != right.operator_id
                && left.verdict == DecisionVerdict::Approve
                && right.verdict == DecisionVerdict::Approve
                && evaluated_at_ns <= left.valid_until_ns
                && evaluated_at_ns <= right.valid_until_ns
        });
        let valid_until_ns = risk
            .into_iter()
            .chain(release)
            .map(|decision| decision.valid_until_ns)
            .fold(submission.expires_at_ns, i64::min);
        let mut record = CanaryEligibilityRecord {
            record_id,
            candidate_id,
            candidate_digest: submission.candidate_digest,
            evidence_set_digest: submission.evidence_set_digest,
            baseline_digest: submission.baseline.baseline_digest,
            artifacts_digest: submission.artifacts.artifacts_digest,
            rollback_digest: submission.rollback.criteria_digest,
            policy_digest: submission.policy_digest,
            evaluated_at_ns,
            valid_until_ns,
            status: if reasons.is_empty() && dual_control_complete {
                CanaryStatus::CanaryEligible
            } else {
                CanaryStatus::NotEligible
            },
            reasons,
            aggregate: self.aggregate.clone().ok_or(Error::Candidate)?,
            risk_decision_digest: risk.map(|value| value.decision_digest),
            release_decision_digest: release.map(|value| value.decision_digest),
            dual_control_complete,
            operator_execution_required: true,
            rollback_required_on_threshold: true,
            canary_execution_authority_granted: false,
            promotion_authority_granted: false,
            deployment_authority_granted: false,
            credential_authority_granted: false,
            live_trading_authority_granted: false,
            record_digest: [0; 32],
        };
        record.record_digest = canary_record_digest(&record);
        self.final_record = Some(record.clone());
        Ok(GovernanceDetail::Finalized(Box::new(record)))
    }

    #[must_use]
    pub fn snapshot(&self) -> GovernanceSnapshot {
        GovernanceSnapshot {
            accepted_commands: self.accepted_commands,
            candidate_id: self.submission.as_ref().map(|value| value.candidate_id),
            candidate_digest: self.submission.as_ref().map(|value| value.candidate_digest),
            decisions: self.decisions.clone(),
            last_record: self.final_record.clone(),
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
        hasher.update(b"promotion-governance-state-v1");
        hash_json(&mut hasher, &self.policy);
        hash_json(&mut hasher, &self.submission);
        hash_json(&mut hasher, &self.aggregate);
        hash_json(&mut hasher, &self.base_reasons);
        for (role, decision) in &self.decisions {
            hash_json(&mut hasher, role);
            hash_json(&mut hasher, decision);
        }
        for (id, digest) in &self.decision_ids {
            hasher.update(id);
            hasher.update(digest);
        }
        hash_json(&mut hasher, &self.final_record);
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

fn validate_policy(policy: &GovernancePolicy) -> Result<(), Error> {
    let diversity_max = policy
        .minimum_distinct_manifests
        .max(policy.minimum_distinct_schedules)
        .max(policy.minimum_distinct_final_states);
    if policy.max_bundles == 0
        || policy.max_bundles > MAX_BUNDLES_HARD
        || policy.minimum_campaigns == 0
        || policy.minimum_campaigns > policy.max_bundles
        || diversity_max == 0
        || diversity_max > policy.minimum_campaigns
        || policy.minimum_total_sessions == 0
        || policy.minimum_total_steps == 0
        || policy.minimum_total_fault_cycles == 0
        || policy.minimum_regression_retention_bps == 0
        || u64::from(policy.minimum_regression_retention_bps) > BASIS_POINTS
        || policy.maximum_bundle_age_ns <= 0
        || policy.maximum_candidate_age_ns <= 0
        || policy.maximum_decision_age_ns <= 0
        || policy.maximum_decision_age_ns > policy.maximum_candidate_age_ns
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn validate_submission(
    submission: &ReleaseCandidateSubmission,
    policy: &GovernancePolicy,
    recorded_at_ns: i64,
) -> Result<(), Error> {
    if submission.candidate_id == [0; 32]
        || submission.created_at_ns < 0
        || submission.created_at_ns != recorded_at_ns
        || submission.expires_at_ns <= submission.created_at_ns
        || submission.expires_at_ns - submission.created_at_ns > policy.maximum_candidate_age_ns
        || submission.evidence.is_empty()
        || submission.evidence.len() > policy.max_bundles
        || !submission.verify_digest(policy)
        || !submission
            .evidence
            .windows(2)
            .all(|pair| pair[0].bundle_digest <= pair[1].bundle_digest)
    {
        return Err(Error::Candidate);
    }
    validate_baseline(&submission.baseline)?;
    validate_artifacts(&submission.artifacts)?;
    validate_rollback(&submission.rollback)?;
    let mut campaigns = BTreeMap::new();
    for bundle in &submission.evidence {
        validate_bundle(bundle, submission.created_at_ns)?;
        if let Some(previous) = campaigns.insert(bundle.campaign_id, bundle.bundle_digest) {
            if previous != bundle.bundle_digest {
                return Err(Error::Evidence);
            }
        }
    }
    Ok(())
}

fn validate_bundle(bundle: &OperatorEvidenceBundle, created_at_ns: i64) -> Result<(), Error> {
    let eligible = bundle.status == CampaignStatus::PromotionEligible;
    let coverage: BTreeSet<_> = bundle.covered_scenarios.iter().copied().collect();
    if bundle.bundle_id == [0; 32]
        || bundle.campaign_id == [0; 32]
        || bundle.manifest_digest == [0; 32]
        || bundle.schedule_digest == [0; 32]
        || bundle.evaluated_at_ns < 0
        || bundle.evaluated_at_ns > created_at_ns
        || !bundle.verify_digest()
        || !bundle.operator_decision_required
        || bundle.promotion_authority_granted
        || bundle.deployment_authority_granted
        || eligible != bundle.reasons.is_empty()
        || (eligible
            && (bundle.session_count != bundle.completed_session_count
                || bundle
                    .required_scenarios
                    .iter()
                    .any(|item| !coverage.contains(item))
                || bundle.final_cash_reserved_micros != 0
                || bundle.final_pending_conversion_count != 0
                || !bundle.final_gateway_ready))
    {
        Err(Error::Evidence)
    } else {
        Ok(())
    }
}

fn validate_baseline(baseline: &RegressionBaseline) -> Result<(), Error> {
    if baseline.baseline_id == [0; 32]
        || baseline.campaign_count == 0
        || baseline.total_sessions == 0
        || baseline.total_steps == 0
        || baseline.total_fault_cycles == 0
        || !baseline.verify_digest()
    {
        Err(Error::Baseline)
    } else {
        Ok(())
    }
}

fn validate_artifacts(artifacts: &ReleaseArtifacts) -> Result<(), Error> {
    let values = [
        artifacts.release_id,
        artifacts.source_digest,
        artifacts.binary_digest,
        artifacts.toolchain_digest,
        artifacts.dependency_lock_digest,
        artifacts.sbom_digest,
        artifacts.configuration_digest,
    ];
    if values.contains(&[0; 32]) || !artifacts.verify_digest() {
        Err(Error::Artifacts)
    } else {
        Ok(())
    }
}

fn validate_rollback(criteria: &RollbackCriteria) -> Result<(), Error> {
    if criteria.criteria_id == [0; 32]
        || criteria.rollback_target_digest == [0; 32]
        || criteria.maximum_canary_duration_ns <= 0
        || criteria.maximum_unreconciled_ns <= 0
        || criteria.maximum_unknown_state_ns <= 0
        || criteria.maximum_session_loss_micros < 0
        || criteria.maximum_consecutive_faults == 0
        || !criteria.require_capital_floor_halt
        || !criteria.require_reconciliation_halt
        || !criteria.verify_digest()
    {
        Err(Error::Rollback)
    } else {
        Ok(())
    }
}

fn aggregate_candidate(
    submission: &ReleaseCandidateSubmission,
    policy: &GovernancePolicy,
) -> Result<(CandidateAggregate, BTreeSet<EligibilityReason>), Error> {
    let mut reasons = BTreeSet::new();
    let mut unique_bundles = BTreeSet::new();
    let mut campaigns = BTreeMap::new();
    for bundle in &submission.evidence {
        if !unique_bundles.insert(bundle.bundle_digest) {
            reasons.insert(EligibilityReason::DuplicateEvidence);
            continue;
        }
        campaigns.entry(bundle.campaign_id).or_insert(bundle);
    }
    if campaigns.len() != unique_bundles.len() {
        reasons.insert(EligibilityReason::DuplicateEvidence);
    }
    let mut manifests = BTreeSet::new();
    let mut schedules = BTreeSet::new();
    let mut final_states = BTreeSet::new();
    let mut aggregate = CandidateAggregate {
        unique_campaigns: 0,
        distinct_manifests: 0,
        distinct_schedules: 0,
        distinct_final_states: 0,
        total_sessions: 0,
        total_steps: 0,
        total_fault_cycles: 0,
    };
    for bundle in campaigns.values() {
        if bundle.status != CampaignStatus::PromotionEligible {
            reasons.insert(EligibilityReason::CampaignNotEligible(bundle.bundle_id));
            continue;
        }
        if submission.created_at_ns - bundle.evaluated_at_ns > policy.maximum_bundle_age_ns {
            reasons.insert(EligibilityReason::CampaignStale(bundle.bundle_id));
            continue;
        }
        aggregate.unique_campaigns = checked_add(aggregate.unique_campaigns, 1)?;
        aggregate.total_sessions = checked_add(
            aggregate.total_sessions,
            u64::try_from(bundle.completed_session_count).map_err(|_| Error::Overflow)?,
        )?;
        aggregate.total_steps = checked_add(aggregate.total_steps, bundle.applied_step_count)?;
        let faults = bundle
            .dead_man_count
            .checked_add(bundle.restart_count)
            .and_then(|value| value.checked_add(bundle.unknown_recovery_count))
            .ok_or(Error::Overflow)?;
        aggregate.total_fault_cycles = checked_add(aggregate.total_fault_cycles, faults)?;
        manifests.insert(bundle.manifest_digest);
        schedules.insert(bundle.schedule_digest);
        final_states.insert(bundle.final_gateway_digest);
    }
    aggregate.distinct_manifests = usize_to_u64(manifests.len())?;
    aggregate.distinct_schedules = usize_to_u64(schedules.len())?;
    aggregate.distinct_final_states = usize_to_u64(final_states.len())?;
    apply_absolute_reasons(&aggregate, policy, &mut reasons);
    apply_regression_reasons(&aggregate, &submission.baseline, policy, &mut reasons)?;
    Ok((aggregate, reasons))
}

fn apply_absolute_reasons(
    aggregate: &CandidateAggregate,
    policy: &GovernancePolicy,
    reasons: &mut BTreeSet<EligibilityReason>,
) {
    if aggregate.unique_campaigns < policy.minimum_campaigns as u64 {
        reasons.insert(EligibilityReason::InsufficientCampaigns);
    }
    if aggregate.distinct_manifests < policy.minimum_distinct_manifests as u64 {
        reasons.insert(EligibilityReason::InsufficientManifestDiversity);
    }
    if aggregate.distinct_schedules < policy.minimum_distinct_schedules as u64 {
        reasons.insert(EligibilityReason::InsufficientScheduleDiversity);
    }
    if aggregate.distinct_final_states < policy.minimum_distinct_final_states as u64 {
        reasons.insert(EligibilityReason::InsufficientFinalStateDiversity);
    }
    if aggregate.total_sessions < policy.minimum_total_sessions {
        reasons.insert(EligibilityReason::InsufficientSessions);
    }
    if aggregate.total_steps < policy.minimum_total_steps {
        reasons.insert(EligibilityReason::InsufficientSteps);
    }
    if aggregate.total_fault_cycles < policy.minimum_total_fault_cycles {
        reasons.insert(EligibilityReason::InsufficientFaultCycles);
    }
}

fn apply_regression_reasons(
    aggregate: &CandidateAggregate,
    baseline: &RegressionBaseline,
    policy: &GovernancePolicy,
    reasons: &mut BTreeSet<EligibilityReason>,
) -> Result<(), Error> {
    let retention = u64::from(policy.minimum_regression_retention_bps);
    if aggregate.unique_campaigns < retained_floor(baseline.campaign_count, retention)? {
        reasons.insert(EligibilityReason::RegressionCampaignCount);
    }
    if aggregate.total_sessions < retained_floor(baseline.total_sessions, retention)? {
        reasons.insert(EligibilityReason::RegressionSessionCount);
    }
    if aggregate.total_steps < retained_floor(baseline.total_steps, retention)? {
        reasons.insert(EligibilityReason::RegressionStepCount);
    }
    if aggregate.total_fault_cycles < retained_floor(baseline.total_fault_cycles, retention)? {
        reasons.insert(EligibilityReason::RegressionFaultCycles);
    }
    Ok(())
}

fn retained_floor(baseline: u64, retention_bps: u64) -> Result<u64, Error> {
    let product = u128::from(baseline)
        .checked_mul(u128::from(retention_bps))
        .ok_or(Error::Overflow)?;
    let rounded = product
        .checked_add(u128::from(BASIS_POINTS - 1))
        .ok_or(Error::Overflow)?
        / u128::from(BASIS_POINTS);
    u64::try_from(rounded).map_err(|_| Error::Overflow)
}

fn checked_add(left: u64, right: u64) -> Result<u64, Error> {
    left.checked_add(right).ok_or(Error::Overflow)
}

fn usize_to_u64(value: usize) -> Result<u64, Error> {
    u64::try_from(value).map_err(|_| Error::Overflow)
}

fn regression_digest(value: &RegressionBaseline) -> [u8; 32] {
    let mut clone = value.clone();
    clone.baseline_digest = [0; 32];
    digest_json(b"promotion-regression-baseline-v1", &clone)
}

fn artifacts_digest(value: &ReleaseArtifacts) -> [u8; 32] {
    let mut clone = value.clone();
    clone.artifacts_digest = [0; 32];
    digest_json(b"promotion-release-artifacts-v1", &clone)
}

fn rollback_digest(value: &RollbackCriteria) -> [u8; 32] {
    let mut clone = value.clone();
    clone.criteria_digest = [0; 32];
    digest_json(b"promotion-rollback-criteria-v1", &clone)
}

fn evidence_set_digest(evidence: &[OperatorEvidenceBundle]) -> [u8; 32] {
    let digests: Vec<_> = evidence.iter().map(|bundle| bundle.bundle_digest).collect();
    digest_json(b"promotion-evidence-set-v1", &digests)
}

fn candidate_digest(value: &ReleaseCandidateSubmission) -> [u8; 32] {
    let mut clone = value.clone();
    clone.candidate_digest = [0; 32];
    digest_json(b"promotion-release-candidate-v1", &clone)
}

fn decision_digest(value: &OperatorDecision) -> [u8; 32] {
    let mut clone = value.clone();
    clone.decision_digest = [0; 32];
    digest_json(b"promotion-operator-decision-v1", &clone)
}

fn canary_record_digest(value: &CanaryEligibilityRecord) -> [u8; 32] {
    let mut clone = value.clone();
    clone.record_digest = [0; 32];
    digest_json(b"promotion-canary-record-v1", &clone)
}

fn governance_outcome_digest(value: &GovernanceOutcome) -> [u8; 32] {
    let mut clone = value.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"promotion-governance-outcome-v1", &clone)
}

fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable governance state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: GovernanceCommand,
}

/// Encodes one bounded versioned governance command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &GovernanceCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one bounded versioned governance command.
///
/// # Errors
///
/// Rejects malformed, oversized, trailing, or unsupported input.
pub fn decode_command(bytes: &[u8]) -> Result<GovernanceCommand, Error> {
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
