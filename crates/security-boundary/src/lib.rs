#![forbid(unsafe_code)]

//! Deterministic local certification of the production security boundary.
//!
//! Fake provider contracts carry no secret, key, signature, socket, provider
//! client, wallet, deployment, trading, or order-submission capability.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableSecurityBoundary,
    SecurityCheckpoint, SecurityRecovery, SecurityStorageError,
};
pub use report::{read_report, write_report_create_new, SecurityReportFileError};

use durable_infrastructure::{
    BackendKind, InfrastructureReport, InfrastructureReportStatus, InfrastructureScenario,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SecurityCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityPolicy {
    pub maximum_infrastructure_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_observation_age_ns: i64,
    pub maximum_identity_lifetime_ns: i64,
    pub maximum_backoff_ns: i64,
    pub maximum_request_units: u64,
    pub maximum_requests_per_window: u64,
    pub maximum_identity_epochs: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderClass {
    Vault,
    Kms,
    Hsm,
}

impl ProviderClass {
    pub const ALL: [Self; 3] = [Self::Vault, Self::Kms, Self::Hsm];
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigningPurpose {
    OrderIntent,
    CancelIntent,
    ConversionIntent,
}

impl SigningPurpose {
    pub const ALL: [Self; 3] = [
        Self::OrderIntent,
        Self::CancelIntent,
        Self::ConversionIntent,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct FakeProviderContract {
    pub provider: ProviderClass,
    pub provider_subject_digest: [u8; 32],
    pub primary_region_digest: [u8; 32],
    pub recovery_region_digest: [u8; 32],
    pub attestation_policy_digest: [u8; 32],
    pub fake_only: bool,
    pub credential_embedded: bool,
    pub key_material_embedded: bool,
    pub export_allowed: bool,
    pub network_enabled: bool,
    pub signing_enabled: bool,
    pub contract_digest: [u8; 32],
}

impl FakeProviderContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.contract_digest = digest_without(b"fake-provider-contract-v1", &self, |v| {
            v.contract_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest
            == digest_without(b"fake-provider-contract-v1", self, |v| {
                v.contract_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct WorkloadIdentityContract {
    pub cluster_digest: [u8; 32],
    pub namespace_digest: [u8; 32],
    pub service_account_digest: [u8; 32],
    pub audience_digest: [u8; 32],
    pub attestation_digest: [u8; 32],
    pub maximum_lifetime_ns: i64,
    pub secret_value_embedded: bool,
    pub bearer_token_embedded: bool,
    pub strategy_access_allowed: bool,
    pub export_allowed: bool,
    pub contract_digest: [u8; 32],
}

impl WorkloadIdentityContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.contract_digest = digest_without(b"workload-identity-contract-v1", &self, |v| {
            v.contract_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest
            == digest_without(b"workload-identity-contract-v1", self, |v| {
                v.contract_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct IsolatedSignerContract {
    pub process_identity_digest: [u8; 32],
    pub allowed_purposes: Vec<SigningPurpose>,
    pub allowed_resource_digests: Vec<[u8; 32]>,
    pub maximum_request_units: u64,
    pub maximum_requests_per_window: u64,
    pub request_lifetime_ns: i64,
    pub dual_control_required: bool,
    pub arbitrary_payload_allowed: bool,
    pub transfer_allowed: bool,
    pub withdrawal_allowed: bool,
    pub contract_upgrade_allowed: bool,
    pub strategy_direct_access_allowed: bool,
    pub contract_digest: [u8; 32],
}

impl IsolatedSignerContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.allowed_purposes.sort();
        self.allowed_resource_digests.sort_unstable();
        self.contract_digest = digest_without(b"isolated-signer-contract-v1", &self, |v| {
            v.contract_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest
            == digest_without(b"isolated-signer-contract-v1", self, |v| {
                v.contract_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct OpaqueWorkloadIdentity {
    pub identity_id_digest: [u8; 32],
    pub contract_digest: [u8; 32],
    pub predecessor_identity_digest: [u8; 32],
    pub epoch: u64,
    pub issued_at_ns: i64,
    pub expires_at_ns: i64,
    pub attestation_digest: [u8; 32],
    pub secret_material_present: bool,
    pub token_value_present: bool,
    pub provider_contacted: bool,
    pub identity_digest: [u8; 32],
}

impl OpaqueWorkloadIdentity {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.identity_digest = digest_without(b"opaque-workload-identity-v1", &self, |v| {
            v.identity_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.identity_digest
            == digest_without(b"opaque-workload-identity-v1", self, |v| {
                v.identity_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityScenario {
    IdentityIssue,
    IdentityRotation,
    IdentityRevocation,
    ProviderOutage,
    SignerDenial,
    RateLimit,
    DualControl,
    ReplayDenied,
    CompromiseContainment,
    DisasterRecovery,
}

impl SecurityScenario {
    pub const ALL: [Self; 10] = [
        Self::IdentityIssue,
        Self::IdentityRotation,
        Self::IdentityRevocation,
        Self::ProviderOutage,
        Self::SignerDenial,
        Self::RateLimit,
        Self::DualControl,
        Self::ReplayDenied,
        Self::CompromiseContainment,
        Self::DisasterRecovery,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityDisposition {
    Issue,
    Rotate,
    Revoke,
    Backoff,
    Deny,
    Record,
    Reconcile,
    ManualRecovery,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct SecurityObservation {
    pub observation_id: [u8; 32],
    pub scenario: SecurityScenario,
    pub disposition: SecurityDisposition,
    pub provider: ProviderClass,
    pub provider_contract_digest: [u8; 32],
    pub identity: Option<OpaqueWorkloadIdentity>,
    pub security_operator_digest: [u8; 32],
    pub operations_operator_digest: [u8; 32],
    pub destination_region_digest: [u8; 32],
    pub backoff_ns: i64,
    pub observed_at_ns: i64,
    pub automatic_retry_attempted: bool,
    pub secret_material_observed: bool,
    pub signature_produced: bool,
    pub provider_contacted: bool,
    pub socket_opened: bool,
    pub external_mutation_observed: bool,
    pub signer_activated: bool,
    pub observation_digest: [u8; 32],
}

impl SecurityObservation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.observation_digest = digest_without(b"security-observation-v1", &self, |v| {
            v.observation_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.observation_digest
            == digest_without(b"security-observation-v1", self, |v| {
                v.observation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityPlan {
    pub plan_id: [u8; 32],
    pub infrastructure_report: InfrastructureReport,
    pub workload_contract: WorkloadIdentityContract,
    pub signer_contract: IsolatedSignerContract,
    pub provider_contracts: Vec<FakeProviderContract>,
    pub required_scenarios: Vec<SecurityScenario>,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl SecurityPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &SecurityPolicy) -> Self {
        self.provider_contracts.sort_by_key(|v| v.provider);
        self.required_scenarios.sort();
        self.policy_digest = digest_json(b"security-policy-v1", policy);
        self.plan_digest = digest_without(b"security-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self, policy: &SecurityPolicy) -> bool {
        self.policy_digest == digest_json(b"security-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"security-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityRecoveryRequirement {
    pub compromise_observation_digest: [u8; 32],
    pub revoked_identity_digest: [u8; 32],
    pub required_epoch: u64,
    pub requirement_digest: [u8; 32],
}
impl SecurityRecoveryRequirement {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.requirement_digest
            == digest_without(b"security-recovery-requirement-v1", self, |v| {
                v.requirement_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct SecurityRecoveryEvidence {
    pub recovery_id: [u8; 32],
    pub requirement_digest: [u8; 32],
    pub recovered_epoch: u64,
    pub state_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub no_mutation_observed: bool,
    pub secret_material_observed: bool,
    pub signature_produced: bool,
    pub provider_contacted: bool,
    pub socket_opened: bool,
    pub identity_activated: bool,
    pub evidence_digest: [u8; 32],
}
impl SecurityRecoveryEvidence {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.evidence_digest = digest_without(b"security-recovery-evidence-v1", &self, |v| {
            v.evidence_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evidence_digest
            == digest_without(b"security-recovery-evidence-v1", self, |v| {
                v.evidence_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityReportStatus {
    LocallyCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct SecurityReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub infrastructure_report_digest: [u8; 32],
    pub covered_scenarios: Vec<SecurityScenario>,
    pub covered_providers: Vec<ProviderClass>,
    pub final_identity_epoch: u64,
    pub finalized_at_ns: i64,
    pub status: SecurityReportStatus,
    pub real_provider_certified: bool,
    pub secret_material_created: bool,
    pub signature_produced: bool,
    pub provider_contacted: bool,
    pub socket_opened: bool,
    pub signer_activated: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub submission_authority_granted: bool,
    pub report_digest: [u8; 32],
}
impl SecurityReport {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"security-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"security-report-v1", self, |v| {
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
pub enum SecurityCommand {
    Register {
        command_id: SecurityCommandId,
        plan: Box<SecurityPlan>,
        recorded_at_ns: i64,
    },
    Observe {
        command_id: SecurityCommandId,
        observation: Box<SecurityObservation>,
        recorded_at_ns: i64,
    },
    Recover {
        command_id: SecurityCommandId,
        requirement: Box<SecurityRecoveryRequirement>,
        evidence: SecurityRecoveryEvidence,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: SecurityCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}
impl SecurityCommand {
    #[must_use]
    pub const fn command_id(&self) -> SecurityCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::Observe { command_id, .. }
            | Self::Recover { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::Observe { recorded_at_ns, .. }
            | Self::Recover { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SecurityDetail {
    Registered,
    ObservationAccepted,
    RecoveryRequired(Box<SecurityRecoveryRequirement>),
    Recovered,
    Finalized(Box<SecurityReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SecurityOutcome {
    pub command_id: SecurityCommandId,
    pub detail: SecurityDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecuritySnapshot {
    pub current_identity: Option<OpaqueWorkloadIdentity>,
    pub recovery_requirement: Option<SecurityRecoveryRequirement>,
    pub covered_scenarios: BTreeSet<SecurityScenario>,
    pub covered_providers: BTreeSet<ProviderClass>,
    pub accepted_commands: u64,
    pub report: Option<SecurityReport>,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("security policy invalid")]
    Config,
    #[error("security timestamp invalid or regressed")]
    Timestamp,
    #[error("security command exceeds bound")]
    CommandBound,
    #[error("security JSON invalid: {0}")]
    Json(String),
    #[error("unsupported security command version: {0}")]
    Version(u16),
    #[error("security command id conflict")]
    IdempotencyConflict,
    #[error("Phase 3.0 evidence invalid, stale, incomplete, or authority-bearing")]
    Upstream,
    #[error("security plan or contract invalid")]
    Plan,
    #[error("security observation invalid or side-effect-bearing")]
    Observation,
    #[error("security identity lifecycle invalid")]
    Lifecycle,
    #[error("security recovery invalid or substituted")]
    Recovery,
    #[error("security finalization invalid")]
    Finalize,
    #[error("security arithmetic overflow")]
    Overflow,
    #[error("security owner halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct SecurityBoundaryCertification {
    policy: SecurityPolicy,
    plan: Option<SecurityPlan>,
    current_identity: Option<OpaqueWorkloadIdentity>,
    recovery: Option<SecurityRecoveryRequirement>,
    revoked_identities: BTreeSet<[u8; 32]>,
    highest_epoch: u64,
    covered_scenarios: BTreeSet<SecurityScenario>,
    covered_providers: BTreeSet<ProviderClass>,
    used_observations: BTreeSet<[u8; 32]>,
    used_recoveries: BTreeSet<[u8; 32]>,
    processed: BTreeMap<SecurityCommandId, ([u8; 32], SecurityOutcome)>,
    accepted_commands: u64,
    report: Option<SecurityReport>,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl SecurityBoundaryCertification {
    /// Creates an empty local security-boundary owner.
    ///
    /// # Errors
    ///
    /// Rejects zero or excessive policy bounds.
    pub fn new(policy: SecurityPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            current_identity: None,
            recovery: None,
            revoked_identities: BTreeSet::new(),
            highest_epoch: 0,
            covered_scenarios: BTreeSet::new(),
            covered_providers: BTreeSet::new(),
            used_observations: BTreeSet::new(),
            used_recoveries: BTreeSet::new(),
            processed: BTreeMap::new(),
            accepted_commands: 0,
            report: None,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic security certification command.
    ///
    /// # Errors
    ///
    /// Invalid chronology, evidence, lifecycle, recovery, or identity halts.
    pub fn apply(&mut self, command: &SecurityCommand) -> Result<SecurityOutcome, Error> {
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
        let mut outcome = SecurityOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"security-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    fn transition(&mut self, command: &SecurityCommand) -> Result<SecurityDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            SecurityCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some()
                    || !valid_upstream(&plan.infrastructure_report, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(SecurityDetail::Registered)
            }
            SecurityCommand::Observe {
                observation,
                recorded_at_ns,
                ..
            } => self.observe(observation, *recorded_at_ns),
            SecurityCommand::Recover {
                requirement,
                evidence,
                recorded_at_ns,
                ..
            } => {
                let current = self.recovery.as_ref().ok_or(Error::Recovery)?;
                if **requirement != *current
                    || !requirement.verify_digest()
                    || !valid_recovery(evidence, requirement, *recorded_at_ns)
                    || self.used_recoveries.contains(&evidence.recovery_id)
                {
                    return Err(Error::Recovery);
                }
                self.used_recoveries.insert(evidence.recovery_id);
                self.recovery = None;
                Ok(SecurityDetail::Recovered)
            }
            SecurityCommand::Finalize {
                report_id,
                finalized_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                if *report_id == [0; 32]
                    || self.current_identity.is_some()
                    || self.recovery.is_some()
                    || !plan
                        .required_scenarios
                        .iter()
                        .all(|s| self.covered_scenarios.contains(s))
                    || !ProviderClass::ALL
                        .iter()
                        .all(|p| self.covered_providers.contains(p))
                    || *finalized_at_ns < plan.created_at_ns
                    || *finalized_at_ns > *recorded_at_ns
                {
                    return Err(Error::Finalize);
                }
                let mut report = SecurityReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    infrastructure_report_digest: plan.infrastructure_report.report_digest,
                    covered_scenarios: self.covered_scenarios.iter().copied().collect(),
                    covered_providers: self.covered_providers.iter().copied().collect(),
                    final_identity_epoch: self.highest_epoch,
                    finalized_at_ns: *finalized_at_ns,
                    status: SecurityReportStatus::LocallyCertified,
                    real_provider_certified: false,
                    secret_material_created: false,
                    signature_produced: false,
                    provider_contacted: false,
                    socket_opened: false,
                    signer_activated: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    submission_authority_granted: false,
                    report_digest: [0; 32],
                };
                report.report_digest = digest_without(b"security-report-v1", &report, |v| {
                    v.report_digest = [0; 32];
                });
                self.report = Some(report.clone());
                Ok(SecurityDetail::Finalized(Box::new(report)))
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn observe(&mut self, value: &SecurityObservation, at: i64) -> Result<SecurityDetail, Error> {
        let plan = self.plan.as_ref().ok_or(Error::Observation)?;
        let provider = plan
            .provider_contracts
            .iter()
            .find(|p| p.provider == value.provider)
            .ok_or(Error::Observation)?;
        if self.recovery.is_some()
            || self.used_observations.contains(&value.observation_id)
            || !valid_observation(
                value,
                provider,
                &plan.workload_contract,
                &self.policy,
                plan,
                at,
            )
        {
            return Err(Error::Observation);
        }
        match value.scenario {
            SecurityScenario::IdentityIssue => {
                let identity = value.identity.as_ref().ok_or(Error::Lifecycle)?;
                if self.current_identity.is_some()
                    || value.disposition != SecurityDisposition::Issue
                    || identity.epoch != self.highest_epoch.checked_add(1).ok_or(Error::Overflow)?
                    || identity.predecessor_identity_digest != [0; 32]
                {
                    return Err(Error::Lifecycle);
                }
                self.highest_epoch = identity.epoch;
                self.current_identity = Some(identity.clone());
            }
            SecurityScenario::IdentityRotation => {
                let prior = self.current_identity.as_ref().ok_or(Error::Lifecycle)?;
                let identity = value.identity.as_ref().ok_or(Error::Lifecycle)?;
                if value.disposition != SecurityDisposition::Rotate
                    || identity.epoch != self.highest_epoch.checked_add(1).ok_or(Error::Overflow)?
                    || identity.predecessor_identity_digest != prior.identity_digest
                {
                    return Err(Error::Lifecycle);
                }
                self.highest_epoch = identity.epoch;
                self.current_identity = Some(identity.clone());
            }
            SecurityScenario::IdentityRevocation => {
                let current = self.current_identity.take().ok_or(Error::Lifecycle)?;
                if value.disposition != SecurityDisposition::Revoke
                    || value
                        .identity
                        .as_ref()
                        .is_some_and(|identity| identity.identity_digest != current.identity_digest)
                {
                    return Err(Error::Lifecycle);
                }
                self.revoked_identities.insert(current.identity_digest);
            }
            SecurityScenario::ProviderOutage | SecurityScenario::RateLimit => {
                if value.disposition != SecurityDisposition::Backoff
                    || value.backoff_ns <= 0
                    || value.backoff_ns > self.policy.maximum_backoff_ns
                    || value.automatic_retry_attempted
                {
                    return Err(Error::Lifecycle);
                }
            }
            SecurityScenario::SignerDenial | SecurityScenario::ReplayDenied => {
                if value.disposition != SecurityDisposition::Deny {
                    return Err(Error::Lifecycle);
                }
            }
            SecurityScenario::DualControl => {
                if value.disposition != SecurityDisposition::Record
                    || value.security_operator_digest == [0; 32]
                    || value.operations_operator_digest == [0; 32]
                    || value.security_operator_digest == value.operations_operator_digest
                    || value.signer_activated
                {
                    return Err(Error::Lifecycle);
                }
            }
            SecurityScenario::CompromiseContainment => {
                let current = self.current_identity.take().ok_or(Error::Lifecycle)?;
                if value.disposition != SecurityDisposition::Reconcile {
                    return Err(Error::Lifecycle);
                }
                self.revoked_identities.insert(current.identity_digest);
                let mut requirement = SecurityRecoveryRequirement {
                    compromise_observation_digest: value.observation_digest,
                    revoked_identity_digest: current.identity_digest,
                    required_epoch: self.highest_epoch,
                    requirement_digest: [0; 32],
                };
                requirement.requirement_digest =
                    digest_without(b"security-recovery-requirement-v1", &requirement, |v| {
                        v.requirement_digest = [0; 32];
                    });
                self.recovery = Some(requirement.clone());
                self.used_observations.insert(value.observation_id);
                self.covered_scenarios.insert(value.scenario);
                self.covered_providers.insert(value.provider);
                return Ok(SecurityDetail::RecoveryRequired(Box::new(requirement)));
            }
            SecurityScenario::DisasterRecovery => {
                if value.disposition != SecurityDisposition::ManualRecovery
                    || self.current_identity.is_some()
                    || value.identity.is_some()
                    || value.destination_region_digest != provider.recovery_region_digest
                    || value.destination_region_digest == provider.primary_region_digest
                {
                    return Err(Error::Lifecycle);
                }
            }
        }
        self.used_observations.insert(value.observation_id);
        self.covered_scenarios.insert(value.scenario);
        self.covered_providers.insert(value.provider);
        Ok(SecurityDetail::ObservationAccepted)
    }

    #[must_use]
    pub fn snapshot(&self) -> SecuritySnapshot {
        SecuritySnapshot {
            current_identity: self.current_identity.clone(),
            recovery_requirement: self.recovery.clone(),
            covered_scenarios: self.covered_scenarios.clone(),
            covered_providers: self.covered_providers.clone(),
            accepted_commands: self.accepted_commands,
            report: self.report.clone(),
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
        hasher.update(b"security-boundary-state-v1");
        hash_value(
            &mut hasher,
            &(
                &self.policy,
                &self.plan,
                &self.current_identity,
                &self.recovery,
                &self.revoked_identities,
                self.highest_epoch,
                &self.covered_scenarios,
                &self.covered_providers,
                &self.used_observations,
                &self.used_recoveries,
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

fn validate_policy(v: &SecurityPolicy) -> Result<(), Error> {
    if v.maximum_infrastructure_report_age_ns <= 0
        || v.maximum_plan_lifetime_ns <= 0
        || v.maximum_observation_age_ns <= 0
        || v.maximum_identity_lifetime_ns <= 0
        || v.maximum_backoff_ns <= 0
        || v.maximum_request_units == 0
        || v.maximum_requests_per_window == 0
        || v.maximum_identity_epochs == 0
        || v.maximum_identity_epochs > u64::try_from(MAX_ITEMS).unwrap_or(u64::MAX)
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn valid_upstream(v: &InfrastructureReport, policy: &SecurityPolicy, at: i64) -> bool {
    v.verify_digest()
        && v.status == InfrastructureReportStatus::LocallyCertified
        && v.covered_matrix.len() == BackendKind::ALL.len() * InfrastructureScenario::ALL.len()
        && BackendKind::ALL.iter().all(|b| {
            InfrastructureScenario::ALL
                .iter()
                .all(|s| v.covered_matrix.contains(&(*b, *s)))
        })
        && !v.external_environment_certified
        && !v.credential_material_created
        && !v.socket_opened
        && !v.external_mutation_observed
        && !v.financial_authority_granted
        && !v.deployment_authority_granted
        && !v.trading_authority_granted
        && !v.submission_authority_granted
        && at
            .checked_sub(v.finalized_at_ns)
            .is_some_and(|age| age <= policy.maximum_infrastructure_report_age_ns)
}

fn valid_plan(v: &SecurityPlan, policy: &SecurityPolicy, at: i64) -> bool {
    v.verify_digest(policy)
        && v.plan_id != [0; 32]
        && v.provider_contracts.len() == ProviderClass::ALL.len()
        && v.provider_contracts
            .iter()
            .map(|p| p.provider)
            .eq(ProviderClass::ALL)
        && v.required_scenarios == SecurityScenario::ALL
        && v.created_at_ns <= at
        && v.expires_at_ns > at
        && v.expires_at_ns
            <= v.created_at_ns
                .checked_add(policy.maximum_plan_lifetime_ns)
                .unwrap_or(i64::MIN)
        && valid_workload(&v.workload_contract, policy)
        && valid_signer(&v.signer_contract, policy)
        && v.provider_contracts.iter().all(valid_provider)
}
fn valid_provider(v: &FakeProviderContract) -> bool {
    v.verify_digest()
        && v.provider_subject_digest != [0; 32]
        && v.primary_region_digest != [0; 32]
        && v.recovery_region_digest != [0; 32]
        && v.primary_region_digest != v.recovery_region_digest
        && v.attestation_policy_digest != [0; 32]
        && v.fake_only
        && !v.credential_embedded
        && !v.key_material_embedded
        && !v.export_allowed
        && !v.network_enabled
        && !v.signing_enabled
}
fn valid_workload(v: &WorkloadIdentityContract, policy: &SecurityPolicy) -> bool {
    v.verify_digest()
        && v.cluster_digest != [0; 32]
        && v.namespace_digest != [0; 32]
        && v.service_account_digest != [0; 32]
        && v.audience_digest != [0; 32]
        && v.attestation_digest != [0; 32]
        && v.maximum_lifetime_ns > 0
        && v.maximum_lifetime_ns <= policy.maximum_identity_lifetime_ns
        && !v.secret_value_embedded
        && !v.bearer_token_embedded
        && !v.strategy_access_allowed
        && !v.export_allowed
}
fn valid_signer(v: &IsolatedSignerContract, policy: &SecurityPolicy) -> bool {
    v.verify_digest()
        && v.process_identity_digest != [0; 32]
        && v.allowed_purposes == SigningPurpose::ALL
        && !v.allowed_resource_digests.is_empty()
        && v.allowed_resource_digests.len() <= MAX_ITEMS
        && v.allowed_resource_digests.iter().all(|r| *r != [0; 32])
        && v.maximum_request_units > 0
        && v.maximum_request_units <= policy.maximum_request_units
        && v.maximum_requests_per_window > 0
        && v.maximum_requests_per_window <= policy.maximum_requests_per_window
        && v.request_lifetime_ns > 0
        && v.request_lifetime_ns <= policy.maximum_identity_lifetime_ns
        && v.dual_control_required
        && !v.arbitrary_payload_allowed
        && !v.transfer_allowed
        && !v.withdrawal_allowed
        && !v.contract_upgrade_allowed
        && !v.strategy_direct_access_allowed
}
fn valid_identity(
    v: &OpaqueWorkloadIdentity,
    contract: &WorkloadIdentityContract,
    policy: &SecurityPolicy,
    at: i64,
) -> bool {
    v.verify_digest()
        && v.identity_id_digest != [0; 32]
        && v.contract_digest == contract.contract_digest
        && v.epoch > 0
        && v.epoch <= policy.maximum_identity_epochs
        && v.issued_at_ns <= at
        && v.expires_at_ns > v.issued_at_ns
        && v.expires_at_ns
            <= v.issued_at_ns
                .checked_add(contract.maximum_lifetime_ns)
                .unwrap_or(i64::MIN)
        && v.attestation_digest == contract.attestation_digest
        && !v.secret_material_present
        && !v.token_value_present
        && !v.provider_contacted
}
fn valid_observation(
    v: &SecurityObservation,
    provider: &FakeProviderContract,
    workload: &WorkloadIdentityContract,
    policy: &SecurityPolicy,
    plan: &SecurityPlan,
    at: i64,
) -> bool {
    v.verify_digest()
        && v.observation_id != [0; 32]
        && v.provider_contract_digest == provider.contract_digest
        && v.observed_at_ns >= plan.created_at_ns
        && v.observed_at_ns <= at
        && at
            .checked_sub(v.observed_at_ns)
            .is_some_and(|age| age <= policy.maximum_observation_age_ns)
        && !v.secret_material_observed
        && !v.signature_produced
        && !v.provider_contacted
        && !v.socket_opened
        && !v.external_mutation_observed
        && !v.signer_activated
        && v.identity
            .as_ref()
            .is_none_or(|identity| valid_identity(identity, workload, policy, v.observed_at_ns))
}
fn valid_recovery(
    v: &SecurityRecoveryEvidence,
    requirement: &SecurityRecoveryRequirement,
    at: i64,
) -> bool {
    v.verify_digest()
        && v.recovery_id != [0; 32]
        && v.requirement_digest == requirement.requirement_digest
        && v.recovered_epoch == requirement.required_epoch
        && v.state_digest != [0; 32]
        && v.observed_at_ns <= at
        && v.no_mutation_observed
        && !v.secret_material_observed
        && !v.signature_produced
        && !v.provider_contacted
        && !v.socket_opened
        && !v.identity_activated
}

/// Encodes one bounded versioned command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &SecurityCommand) -> Result<Vec<u8>, Error> {
    let body = serde_json::to_vec(command).map_err(|e| Error::Json(e.to_string()))?;
    if body.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut bytes = Vec::with_capacity(body.len() + 2);
    bytes.extend_from_slice(&WIRE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}
/// Decodes one bounded versioned command.
///
/// # Errors
///
/// Rejects size, version, JSON, unknown fields, or trailing bytes.
pub fn decode_command(bytes: &[u8]) -> Result<SecurityCommand, Error> {
    if bytes.len() < 2 || bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let version = u16::from_le_bytes(bytes[..2].try_into().map_err(|_| Error::CommandBound)?);
    if version != WIRE_VERSION {
        return Err(Error::Version(version));
    }
    let mut decoder = serde_json::Deserializer::from_slice(&bytes[2..]);
    let command =
        SecurityCommand::deserialize(&mut decoder).map_err(|e| Error::Json(e.to_string()))?;
    decoder.end().map_err(|e| Error::Json(e.to_string()))?;
    Ok(command)
}
fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(&serde_json::to_vec(value).expect("serializable security state"));
    *h.finalize().as_bytes()
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
fn hash_value<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable security state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[cfg(test)]
mod tests;
