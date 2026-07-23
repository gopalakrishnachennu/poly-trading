#![forbid(unsafe_code)]

//! Deterministic accelerated certification of continuous read-only operation.
//!
//! Logical campaign duration never represents real elapsed environment time.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, CampaignCheckpoint,
    CampaignRecovery, CampaignStorageError, DurableCampaign,
};
pub use report::{read_report, write_report_create_new, CampaignReportFileError};

use chain_observer::{ChainReport, ChainReportStatus, ChainScenario};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CampaignCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignPolicy {
    pub maximum_chain_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_tick_age_ns: i64,
    pub minimum_accelerated_duration_ns: i64,
    pub minimum_rollovers: u64,
    pub maximum_queue_depth: u64,
    pub maximum_memory_bytes: u64,
    pub maximum_open_files: u64,
    pub maximum_journal_bytes: u64,
    pub maximum_latency_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSubjects {
    pub artifact_digest: [u8; 32],
    pub configuration_digest: [u8; 32],
    pub venue_runtime_digest: [u8; 32],
    pub chain_runtime_digest: [u8; 32],
    pub checkpoint_schema_digest: [u8; 32],
    pub subjects_digest: [u8; 32],
}
impl RuntimeSubjects {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.subjects_digest = digest_without(b"shadow-runtime-subjects-v1", &self, |v| {
            v.subjects_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.subjects_digest
            == digest_without(b"shadow-runtime-subjects-v1", self, |v| {
                v.subjects_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceSample {
    pub queue_depth: u64,
    pub memory_bytes: u64,
    pub open_files: u64,
    pub journal_bytes: u64,
    pub maximum_latency_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignTick {
    pub tick_id: [u8; 32],
    pub sequence: u64,
    pub hour_index: u64,
    pub logical_time_ns: i64,
    pub event_time_ns: i64,
    pub received_time_ns: i64,
    pub observed_at_ns: i64,
    pub venue_state_digest: [u8; 32],
    pub chain_state_digest: [u8; 32],
    pub resources: ResourceSample,
    pub healthy: bool,
    pub tick_digest: [u8; 32],
}
impl CampaignTick {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.tick_digest = digest_without(b"continuous-shadow-tick-v1", &self, |v| {
            v.tick_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.tick_digest
            == digest_without(b"continuous-shadow-tick-v1", self, |v| {
                v.tick_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignScenario {
    SteadyOperation,
    ResourceBudgets,
    HourlyRollover,
    CheckpointRestart,
    VenuePartition,
    ChainPartition,
    DeadMan,
    ClockRegression,
    DurableCorruption,
}
impl CampaignScenario {
    pub const ALL: [Self; 9] = [
        Self::SteadyOperation,
        Self::ResourceBudgets,
        Self::HourlyRollover,
        Self::CheckpointRestart,
        Self::VenuePartition,
        Self::ChainPartition,
        Self::DeadMan,
        Self::ClockRegression,
        Self::DurableCorruption,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisruptionKind {
    CheckpointRestart,
    VenuePartition,
    ChainPartition,
    DeadMan,
}
impl DisruptionKind {
    const fn scenario(self) -> CampaignScenario {
        match self {
            Self::CheckpointRestart => CampaignScenario::CheckpointRestart,
            Self::VenuePartition => CampaignScenario::VenuePartition,
            Self::ChainPartition => CampaignScenario::ChainPartition,
            Self::DeadMan => CampaignScenario::DeadMan,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequirement {
    pub requirement_id: [u8; 32],
    pub kind: DisruptionKind,
    pub prior_tick_digest: [u8; 32],
    pub prior_sequence: u64,
    pub checkpoint_digest: [u8; 32],
    pub trigger_digest: [u8; 32],
    pub requirement_digest: [u8; 32],
}
impl RecoveryRequirement {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.requirement_digest
            == digest_without(b"shadow-recovery-requirement-v1", self, |v| {
                v.requirement_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecoveryEvidence {
    pub evidence_id: [u8; 32],
    pub requirement_digest: [u8; 32],
    pub checkpoint_digest: [u8; 32],
    pub tick: CampaignTick,
    pub no_mutation_observed: bool,
    pub credential_present: bool,
    pub connection_opened: bool,
    pub wallet_action_observed: bool,
    pub evidence_digest: [u8; 32],
}
impl RecoveryEvidence {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.evidence_digest = digest_without(b"shadow-recovery-evidence-v1", &self, |v| {
            v.evidence_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evidence_digest
            == digest_without(b"shadow-recovery-evidence-v1", self, |v| {
                v.evidence_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrityFixture {
    pub fixture_id: [u8; 32],
    pub scenario: CampaignScenario,
    pub trigger_digest: [u8; 32],
    pub isolated: bool,
    pub halted: bool,
    pub state_contribution: bool,
    pub fixture_digest: [u8; 32],
}
impl IntegrityFixture {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.fixture_digest = digest_without(b"shadow-integrity-fixture-v1", &self, |v| {
            v.fixture_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.fixture_digest
            == digest_without(b"shadow-integrity-fixture-v1", self, |v| {
                v.fixture_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignPlan {
    pub plan_id: [u8; 32],
    pub chain_report: ChainReport,
    pub subjects: RuntimeSubjects,
    pub required_scenarios: Vec<CampaignScenario>,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}
impl CampaignPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &CampaignPolicy) -> Self {
        self.required_scenarios.sort();
        self.policy_digest = digest_json(b"continuous-shadow-policy-v1", policy);
        self.plan_digest = digest_without(b"continuous-shadow-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self, policy: &CampaignPolicy) -> bool {
        self.policy_digest == digest_json(b"continuous-shadow-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"continuous-shadow-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignReportStatus {
    LocallyCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct CampaignReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub chain_report_digest: [u8; 32],
    pub final_tick_digest: [u8; 32],
    pub accelerated_duration_ns: i64,
    pub real_elapsed_duration_ns: i64,
    pub rollover_count: u64,
    pub covered_scenarios: Vec<CampaignScenario>,
    pub operations_operator_digest: [u8; 32],
    pub risk_operator_digest: [u8; 32],
    pub finalized_at_ns: i64,
    pub status: CampaignReportStatus,
    pub real_multi_day_environment_certified: bool,
    pub credential_material_created: bool,
    pub external_connection_opened: bool,
    pub external_mutation_observed: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub submission_authority_granted: bool,
    pub report_digest: [u8; 32],
}
impl CampaignReport {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"continuous-shadow-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"continuous-shadow-report-v1", self, |v| {
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
pub enum CampaignCommand {
    Register {
        command_id: CampaignCommandId,
        plan: Box<CampaignPlan>,
        recorded_at_ns: i64,
    },
    ObserveTick {
        command_id: CampaignCommandId,
        tick: CampaignTick,
        recorded_at_ns: i64,
    },
    Disrupt {
        command_id: CampaignCommandId,
        requirement_id: [u8; 32],
        kind: DisruptionKind,
        checkpoint_digest: [u8; 32],
        trigger_digest: [u8; 32],
        recorded_at_ns: i64,
    },
    Recover {
        command_id: CampaignCommandId,
        requirement: Box<RecoveryRequirement>,
        evidence: Box<RecoveryEvidence>,
        recorded_at_ns: i64,
    },
    RecordIntegrityFixture {
        command_id: CampaignCommandId,
        fixture: IntegrityFixture,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: CampaignCommandId,
        report_id: [u8; 32],
        operations_operator_digest: [u8; 32],
        risk_operator_digest: [u8; 32],
        real_elapsed_duration_ns: i64,
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}
impl CampaignCommand {
    #[must_use]
    pub const fn command_id(&self) -> CampaignCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::ObserveTick { command_id, .. }
            | Self::Disrupt { command_id, .. }
            | Self::Recover { command_id, .. }
            | Self::RecordIntegrityFixture { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::ObserveTick { recorded_at_ns, .. }
            | Self::Disrupt { recorded_at_ns, .. }
            | Self::Recover { recorded_at_ns, .. }
            | Self::RecordIntegrityFixture { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CampaignDetail {
    Registered,
    TickAccepted,
    RecoveryRequired(Box<RecoveryRequirement>),
    Recovered,
    FixtureRecorded,
    Finalized(Box<CampaignReport>),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CampaignOutcome {
    pub command_id: CampaignCommandId,
    pub detail: CampaignDetail,
    pub outcome_digest: [u8; 32],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CampaignSnapshot {
    pub tick: Option<CampaignTick>,
    pub recovery: Option<RecoveryRequirement>,
    pub covered_scenarios: BTreeSet<CampaignScenario>,
    pub rollover_count: u64,
    pub ready: bool,
    pub report: Option<CampaignReport>,
    pub accepted_commands: u64,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("campaign policy invalid")]
    Config,
    #[error("campaign timestamp invalid or regressed")]
    Timestamp,
    #[error("campaign command exceeds bound")]
    CommandBound,
    #[error("campaign JSON invalid: {0}")]
    Json(String),
    #[error("unsupported campaign command version: {0}")]
    Version(u16),
    #[error("campaign command id conflict")]
    IdempotencyConflict,
    #[error("Phase 3.3 evidence invalid, stale, incomplete, or authority-bearing")]
    Upstream,
    #[error("continuous shadow plan invalid")]
    Plan,
    #[error("campaign tick invalid, stale, over budget, or discontinuous")]
    Tick,
    #[error("campaign disruption or recovery invalid")]
    Recovery,
    #[error("integrity fixture invalid")]
    Fixture,
    #[error("campaign finalization invalid")]
    Finalize,
    #[error("campaign arithmetic overflow")]
    Overflow,
    #[error("campaign halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct ContinuousCampaign {
    policy: CampaignPolicy,
    plan: Option<CampaignPlan>,
    first_logical_time_ns: Option<i64>,
    tick: Option<CampaignTick>,
    recovery: Option<RecoveryRequirement>,
    covered: BTreeSet<CampaignScenario>,
    rollovers: u64,
    used_ticks: BTreeSet<[u8; 32]>,
    used_fixtures: BTreeSet<[u8; 32]>,
    processed: BTreeMap<CampaignCommandId, ([u8; 32], CampaignOutcome)>,
    accepted_commands: u64,
    report: Option<CampaignReport>,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl ContinuousCampaign {
    /// Creates an empty campaign.
    /// # Errors
    /// Rejects non-positive or inconsistent policy bounds.
    pub fn new(policy: CampaignPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            first_logical_time_ns: None,
            tick: None,
            recovery: None,
            covered: BTreeSet::new(),
            rollovers: 0,
            used_ticks: BTreeSet::new(),
            used_fixtures: BTreeSet::new(),
            processed: BTreeMap::new(),
            accepted_commands: 0,
            report: None,
            last_recorded_at_ns: None,
            halted: None,
        })
    }
    /// Applies one deterministic campaign command.
    /// # Errors
    /// Invalid chronology, evidence, recovery or finalization halts.
    pub fn apply(&mut self, command: &CampaignCommand) -> Result<CampaignOutcome, Error> {
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
        let mut outcome = CampaignOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"continuous-shadow-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &CampaignCommand) -> Result<CampaignDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            CampaignCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some()
                    || !valid_upstream(&plan.chain_report, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(CampaignDetail::Registered)
            }
            CampaignCommand::ObserveTick {
                tick,
                recorded_at_ns,
                ..
            } => {
                if self.recovery.is_some()
                    || self.used_ticks.contains(&tick.tick_id)
                    || !valid_tick(tick, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Tick);
                }
                if let Some(prior) = &self.tick {
                    if tick.sequence != prior.sequence.checked_add(1).ok_or(Error::Overflow)?
                        || tick.logical_time_ns <= prior.logical_time_ns
                        || tick.event_time_ns < prior.event_time_ns
                        || tick.received_time_ns < prior.received_time_ns
                        || !(tick.hour_index == prior.hour_index
                            || tick.hour_index
                                == prior.hour_index.checked_add(1).ok_or(Error::Overflow)?)
                    {
                        return Err(Error::Tick);
                    }
                    if tick.hour_index > prior.hour_index {
                        self.rollovers = self.rollovers.checked_add(1).ok_or(Error::Overflow)?;
                        self.covered.insert(CampaignScenario::HourlyRollover);
                    }
                } else if tick.sequence != 1 {
                    return Err(Error::Tick);
                }
                self.first_logical_time_ns
                    .get_or_insert(tick.logical_time_ns);
                self.used_ticks.insert(tick.tick_id);
                self.tick = Some(tick.clone());
                self.covered.insert(CampaignScenario::SteadyOperation);
                self.covered.insert(CampaignScenario::ResourceBudgets);
                Ok(CampaignDetail::TickAccepted)
            }
            CampaignCommand::Disrupt {
                requirement_id,
                kind,
                checkpoint_digest,
                trigger_digest,
                ..
            } => {
                let prior = self.tick.take().ok_or(Error::Recovery)?;
                if self.recovery.is_some()
                    || *checkpoint_digest == [0; 32]
                    || *trigger_digest == [0; 32]
                {
                    return Err(Error::Recovery);
                }
                let mut requirement = RecoveryRequirement {
                    requirement_id: *requirement_id,
                    kind: *kind,
                    prior_tick_digest: prior.tick_digest,
                    prior_sequence: prior.sequence,
                    checkpoint_digest: *checkpoint_digest,
                    trigger_digest: *trigger_digest,
                    requirement_digest: [0; 32],
                };
                requirement.requirement_digest =
                    digest_without(b"shadow-recovery-requirement-v1", &requirement, |v| {
                        v.requirement_digest = [0; 32];
                    });
                self.recovery = Some(requirement.clone());
                Ok(CampaignDetail::RecoveryRequired(Box::new(requirement)))
            }
            CampaignCommand::Recover {
                requirement,
                evidence,
                recorded_at_ns,
                ..
            } => {
                let expected = self.recovery.as_ref().ok_or(Error::Recovery)?;
                if requirement.as_ref() != expected
                    || !requirement.verify_digest()
                    || !evidence.verify_digest()
                    || evidence.requirement_digest != requirement.requirement_digest
                    || evidence.checkpoint_digest != requirement.checkpoint_digest
                    || !evidence.no_mutation_observed
                    || evidence.credential_present
                    || evidence.connection_opened
                    || evidence.wallet_action_observed
                    || evidence.tick.sequence
                        != requirement
                            .prior_sequence
                            .checked_add(1)
                            .ok_or(Error::Overflow)?
                    || self.used_ticks.contains(&evidence.tick.tick_id)
                    || !valid_tick(&evidence.tick, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Recovery);
                }
                self.used_ticks.insert(evidence.tick.tick_id);
                self.tick = Some(evidence.tick.clone());
                self.covered.insert(requirement.kind.scenario());
                self.recovery = None;
                Ok(CampaignDetail::Recovered)
            }
            CampaignCommand::RecordIntegrityFixture { fixture, .. } => {
                if self.used_fixtures.contains(&fixture.fixture_id)
                    || !fixture.verify_digest()
                    || !matches!(
                        fixture.scenario,
                        CampaignScenario::ClockRegression | CampaignScenario::DurableCorruption
                    )
                    || !fixture.isolated
                    || !fixture.halted
                    || fixture.state_contribution
                {
                    return Err(Error::Fixture);
                }
                self.used_fixtures.insert(fixture.fixture_id);
                self.covered.insert(fixture.scenario);
                Ok(CampaignDetail::FixtureRecorded)
            }
            CampaignCommand::Finalize {
                report_id,
                operations_operator_digest,
                risk_operator_digest,
                real_elapsed_duration_ns,
                finalized_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                let tick = self.tick.as_ref().ok_or(Error::Finalize)?;
                let first = self.first_logical_time_ns.ok_or(Error::Finalize)?;
                let accelerated = tick
                    .logical_time_ns
                    .checked_sub(first)
                    .ok_or(Error::Overflow)?;
                if self.recovery.is_some()
                    || !tick.healthy
                    || !valid_tick(tick, &self.policy, *finalized_at_ns)
                    || accelerated < self.policy.minimum_accelerated_duration_ns
                    || self.rollovers < self.policy.minimum_rollovers
                    || !plan
                        .required_scenarios
                        .iter()
                        .all(|v| self.covered.contains(v))
                    || *operations_operator_digest == [0; 32]
                    || *risk_operator_digest == [0; 32]
                    || operations_operator_digest == risk_operator_digest
                    || *real_elapsed_duration_ns < 0
                    || *finalized_at_ns > plan.expires_at_ns
                {
                    return Err(Error::Finalize);
                }
                let report = CampaignReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    chain_report_digest: plan.chain_report.report_digest,
                    final_tick_digest: tick.tick_digest,
                    accelerated_duration_ns: accelerated,
                    real_elapsed_duration_ns: *real_elapsed_duration_ns,
                    rollover_count: self.rollovers,
                    covered_scenarios: self.covered.iter().copied().collect(),
                    operations_operator_digest: *operations_operator_digest,
                    risk_operator_digest: *risk_operator_digest,
                    finalized_at_ns: *finalized_at_ns,
                    status: CampaignReportStatus::LocallyCertified,
                    real_multi_day_environment_certified: false,
                    credential_material_created: false,
                    external_connection_opened: false,
                    external_mutation_observed: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    submission_authority_granted: false,
                    report_digest: [0; 32],
                }
                .sealed();
                self.report = Some(report.clone());
                Ok(CampaignDetail::Finalized(Box::new(report)))
            }
        }
    }
    #[must_use]
    pub fn snapshot(&self, at: i64) -> CampaignSnapshot {
        let ready = self.recovery.is_none()
            && self
                .tick
                .as_ref()
                .is_some_and(|v| v.healthy && valid_tick(v, &self.policy, at));
        let material = (
            &self.tick,
            &self.recovery,
            &self.covered,
            self.rollovers,
            ready,
            &self.report,
            self.accepted_commands,
            &self.halted,
        );
        CampaignSnapshot {
            tick: self.tick.clone(),
            recovery: self.recovery.clone(),
            covered_scenarios: self.covered.clone(),
            rollover_count: self.rollovers,
            ready,
            report: self.report.clone(),
            accepted_commands: self.accepted_commands,
            halted: self.halted.is_some(),
            digest: digest_json(b"continuous-shadow-state-v1", &material),
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

fn validate_policy(p: &CampaignPolicy) -> Result<(), Error> {
    if p.maximum_chain_report_age_ns <= 0
        || p.maximum_plan_lifetime_ns <= 0
        || p.maximum_tick_age_ns <= 0
        || p.minimum_accelerated_duration_ns <= 0
        || p.minimum_rollovers == 0
        || p.maximum_queue_depth == 0
        || p.maximum_memory_bytes == 0
        || p.maximum_open_files == 0
        || p.maximum_journal_bytes == 0
        || p.maximum_latency_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}
fn valid_upstream(r: &ChainReport, p: &CampaignPolicy, at: i64) -> bool {
    r.verify_digest()
        && r.status == ChainReportStatus::LocallyCertified
        && r.covered_scenarios == ChainScenario::ALL
        && r.finalized_at_ns <= at
        && at - r.finalized_at_ns <= p.maximum_chain_report_age_ns
        && !r.live_environment_certified
        && !r.rpc_connection_opened
        && !r.credential_material_created
        && !r.wallet_access_granted
        && !r.signature_produced
        && !r.transaction_submitted
        && !r.deployment_authority_granted
        && !r.trading_authority_granted
        && !r.submission_authority_granted
}
fn valid_plan(plan: &CampaignPlan, p: &CampaignPolicy, at: i64) -> bool {
    plan.verify_digest(p)
        && plan.subjects.verify_digest()
        && plan.required_scenarios == CampaignScenario::ALL
        && plan.created_at_ns <= at
        && plan.expires_at_ns >= at
        && plan.expires_at_ns - plan.created_at_ns <= p.maximum_plan_lifetime_ns
}
fn valid_tick(t: &CampaignTick, p: &CampaignPolicy, at: i64) -> bool {
    t.verify_digest()
        && t.sequence > 0
        && t.logical_time_ns >= 0
        && t.event_time_ns <= t.received_time_ns
        && t.received_time_ns <= t.observed_at_ns
        && t.observed_at_ns <= at
        && at - t.observed_at_ns <= p.maximum_tick_age_ns
        && t.healthy
        && t.venue_state_digest != [0; 32]
        && t.chain_state_digest != [0; 32]
        && t.resources.queue_depth <= p.maximum_queue_depth
        && t.resources.memory_bytes <= p.maximum_memory_bytes
        && t.resources.open_files <= p.maximum_open_files
        && t.resources.journal_bytes <= p.maximum_journal_bytes
        && t.resources.maximum_latency_ns >= 0
        && t.resources.maximum_latency_ns <= p.maximum_latency_ns
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

/// Encodes one bounded versioned campaign command.
/// # Errors
/// Rejects serialization or size failure.
pub fn encode_command(command: &CampaignCommand) -> Result<Vec<u8>, Error> {
    let bytes =
        serde_json::to_vec(&(WIRE_VERSION, command)).map_err(|e| Error::Json(e.to_string()))?;
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    Ok(bytes)
}
/// Decodes one strict canonical campaign command.
/// # Errors
/// Rejects malformed, trailing, noncanonical, oversized or unsupported data.
pub fn decode_command(bytes: &[u8]) -> Result<CampaignCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let (version, command): (u16, CampaignCommand) =
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
