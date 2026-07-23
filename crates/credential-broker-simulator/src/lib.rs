#![forbid(unsafe_code)]

//! Deterministic credential-broker and signing-policy simulation.
//!
//! No key material, cryptographic signature, provider client, credential,
//! network transport, or external submission exists in this crate.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, BrokerCheckpoint,
    BrokerRecovery, BrokerStorageError, DurableCredentialBroker,
};
pub use report::{read_report, write_report_create_new, BrokerReportFileError};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use transport_adapter_certification::{TransportAdapterCertificate, TransportCertificateStatus};

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct BrokerCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerPolicy {
    pub maximum_certificate_age_ns: i64,
    pub maximum_campaign_age_ns: i64,
    pub maximum_approval_age_ns: i64,
    pub maximum_permit_lifetime_ns: i64,
    pub maximum_requests: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimulatedAlgorithm {
    Ed25519,
    Secp256k1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct OpaqueKeyHandle {
    pub handle_digest: [u8; 32],
    pub provider_attestation_digest: [u8; 32],
    pub algorithm: SimulatedAlgorithm,
    pub key_material_present: bool,
    pub exportable: bool,
    pub provider_access_enabled: bool,
    pub initially_revoked: bool,
    pub descriptor_digest: [u8; 32],
}

impl OpaqueKeyHandle {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.descriptor_digest = digest_without(b"opaque-key-handle-v1", &self, |v| {
            v.descriptor_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.descriptor_digest
            == digest_without(b"opaque-key-handle-v1", self, |v| {
                v.descriptor_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigningPurpose {
    DeploymentRequest,
    HealthVerification,
    RollbackRequest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct SigningPolicyContract {
    pub allowed_purposes: Vec<SigningPurpose>,
    pub allowed_subject_digests: Vec<[u8; 32]>,
    pub maximum_units_per_request: u64,
    pub maximum_total_units: u64,
    pub valid_from_ns: i64,
    pub valid_until_ns: i64,
    pub dual_authorization_required: bool,
    pub arbitrary_payload_allowed: bool,
    pub transfer_allowed: bool,
    pub withdrawal_allowed: bool,
    pub wallet_access_allowed: bool,
    pub external_submission_allowed: bool,
    pub policy_digest: [u8; 32],
}

impl SigningPolicyContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.allowed_purposes.sort();
        self.allowed_subject_digests.sort_unstable();
        self.policy_digest = digest_without(b"simulated-signing-policy-v1", &self, |v| {
            v.policy_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.policy_digest
            == digest_without(b"simulated-signing-policy-v1", self, |v| {
                v.policy_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SigningRequest {
    pub sequence: u32,
    pub request_id: [u8; 32],
    pub purpose: SigningPurpose,
    pub subject_digest: [u8; 32],
    pub payload_digest: [u8; 32],
    pub nonce_digest: [u8; 32],
    pub units: u64,
    pub not_before_ns: i64,
    pub expires_at_ns: i64,
    pub request_digest: [u8; 32],
}

impl SigningRequest {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.request_digest = digest_without(b"simulated-signing-request-v1", &self, |v| {
            v.request_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.request_digest
            == digest_without(b"simulated-signing-request-v1", self, |v| {
                v.request_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerPlan {
    pub plan_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub transport_certificate: TransportAdapterCertificate,
    pub key_handle: OpaqueKeyHandle,
    pub signing_policy: SigningPolicyContract,
    pub requests: Vec<SigningRequest>,
    pub broker_policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl BrokerPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &BrokerPolicy) -> Self {
        self.broker_policy_digest = digest_json(b"credential-broker-policy-v1", policy);
        self.plan_digest = digest_without(b"credential-broker-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self, policy: &BrokerPolicy) -> bool {
        self.broker_policy_digest == digest_json(b"credential-broker-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"credential-broker-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignerFixtureCase {
    ValidDryRun,
    WrongPurpose,
    WrongSubject,
    UnitsExceeded,
    ExpiredRequest,
    NonceReplay,
    RevokedHandle,
    ProviderUnavailable,
    AttestationMismatch,
}

impl SignerFixtureCase {
    pub const ALL: [Self; 9] = [
        Self::ValidDryRun,
        Self::WrongPurpose,
        Self::WrongSubject,
        Self::UnitsExceeded,
        Self::ExpiredRequest,
        Self::NonceReplay,
        Self::RevokedHandle,
        Self::ProviderUnavailable,
        Self::AttestationMismatch,
    ];
    #[must_use]
    pub const fn expected(self) -> SignerFixtureDisposition {
        match self {
            Self::ValidDryRun => SignerFixtureDisposition::SimulatedReceiptOnly,
            Self::ProviderUnavailable => SignerFixtureDisposition::FailClosed,
            _ => SignerFixtureDisposition::Deny,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignerFixtureDisposition {
    SimulatedReceiptOnly,
    Deny,
    FailClosed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecordedSignerFixture {
    pub sequence: u8,
    pub case: SignerFixtureCase,
    pub expected: SignerFixtureDisposition,
    pub observed: SignerFixtureDisposition,
    pub observed_at_ns: i64,
    pub fixture_source_digest: [u8; 32],
    pub recorded_fixture: bool,
    pub key_material_accessed: bool,
    pub provider_contacted: bool,
    pub real_signature_produced: bool,
    pub credential_created: bool,
    pub authenticated_transport_used: bool,
    pub external_submission_observed: bool,
    pub fixture_digest: [u8; 32],
}

impl RecordedSignerFixture {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.fixture_digest = digest_without(b"recorded-signer-fixture-v1", &self, |v| {
            v.fixture_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.fixture_digest
            == digest_without(b"recorded-signer-fixture-v1", self, |v| {
                v.fixture_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationRole {
    Security,
    Operations,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestAuthorization {
    pub authorization_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub request_digest: [u8; 32],
    pub role: AuthorizationRole,
    pub operator_id: [u8; 32],
    pub approved: bool,
    pub authorized_at_ns: i64,
    pub valid_until_ns: i64,
    pub authorization_digest: [u8; 32],
}

impl RequestAuthorization {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.authorization_digest =
            digest_without(b"simulated-signing-authorization-v1", &self, |v| {
                v.authorization_digest = [0; 32];
            });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.authorization_digest
            == digest_without(b"simulated-signing-authorization-v1", self, |v| {
                v.authorization_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulatedSigningPermit {
    pub permit_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub key_descriptor_digest: [u8; 32],
    pub signing_policy_digest: [u8; 32],
    pub request: SigningRequest,
    pub security_authorization_digest: [u8; 32],
    pub operations_authorization_digest: [u8; 32],
    pub issued_at_ns: i64,
    pub expires_at_ns: i64,
    pub one_use: bool,
    pub simulated_only: bool,
    pub permit_digest: [u8; 32],
}

impl SimulatedSigningPermit {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.permit_digest
            == digest_without(b"simulated-signing-permit-v1", self, |v| {
                v.permit_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct SimulatedSigningReceipt {
    pub receipt_id: [u8; 32],
    pub permit_digest: [u8; 32],
    pub request_digest: [u8; 32],
    pub nonce_digest: [u8; 32],
    pub consumed_at_ns: i64,
    pub simulated_only: bool,
    pub signature_bytes_present: bool,
    pub key_material_accessed: bool,
    pub provider_contacted: bool,
    pub authentication_authority_granted: bool,
    pub external_submission_authority_granted: bool,
    pub receipt_digest: [u8; 32],
}

impl SimulatedSigningReceipt {
    /// Seals a simulator-generated receipt for deterministic downstream tests.
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.receipt_digest = digest_without(b"simulated-signing-receipt-v1", &self, |v| {
            v.receipt_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.receipt_digest
            == digest_without(b"simulated-signing-receipt-v1", self, |v| {
                v.receipt_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerReportStatus {
    SimulationCompleted,
    HandleRevoked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct BrokerCertificationReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub transport_certificate_digest: [u8; 32],
    pub key_descriptor_digest: [u8; 32],
    pub fixture_chain_digest: [u8; 32],
    pub receipt_chain_digest: [u8; 32],
    pub completed_request_count: usize,
    pub finalized_at_ns: i64,
    pub status: BrokerReportStatus,
    pub key_material_created: bool,
    pub real_signature_produced: bool,
    pub provider_contacted: bool,
    pub authentication_authority_granted: bool,
    pub external_submission_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl BrokerCertificationReport {
    /// Seals a non-authorizing report for deterministic downstream use.
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"credential-broker-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"credential-broker-report-v1", self, |v| {
                v.report_digest = [0; 32];
            })
    }
}

/// Recomputes the canonical Phase 2.30 receipt chain.
#[must_use]
pub fn simulated_receipt_chain_digest(receipts: &[SimulatedSigningReceipt]) -> [u8; 32] {
    receipts.iter().fold([0; 32], |prior, receipt| {
        chain_digest(b"simulated-receipt-chain-v1", prior, receipt.receipt_digest)
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum BrokerCommand {
    Register {
        command_id: BrokerCommandId,
        plan: Box<BrokerPlan>,
        recorded_at_ns: i64,
    },
    RecordFixture {
        command_id: BrokerCommandId,
        fixture: Box<RecordedSignerFixture>,
        recorded_at_ns: i64,
    },
    Authorize {
        command_id: BrokerCommandId,
        authorization: RequestAuthorization,
        recorded_at_ns: i64,
    },
    IssuePermit {
        command_id: BrokerCommandId,
        permit_id: [u8; 32],
        issued_at_ns: i64,
        requested_expires_at_ns: i64,
        recorded_at_ns: i64,
    },
    ConsumePermit {
        command_id: BrokerCommandId,
        permit: Box<SimulatedSigningPermit>,
        receipt_id: [u8; 32],
        consumed_at_ns: i64,
        recorded_at_ns: i64,
    },
    Revoke {
        command_id: BrokerCommandId,
        revocation_id: [u8; 32],
        revoked_at_ns: i64,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: BrokerCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl BrokerCommand {
    #[must_use]
    pub const fn command_id(&self) -> BrokerCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::RecordFixture { command_id, .. }
            | Self::Authorize { command_id, .. }
            | Self::IssuePermit { command_id, .. }
            | Self::ConsumePermit { command_id, .. }
            | Self::Revoke { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::RecordFixture { recorded_at_ns, .. }
            | Self::Authorize { recorded_at_ns, .. }
            | Self::IssuePermit { recorded_at_ns, .. }
            | Self::ConsumePermit { recorded_at_ns, .. }
            | Self::Revoke { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum BrokerDetail {
    Registered,
    FixtureRecorded(SignerFixtureCase),
    Authorized(AuthorizationRole),
    PermitIssued(Box<SimulatedSigningPermit>),
    PermitConsumed(Box<SimulatedSigningReceipt>),
    Revoked,
    Finalized(Box<BrokerCertificationReport>),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerOutcome {
    pub command_id: BrokerCommandId,
    pub detail: BrokerDetail,
    pub outcome_digest: [u8; 32],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokerSnapshot {
    pub accepted_commands: u64,
    pub fixture_count: usize,
    pub completed_requests: usize,
    pub active_permit: Option<SimulatedSigningPermit>,
    pub revoked: bool,
    pub last_report: Option<BrokerCertificationReport>,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("credential-broker policy is invalid")]
    Config,
    #[error("credential-broker timestamp is invalid or regressed")]
    Timestamp,
    #[error("credential-broker command exceeds its bound")]
    CommandBound,
    #[error("credential-broker JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported credential-broker command version: {0}")]
    Version(u16),
    #[error("credential-broker command id conflict")]
    IdempotencyConflict,
    #[error("Phase 2.29 certificate is invalid, stale, substituted, or authority-bearing")]
    Upstream,
    #[error("key descriptor, signing policy, request, or plan is invalid")]
    Plan,
    #[error("signer fixture is invalid, out of order, or side-effect-bearing")]
    Fixture,
    #[error("request authorization is invalid, stale, duplicated, or substituted")]
    Authorization,
    #[error("signing permit is invalid, expired, replayed, or out of order")]
    Permit,
    #[error("opaque key handle is revoked")]
    Revoked,
    #[error("credential-broker finalization is invalid")]
    Finalize,
    #[error("credential-broker arithmetic overflow")]
    Overflow,
    #[error("credential-broker is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct CredentialBrokerSimulator {
    policy: BrokerPolicy,
    plan: Option<BrokerPlan>,
    fixtures: Vec<RecordedSignerFixture>,
    fixture_chain_digest: [u8; 32],
    authorizations: BTreeMap<(u32, AuthorizationRole), RequestAuthorization>,
    authorization_ids: BTreeSet<[u8; 32]>,
    active_permit: Option<SimulatedSigningPermit>,
    used_permit_ids: BTreeSet<[u8; 32]>,
    used_receipt_ids: BTreeSet<[u8; 32]>,
    consumed_nonces: BTreeSet<[u8; 32]>,
    completed_requests: usize,
    consumed_units: u64,
    receipt_chain_digest: [u8; 32],
    revoked: bool,
    report: Option<BrokerCertificationReport>,
    processed: BTreeMap<BrokerCommandId, ([u8; 32], BrokerOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl CredentialBrokerSimulator {
    /// Creates one empty offline broker simulator.
    ///
    /// # Errors
    ///
    /// Rejects zero, excessive, or inconsistent bounds.
    pub fn new(policy: BrokerPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            fixtures: Vec::new(),
            fixture_chain_digest: [0; 32],
            authorizations: BTreeMap::new(),
            authorization_ids: BTreeSet::new(),
            active_permit: None,
            used_permit_ids: BTreeSet::new(),
            used_receipt_ids: BTreeSet::new(),
            consumed_nonces: BTreeSet::new(),
            completed_requests: 0,
            consumed_units: 0,
            receipt_chain_digest: [0; 32],
            revoked: false,
            report: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic broker command.
    ///
    /// # Errors
    ///
    /// Integrity, chronology, control, replay, or lifecycle failures halt.
    pub fn apply(&mut self, command: &BrokerCommand) -> Result<BrokerOutcome, Error> {
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
        let mut outcome = BrokerOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"credential-broker-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &BrokerCommand) -> Result<BrokerDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            BrokerCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some() {
                    return Err(Error::Plan);
                }
                if !valid_upstream(&plan.transport_certificate, &self.policy, *recorded_at_ns) {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(BrokerDetail::Registered)
            }
            BrokerCommand::RecordFixture {
                fixture,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Fixture)?;
                let index = self.fixtures.len();
                if index >= SignerFixtureCase::ALL.len()
                    || fixture.sequence as usize != index
                    || fixture.case != SignerFixtureCase::ALL[index]
                    || fixture.expected != fixture.case.expected()
                    || fixture.observed != fixture.expected
                    || fixture.observed_at_ns < plan.created_at_ns
                    || fixture.observed_at_ns > *recorded_at_ns
                    || !valid_fixture(fixture)
                {
                    return Err(Error::Fixture);
                }
                self.fixture_chain_digest = chain_digest(
                    b"signer-fixture-chain-v1",
                    self.fixture_chain_digest,
                    fixture.fixture_digest,
                );
                self.fixtures.push((**fixture).clone());
                Ok(BrokerDetail::FixtureRecorded(fixture.case))
            }
            BrokerCommand::Authorize {
                authorization,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Authorization)?;
                if self.revoked
                    || self.fixtures.len() != SignerFixtureCase::ALL.len()
                    || self.completed_requests >= plan.requests.len()
                {
                    return Err(if self.revoked {
                        Error::Revoked
                    } else {
                        Error::Authorization
                    });
                }
                let request = &plan.requests[self.completed_requests];
                if !authorization.verify_digest()
                    || authorization.authorization_id == [0; 32]
                    || self
                        .authorization_ids
                        .contains(&authorization.authorization_id)
                    || authorization.plan_digest != plan.plan_digest
                    || authorization.request_digest != request.request_digest
                    || authorization.operator_id == [0; 32]
                    || !authorization.approved
                    || authorization.authorized_at_ns > *recorded_at_ns
                    || authorization.valid_until_ns <= authorization.authorized_at_ns
                    || authorization.valid_until_ns > request.expires_at_ns
                    || authorization
                        .authorized_at_ns
                        .checked_sub(plan.created_at_ns)
                        .is_none_or(|age| age > self.policy.maximum_approval_age_ns)
                    || self
                        .authorizations
                        .contains_key(&(request.sequence, authorization.role))
                {
                    return Err(Error::Authorization);
                }
                if self.authorizations.iter().any(|((sequence, _), prior)| {
                    *sequence == request.sequence && prior.operator_id == authorization.operator_id
                }) {
                    return Err(Error::Authorization);
                }
                self.authorization_ids
                    .insert(authorization.authorization_id);
                self.authorizations.insert(
                    (request.sequence, authorization.role),
                    authorization.clone(),
                );
                Ok(BrokerDetail::Authorized(authorization.role))
            }
            BrokerCommand::IssuePermit {
                permit_id,
                issued_at_ns,
                requested_expires_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Permit)?;
                if self.revoked {
                    return Err(Error::Revoked);
                }
                if self.fixtures.len() != SignerFixtureCase::ALL.len()
                    || self.active_permit.is_some()
                    || self.completed_requests >= plan.requests.len()
                    || *permit_id == [0; 32]
                    || self.used_permit_ids.contains(permit_id)
                    || *issued_at_ns > *recorded_at_ns
                    || *requested_expires_at_ns <= *issued_at_ns
                    || *requested_expires_at_ns > plan.expires_at_ns
                {
                    return Err(Error::Permit);
                }
                let maximum = issued_at_ns
                    .checked_add(self.policy.maximum_permit_lifetime_ns)
                    .ok_or(Error::Overflow)?;
                let request = &plan.requests[self.completed_requests];
                if *requested_expires_at_ns > maximum
                    || *requested_expires_at_ns > request.expires_at_ns
                    || *issued_at_ns < request.not_before_ns
                    || self.consumed_nonces.contains(&request.nonce_digest)
                {
                    return Err(Error::Permit);
                }
                let security = current_authorization(
                    &self.authorizations,
                    request,
                    AuthorizationRole::Security,
                    *issued_at_ns,
                    self.policy.maximum_approval_age_ns,
                )?;
                let operations = current_authorization(
                    &self.authorizations,
                    request,
                    AuthorizationRole::Operations,
                    *issued_at_ns,
                    self.policy.maximum_approval_age_ns,
                )?;
                if security.operator_id == operations.operator_id {
                    return Err(Error::Authorization);
                }
                let mut permit = SimulatedSigningPermit {
                    permit_id: *permit_id,
                    plan_digest: plan.plan_digest,
                    key_descriptor_digest: plan.key_handle.descriptor_digest,
                    signing_policy_digest: plan.signing_policy.policy_digest,
                    request: request.clone(),
                    security_authorization_digest: security.authorization_digest,
                    operations_authorization_digest: operations.authorization_digest,
                    issued_at_ns: *issued_at_ns,
                    expires_at_ns: *requested_expires_at_ns,
                    one_use: true,
                    simulated_only: true,
                    permit_digest: [0; 32],
                };
                permit.permit_digest =
                    digest_without(b"simulated-signing-permit-v1", &permit, |v| {
                        v.permit_digest = [0; 32];
                    });
                self.used_permit_ids.insert(*permit_id);
                self.active_permit = Some(permit.clone());
                Ok(BrokerDetail::PermitIssued(Box::new(permit)))
            }
            BrokerCommand::ConsumePermit {
                permit,
                receipt_id,
                consumed_at_ns,
                recorded_at_ns,
                ..
            } => {
                let active = self.active_permit.as_ref().ok_or(Error::Permit)?;
                if self.revoked {
                    return Err(Error::Revoked);
                }
                if **permit != *active
                    || !permit.verify_digest()
                    || *receipt_id == [0; 32]
                    || self.used_receipt_ids.contains(receipt_id)
                    || *consumed_at_ns > *recorded_at_ns
                    || *consumed_at_ns < permit.issued_at_ns
                    || *consumed_at_ns > permit.expires_at_ns
                    || self.consumed_nonces.contains(&permit.request.nonce_digest)
                {
                    return Err(Error::Permit);
                }
                let mut receipt = SimulatedSigningReceipt {
                    receipt_id: *receipt_id,
                    permit_digest: permit.permit_digest,
                    request_digest: permit.request.request_digest,
                    nonce_digest: permit.request.nonce_digest,
                    consumed_at_ns: *consumed_at_ns,
                    simulated_only: true,
                    signature_bytes_present: false,
                    key_material_accessed: false,
                    provider_contacted: false,
                    authentication_authority_granted: false,
                    external_submission_authority_granted: false,
                    receipt_digest: [0; 32],
                };
                receipt.receipt_digest =
                    digest_without(b"simulated-signing-receipt-v1", &receipt, |v| {
                        v.receipt_digest = [0; 32];
                    });
                self.consumed_nonces.insert(permit.request.nonce_digest);
                self.used_receipt_ids.insert(*receipt_id);
                self.consumed_units = self
                    .consumed_units
                    .checked_add(permit.request.units)
                    .ok_or(Error::Overflow)?;
                self.completed_requests = self
                    .completed_requests
                    .checked_add(1)
                    .ok_or(Error::Overflow)?;
                self.receipt_chain_digest = chain_digest(
                    b"simulated-receipt-chain-v1",
                    self.receipt_chain_digest,
                    receipt.receipt_digest,
                );
                self.active_permit = None;
                Ok(BrokerDetail::PermitConsumed(Box::new(receipt)))
            }
            BrokerCommand::Revoke {
                revocation_id,
                revoked_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Revoked)?;
                if self.revoked
                    || *revocation_id == [0; 32]
                    || *revoked_at_ns < plan.created_at_ns
                    || *revoked_at_ns > *recorded_at_ns
                {
                    return Err(Error::Revoked);
                }
                self.revoked = true;
                self.active_permit = None;
                Ok(BrokerDetail::Revoked)
            }
            BrokerCommand::Finalize {
                report_id,
                finalized_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                let normal = self.completed_requests == plan.requests.len();
                if *report_id == [0; 32]
                    || self.fixtures.len() != SignerFixtureCase::ALL.len()
                    || self.active_permit.is_some()
                    || !(normal || self.revoked)
                    || *finalized_at_ns < plan.created_at_ns
                    || *finalized_at_ns > *recorded_at_ns
                {
                    return Err(Error::Finalize);
                }
                let mut report = BrokerCertificationReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    transport_certificate_digest: plan.transport_certificate.certificate_digest,
                    key_descriptor_digest: plan.key_handle.descriptor_digest,
                    fixture_chain_digest: self.fixture_chain_digest,
                    receipt_chain_digest: self.receipt_chain_digest,
                    completed_request_count: self.completed_requests,
                    finalized_at_ns: *finalized_at_ns,
                    status: if self.revoked {
                        BrokerReportStatus::HandleRevoked
                    } else {
                        BrokerReportStatus::SimulationCompleted
                    },
                    key_material_created: false,
                    real_signature_produced: false,
                    provider_contacted: false,
                    authentication_authority_granted: false,
                    external_submission_authority_granted: false,
                    deployment_authority_granted: false,
                    report_digest: [0; 32],
                };
                report.report_digest =
                    digest_without(b"credential-broker-report-v1", &report, |v| {
                        v.report_digest = [0; 32];
                    });
                self.report = Some(report.clone());
                Ok(BrokerDetail::Finalized(Box::new(report)))
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> BrokerSnapshot {
        BrokerSnapshot {
            accepted_commands: self.accepted_commands,
            fixture_count: self.fixtures.len(),
            completed_requests: self.completed_requests,
            active_permit: self.active_permit.clone(),
            revoked: self.revoked,
            last_report: self.report.clone(),
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
        let mut h = blake3::Hasher::new();
        h.update(b"credential-broker-simulator-state-v1");
        hash_value(
            &mut h,
            &(
                &self.policy,
                &self.plan,
                &self.fixtures,
                self.fixture_chain_digest,
            ),
        );
        for ((sequence, role), authorization) in &self.authorizations {
            hash_value(&mut h, &(sequence, role, authorization));
        }
        hash_value(
            &mut h,
            &(
                &self.authorization_ids,
                &self.active_permit,
                &self.used_permit_ids,
                &self.used_receipt_ids,
                &self.consumed_nonces,
                self.completed_requests,
                self.consumed_units,
                self.receipt_chain_digest,
                self.revoked,
                &self.report,
            ),
        );
        for (id, (content, outcome)) in &self.processed {
            h.update(&id.0);
            h.update(content);
            hash_value(&mut h, outcome);
        }
        hash_value(
            &mut h,
            &(
                self.accepted_commands,
                self.last_recorded_at_ns,
                &self.halted,
            ),
        );
        *h.finalize().as_bytes()
    }
}

fn validate_policy(value: &BrokerPolicy) -> Result<(), Error> {
    if value.maximum_certificate_age_ns <= 0
        || value.maximum_campaign_age_ns <= 0
        || value.maximum_approval_age_ns <= 0
        || value.maximum_permit_lifetime_ns <= 0
        || value.maximum_requests == 0
        || value.maximum_requests > MAX_ITEMS
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn valid_plan(plan: &BrokerPlan, policy: &BrokerPolicy, at: i64) -> bool {
    let cert = &plan.transport_certificate;
    let key = &plan.key_handle;
    let signing = &plan.signing_policy;
    let request_ids: BTreeSet<_> = plan.requests.iter().map(|v| v.request_id).collect();
    let nonces: BTreeSet<_> = plan.requests.iter().map(|v| v.nonce_digest).collect();
    let total = plan
        .requests
        .iter()
        .try_fold(0_u64, |sum, value| sum.checked_add(value.units));
    plan.plan_id != [0; 32]
        && plan.verify_digest(policy)
        && cert.verify_digest()
        && cert.status == TransportCertificateStatus::RecordedFixtureCertified
        && cert.recorded_fixtures_only
        && !cert.socket_authority_granted
        && !cert.credential_material_created
        && !cert.authentication_authority_granted
        && !cert.external_submission_authority_granted
        && !cert.deployment_authority_granted
        && key.verify_digest()
        && key.handle_digest != [0; 32]
        && key.provider_attestation_digest != [0; 32]
        && !key.key_material_present
        && !key.exportable
        && !key.provider_access_enabled
        && !key.initially_revoked
        && valid_signing_policy(signing)
        && !plan.requests.is_empty()
        && plan.requests.len() <= policy.maximum_requests
        && request_ids.len() == plan.requests.len()
        && nonces.len() == plan.requests.len()
        && plan.requests.iter().enumerate().all(|(index, request)| {
            request.sequence as usize == index
                && request.verify_digest()
                && request.request_id != [0; 32]
                && request.payload_digest != [0; 32]
                && request.nonce_digest != [0; 32]
                && request.units > 0
                && request.units <= signing.maximum_units_per_request
                && signing
                    .allowed_purposes
                    .binary_search(&request.purpose)
                    .is_ok()
                && signing
                    .allowed_subject_digests
                    .binary_search(&request.subject_digest)
                    .is_ok()
                && request.not_before_ns >= plan.created_at_ns
                && request.expires_at_ns > request.not_before_ns
                && request.expires_at_ns <= plan.expires_at_ns
        })
        && total.is_some_and(|v| v <= signing.maximum_total_units)
        && plan.created_at_ns >= cert.certified_at_ns
        && plan.created_at_ns <= at
        && plan.expires_at_ns > plan.created_at_ns
        && plan.expires_at_ns <= signing.valid_until_ns
        && plan.created_at_ns >= signing.valid_from_ns
        && plan
            .expires_at_ns
            .checked_sub(plan.created_at_ns)
            .is_some_and(|v| v <= policy.maximum_campaign_age_ns)
        && at
            .checked_sub(cert.certified_at_ns)
            .is_some_and(|v| v <= policy.maximum_certificate_age_ns)
}

fn valid_upstream(
    certificate: &TransportAdapterCertificate,
    policy: &BrokerPolicy,
    at: i64,
) -> bool {
    certificate.verify_digest()
        && certificate.status == TransportCertificateStatus::RecordedFixtureCertified
        && certificate.recorded_fixtures_only
        && !certificate.socket_authority_granted
        && !certificate.credential_material_created
        && !certificate.authentication_authority_granted
        && !certificate.external_submission_authority_granted
        && !certificate.deployment_authority_granted
        && at
            .checked_sub(certificate.certified_at_ns)
            .is_some_and(|age| age <= policy.maximum_certificate_age_ns)
}

fn valid_signing_policy(value: &SigningPolicyContract) -> bool {
    value.verify_digest()
        && !value.allowed_purposes.is_empty()
        && canonical(&value.allowed_purposes)
        && !value.allowed_subject_digests.is_empty()
        && canonical(&value.allowed_subject_digests)
        && value.allowed_subject_digests.iter().all(|v| *v != [0; 32])
        && value.maximum_units_per_request > 0
        && value.maximum_total_units >= value.maximum_units_per_request
        && value.valid_from_ns >= 0
        && value.valid_until_ns > value.valid_from_ns
        && value.dual_authorization_required
        && !value.arbitrary_payload_allowed
        && !value.transfer_allowed
        && !value.withdrawal_allowed
        && !value.wallet_access_allowed
        && !value.external_submission_allowed
}
fn valid_fixture(value: &RecordedSignerFixture) -> bool {
    value.verify_digest()
        && value.fixture_source_digest != [0; 32]
        && value.recorded_fixture
        && !value.key_material_accessed
        && !value.provider_contacted
        && !value.real_signature_produced
        && !value.credential_created
        && !value.authenticated_transport_used
        && !value.external_submission_observed
}
fn current_authorization<'a>(
    values: &'a BTreeMap<(u32, AuthorizationRole), RequestAuthorization>,
    request: &SigningRequest,
    role: AuthorizationRole,
    at: i64,
    maximum_age_ns: i64,
) -> Result<&'a RequestAuthorization, Error> {
    values
        .get(&(request.sequence, role))
        .filter(|v| {
            v.approved
                && v.request_digest == request.request_digest
                && at >= v.authorized_at_ns
                && at <= v.valid_until_ns
                && at
                    .checked_sub(v.authorized_at_ns)
                    .is_some_and(|age| age <= maximum_age_ns)
        })
        .ok_or(Error::Authorization)
}
fn canonical<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|v| v[0] < v[1])
}
fn chain_digest(domain: &[u8], prior: [u8; 32], next: [u8; 32]) -> [u8; 32] {
    digest_json(domain, &(prior, next))
}
fn digest_without<T: Clone + Serialize>(
    domain: &[u8],
    value: &T,
    clear: impl FnOnce(&mut T),
) -> [u8; 32] {
    let mut clone = value.clone();
    clear(&mut clone);
    digest_json(domain, &clone)
}
fn digest_json<T: Serialize + ?Sized>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    hash_value(&mut h, value);
    *h.finalize().as_bytes()
}
fn hash_value<T: Serialize + ?Sized>(h: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable credential-broker state");
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandWire {
    version: u16,
    command: BrokerCommand,
}
/// Encodes one bounded canonical command.
///
/// # Errors
///
/// Rejects serialization failures and oversized commands.
pub fn encode_command(command: &BrokerCommand) -> Result<Vec<u8>, Error> {
    let bytes = serde_json::to_vec(&CommandWire {
        version: WIRE_VERSION,
        command: command.clone(),
    })
    .map_err(|e| Error::Json(e.to_string()))?;
    if bytes.len() > MAX_COMMAND_BYTES {
        Err(Error::CommandBound)
    } else {
        Ok(bytes)
    }
}
/// Decodes one bounded canonical command.
///
/// # Errors
///
/// Rejects malformed, unsupported, trailing, noncanonical, or oversized input.
pub fn decode_command(bytes: &[u8]) -> Result<BrokerCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let wire = CommandWire::deserialize(&mut de).map_err(|e| Error::Json(e.to_string()))?;
    de.end().map_err(|e| Error::Json(e.to_string()))?;
    if wire.version != WIRE_VERSION {
        return Err(Error::Version(wire.version));
    }
    if serde_json::to_vec(&wire).map_err(|e| Error::Json(e.to_string()))? != bytes {
        return Err(Error::Json("noncanonical command".into()));
    }
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
