#![forbid(unsafe_code)]

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableReleaseController,
    ReleaseCheckpoint, ReleaseRecovery, ReleaseStorageError,
};
use micro_capital_canary_controller::{CanaryReport, CanaryReportStatus, CanaryScenario};
pub use report::{read_report, write_report_create_new, ReleaseReportFileError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ReleaseCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePolicy {
    pub maximum_canary_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_evidence_age_ns: i64,
    pub maximum_cases: usize,
    pub minimum_regions: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapitalStage {
    pub index: u32,
    pub capital_ceiling_micros: i128,
    pub exposure_ceiling_micros: i128,
    pub session_loss_ceiling_micros: i128,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseScenario {
    NoTrade,
    StagedCeilings,
    MultiRegionHealth,
    ContinuousReconciliation,
    EvidenceExpiry,
    IncidentResponse,
    DisasterRecovery,
    Rollback,
    Revocation,
    Governance,
}

impl ReleaseScenario {
    pub const ALL: [Self; 10] = [
        Self::NoTrade,
        Self::StagedCeilings,
        Self::MultiRegionHealth,
        Self::ContinuousReconciliation,
        Self::EvidenceExpiry,
        Self::IncidentResponse,
        Self::DisasterRecovery,
        Self::Rollback,
        Self::Revocation,
        Self::Governance,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseDisposition {
    NoTrade,
    CodeEligible,
    RollbackRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSubjects {
    pub release_digest: [u8; 32],
    pub artifact_digest: [u8; 32],
    pub configuration_digest: [u8; 32],
    pub infrastructure_digest: [u8; 32],
    pub reconciliation_digest: [u8; 32],
    pub incident_runbook_digest: [u8; 32],
    pub disaster_recovery_digest: [u8; 32],
    pub rollback_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePlan {
    pub plan_id: [u8; 32],
    pub canary_report: CanaryReport,
    pub subjects: ReleaseSubjects,
    pub capital_stages: Vec<CapitalStage>,
    pub required_regions: Vec<[u8; 32]>,
    pub required_scenarios: Vec<ReleaseScenario>,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl ReleasePlan {
    #[must_use]
    pub fn sealed(mut self, policy: &ReleasePolicy) -> Self {
        self.required_regions.sort_unstable();
        self.required_scenarios.sort();
        self.policy_digest = digest_json(b"controlled-release-policy-v1", policy);
        self.plan_digest = digest_without(b"controlled-release-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &ReleasePolicy) -> bool {
        self.policy_digest == digest_json(b"controlled-release-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"controlled-release-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RegionHealth {
    pub evidence_id: [u8; 32],
    pub region_digest: [u8; 32],
    pub sequence: u64,
    pub observed_at_ns: i64,
    pub healthy: bool,
    pub reconciliation_current: bool,
    pub capital_floor_intact: bool,
    pub no_unknown_state: bool,
    pub external_mutation_observed: bool,
    pub evidence_digest: [u8; 32],
}

impl RegionHealth {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.evidence_digest = digest_without(b"release-region-health-v1", &self, |v| {
            v.evidence_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evidence_digest
            == digest_without(b"release-region-health-v1", self, |v| {
                v.evidence_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ReleaseCase {
    pub case_id: [u8; 32],
    pub sequence: u64,
    pub scenario: ReleaseScenario,
    pub stage_index: u32,
    pub observed_at_ns: i64,
    pub disposition: ReleaseDisposition,
    pub reconciliation_current: bool,
    pub incident_process_proven: bool,
    pub disaster_recovery_proven: bool,
    pub rollback_proven: bool,
    pub revocation_proven: bool,
    pub no_trade_available: bool,
    pub external_action_observed: bool,
    pub case_digest: [u8; 32],
}

impl ReleaseCase {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.case_digest = digest_without(b"controlled-release-case-v1", &self, |v| {
            v.case_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.case_digest
            == digest_without(b"controlled-release-case-v1", self, |v| {
                v.case_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReportStatus {
    CodeEligible,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ControlledReleaseReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub canary_report_digest: [u8; 32],
    pub covered_regions: Vec<[u8; 32]>,
    pub covered_scenarios: Vec<ReleaseScenario>,
    pub finalized_at_ns: i64,
    pub status: ReleaseReportStatus,
    pub target_environment_certified: bool,
    pub production_release_complete: bool,
    pub legal_eligibility_confirmed: bool,
    pub real_capital_allocated: bool,
    pub credential_material_created: bool,
    pub signature_produced: bool,
    pub external_order_submitted: bool,
    pub capital_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub submission_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl ControlledReleaseReport {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.covered_regions.sort_unstable();
        self.covered_scenarios.sort();
        self.report_digest = digest_without(b"controlled-release-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"controlled-release-report-v1", self, |v| {
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
pub enum ReleaseCommand {
    Register {
        command_id: ReleaseCommandId,
        plan: Box<ReleasePlan>,
        recorded_at_ns: i64,
    },
    Approve {
        command_id: ReleaseCommandId,
        release_operator_digest: [u8; 32],
        risk_operator_digest: [u8; 32],
        operations_operator_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    RecordRegion {
        command_id: ReleaseCommandId,
        evidence: RegionHealth,
        recorded_at_ns: i64,
    },
    RecordCase {
        command_id: ReleaseCommandId,
        case: ReleaseCase,
        recorded_at_ns: i64,
    },
    Revoke {
        command_id: ReleaseCommandId,
        reason_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: ReleaseCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl ReleaseCommand {
    #[must_use]
    pub const fn command_id(&self) -> ReleaseCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::Approve { command_id, .. }
            | Self::RecordRegion { command_id, .. }
            | Self::RecordCase { command_id, .. }
            | Self::Revoke { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::Approve { recorded_at_ns, .. }
            | Self::RecordRegion { recorded_at_ns, .. }
            | Self::RecordCase { recorded_at_ns, .. }
            | Self::Revoke { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ReleaseDetail {
    Registered,
    Approved,
    RegionAccepted,
    CaseAccepted,
    Revoked,
    Finalized(Box<ControlledReleaseReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReleaseOutcome {
    pub command_id: ReleaseCommandId,
    pub detail: ReleaseDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseSnapshot {
    pub approved: bool,
    pub revoked: bool,
    pub covered_regions: BTreeSet<[u8; 32]>,
    pub covered_scenarios: BTreeSet<ReleaseScenario>,
    pub report: Option<ControlledReleaseReport>,
    pub accepted_commands: u64,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("release policy invalid")]
    Config,
    #[error("release timestamp invalid")]
    Timestamp,
    #[error("release command bound")]
    CommandBound,
    #[error("release JSON invalid: {0}")]
    Json(String),
    #[error("release version {0}")]
    Version(u16),
    #[error("release id conflict")]
    IdempotencyConflict,
    #[error("Phase 3.7 evidence invalid")]
    Upstream,
    #[error("release plan invalid")]
    Plan,
    #[error("release approval invalid")]
    Approval,
    #[error("region health invalid")]
    Region,
    #[error("release case invalid")]
    Case,
    #[error("release revocation invalid")]
    Revoke,
    #[error("release finalization invalid")]
    Finalize,
    #[error("release overflow")]
    Overflow,
    #[error("release halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct ControlledProductionRelease {
    policy: ReleasePolicy,
    plan: Option<ReleasePlan>,
    approved: bool,
    revoked: bool,
    regions: BTreeMap<[u8; 32], RegionHealth>,
    covered: BTreeSet<ReleaseScenario>,
    used_cases: BTreeSet<[u8; 32]>,
    case_count: u64,
    processed: BTreeMap<ReleaseCommandId, ([u8; 32], ReleaseOutcome)>,
    accepted_commands: u64,
    report: Option<ControlledReleaseReport>,
    last_at: Option<i64>,
    halted: Option<String>,
}

impl ControlledProductionRelease {
    /// Creates an empty release controller.
    /// # Errors
    /// Rejects invalid policy.
    pub fn new(policy: ReleasePolicy) -> Result<Self, Error> {
        if policy.maximum_canary_report_age_ns <= 0
            || policy.maximum_plan_lifetime_ns <= 0
            || policy.maximum_evidence_age_ns <= 0
            || policy.maximum_cases == 0
            || policy.minimum_regions < 2
        {
            return Err(Error::Config);
        }
        Ok(Self {
            policy,
            plan: None,
            approved: false,
            revoked: false,
            regions: BTreeMap::new(),
            covered: BTreeSet::new(),
            used_cases: BTreeSet::new(),
            case_count: 0,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            report: None,
            last_at: None,
            halted: None,
        })
    }

    /// Applies one journalable command.
    /// # Errors
    /// Invalid evidence, ordering, arithmetic or authority claims halt.
    pub fn apply(&mut self, command: &ReleaseCommand) -> Result<ReleaseOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        if command.recorded_at_ns() < 0
            || self.last_at.is_some_and(|v| command.recorded_at_ns() < v)
        {
            return self.halt(Error::Timestamp);
        }
        let bytes = encode_command(command)?;
        let content = *blake3::hash(&bytes).as_bytes();
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
        next.last_at = Some(command.recorded_at_ns());
        let mut outcome = ReleaseOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"controlled-release-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &ReleaseCommand) -> Result<ReleaseDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            ReleaseCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some()
                    || !valid_upstream(&plan.canary_report, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(ReleaseDetail::Registered)
            }
            ReleaseCommand::Approve {
                release_operator_digest,
                risk_operator_digest,
                operations_operator_digest,
                ..
            } => {
                let operators = [
                    *release_operator_digest,
                    *risk_operator_digest,
                    *operations_operator_digest,
                ];
                if self.plan.is_none()
                    || self.approved
                    || operators.contains(&[0; 32])
                    || operators.into_iter().collect::<BTreeSet<_>>().len() != 3
                {
                    return Err(Error::Approval);
                }
                self.approved = true;
                Ok(ReleaseDetail::Approved)
            }
            ReleaseCommand::RecordRegion {
                evidence,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Region)?;
                if !self.approved
                    || self.revoked
                    || !evidence.verify_digest()
                    || evidence.external_mutation_observed
                    || !plan.required_regions.contains(&evidence.region_digest)
                    || evidence.observed_at_ns > *recorded_at_ns
                    || *recorded_at_ns - evidence.observed_at_ns
                        > self.policy.maximum_evidence_age_ns
                    || !evidence.healthy
                    || !evidence.reconciliation_current
                    || !evidence.capital_floor_intact
                    || !evidence.no_unknown_state
                    || self
                        .regions
                        .get(&evidence.region_digest)
                        .is_some_and(|v| evidence.sequence != v.sequence + 1)
                    || !self.regions.contains_key(&evidence.region_digest) && evidence.sequence != 1
                {
                    return Err(Error::Region);
                }
                self.regions
                    .insert(evidence.region_digest, evidence.clone());
                Ok(ReleaseDetail::RegionAccepted)
            }
            ReleaseCommand::RecordCase {
                case,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Case)?;
                if !self.approved
                    || self.revoked
                    || self.used_cases.contains(&case.case_id)
                    || self.case_count
                        >= u64::try_from(self.policy.maximum_cases).map_err(|_| Error::Overflow)?
                    || case.sequence != self.case_count.checked_add(1).ok_or(Error::Overflow)?
                    || !case.verify_digest()
                    || case.external_action_observed
                    || !valid_case(case, plan, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Case);
                }
                self.case_count += 1;
                self.used_cases.insert(case.case_id);
                self.covered.insert(case.scenario);
                Ok(ReleaseDetail::CaseAccepted)
            }
            ReleaseCommand::Revoke { reason_digest, .. } => {
                if self.plan.is_none() || self.revoked || *reason_digest == [0; 32] {
                    return Err(Error::Revoke);
                }
                self.revoked = true;
                Ok(ReleaseDetail::Revoked)
            }
            ReleaseCommand::Finalize {
                report_id,
                finalized_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                let regions_current = plan.required_regions.iter().all(|region| {
                    self.regions.get(region).is_some_and(|e| {
                        *finalized_at_ns >= e.observed_at_ns
                            && *finalized_at_ns - e.observed_at_ns
                                <= self.policy.maximum_evidence_age_ns
                    })
                });
                if !self.approved
                    || self.revoked
                    || !regions_current
                    || *finalized_at_ns > plan.expires_at_ns
                    || !plan
                        .required_scenarios
                        .iter()
                        .all(|v| self.covered.contains(v))
                {
                    return Err(Error::Finalize);
                }
                let report = ControlledReleaseReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    canary_report_digest: plan.canary_report.report_digest,
                    covered_regions: self.regions.keys().copied().collect(),
                    covered_scenarios: self.covered.iter().copied().collect(),
                    finalized_at_ns: *finalized_at_ns,
                    status: ReleaseReportStatus::CodeEligible,
                    target_environment_certified: false,
                    production_release_complete: false,
                    legal_eligibility_confirmed: false,
                    real_capital_allocated: false,
                    credential_material_created: false,
                    signature_produced: false,
                    external_order_submitted: false,
                    capital_authority_granted: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    submission_authority_granted: false,
                    report_digest: [0; 32],
                }
                .sealed();
                self.report = Some(report.clone());
                Ok(ReleaseDetail::Finalized(Box::new(report)))
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ReleaseSnapshot {
        let state = (
            self.approved,
            self.revoked,
            &self.regions,
            &self.covered,
            self.case_count,
            &self.report,
            self.accepted_commands,
            &self.halted,
        );
        ReleaseSnapshot {
            approved: self.approved,
            revoked: self.revoked,
            covered_regions: self.regions.keys().copied().collect(),
            covered_scenarios: self.covered.clone(),
            report: self.report.clone(),
            accepted_commands: self.accepted_commands,
            halted: self.halted.is_some(),
            digest: digest_json(b"controlled-release-state-v1", &state),
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
}

fn valid_upstream(report: &CanaryReport, policy: &ReleasePolicy, at: i64) -> bool {
    report.verify_digest()
        && report.status == CanaryReportStatus::CodeEligible
        && report.covered_scenarios == CanaryScenario::ALL
        && report.finalized_at_ns <= at
        && at - report.finalized_at_ns <= policy.maximum_canary_report_age_ns
        && !report.live_canary_complete
        && !report.legal_eligibility_confirmed
        && !report.real_capital_allocated
        && !report.credential_material_created
        && !report.signature_produced
        && !report.external_order_submitted
        && !report.capital_authority_granted
        && !report.deployment_authority_granted
        && !report.trading_authority_granted
        && !report.submission_authority_granted
}

fn valid_plan(plan: &ReleasePlan, policy: &ReleasePolicy, at: i64) -> bool {
    let nonzero_subjects = [
        plan.subjects.release_digest,
        plan.subjects.artifact_digest,
        plan.subjects.configuration_digest,
        plan.subjects.infrastructure_digest,
        plan.subjects.reconciliation_digest,
        plan.subjects.incident_runbook_digest,
        plan.subjects.disaster_recovery_digest,
        plan.subjects.rollback_digest,
    ]
    .into_iter()
    .all(|v| v != [0; 32]);
    let regions = plan
        .required_regions
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let stages_valid = !plan.capital_stages.is_empty()
        && plan.capital_stages.iter().enumerate().all(|(i, s)| {
            s.index == u32::try_from(i).unwrap_or(u32::MAX)
                && s.capital_ceiling_micros > 0
                && s.exposure_ceiling_micros > 0
                && s.exposure_ceiling_micros <= s.capital_ceiling_micros
                && s.session_loss_ceiling_micros > 0
                && s.session_loss_ceiling_micros <= s.exposure_ceiling_micros
                && (i == 0
                    || s.capital_ceiling_micros > plan.capital_stages[i - 1].capital_ceiling_micros)
        });
    plan.verify_digest(policy)
        && plan.required_scenarios == ReleaseScenario::ALL
        && nonzero_subjects
        && regions.len() == plan.required_regions.len()
        && regions.len() >= policy.minimum_regions
        && !regions.contains(&[0; 32])
        && stages_valid
        && plan.created_at_ns <= at
        && plan.expires_at_ns >= at
        && plan.expires_at_ns - plan.created_at_ns <= policy.maximum_plan_lifetime_ns
}

fn valid_case(case: &ReleaseCase, plan: &ReleasePlan, policy: &ReleasePolicy, at: i64) -> bool {
    if usize::try_from(case.stage_index).map_or(true, |i| i >= plan.capital_stages.len())
        || case.observed_at_ns > at
        || !case.no_trade_available
    {
        return false;
    }
    let stale = at - case.observed_at_ns > policy.maximum_evidence_age_ns;
    match case.scenario {
        ReleaseScenario::NoTrade => case.disposition == ReleaseDisposition::NoTrade,
        ReleaseScenario::StagedCeilings
        | ReleaseScenario::MultiRegionHealth
        | ReleaseScenario::Governance => {
            !stale && case.disposition == ReleaseDisposition::CodeEligible
        }
        ReleaseScenario::ContinuousReconciliation => {
            !stale
                && case.reconciliation_current
                && case.disposition == ReleaseDisposition::CodeEligible
        }
        ReleaseScenario::EvidenceExpiry => stale && case.disposition == ReleaseDisposition::NoTrade,
        ReleaseScenario::IncidentResponse => {
            case.incident_process_proven && case.disposition == ReleaseDisposition::RollbackRequired
        }
        ReleaseScenario::DisasterRecovery => {
            case.disaster_recovery_proven
                && case.disposition == ReleaseDisposition::RollbackRequired
        }
        ReleaseScenario::Rollback => {
            case.rollback_proven && case.disposition == ReleaseDisposition::RollbackRequired
        }
        ReleaseScenario::Revocation => {
            case.revocation_proven && case.disposition == ReleaseDisposition::NoTrade
        }
    }
}

fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(&serde_json::to_vec(value).expect("bounded serialization"));
    *hasher.finalize().as_bytes()
}

fn digest_without<T: Clone + Serialize>(
    domain: &[u8],
    value: &T,
    clear: impl FnOnce(&mut T),
) -> [u8; 32] {
    let mut copy = value.clone();
    clear(&mut copy);
    digest_json(domain, &copy)
}

/// Encodes a strict bounded command.
/// # Errors
/// Rejects serialization and oversized commands.
pub fn encode_command(command: &ReleaseCommand) -> Result<Vec<u8>, Error> {
    let bytes =
        serde_json::to_vec(&(WIRE_VERSION, command)).map_err(|e| Error::Json(e.to_string()))?;
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    Ok(bytes)
}

/// Decodes a strict canonical command.
/// # Errors
/// Rejects malformed, trailing, noncanonical, oversized or unsupported data.
pub fn decode_command(bytes: &[u8]) -> Result<ReleaseCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let (version, command): (u16, ReleaseCommand) =
        Deserialize::deserialize(&mut deserializer).map_err(|e| Error::Json(e.to_string()))?;
    deserializer.end().map_err(|e| Error::Json(e.to_string()))?;
    if version != WIRE_VERSION {
        return Err(Error::Version(version));
    }
    if encode_command(&command)? != bytes {
        return Err(Error::Json("noncanonical".into()));
    }
    Ok(command)
}

#[cfg(test)]
mod tests;
