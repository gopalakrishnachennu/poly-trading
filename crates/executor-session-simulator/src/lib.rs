#![forbid(unsafe_code)]

//! Deterministic offline executor-session protocol simulation.
//!
//! No type in this crate can load credentials, open a network connection,
//! authenticate, submit an external request, or mutate infrastructure.

mod dossier;
mod durable;

pub use dossier::{read_dossier, write_dossier_create_new, SessionDossierFileError};
pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableExecutorSession,
    ExecutorSessionCheckpoint, ExecutorSessionRecovery, ExecutorSessionStorageError,
};

use deployment_execution_intent::{
    DeploymentOperation, ExecutionCertificationReport, ExecutionIntentPlan, ExecutionIntentPolicy,
    ExecutionReportStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SessionCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorSessionPolicy {
    pub maximum_report_age_ns: i64,
    pub maximum_session_duration_ns: i64,
    pub maximum_lease_lifetime_ns: i64,
    pub maximum_heartbeat_gap_ns: i64,
    pub maximum_request_lifetime_ns: i64,
    pub maximum_requests: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ProcessIsolationContract {
    pub runtime_binary_digest: [u8; 32],
    pub sandbox_profile_digest: [u8; 32],
    pub audit_schema_digest: [u8; 32],
    pub network_access: bool,
    pub credential_access: bool,
    pub signing_access: bool,
    pub privileged_process: bool,
    pub arbitrary_shell: bool,
    pub filesystem_escape: bool,
    pub host_namespace_access: bool,
    pub isolation_digest: [u8; 32],
}

impl ProcessIsolationContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.isolation_digest = digest_without(b"executor-process-isolation-v1", &self, |v| {
            v.isolation_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.isolation_digest
            == digest_without(b"executor-process-isolation-v1", self, |v| {
                v.isolation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorRequestTemplate {
    pub sequence: u32,
    pub region: String,
    pub operation: DeploymentOperation,
    pub resource_digest: [u8; 32],
    pub payload_digest: [u8; 32],
    pub template_digest: [u8; 32],
}

impl ExecutorRequestTemplate {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.template_digest = digest_without(b"executor-request-template-v1", &self, |v| {
            v.template_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.template_digest
            == digest_without(b"executor-request-template-v1", self, |v| {
                v.template_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorSessionPlan {
    pub plan_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub upstream_policy: ExecutionIntentPolicy,
    pub execution_plan: ExecutionIntentPlan,
    pub execution_report: ExecutionCertificationReport,
    pub isolation_contract: ProcessIsolationContract,
    pub request_templates: Vec<ExecutorRequestTemplate>,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl ExecutorSessionPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &ExecutorSessionPolicy) -> Self {
        self.policy_digest = digest_json(b"executor-session-policy-v1", policy);
        self.plan_digest = digest_without(b"executor-session-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &ExecutorSessionPolicy) -> bool {
        self.policy_digest == digest_json(b"executor-session-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"executor-session-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLease {
    pub lease_id: [u8; 32],
    pub session_plan_digest: [u8; 32],
    pub owner_label_digest: [u8; 32],
    pub acquired_at_ns: i64,
    pub expires_at_ns: i64,
    pub lease_digest: [u8; 32],
}

impl SessionLease {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.lease_digest
            == digest_without(b"executor-session-lease-v1", self, |v| {
                v.lease_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ExecutorRequestEnvelope {
    pub request_id: [u8; 32],
    pub session_id: [u8; 32],
    pub process_instance_digest: [u8; 32],
    pub lease_digest: [u8; 32],
    pub session_plan_digest: [u8; 32],
    pub execution_report_digest: [u8; 32],
    pub executor_contract_digest: [u8; 32],
    pub template: ExecutorRequestTemplate,
    pub issued_at_ns: i64,
    pub expires_at_ns: i64,
    pub one_use: bool,
    pub simulated_only: bool,
    pub credential_material_present: bool,
    pub signature_present: bool,
    pub authenticated_transport: bool,
    pub external_submission_authority: bool,
    pub request_digest: [u8; 32],
}

impl ExecutorRequestEnvelope {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.request_digest
            == digest_without(b"executor-request-envelope-v1", self, |v| {
                v.request_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimulatedObservationKind {
    Acknowledged,
    Rejected,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct SimulatedExecutorObservation {
    pub request_id: [u8; 32],
    pub request_digest: [u8; 32],
    pub kind: SimulatedObservationKind,
    pub observed_at_ns: i64,
    pub source_fixture_digest: [u8; 32],
    pub simulated_only: bool,
    pub credential_loaded: bool,
    pub signature_produced: bool,
    pub authenticated_request_sent: bool,
    pub external_submission_observed: bool,
    pub external_mutation_observed: bool,
    pub observation_digest: [u8; 32],
}

impl SimulatedExecutorObservation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.observation_digest =
            digest_without(b"simulated-executor-observation-v1", &self, |v| {
                v.observation_digest = [0; 32];
            });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.observation_digest
            == digest_without(b"simulated-executor-observation-v1", self, |v| {
                v.observation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoMutationReconciliation {
    pub request_id: Option<[u8; 32]>,
    pub prior_state_digest: [u8; 32],
    pub durable_state_digest: [u8; 32],
    pub external_state_digest: [u8; 32],
    pub reconciled_at_ns: i64,
    pub no_external_mutation: bool,
    pub reconciliation_digest: [u8; 32],
}

impl NoMutationReconciliation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.reconciliation_digest =
            digest_without(b"executor-no-mutation-reconciliation-v1", &self, |v| {
                v.reconciliation_digest = [0; 32];
            });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.reconciliation_digest
            == digest_without(b"executor-no-mutation-reconciliation-v1", self, |v| {
                v.reconciliation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Registered,
    Open,
    ReconciliationRequired,
    RestartRecoveryRequired,
    Paused,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDossierStatus {
    ProtocolSimulationCompleted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ExecutorSessionDossier {
    pub dossier_id: [u8; 32],
    pub session_plan_digest: [u8; 32],
    pub execution_report_digest: [u8; 32],
    pub isolation_digest: [u8; 32],
    pub request_template_digests: Vec<[u8; 32]>,
    pub request_chain_digest: [u8; 32],
    pub reconciliation_chain_digest: [u8; 32],
    pub resolved_request_count: usize,
    pub finalized_at_ns: i64,
    pub status: SessionDossierStatus,
    pub simulated_only: bool,
    pub credential_material_created: bool,
    pub signature_authority_granted: bool,
    pub authenticated_transport_granted: bool,
    pub external_submission_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub dossier_digest: [u8; 32],
}

impl ExecutorSessionDossier {
    /// Seals non-executable session evidence for downstream verification.
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.dossier_digest = digest_without(b"executor-session-dossier-v2", &self, |v| {
            v.dossier_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.dossier_digest
            == digest_without(b"executor-session-dossier-v2", self, |v| {
                v.dossier_digest = [0; 32];
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
pub enum SessionCommand {
    Register {
        command_id: SessionCommandId,
        plan: Box<ExecutorSessionPlan>,
        recorded_at_ns: i64,
    },
    AcquireLease {
        command_id: SessionCommandId,
        lease_id: [u8; 32],
        owner_label_digest: [u8; 32],
        acquired_at_ns: i64,
        requested_expires_at_ns: i64,
        recorded_at_ns: i64,
    },
    OpenSession {
        command_id: SessionCommandId,
        session_id: [u8; 32],
        process_instance_digest: [u8; 32],
        opened_at_ns: i64,
        recorded_at_ns: i64,
    },
    Heartbeat {
        command_id: SessionCommandId,
        lease_id: [u8; 32],
        sequence: u64,
        observed_at_ns: i64,
        process_healthy: bool,
        journal_healthy: bool,
        reconciliation_healthy: bool,
        clock_healthy: bool,
        recorded_at_ns: i64,
    },
    IssueRequest {
        command_id: SessionCommandId,
        request_id: [u8; 32],
        issued_at_ns: i64,
        requested_expires_at_ns: i64,
        recorded_at_ns: i64,
    },
    Observe {
        command_id: SessionCommandId,
        observation: SimulatedExecutorObservation,
        recorded_at_ns: i64,
    },
    ExpireDeadMan {
        command_id: SessionCommandId,
        observed_at_ns: i64,
        recorded_at_ns: i64,
    },
    Restart {
        command_id: SessionCommandId,
        prior_state_digest: [u8; 32],
        restarted_at_ns: i64,
        recorded_at_ns: i64,
    },
    Reconcile {
        command_id: SessionCommandId,
        evidence: NoMutationReconciliation,
        recorded_at_ns: i64,
    },
    Close {
        command_id: SessionCommandId,
        closed_at_ns: i64,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: SessionCommandId,
        dossier_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl SessionCommand {
    #[must_use]
    pub const fn command_id(&self) -> SessionCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::AcquireLease { command_id, .. }
            | Self::OpenSession { command_id, .. }
            | Self::Heartbeat { command_id, .. }
            | Self::IssueRequest { command_id, .. }
            | Self::Observe { command_id, .. }
            | Self::ExpireDeadMan { command_id, .. }
            | Self::Restart { command_id, .. }
            | Self::Reconcile { command_id, .. }
            | Self::Close { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::AcquireLease { recorded_at_ns, .. }
            | Self::OpenSession { recorded_at_ns, .. }
            | Self::Heartbeat { recorded_at_ns, .. }
            | Self::IssueRequest { recorded_at_ns, .. }
            | Self::Observe { recorded_at_ns, .. }
            | Self::ExpireDeadMan { recorded_at_ns, .. }
            | Self::Restart { recorded_at_ns, .. }
            | Self::Reconcile { recorded_at_ns, .. }
            | Self::Close { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SessionDetail {
    Registered,
    LeaseAcquired(Box<SessionLease>),
    SessionOpened,
    HeartbeatAccepted(u64),
    RequestIssued(Box<ExecutorRequestEnvelope>),
    RequestResolved(SimulatedObservationKind),
    ReconciliationRequired,
    DeadManExpired,
    Restarted,
    Reconciled,
    Closed,
    Finalized(Box<ExecutorSessionDossier>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOutcome {
    pub command_id: SessionCommandId,
    pub detail: SessionDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSnapshot {
    pub accepted_commands: u64,
    pub status: Option<SessionStatus>,
    pub lease: Option<SessionLease>,
    pub active_request: Option<ExecutorRequestEnvelope>,
    pub resolved_requests: usize,
    pub last_dossier: Option<ExecutorSessionDossier>,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("executor-session policy is invalid")]
    Config,
    #[error("executor-session timestamp is invalid or regressed")]
    Timestamp,
    #[error("executor-session command exceeds its bound")]
    CommandBound,
    #[error("executor-session JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported executor-session command version: {0}")]
    Version(u16),
    #[error("executor-session command id conflict")]
    IdempotencyConflict,
    #[error("Phase 2.27 evidence is invalid, stale, substituted, or authority-bearing")]
    Upstream,
    #[error("executor-session isolation contract or plan is invalid")]
    Plan,
    #[error("executor-session lease is invalid, expired, or conflicting")]
    Lease,
    #[error("executor-session lifecycle is invalid")]
    Session,
    #[error("executor request is invalid, expired, replayed, or out of order")]
    Request,
    #[error("executor observation is invalid or claims an external side effect")]
    Observation,
    #[error("executor-session reconciliation is invalid")]
    Reconciliation,
    #[error("executor-session finalization is invalid")]
    Finalize,
    #[error("executor-session arithmetic overflow")]
    Overflow,
    #[error("executor-session owner is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct ExecutorSessionSimulator {
    policy: ExecutorSessionPolicy,
    plan: Option<ExecutorSessionPlan>,
    status: Option<SessionStatus>,
    lease: Option<SessionLease>,
    used_lease_ids: BTreeSet<[u8; 32]>,
    session_id: Option<[u8; 32]>,
    process_instance_digest: Option<[u8; 32]>,
    heartbeat_sequence: Option<u64>,
    last_heartbeat_at_ns: Option<i64>,
    active_request: Option<ExecutorRequestEnvelope>,
    used_request_ids: BTreeSet<[u8; 32]>,
    resolved_requests: usize,
    request_chain_digest: [u8; 32],
    reconciliation_chain_digest: [u8; 32],
    recovery_prior_digest: Option<[u8; 32]>,
    dossier: Option<ExecutorSessionDossier>,
    processed: BTreeMap<SessionCommandId, ([u8; 32], SessionOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl ExecutorSessionSimulator {
    /// Creates an empty offline executor-session simulator.
    ///
    /// # Errors
    ///
    /// Rejects zero, excessive, or inconsistent bounds.
    pub fn new(policy: ExecutorSessionPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            status: None,
            lease: None,
            used_lease_ids: BTreeSet::new(),
            session_id: None,
            process_instance_digest: None,
            heartbeat_sequence: None,
            last_heartbeat_at_ns: None,
            active_request: None,
            used_request_ids: BTreeSet::new(),
            resolved_requests: 0,
            request_chain_digest: [0; 32],
            reconciliation_chain_digest: [0; 32],
            recovery_prior_digest: None,
            dossier: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic command transactionally.
    ///
    /// # Errors
    ///
    /// Integrity, chronology, lease, lifecycle, or side-effect failures halt.
    pub fn apply(&mut self, command: &SessionCommand) -> Result<SessionOutcome, Error> {
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
        let mut outcome = SessionOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"executor-session-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &SessionCommand) -> Result<SessionDetail, Error> {
        if self.dossier.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            SessionCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some() || !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                self.status = Some(SessionStatus::Registered);
                Ok(SessionDetail::Registered)
            }
            SessionCommand::AcquireLease {
                lease_id,
                owner_label_digest,
                acquired_at_ns,
                requested_expires_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Lease)?;
                if self.lease.is_some()
                    || !matches!(
                        self.status,
                        Some(SessionStatus::Registered | SessionStatus::Paused)
                    )
                    || *lease_id == [0; 32]
                    || self.used_lease_ids.contains(lease_id)
                    || *owner_label_digest == [0; 32]
                    || *acquired_at_ns > *recorded_at_ns
                    || *requested_expires_at_ns <= *acquired_at_ns
                    || *requested_expires_at_ns > plan.expires_at_ns
                {
                    return Err(Error::Lease);
                }
                let maximum = acquired_at_ns
                    .checked_add(self.policy.maximum_lease_lifetime_ns)
                    .ok_or(Error::Overflow)?;
                if *requested_expires_at_ns > maximum {
                    return Err(Error::Lease);
                }
                let mut lease = SessionLease {
                    lease_id: *lease_id,
                    session_plan_digest: plan.plan_digest,
                    owner_label_digest: *owner_label_digest,
                    acquired_at_ns: *acquired_at_ns,
                    expires_at_ns: *requested_expires_at_ns,
                    lease_digest: [0; 32],
                };
                lease.lease_digest = digest_without(b"executor-session-lease-v1", &lease, |v| {
                    v.lease_digest = [0; 32];
                });
                self.lease = Some(lease.clone());
                self.used_lease_ids.insert(*lease_id);
                self.heartbeat_sequence = None;
                self.last_heartbeat_at_ns = None;
                if self.session_id.is_some() {
                    self.status = Some(SessionStatus::Open);
                }
                Ok(SessionDetail::LeaseAcquired(Box::new(lease)))
            }
            SessionCommand::OpenSession {
                session_id,
                process_instance_digest,
                opened_at_ns,
                recorded_at_ns,
                ..
            } => {
                if self.status != Some(SessionStatus::Registered)
                    || self.session_id.is_some()
                    || *session_id == [0; 32]
                    || *process_instance_digest == [0; 32]
                    || *opened_at_ns > *recorded_at_ns
                    || !self.live_lease(*opened_at_ns)
                {
                    return Err(Error::Session);
                }
                self.session_id = Some(*session_id);
                self.process_instance_digest = Some(*process_instance_digest);
                self.status = Some(SessionStatus::Open);
                Ok(SessionDetail::SessionOpened)
            }
            SessionCommand::Heartbeat {
                lease_id,
                sequence,
                observed_at_ns,
                process_healthy,
                journal_healthy,
                reconciliation_healthy,
                clock_healthy,
                recorded_at_ns,
                ..
            } => {
                let lease = self.lease.as_ref().ok_or(Error::Lease)?;
                if lease.lease_id != *lease_id
                    || *observed_at_ns > *recorded_at_ns
                    || !self.live_lease(*observed_at_ns)
                    || self
                        .heartbeat_sequence
                        .is_some_and(|v| v.checked_add(1) != Some(*sequence))
                    || self.heartbeat_sequence.is_none() && *sequence != 0
                    || self.last_heartbeat_at_ns.is_some_and(|v| {
                        observed_at_ns
                            .checked_sub(v)
                            .is_none_or(|gap| gap > self.policy.maximum_heartbeat_gap_ns)
                    })
                {
                    return Err(Error::Lease);
                }
                self.heartbeat_sequence = Some(*sequence);
                self.last_heartbeat_at_ns = Some(*observed_at_ns);
                if !(*process_healthy
                    && *journal_healthy
                    && *reconciliation_healthy
                    && *clock_healthy)
                {
                    self.lease = None;
                    self.status = Some(if self.active_request.is_some() {
                        SessionStatus::ReconciliationRequired
                    } else {
                        SessionStatus::Paused
                    });
                    return Ok(SessionDetail::ReconciliationRequired);
                }
                Ok(SessionDetail::HeartbeatAccepted(*sequence))
            }
            SessionCommand::IssueRequest {
                request_id,
                issued_at_ns,
                requested_expires_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Request)?;
                let lease = self.lease.as_ref().ok_or(Error::Request)?;
                if self.status != Some(SessionStatus::Open)
                    || self.active_request.is_some()
                    || self.resolved_requests >= plan.request_templates.len()
                    || *request_id == [0; 32]
                    || self.used_request_ids.contains(request_id)
                    || *issued_at_ns > *recorded_at_ns
                    || !self.live_lease(*issued_at_ns)
                    || self.heartbeat_sequence.is_none()
                    || self.last_heartbeat_at_ns.is_none_or(|last| {
                        issued_at_ns
                            .checked_sub(last)
                            .is_none_or(|gap| gap > self.policy.maximum_heartbeat_gap_ns)
                    })
                    || *requested_expires_at_ns <= *issued_at_ns
                    || *requested_expires_at_ns > lease.expires_at_ns
                    || *requested_expires_at_ns > plan.expires_at_ns
                {
                    return Err(Error::Request);
                }
                let maximum = issued_at_ns
                    .checked_add(self.policy.maximum_request_lifetime_ns)
                    .ok_or(Error::Overflow)?;
                if *requested_expires_at_ns > maximum {
                    return Err(Error::Request);
                }
                let mut request = ExecutorRequestEnvelope {
                    request_id: *request_id,
                    session_id: self.session_id.ok_or(Error::Session)?,
                    process_instance_digest: self.process_instance_digest.ok_or(Error::Session)?,
                    lease_digest: lease.lease_digest,
                    session_plan_digest: plan.plan_digest,
                    execution_report_digest: plan.execution_report.report_digest,
                    executor_contract_digest: plan.execution_plan.executor_contract.contract_digest,
                    template: plan.request_templates[self.resolved_requests].clone(),
                    issued_at_ns: *issued_at_ns,
                    expires_at_ns: *requested_expires_at_ns,
                    one_use: true,
                    simulated_only: true,
                    credential_material_present: false,
                    signature_present: false,
                    authenticated_transport: false,
                    external_submission_authority: false,
                    request_digest: [0; 32],
                };
                request.request_digest =
                    digest_without(b"executor-request-envelope-v1", &request, |v| {
                        v.request_digest = [0; 32];
                    });
                self.used_request_ids.insert(*request_id);
                self.active_request = Some(request.clone());
                Ok(SessionDetail::RequestIssued(Box::new(request)))
            }
            SessionCommand::Observe {
                observation,
                recorded_at_ns,
                ..
            } => {
                let request = self.active_request.as_ref().ok_or(Error::Observation)?;
                if !valid_observation(observation, request, *recorded_at_ns) {
                    return Err(Error::Observation);
                }
                self.request_chain_digest = chain_digest(
                    b"executor-request-chain-v1",
                    self.request_chain_digest,
                    observation.observation_digest,
                );
                if observation.kind == SimulatedObservationKind::Unknown {
                    self.lease = None;
                    self.status = Some(SessionStatus::ReconciliationRequired);
                    Ok(SessionDetail::ReconciliationRequired)
                } else {
                    self.active_request = None;
                    self.resolved_requests = self
                        .resolved_requests
                        .checked_add(1)
                        .ok_or(Error::Overflow)?;
                    Ok(SessionDetail::RequestResolved(observation.kind))
                }
            }
            SessionCommand::ExpireDeadMan {
                observed_at_ns,
                recorded_at_ns,
                ..
            } => {
                let lease = self.lease.as_ref().ok_or(Error::Lease)?;
                if *observed_at_ns > *recorded_at_ns || *observed_at_ns < lease.expires_at_ns {
                    return Err(Error::Lease);
                }
                self.lease = None;
                self.status = Some(if self.active_request.is_some() {
                    SessionStatus::ReconciliationRequired
                } else {
                    SessionStatus::Paused
                });
                Ok(SessionDetail::DeadManExpired)
            }
            SessionCommand::Restart {
                prior_state_digest,
                restarted_at_ns,
                recorded_at_ns,
                ..
            } => {
                if *prior_state_digest != self.state_digest()
                    || *restarted_at_ns > *recorded_at_ns
                    || !matches!(
                        self.status,
                        Some(
                            SessionStatus::Open
                                | SessionStatus::Paused
                                | SessionStatus::ReconciliationRequired
                        )
                    )
                {
                    return Err(Error::Session);
                }
                self.recovery_prior_digest = Some(*prior_state_digest);
                self.lease = None;
                self.status = Some(SessionStatus::RestartRecoveryRequired);
                Ok(SessionDetail::Restarted)
            }
            SessionCommand::Reconcile {
                evidence,
                recorded_at_ns,
                ..
            } => {
                let recovery = self.status == Some(SessionStatus::RestartRecoveryRequired);
                let uncertain = self.status == Some(SessionStatus::ReconciliationRequired);
                if !(recovery || uncertain)
                    || !evidence.verify_digest()
                    || !evidence.no_external_mutation
                    || evidence.reconciled_at_ns > *recorded_at_ns
                    || evidence.durable_state_digest == [0; 32]
                    || evidence.external_state_digest == [0; 32]
                    || recovery && Some(evidence.prior_state_digest) != self.recovery_prior_digest
                    || evidence.request_id != self.active_request.as_ref().map(|v| v.request_id)
                {
                    return Err(Error::Reconciliation);
                }
                self.reconciliation_chain_digest = chain_digest(
                    b"executor-reconciliation-chain-v1",
                    self.reconciliation_chain_digest,
                    evidence.reconciliation_digest,
                );
                if self.active_request.is_some() {
                    self.active_request = None;
                    self.resolved_requests = self
                        .resolved_requests
                        .checked_add(1)
                        .ok_or(Error::Overflow)?;
                }
                self.recovery_prior_digest = None;
                self.status = Some(SessionStatus::Paused);
                Ok(SessionDetail::Reconciled)
            }
            SessionCommand::Close {
                closed_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Session)?;
                if self.active_request.is_some()
                    || self.resolved_requests != plan.request_templates.len()
                    || *closed_at_ns > *recorded_at_ns
                    || !matches!(
                        self.status,
                        Some(SessionStatus::Open | SessionStatus::Paused)
                    )
                {
                    return Err(Error::Session);
                }
                self.lease = None;
                self.status = Some(SessionStatus::Closed);
                Ok(SessionDetail::Closed)
            }
            SessionCommand::Finalize {
                dossier_id,
                finalized_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                if *dossier_id == [0; 32]
                    || self.status != Some(SessionStatus::Closed)
                    || self.lease.is_some()
                    || self.active_request.is_some()
                    || self.resolved_requests != plan.request_templates.len()
                    || *finalized_at_ns > *recorded_at_ns
                {
                    return Err(Error::Finalize);
                }
                let mut dossier = ExecutorSessionDossier {
                    dossier_id: *dossier_id,
                    session_plan_digest: plan.plan_digest,
                    execution_report_digest: plan.execution_report.report_digest,
                    isolation_digest: plan.isolation_contract.isolation_digest,
                    request_template_digests: plan
                        .request_templates
                        .iter()
                        .map(|template| template.template_digest)
                        .collect(),
                    request_chain_digest: self.request_chain_digest,
                    reconciliation_chain_digest: self.reconciliation_chain_digest,
                    resolved_request_count: self.resolved_requests,
                    finalized_at_ns: *finalized_at_ns,
                    status: SessionDossierStatus::ProtocolSimulationCompleted,
                    simulated_only: true,
                    credential_material_created: false,
                    signature_authority_granted: false,
                    authenticated_transport_granted: false,
                    external_submission_authority_granted: false,
                    deployment_authority_granted: false,
                    dossier_digest: [0; 32],
                };
                dossier.dossier_digest =
                    digest_without(b"executor-session-dossier-v2", &dossier, |v| {
                        v.dossier_digest = [0; 32];
                    });
                self.dossier = Some(dossier.clone());
                Ok(SessionDetail::Finalized(Box::new(dossier)))
            }
        }
    }

    fn live_lease(&self, at: i64) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|v| at >= v.acquired_at_ns && at < v.expires_at_ns && v.verify_digest())
    }
    #[must_use]
    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            accepted_commands: self.accepted_commands,
            status: self.status,
            lease: self.lease.clone(),
            active_request: self.active_request.clone(),
            resolved_requests: self.resolved_requests,
            last_dossier: self.dossier.clone(),
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
        hasher.update(b"executor-session-state-v1");
        hash_value(
            &mut hasher,
            &(
                &self.policy,
                &self.plan,
                self.status,
                &self.lease,
                &self.used_lease_ids,
                self.session_id,
                self.process_instance_digest,
                self.heartbeat_sequence,
                self.last_heartbeat_at_ns,
                &self.active_request,
                &self.used_request_ids,
                self.resolved_requests,
                self.request_chain_digest,
                self.reconciliation_chain_digest,
                self.recovery_prior_digest,
                &self.dossier,
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

fn validate_policy(policy: &ExecutorSessionPolicy) -> Result<(), Error> {
    if policy.maximum_report_age_ns <= 0
        || policy.maximum_session_duration_ns <= 0
        || policy.maximum_lease_lifetime_ns <= 0
        || policy.maximum_heartbeat_gap_ns <= 0
        || policy.maximum_request_lifetime_ns <= 0
        || policy.maximum_requests == 0
        || policy.maximum_requests > MAX_ITEMS
        || policy.maximum_request_lifetime_ns > policy.maximum_lease_lifetime_ns
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn valid_plan(plan: &ExecutorSessionPlan, policy: &ExecutorSessionPolicy, at: i64) -> bool {
    let upstream = &plan.execution_plan;
    let report = &plan.execution_report;
    let isolation = &plan.isolation_contract;
    plan.plan_id != [0; 32]
        && plan.verify_digest(policy)
        && upstream.verify_digest(&plan.upstream_policy)
        && report.verify_digest()
        && report.status == ExecutionReportStatus::SimulatedHandoffsCompleted
        && report.plan_digest == upstream.plan_digest
        && report.readiness_record_digest == upstream.readiness_record.record_digest
        && report.subject_digest == upstream.subject.subject_digest
        && report.contract_digest == upstream.executor_contract.contract_digest
        && report.completed_step_count == upstream.steps.len()
        && report.manual_execution_still_required
        && !report.credential_material_created
        && !report.signature_authority_granted
        && !report.authenticated_transport_granted
        && !report.deployment_authority_granted
        && isolation.verify_digest()
        && isolation.runtime_binary_digest != [0; 32]
        && isolation.sandbox_profile_digest != [0; 32]
        && isolation.audit_schema_digest != [0; 32]
        && !isolation.network_access
        && !isolation.credential_access
        && !isolation.signing_access
        && !isolation.privileged_process
        && !isolation.arbitrary_shell
        && !isolation.filesystem_escape
        && !isolation.host_namespace_access
        && !plan.request_templates.is_empty()
        && plan.request_templates.len() <= policy.maximum_requests
        && plan.request_templates.len() == upstream.steps.len()
        && plan
            .request_templates
            .iter()
            .zip(&upstream.steps)
            .enumerate()
            .all(|(index, (template, step))| {
                template.sequence as usize == index
                    && template.verify_digest()
                    && template.payload_digest != [0; 32]
                    && template.region == step.region
                    && template.operation == step.operation
                    && template.resource_digest == step.resource_digest
            })
        && plan.created_at_ns >= report.finalized_at_ns
        && plan.created_at_ns <= at
        && plan.expires_at_ns > plan.created_at_ns
        && at <= plan.expires_at_ns
        && plan
            .expires_at_ns
            .checked_sub(plan.created_at_ns)
            .is_some_and(|v| v <= policy.maximum_session_duration_ns)
        && at
            .checked_sub(report.finalized_at_ns)
            .is_some_and(|v| v <= policy.maximum_report_age_ns)
}

fn valid_observation(
    value: &SimulatedExecutorObservation,
    request: &ExecutorRequestEnvelope,
    recorded_at: i64,
) -> bool {
    value.verify_digest()
        && value.request_id == request.request_id
        && value.request_digest == request.request_digest
        && value.observed_at_ns >= request.issued_at_ns
        && value.observed_at_ns <= request.expires_at_ns
        && value.observed_at_ns <= recorded_at
        && value.source_fixture_digest != [0; 32]
        && value.simulated_only
        && !value.credential_loaded
        && !value.signature_produced
        && !value.authenticated_request_sent
        && !value.external_submission_observed
        && !value.external_mutation_observed
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
    let bytes = serde_json::to_vec(value).expect("serializable executor-session state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandWire {
    version: u16,
    command: SessionCommand,
}

/// Encodes one bounded canonical command.
///
/// # Errors
///
/// Rejects serialization failures and oversized commands.
pub fn encode_command(command: &SessionCommand) -> Result<Vec<u8>, Error> {
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
/// Rejects oversized, malformed, unsupported, trailing, or noncanonical input.
pub fn decode_command(bytes: &[u8]) -> Result<SessionCommand, Error> {
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
