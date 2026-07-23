#![forbid(unsafe_code)]

//! Deterministic certification of recorded transport-adapter fixtures.
//!
//! This crate has no resolver, socket, TLS, HTTP, credential, or submission
//! implementation. It certifies sealed evidence only.

mod certificate;
mod durable;

pub use certificate::{
    read_certificate, write_certificate_create_new, TransportCertificateFileError,
};
pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableTransportCertification,
    TransportCheckpoint, TransportRecovery, TransportStorageError,
};

use executor_session_simulator::{ExecutorSessionDossier, SessionDossierStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct TransportCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportCertificationPolicy {
    pub maximum_dossier_age_ns: i64,
    pub maximum_campaign_age_ns: i64,
    pub maximum_backoff_ns: i64,
    pub maximum_endpoints: usize,
    pub maximum_pins: usize,
    pub maximum_bindings: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsMinimumVersion {
    Tls13,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct EndpointPolicy {
    pub hostname: String,
    pub port: u16,
    pub server_name: String,
    pub allowed_paths: Vec<String>,
    pub certificate_spki_pins: Vec<[u8; 32]>,
    pub resolver_policy_digest: [u8; 32],
    pub minimum_tls_version: TlsMinimumVersion,
    pub redirects_allowed: bool,
    pub proxy_allowed: bool,
    pub cookies_allowed: bool,
    pub authorization_headers_allowed: bool,
    pub query_credentials_allowed: bool,
    pub wildcard_identity_allowed: bool,
    pub policy_digest: [u8; 32],
}

impl EndpointPolicy {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.allowed_paths.sort();
        self.certificate_spki_pins.sort_unstable();
        self.policy_digest = digest_without(b"transport-endpoint-policy-v1", &self, |v| {
            v.policy_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.policy_digest
            == digest_without(b"transport-endpoint-policy-v1", self, |v| {
                v.policy_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalRequestBinding {
    pub sequence: u32,
    pub template_digest: [u8; 32],
    pub method: HttpMethod,
    pub path: String,
    pub body_digest: [u8; 32],
    pub canonical_bytes_digest: [u8; 32],
    pub binding_digest: [u8; 32],
}

impl CanonicalRequestBinding {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.binding_digest = digest_without(b"canonical-transport-request-v1", &self, |v| {
            v.binding_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.binding_digest
            == digest_without(b"canonical-transport-request-v1", self, |v| {
                v.binding_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportCertificationPlan {
    pub plan_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub session_dossier: ExecutorSessionDossier,
    pub endpoint_policy: EndpointPolicy,
    pub request_bindings: Vec<CanonicalRequestBinding>,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl TransportCertificationPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &TransportCertificationPolicy) -> Self {
        self.policy_digest = digest_json(b"transport-certification-policy-v1", policy);
        self.plan_digest = digest_without(b"transport-certification-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self, policy: &TransportCertificationPolicy) -> bool {
        self.policy_digest == digest_json(b"transport-certification-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"transport-certification-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportFixtureCase {
    DnsExact,
    DnsWrongHost,
    TlsPinned,
    TlsWrongPin,
    EndpointAllowed,
    EndpointForbidden,
    CanonicalRequest,
    NoncanonicalRequest,
    Timeout,
    RateLimited,
    UnknownResponse,
    NoMutationReconciliation,
}

impl TransportFixtureCase {
    pub const ALL: [Self; 12] = [
        Self::DnsExact,
        Self::DnsWrongHost,
        Self::TlsPinned,
        Self::TlsWrongPin,
        Self::EndpointAllowed,
        Self::EndpointForbidden,
        Self::CanonicalRequest,
        Self::NoncanonicalRequest,
        Self::Timeout,
        Self::RateLimited,
        Self::UnknownResponse,
        Self::NoMutationReconciliation,
    ];
    #[must_use]
    pub const fn expected(self) -> TransportDisposition {
        match self {
            Self::DnsExact | Self::TlsPinned | Self::EndpointAllowed | Self::CanonicalRequest => {
                TransportDisposition::AllowOfflineSerialization
            }
            Self::DnsWrongHost
            | Self::TlsWrongPin
            | Self::EndpointForbidden
            | Self::NoncanonicalRequest => TransportDisposition::Deny,
            Self::Timeout | Self::RateLimited => TransportDisposition::BackoffOnly,
            Self::UnknownResponse => TransportDisposition::ReconciliationRequired,
            Self::NoMutationReconciliation => TransportDisposition::NoMutationReconciled,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportDisposition {
    AllowOfflineSerialization,
    Deny,
    BackoffOnly,
    ReconciliationRequired,
    NoMutationReconciled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecordedTransportFixture {
    pub sequence: u8,
    pub case: TransportFixtureCase,
    pub expected: TransportDisposition,
    pub observed: TransportDisposition,
    pub observed_at_ns: i64,
    pub hostname: String,
    pub resolver_answer_digest: [u8; 32],
    pub server_name: String,
    pub presented_spki_digest: [u8; 32],
    pub path: String,
    pub serialized_request_digest: [u8; 32],
    pub status_code: Option<u16>,
    pub backoff_ns: Option<i64>,
    pub ambiguity_digest: Option<[u8; 32]>,
    pub reconciliation_digest: Option<[u8; 32]>,
    pub recorded_fixture: bool,
    pub socket_opened: bool,
    pub credential_loaded: bool,
    pub signature_produced: bool,
    pub authenticated_request_sent: bool,
    pub external_submission_observed: bool,
    pub external_mutation_observed: bool,
    pub fixture_source_digest: [u8; 32],
    pub fixture_digest: [u8; 32],
}

impl RecordedTransportFixture {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.fixture_digest = digest_without(b"recorded-transport-fixture-v1", &self, |v| {
            v.fixture_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.fixture_digest
            == digest_without(b"recorded-transport-fixture-v1", self, |v| {
                v.fixture_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportCertificateStatus {
    RecordedFixtureCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct TransportAdapterCertificate {
    pub certificate_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub session_dossier_digest: [u8; 32],
    pub endpoint_policy_digest: [u8; 32],
    pub fixture_chain_digest: [u8; 32],
    pub fixture_count: usize,
    pub certified_at_ns: i64,
    pub status: TransportCertificateStatus,
    pub recorded_fixtures_only: bool,
    pub socket_authority_granted: bool,
    pub credential_material_created: bool,
    pub authentication_authority_granted: bool,
    pub external_submission_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub certificate_digest: [u8; 32],
}

impl TransportAdapterCertificate {
    /// Seals recorded-fixture evidence for deterministic downstream use.
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.certificate_digest = digest_without(b"transport-adapter-certificate-v1", &self, |v| {
            v.certificate_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.certificate_digest
            == digest_without(b"transport-adapter-certificate-v1", self, |v| {
                v.certificate_digest = [0; 32];
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
pub enum TransportCommand {
    Register {
        command_id: TransportCommandId,
        plan: Box<TransportCertificationPlan>,
        recorded_at_ns: i64,
    },
    RecordFixture {
        command_id: TransportCommandId,
        fixture: Box<RecordedTransportFixture>,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: TransportCommandId,
        certificate_id: [u8; 32],
        certified_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl TransportCommand {
    #[must_use]
    pub const fn command_id(&self) -> TransportCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::RecordFixture { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::RecordFixture { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum TransportDetail {
    Registered,
    FixtureRecorded(TransportFixtureCase),
    Finalized(Box<TransportAdapterCertificate>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportOutcome {
    pub command_id: TransportCommandId,
    pub detail: TransportDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportSnapshot {
    pub accepted_commands: u64,
    pub fixture_count: usize,
    pub last_certificate: Option<TransportAdapterCertificate>,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("transport certification policy is invalid")]
    Config,
    #[error("transport certification timestamp is invalid or regressed")]
    Timestamp,
    #[error("transport command exceeds its canonical bound")]
    CommandBound,
    #[error("transport command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported transport command version: {0}")]
    Version(u16),
    #[error("transport command id conflict")]
    IdempotencyConflict,
    #[error("Phase 2.28 dossier is invalid, stale, substituted, or authority-bearing")]
    Upstream,
    #[error("transport endpoint, binding, or plan is invalid")]
    Plan,
    #[error("transport fixture is invalid, out of order, or side-effect-bearing")]
    Fixture,
    #[error("transport certification finalization is invalid")]
    Finalize,
    #[error("transport arithmetic overflow")]
    Overflow,
    #[error("transport certification is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct TransportAdapterCertification {
    policy: TransportCertificationPolicy,
    plan: Option<TransportCertificationPlan>,
    fixtures: Vec<RecordedTransportFixture>,
    fixture_chain_digest: [u8; 32],
    certificate: Option<TransportAdapterCertificate>,
    processed: BTreeMap<TransportCommandId, ([u8; 32], TransportOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl TransportAdapterCertification {
    /// Creates an empty recorded-fixture certifier.
    ///
    /// # Errors
    ///
    /// Rejects zero, excessive, or inconsistent policy bounds.
    pub fn new(policy: TransportCertificationPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            fixtures: Vec::new(),
            fixture_chain_digest: [0; 32],
            certificate: None,
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
    /// Integrity, chronology, evidence, or lifecycle failures halt.
    pub fn apply(&mut self, command: &TransportCommand) -> Result<TransportOutcome, Error> {
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
        let mut outcome = TransportOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest =
            digest_without(b"transport-certification-outcome-v1", &outcome, |v| {
                v.outcome_digest = [0; 32];
            });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    fn transition(&mut self, command: &TransportCommand) -> Result<TransportDetail, Error> {
        if self.certificate.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            TransportCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some() || !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(TransportDetail::Registered)
            }
            TransportCommand::RecordFixture {
                fixture,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Fixture)?;
                let index = self.fixtures.len();
                if index >= TransportFixtureCase::ALL.len()
                    || fixture.sequence as usize != index
                    || fixture.case != TransportFixtureCase::ALL[index]
                    || fixture.expected != fixture.case.expected()
                    || fixture.observed != fixture.expected
                    || fixture.observed_at_ns > *recorded_at_ns
                    || fixture.observed_at_ns < plan.created_at_ns
                    || !valid_fixture(fixture, plan, &self.policy)
                    || fixture.case == TransportFixtureCase::NoMutationReconciliation
                        && self
                            .fixtures
                            .last()
                            .and_then(|prior| prior.ambiguity_digest)
                            != fixture.ambiguity_digest
                {
                    return Err(Error::Fixture);
                }
                self.fixture_chain_digest =
                    chain_digest(self.fixture_chain_digest, fixture.fixture_digest);
                self.fixtures.push((**fixture).clone());
                Ok(TransportDetail::FixtureRecorded(fixture.case))
            }
            TransportCommand::Finalize {
                certificate_id,
                certified_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                if *certificate_id == [0; 32]
                    || self.fixtures.len() != TransportFixtureCase::ALL.len()
                    || *certified_at_ns > *recorded_at_ns
                    || *certified_at_ns > plan.expires_at_ns
                {
                    return Err(Error::Finalize);
                }
                let mut certificate = TransportAdapterCertificate {
                    certificate_id: *certificate_id,
                    plan_digest: plan.plan_digest,
                    session_dossier_digest: plan.session_dossier.dossier_digest,
                    endpoint_policy_digest: plan.endpoint_policy.policy_digest,
                    fixture_chain_digest: self.fixture_chain_digest,
                    fixture_count: self.fixtures.len(),
                    certified_at_ns: *certified_at_ns,
                    status: TransportCertificateStatus::RecordedFixtureCertified,
                    recorded_fixtures_only: true,
                    socket_authority_granted: false,
                    credential_material_created: false,
                    authentication_authority_granted: false,
                    external_submission_authority_granted: false,
                    deployment_authority_granted: false,
                    certificate_digest: [0; 32],
                };
                certificate.certificate_digest =
                    digest_without(b"transport-adapter-certificate-v1", &certificate, |v| {
                        v.certificate_digest = [0; 32];
                    });
                self.certificate = Some(certificate.clone());
                Ok(TransportDetail::Finalized(Box::new(certificate)))
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> TransportSnapshot {
        TransportSnapshot {
            accepted_commands: self.accepted_commands,
            fixture_count: self.fixtures.len(),
            last_certificate: self.certificate.clone(),
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
        h.update(b"transport-adapter-certification-state-v1");
        hash_value(
            &mut h,
            &(
                &self.policy,
                &self.plan,
                &self.fixtures,
                self.fixture_chain_digest,
                &self.certificate,
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

fn validate_policy(value: &TransportCertificationPolicy) -> Result<(), Error> {
    if value.maximum_dossier_age_ns <= 0
        || value.maximum_campaign_age_ns <= 0
        || value.maximum_backoff_ns <= 0
        || value.maximum_endpoints == 0
        || value.maximum_endpoints > MAX_ITEMS
        || value.maximum_pins == 0
        || value.maximum_pins > MAX_ITEMS
        || value.maximum_bindings == 0
        || value.maximum_bindings > MAX_ITEMS
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn valid_plan(
    plan: &TransportCertificationPlan,
    policy: &TransportCertificationPolicy,
    at: i64,
) -> bool {
    let dossier = &plan.session_dossier;
    let endpoint = &plan.endpoint_policy;
    plan.plan_id != [0; 32]
        && plan.verify_digest(policy)
        && dossier.verify_digest()
        && dossier.status == SessionDossierStatus::ProtocolSimulationCompleted
        && dossier.simulated_only
        && !dossier.credential_material_created
        && !dossier.signature_authority_granted
        && !dossier.authenticated_transport_granted
        && !dossier.external_submission_authority_granted
        && !dossier.deployment_authority_granted
        && dossier.resolved_request_count == dossier.request_template_digests.len()
        && valid_endpoint(endpoint, policy)
        && !plan.request_bindings.is_empty()
        && plan.request_bindings.len() <= policy.maximum_bindings
        && plan.request_bindings.len() == dossier.request_template_digests.len()
        && plan
            .request_bindings
            .iter()
            .zip(&dossier.request_template_digests)
            .enumerate()
            .all(|(index, (binding, template))| {
                binding.sequence as usize == index
                    && binding.template_digest == *template
                    && binding.verify_digest()
                    && binding.body_digest != [0; 32]
                    && binding.canonical_bytes_digest != [0; 32]
                    && endpoint.allowed_paths.binary_search(&binding.path).is_ok()
            })
        && plan.created_at_ns >= dossier.finalized_at_ns
        && plan.created_at_ns <= at
        && plan.expires_at_ns > plan.created_at_ns
        && at <= plan.expires_at_ns
        && plan
            .expires_at_ns
            .checked_sub(plan.created_at_ns)
            .is_some_and(|v| v <= policy.maximum_campaign_age_ns)
        && at
            .checked_sub(dossier.finalized_at_ns)
            .is_some_and(|v| v <= policy.maximum_dossier_age_ns)
}

fn valid_endpoint(value: &EndpointPolicy, policy: &TransportCertificationPolicy) -> bool {
    value.verify_digest()
        && !value.hostname.is_empty()
        && value.hostname.len() <= 253
        && value
            .hostname
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && !value.hostname.contains('*')
        && value.server_name == value.hostname
        && value.port == 443
        && value.resolver_policy_digest != [0; 32]
        && !value.allowed_paths.is_empty()
        && value.allowed_paths.len() <= policy.maximum_endpoints
        && canonical(&value.allowed_paths)
        && value.allowed_paths.iter().all(|path| valid_path(path))
        && !value.certificate_spki_pins.is_empty()
        && value.certificate_spki_pins.len() <= policy.maximum_pins
        && canonical(&value.certificate_spki_pins)
        && value
            .certificate_spki_pins
            .iter()
            .all(|pin| *pin != [0; 32])
        && !value.redirects_allowed
        && !value.proxy_allowed
        && !value.cookies_allowed
        && !value.authorization_headers_allowed
        && !value.query_credentials_allowed
        && !value.wildcard_identity_allowed
}

fn valid_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 512
        && !value.contains("..")
        && !value.contains('?')
        && !value.contains('#')
        && !value.contains('*')
}

fn valid_fixture(
    value: &RecordedTransportFixture,
    plan: &TransportCertificationPlan,
    policy: &TransportCertificationPolicy,
) -> bool {
    if !value.verify_digest()
        || value.fixture_source_digest == [0; 32]
        || !value.recorded_fixture
        || value.socket_opened
        || value.credential_loaded
        || value.signature_produced
        || value.authenticated_request_sent
        || value.external_submission_observed
        || value.external_mutation_observed
    {
        return false;
    }
    let endpoint = &plan.endpoint_policy;
    let binding = &plan.request_bindings[0];
    match value.case {
        TransportFixtureCase::DnsExact => {
            value.hostname == endpoint.hostname && value.resolver_answer_digest != [0; 32]
        }
        TransportFixtureCase::DnsWrongHost => {
            !value.hostname.is_empty() && value.hostname != endpoint.hostname
        }
        TransportFixtureCase::TlsPinned => {
            value.server_name == endpoint.server_name
                && endpoint
                    .certificate_spki_pins
                    .binary_search(&value.presented_spki_digest)
                    .is_ok()
        }
        TransportFixtureCase::TlsWrongPin => {
            value.server_name == endpoint.server_name
                && value.presented_spki_digest != [0; 32]
                && endpoint
                    .certificate_spki_pins
                    .binary_search(&value.presented_spki_digest)
                    .is_err()
        }
        TransportFixtureCase::EndpointAllowed => {
            endpoint.allowed_paths.binary_search(&value.path).is_ok()
        }
        TransportFixtureCase::EndpointForbidden => {
            !value.path.is_empty() && endpoint.allowed_paths.binary_search(&value.path).is_err()
        }
        TransportFixtureCase::CanonicalRequest => {
            value.serialized_request_digest == binding.canonical_bytes_digest
        }
        TransportFixtureCase::NoncanonicalRequest => {
            value.serialized_request_digest != [0; 32]
                && value.serialized_request_digest != binding.canonical_bytes_digest
        }
        TransportFixtureCase::Timeout => {
            value
                .backoff_ns
                .is_some_and(|v| v > 0 && v <= policy.maximum_backoff_ns)
                && value.status_code.is_none()
        }
        TransportFixtureCase::RateLimited => {
            value.status_code == Some(429)
                && value
                    .backoff_ns
                    .is_some_and(|v| v > 0 && v <= policy.maximum_backoff_ns)
        }
        TransportFixtureCase::UnknownResponse => {
            value.ambiguity_digest.is_some_and(|v| v != [0; 32])
                && value.reconciliation_digest.is_none()
        }
        TransportFixtureCase::NoMutationReconciliation => {
            value.ambiguity_digest.is_some_and(|v| v != [0; 32])
                && value.reconciliation_digest.is_some_and(|v| v != [0; 32])
        }
    }
}

fn canonical<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|v| v[0] < v[1])
}
fn chain_digest(prior: [u8; 32], next: [u8; 32]) -> [u8; 32] {
    digest_json(b"transport-fixture-chain-v1", &(prior, next))
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
    let bytes = serde_json::to_vec(value).expect("serializable transport certification state");
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandWire {
    version: u16,
    command: TransportCommand,
}

/// Encodes one bounded canonical command.
///
/// # Errors
///
/// Rejects serialization failure and oversized commands.
pub fn encode_command(command: &TransportCommand) -> Result<Vec<u8>, Error> {
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
pub fn decode_command(bytes: &[u8]) -> Result<TransportCommand, Error> {
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
