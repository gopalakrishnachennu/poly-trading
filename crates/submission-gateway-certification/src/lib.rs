#![forbid(unsafe_code)]

//! Deterministic recorded-fixture submission-gateway certification.
//!
//! This crate has no credential, signature, resolver, socket, HTTP client,
//! authenticated transport, external submission, or mutation capability.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableSubmissionGateway,
    GatewayCheckpoint, GatewayRecovery, GatewayStorageError,
};
pub use report::{read_report, write_report_create_new, GatewayReportFileError};

use credential_broker_simulator::{
    simulated_receipt_chain_digest, BrokerCertificationReport, BrokerPlan, BrokerPolicy,
    BrokerReportStatus, SimulatedSigningReceipt,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use transport_adapter_certification::{
    TransportCertificateStatus, TransportCertificationPlan, TransportCertificationPolicy,
};

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct GatewayCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayPolicy {
    pub maximum_transport_age_ns: i64,
    pub maximum_broker_age_ns: i64,
    pub maximum_campaign_age_ns: i64,
    pub maximum_envelope_lifetime_ns: i64,
    pub maximum_backoff_ns: i64,
    pub maximum_envelopes: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowAuthenticationScheme {
    RecordedApiMac,
    RecordedSessionProof,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ShadowAuthenticationContract {
    pub scheme: ShadowAuthenticationScheme,
    pub channel_binding_digest: [u8; 32],
    pub token_binding_digest: [u8; 32],
    pub canonical_header_names: Vec<String>,
    pub credential_material_present: bool,
    pub authorization_header_values_present: bool,
    pub cookie_values_present: bool,
    pub signature_bytes_present: bool,
    pub provider_access_enabled: bool,
    pub socket_access_enabled: bool,
    pub external_submission_enabled: bool,
    pub contract_digest: [u8; 32],
}

impl ShadowAuthenticationContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.canonical_header_names.sort();
        self.contract_digest = digest_without(b"shadow-auth-contract-v1", &self, |value| {
            value.contract_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest
            == digest_without(b"shadow-auth-contract-v1", self, |value| {
                value.contract_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ShadowAuthenticatedEnvelope {
    pub sequence: u32,
    pub envelope_id: [u8; 32],
    pub broker_request_digest: [u8; 32],
    pub signing_receipt: SimulatedSigningReceipt,
    pub transport_binding_digest: [u8; 32],
    pub endpoint_policy_digest: [u8; 32],
    pub channel_binding_digest: [u8; 32],
    pub token_binding_digest: [u8; 32],
    pub idempotency_key_digest: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub simulated_only: bool,
    pub credential_material_present: bool,
    pub authorization_header_values_present: bool,
    pub signature_bytes_present: bool,
    pub external_submission_authority_granted: bool,
    pub envelope_digest: [u8; 32],
}

impl ShadowAuthenticatedEnvelope {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.envelope_digest =
            digest_without(b"shadow-authenticated-envelope-v1", &self, |value| {
                value.envelope_digest = [0; 32];
            });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.envelope_digest
            == digest_without(b"shadow-authenticated-envelope-v1", self, |value| {
                value.envelope_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayCertificationPlan {
    pub plan_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub transport_policy: TransportCertificationPolicy,
    pub transport_plan: TransportCertificationPlan,
    pub broker_policy: BrokerPolicy,
    pub broker_plan: BrokerPlan,
    pub broker_report: BrokerCertificationReport,
    pub authentication_contract: ShadowAuthenticationContract,
    pub envelopes: Vec<ShadowAuthenticatedEnvelope>,
    pub gateway_policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl GatewayCertificationPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &GatewayPolicy) -> Self {
        self.gateway_policy_digest = digest_json(b"submission-gateway-policy-v1", policy);
        self.plan_digest = digest_without(b"submission-gateway-plan-v1", &self, |value| {
            value.plan_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &GatewayPolicy) -> bool {
        self.gateway_policy_digest == digest_json(b"submission-gateway-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"submission-gateway-plan-v1", self, |value| {
                    value.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayFixtureCase {
    ValidEnvelope,
    WrongEndpoint,
    WrongChannelBinding,
    WrongTokenBinding,
    ReceiptReplay,
    IdempotencyConflict,
    ExpiredEnvelope,
    RateLimited,
    UnknownResponse,
    NoMutationReconciliation,
}

impl GatewayFixtureCase {
    pub const ALL: [Self; 10] = [
        Self::ValidEnvelope,
        Self::WrongEndpoint,
        Self::WrongChannelBinding,
        Self::WrongTokenBinding,
        Self::ReceiptReplay,
        Self::IdempotencyConflict,
        Self::ExpiredEnvelope,
        Self::RateLimited,
        Self::UnknownResponse,
        Self::NoMutationReconciliation,
    ];

    #[must_use]
    pub const fn expected(self) -> GatewayFixtureDisposition {
        match self {
            Self::ValidEnvelope => GatewayFixtureDisposition::ShadowAccepted,
            Self::RateLimited => GatewayFixtureDisposition::ManualBackoff,
            Self::UnknownResponse => GatewayFixtureDisposition::RequireReconciliation,
            Self::NoMutationReconciliation => GatewayFixtureDisposition::ReconciledNoMutation,
            Self::IdempotencyConflict => GatewayFixtureDisposition::FailClosed,
            Self::WrongEndpoint
            | Self::WrongChannelBinding
            | Self::WrongTokenBinding
            | Self::ReceiptReplay
            | Self::ExpiredEnvelope => GatewayFixtureDisposition::Deny,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayFixtureDisposition {
    ShadowAccepted,
    Deny,
    ManualBackoff,
    RequireReconciliation,
    ReconciledNoMutation,
    FailClosed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecordedGatewayFixture {
    pub sequence: u8,
    pub case: GatewayFixtureCase,
    pub expected: GatewayFixtureDisposition,
    pub observed: GatewayFixtureDisposition,
    pub observed_at_ns: i64,
    pub envelope_digest: [u8; 32],
    pub endpoint_policy_digest: [u8; 32],
    pub channel_binding_digest: [u8; 32],
    pub token_binding_digest: [u8; 32],
    pub idempotency_key_digest: [u8; 32],
    pub envelope_expires_at_ns: i64,
    pub backoff_ns: Option<i64>,
    pub ambiguity_digest: Option<[u8; 32]>,
    pub reconciliation_digest: Option<[u8; 32]>,
    pub fixture_source_digest: [u8; 32],
    pub recorded_fixture: bool,
    pub credential_loaded: bool,
    pub signature_produced: bool,
    pub socket_opened: bool,
    pub authenticated_request_sent: bool,
    pub external_submission_observed: bool,
    pub external_mutation_observed: bool,
    pub fixture_digest: [u8; 32],
}

impl RecordedGatewayFixture {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.fixture_digest = digest_without(b"recorded-gateway-fixture-v1", &self, |value| {
            value.fixture_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.fixture_digest
            == digest_without(b"recorded-gateway-fixture-v1", self, |value| {
                value.fixture_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowSubmissionState {
    Staged,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowSubmission {
    pub submission_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub envelope: ShadowAuthenticatedEnvelope,
    pub staged_at_ns: i64,
    pub state: ShadowSubmissionState,
    pub unknown_observation_digest: Option<[u8; 32]>,
    pub simulated_only: bool,
    pub submission_digest: [u8; 32],
}

impl ShadowSubmission {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.submission_digest
            == digest_without(b"shadow-submission-v1", self, |value| {
                value.submission_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedSubmissionOutcome {
    Accepted,
    Rejected,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecordedSubmissionObservation {
    pub observation_id: [u8; 32],
    pub submission_digest: [u8; 32],
    pub outcome: RecordedSubmissionOutcome,
    pub observed_at_ns: i64,
    pub source_digest: [u8; 32],
    pub recorded_fixture: bool,
    pub credential_loaded: bool,
    pub signature_produced: bool,
    pub socket_opened: bool,
    pub authenticated_request_sent: bool,
    pub external_submission_observed: bool,
    pub external_mutation_observed: bool,
    pub observation_digest: [u8; 32],
}

impl RecordedSubmissionObservation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.observation_digest =
            digest_without(b"recorded-submission-observation-v1", &self, |value| {
                value.observation_digest = [0; 32];
            });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.observation_digest
            == digest_without(b"recorded-submission-observation-v1", self, |value| {
                value.observation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecordedNoMutationEvidence {
    pub evidence_id: [u8; 32],
    pub submission_digest: [u8; 32],
    pub unknown_observation_digest: [u8; 32],
    pub external_state_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub recorded_fixture: bool,
    pub credential_loaded: bool,
    pub socket_opened: bool,
    pub authenticated_request_sent: bool,
    pub external_submission_observed: bool,
    pub external_mutation_observed: bool,
    pub evidence_digest: [u8; 32],
}

impl RecordedNoMutationEvidence {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.evidence_digest =
            digest_without(b"recorded-no-mutation-evidence-v1", &self, |value| {
                value.evidence_digest = [0; 32];
            });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evidence_digest
            == digest_without(b"recorded-no-mutation-evidence-v1", self, |value| {
                value.evidence_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayReportStatus {
    ShadowCertified,
    NotCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct GatewayCertificationReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub broker_report_digest: [u8; 32],
    pub transport_certificate_digest: [u8; 32],
    pub authentication_contract_digest: [u8; 32],
    pub channel_binding_digest: [u8; 32],
    pub token_binding_digest: [u8; 32],
    pub fixture_chain_digest: [u8; 32],
    pub submission_chain_digest: [u8; 32],
    pub completed_envelopes: usize,
    pub rejected_envelopes: usize,
    pub reconciled_unknowns: usize,
    pub finalized_at_ns: i64,
    pub status: GatewayReportStatus,
    pub credential_material_created: bool,
    pub signature_produced: bool,
    pub socket_opened: bool,
    pub authentication_authority_granted: bool,
    pub external_submission_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl GatewayCertificationReport {
    /// Seals non-authorizing gateway evidence for deterministic downstream use.
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"submission-gateway-report-v1", &self, |value| {
            value.report_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"submission-gateway-report-v1", self, |value| {
                value.report_digest = [0; 32];
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
pub enum GatewayCommand {
    Register {
        command_id: GatewayCommandId,
        plan: Box<GatewayCertificationPlan>,
        recorded_at_ns: i64,
    },
    RecordFixture {
        command_id: GatewayCommandId,
        fixture: Box<RecordedGatewayFixture>,
        recorded_at_ns: i64,
    },
    StageNext {
        command_id: GatewayCommandId,
        submission_id: [u8; 32],
        staged_at_ns: i64,
        recorded_at_ns: i64,
    },
    Observe {
        command_id: GatewayCommandId,
        submission: Box<ShadowSubmission>,
        observation: RecordedSubmissionObservation,
        recorded_at_ns: i64,
    },
    ReconcileUnknown {
        command_id: GatewayCommandId,
        submission: Box<ShadowSubmission>,
        evidence: RecordedNoMutationEvidence,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: GatewayCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl GatewayCommand {
    #[must_use]
    pub const fn command_id(&self) -> GatewayCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::RecordFixture { command_id, .. }
            | Self::StageNext { command_id, .. }
            | Self::Observe { command_id, .. }
            | Self::ReconcileUnknown { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::RecordFixture { recorded_at_ns, .. }
            | Self::StageNext { recorded_at_ns, .. }
            | Self::Observe { recorded_at_ns, .. }
            | Self::ReconcileUnknown { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum GatewayDetail {
    Registered,
    FixtureRecorded(GatewayFixtureCase),
    Staged(Box<ShadowSubmission>),
    Observed(RecordedSubmissionOutcome),
    UnknownReconciled,
    Finalized(Box<GatewayCertificationReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayOutcome {
    pub command_id: GatewayCommandId,
    pub detail: GatewayDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewaySnapshot {
    pub accepted_commands: u64,
    pub fixture_count: usize,
    pub completed_envelopes: usize,
    pub active_submission: Option<ShadowSubmission>,
    pub last_report: Option<GatewayCertificationReport>,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("submission-gateway policy is invalid")]
    Config,
    #[error("submission-gateway timestamp is invalid or regressed")]
    Timestamp,
    #[error("submission-gateway command exceeds its bound")]
    CommandBound,
    #[error("submission-gateway JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported submission-gateway command version: {0}")]
    Version(u16),
    #[error("submission-gateway command id conflict")]
    IdempotencyConflict,
    #[error("Phase 2.29 or Phase 2.30 upstream evidence is invalid")]
    Upstream,
    #[error("gateway plan or envelope binding is invalid")]
    Plan,
    #[error("recorded gateway fixture is invalid or out of order")]
    Fixture,
    #[error("shadow submission is invalid, expired, replayed, or out of order")]
    Submission,
    #[error("recorded submission observation is invalid")]
    Observation,
    #[error("unknown submission requires exact no-mutation reconciliation")]
    Reconciliation,
    #[error("submission-gateway finalization is invalid")]
    Finalize,
    #[error("submission-gateway arithmetic overflow")]
    Overflow,
    #[error("submission-gateway is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct SubmissionGatewayCertification {
    policy: GatewayPolicy,
    plan: Option<GatewayCertificationPlan>,
    fixtures: Vec<RecordedGatewayFixture>,
    fixture_chain_digest: [u8; 32],
    active_submission: Option<ShadowSubmission>,
    used_submission_ids: BTreeSet<[u8; 32]>,
    used_receipt_ids: BTreeSet<[u8; 32]>,
    used_idempotency_keys: BTreeSet<[u8; 32]>,
    used_observation_ids: BTreeSet<[u8; 32]>,
    used_reconciliation_ids: BTreeSet<[u8; 32]>,
    completed_envelopes: usize,
    rejected_envelopes: usize,
    reconciled_unknowns: usize,
    submission_chain_digest: [u8; 32],
    report: Option<GatewayCertificationReport>,
    processed: BTreeMap<GatewayCommandId, ([u8; 32], GatewayOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl SubmissionGatewayCertification {
    /// Creates one empty offline gateway certification owner.
    ///
    /// # Errors
    ///
    /// Rejects zero, excessive, or inconsistent policy bounds.
    pub fn new(policy: GatewayPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            fixtures: Vec::new(),
            fixture_chain_digest: [0; 32],
            active_submission: None,
            used_submission_ids: BTreeSet::new(),
            used_receipt_ids: BTreeSet::new(),
            used_idempotency_keys: BTreeSet::new(),
            used_observation_ids: BTreeSet::new(),
            used_reconciliation_ids: BTreeSet::new(),
            completed_envelopes: 0,
            rejected_envelopes: 0,
            reconciled_unknowns: 0,
            submission_chain_digest: [0; 32],
            report: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic gateway command.
    ///
    /// # Errors
    ///
    /// Integrity, chronology, replay, binding, or lifecycle failures halt.
    pub fn apply(&mut self, command: &GatewayCommand) -> Result<GatewayOutcome, Error> {
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
        let mut outcome = GatewayOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest =
            digest_without(b"submission-gateway-outcome-v1", &outcome, |value| {
                value.outcome_digest = [0; 32];
            });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &GatewayCommand) -> Result<GatewayDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            GatewayCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some() {
                    return Err(Error::Plan);
                }
                if !valid_upstream(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(GatewayDetail::Registered)
            }
            GatewayCommand::RecordFixture {
                fixture,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Fixture)?;
                let index = self.fixtures.len();
                if index >= GatewayFixtureCase::ALL.len()
                    || fixture.sequence as usize != index
                    || fixture.case != GatewayFixtureCase::ALL[index]
                    || fixture.expected != fixture.case.expected()
                    || fixture.observed != fixture.expected
                    || fixture.observed_at_ns < plan.created_at_ns
                    || fixture.observed_at_ns > plan.expires_at_ns
                    || fixture.observed_at_ns > *recorded_at_ns
                    || !valid_fixture(plan, &self.policy, fixture, self.fixtures.last())
                {
                    return Err(Error::Fixture);
                }
                self.fixture_chain_digest = chain_digest(
                    b"submission-gateway-fixture-chain-v1",
                    self.fixture_chain_digest,
                    fixture.fixture_digest,
                );
                self.fixtures.push((**fixture).clone());
                Ok(GatewayDetail::FixtureRecorded(fixture.case))
            }
            GatewayCommand::StageNext {
                submission_id,
                staged_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Submission)?;
                if self.fixtures.len() != GatewayFixtureCase::ALL.len()
                    || self.active_submission.is_some()
                    || self.completed_envelopes >= plan.envelopes.len()
                    || *submission_id == [0; 32]
                    || self.used_submission_ids.contains(submission_id)
                    || *staged_at_ns > *recorded_at_ns
                {
                    return Err(Error::Submission);
                }
                let envelope = &plan.envelopes[self.completed_envelopes];
                if *staged_at_ns < envelope.created_at_ns
                    || *staged_at_ns > envelope.expires_at_ns
                    || self
                        .used_receipt_ids
                        .contains(&envelope.signing_receipt.receipt_id)
                    || self
                        .used_idempotency_keys
                        .contains(&envelope.idempotency_key_digest)
                {
                    return Err(Error::Submission);
                }
                let mut submission = ShadowSubmission {
                    submission_id: *submission_id,
                    plan_digest: plan.plan_digest,
                    envelope: envelope.clone(),
                    staged_at_ns: *staged_at_ns,
                    state: ShadowSubmissionState::Staged,
                    unknown_observation_digest: None,
                    simulated_only: true,
                    submission_digest: [0; 32],
                };
                submission.submission_digest =
                    digest_without(b"shadow-submission-v1", &submission, |value| {
                        value.submission_digest = [0; 32];
                    });
                self.used_submission_ids.insert(*submission_id);
                self.used_receipt_ids
                    .insert(envelope.signing_receipt.receipt_id);
                self.used_idempotency_keys
                    .insert(envelope.idempotency_key_digest);
                self.active_submission = Some(submission.clone());
                Ok(GatewayDetail::Staged(Box::new(submission)))
            }
            GatewayCommand::Observe {
                submission,
                observation,
                recorded_at_ns,
                ..
            } => {
                let active = self.active_submission.as_ref().ok_or(Error::Observation)?;
                if **submission != *active
                    || !submission.verify_digest()
                    || submission.state != ShadowSubmissionState::Staged
                    || !valid_observation(observation, submission, *recorded_at_ns)
                    || self
                        .used_observation_ids
                        .contains(&observation.observation_id)
                {
                    return Err(Error::Observation);
                }
                self.used_observation_ids.insert(observation.observation_id);
                self.submission_chain_digest = chain_digest(
                    b"shadow-submission-observation-chain-v1",
                    self.submission_chain_digest,
                    observation.observation_digest,
                );
                match observation.outcome {
                    RecordedSubmissionOutcome::Accepted => {
                        self.complete_active(false, false)?;
                    }
                    RecordedSubmissionOutcome::Rejected => {
                        self.complete_active(true, false)?;
                    }
                    RecordedSubmissionOutcome::Unknown => {
                        let mut unknown = (**submission).clone();
                        unknown.state = ShadowSubmissionState::Unknown;
                        unknown.unknown_observation_digest = Some(observation.observation_digest);
                        unknown.submission_digest =
                            digest_without(b"shadow-submission-v1", &unknown, |value| {
                                value.submission_digest = [0; 32];
                            });
                        self.active_submission = Some(unknown);
                    }
                }
                Ok(GatewayDetail::Observed(observation.outcome))
            }
            GatewayCommand::ReconcileUnknown {
                submission,
                evidence,
                recorded_at_ns,
                ..
            } => {
                let active = self
                    .active_submission
                    .as_ref()
                    .ok_or(Error::Reconciliation)?;
                if **submission != *active
                    || !submission.verify_digest()
                    || submission.state != ShadowSubmissionState::Unknown
                    || !valid_reconciliation(evidence, submission, *recorded_at_ns)
                    || self.used_reconciliation_ids.contains(&evidence.evidence_id)
                {
                    return Err(Error::Reconciliation);
                }
                self.used_reconciliation_ids.insert(evidence.evidence_id);
                self.submission_chain_digest = chain_digest(
                    b"shadow-submission-reconciliation-chain-v1",
                    self.submission_chain_digest,
                    evidence.evidence_digest,
                );
                self.complete_active(false, true)?;
                Ok(GatewayDetail::UnknownReconciled)
            }
            GatewayCommand::Finalize {
                report_id,
                finalized_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                if *report_id == [0; 32]
                    || self.fixtures.len() != GatewayFixtureCase::ALL.len()
                    || self.completed_envelopes != plan.envelopes.len()
                    || self.active_submission.is_some()
                    || *finalized_at_ns < plan.created_at_ns
                    || *finalized_at_ns > *recorded_at_ns
                {
                    return Err(Error::Finalize);
                }
                let mut report = GatewayCertificationReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    broker_report_digest: plan.broker_report.report_digest,
                    transport_certificate_digest: plan
                        .broker_plan
                        .transport_certificate
                        .certificate_digest,
                    authentication_contract_digest: plan.authentication_contract.contract_digest,
                    channel_binding_digest: plan.authentication_contract.channel_binding_digest,
                    token_binding_digest: plan.authentication_contract.token_binding_digest,
                    fixture_chain_digest: self.fixture_chain_digest,
                    submission_chain_digest: self.submission_chain_digest,
                    completed_envelopes: self.completed_envelopes,
                    rejected_envelopes: self.rejected_envelopes,
                    reconciled_unknowns: self.reconciled_unknowns,
                    finalized_at_ns: *finalized_at_ns,
                    status: if self.rejected_envelopes == 0 {
                        GatewayReportStatus::ShadowCertified
                    } else {
                        GatewayReportStatus::NotCertified
                    },
                    credential_material_created: false,
                    signature_produced: false,
                    socket_opened: false,
                    authentication_authority_granted: false,
                    external_submission_authority_granted: false,
                    deployment_authority_granted: false,
                    report_digest: [0; 32],
                };
                report.report_digest =
                    digest_without(b"submission-gateway-report-v1", &report, |value| {
                        value.report_digest = [0; 32];
                    });
                self.report = Some(report.clone());
                Ok(GatewayDetail::Finalized(Box::new(report)))
            }
        }
    }

    fn complete_active(&mut self, rejected: bool, reconciled: bool) -> Result<(), Error> {
        self.completed_envelopes = self
            .completed_envelopes
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        if rejected {
            self.rejected_envelopes = self
                .rejected_envelopes
                .checked_add(1)
                .ok_or(Error::Overflow)?;
        }
        if reconciled {
            self.reconciled_unknowns = self
                .reconciled_unknowns
                .checked_add(1)
                .ok_or(Error::Overflow)?;
        }
        self.active_submission = None;
        Ok(())
    }

    #[must_use]
    pub fn snapshot(&self) -> GatewaySnapshot {
        GatewaySnapshot {
            accepted_commands: self.accepted_commands,
            fixture_count: self.fixtures.len(),
            completed_envelopes: self.completed_envelopes,
            active_submission: self.active_submission.clone(),
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
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"submission-gateway-certification-state-v1");
        hash_value(
            &mut hasher,
            &(
                &self.policy,
                &self.plan,
                &self.fixtures,
                self.fixture_chain_digest,
                &self.active_submission,
                &self.used_submission_ids,
                &self.used_receipt_ids,
                &self.used_idempotency_keys,
                &self.used_observation_ids,
                &self.used_reconciliation_ids,
                self.completed_envelopes,
                self.rejected_envelopes,
                self.reconciled_unknowns,
                self.submission_chain_digest,
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

fn validate_policy(policy: &GatewayPolicy) -> Result<(), Error> {
    if policy.maximum_transport_age_ns <= 0
        || policy.maximum_broker_age_ns <= 0
        || policy.maximum_campaign_age_ns <= 0
        || policy.maximum_envelope_lifetime_ns <= 0
        || policy.maximum_backoff_ns <= 0
        || policy.maximum_envelopes == 0
        || policy.maximum_envelopes > MAX_ITEMS
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn valid_upstream(plan: &GatewayCertificationPlan, policy: &GatewayPolicy, at: i64) -> bool {
    let certificate = &plan.broker_plan.transport_certificate;
    let report = &plan.broker_report;
    plan.transport_plan.verify_digest(&plan.transport_policy)
        && certificate.verify_digest()
        && certificate.status == TransportCertificateStatus::RecordedFixtureCertified
        && certificate.recorded_fixtures_only
        && certificate.plan_digest == plan.transport_plan.plan_digest
        && certificate.session_dossier_digest == plan.transport_plan.session_dossier.dossier_digest
        && certificate.endpoint_policy_digest == plan.transport_plan.endpoint_policy.policy_digest
        && !certificate.socket_authority_granted
        && !certificate.credential_material_created
        && !certificate.authentication_authority_granted
        && !certificate.external_submission_authority_granted
        && !certificate.deployment_authority_granted
        && plan.broker_plan.verify_digest(&plan.broker_policy)
        && report.verify_digest()
        && report.status == BrokerReportStatus::SimulationCompleted
        && report.plan_digest == plan.broker_plan.plan_digest
        && report.transport_certificate_digest == certificate.certificate_digest
        && report.key_descriptor_digest == plan.broker_plan.key_handle.descriptor_digest
        && report.finalized_at_ns >= plan.broker_plan.created_at_ns
        && !report.key_material_created
        && !report.real_signature_produced
        && !report.provider_contacted
        && !report.authentication_authority_granted
        && !report.external_submission_authority_granted
        && !report.deployment_authority_granted
        && at
            .checked_sub(certificate.certified_at_ns)
            .is_some_and(|age| age <= policy.maximum_transport_age_ns)
        && at
            .checked_sub(report.finalized_at_ns)
            .is_some_and(|age| age <= policy.maximum_broker_age_ns)
}

fn valid_plan(plan: &GatewayCertificationPlan, policy: &GatewayPolicy, at: i64) -> bool {
    let contract = &plan.authentication_contract;
    let receipt_chain: Vec<_> = plan
        .envelopes
        .iter()
        .map(|envelope| envelope.signing_receipt.clone())
        .collect();
    let envelope_ids: BTreeSet<_> = plan
        .envelopes
        .iter()
        .map(|value| value.envelope_id)
        .collect();
    let receipt_ids: BTreeSet<_> = plan
        .envelopes
        .iter()
        .map(|value| value.signing_receipt.receipt_id)
        .collect();
    let idempotency_keys: BTreeSet<_> = plan
        .envelopes
        .iter()
        .map(|value| value.idempotency_key_digest)
        .collect();
    plan.plan_id != [0; 32]
        && plan.verify_digest(policy)
        && valid_auth_contract(contract)
        && !plan.envelopes.is_empty()
        && plan.envelopes.len() <= policy.maximum_envelopes
        && plan.envelopes.len() == plan.broker_plan.requests.len()
        && plan.envelopes.len() == plan.transport_plan.request_bindings.len()
        && plan.envelopes.len() == plan.broker_report.completed_request_count
        && envelope_ids.len() == plan.envelopes.len()
        && receipt_ids.len() == plan.envelopes.len()
        && idempotency_keys.len() == plan.envelopes.len()
        && simulated_receipt_chain_digest(&receipt_chain) == plan.broker_report.receipt_chain_digest
        && plan.envelopes.iter().enumerate().all(|(index, envelope)| {
            valid_envelope(
                envelope,
                &plan.broker_plan.requests[index],
                &plan.transport_plan.request_bindings[index],
                &plan.transport_plan.endpoint_policy.policy_digest,
                contract,
                plan,
                policy,
                index,
            )
        })
        && plan.created_at_ns >= plan.broker_report.finalized_at_ns
        && plan.created_at_ns <= at
        && plan.expires_at_ns > plan.created_at_ns
        && plan
            .expires_at_ns
            .checked_sub(plan.created_at_ns)
            .is_some_and(|age| age <= policy.maximum_campaign_age_ns)
}

fn valid_auth_contract(contract: &ShadowAuthenticationContract) -> bool {
    contract.verify_digest()
        && contract.channel_binding_digest != [0; 32]
        && contract.token_binding_digest != [0; 32]
        && !contract.canonical_header_names.is_empty()
        && canonical(&contract.canonical_header_names)
        && contract.canonical_header_names.iter().all(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        && !contract.credential_material_present
        && !contract.authorization_header_values_present
        && !contract.cookie_values_present
        && !contract.signature_bytes_present
        && !contract.provider_access_enabled
        && !contract.socket_access_enabled
        && !contract.external_submission_enabled
}

#[allow(clippy::too_many_arguments)]
fn valid_envelope(
    envelope: &ShadowAuthenticatedEnvelope,
    request: &credential_broker_simulator::SigningRequest,
    binding: &transport_adapter_certification::CanonicalRequestBinding,
    endpoint_policy_digest: &[u8; 32],
    contract: &ShadowAuthenticationContract,
    plan: &GatewayCertificationPlan,
    policy: &GatewayPolicy,
    index: usize,
) -> bool {
    envelope.sequence as usize == index
        && envelope.verify_digest()
        && envelope.envelope_id != [0; 32]
        && envelope.broker_request_digest == request.request_digest
        && envelope.signing_receipt.verify_digest()
        && envelope.signing_receipt.request_digest == request.request_digest
        && envelope.signing_receipt.nonce_digest == request.nonce_digest
        && envelope.signing_receipt.simulated_only
        && !envelope.signing_receipt.signature_bytes_present
        && !envelope.signing_receipt.key_material_accessed
        && !envelope.signing_receipt.provider_contacted
        && !envelope.signing_receipt.authentication_authority_granted
        && !envelope
            .signing_receipt
            .external_submission_authority_granted
        && binding.sequence as usize == index
        && binding.verify_digest()
        && envelope.transport_binding_digest == binding.binding_digest
        && envelope.endpoint_policy_digest == *endpoint_policy_digest
        && envelope.channel_binding_digest == contract.channel_binding_digest
        && envelope.token_binding_digest == contract.token_binding_digest
        && envelope.idempotency_key_digest != [0; 32]
        && envelope.created_at_ns >= plan.created_at_ns
        && envelope.created_at_ns >= envelope.signing_receipt.consumed_at_ns
        && envelope.expires_at_ns > envelope.created_at_ns
        && envelope.expires_at_ns <= plan.expires_at_ns
        && envelope
            .expires_at_ns
            .checked_sub(envelope.created_at_ns)
            .is_some_and(|age| age <= policy.maximum_envelope_lifetime_ns)
        && envelope.simulated_only
        && !envelope.credential_material_present
        && !envelope.authorization_header_values_present
        && !envelope.signature_bytes_present
        && !envelope.external_submission_authority_granted
}

fn valid_fixture(
    plan: &GatewayCertificationPlan,
    policy: &GatewayPolicy,
    fixture: &RecordedGatewayFixture,
    prior: Option<&RecordedGatewayFixture>,
) -> bool {
    let envelope = &plan.envelopes[0];
    let common = fixture.verify_digest()
        && fixture.fixture_source_digest != [0; 32]
        && fixture.recorded_fixture
        && !fixture.credential_loaded
        && !fixture.signature_produced
        && !fixture.socket_opened
        && !fixture.authenticated_request_sent
        && !fixture.external_submission_observed
        && !fixture.external_mutation_observed;
    if !common {
        return false;
    }
    match fixture.case {
        GatewayFixtureCase::ValidEnvelope => {
            exact_fixture_subject(fixture, envelope)
                && fixture.envelope_expires_at_ns == envelope.expires_at_ns
        }
        GatewayFixtureCase::WrongEndpoint => {
            fixture.envelope_digest == envelope.envelope_digest
                && fixture.endpoint_policy_digest != envelope.endpoint_policy_digest
        }
        GatewayFixtureCase::WrongChannelBinding => {
            fixture.envelope_digest == envelope.envelope_digest
                && fixture.channel_binding_digest != envelope.channel_binding_digest
        }
        GatewayFixtureCase::WrongTokenBinding => {
            fixture.envelope_digest == envelope.envelope_digest
                && fixture.token_binding_digest != envelope.token_binding_digest
        }
        GatewayFixtureCase::ReceiptReplay => exact_fixture_subject(fixture, envelope),
        GatewayFixtureCase::IdempotencyConflict => {
            fixture.envelope_digest != envelope.envelope_digest
                && fixture.envelope_digest != [0; 32]
                && fixture.idempotency_key_digest == envelope.idempotency_key_digest
        }
        GatewayFixtureCase::ExpiredEnvelope => {
            exact_fixture_subject(fixture, envelope)
                && fixture.envelope_expires_at_ns >= plan.created_at_ns
                && fixture.observed_at_ns > fixture.envelope_expires_at_ns
        }
        GatewayFixtureCase::RateLimited => {
            exact_fixture_subject(fixture, envelope)
                && fixture.envelope_expires_at_ns == envelope.expires_at_ns
                && fixture
                    .backoff_ns
                    .is_some_and(|backoff| backoff > 0 && backoff <= policy.maximum_backoff_ns)
        }
        GatewayFixtureCase::UnknownResponse => {
            exact_fixture_subject(fixture, envelope)
                && fixture.envelope_expires_at_ns == envelope.expires_at_ns
                && fixture
                    .ambiguity_digest
                    .is_some_and(|value| value != [0; 32])
                && fixture.reconciliation_digest.is_none()
        }
        GatewayFixtureCase::NoMutationReconciliation => prior.is_some_and(|previous| {
            previous.case == GatewayFixtureCase::UnknownResponse
                && exact_fixture_subject(fixture, envelope)
                && fixture.envelope_expires_at_ns == envelope.expires_at_ns
                && fixture.ambiguity_digest == previous.ambiguity_digest
                && fixture
                    .reconciliation_digest
                    .is_some_and(|value| value != [0; 32])
        }),
    }
}

fn exact_fixture_subject(
    fixture: &RecordedGatewayFixture,
    envelope: &ShadowAuthenticatedEnvelope,
) -> bool {
    fixture.envelope_digest == envelope.envelope_digest
        && fixture.endpoint_policy_digest == envelope.endpoint_policy_digest
        && fixture.channel_binding_digest == envelope.channel_binding_digest
        && fixture.token_binding_digest == envelope.token_binding_digest
        && fixture.idempotency_key_digest == envelope.idempotency_key_digest
}

fn valid_observation(
    observation: &RecordedSubmissionObservation,
    submission: &ShadowSubmission,
    recorded_at_ns: i64,
) -> bool {
    observation.verify_digest()
        && observation.observation_id != [0; 32]
        && observation.submission_digest == submission.submission_digest
        && observation.observed_at_ns >= submission.staged_at_ns
        && observation.observed_at_ns <= recorded_at_ns
        && observation.source_digest != [0; 32]
        && observation.recorded_fixture
        && !observation.credential_loaded
        && !observation.signature_produced
        && !observation.socket_opened
        && !observation.authenticated_request_sent
        && !observation.external_submission_observed
        && !observation.external_mutation_observed
}

fn valid_reconciliation(
    evidence: &RecordedNoMutationEvidence,
    submission: &ShadowSubmission,
    recorded_at_ns: i64,
) -> bool {
    evidence.verify_digest()
        && evidence.evidence_id != [0; 32]
        && evidence.submission_digest == submission.submission_digest
        && submission
            .unknown_observation_digest
            .is_some_and(|digest| digest == evidence.unknown_observation_digest)
        && evidence.external_state_digest != [0; 32]
        && evidence.observed_at_ns >= submission.staged_at_ns
        && evidence.observed_at_ns <= recorded_at_ns
        && evidence.recorded_fixture
        && !evidence.credential_loaded
        && !evidence.socket_opened
        && !evidence.authenticated_request_sent
        && !evidence.external_submission_observed
        && !evidence.external_mutation_observed
}

fn canonical<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
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
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_value(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_value<T: Serialize + ?Sized>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable submission-gateway state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandWire {
    version: u16,
    command: GatewayCommand,
}

/// Encodes one bounded canonical gateway command.
///
/// # Errors
///
/// Rejects serialization failures and oversized commands.
pub fn encode_command(command: &GatewayCommand) -> Result<Vec<u8>, Error> {
    let bytes = serde_json::to_vec(&CommandWire {
        version: WIRE_VERSION,
        command: command.clone(),
    })
    .map_err(|error| Error::Json(error.to_string()))?;
    if bytes.len() > MAX_COMMAND_BYTES {
        Err(Error::CommandBound)
    } else {
        Ok(bytes)
    }
}

/// Decodes one bounded canonical gateway command.
///
/// # Errors
///
/// Rejects malformed, unsupported, trailing, noncanonical, or oversized input.
pub fn decode_command(bytes: &[u8]) -> Result<GatewayCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let wire = CommandWire::deserialize(&mut deserializer)
        .map_err(|error| Error::Json(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| Error::Json(error.to_string()))?;
    if wire.version != WIRE_VERSION {
        return Err(Error::Version(wire.version));
    }
    if serde_json::to_vec(&wire).map_err(|error| Error::Json(error.to_string()))? != bytes {
        return Err(Error::Json("noncanonical command".into()));
    }
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
