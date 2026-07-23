#![forbid(unsafe_code)]

//! Deterministic offline deployment-adapter and disaster-recovery certification.
//!
//! This crate has no control-plane client and grants no external authority.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, CertificationCheckpoint,
    CertificationRecovery, CertificationStorageError, DurableCertification,
};
pub use report::{read_report, write_report_create_new, AdapterCertificationReportFileError};

use deployment_orchestration_simulator::{OrchestrationReport, OrchestrationReportStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 2;
const MAX_COMMAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_REGIONS_HARD: usize = 128;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CertificationCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationPolicy {
    pub maximum_regions: usize,
    pub maximum_report_age_ns: i64,
    pub maximum_campaign_age_ns: i64,
    pub maximum_fixture_age_ns: i64,
    pub maximum_recovery_duration_ns: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterOperation {
    ReadState,
    ServerSideDryRun,
    PlanApply,
    PlanTrafficShift,
    PlanRollback,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct AdapterPrivilegePolicy {
    pub allowed_operations: Vec<AdapterOperation>,
    pub allowed_resource_digest: [u8; 32],
    pub credential_material_allowed: bool,
    pub wildcard_resources_allowed: bool,
    pub secret_read_allowed: bool,
    pub cluster_admin_allowed: bool,
    pub arbitrary_exec_allowed: bool,
    pub privilege_escalation_allowed: bool,
    pub cross_region_mutation_allowed: bool,
    pub policy_digest: [u8; 32],
}

impl AdapterPrivilegePolicy {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.allowed_operations.sort();
        self.policy_digest = privilege_policy_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.policy_digest == privilege_policy_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentAdapterContract {
    pub contract_id: [u8; 32],
    pub interface_schema_digest: [u8; 32],
    pub deployment_manifest_digest: [u8; 32],
    pub rollback_manifest_digest: [u8; 32],
    pub recovery_runbook_digest: [u8; 32],
    pub regions: Vec<String>,
    pub privilege_policy: AdapterPrivilegePolicy,
    pub contract_digest: [u8; 32],
}

impl DeploymentAdapterContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.regions.sort();
        self.contract_digest = adapter_contract_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest == adapter_contract_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationCampaign {
    pub campaign_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub completion_report: OrchestrationReport,
    pub rollback_report: OrchestrationReport,
    pub adapter_contract: DeploymentAdapterContract,
    pub policy_digest: [u8; 32],
    pub campaign_digest: [u8; 32],
}

impl CertificationCampaign {
    #[must_use]
    pub fn sealed(mut self, policy: &CertificationPolicy) -> Self {
        self.policy_digest = digest_json(b"deployment-adapter-cert-policy-v1", policy);
        self.campaign_digest = campaign_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &CertificationPolicy) -> bool {
        self.policy_digest == digest_json(b"deployment-adapter-cert-policy-v1", policy)
            && self.campaign_digest == campaign_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureClass {
    DiscoverState,
    ServerSideDryRun,
    ApplyPlan,
    ObserveHealth,
    TrafficShiftPlan,
    RollbackPlan,
    RegionalPartition,
    RateLimit,
    AuthenticationDenied,
    UnknownOperation,
}

impl FixtureClass {
    pub const ALL: [Self; 10] = [
        Self::DiscoverState,
        Self::ServerSideDryRun,
        Self::ApplyPlan,
        Self::ObserveHealth,
        Self::TrafficShiftPlan,
        Self::RollbackPlan,
        Self::RegionalPartition,
        Self::RateLimit,
        Self::AuthenticationDenied,
        Self::UnknownOperation,
    ];

    const fn index(self) -> u64 {
        match self {
            Self::DiscoverState => 0,
            Self::ServerSideDryRun => 1,
            Self::ApplyPlan => 2,
            Self::ObserveHealth => 3,
            Self::TrafficShiftPlan => 4,
            Self::RollbackPlan => 5,
            Self::RegionalPartition => 6,
            Self::RateLimit => 7,
            Self::AuthenticationDenied => 8,
            Self::UnknownOperation => 9,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureDisposition {
    ReadOnlyObserved,
    ManualExecutionRequired,
    ReconcileRequired,
    ManualBackoff,
    Denied,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedAdapterFixture {
    pub fixture_id: [u8; 32],
    pub campaign_id: [u8; 32],
    pub contract_digest: [u8; 32],
    pub region: String,
    pub sequence: u64,
    pub class: FixtureClass,
    pub disposition: FixtureDisposition,
    pub observed_at_ns: i64,
    pub source_digest: [u8; 32],
    pub request_digest: [u8; 32],
    pub response_digest: [u8; 32],
    pub mutation_performed: bool,
    pub credential_loaded: bool,
    pub fixture_digest: [u8; 32],
}

impl RecordedAdapterFixture {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.fixture_digest = fixture_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.fixture_digest == fixture_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivilegeTestClass {
    BaselinePolicyData,
    WildcardResourceDenied,
    SecretReadDenied,
    ClusterAdminDenied,
    ArbitraryExecDenied,
    EscalationDenied,
    CrossRegionDenied,
}

impl PrivilegeTestClass {
    pub const ALL: [Self; 7] = [
        Self::BaselinePolicyData,
        Self::WildcardResourceDenied,
        Self::SecretReadDenied,
        Self::ClusterAdminDenied,
        Self::ArbitraryExecDenied,
        Self::EscalationDenied,
        Self::CrossRegionDenied,
    ];

    const fn index(self) -> u64 {
        match self {
            Self::BaselinePolicyData => 0,
            Self::WildcardResourceDenied => 1,
            Self::SecretReadDenied => 2,
            Self::ClusterAdminDenied => 3,
            Self::ArbitraryExecDenied => 4,
            Self::EscalationDenied => 5,
            Self::CrossRegionDenied => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivilegeTestResult {
    PolicyDataAccepted,
    Denied,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivilegeTestEvidence {
    pub evidence_id: [u8; 32],
    pub campaign_id: [u8; 32],
    pub policy_digest: [u8; 32],
    pub sequence: u64,
    pub class: PrivilegeTestClass,
    pub result: PrivilegeTestResult,
    pub observed_at_ns: i64,
    pub source_digest: [u8; 32],
    pub credential_loaded: bool,
    pub signature_created: bool,
    pub executable_request_created: bool,
    pub evidence_digest: [u8; 32],
}

impl PrivilegeTestEvidence {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.evidence_digest = privilege_evidence_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evidence_digest == privilege_evidence_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryScenario {
    RegionUnavailable,
    ControlPlanePartition,
    DurableStateLoss,
    ArtifactUnavailable,
}

impl RecoveryScenario {
    pub const ALL: [Self; 4] = [
        Self::RegionUnavailable,
        Self::ControlPlanePartition,
        Self::DurableStateLoss,
        Self::ArtifactUnavailable,
    ];

    const fn index(self) -> u64 {
        match self {
            Self::RegionUnavailable => 0,
            Self::ControlPlanePartition => 1,
            Self::DurableStateLoss => 2,
            Self::ArtifactUnavailable => 3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecoveryDrillEvidence {
    pub drill_id: [u8; 32],
    pub campaign_id: [u8; 32],
    pub contract_digest: [u8; 32],
    pub rollback_package_digest: [u8; 32],
    pub sequence: u64,
    pub scenario: RecoveryScenario,
    pub failed_region: String,
    pub recovery_region: String,
    pub started_at_ns: i64,
    pub recovered_at_ns: i64,
    pub journal_replayed: bool,
    pub checkpoint_verified: bool,
    pub reconciliation_restored: bool,
    pub rollback_available: bool,
    pub failover_observed: bool,
    pub manual_promotion_required: bool,
    pub traffic_shift_performed: bool,
    pub credential_loaded: bool,
    pub source_digest: [u8; 32],
    pub evidence_digest: [u8; 32],
}

impl RecoveryDrillEvidence {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.evidence_digest = recovery_evidence_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evidence_digest == recovery_evidence_digest(self)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CertificationReason {
    MissingFixture { region: String, class: FixtureClass },
    StaleFixture { region: String, class: FixtureClass },
    MissingPrivilegeTest(PrivilegeTestClass),
    StalePrivilegeTest(PrivilegeTestClass),
    MissingRecoveryScenario(RecoveryScenario),
    StaleRecoveryScenario(RecoveryScenario),
    MissingRecoveryRegion(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificationStatus {
    Certified,
    NotCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct AdapterCertificationReport {
    pub report_id: [u8; 32],
    pub campaign_id: [u8; 32],
    pub campaign_digest: [u8; 32],
    pub contract_digest: [u8; 32],
    pub completion_report_digest: [u8; 32],
    pub rollback_report_digest: [u8; 32],
    pub preflight_report_digest: [u8; 32],
    pub rollback_package_digest: [u8; 32],
    pub regions: Vec<String>,
    pub finalized_at_ns: i64,
    pub status: CertificationStatus,
    pub reasons: Vec<CertificationReason>,
    pub fixture_count: usize,
    pub privilege_test_count: usize,
    pub recovery_drill_count: usize,
    pub covered_recovery_regions: Vec<String>,
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

impl AdapterCertificationReport {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest == certification_report_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CertificationCommand {
    Register {
        command_id: CertificationCommandId,
        campaign: Box<CertificationCampaign>,
        recorded_at_ns: i64,
    },
    RecordFixture {
        command_id: CertificationCommandId,
        fixture: RecordedAdapterFixture,
        recorded_at_ns: i64,
    },
    RecordPrivilegeTest {
        command_id: CertificationCommandId,
        evidence: PrivilegeTestEvidence,
        recorded_at_ns: i64,
    },
    RecordRecoveryDrill {
        command_id: CertificationCommandId,
        evidence: RecoveryDrillEvidence,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: CertificationCommandId,
        campaign_id: [u8; 32],
        report_id: [u8; 32],
        recorded_at_ns: i64,
    },
}

impl CertificationCommand {
    #[must_use]
    pub const fn command_id(&self) -> CertificationCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::RecordFixture { command_id, .. }
            | Self::RecordPrivilegeTest { command_id, .. }
            | Self::RecordRecoveryDrill { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::RecordFixture { recorded_at_ns, .. }
            | Self::RecordPrivilegeTest { recorded_at_ns, .. }
            | Self::RecordRecoveryDrill { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CertificationDetail {
    Registered,
    FixtureRecorded { region: String, class: FixtureClass },
    PrivilegeTestRecorded(PrivilegeTestClass),
    RecoveryDrillRecorded(RecoveryScenario),
    Finalized(Box<AdapterCertificationReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationOutcome {
    pub command_id: CertificationCommandId,
    pub detail: CertificationDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CertificationSnapshot {
    pub accepted_commands: u64,
    pub campaign_id: Option<[u8; 32]>,
    pub fixture_count: usize,
    pub privilege_test_count: usize,
    pub recovery_drill_count: usize,
    pub last_report: Option<AdapterCertificationReport>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("deployment-adapter certification configuration is invalid")]
    Config,
    #[error("deployment-adapter certification timestamp is invalid or regressed")]
    Timestamp,
    #[error("deployment-adapter certification command exceeds its canonical bound")]
    CommandBound,
    #[error("deployment-adapter certification JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported deployment-adapter certification version: {0}")]
    Version(u16),
    #[error("certification command id was reused for different content")]
    IdempotencyConflict,
    #[error("certification campaign or orchestration subject is invalid")]
    Campaign,
    #[error("deployment-adapter contract or privilege policy is invalid")]
    Contract,
    #[error("recorded deployment-adapter fixture is invalid")]
    Fixture,
    #[error("deployment-adapter privilege evidence is invalid")]
    Privilege,
    #[error("deployment-adapter recovery evidence is invalid")]
    Recovery,
    #[error("deployment-adapter certification report lifecycle is invalid")]
    Report,
    #[error("deployment-adapter certification is already finalized")]
    Finalized,
    #[error("deployment-adapter certification arithmetic overflow")]
    Overflow,
    #[error("deployment-adapter certification is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct DeploymentAdapterCertification {
    policy: CertificationPolicy,
    campaign: Option<CertificationCampaign>,
    fixtures: BTreeMap<(String, FixtureClass), RecordedAdapterFixture>,
    fixture_ids: BTreeSet<[u8; 32]>,
    region_sequences: BTreeMap<String, u64>,
    privilege_tests: BTreeMap<PrivilegeTestClass, PrivilegeTestEvidence>,
    privilege_ids: BTreeSet<[u8; 32]>,
    recovery_drills: BTreeMap<RecoveryScenario, RecoveryDrillEvidence>,
    recovery_ids: BTreeSet<[u8; 32]>,
    report: Option<AdapterCertificationReport>,
    processed: BTreeMap<CertificationCommandId, ([u8; 32], CertificationOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl DeploymentAdapterCertification {
    /// Creates one empty offline certification owner.
    ///
    /// # Errors
    ///
    /// Rejects invalid region, age, campaign or recovery bounds.
    pub fn new(policy: CertificationPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            campaign: None,
            fixtures: BTreeMap::new(),
            fixture_ids: BTreeSet::new(),
            region_sequences: BTreeMap::new(),
            privilege_tests: BTreeMap::new(),
            privilege_ids: BTreeSet::new(),
            recovery_drills: BTreeMap::new(),
            recovery_ids: BTreeSet::new(),
            report: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic certification command.
    ///
    /// # Errors
    ///
    /// Integrity, chronology, evidence, lifecycle or arithmetic failures halt.
    pub fn apply(&mut self, command: &CertificationCommand) -> Result<CertificationOutcome, Error> {
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
        let mut outcome = CertificationOutcome {
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
        command: &CertificationCommand,
    ) -> Result<CertificationDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalized);
        }
        match command {
            CertificationCommand::Register {
                campaign,
                recorded_at_ns,
                ..
            } => {
                if self.campaign.is_some() {
                    return Err(Error::Campaign);
                }
                validate_campaign(campaign, &self.policy, *recorded_at_ns)?;
                self.campaign = Some((**campaign).clone());
                Ok(CertificationDetail::Registered)
            }
            CertificationCommand::RecordFixture {
                fixture,
                recorded_at_ns,
                ..
            } => self.record_fixture(fixture, *recorded_at_ns),
            CertificationCommand::RecordPrivilegeTest {
                evidence,
                recorded_at_ns,
                ..
            } => self.record_privilege(evidence, *recorded_at_ns),
            CertificationCommand::RecordRecoveryDrill {
                evidence,
                recorded_at_ns,
                ..
            } => self.record_recovery(evidence, *recorded_at_ns),
            CertificationCommand::Finalize {
                campaign_id,
                report_id,
                recorded_at_ns,
                ..
            } => self.finalize(*campaign_id, *report_id, *recorded_at_ns),
        }
    }

    fn record_fixture(
        &mut self,
        fixture: &RecordedAdapterFixture,
        at: i64,
    ) -> Result<CertificationDetail, Error> {
        let campaign = self.campaign.as_ref().ok_or(Error::Campaign)?;
        let expected = self
            .region_sequences
            .get(&fixture.region)
            .map_or(0, |value| *value);
        if fixture.fixture_id == [0; 32]
            || self.fixture_ids.contains(&fixture.fixture_id)
            || fixture.campaign_id != campaign.campaign_id
            || fixture.contract_digest != campaign.adapter_contract.contract_digest
            || !campaign.adapter_contract.regions.contains(&fixture.region)
            || fixture.sequence != expected
            || fixture.sequence != fixture.class.index()
            || fixture.observed_at_ns != at
            || at > campaign.expires_at_ns
            || at - campaign.created_at_ns > self.policy.maximum_fixture_age_ns
            || fixture.source_digest == [0; 32]
            || fixture.request_digest == [0; 32]
            || fixture.response_digest == [0; 32]
            || fixture.mutation_performed
            || fixture.credential_loaded
            || fixture.disposition != expected_disposition(fixture.class)
            || !fixture.verify_digest()
            || self
                .fixtures
                .contains_key(&(fixture.region.clone(), fixture.class))
        {
            return Err(Error::Fixture);
        }
        self.fixture_ids.insert(fixture.fixture_id);
        self.region_sequences.insert(
            fixture.region.clone(),
            expected.checked_add(1).ok_or(Error::Overflow)?,
        );
        self.fixtures
            .insert((fixture.region.clone(), fixture.class), fixture.clone());
        Ok(CertificationDetail::FixtureRecorded {
            region: fixture.region.clone(),
            class: fixture.class,
        })
    }

    fn record_privilege(
        &mut self,
        evidence: &PrivilegeTestEvidence,
        at: i64,
    ) -> Result<CertificationDetail, Error> {
        let campaign = self.campaign.as_ref().ok_or(Error::Campaign)?;
        let expected_result = if evidence.class == PrivilegeTestClass::BaselinePolicyData {
            PrivilegeTestResult::PolicyDataAccepted
        } else {
            PrivilegeTestResult::Denied
        };
        if evidence.evidence_id == [0; 32]
            || self.privilege_ids.contains(&evidence.evidence_id)
            || evidence.campaign_id != campaign.campaign_id
            || evidence.policy_digest != campaign.adapter_contract.privilege_policy.policy_digest
            || evidence.sequence != evidence.class.index()
            || evidence.sequence
                != u64::try_from(self.privilege_tests.len()).map_err(|_| Error::Overflow)?
            || evidence.result != expected_result
            || evidence.observed_at_ns != at
            || at > campaign.expires_at_ns
            || at - campaign.created_at_ns > self.policy.maximum_fixture_age_ns
            || evidence.source_digest == [0; 32]
            || evidence.credential_loaded
            || evidence.signature_created
            || evidence.executable_request_created
            || !evidence.verify_digest()
            || self.privilege_tests.contains_key(&evidence.class)
        {
            return Err(Error::Privilege);
        }
        self.privilege_ids.insert(evidence.evidence_id);
        self.privilege_tests
            .insert(evidence.class, evidence.clone());
        Ok(CertificationDetail::PrivilegeTestRecorded(evidence.class))
    }

    fn record_recovery(
        &mut self,
        evidence: &RecoveryDrillEvidence,
        at: i64,
    ) -> Result<CertificationDetail, Error> {
        let campaign = self.campaign.as_ref().ok_or(Error::Campaign)?;
        let regions = &campaign.adapter_contract.regions;
        let duration = evidence
            .recovered_at_ns
            .checked_sub(evidence.started_at_ns)
            .ok_or(Error::Overflow)?;
        if evidence.drill_id == [0; 32]
            || self.recovery_ids.contains(&evidence.drill_id)
            || evidence.campaign_id != campaign.campaign_id
            || evidence.contract_digest != campaign.adapter_contract.contract_digest
            || evidence.rollback_package_digest != campaign.rollback_report.rollback_package_digest
            || evidence.sequence != evidence.scenario.index()
            || evidence.sequence
                != u64::try_from(self.recovery_drills.len()).map_err(|_| Error::Overflow)?
            || evidence.failed_region == evidence.recovery_region
            || !regions.contains(&evidence.failed_region)
            || !regions.contains(&evidence.recovery_region)
            || evidence.recovered_at_ns != at
            || evidence.started_at_ns < campaign.created_at_ns
            || at > campaign.expires_at_ns
            || duration < 0
            || duration > self.policy.maximum_recovery_duration_ns
            || !evidence.journal_replayed
            || !evidence.checkpoint_verified
            || !evidence.reconciliation_restored
            || !evidence.rollback_available
            || !evidence.failover_observed
            || !evidence.manual_promotion_required
            || evidence.traffic_shift_performed
            || evidence.credential_loaded
            || evidence.source_digest == [0; 32]
            || !evidence.verify_digest()
            || self.recovery_drills.contains_key(&evidence.scenario)
        {
            return Err(Error::Recovery);
        }
        self.recovery_ids.insert(evidence.drill_id);
        self.recovery_drills
            .insert(evidence.scenario, evidence.clone());
        Ok(CertificationDetail::RecoveryDrillRecorded(
            evidence.scenario,
        ))
    }

    fn finalize(
        &mut self,
        campaign_id: [u8; 32],
        report_id: [u8; 32],
        at: i64,
    ) -> Result<CertificationDetail, Error> {
        let campaign = self.campaign.as_ref().ok_or(Error::Campaign)?;
        if campaign.campaign_id != campaign_id
            || report_id == [0; 32]
            || at > campaign.expires_at_ns
        {
            return Err(Error::Report);
        }
        let (reasons, covered_recovery_regions) = self.certification_reasons(campaign, at);
        let status = if reasons.is_empty() {
            CertificationStatus::Certified
        } else {
            CertificationStatus::NotCertified
        };
        let mut report = AdapterCertificationReport {
            report_id,
            campaign_id,
            campaign_digest: campaign.campaign_digest,
            contract_digest: campaign.adapter_contract.contract_digest,
            completion_report_digest: campaign.completion_report.report_digest,
            rollback_report_digest: campaign.rollback_report.report_digest,
            preflight_report_digest: campaign.completion_report.preflight_report_digest,
            rollback_package_digest: campaign.rollback_report.rollback_package_digest,
            regions: campaign.adapter_contract.regions.clone(),
            finalized_at_ns: at,
            status,
            reasons,
            fixture_count: self.fixtures.len(),
            privilege_test_count: self.privilege_tests.len(),
            recovery_drill_count: self.recovery_drills.len(),
            covered_recovery_regions,
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
        report.report_digest = certification_report_digest(&report);
        self.report = Some(report.clone());
        Ok(CertificationDetail::Finalized(Box::new(report)))
    }

    fn certification_reasons(
        &self,
        campaign: &CertificationCampaign,
        at: i64,
    ) -> (Vec<CertificationReason>, Vec<String>) {
        let mut reasons = Vec::new();
        for region in &campaign.adapter_contract.regions {
            for class in FixtureClass::ALL {
                match self.fixtures.get(&(region.clone(), class)) {
                    None => reasons.push(CertificationReason::MissingFixture {
                        region: region.clone(),
                        class,
                    }),
                    Some(fixture)
                        if !evidence_current(
                            fixture.observed_at_ns,
                            at,
                            self.policy.maximum_fixture_age_ns,
                        ) =>
                    {
                        reasons.push(CertificationReason::StaleFixture {
                            region: region.clone(),
                            class,
                        });
                    }
                    Some(_) => {}
                }
            }
        }
        for class in PrivilegeTestClass::ALL {
            match self.privilege_tests.get(&class) {
                None => reasons.push(CertificationReason::MissingPrivilegeTest(class)),
                Some(evidence)
                    if !evidence_current(
                        evidence.observed_at_ns,
                        at,
                        self.policy.maximum_fixture_age_ns,
                    ) =>
                {
                    reasons.push(CertificationReason::StalePrivilegeTest(class));
                }
                Some(_) => {}
            }
        }
        for scenario in RecoveryScenario::ALL {
            match self.recovery_drills.get(&scenario) {
                None => reasons.push(CertificationReason::MissingRecoveryScenario(scenario)),
                Some(evidence)
                    if !evidence_current(
                        evidence.recovered_at_ns,
                        at,
                        self.policy.maximum_fixture_age_ns,
                    ) =>
                {
                    reasons.push(CertificationReason::StaleRecoveryScenario(scenario));
                }
                Some(_) => {}
            }
        }
        let covered_recovery_regions = self
            .recovery_drills
            .values()
            .filter(|evidence| {
                evidence_current(
                    evidence.recovered_at_ns,
                    at,
                    self.policy.maximum_fixture_age_ns,
                )
            })
            .map(|evidence| evidence.recovery_region.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        for region in &campaign.adapter_contract.regions {
            if !covered_recovery_regions.contains(region) {
                reasons.push(CertificationReason::MissingRecoveryRegion(region.clone()));
            }
        }
        reasons.sort();
        (reasons, covered_recovery_regions)
    }

    #[must_use]
    pub fn snapshot(&self) -> CertificationSnapshot {
        CertificationSnapshot {
            accepted_commands: self.accepted_commands,
            campaign_id: self.campaign.as_ref().map(|item| item.campaign_id),
            fixture_count: self.fixtures.len(),
            privilege_test_count: self.privilege_tests.len(),
            recovery_drill_count: self.recovery_drills.len(),
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
        hasher.update(b"deployment-adapter-cert-state-v1");
        hash_json(&mut hasher, &self.policy);
        hash_json(&mut hasher, &self.campaign);
        for ((region, class), fixture) in &self.fixtures {
            hash_json(&mut hasher, region);
            hash_json(&mut hasher, class);
            hash_json(&mut hasher, fixture);
        }
        hash_json(&mut hasher, &self.fixture_ids);
        hash_json(&mut hasher, &self.region_sequences);
        hash_json(&mut hasher, &self.privilege_tests);
        hash_json(&mut hasher, &self.privilege_ids);
        hash_json(&mut hasher, &self.recovery_drills);
        hash_json(&mut hasher, &self.recovery_ids);
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

fn evidence_current(observed_at_ns: i64, at: i64, maximum_age_ns: i64) -> bool {
    at.checked_sub(observed_at_ns)
        .is_some_and(|age| age >= 0 && age <= maximum_age_ns)
}

fn validate_policy(policy: &CertificationPolicy) -> Result<(), Error> {
    if policy.maximum_regions < 2
        || policy.maximum_regions > MAX_REGIONS_HARD
        || policy.maximum_report_age_ns <= 0
        || policy.maximum_campaign_age_ns <= 0
        || policy.maximum_fixture_age_ns <= 0
        || policy.maximum_recovery_duration_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn validate_campaign(
    campaign: &CertificationCampaign,
    policy: &CertificationPolicy,
    at: i64,
) -> Result<(), Error> {
    let completion = &campaign.completion_report;
    let rollback = &campaign.rollback_report;
    let contract = &campaign.adapter_contract;
    if campaign.campaign_id == [0; 32]
        || campaign.created_at_ns != at
        || campaign.expires_at_ns <= at
        || campaign.expires_at_ns - at > policy.maximum_campaign_age_ns
        || !campaign.verify_digest(policy)
        || !validate_contract(contract, policy)
        || !valid_orchestration_report(completion, at, policy.maximum_report_age_ns)
        || !valid_orchestration_report(rollback, at, policy.maximum_report_age_ns)
        || completion.status != OrchestrationReportStatus::SimulatedCompleted
        || rollback.status != OrchestrationReportStatus::SimulatedRolledBack
        || completion.preflight_report_digest != rollback.preflight_report_digest
        || completion.rollback_package_digest != rollback.rollback_package_digest
        || !regions_exact(&completion.activated_regions, &contract.regions)
        || !completion.rolled_back_regions.is_empty()
        || completion.completed_wave_count == 0
        || !regions_exact(&rollback.activated_regions, &contract.regions)
        || rollback.rolled_back_regions
            != rollback
                .activated_regions
                .iter()
                .rev()
                .cloned()
                .collect::<Vec<_>>()
        || rollback.rollback_trigger.is_none()
    {
        Err(Error::Campaign)
    } else {
        Ok(())
    }
}

fn regions_exact(observed: &[String], canonical: &[String]) -> bool {
    let mut normalized = observed.to_vec();
    normalized.sort();
    normalized.windows(2).all(|pair| pair[0] < pair[1]) && normalized == canonical
}

fn validate_contract(contract: &DeploymentAdapterContract, policy: &CertificationPolicy) -> bool {
    let privileges = &contract.privilege_policy;
    contract.contract_id != [0; 32]
        && contract.interface_schema_digest != [0; 32]
        && contract.deployment_manifest_digest != [0; 32]
        && contract.rollback_manifest_digest != [0; 32]
        && contract.recovery_runbook_digest != [0; 32]
        && contract.regions.len() >= 2
        && contract.regions.len() <= policy.maximum_regions
        && contract.regions.windows(2).all(|pair| pair[0] < pair[1])
        && contract.verify_digest()
        && privileges.verify_digest()
        && privileges.allowed_resource_digest != [0; 32]
        && privileges.allowed_operations
            == [
                AdapterOperation::ReadState,
                AdapterOperation::ServerSideDryRun,
                AdapterOperation::PlanApply,
                AdapterOperation::PlanTrafficShift,
                AdapterOperation::PlanRollback,
            ]
        && !privileges.credential_material_allowed
        && !privileges.wildcard_resources_allowed
        && !privileges.secret_read_allowed
        && !privileges.cluster_admin_allowed
        && !privileges.arbitrary_exec_allowed
        && !privileges.privilege_escalation_allowed
        && !privileges.cross_region_mutation_allowed
}

fn valid_orchestration_report(report: &OrchestrationReport, at: i64, max_age: i64) -> bool {
    report.verify_digest()
        && report.finalized_at_ns <= at
        && at
            .checked_sub(report.finalized_at_ns)
            .is_some_and(|age| age <= max_age)
        && report.manual_operator_execution_required
        && !report.credential_material_created
        && !report.deployment_authority_granted
        && !report.rollback_execution_authority_granted
        && !report.cloud_control_authority_granted
        && !report.live_trading_authority_granted
}

const fn expected_disposition(class: FixtureClass) -> FixtureDisposition {
    match class {
        FixtureClass::DiscoverState | FixtureClass::ObserveHealth => {
            FixtureDisposition::ReadOnlyObserved
        }
        FixtureClass::ServerSideDryRun
        | FixtureClass::ApplyPlan
        | FixtureClass::TrafficShiftPlan
        | FixtureClass::RollbackPlan => FixtureDisposition::ManualExecutionRequired,
        FixtureClass::RegionalPartition | FixtureClass::UnknownOperation => {
            FixtureDisposition::ReconcileRequired
        }
        FixtureClass::RateLimit => FixtureDisposition::ManualBackoff,
        FixtureClass::AuthenticationDenied => FixtureDisposition::Denied,
    }
}

fn privilege_policy_digest(value: &AdapterPrivilegePolicy) -> [u8; 32] {
    let mut clone = value.clone();
    clone.policy_digest = [0; 32];
    digest_json(b"deployment-adapter-privilege-v1", &clone)
}

fn adapter_contract_digest(value: &DeploymentAdapterContract) -> [u8; 32] {
    let mut clone = value.clone();
    clone.contract_digest = [0; 32];
    digest_json(b"deployment-adapter-contract-v1", &clone)
}

fn campaign_digest(value: &CertificationCampaign) -> [u8; 32] {
    let mut clone = value.clone();
    clone.campaign_digest = [0; 32];
    digest_json(b"deployment-adapter-campaign-v1", &clone)
}

fn fixture_digest(value: &RecordedAdapterFixture) -> [u8; 32] {
    let mut clone = value.clone();
    clone.fixture_digest = [0; 32];
    digest_json(b"deployment-adapter-fixture-v1", &clone)
}

fn privilege_evidence_digest(value: &PrivilegeTestEvidence) -> [u8; 32] {
    let mut clone = value.clone();
    clone.evidence_digest = [0; 32];
    digest_json(b"deployment-adapter-privilege-evidence-v1", &clone)
}

fn recovery_evidence_digest(value: &RecoveryDrillEvidence) -> [u8; 32] {
    let mut clone = value.clone();
    clone.evidence_digest = [0; 32];
    digest_json(b"deployment-adapter-recovery-evidence-v1", &clone)
}

fn certification_report_digest(value: &AdapterCertificationReport) -> [u8; 32] {
    let mut clone = value.clone();
    clone.report_digest = [0; 32];
    digest_json(b"deployment-adapter-cert-report-v2", &clone)
}

fn outcome_digest(value: &CertificationOutcome) -> [u8; 32] {
    let mut clone = value.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"deployment-adapter-cert-outcome-v1", &clone)
}

fn digest_json<T: Serialize + ?Sized>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize + ?Sized>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable certification state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: CertificationCommand,
}

/// Encodes one bounded, versioned certification command.
///
/// # Errors
///
/// Rejects serialization failure or a command exceeding the canonical bound.
pub fn encode_command(command: &CertificationCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one complete, bounded, versioned certification command.
///
/// # Errors
///
/// Rejects oversized, malformed, trailing, or unsupported-version input.
pub fn decode_command(bytes: &[u8]) -> Result<CertificationCommand, Error> {
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
