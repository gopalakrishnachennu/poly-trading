#![forbid(unsafe_code)]

//! Deterministic durable-infrastructure ports and offline certification.
//!
//! `PostgreSQL` is the only authoritative durable projection. Redpanda,
//! `ClickHouse` and Parquet-compatible archives are derived or replay surfaces.
//! This crate contains no credentials, network clients, sockets, or mutations.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableInfrastructureOwner,
    InfrastructureCheckpoint, InfrastructureRecovery, InfrastructureStorageError,
};
pub use report::{read_report, write_report_create_new, InfrastructureReportFileError};

use credential_provider_certification::{
    ProviderCertificationReport, ProviderReportStatus, ProviderScenario,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 512;
const MAX_ENDPOINT_URI_BYTES: usize = 2048;
const MAX_CONFIG_STRING_BYTES: usize = 256;

/// A reference to a durable backend.  This is intentionally only metadata:
/// the crate never resolves DNS, opens sockets, or loads the referenced
/// credential.  `credential_ref` is an identifier for a future secret
/// provider, never the secret itself.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendEndpoint {
    pub backend: BackendKind,
    pub uri: String,
    pub credential_ref: Option<String>,
}

/// Bounded deployment configuration for the durable infrastructure adapters.
/// This is a production *contract* and validation boundary, not a client
/// factory.  It remains safe to use in offline certification and CI.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductionInfrastructureConfig {
    pub config_id: [u8; 32],
    pub environment: String,
    pub region: String,
    pub endpoints: Vec<BackendEndpoint>,
    pub maximum_connections_per_backend: u32,
    pub request_timeout_ns: i64,
    pub archive_retention_days: u32,
    pub read_only: bool,
    pub order_submission_enabled: bool,
    pub config_digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProductionConfigError {
    #[error("production configuration field is invalid: {0}")]
    Field(&'static str),
    #[error("production configuration requires exactly one endpoint per backend")]
    EndpointSet,
    #[error("endpoint URI is invalid for {backend:?}")]
    EndpointScheme { backend: BackendKind },
    #[error("endpoint URI contains embedded credentials or query material")]
    EmbeddedCredential,
    #[error("endpoint URI is too long")]
    EndpointTooLong,
}

impl ProductionInfrastructureConfig {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.config_digest = digest_without(b"production-infrastructure-config-v1", &self, |v| {
            v.config_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.config_digest
            == digest_without(b"production-infrastructure-config-v1", self, |v| {
                v.config_digest = [0; 32];
            })
    }

    /// Validates limits and endpoint shape without contacting any service.
    ///
    /// # Errors
    ///
    /// Returns an error when the sealed configuration, field bounds, endpoint
    /// scheme, duplicate backend mapping or safety flags are invalid.
    pub fn validate(&self) -> Result<(), ProductionConfigError> {
        if self.config_id == [0; 32] || !self.verify_digest() {
            return Err(ProductionConfigError::Field("config_id or config_digest"));
        }
        for value in [&self.environment, &self.region] {
            if value.is_empty()
                || value.len() > MAX_CONFIG_STRING_BYTES
                || value.chars().any(char::is_whitespace)
            {
                return Err(ProductionConfigError::Field("environment/region"));
            }
        }
        if self.endpoints.len() != BackendKind::ALL.len()
            || !BackendKind::ALL.iter().all(|kind| {
                self.endpoints
                    .iter()
                    .filter(|value| value.backend == *kind)
                    .count()
                    == 1
            })
        {
            return Err(ProductionConfigError::EndpointSet);
        }
        if self.maximum_connections_per_backend == 0 || self.maximum_connections_per_backend > 4096
        {
            return Err(ProductionConfigError::Field(
                "maximum_connections_per_backend",
            ));
        }
        if self.request_timeout_ns <= 0 || self.request_timeout_ns > 300_000_000_000 {
            return Err(ProductionConfigError::Field("request_timeout_ns"));
        }
        if self.archive_retention_days == 0 || self.archive_retention_days > 36_500 {
            return Err(ProductionConfigError::Field("archive_retention_days"));
        }
        if !self.read_only || self.order_submission_enabled {
            return Err(ProductionConfigError::Field(
                "read_only/order_submission_enabled",
            ));
        }
        for endpoint in &self.endpoints {
            validate_endpoint(endpoint)?;
        }
        Ok(())
    }
}

fn validate_endpoint(endpoint: &BackendEndpoint) -> Result<(), ProductionConfigError> {
    if endpoint.uri.is_empty() || endpoint.uri.len() > MAX_ENDPOINT_URI_BYTES {
        return Err(ProductionConfigError::EndpointTooLong);
    }
    if endpoint
        .uri
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
        || endpoint.uri.contains('@')
        || endpoint.uri.contains('?')
        || endpoint.uri.contains('#')
    {
        return Err(ProductionConfigError::EmbeddedCredential);
    }
    if let Some(reference) = &endpoint.credential_ref {
        if reference.is_empty()
            || reference.len() > MAX_CONFIG_STRING_BYTES
            || reference.chars().any(char::is_whitespace)
        {
            return Err(ProductionConfigError::Field("credential_ref"));
        }
    }
    let (scheme, authority) = endpoint
        .uri
        .split_once("://")
        .map_or((None, ""), |(scheme, rest)| (Some(scheme), rest));
    if authority.is_empty() || authority.starts_with('/') {
        return Err(ProductionConfigError::EndpointScheme {
            backend: endpoint.backend,
        });
    }
    let valid = match endpoint.backend {
        // Explicit TLS schemes avoid silently accepting a plaintext default.
        BackendKind::PostgreSql => matches!(scheme, Some("postgresql+tls")),
        BackendKind::Redpanda => matches!(scheme, Some("kafka+tls")),
        BackendKind::ClickHouse => matches!(scheme, Some("https")),
        BackendKind::ParquetArchive => matches!(scheme, Some("s3" | "https")),
    };
    if !valid {
        return Err(ProductionConfigError::EndpointScheme {
            backend: endpoint.backend,
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct InfrastructureCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfrastructurePolicy {
    pub maximum_provider_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_observation_age_ns: i64,
    pub maximum_backoff_ns: i64,
    pub maximum_batch_bytes: u64,
    pub maximum_schema_epoch: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    PostgreSql,
    Redpanda,
    ClickHouse,
    ParquetArchive,
}

impl BackendKind {
    pub const ALL: [Self; 4] = [
        Self::PostgreSql,
        Self::Redpanda,
        Self::ClickHouse,
        Self::ParquetArchive,
    ];
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityClass {
    AuthoritativeLedgerProjection,
    OrderedEventDistribution,
    DerivedAnalytics,
    ImmutableReplayArchive,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct BackendContract {
    pub backend: BackendKind,
    pub authority: AuthorityClass,
    pub cluster_digest: [u8; 32],
    pub region_digest: [u8; 32],
    pub namespace_digest: [u8; 32],
    pub schema_digest: [u8; 32],
    pub initial_schema_epoch: u64,
    pub maximum_batch_bytes: u64,
    pub tls_required: bool,
    pub public_administration_allowed: bool,
    pub credential_embedded: bool,
    pub external_connection_enabled: bool,
    pub financial_fact_origination_allowed: bool,
    pub contract_digest: [u8; 32],
}

impl BackendContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.contract_digest = digest_without(b"durable-backend-contract-v1", &self, |v| {
            v.contract_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest
            == digest_without(b"durable-backend-contract-v1", self, |v| {
                v.contract_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableRecord {
    pub record_id: [u8; 32],
    pub idempotency_digest: [u8; 32],
    pub backend: BackendKind,
    pub sequence: u64,
    pub event_time_ns: i64,
    pub received_time_ns: i64,
    pub payload_digest: [u8; 32],
    pub previous_record_digest: [u8; 32],
    pub byte_length: u64,
    pub record_digest: [u8; 32],
}

impl DurableRecord {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.record_digest = digest_without(b"durable-record-v1", &self, |v| {
            v.record_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.record_digest
            == digest_without(b"durable-record-v1", self, |v| {
                v.record_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InfrastructureScenario {
    Commit,
    IdempotentReplay,
    IdempotencyConflict,
    SequenceGap,
    Backpressure,
    Corruption,
    MigrationForward,
    MigrationRollback,
    SnapshotRestore,
    ReplayConvergence,
}

impl InfrastructureScenario {
    pub const ALL: [Self; 10] = [
        Self::Commit,
        Self::IdempotentReplay,
        Self::IdempotencyConflict,
        Self::SequenceGap,
        Self::Backpressure,
        Self::Corruption,
        Self::MigrationForward,
        Self::MigrationRollback,
        Self::SnapshotRestore,
        Self::ReplayConvergence,
    ];
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InfrastructureDisposition {
    Commit,
    NoOp,
    Halt,
    Backoff,
    Migrate,
    Rollback,
    Restore,
    Converged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct InfrastructureObservation {
    pub observation_id: [u8; 32],
    pub backend: BackendKind,
    pub scenario: InfrastructureScenario,
    pub disposition: InfrastructureDisposition,
    pub contract_digest: [u8; 32],
    pub record: Option<DurableRecord>,
    pub prior_schema_digest: [u8; 32],
    pub resulting_schema_digest: [u8; 32],
    pub schema_epoch: u64,
    pub manifest_digest: [u8; 32],
    pub expected_state_digest: [u8; 32],
    pub observed_state_digest: [u8; 32],
    pub backoff_ns: i64,
    pub observed_at_ns: i64,
    pub isolated_fixture: bool,
    pub record_dropped: bool,
    pub automatic_retry_attempted: bool,
    pub credential_loaded: bool,
    pub socket_opened: bool,
    pub external_mutation_observed: bool,
    pub financial_authority_granted: bool,
    pub observation_digest: [u8; 32],
}

impl InfrastructureObservation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.observation_digest = digest_without(b"infrastructure-observation-v1", &self, |v| {
            v.observation_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.observation_digest
            == digest_without(b"infrastructure-observation-v1", self, |v| {
                v.observation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfrastructurePlan {
    pub plan_id: [u8; 32],
    pub provider_report: ProviderCertificationReport,
    pub contracts: Vec<BackendContract>,
    pub required_scenarios: Vec<InfrastructureScenario>,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl InfrastructurePlan {
    #[must_use]
    pub fn sealed(mut self, policy: &InfrastructurePolicy) -> Self {
        self.contracts.sort_by_key(|value| value.backend);
        self.required_scenarios.sort();
        self.policy_digest = digest_json(b"infrastructure-policy-v1", policy);
        self.plan_digest = digest_without(b"infrastructure-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &InfrastructurePolicy) -> bool {
        self.policy_digest == digest_json(b"infrastructure-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"infrastructure-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackendState {
    last_record: Option<DurableRecord>,
    schema_epoch: u64,
    schema_digest: [u8; 32],
    rollback_schema_digest: Option<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InfrastructureReportStatus {
    LocallyCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct InfrastructureReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub provider_report_digest: [u8; 32],
    pub covered_matrix: Vec<(BackendKind, InfrastructureScenario)>,
    pub terminal_state_digest: [u8; 32],
    pub finalized_at_ns: i64,
    pub status: InfrastructureReportStatus,
    pub external_environment_certified: bool,
    pub credential_material_created: bool,
    pub socket_opened: bool,
    pub external_mutation_observed: bool,
    pub financial_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub submission_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl InfrastructureReport {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"infrastructure-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"infrastructure-report-v1", self, |v| {
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
pub enum InfrastructureCommand {
    Register {
        command_id: InfrastructureCommandId,
        plan: Box<InfrastructurePlan>,
        recorded_at_ns: i64,
    },
    Observe {
        command_id: InfrastructureCommandId,
        observation: Box<InfrastructureObservation>,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: InfrastructureCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl InfrastructureCommand {
    #[must_use]
    pub const fn command_id(&self) -> InfrastructureCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::Observe { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::Observe { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum InfrastructureDetail {
    Registered,
    ObservationAccepted,
    Finalized(Box<InfrastructureReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InfrastructureOutcome {
    pub command_id: InfrastructureCommandId,
    pub detail: InfrastructureDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InfrastructureSnapshot {
    pub covered_matrix: BTreeSet<(BackendKind, InfrastructureScenario)>,
    pub accepted_commands: u64,
    pub report: Option<InfrastructureReport>,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("durable-infrastructure policy invalid")]
    Config,
    #[error("durable-infrastructure timestamp invalid or regressed")]
    Timestamp,
    #[error("durable-infrastructure command exceeds bound")]
    CommandBound,
    #[error("durable-infrastructure JSON invalid: {0}")]
    Json(String),
    #[error("unsupported durable-infrastructure command version: {0}")]
    Version(u16),
    #[error("durable-infrastructure idempotency conflict")]
    IdempotencyConflict,
    #[error("Phase 2.33 evidence invalid, stale, incomplete, or authority-bearing")]
    Upstream,
    #[error("durable-infrastructure plan or contract invalid")]
    Plan,
    #[error("durable-infrastructure observation invalid or side-effect-bearing")]
    Observation,
    #[error("durable-infrastructure state transition invalid")]
    Transition,
    #[error("durable-infrastructure finalization invalid")]
    Finalize,
    #[error("durable-infrastructure arithmetic overflow")]
    Overflow,
    #[error("durable-infrastructure owner halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct DurableInfrastructureCertification {
    policy: InfrastructurePolicy,
    plan: Option<InfrastructurePlan>,
    states: BTreeMap<BackendKind, BackendState>,
    covered: BTreeSet<(BackendKind, InfrastructureScenario)>,
    used_observations: BTreeSet<[u8; 32]>,
    processed: BTreeMap<InfrastructureCommandId, ([u8; 32], InfrastructureOutcome)>,
    report: Option<InfrastructureReport>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl DurableInfrastructureCertification {
    /// Creates an empty local infrastructure certification owner.
    ///
    /// # Errors
    ///
    /// Rejects zero or excessive policy bounds.
    pub fn new(policy: InfrastructurePolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            states: BTreeMap::new(),
            covered: BTreeSet::new(),
            used_observations: BTreeSet::new(),
            processed: BTreeMap::new(),
            report: None,
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic infrastructure certification command.
    ///
    /// # Errors
    ///
    /// Invalid chronology, evidence, ordering, migration, or identity halts.
    pub fn apply(
        &mut self,
        command: &InfrastructureCommand,
    ) -> Result<InfrastructureOutcome, Error> {
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
        let mut outcome = InfrastructureOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"infrastructure-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    fn transition(
        &mut self,
        command: &InfrastructureCommand,
    ) -> Result<InfrastructureDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            InfrastructureCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some()
                    || !valid_upstream(&plan.provider_report, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                for contract in &plan.contracts {
                    self.states.insert(
                        contract.backend,
                        BackendState {
                            last_record: None,
                            schema_epoch: contract.initial_schema_epoch,
                            schema_digest: contract.schema_digest,
                            rollback_schema_digest: None,
                        },
                    );
                }
                self.plan = Some((**plan).clone());
                Ok(InfrastructureDetail::Registered)
            }
            InfrastructureCommand::Observe {
                observation,
                recorded_at_ns,
                ..
            } => {
                self.observe(observation, *recorded_at_ns)?;
                Ok(InfrastructureDetail::ObservationAccepted)
            }
            InfrastructureCommand::Finalize {
                report_id,
                finalized_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                let complete = BackendKind::ALL.iter().all(|backend| {
                    plan.required_scenarios
                        .iter()
                        .all(|scenario| self.covered.contains(&(*backend, *scenario)))
                });
                if *report_id == [0; 32]
                    || !complete
                    || *finalized_at_ns < plan.created_at_ns
                    || *finalized_at_ns > *recorded_at_ns
                {
                    return Err(Error::Finalize);
                }
                let terminal_state_digest =
                    digest_json(b"infrastructure-terminal-state-v1", &self.states);
                let mut report = InfrastructureReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    provider_report_digest: plan.provider_report.report_digest,
                    covered_matrix: self.covered.iter().copied().collect(),
                    terminal_state_digest,
                    finalized_at_ns: *finalized_at_ns,
                    status: InfrastructureReportStatus::LocallyCertified,
                    external_environment_certified: false,
                    credential_material_created: false,
                    socket_opened: false,
                    external_mutation_observed: false,
                    financial_authority_granted: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    submission_authority_granted: false,
                    report_digest: [0; 32],
                };
                report.report_digest = digest_without(b"infrastructure-report-v1", &report, |v| {
                    v.report_digest = [0; 32];
                });
                self.report = Some(report.clone());
                Ok(InfrastructureDetail::Finalized(Box::new(report)))
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn observe(&mut self, value: &InfrastructureObservation, at: i64) -> Result<(), Error> {
        let plan = self.plan.as_ref().ok_or(Error::Observation)?;
        let contract = plan
            .contracts
            .iter()
            .find(|item| item.backend == value.backend)
            .ok_or(Error::Observation)?;
        if self.used_observations.contains(&value.observation_id)
            || !valid_observation(value, contract, &self.policy, plan, at)
        {
            return Err(Error::Observation);
        }
        let state = self
            .states
            .get_mut(&value.backend)
            .ok_or(Error::Transition)?;
        match value.scenario {
            InfrastructureScenario::Commit => {
                let record = value.record.as_ref().ok_or(Error::Transition)?;
                let expected_sequence = state
                    .last_record
                    .as_ref()
                    .map_or(0, |prior| prior.sequence.checked_add(1).unwrap_or(u64::MAX));
                let expected_previous = state
                    .last_record
                    .as_ref()
                    .map_or([0; 32], |prior| prior.record_digest);
                if value.disposition != InfrastructureDisposition::Commit
                    || record.sequence != expected_sequence
                    || record.previous_record_digest != expected_previous
                {
                    return Err(Error::Transition);
                }
                state.last_record = Some(record.clone());
            }
            InfrastructureScenario::IdempotentReplay => {
                if value.disposition != InfrastructureDisposition::NoOp
                    || value.record.as_ref() != state.last_record.as_ref()
                {
                    return Err(Error::Transition);
                }
            }
            InfrastructureScenario::IdempotencyConflict
            | InfrastructureScenario::SequenceGap
            | InfrastructureScenario::Corruption => {
                if value.disposition != InfrastructureDisposition::Halt || !value.isolated_fixture {
                    return Err(Error::Transition);
                }
            }
            InfrastructureScenario::Backpressure => {
                if value.disposition != InfrastructureDisposition::Backoff
                    || value.backoff_ns <= 0
                    || value.backoff_ns > self.policy.maximum_backoff_ns
                    || value.record_dropped
                    || value.automatic_retry_attempted
                {
                    return Err(Error::Transition);
                }
            }
            InfrastructureScenario::MigrationForward => {
                if value.disposition != InfrastructureDisposition::Migrate
                    || value.prior_schema_digest != state.schema_digest
                    || value.resulting_schema_digest == [0; 32]
                    || value.resulting_schema_digest == state.schema_digest
                    || value.schema_epoch
                        != state.schema_epoch.checked_add(1).ok_or(Error::Overflow)?
                    || value.schema_epoch > self.policy.maximum_schema_epoch
                {
                    return Err(Error::Transition);
                }
                state.rollback_schema_digest = Some(state.schema_digest);
                state.schema_digest = value.resulting_schema_digest;
                state.schema_epoch = value.schema_epoch;
            }
            InfrastructureScenario::MigrationRollback => {
                let prior = state.rollback_schema_digest.ok_or(Error::Transition)?;
                if value.disposition != InfrastructureDisposition::Rollback
                    || value.prior_schema_digest != state.schema_digest
                    || value.resulting_schema_digest != prior
                    || value.schema_epoch != state.schema_epoch
                {
                    return Err(Error::Transition);
                }
                state.schema_digest = prior;
                state.rollback_schema_digest = None;
            }
            InfrastructureScenario::SnapshotRestore => {
                if value.disposition != InfrastructureDisposition::Restore
                    || value.manifest_digest == [0; 32]
                    || value.expected_state_digest == [0; 32]
                    || value.expected_state_digest != value.observed_state_digest
                {
                    return Err(Error::Transition);
                }
            }
            InfrastructureScenario::ReplayConvergence => {
                if value.disposition != InfrastructureDisposition::Converged
                    || value.manifest_digest == [0; 32]
                    || value.expected_state_digest == [0; 32]
                    || value.expected_state_digest != value.observed_state_digest
                {
                    return Err(Error::Transition);
                }
            }
        }
        self.used_observations.insert(value.observation_id);
        self.covered.insert((value.backend, value.scenario));
        Ok(())
    }

    #[must_use]
    pub fn snapshot(&self) -> InfrastructureSnapshot {
        InfrastructureSnapshot {
            covered_matrix: self.covered.clone(),
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
        hasher.update(b"durable-infrastructure-state-v1");
        hash_value(
            &mut hasher,
            &(
                &self.policy,
                &self.plan,
                &self.states,
                &self.covered,
                &self.used_observations,
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

fn validate_policy(value: &InfrastructurePolicy) -> Result<(), Error> {
    if value.maximum_provider_report_age_ns <= 0
        || value.maximum_plan_lifetime_ns <= 0
        || value.maximum_observation_age_ns <= 0
        || value.maximum_backoff_ns <= 0
        || value.maximum_batch_bytes == 0
        || value.maximum_schema_epoch == 0
        || value.maximum_schema_epoch > u64::try_from(MAX_ITEMS).unwrap_or(u64::MAX)
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn valid_upstream(
    value: &ProviderCertificationReport,
    policy: &InfrastructurePolicy,
    at: i64,
) -> bool {
    value.verify_digest()
        && value.status == ProviderReportStatus::OfflineCertified
        && ProviderScenario::required()
            .iter()
            .all(|scenario| value.covered_scenarios.contains(scenario))
        && !value.key_material_created
        && !value.provider_credential_created
        && !value.signature_produced
        && !value.provider_contacted
        && !value.socket_opened
        && !value.external_mutation_observed
        && !value.signing_authority_granted
        && !value.submission_authority_granted
        && !value.deployment_authority_granted
        && !value.trading_authority_granted
        && at
            .checked_sub(value.finalized_at_ns)
            .is_some_and(|age| age <= policy.maximum_provider_report_age_ns)
}

fn valid_plan(value: &InfrastructurePlan, policy: &InfrastructurePolicy, at: i64) -> bool {
    value.verify_digest(policy)
        && value.plan_id != [0; 32]
        && value.contracts.len() == BackendKind::ALL.len()
        && value
            .contracts
            .iter()
            .map(|item| item.backend)
            .eq(BackendKind::ALL)
        && value.required_scenarios == InfrastructureScenario::ALL
        && value.created_at_ns <= at
        && value.expires_at_ns > at
        && value.expires_at_ns
            <= value
                .created_at_ns
                .checked_add(policy.maximum_plan_lifetime_ns)
                .unwrap_or(i64::MIN)
        && value
            .contracts
            .iter()
            .all(|contract| valid_contract(contract, policy))
}

fn valid_contract(value: &BackendContract, policy: &InfrastructurePolicy) -> bool {
    let authority = match value.backend {
        BackendKind::PostgreSql => AuthorityClass::AuthoritativeLedgerProjection,
        BackendKind::Redpanda => AuthorityClass::OrderedEventDistribution,
        BackendKind::ClickHouse => AuthorityClass::DerivedAnalytics,
        BackendKind::ParquetArchive => AuthorityClass::ImmutableReplayArchive,
    };
    value.verify_digest()
        && value.authority == authority
        && value.cluster_digest != [0; 32]
        && value.region_digest != [0; 32]
        && value.namespace_digest != [0; 32]
        && value.schema_digest != [0; 32]
        && value.initial_schema_epoch > 0
        && value.initial_schema_epoch <= policy.maximum_schema_epoch
        && value.maximum_batch_bytes > 0
        && value.maximum_batch_bytes <= policy.maximum_batch_bytes
        && value.tls_required
        && !value.public_administration_allowed
        && !value.credential_embedded
        && !value.external_connection_enabled
        && !value.financial_fact_origination_allowed
}

fn valid_observation(
    value: &InfrastructureObservation,
    contract: &BackendContract,
    policy: &InfrastructurePolicy,
    plan: &InfrastructurePlan,
    at: i64,
) -> bool {
    value.verify_digest()
        && value.observation_id != [0; 32]
        && value.contract_digest == contract.contract_digest
        && value.observed_at_ns >= plan.created_at_ns
        && value.observed_at_ns <= at
        && at
            .checked_sub(value.observed_at_ns)
            .is_some_and(|age| age <= policy.maximum_observation_age_ns)
        && !value.credential_loaded
        && !value.socket_opened
        && !value.external_mutation_observed
        && !value.financial_authority_granted
        && value
            .record
            .as_ref()
            .is_none_or(|record| valid_record(record, contract))
}

fn valid_record(value: &DurableRecord, contract: &BackendContract) -> bool {
    value.verify_digest()
        && value.record_id != [0; 32]
        && value.idempotency_digest != [0; 32]
        && value.backend == contract.backend
        && value.received_time_ns >= value.event_time_ns
        && value.payload_digest != [0; 32]
        && value.byte_length > 0
        && value.byte_length <= contract.maximum_batch_bytes
}

/// Encodes one bounded versioned command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &InfrastructureCommand) -> Result<Vec<u8>, Error> {
    let body = serde_json::to_vec(command).map_err(|error| Error::Json(error.to_string()))?;
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
/// Rejects size, version, JSON, unknown fields or trailing bytes.
pub fn decode_command(bytes: &[u8]) -> Result<InfrastructureCommand, Error> {
    if bytes.len() < 2 || bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let version = u16::from_le_bytes(bytes[..2].try_into().map_err(|_| Error::CommandBound)?);
    if version != WIRE_VERSION {
        return Err(Error::Version(version));
    }
    let mut decoder = serde_json::Deserializer::from_slice(&bytes[2..]);
    let command = InfrastructureCommand::deserialize(&mut decoder)
        .map_err(|error| Error::Json(error.to_string()))?;
    decoder
        .end()
        .map_err(|error| Error::Json(error.to_string()))?;
    Ok(command)
}

fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(&serde_json::to_vec(value).expect("serializable infrastructure state"));
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
fn hash_value<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable infrastructure state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[cfg(test)]
mod tests;
