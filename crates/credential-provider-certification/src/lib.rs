#![forbid(unsafe_code)]

//! Deterministic offline credential-provider protocol certification.
//!
//! This crate contains no credential value, key material, provider client,
//! cryptographic signing, network transport, wallet, or order submission.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new,
    DurableCredentialProviderCertification, ProviderCheckpoint, ProviderRecovery,
    ProviderStorageError,
};
pub use report::{read_report, write_report_create_new, ProviderReportFileError};

use serde::{Deserialize, Serialize};
use shadow_auth_session::{SessionReportStatus, SessionScenario, ShadowSessionReport};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ProviderCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPolicy {
    pub maximum_session_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_fixture_age_ns: i64,
    pub maximum_handle_epochs: u64,
    pub maximum_quota_units: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAlgorithm {
    Ed25519,
    Secp256k1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ProviderContract {
    pub provider_id_digest: [u8; 32],
    pub tenant_digest: [u8; 32],
    pub primary_region_digest: [u8; 32],
    pub recovery_region_digest: [u8; 32],
    pub key_purpose_digest: [u8; 32],
    pub algorithm: ProviderAlgorithm,
    pub maximum_quota_units: u64,
    pub valid_from_ns: i64,
    pub valid_until_ns: i64,
    pub key_material_embedded: bool,
    pub provider_credential_embedded: bool,
    pub export_allowed: bool,
    pub signing_allowed: bool,
    pub external_mutation_allowed: bool,
    pub contract_digest: [u8; 32],
}

impl ProviderContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.contract_digest = digest_without(b"provider-contract-v1", &self, |v| {
            v.contract_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest
            == digest_without(b"provider-contract-v1", self, |v| {
                v.contract_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct OpaqueProviderHandle {
    pub handle_id_digest: [u8; 32],
    pub contract_digest: [u8; 32],
    pub predecessor_handle_digest: [u8; 32],
    pub epoch: u64,
    pub attestation_digest: [u8; 32],
    pub region_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub key_material_present: bool,
    pub credential_material_present: bool,
    pub signature_bytes_present: bool,
    pub provider_contacted: bool,
    pub handle_digest: [u8; 32],
}

impl OpaqueProviderHandle {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.handle_digest = digest_without(b"opaque-provider-handle-v1", &self, |v| {
            v.handle_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.handle_digest
            == digest_without(b"opaque-provider-handle-v1", self, |v| {
                v.handle_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderScenario {
    AcquisitionSuccess,
    RotationSuccess,
    RevocationSuccess,
    QuotaExceeded,
    ProviderOutage,
    AttestationMismatch,
    SplitBrainAttempt,
    DisasterRecovery,
    StaleEpoch,
}

impl ProviderScenario {
    #[must_use]
    pub fn required() -> Vec<Self> {
        vec![
            Self::AcquisitionSuccess,
            Self::RotationSuccess,
            Self::RevocationSuccess,
            Self::QuotaExceeded,
            Self::ProviderOutage,
            Self::AttestationMismatch,
            Self::SplitBrainAttempt,
            Self::DisasterRecovery,
            Self::StaleEpoch,
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderDisposition {
    Accept,
    Deny,
    Revoke,
    Backoff,
    Reconcile,
    ManualRecovery,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecordedProviderFixture {
    pub fixture_id: [u8; 32],
    pub scenario: ProviderScenario,
    pub disposition: ProviderDisposition,
    pub handle: Option<OpaqueProviderHandle>,
    pub subject_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub quota_units: u64,
    pub retry_attempted: bool,
    pub key_material_observed: bool,
    pub credential_material_observed: bool,
    pub signature_produced: bool,
    pub provider_contacted: bool,
    pub socket_opened: bool,
    pub external_mutation_observed: bool,
    pub fixture_digest: [u8; 32],
}

impl RecordedProviderFixture {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.fixture_digest = digest_without(b"recorded-provider-fixture-v1", &self, |v| {
            v.fixture_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.fixture_digest
            == digest_without(b"recorded-provider-fixture-v1", self, |v| {
                v.fixture_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCertificationPlan {
    pub plan_id: [u8; 32],
    pub session_report: ShadowSessionReport,
    pub contract: ProviderContract,
    pub required_scenarios: Vec<ProviderScenario>,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl ProviderCertificationPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &ProviderPolicy) -> Self {
        self.required_scenarios.sort();
        self.policy_digest = digest_json(b"provider-policy-v1", policy);
        self.plan_digest = digest_without(b"provider-certification-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &ProviderPolicy) -> bool {
        self.policy_digest == digest_json(b"provider-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"provider-certification-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRecoveryRequirement {
    pub split_brain_fixture_digest: [u8; 32],
    pub revoked_handle_digest: [u8; 32],
    pub required_epoch: u64,
    pub requirement_digest: [u8; 32],
}

impl ProviderRecoveryRequirement {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.requirement_digest
            == digest_without(b"provider-recovery-requirement-v1", self, |v| {
                v.requirement_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecordedProviderRecovery {
    pub recovery_id: [u8; 32],
    pub requirement_digest: [u8; 32],
    pub recovered_epoch: u64,
    pub state_digest: [u8; 32],
    pub destination_region_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub no_mutation_observed: bool,
    pub key_material_observed: bool,
    pub credential_material_observed: bool,
    pub signature_produced: bool,
    pub provider_contacted: bool,
    pub socket_opened: bool,
    pub handle_activated: bool,
    pub recovery_digest: [u8; 32],
}

impl RecordedProviderRecovery {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.recovery_digest = digest_without(b"recorded-provider-recovery-v1", &self, |v| {
            v.recovery_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.recovery_digest
            == digest_without(b"recorded-provider-recovery-v1", self, |v| {
                v.recovery_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReportStatus {
    OfflineCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ProviderCertificationReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub session_report_digest: [u8; 32],
    pub contract_digest: [u8; 32],
    pub covered_scenarios: Vec<ProviderScenario>,
    pub final_epoch: u64,
    pub finalized_at_ns: i64,
    pub status: ProviderReportStatus,
    pub key_material_created: bool,
    pub provider_credential_created: bool,
    pub signature_produced: bool,
    pub provider_contacted: bool,
    pub socket_opened: bool,
    pub external_mutation_observed: bool,
    pub signing_authority_granted: bool,
    pub submission_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl ProviderCertificationReport {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"provider-certification-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"provider-certification-report-v1", self, |v| {
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
pub enum ProviderCommand {
    Register {
        command_id: ProviderCommandId,
        plan: Box<ProviderCertificationPlan>,
        recorded_at_ns: i64,
    },
    RecordFixture {
        command_id: ProviderCommandId,
        fixture: Box<RecordedProviderFixture>,
        recorded_at_ns: i64,
    },
    Recover {
        command_id: ProviderCommandId,
        requirement: Box<ProviderRecoveryRequirement>,
        evidence: RecordedProviderRecovery,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: ProviderCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl ProviderCommand {
    #[must_use]
    pub const fn command_id(&self) -> ProviderCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::RecordFixture { command_id, .. }
            | Self::Recover { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::RecordFixture { recorded_at_ns, .. }
            | Self::Recover { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ProviderDetail {
    Registered,
    FixtureRecorded,
    RecoveryRequired(Box<ProviderRecoveryRequirement>),
    Recovered,
    Finalized(Box<ProviderCertificationReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderOutcome {
    pub command_id: ProviderCommandId,
    pub detail: ProviderDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSnapshot {
    pub current_handle: Option<OpaqueProviderHandle>,
    pub recovery_requirement: Option<ProviderRecoveryRequirement>,
    pub covered_scenarios: BTreeSet<ProviderScenario>,
    pub report: Option<ProviderCertificationReport>,
    pub accepted_commands: u64,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("credential-provider policy invalid")]
    Config,
    #[error("credential-provider timestamp invalid or regressed")]
    Timestamp,
    #[error("credential-provider command exceeds bound")]
    CommandBound,
    #[error("credential-provider JSON invalid: {0}")]
    Json(String),
    #[error("unsupported credential-provider command version: {0}")]
    Version(u16),
    #[error("credential-provider idempotency conflict")]
    IdempotencyConflict,
    #[error("Phase 2.32 report invalid, stale, incomplete, or authority-bearing")]
    Upstream,
    #[error("credential-provider plan or contract invalid")]
    Plan,
    #[error("credential-provider fixture invalid or unsafe")]
    Fixture,
    #[error("credential-provider lifecycle transition invalid")]
    Lifecycle,
    #[error("credential-provider recovery invalid or substituted")]
    Recovery,
    #[error("credential-provider finalization invalid")]
    Finalize,
    #[error("credential-provider arithmetic overflow")]
    Overflow,
    #[error("credential-provider coordinator halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct CredentialProviderCertification {
    policy: ProviderPolicy,
    plan: Option<ProviderCertificationPlan>,
    current_handle: Option<OpaqueProviderHandle>,
    recovery: Option<ProviderRecoveryRequirement>,
    revoked_handles: BTreeSet<[u8; 32]>,
    used_fixture_ids: BTreeSet<[u8; 32]>,
    used_recovery_ids: BTreeSet<[u8; 32]>,
    covered: BTreeSet<ProviderScenario>,
    highest_epoch: u64,
    report: Option<ProviderCertificationReport>,
    processed: BTreeMap<ProviderCommandId, ([u8; 32], ProviderOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl CredentialProviderCertification {
    /// Creates an empty offline provider-certification owner.
    ///
    /// # Errors
    ///
    /// Rejects zero or excessive policy bounds.
    pub fn new(policy: ProviderPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            current_handle: None,
            recovery: None,
            revoked_handles: BTreeSet::new(),
            used_fixture_ids: BTreeSet::new(),
            used_recovery_ids: BTreeSet::new(),
            covered: BTreeSet::new(),
            highest_epoch: 0,
            report: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic recorded command.
    ///
    /// # Errors
    ///
    /// Invalid chronology, evidence, lifecycle, recovery, or identity halts.
    pub fn apply(&mut self, command: &ProviderCommand) -> Result<ProviderOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        if command.recorded_at_ns() < 0
            || self
                .last_recorded_at_ns
                .is_some_and(|prior| command.recorded_at_ns() < prior)
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
            Ok(value) => value,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = next
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        let mut outcome = ProviderOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"provider-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    fn transition(&mut self, command: &ProviderCommand) -> Result<ProviderDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            ProviderCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some()
                    || !valid_upstream(&plan.session_report, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(ProviderDetail::Registered)
            }
            ProviderCommand::RecordFixture {
                fixture,
                recorded_at_ns,
                ..
            } => self.record_fixture(fixture, *recorded_at_ns),
            ProviderCommand::Recover {
                requirement,
                evidence,
                recorded_at_ns,
                ..
            } => {
                let current = self.recovery.as_ref().ok_or(Error::Recovery)?;
                let plan = self.plan.as_ref().ok_or(Error::Recovery)?;
                if **requirement != *current
                    || !requirement.verify_digest()
                    || !valid_recovery(evidence, requirement, &plan.contract, *recorded_at_ns)
                    || self.used_recovery_ids.contains(&evidence.recovery_id)
                {
                    return Err(Error::Recovery);
                }
                self.used_recovery_ids.insert(evidence.recovery_id);
                self.recovery = None;
                Ok(ProviderDetail::Recovered)
            }
            ProviderCommand::Finalize {
                report_id,
                finalized_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                if *report_id == [0; 32]
                    || self.current_handle.is_some()
                    || self.recovery.is_some()
                    || !plan
                        .required_scenarios
                        .iter()
                        .all(|scenario| self.covered.contains(scenario))
                    || *finalized_at_ns < plan.created_at_ns
                    || *finalized_at_ns > *recorded_at_ns
                {
                    return Err(Error::Finalize);
                }
                let mut report = ProviderCertificationReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    session_report_digest: plan.session_report.report_digest,
                    contract_digest: plan.contract.contract_digest,
                    covered_scenarios: self.covered.iter().copied().collect(),
                    final_epoch: self.highest_epoch,
                    finalized_at_ns: *finalized_at_ns,
                    status: ProviderReportStatus::OfflineCertified,
                    key_material_created: false,
                    provider_credential_created: false,
                    signature_produced: false,
                    provider_contacted: false,
                    socket_opened: false,
                    external_mutation_observed: false,
                    signing_authority_granted: false,
                    submission_authority_granted: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    report_digest: [0; 32],
                };
                report.report_digest =
                    digest_without(b"provider-certification-report-v1", &report, |v| {
                        v.report_digest = [0; 32];
                    });
                self.report = Some(report.clone());
                Ok(ProviderDetail::Finalized(Box::new(report)))
            }
        }
    }

    fn record_fixture(
        &mut self,
        fixture: &RecordedProviderFixture,
        at: i64,
    ) -> Result<ProviderDetail, Error> {
        let plan = self.plan.as_ref().ok_or(Error::Fixture)?;
        if self.recovery.is_some()
            || self.used_fixture_ids.contains(&fixture.fixture_id)
            || !valid_fixture(fixture, plan, &self.policy, at)
        {
            return Err(Error::Fixture);
        }
        match fixture.scenario {
            ProviderScenario::AcquisitionSuccess => {
                let handle = fixture.handle.as_ref().ok_or(Error::Lifecycle)?;
                if self.current_handle.is_some()
                    || fixture.disposition != ProviderDisposition::Accept
                    || handle.epoch != self.highest_epoch.checked_add(1).ok_or(Error::Overflow)?
                    || handle.predecessor_handle_digest != [0; 32]
                {
                    return Err(Error::Lifecycle);
                }
                self.highest_epoch = handle.epoch;
                self.current_handle = Some(handle.clone());
            }
            ProviderScenario::RotationSuccess => {
                let prior = self.current_handle.as_ref().ok_or(Error::Lifecycle)?;
                let handle = fixture.handle.as_ref().ok_or(Error::Lifecycle)?;
                if fixture.disposition != ProviderDisposition::Accept
                    || handle.epoch != self.highest_epoch.checked_add(1).ok_or(Error::Overflow)?
                    || handle.predecessor_handle_digest != prior.handle_digest
                    || self.revoked_handles.contains(&handle.handle_digest)
                {
                    return Err(Error::Lifecycle);
                }
                self.highest_epoch = handle.epoch;
                self.current_handle = Some(handle.clone());
            }
            ProviderScenario::RevocationSuccess => {
                let current = self.current_handle.take().ok_or(Error::Lifecycle)?;
                if fixture.disposition != ProviderDisposition::Revoke
                    || fixture
                        .handle
                        .as_ref()
                        .is_some_and(|value| value.handle_digest != current.handle_digest)
                {
                    return Err(Error::Lifecycle);
                }
                self.revoked_handles.insert(current.handle_digest);
            }
            ProviderScenario::QuotaExceeded => {
                if fixture.disposition != ProviderDisposition::Backoff
                    || fixture.quota_units <= plan.contract.maximum_quota_units
                    || fixture.retry_attempted
                {
                    return Err(Error::Lifecycle);
                }
            }
            ProviderScenario::ProviderOutage => {
                if fixture.disposition != ProviderDisposition::Backoff || fixture.retry_attempted {
                    return Err(Error::Lifecycle);
                }
            }
            ProviderScenario::AttestationMismatch | ProviderScenario::StaleEpoch => {
                if fixture.disposition != ProviderDisposition::Deny {
                    return Err(Error::Lifecycle);
                }
            }
            ProviderScenario::SplitBrainAttempt => {
                let current = self.current_handle.take().ok_or(Error::Lifecycle)?;
                if fixture.disposition != ProviderDisposition::Reconcile {
                    return Err(Error::Lifecycle);
                }
                self.revoked_handles.insert(current.handle_digest);
                let mut requirement = ProviderRecoveryRequirement {
                    split_brain_fixture_digest: fixture.fixture_digest,
                    revoked_handle_digest: current.handle_digest,
                    required_epoch: self.highest_epoch,
                    requirement_digest: [0; 32],
                };
                requirement.requirement_digest =
                    digest_without(b"provider-recovery-requirement-v1", &requirement, |v| {
                        v.requirement_digest = [0; 32];
                    });
                self.recovery = Some(requirement.clone());
                self.used_fixture_ids.insert(fixture.fixture_id);
                self.covered.insert(fixture.scenario);
                return Ok(ProviderDetail::RecoveryRequired(Box::new(requirement)));
            }
            ProviderScenario::DisasterRecovery => {
                if fixture.disposition != ProviderDisposition::ManualRecovery
                    || self.current_handle.is_some()
                    || fixture.handle.is_some()
                {
                    return Err(Error::Lifecycle);
                }
            }
        }
        self.used_fixture_ids.insert(fixture.fixture_id);
        self.covered.insert(fixture.scenario);
        Ok(ProviderDetail::FixtureRecorded)
    }

    #[must_use]
    pub fn snapshot(&self) -> ProviderSnapshot {
        ProviderSnapshot {
            current_handle: self.current_handle.clone(),
            recovery_requirement: self.recovery.clone(),
            covered_scenarios: self.covered.clone(),
            report: self.report.clone(),
            accepted_commands: self.accepted_commands,
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
        hasher.update(b"credential-provider-certification-state-v1");
        hash_value(
            &mut hasher,
            &(
                &self.policy,
                &self.plan,
                &self.current_handle,
                &self.recovery,
                &self.revoked_handles,
                &self.used_fixture_ids,
                &self.used_recovery_ids,
                &self.covered,
                self.highest_epoch,
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

fn validate_policy(policy: &ProviderPolicy) -> Result<(), Error> {
    if policy.maximum_session_report_age_ns <= 0
        || policy.maximum_plan_lifetime_ns <= 0
        || policy.maximum_fixture_age_ns <= 0
        || policy.maximum_handle_epochs == 0
        || policy.maximum_handle_epochs > u64::try_from(MAX_ITEMS).unwrap_or(u64::MAX)
        || policy.maximum_quota_units == 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn valid_upstream(report: &ShadowSessionReport, policy: &ProviderPolicy, at: i64) -> bool {
    report.verify_digest()
        && report.status == SessionReportStatus::SimulationCompleted
        && SessionScenario::ALL
            .iter()
            .all(|scenario| report.covered_scenarios.contains(scenario))
        && !report.credential_material_created
        && !report.signature_produced
        && !report.provider_contacted
        && !report.socket_opened
        && !report.authentication_authority_granted
        && !report.external_submission_authority_granted
        && !report.deployment_authority_granted
        && !report.trading_authority_granted
        && at
            .checked_sub(report.finalized_at_ns)
            .is_some_and(|age| age <= policy.maximum_session_report_age_ns)
}

fn valid_plan(plan: &ProviderCertificationPlan, policy: &ProviderPolicy, at: i64) -> bool {
    let contract = &plan.contract;
    plan.verify_digest(policy)
        && plan.plan_id != [0; 32]
        && plan.required_scenarios == ProviderScenario::required()
        && plan.created_at_ns <= at
        && plan.expires_at_ns > at
        && plan.expires_at_ns
            <= plan
                .created_at_ns
                .checked_add(policy.maximum_plan_lifetime_ns)
                .unwrap_or(i64::MIN)
        && contract.verify_digest()
        && contract.provider_id_digest != [0; 32]
        && contract.tenant_digest != [0; 32]
        && contract.primary_region_digest != [0; 32]
        && contract.recovery_region_digest != [0; 32]
        && contract.primary_region_digest != contract.recovery_region_digest
        && contract.key_purpose_digest != [0; 32]
        && contract.maximum_quota_units > 0
        && contract.maximum_quota_units <= policy.maximum_quota_units
        && contract.valid_from_ns <= at
        && contract.valid_until_ns >= plan.expires_at_ns
        && !contract.key_material_embedded
        && !contract.provider_credential_embedded
        && !contract.export_allowed
        && !contract.signing_allowed
        && !contract.external_mutation_allowed
}

fn valid_fixture(
    value: &RecordedProviderFixture,
    plan: &ProviderCertificationPlan,
    policy: &ProviderPolicy,
    at: i64,
) -> bool {
    value.verify_digest()
        && value.fixture_id != [0; 32]
        && value.subject_digest == plan.contract.contract_digest
        && value.observed_at_ns >= plan.created_at_ns
        && value.observed_at_ns <= at
        && at
            .checked_sub(value.observed_at_ns)
            .is_some_and(|age| age <= policy.maximum_fixture_age_ns)
        && value.quota_units
            <= policy
                .maximum_quota_units
                .checked_add(1)
                .unwrap_or(u64::MAX)
        && !value.key_material_observed
        && !value.credential_material_observed
        && !value.signature_produced
        && !value.provider_contacted
        && !value.socket_opened
        && !value.external_mutation_observed
        && value
            .handle
            .as_ref()
            .is_none_or(|handle| valid_handle(handle, &plan.contract, policy, value.observed_at_ns))
}

fn valid_handle(
    handle: &OpaqueProviderHandle,
    contract: &ProviderContract,
    policy: &ProviderPolicy,
    at: i64,
) -> bool {
    handle.verify_digest()
        && handle.handle_id_digest != [0; 32]
        && handle.contract_digest == contract.contract_digest
        && handle.epoch > 0
        && handle.epoch <= policy.maximum_handle_epochs
        && handle.attestation_digest != [0; 32]
        && handle.region_digest == contract.primary_region_digest
        && handle.observed_at_ns <= at
        && !handle.key_material_present
        && !handle.credential_material_present
        && !handle.signature_bytes_present
        && !handle.provider_contacted
}

fn valid_recovery(
    value: &RecordedProviderRecovery,
    requirement: &ProviderRecoveryRequirement,
    contract: &ProviderContract,
    at: i64,
) -> bool {
    value.verify_digest()
        && value.recovery_id != [0; 32]
        && value.requirement_digest == requirement.requirement_digest
        && value.recovered_epoch == requirement.required_epoch
        && value.state_digest != [0; 32]
        && value.destination_region_digest == contract.recovery_region_digest
        && value.observed_at_ns <= at
        && value.no_mutation_observed
        && !value.key_material_observed
        && !value.credential_material_observed
        && !value.signature_produced
        && !value.provider_contacted
        && !value.socket_opened
        && !value.handle_activated
}

/// Encodes one bounded, versioned command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &ProviderCommand) -> Result<Vec<u8>, Error> {
    let body = serde_json::to_vec(command).map_err(|error| Error::Json(error.to_string()))?;
    if body.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut bytes = Vec::with_capacity(body.len() + 2);
    bytes.extend_from_slice(&WIRE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

/// Decodes one bounded, versioned command.
///
/// # Errors
///
/// Rejects invalid size, version, JSON, trailing bytes, or unknown fields.
pub fn decode_command(bytes: &[u8]) -> Result<ProviderCommand, Error> {
    if bytes.len() < 2 || bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let version = u16::from_le_bytes(bytes[..2].try_into().map_err(|_| Error::CommandBound)?);
    if version != WIRE_VERSION {
        return Err(Error::Version(version));
    }
    let mut decoder = serde_json::Deserializer::from_slice(&bytes[2..]);
    let command = ProviderCommand::deserialize(&mut decoder)
        .map_err(|error| Error::Json(error.to_string()))?;
    decoder
        .end()
        .map_err(|error| Error::Json(error.to_string()))?;
    Ok(command)
}

fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(&serde_json::to_vec(value).expect("serializable certification state"));
    *hasher.finalize().as_bytes()
}

fn hash_value<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable certification state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
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

#[cfg(test)]
mod tests;
