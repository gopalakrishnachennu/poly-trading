#![forbid(unsafe_code)]

//! Deterministic offline governance for research models.
//!
//! This crate has no network, clock, model-training, signing, wallet, capital,
//! reservation, order or submission capability. It evaluates supplied evidence
//! and can only return a paper candidate or a non-bypassable `NO_TRADE`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

pub const WIRE_VERSION: u16 = 1;
const MAX_LABEL_BYTES: usize = 128;
const MAX_MODEL_AGE_NS: i64 = 90 * 24 * 60 * 60 * 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ModelId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoldKind {
    Train,
    Validation,
    Test,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkForwardFold {
    pub kind: FoldKind,
    pub start_available_time_ns: i64,
    pub end_available_time_ns: i64,
    pub data_digest: [u8; 32],
    pub fold_digest: [u8; 32],
}

impl WalkForwardFold {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.fold_digest = digest_without(b"model-governance-fold-v1", &self, |value| {
            value.fold_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.fold_digest
            == digest_without(b"model-governance-fold-v1", self, |value| {
                value.fold_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkForwardPlan {
    pub train: WalkForwardFold,
    pub validation: WalkForwardFold,
    pub test: WalkForwardFold,
    pub plan_digest: [u8; 32],
}

impl WalkForwardPlan {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.plan_digest = digest_without(b"model-governance-plan-v1", &self, |value| {
            value.plan_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.plan_digest
            == digest_without(b"model-governance-plan-v1", self, |value| {
                value.plan_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelArtifact {
    pub model_id: ModelId,
    pub label: String,
    pub training_fold_digest: [u8; 32],
    pub validation_fold_digest: [u8; 32],
    pub feature_schema_digest: [u8; 32],
    pub configuration_digest: [u8; 32],
    pub code_digest: [u8; 32],
    pub trained_at_ns: i64,
    pub frozen_at_ns: i64,
    pub artifact_digest: [u8; 32],
}

impl ModelArtifact {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.artifact_digest = digest_without(b"model-governance-artifact-v1", &self, |value| {
            value.artifact_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.artifact_digest
            == digest_without(b"model-governance-artifact-v1", self, |value| {
                value.artifact_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationMetrics {
    pub net_pnl_micros: i64,
    pub max_drawdown_micros: i64,
    pub cvar_loss_micros: i64,
    pub fees_micros: i64,
    pub slippage_micros: i64,
    pub fill_rate_bps: u16,
    pub data_coverage_bps: u16,
    pub hedge_failures: u64,
    pub observations: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleAttestations {
    pub research_label: String,
    pub evaluation_label: String,
    pub adversarial_label: String,
    pub adversarial_passed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEvidence {
    pub artifact: ModelArtifact,
    pub test_fold_digest: [u8; 32],
    pub evaluated_at_ns: i64,
    pub model_drift_bps: u16,
    pub metrics: EvaluationMetrics,
    pub roles: RoleAttestations,
    pub evidence_digest: [u8; 32],
}

impl ModelEvidence {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.evidence_digest = digest_without(b"model-governance-evidence-v1", &self, |value| {
            value.evidence_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evidence_digest
            == digest_without(b"model-governance-evidence-v1", self, |value| {
                value.evidence_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernancePolicy {
    pub minimum_observations: u64,
    pub minimum_net_pnl_micros: i64,
    pub minimum_challenger_improvement_micros: i64,
    pub maximum_drawdown_micros: i64,
    pub maximum_cvar_loss_micros: i64,
    pub minimum_fill_rate_bps: u16,
    pub minimum_data_coverage_bps: u16,
    pub maximum_hedge_failures: u64,
    pub maximum_model_drift_bps: u16,
    pub maximum_model_age_ns: i64,
    pub policy_digest: [u8; 32],
}

impl GovernancePolicy {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.policy_digest = digest_without(b"model-governance-policy-v1", &self, |value| {
            value.policy_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.policy_digest
            == digest_without(b"model-governance-policy-v1", self, |value| {
                value.policy_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceOutcome {
    PaperChampionCandidate,
    ChampionRetained,
    NoTrade,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct GovernanceDecision {
    pub outcome: GovernanceOutcome,
    pub selected_model_id: Option<ModelId>,
    pub reason: String,
    pub policy_digest: [u8; 32],
    pub candidate_evidence_digest: [u8; 32],
    pub champion_evidence_digest: Option<[u8; 32]>,
    pub capital_authority: bool,
    pub risk_authority: bool,
    pub placement_authority: bool,
    pub signing_authority: bool,
    pub submission_authority: bool,
    pub decision_digest: [u8; 32],
}

impl GovernanceDecision {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest
            == digest_without(b"model-governance-decision-v1", self, |value| {
                value.decision_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum GovernanceError {
    #[error("policy invalid")]
    Policy,
    #[error("walk-forward folds invalid")]
    Folds,
    #[error("model artifact invalid")]
    Artifact,
    #[error("model evidence invalid")]
    Evidence,
    #[error("model evidence is stale or future dated")]
    Time,
    #[error("evaluation roles are invalid or not independent")]
    Roles,
    #[error("financial or evaluation metric is invalid")]
    Metrics,
}

/// Evaluates a challenger against immutable, point-in-time-valid evidence.
/// The returned candidate is paper-only and grants no financial authority.
///
/// # Errors
///
/// Returns an error when policy, fold, model, timestamp, role, provenance or
/// fixed-point metric validation fails.
pub fn govern(
    policy: &GovernancePolicy,
    plan: &WalkForwardPlan,
    now_ns: i64,
    champion: Option<&ModelEvidence>,
    challenger: &ModelEvidence,
) -> Result<GovernanceDecision, GovernanceError> {
    if now_ns < 0 {
        return Err(GovernanceError::Time);
    }
    validate_policy(policy)?;
    validate_plan(plan)?;
    validate_evidence(policy, plan, now_ns, challenger)?;
    if let Some(value) = champion {
        if value.artifact.model_id == challenger.artifact.model_id {
            return Err(GovernanceError::Evidence);
        }
        validate_evidence(policy, plan, now_ns, value)?;
    }

    let challenger_passes = passes_policy(policy, challenger);
    let (outcome, selected_model_id, reason) = match champion {
        None if challenger_passes => (
            GovernanceOutcome::PaperChampionCandidate,
            Some(challenger.artifact.model_id),
            "challenger passed immutable paper-evidence gates".to_owned(),
        ),
        None => (
            GovernanceOutcome::NoTrade,
            None,
            "challenger failed one or more paper-evidence gates".to_owned(),
        ),
        Some(current) if !passes_policy(policy, current) && challenger_passes => (
            GovernanceOutcome::PaperChampionCandidate,
            Some(challenger.artifact.model_id),
            "champion failed gates; challenger passed immutable paper-evidence gates".to_owned(),
        ),
        Some(current)
            if challenger_passes
                && challenger.metrics.net_pnl_micros
                    >= current
                        .metrics
                        .net_pnl_micros
                        .saturating_add(policy.minimum_challenger_improvement_micros) =>
        {
            (
                GovernanceOutcome::PaperChampionCandidate,
                Some(challenger.artifact.model_id),
                "challenger improved conservative net P&L on unseen evidence".to_owned(),
            )
        }
        Some(current) if passes_policy(policy, current) => (
            GovernanceOutcome::ChampionRetained,
            Some(current.artifact.model_id),
            "champion retained; challenger did not clear required improvement".to_owned(),
        ),
        Some(_) => (
            GovernanceOutcome::NoTrade,
            None,
            "neither champion nor challenger passed immutable paper-evidence gates".to_owned(),
        ),
    };
    Ok(seal_decision(GovernanceDecision {
        outcome,
        selected_model_id,
        reason,
        policy_digest: policy.policy_digest,
        candidate_evidence_digest: challenger.evidence_digest,
        champion_evidence_digest: champion.map(|value| value.evidence_digest),
        capital_authority: false,
        risk_authority: false,
        placement_authority: false,
        signing_authority: false,
        submission_authority: false,
        decision_digest: [0; 32],
    }))
}

fn validate_policy(policy: &GovernancePolicy) -> Result<(), GovernanceError> {
    if !policy.verify_digest()
        || policy.maximum_drawdown_micros < 0
        || policy.maximum_cvar_loss_micros < 0
        || policy.minimum_fill_rate_bps > 10_000
        || policy.minimum_data_coverage_bps > 10_000
        || policy.maximum_model_drift_bps > 10_000
        || policy.maximum_model_age_ns <= 0
        || policy.maximum_model_age_ns > MAX_MODEL_AGE_NS
    {
        return Err(GovernanceError::Policy);
    }
    Ok(())
}

fn validate_plan(plan: &WalkForwardPlan) -> Result<(), GovernanceError> {
    if !plan.verify_digest()
        || !plan.train.verify_digest()
        || !plan.validation.verify_digest()
        || !plan.test.verify_digest()
        || plan.train.kind != FoldKind::Train
        || plan.validation.kind != FoldKind::Validation
        || plan.test.kind != FoldKind::Test
        || plan.train.start_available_time_ns < 0
        || plan.train.start_available_time_ns >= plan.train.end_available_time_ns
        || plan.train.end_available_time_ns > plan.validation.start_available_time_ns
        || plan.validation.start_available_time_ns >= plan.validation.end_available_time_ns
        || plan.validation.end_available_time_ns > plan.test.start_available_time_ns
        || plan.test.start_available_time_ns >= plan.test.end_available_time_ns
        || is_zero(&plan.train.data_digest)
        || is_zero(&plan.validation.data_digest)
        || is_zero(&plan.test.data_digest)
    {
        return Err(GovernanceError::Folds);
    }
    Ok(())
}

fn validate_evidence(
    policy: &GovernancePolicy,
    plan: &WalkForwardPlan,
    now_ns: i64,
    evidence: &ModelEvidence,
) -> Result<(), GovernanceError> {
    let artifact = &evidence.artifact;
    if !evidence.verify_digest() {
        return Err(GovernanceError::Evidence);
    }
    if !artifact.verify_digest()
        || artifact.label.is_empty()
        || artifact.label.len() > MAX_LABEL_BYTES
        || is_zero(&artifact.model_id.0)
        || is_zero(&artifact.feature_schema_digest)
        || is_zero(&artifact.configuration_digest)
        || is_zero(&artifact.code_digest)
        || artifact.training_fold_digest != plan.train.fold_digest
        || artifact.validation_fold_digest != plan.validation.fold_digest
        || evidence.test_fold_digest != plan.test.fold_digest
        || artifact.trained_at_ns < plan.train.end_available_time_ns
        || artifact.frozen_at_ns < artifact.trained_at_ns
        || artifact.frozen_at_ns > plan.test.start_available_time_ns
    {
        return Err(GovernanceError::Artifact);
    }
    if evidence.evaluated_at_ns < plan.test.end_available_time_ns
        || evidence.evaluated_at_ns > now_ns
        || now_ns.saturating_sub(artifact.frozen_at_ns) > policy.maximum_model_age_ns
    {
        return Err(GovernanceError::Time);
    }
    validate_roles(&evidence.roles)?;
    let metrics = &evidence.metrics;
    if metrics.max_drawdown_micros < 0
        || metrics.cvar_loss_micros < 0
        || metrics.fees_micros < 0
        || metrics.slippage_micros < 0
        || metrics.fill_rate_bps > 10_000
        || metrics.data_coverage_bps > 10_000
        || evidence.model_drift_bps > 10_000
    {
        return Err(GovernanceError::Metrics);
    }
    Ok(())
}

fn validate_roles(roles: &RoleAttestations) -> Result<(), GovernanceError> {
    let labels = [
        &roles.research_label,
        &roles.evaluation_label,
        &roles.adversarial_label,
    ];
    if labels
        .iter()
        .any(|label| label.is_empty() || label.len() > MAX_LABEL_BYTES)
    {
        return Err(GovernanceError::Roles);
    }
    let distinct: BTreeSet<&str> = labels.iter().map(|label| label.as_str()).collect();
    if distinct.len() != labels.len() {
        return Err(GovernanceError::Roles);
    }
    Ok(())
}

fn passes_policy(policy: &GovernancePolicy, evidence: &ModelEvidence) -> bool {
    let metrics = &evidence.metrics;
    evidence.roles.adversarial_passed
        && metrics.observations >= policy.minimum_observations
        && metrics.net_pnl_micros >= policy.minimum_net_pnl_micros
        && metrics.max_drawdown_micros <= policy.maximum_drawdown_micros
        && metrics.cvar_loss_micros <= policy.maximum_cvar_loss_micros
        && metrics.fill_rate_bps >= policy.minimum_fill_rate_bps
        && metrics.data_coverage_bps >= policy.minimum_data_coverage_bps
        && metrics.hedge_failures <= policy.maximum_hedge_failures
        && evidence.model_drift_bps <= policy.maximum_model_drift_bps
}

fn seal_decision(mut decision: GovernanceDecision) -> GovernanceDecision {
    decision.decision_digest =
        digest_without(b"model-governance-decision-v1", &decision, |value| {
            value.decision_digest = [0; 32];
        });
    decision
}

fn is_zero(value: &[u8; 32]) -> bool {
    value.iter().all(|byte| *byte == 0)
}

fn digest_without<T: Serialize + Clone>(
    domain: &[u8],
    value: &T,
    clear: impl FnOnce(&mut T),
) -> [u8; 32] {
    let mut copy = value.clone();
    clear(&mut copy);
    let bytes = serde_json::to_vec(&copy).expect("bounded governance serialization");
    *blake3::hash(&[domain, &bytes].concat()).as_bytes()
}

#[cfg(test)]
mod tests;
