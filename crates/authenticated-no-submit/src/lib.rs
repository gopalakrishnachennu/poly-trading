#![forbid(unsafe_code)]

//! Deterministic certification of authenticated observation with no submit path.

mod durable;
mod report;
pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, AuthCheckpoint, AuthRecovery,
    AuthStorageError, DurableAuthCertification,
};
pub use report::{read_report, write_report_create_new, AuthReportFileError};

use live_data_paper_certification::{PaperReport, PaperReportStatus, PaperScenario};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct AuthCommandId(pub [u8; 32]);
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthPolicy {
    pub maximum_paper_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_identity_lifetime_ns: i64,
    pub maximum_fixture_age_ns: i64,
    pub maximum_backoff_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ObservationContract {
    pub workload_digest: [u8; 32],
    pub provider_digest: [u8; 32],
    pub primary_region_digest: [u8; 32],
    pub recovery_region_digest: [u8; 32],
    pub endpoint_digest: [u8; 32],
    pub allowed_events_digest: [u8; 32],
    pub observation_only: bool,
    pub credential_value_present: bool,
    pub private_key_present: bool,
    pub signature_capability_present: bool,
    pub submit_endpoint_present: bool,
    pub cancel_endpoint_present: bool,
    pub wallet_mutation_present: bool,
    pub arbitrary_request_allowed: bool,
    pub submit_policy_denied: bool,
    pub cancel_policy_denied: bool,
    pub transfer_policy_denied: bool,
    pub withdrawal_policy_denied: bool,
    pub upgrade_policy_denied: bool,
    pub contract_digest: [u8; 32],
}
impl ObservationContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.contract_digest = digest_without(b"auth-no-submit-contract-v1", &self, |v| {
            v.contract_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest
            == digest_without(b"auth-no-submit-contract-v1", self, |v| {
                v.contract_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpaqueIdentity {
    pub identity_id: [u8; 32],
    pub epoch: u64,
    pub predecessor_digest: Option<[u8; 32]>,
    pub workload_digest: [u8; 32],
    pub provider_digest: [u8; 32],
    pub issued_at_ns: i64,
    pub expires_at_ns: i64,
    pub revoked: bool,
    pub identity_digest: [u8; 32],
}
impl OpaqueIdentity {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.identity_digest = digest_without(b"opaque-auth-identity-v1", &self, |v| {
            v.identity_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.identity_digest
            == digest_without(b"opaque-auth-identity-v1", self, |v| {
                v.identity_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScenario {
    ActivationDryRun,
    AuthenticatedObservation,
    Rotation,
    Revocation,
    ProviderOutage,
    DeadMan,
    UnknownReconciliation,
    DisasterRecovery,
    PhysicalNoSubmit,
    LogicalNoSubmit,
}
impl AuthScenario {
    pub const ALL: [Self; 10] = [
        Self::ActivationDryRun,
        Self::AuthenticatedObservation,
        Self::Rotation,
        Self::Revocation,
        Self::ProviderOutage,
        Self::DeadMan,
        Self::UnknownReconciliation,
        Self::DisasterRecovery,
        Self::PhysicalNoSubmit,
        Self::LogicalNoSubmit,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct AuthFixture {
    pub fixture_id: [u8; 32],
    pub sequence: u64,
    pub scenario: AuthScenario,
    pub identity_digest: [u8; 32],
    pub region_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub accepted: bool,
    pub no_mutation_observed: bool,
    pub automatic_retry_attempted: bool,
    pub credential_value_present: bool,
    pub signature_produced: bool,
    pub authenticated_connection_opened: bool,
    pub submission_capability_present: bool,
    pub logical_mutation_allowed: bool,
    pub reconciliation_complete: bool,
    pub backoff_ns: i64,
    pub fixture_digest: [u8; 32],
}
impl AuthFixture {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.fixture_digest = digest_without(b"auth-no-submit-fixture-v1", &self, |v| {
            v.fixture_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.fixture_digest
            == digest_without(b"auth-no-submit-fixture-v1", self, |v| {
                v.fixture_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthPlan {
    pub plan_id: [u8; 32],
    pub paper_report: PaperReport,
    pub contract: ObservationContract,
    pub required_scenarios: Vec<AuthScenario>,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}
impl AuthPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &AuthPolicy) -> Self {
        self.required_scenarios.sort();
        self.policy_digest = digest_json(b"auth-no-submit-policy-v1", policy);
        self.plan_digest = digest_without(b"auth-no-submit-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self, policy: &AuthPolicy) -> bool {
        self.policy_digest == digest_json(b"auth-no-submit-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"auth-no-submit-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthReportStatus {
    LocallyCertified,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct AuthReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub paper_report_digest: [u8; 32],
    pub final_identity_epoch: u64,
    pub covered_scenarios: Vec<AuthScenario>,
    pub finalized_at_ns: i64,
    pub status: AuthReportStatus,
    pub real_identity_activated: bool,
    pub credential_material_created: bool,
    pub signature_produced: bool,
    pub authenticated_connection_opened: bool,
    pub submit_capability_present: bool,
    pub capital_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub submission_authority_granted: bool,
    pub report_digest: [u8; 32],
}
impl AuthReport {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"auth-no-submit-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"auth-no-submit-report-v1", self, |v| {
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
pub enum AuthCommand {
    Register {
        command_id: AuthCommandId,
        plan: Box<AuthPlan>,
        recorded_at_ns: i64,
    },
    Issue {
        command_id: AuthCommandId,
        identity: OpaqueIdentity,
        recorded_at_ns: i64,
    },
    Rotate {
        command_id: AuthCommandId,
        predecessor_digest: [u8; 32],
        identity: OpaqueIdentity,
        recorded_at_ns: i64,
    },
    RecordFixture {
        command_id: AuthCommandId,
        fixture: AuthFixture,
        recorded_at_ns: i64,
    },
    Revoke {
        command_id: AuthCommandId,
        identity_digest: [u8; 32],
        revoked_at_ns: i64,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: AuthCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}
impl AuthCommand {
    #[must_use]
    pub const fn command_id(&self) -> AuthCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::Issue { command_id, .. }
            | Self::Rotate { command_id, .. }
            | Self::RecordFixture { command_id, .. }
            | Self::Revoke { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::Issue { recorded_at_ns, .. }
            | Self::Rotate { recorded_at_ns, .. }
            | Self::RecordFixture { recorded_at_ns, .. }
            | Self::Revoke { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum AuthDetail {
    Registered,
    IdentityIssued,
    IdentityRotated,
    FixtureAccepted,
    IdentityRevoked,
    Finalized(Box<AuthReport>),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthOutcome {
    pub command_id: AuthCommandId,
    pub detail: AuthDetail,
    pub outcome_digest: [u8; 32],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthSnapshot {
    pub identity: Option<OpaqueIdentity>,
    pub covered_scenarios: BTreeSet<AuthScenario>,
    pub fixture_count: u64,
    pub report: Option<AuthReport>,
    pub accepted_commands: u64,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("auth no-submit policy invalid")]
    Config,
    #[error("auth no-submit timestamp invalid")]
    Timestamp,
    #[error("auth no-submit command exceeds bound")]
    CommandBound,
    #[error("auth no-submit JSON invalid: {0}")]
    Json(String),
    #[error("unsupported auth no-submit version: {0}")]
    Version(u16),
    #[error("auth command id conflict")]
    IdempotencyConflict,
    #[error("Phase 3.5 evidence invalid or authority-bearing")]
    Upstream,
    #[error("auth no-submit plan invalid")]
    Plan,
    #[error("opaque identity transition invalid")]
    Identity,
    #[error("auth fixture invalid")]
    Fixture,
    #[error("auth finalization invalid")]
    Finalize,
    #[error("auth arithmetic overflow")]
    Overflow,
    #[error("auth certification halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct AuthNoSubmitCertification {
    policy: AuthPolicy,
    plan: Option<AuthPlan>,
    identity: Option<OpaqueIdentity>,
    covered: BTreeSet<AuthScenario>,
    used_fixtures: BTreeSet<[u8; 32]>,
    fixture_count: u64,
    processed: BTreeMap<AuthCommandId, ([u8; 32], AuthOutcome)>,
    accepted_commands: u64,
    report: Option<AuthReport>,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}
impl AuthNoSubmitCertification {
    /// Creates an empty certification owner.
    /// # Errors
    /// Rejects invalid policy bounds.
    pub fn new(policy: AuthPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            identity: None,
            covered: BTreeSet::new(),
            used_fixtures: BTreeSet::new(),
            fixture_count: 0,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            report: None,
            last_recorded_at_ns: None,
            halted: None,
        })
    }
    /// Applies one deterministic command.
    /// # Errors
    /// Invalid chronology, evidence, lifecycle or finalization halts.
    pub fn apply(&mut self, command: &AuthCommand) -> Result<AuthOutcome, Error> {
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
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        let mut outcome = AuthOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"auth-no-submit-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }
    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &AuthCommand) -> Result<AuthDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            AuthCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some()
                    || !valid_upstream(&plan.paper_report, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(AuthDetail::Registered)
            }
            AuthCommand::Issue {
                identity,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Identity)?;
                if self.identity.is_some()
                    || !valid_identity(identity, plan, &self.policy, *recorded_at_ns)
                    || identity.epoch != 1
                    || identity.predecessor_digest.is_some()
                    || identity.revoked
                {
                    return Err(Error::Identity);
                }
                self.identity = Some(identity.clone());
                Ok(AuthDetail::IdentityIssued)
            }
            AuthCommand::Rotate {
                predecessor_digest,
                identity,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Identity)?;
                let prior = self.identity.as_ref().ok_or(Error::Identity)?;
                if prior.revoked
                    || *predecessor_digest != prior.identity_digest
                    || !valid_identity(identity, plan, &self.policy, *recorded_at_ns)
                    || identity.epoch != prior.epoch.checked_add(1).ok_or(Error::Overflow)?
                    || identity.predecessor_digest != Some(prior.identity_digest)
                    || identity.revoked
                {
                    return Err(Error::Identity);
                }
                self.identity = Some(identity.clone());
                self.covered.insert(AuthScenario::Rotation);
                Ok(AuthDetail::IdentityRotated)
            }
            AuthCommand::RecordFixture {
                fixture,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Fixture)?;
                let identity = self.identity.as_ref().ok_or(Error::Fixture)?;
                if self.used_fixtures.contains(&fixture.fixture_id)
                    || !fixture.verify_digest()
                    || fixture.sequence
                        != self.fixture_count.checked_add(1).ok_or(Error::Overflow)?
                    || fixture.identity_digest != identity.identity_digest
                    || fixture.observed_at_ns > *recorded_at_ns
                    || *recorded_at_ns - fixture.observed_at_ns > self.policy.maximum_fixture_age_ns
                    || fixture.credential_value_present
                    || fixture.signature_produced
                    || fixture.authenticated_connection_opened
                    || fixture.submission_capability_present
                    || fixture.logical_mutation_allowed
                    || fixture.automatic_retry_attempted
                    || !fixture.no_mutation_observed
                    || fixture.backoff_ns < 0
                    || fixture.backoff_ns > self.policy.maximum_backoff_ns
                    || (matches!(
                        fixture.scenario,
                        AuthScenario::UnknownReconciliation
                            | AuthScenario::ProviderOutage
                            | AuthScenario::DeadMan
                    ) && !fixture.reconciliation_complete)
                    || (fixture.scenario == AuthScenario::DisasterRecovery
                        && fixture.region_digest != plan.contract.recovery_region_digest)
                    || (fixture.scenario != AuthScenario::DisasterRecovery
                        && fixture.region_digest != plan.contract.primary_region_digest)
                    || !fixture.accepted
                {
                    return Err(Error::Fixture);
                }
                self.fixture_count += 1;
                self.used_fixtures.insert(fixture.fixture_id);
                self.covered.insert(fixture.scenario);
                Ok(AuthDetail::FixtureAccepted)
            }
            AuthCommand::Revoke {
                identity_digest,
                revoked_at_ns,
                ..
            } => {
                let current = self.identity.as_mut().ok_or(Error::Identity)?;
                if current.revoked
                    || *identity_digest != current.identity_digest
                    || *revoked_at_ns < current.issued_at_ns
                {
                    return Err(Error::Identity);
                }
                current.revoked = true;
                *current = current.clone().sealed();
                self.covered.insert(AuthScenario::Revocation);
                Ok(AuthDetail::IdentityRevoked)
            }
            AuthCommand::Finalize {
                report_id,
                finalized_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                let identity = self.identity.as_ref().ok_or(Error::Finalize)?;
                if !identity.revoked
                    || !plan
                        .required_scenarios
                        .iter()
                        .all(|v| self.covered.contains(v))
                    || *finalized_at_ns > plan.expires_at_ns
                {
                    return Err(Error::Finalize);
                }
                let report = AuthReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    paper_report_digest: plan.paper_report.report_digest,
                    final_identity_epoch: identity.epoch,
                    covered_scenarios: self.covered.iter().copied().collect(),
                    finalized_at_ns: *finalized_at_ns,
                    status: AuthReportStatus::LocallyCertified,
                    real_identity_activated: false,
                    credential_material_created: false,
                    signature_produced: false,
                    authenticated_connection_opened: false,
                    submit_capability_present: false,
                    capital_authority_granted: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    submission_authority_granted: false,
                    report_digest: [0; 32],
                }
                .sealed();
                self.report = Some(report.clone());
                Ok(AuthDetail::Finalized(Box::new(report)))
            }
        }
    }
    #[must_use]
    pub fn snapshot(&self) -> AuthSnapshot {
        let material = (
            &self.identity,
            &self.covered,
            self.fixture_count,
            &self.report,
            self.accepted_commands,
            &self.halted,
        );
        AuthSnapshot {
            identity: self.identity.clone(),
            covered_scenarios: self.covered.clone(),
            fixture_count: self.fixture_count,
            report: self.report.clone(),
            accepted_commands: self.accepted_commands,
            halted: self.halted.is_some(),
            digest: digest_json(b"auth-no-submit-state-v1", &material),
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
fn validate_policy(p: &AuthPolicy) -> Result<(), Error> {
    if p.maximum_paper_report_age_ns <= 0
        || p.maximum_plan_lifetime_ns <= 0
        || p.maximum_identity_lifetime_ns <= 0
        || p.maximum_fixture_age_ns <= 0
        || p.maximum_backoff_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}
fn valid_upstream(r: &PaperReport, p: &AuthPolicy, at: i64) -> bool {
    r.verify_digest()
        && r.status == PaperReportStatus::LocallyCertified
        && r.covered_scenarios == PaperScenario::ALL
        && r.finalized_at_ns <= at
        && at - r.finalized_at_ns <= p.maximum_paper_report_age_ns
        && !r.real_pnl_observed
        && !r.credential_material_created
        && !r.external_connection_opened
        && !r.external_mutation_observed
        && !r.capital_authority_granted
        && !r.deployment_authority_granted
        && !r.trading_authority_granted
        && !r.submission_authority_granted
}
fn valid_plan(v: &AuthPlan, p: &AuthPolicy, at: i64) -> bool {
    v.verify_digest(p)
        && v.required_scenarios == AuthScenario::ALL
        && valid_contract(&v.contract)
        && v.created_at_ns <= at
        && v.expires_at_ns >= at
        && v.expires_at_ns - v.created_at_ns <= p.maximum_plan_lifetime_ns
}
fn valid_contract(v: &ObservationContract) -> bool {
    v.verify_digest()
        && v.workload_digest != [0; 32]
        && v.provider_digest != [0; 32]
        && v.primary_region_digest != v.recovery_region_digest
        && v.observation_only
        && !v.credential_value_present
        && !v.private_key_present
        && !v.signature_capability_present
        && !v.submit_endpoint_present
        && !v.cancel_endpoint_present
        && !v.wallet_mutation_present
        && !v.arbitrary_request_allowed
        && v.submit_policy_denied
        && v.cancel_policy_denied
        && v.transfer_policy_denied
        && v.withdrawal_policy_denied
        && v.upgrade_policy_denied
}
fn valid_identity(v: &OpaqueIdentity, plan: &AuthPlan, p: &AuthPolicy, at: i64) -> bool {
    v.verify_digest()
        && v.workload_digest == plan.contract.workload_digest
        && v.provider_digest == plan.contract.provider_digest
        && v.issued_at_ns <= at
        && v.expires_at_ns > at
        && v.expires_at_ns - v.issued_at_ns <= p.maximum_identity_lifetime_ns
}
fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(&serde_json::to_vec(value).expect("bounded serialization"));
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
/// Encodes one bounded command.
///
/// # Errors
///
/// Rejects serialization or size failure.
pub fn encode_command(command: &AuthCommand) -> Result<Vec<u8>, Error> {
    let b = serde_json::to_vec(&(WIRE_VERSION, command)).map_err(|e| Error::Json(e.to_string()))?;
    if b.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    Ok(b)
}
/// Decodes one strict command.
///
/// # Errors
///
/// Rejects malformed, trailing, noncanonical, oversized or unsupported data.
pub fn decode_command(bytes: &[u8]) -> Result<AuthCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let (v, c): (u16, AuthCommand) =
        Deserialize::deserialize(&mut d).map_err(|e| Error::Json(e.to_string()))?;
    d.end().map_err(|e| Error::Json(e.to_string()))?;
    if v != WIRE_VERSION {
        return Err(Error::Version(v));
    }
    if encode_command(&c)? != bytes {
        return Err(Error::Json("noncanonical command".into()));
    }
    Ok(c)
}
#[cfg(test)]
mod tests;
