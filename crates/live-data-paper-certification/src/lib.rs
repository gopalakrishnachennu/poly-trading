#![forbid(unsafe_code)]

//! Point-in-time-correct certification of paper trading over captured data.
//!
//! The crate consumes immutable evidence only. It has no feed, credential,
//! signer, wallet, capital, or submission capability.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurablePaperCertification,
    PaperCheckpoint, PaperRecovery, PaperStorageError,
};
pub use report::{read_report, write_report_create_new, PaperReportFileError};

use continuous_shadow_certification::{CampaignReport, CampaignReportStatus, CampaignScenario};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PaperCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaperPolicy {
    pub maximum_campaign_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_record_age_ns: i64,
    pub maximum_records: usize,
    pub maximum_latency_ns: i64,
    pub minimum_fold_evaluations: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoldKind {
    Train,
    Validation,
    Test,
}
impl FoldKind {
    pub const ALL: [Self; 3] = [Self::Train, Self::Validation, Self::Test];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkForwardFold {
    pub kind: FoldKind,
    pub start_available_time_ns: i64,
    pub end_available_time_ns: i64,
    pub fold_digest: [u8; 32],
}
impl WalkForwardFold {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.fold_digest = digest_without(b"paper-fold-v1", &self, |v| {
            v.fold_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.fold_digest
            == digest_without(b"paper-fold-v1", self, |v| {
                v.fold_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedRecord {
    pub record_id: [u8; 32],
    pub sequence: u64,
    pub event_time_ns: i64,
    pub received_time_ns: i64,
    pub available_time_ns: i64,
    pub provenance_digest: [u8; 32],
    pub payload_digest: [u8; 32],
    pub record_digest: [u8; 32],
}
impl CapturedRecord {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.record_digest = digest_without(b"captured-paper-record-v1", &self, |v| {
            v.record_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.record_digest
            == digest_without(b"captured-paper-record-v1", self, |v| {
                v.record_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueCase {
    Optimistic,
    Estimated,
    Conservative,
}
impl QueueCase {
    pub const ALL: [Self; 3] = [Self::Optimistic, Self::Estimated, Self::Conservative];
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperOutcome {
    ZeroFill,
    PartialFill,
    FullFill,
    Unknown,
    CancelRace,
}
impl PaperOutcome {
    pub const ALL: [Self; 5] = [
        Self::ZeroFill,
        Self::PartialFill,
        Self::FullFill,
        Self::Unknown,
        Self::CancelRace,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatencyProfile {
    pub signal_ns: i64,
    pub submission_ns: i64,
    pub acknowledgement_ns: i64,
    pub cancellation_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct PaperEvaluation {
    pub evaluation_id: [u8; 32],
    pub fold: FoldKind,
    pub decision_time_ns: i64,
    pub consumed_sequences: Vec<u64>,
    pub queue_cases: Vec<QueueCase>,
    pub outcomes: Vec<PaperOutcome>,
    pub latency: LatencyProfile,
    pub price_touch_only_fill: bool,
    pub unknown_retains_reservation: bool,
    pub proposal_digest: [u8; 32],
    pub risk_digest: [u8; 32],
    pub reservation_digest: [u8; 32],
    pub execution_digest: [u8; 32],
    pub settlement_digest: [u8; 32],
    pub accounting_digest: [u8; 32],
    pub external_mutation_observed: bool,
    pub evaluation_digest: [u8; 32],
}
impl PaperEvaluation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.consumed_sequences.sort_unstable();
        self.queue_cases.sort();
        self.outcomes.sort();
        self.evaluation_digest = digest_without(b"paper-evaluation-v1", &self, |v| {
            v.evaluation_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evaluation_digest
            == digest_without(b"paper-evaluation-v1", self, |v| {
                v.evaluation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperScenario {
    CapturedChronology,
    AvailabilityGate,
    QueueCases,
    Latency,
    PartialFill,
    UnknownRetention,
    CancelRace,
    WalkForward,
    FrozenTest,
    DownstreamBinding,
}
impl PaperScenario {
    pub const ALL: [Self; 10] = [
        Self::CapturedChronology,
        Self::AvailabilityGate,
        Self::QueueCases,
        Self::Latency,
        Self::PartialFill,
        Self::UnknownRetention,
        Self::CancelRace,
        Self::WalkForward,
        Self::FrozenTest,
        Self::DownstreamBinding,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaperPlan {
    pub plan_id: [u8; 32],
    pub campaign_report: CampaignReport,
    pub capture_manifest_digest: [u8; 32],
    pub strategy_digest: [u8; 32],
    pub expected_record_count: usize,
    pub folds: Vec<WalkForwardFold>,
    pub required_scenarios: Vec<PaperScenario>,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}
impl PaperPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &PaperPolicy) -> Self {
        self.folds.sort_by_key(|v| v.kind);
        self.required_scenarios.sort();
        self.policy_digest = digest_json(b"paper-cert-policy-v1", policy);
        self.plan_digest = digest_without(b"paper-cert-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self, policy: &PaperPolicy) -> bool {
        self.policy_digest == digest_json(b"paper-cert-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"paper-cert-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperReportStatus {
    LocallyCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct PaperReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub campaign_report_digest: [u8; 32],
    pub capture_manifest_digest: [u8; 32],
    pub strategy_digest: [u8; 32],
    pub record_count: usize,
    pub evaluation_count: u64,
    pub covered_scenarios: Vec<PaperScenario>,
    pub finalized_at_ns: i64,
    pub status: PaperReportStatus,
    pub real_pnl_observed: bool,
    pub credential_material_created: bool,
    pub external_connection_opened: bool,
    pub external_mutation_observed: bool,
    pub capital_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub submission_authority_granted: bool,
    pub report_digest: [u8; 32],
}
impl PaperReport {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"paper-cert-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"paper-cert-report-v1", self, |v| {
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
pub enum PaperCommand {
    Register {
        command_id: PaperCommandId,
        plan: Box<PaperPlan>,
        recorded_at_ns: i64,
    },
    Ingest {
        command_id: PaperCommandId,
        record: CapturedRecord,
        recorded_at_ns: i64,
    },
    FreezeStrategy {
        command_id: PaperCommandId,
        strategy_digest: [u8; 32],
        frozen_at_ns: i64,
        recorded_at_ns: i64,
    },
    Evaluate {
        command_id: PaperCommandId,
        evaluation: Box<PaperEvaluation>,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: PaperCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}
impl PaperCommand {
    #[must_use]
    pub const fn command_id(&self) -> PaperCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::Ingest { command_id, .. }
            | Self::FreezeStrategy { command_id, .. }
            | Self::Evaluate { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::Ingest { recorded_at_ns, .. }
            | Self::FreezeStrategy { recorded_at_ns, .. }
            | Self::Evaluate { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PaperDetail {
    Registered,
    RecordAccepted,
    StrategyFrozen,
    EvaluationAccepted,
    Finalized(Box<PaperReport>),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PaperOutcomeRecord {
    pub command_id: PaperCommandId,
    pub detail: PaperDetail,
    pub outcome_digest: [u8; 32],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaperSnapshot {
    pub records: Vec<CapturedRecord>,
    pub frozen_at_ns: Option<i64>,
    pub evaluations: u64,
    pub covered_scenarios: BTreeSet<PaperScenario>,
    pub report: Option<PaperReport>,
    pub accepted_commands: u64,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("paper certification policy invalid")]
    Config,
    #[error("paper certification timestamp invalid or regressed")]
    Timestamp,
    #[error("paper certification command exceeds bound")]
    CommandBound,
    #[error("paper certification JSON invalid: {0}")]
    Json(String),
    #[error("unsupported paper certification command version: {0}")]
    Version(u16),
    #[error("paper certification command id conflict")]
    IdempotencyConflict,
    #[error("Phase 3.4 evidence invalid, stale, incomplete, or authority-bearing")]
    Upstream,
    #[error("paper certification plan invalid")]
    Plan,
    #[error("captured record invalid or discontinuous")]
    Record,
    #[error("strategy freeze invalid")]
    Freeze,
    #[error("paper evaluation invalid or point-in-time unsafe")]
    Evaluation,
    #[error("paper certification finalization invalid")]
    Finalize,
    #[error("paper certification arithmetic overflow")]
    Overflow,
    #[error("paper certification halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct LiveDataPaperCertification {
    policy: PaperPolicy,
    plan: Option<PaperPlan>,
    records: Vec<CapturedRecord>,
    used_records: BTreeSet<[u8; 32]>,
    frozen_at_ns: Option<i64>,
    fold_counts: BTreeMap<FoldKind, u64>,
    evaluations: u64,
    used_evaluations: BTreeSet<[u8; 32]>,
    covered: BTreeSet<PaperScenario>,
    processed: BTreeMap<PaperCommandId, ([u8; 32], PaperOutcomeRecord)>,
    accepted_commands: u64,
    report: Option<PaperReport>,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl LiveDataPaperCertification {
    /// Creates an empty certification owner.
    /// # Errors
    /// Rejects invalid policy bounds.
    pub fn new(policy: PaperPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            records: Vec::new(),
            used_records: BTreeSet::new(),
            frozen_at_ns: None,
            fold_counts: BTreeMap::new(),
            evaluations: 0,
            used_evaluations: BTreeSet::new(),
            covered: BTreeSet::new(),
            processed: BTreeMap::new(),
            accepted_commands: 0,
            report: None,
            last_recorded_at_ns: None,
            halted: None,
        })
    }
    /// Applies one deterministic certification command.
    /// # Errors
    /// Invalid chronology, evidence, folds or finalization halt.
    pub fn apply(&mut self, command: &PaperCommand) -> Result<PaperOutcomeRecord, Error> {
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
        let mut outcome = PaperOutcomeRecord {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"paper-cert-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }
    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &PaperCommand) -> Result<PaperDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            PaperCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some()
                    || !valid_upstream(&plan.campaign_report, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(PaperDetail::Registered)
            }
            PaperCommand::Ingest {
                record,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Record)?;
                if self.frozen_at_ns.is_some()
                    || self.records.len() >= plan.expected_record_count
                    || self.used_records.contains(&record.record_id)
                    || !record.verify_digest()
                    || record.sequence
                        != u64::try_from(self.records.len())
                            .map_err(|_| Error::Overflow)?
                            .checked_add(1)
                            .ok_or(Error::Overflow)?
                    || record.event_time_ns > record.received_time_ns
                    || record.received_time_ns > record.available_time_ns
                    || record.available_time_ns > *recorded_at_ns
                    || *recorded_at_ns - record.available_time_ns
                        > self.policy.maximum_record_age_ns
                    || record.provenance_digest == [0; 32]
                    || record.payload_digest == [0; 32]
                    || self
                        .records
                        .last()
                        .is_some_and(|v| record.available_time_ns < v.available_time_ns)
                {
                    return Err(Error::Record);
                }
                self.used_records.insert(record.record_id);
                self.records.push(record.clone());
                self.covered.insert(PaperScenario::CapturedChronology);
                self.covered.insert(PaperScenario::AvailabilityGate);
                Ok(PaperDetail::RecordAccepted)
            }
            PaperCommand::FreezeStrategy {
                strategy_digest,
                frozen_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Freeze)?;
                if self.frozen_at_ns.is_some()
                    || *strategy_digest != plan.strategy_digest
                    || self.records.len() != plan.expected_record_count
                    || *frozen_at_ns < plan.folds[1].end_available_time_ns
                    || *frozen_at_ns >= plan.folds[2].start_available_time_ns
                {
                    return Err(Error::Freeze);
                }
                self.frozen_at_ns = Some(*frozen_at_ns);
                Ok(PaperDetail::StrategyFrozen)
            }
            PaperCommand::Evaluate { evaluation, .. } => {
                let plan = self.plan.as_ref().ok_or(Error::Evaluation)?;
                let fold = plan
                    .folds
                    .iter()
                    .find(|v| v.kind == evaluation.fold)
                    .ok_or(Error::Evaluation)?;
                if self.used_evaluations.contains(&evaluation.evaluation_id)
                    || !evaluation.verify_digest()
                    || evaluation.queue_cases != QueueCase::ALL
                    || evaluation.outcomes != PaperOutcome::ALL
                    || evaluation.price_touch_only_fill
                    || !evaluation.unknown_retains_reservation
                    || evaluation.external_mutation_observed
                    || !valid_latency(&evaluation.latency, &self.policy)
                    || evaluation.decision_time_ns < fold.start_available_time_ns
                    || evaluation.decision_time_ns > fold.end_available_time_ns
                    || evaluation.consumed_sequences.is_empty()
                    || evaluation
                        .consumed_sequences
                        .windows(2)
                        .any(|w| w[0] >= w[1])
                    || evaluation.consumed_sequences.iter().any(|sequence| {
                        self.records
                            .iter()
                            .find(|v| v.sequence == *sequence)
                            .is_none_or(|v| {
                                v.available_time_ns > evaluation.decision_time_ns
                                    || !within_fold(v.available_time_ns, fold)
                            })
                    })
                    || [
                        &evaluation.proposal_digest,
                        &evaluation.risk_digest,
                        &evaluation.reservation_digest,
                        &evaluation.execution_digest,
                        &evaluation.settlement_digest,
                        &evaluation.accounting_digest,
                    ]
                    .into_iter()
                    .any(|v| *v == [0; 32])
                    || (evaluation.fold == FoldKind::Test
                        && self
                            .frozen_at_ns
                            .is_none_or(|v| v > evaluation.decision_time_ns))
                {
                    return Err(Error::Evaluation);
                }
                self.used_evaluations.insert(evaluation.evaluation_id);
                self.evaluations = self.evaluations.checked_add(1).ok_or(Error::Overflow)?;
                *self.fold_counts.entry(evaluation.fold).or_insert(0) += 1;
                self.covered.extend([
                    PaperScenario::QueueCases,
                    PaperScenario::Latency,
                    PaperScenario::PartialFill,
                    PaperScenario::UnknownRetention,
                    PaperScenario::CancelRace,
                    PaperScenario::WalkForward,
                    PaperScenario::DownstreamBinding,
                ]);
                if evaluation.fold == FoldKind::Test {
                    self.covered.insert(PaperScenario::FrozenTest);
                }
                Ok(PaperDetail::EvaluationAccepted)
            }
            PaperCommand::Finalize {
                report_id,
                finalized_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                if self.records.len() != plan.expected_record_count
                    || self.frozen_at_ns.is_none()
                    || FoldKind::ALL.iter().any(|v| {
                        self.fold_counts.get(v).copied().unwrap_or(0)
                            < self.policy.minimum_fold_evaluations
                    })
                    || !plan
                        .required_scenarios
                        .iter()
                        .all(|v| self.covered.contains(v))
                    || *finalized_at_ns > plan.expires_at_ns
                {
                    return Err(Error::Finalize);
                }
                let report = PaperReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    campaign_report_digest: plan.campaign_report.report_digest,
                    capture_manifest_digest: plan.capture_manifest_digest,
                    strategy_digest: plan.strategy_digest,
                    record_count: self.records.len(),
                    evaluation_count: self.evaluations,
                    covered_scenarios: self.covered.iter().copied().collect(),
                    finalized_at_ns: *finalized_at_ns,
                    status: PaperReportStatus::LocallyCertified,
                    real_pnl_observed: false,
                    credential_material_created: false,
                    external_connection_opened: false,
                    external_mutation_observed: false,
                    capital_authority_granted: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    submission_authority_granted: false,
                    report_digest: [0; 32],
                }
                .sealed();
                self.report = Some(report.clone());
                Ok(PaperDetail::Finalized(Box::new(report)))
            }
        }
    }
    #[must_use]
    pub fn snapshot(&self) -> PaperSnapshot {
        let material = (
            &self.records,
            self.frozen_at_ns,
            self.evaluations,
            &self.covered,
            &self.report,
            self.accepted_commands,
            &self.halted,
        );
        PaperSnapshot {
            records: self.records.clone(),
            frozen_at_ns: self.frozen_at_ns,
            evaluations: self.evaluations,
            covered_scenarios: self.covered.clone(),
            report: self.report.clone(),
            accepted_commands: self.accepted_commands,
            halted: self.halted.is_some(),
            digest: digest_json(b"paper-cert-state-v1", &material),
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

fn validate_policy(p: &PaperPolicy) -> Result<(), Error> {
    if p.maximum_campaign_report_age_ns <= 0
        || p.maximum_plan_lifetime_ns <= 0
        || p.maximum_record_age_ns <= 0
        || p.maximum_records == 0
        || p.maximum_latency_ns <= 0
        || p.minimum_fold_evaluations == 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}
fn valid_upstream(r: &CampaignReport, p: &PaperPolicy, at: i64) -> bool {
    r.verify_digest()
        && r.status == CampaignReportStatus::LocallyCertified
        && r.covered_scenarios == CampaignScenario::ALL
        && r.finalized_at_ns <= at
        && at - r.finalized_at_ns <= p.maximum_campaign_report_age_ns
        && !r.real_multi_day_environment_certified
        && !r.credential_material_created
        && !r.external_connection_opened
        && !r.external_mutation_observed
        && !r.deployment_authority_granted
        && !r.trading_authority_granted
        && !r.submission_authority_granted
}
fn valid_plan(plan: &PaperPlan, p: &PaperPolicy, at: i64) -> bool {
    plan.verify_digest(p)
        && plan.capture_manifest_digest != [0; 32]
        && plan.strategy_digest != [0; 32]
        && plan.expected_record_count > 0
        && plan.expected_record_count <= p.maximum_records
        && plan.required_scenarios == PaperScenario::ALL
        && plan.folds.len() == 3
        && plan.folds.iter().map(|v| v.kind).collect::<Vec<_>>() == FoldKind::ALL
        && plan
            .folds
            .iter()
            .all(|v| v.verify_digest() && v.start_available_time_ns <= v.end_available_time_ns)
        && plan
            .folds
            .windows(2)
            .all(|w| w[0].end_available_time_ns < w[1].start_available_time_ns)
        && plan.created_at_ns <= at
        && plan.expires_at_ns >= at
        && plan.expires_at_ns - plan.created_at_ns <= p.maximum_plan_lifetime_ns
}
fn valid_latency(v: &LatencyProfile, p: &PaperPolicy) -> bool {
    [
        v.signal_ns,
        v.submission_ns,
        v.acknowledgement_ns,
        v.cancellation_ns,
    ]
    .into_iter()
    .all(|value| value >= 0 && value <= p.maximum_latency_ns)
}
fn within_fold(at: i64, fold: &WalkForwardFold) -> bool {
    at >= fold.start_available_time_ns && at <= fold.end_available_time_ns
}
fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(&serde_json::to_vec(value).expect("bounded internal serialization cannot fail"));
    *h.finalize().as_bytes()
}
fn digest_without<T: Clone + Serialize>(
    domain: &[u8],
    value: &T,
    clear: impl FnOnce(&mut T),
) -> [u8; 32] {
    let mut v = value.clone();
    clear(&mut v);
    digest_json(domain, &v)
}
/// Encodes one bounded versioned command.
/// # Errors
/// Rejects serialization or size failure.
pub fn encode_command(command: &PaperCommand) -> Result<Vec<u8>, Error> {
    let bytes =
        serde_json::to_vec(&(WIRE_VERSION, command)).map_err(|e| Error::Json(e.to_string()))?;
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    Ok(bytes)
}
/// Decodes one strict canonical command.
/// # Errors
/// Rejects malformed, trailing, noncanonical, oversized or unsupported data.
pub fn decode_command(bytes: &[u8]) -> Result<PaperCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let (version, command): (u16, PaperCommand) =
        Deserialize::deserialize(&mut de).map_err(|e| Error::Json(e.to_string()))?;
    de.end().map_err(|e| Error::Json(e.to_string()))?;
    if version != WIRE_VERSION {
        return Err(Error::Version(version));
    }
    if encode_command(&command)? != bytes {
        return Err(Error::Json("noncanonical command".into()));
    }
    Ok(command)
}

#[cfg(test)]
mod tests;
