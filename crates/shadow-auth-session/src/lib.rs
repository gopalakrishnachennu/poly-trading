#![forbid(unsafe_code)]

//! Deterministic offline shadow authenticated-session coordination.
//!
//! Leases and attestations are simulation evidence only. This crate contains no
//! credential, key, signature, provider, socket, transport, or submission path.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableShadowAuthSession,
    SessionCheckpoint, SessionRecovery, SessionStorageError,
};
pub use report::{read_report, write_report_create_new, SessionReportFileError};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use submission_gateway_certification::{GatewayCertificationReport, GatewayReportStatus};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_ITEMS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SessionCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionPolicy {
    pub maximum_gateway_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_attestation_lifetime_ns: i64,
    pub maximum_lease_lifetime_ns: i64,
    pub maximum_heartbeat_age_ns: i64,
    pub maximum_rotations: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecordedSessionAttestation {
    pub epoch: u64,
    pub attestation_id: [u8; 32],
    pub predecessor_digest: [u8; 32],
    pub gateway_report_digest: [u8; 32],
    pub authentication_contract_digest: [u8; 32],
    pub channel_binding_digest: [u8; 32],
    pub token_binding_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub valid_until_ns: i64,
    pub source_digest: [u8; 32],
    pub recorded_only: bool,
    pub credential_material_present: bool,
    pub certificate_private_key_present: bool,
    pub signature_bytes_present: bool,
    pub provider_contacted: bool,
    pub socket_opened: bool,
    pub external_authority_granted: bool,
    pub attestation_digest: [u8; 32],
}

impl RecordedSessionAttestation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.attestation_digest = digest_without(b"recorded-session-attestation-v1", &self, |v| {
            v.attestation_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.attestation_digest
            == digest_without(b"recorded-session-attestation-v1", self, |v| {
                v.attestation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionScenario {
    CleanClose,
    AttestationRotation,
    DeadManExpiry,
    RestartRevocation,
    AmbiguityRecovery,
}

impl SessionScenario {
    pub const ALL: [Self; 5] = [
        Self::CleanClose,
        Self::AttestationRotation,
        Self::DeadManExpiry,
        Self::RestartRevocation,
        Self::AmbiguityRecovery,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowSessionPlan {
    pub plan_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub gateway_report: GatewayCertificationReport,
    pub initial_attestation: RecordedSessionAttestation,
    pub required_scenarios: Vec<SessionScenario>,
    pub session_policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}

impl ShadowSessionPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &SessionPolicy) -> Self {
        self.required_scenarios.sort();
        self.session_policy_digest = digest_json(b"shadow-auth-session-policy-v1", policy);
        self.plan_digest = digest_without(b"shadow-auth-session-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &SessionPolicy) -> bool {
        self.session_policy_digest == digest_json(b"shadow-auth-session-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"shadow-auth-session-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowSessionLease {
    pub lease_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub gateway_report_digest: [u8; 32],
    pub attestation_digest: [u8; 32],
    pub opaque_owner_id: [u8; 32],
    pub opened_at_ns: i64,
    pub expires_at_ns: i64,
    pub heartbeat_sequence: u64,
    pub last_heartbeat_at_ns: i64,
    pub simulated_only: bool,
    pub lease_digest: [u8; 32],
}

impl ShadowSessionLease {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.lease_digest
            == digest_without(b"shadow-auth-session-lease-v1", self, |v| {
                v.lease_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatHealth {
    Healthy,
    Unhealthy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecordedSessionHeartbeat {
    pub heartbeat_id: [u8; 32],
    pub lease_digest: [u8; 32],
    pub sequence: u64,
    pub observed_at_ns: i64,
    pub health: HeartbeatHealth,
    pub source_digest: [u8; 32],
    pub recorded_only: bool,
    pub credential_loaded: bool,
    pub signature_produced: bool,
    pub provider_contacted: bool,
    pub socket_opened: bool,
    pub authenticated_transport_used: bool,
    pub external_submission_observed: bool,
    pub external_mutation_observed: bool,
    pub heartbeat_digest: [u8; 32],
}

impl RecordedSessionHeartbeat {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.heartbeat_digest = digest_without(b"recorded-session-heartbeat-v1", &self, |v| {
            v.heartbeat_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.heartbeat_digest
            == digest_without(b"recorded-session-heartbeat-v1", self, |v| {
                v.heartbeat_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReason {
    DeadMan,
    UnhealthyHeartbeat,
    Restart,
    Ambiguity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecoveryRequirement {
    pub reason: RecoveryReason,
    pub subject_digest: [u8; 32],
    pub revoked_lease_digest: [u8; 32],
    pub attestation_digest: [u8; 32],
    pub trigger_digest: [u8; 32],
    pub triggered_at_ns: i64,
    pub requirement_digest: [u8; 32],
}

impl SessionRecoveryRequirement {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.requirement_digest
            == digest_without(b"session-recovery-requirement-v1", self, |v| {
                v.requirement_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RecordedSessionRecovery {
    pub recovery_id: [u8; 32],
    pub requirement_digest: [u8; 32],
    pub subject_digest: [u8; 32],
    pub attestation_digest: [u8; 32],
    pub no_mutation_digest: [u8; 32],
    pub opaque_operator_id: [u8; 32],
    pub observed_at_ns: i64,
    pub recorded_only: bool,
    pub credential_loaded: bool,
    pub signature_produced: bool,
    pub provider_contacted: bool,
    pub socket_opened: bool,
    pub authenticated_transport_used: bool,
    pub external_submission_observed: bool,
    pub external_mutation_observed: bool,
    pub recovery_digest: [u8; 32],
}

impl RecordedSessionRecovery {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.recovery_digest = digest_without(b"recorded-session-recovery-v1", &self, |v| {
            v.recovery_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.recovery_digest
            == digest_without(b"recorded-session-recovery-v1", self, |v| {
                v.recovery_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionReportStatus {
    SimulationCompleted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ShadowSessionReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub gateway_report_digest: [u8; 32],
    pub final_attestation_digest: [u8; 32],
    pub covered_scenarios: Vec<SessionScenario>,
    pub opened_lease_count: usize,
    pub rotation_count: usize,
    pub recovery_count: usize,
    pub finalized_at_ns: i64,
    pub status: SessionReportStatus,
    pub credential_material_created: bool,
    pub signature_produced: bool,
    pub provider_contacted: bool,
    pub socket_opened: bool,
    pub authentication_authority_granted: bool,
    pub external_submission_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl ShadowSessionReport {
    /// Seals recorded downstream certification evidence.
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"shadow-auth-session-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"shadow-auth-session-report-v1", self, |v| {
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
pub enum SessionCommand {
    Register {
        command_id: SessionCommandId,
        plan: Box<ShadowSessionPlan>,
        recorded_at_ns: i64,
    },
    OpenLease {
        command_id: SessionCommandId,
        lease_id: [u8; 32],
        opaque_owner_id: [u8; 32],
        opened_at_ns: i64,
        requested_expires_at_ns: i64,
        recorded_at_ns: i64,
    },
    Heartbeat {
        command_id: SessionCommandId,
        lease: Box<ShadowSessionLease>,
        heartbeat: RecordedSessionHeartbeat,
        recorded_at_ns: i64,
    },
    CloseLease {
        command_id: SessionCommandId,
        lease: Box<ShadowSessionLease>,
        closed_at_ns: i64,
        recorded_at_ns: i64,
    },
    RotateAttestation {
        command_id: SessionCommandId,
        attestation: Box<RecordedSessionAttestation>,
        recorded_at_ns: i64,
    },
    EvaluateDeadMan {
        command_id: SessionCommandId,
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    },
    Restart {
        command_id: SessionCommandId,
        restart_id: [u8; 32],
        restarted_at_ns: i64,
        recorded_at_ns: i64,
    },
    ObserveAmbiguity {
        command_id: SessionCommandId,
        lease: Box<ShadowSessionLease>,
        ambiguity_id: [u8; 32],
        ambiguity_digest: [u8; 32],
        observed_at_ns: i64,
        recorded_at_ns: i64,
    },
    Recover {
        command_id: SessionCommandId,
        requirement: Box<SessionRecoveryRequirement>,
        evidence: RecordedSessionRecovery,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: SessionCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl SessionCommand {
    #[must_use]
    pub const fn command_id(&self) -> SessionCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::OpenLease { command_id, .. }
            | Self::Heartbeat { command_id, .. }
            | Self::CloseLease { command_id, .. }
            | Self::RotateAttestation { command_id, .. }
            | Self::EvaluateDeadMan { command_id, .. }
            | Self::Restart { command_id, .. }
            | Self::ObserveAmbiguity { command_id, .. }
            | Self::Recover { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::OpenLease { recorded_at_ns, .. }
            | Self::Heartbeat { recorded_at_ns, .. }
            | Self::CloseLease { recorded_at_ns, .. }
            | Self::RotateAttestation { recorded_at_ns, .. }
            | Self::EvaluateDeadMan { recorded_at_ns, .. }
            | Self::Restart { recorded_at_ns, .. }
            | Self::ObserveAmbiguity { recorded_at_ns, .. }
            | Self::Recover { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SessionDetail {
    Registered,
    LeaseOpened(Box<ShadowSessionLease>),
    HeartbeatAccepted(Box<ShadowSessionLease>),
    LeaseRevoked(Box<SessionRecoveryRequirement>),
    LeaseClosed,
    AttestationRotated,
    Recovered,
    Finalized(Box<ShadowSessionReport>),
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
    pub current_attestation: Option<RecordedSessionAttestation>,
    pub active_lease: Option<ShadowSessionLease>,
    pub recovery_requirement: Option<SessionRecoveryRequirement>,
    pub covered_scenarios: BTreeSet<SessionScenario>,
    pub last_report: Option<ShadowSessionReport>,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("shadow session policy is invalid")]
    Config,
    #[error("shadow session timestamp is invalid or regressed")]
    Timestamp,
    #[error("shadow session command exceeds its bound")]
    CommandBound,
    #[error("shadow session JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported shadow session command version: {0}")]
    Version(u16),
    #[error("shadow session command id conflict")]
    IdempotencyConflict,
    #[error("Phase 2.31 evidence is invalid, stale, or authority-bearing")]
    Upstream,
    #[error("shadow session plan or attestation is invalid")]
    Plan,
    #[error("shadow session lease is invalid, expired, occupied, or substituted")]
    Lease,
    #[error("shadow session heartbeat is invalid, regressed, or side-effect-bearing")]
    Heartbeat,
    #[error("shadow session attestation rotation is invalid")]
    Rotation,
    #[error("shadow session dead-man condition is not satisfied")]
    DeadMan,
    #[error("shadow session restart or ambiguity evidence is invalid")]
    Disruption,
    #[error("shadow session recovery evidence is invalid or substituted")]
    Recovery,
    #[error("shadow session finalization is invalid")]
    Finalize,
    #[error("shadow session arithmetic overflow")]
    Overflow,
    #[error("shadow session is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct ShadowAuthSessionCoordinator {
    policy: SessionPolicy,
    plan: Option<ShadowSessionPlan>,
    attestation: Option<RecordedSessionAttestation>,
    active_lease: Option<ShadowSessionLease>,
    recovery: Option<SessionRecoveryRequirement>,
    used_lease_ids: BTreeSet<[u8; 32]>,
    used_heartbeat_ids: BTreeSet<[u8; 32]>,
    used_attestation_ids: BTreeSet<[u8; 32]>,
    used_disruption_ids: BTreeSet<[u8; 32]>,
    used_recovery_ids: BTreeSet<[u8; 32]>,
    covered: BTreeSet<SessionScenario>,
    opened_leases: usize,
    rotations: usize,
    recoveries: usize,
    report: Option<ShadowSessionReport>,
    processed: BTreeMap<SessionCommandId, ([u8; 32], SessionOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl ShadowAuthSessionCoordinator {
    /// Creates one empty offline session coordinator.
    ///
    /// # Errors
    ///
    /// Rejects zero or excessive policy bounds.
    pub fn new(policy: SessionPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            attestation: None,
            active_lease: None,
            recovery: None,
            used_lease_ids: BTreeSet::new(),
            used_heartbeat_ids: BTreeSet::new(),
            used_attestation_ids: BTreeSet::new(),
            used_disruption_ids: BTreeSet::new(),
            used_recovery_ids: BTreeSet::new(),
            covered: BTreeSet::new(),
            opened_leases: 0,
            rotations: 0,
            recoveries: 0,
            report: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic shadow-session command.
    ///
    /// # Errors
    ///
    /// Integrity, chronology, exclusivity, recovery, or lifecycle failures halt.
    pub fn apply(&mut self, command: &SessionCommand) -> Result<SessionOutcome, Error> {
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
            Ok(detail) => detail,
            Err(error) => return self.halt(error),
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
        outcome.outcome_digest = digest_without(b"shadow-auth-session-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &SessionCommand) -> Result<SessionDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            SessionCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some() {
                    return Err(Error::Plan);
                }
                if !valid_upstream(&plan.gateway_report, &self.policy, *recorded_at_ns) {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.used_attestation_ids
                    .insert(plan.initial_attestation.attestation_id);
                self.attestation = Some(plan.initial_attestation.clone());
                self.plan = Some((**plan).clone());
                Ok(SessionDetail::Registered)
            }
            SessionCommand::OpenLease {
                lease_id,
                opaque_owner_id,
                opened_at_ns,
                requested_expires_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Lease)?;
                let attestation = self.attestation.as_ref().ok_or(Error::Lease)?;
                let maximum = opened_at_ns
                    .checked_add(self.policy.maximum_lease_lifetime_ns)
                    .ok_or(Error::Overflow)?;
                if self.active_lease.is_some()
                    || self.recovery.is_some()
                    || !valid_upstream(&plan.gateway_report, &self.policy, *opened_at_ns)
                    || *lease_id == [0; 32]
                    || *opaque_owner_id == [0; 32]
                    || self.used_lease_ids.contains(lease_id)
                    || *opened_at_ns > *recorded_at_ns
                    || *opened_at_ns < attestation.observed_at_ns
                    || *opened_at_ns > attestation.valid_until_ns
                    || *requested_expires_at_ns <= *opened_at_ns
                    || *requested_expires_at_ns > maximum
                    || *requested_expires_at_ns > attestation.valid_until_ns
                    || *requested_expires_at_ns > plan.expires_at_ns
                {
                    return Err(Error::Lease);
                }
                let mut lease = ShadowSessionLease {
                    lease_id: *lease_id,
                    plan_digest: plan.plan_digest,
                    gateway_report_digest: plan.gateway_report.report_digest,
                    attestation_digest: attestation.attestation_digest,
                    opaque_owner_id: *opaque_owner_id,
                    opened_at_ns: *opened_at_ns,
                    expires_at_ns: *requested_expires_at_ns,
                    heartbeat_sequence: 0,
                    last_heartbeat_at_ns: *opened_at_ns,
                    simulated_only: true,
                    lease_digest: [0; 32],
                };
                lease.lease_digest = digest_without(b"shadow-auth-session-lease-v1", &lease, |v| {
                    v.lease_digest = [0; 32];
                });
                self.used_lease_ids.insert(*lease_id);
                self.opened_leases = self.opened_leases.checked_add(1).ok_or(Error::Overflow)?;
                self.active_lease = Some(lease.clone());
                Ok(SessionDetail::LeaseOpened(Box::new(lease)))
            }
            SessionCommand::Heartbeat {
                lease,
                heartbeat,
                recorded_at_ns,
                ..
            } => {
                let active = self.active_lease.as_ref().ok_or(Error::Heartbeat)?;
                if **lease != *active
                    || !lease.verify_digest()
                    || !valid_heartbeat(heartbeat, lease, *recorded_at_ns)
                    || self.used_heartbeat_ids.contains(&heartbeat.heartbeat_id)
                {
                    return Err(Error::Heartbeat);
                }
                self.used_heartbeat_ids.insert(heartbeat.heartbeat_id);
                if heartbeat.health == HeartbeatHealth::Unhealthy {
                    let requirement = self.revoke_for_recovery(
                        RecoveryReason::UnhealthyHeartbeat,
                        heartbeat.heartbeat_digest,
                        heartbeat.observed_at_ns,
                    )?;
                    return Ok(SessionDetail::LeaseRevoked(Box::new(requirement)));
                }
                let mut updated = (**lease).clone();
                updated.heartbeat_sequence = heartbeat.sequence;
                updated.last_heartbeat_at_ns = heartbeat.observed_at_ns;
                updated.lease_digest =
                    digest_without(b"shadow-auth-session-lease-v1", &updated, |v| {
                        v.lease_digest = [0; 32];
                    });
                self.active_lease = Some(updated.clone());
                Ok(SessionDetail::HeartbeatAccepted(Box::new(updated)))
            }
            SessionCommand::CloseLease {
                lease,
                closed_at_ns,
                recorded_at_ns,
                ..
            } => {
                let active = self.active_lease.as_ref().ok_or(Error::Lease)?;
                if **lease != *active
                    || !lease.verify_digest()
                    || self.recovery.is_some()
                    || *closed_at_ns < lease.last_heartbeat_at_ns
                    || *closed_at_ns > lease.expires_at_ns
                    || *closed_at_ns > *recorded_at_ns
                {
                    return Err(Error::Lease);
                }
                self.active_lease = None;
                self.covered.insert(SessionScenario::CleanClose);
                Ok(SessionDetail::LeaseClosed)
            }
            SessionCommand::RotateAttestation {
                attestation,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Rotation)?;
                let current = self.attestation.as_ref().ok_or(Error::Rotation)?;
                if self.active_lease.is_some()
                    || self.recovery.is_some()
                    || !valid_upstream(&plan.gateway_report, &self.policy, *recorded_at_ns)
                    || self.rotations >= self.policy.maximum_rotations
                    || self
                        .used_attestation_ids
                        .contains(&attestation.attestation_id)
                    || !valid_rotated_attestation(
                        attestation,
                        current,
                        plan,
                        &self.policy,
                        *recorded_at_ns,
                    )
                {
                    return Err(Error::Rotation);
                }
                self.used_attestation_ids.insert(attestation.attestation_id);
                self.rotations = self.rotations.checked_add(1).ok_or(Error::Overflow)?;
                self.attestation = Some((**attestation).clone());
                self.covered.insert(SessionScenario::AttestationRotation);
                Ok(SessionDetail::AttestationRotated)
            }
            SessionCommand::EvaluateDeadMan {
                evaluated_at_ns,
                recorded_at_ns,
                ..
            } => {
                let lease = self.active_lease.as_ref().ok_or(Error::DeadMan)?;
                let heartbeat_expiry = lease
                    .last_heartbeat_at_ns
                    .checked_add(self.policy.maximum_heartbeat_age_ns)
                    .ok_or(Error::Overflow)?;
                if *evaluated_at_ns > *recorded_at_ns
                    || (*evaluated_at_ns <= heartbeat_expiry
                        && *evaluated_at_ns <= lease.expires_at_ns)
                {
                    return Err(Error::DeadMan);
                }
                let trigger = digest_json(
                    b"shadow-session-dead-man-trigger-v1",
                    &(lease.lease_digest, evaluated_at_ns),
                );
                let requirement =
                    self.revoke_for_recovery(RecoveryReason::DeadMan, trigger, *evaluated_at_ns)?;
                self.covered.insert(SessionScenario::DeadManExpiry);
                Ok(SessionDetail::LeaseRevoked(Box::new(requirement)))
            }
            SessionCommand::Restart {
                restart_id,
                restarted_at_ns,
                recorded_at_ns,
                ..
            } => {
                let lease = self.active_lease.as_ref().ok_or(Error::Disruption)?;
                if *restart_id == [0; 32]
                    || self.used_disruption_ids.contains(restart_id)
                    || self.recovery.is_some()
                    || *restarted_at_ns < lease.last_heartbeat_at_ns
                    || *restarted_at_ns > *recorded_at_ns
                {
                    return Err(Error::Disruption);
                }
                self.used_disruption_ids.insert(*restart_id);
                let trigger = digest_json(
                    b"shadow-session-restart-trigger-v1",
                    &(restart_id, restarted_at_ns),
                );
                let requirement =
                    self.revoke_for_recovery(RecoveryReason::Restart, trigger, *restarted_at_ns)?;
                self.covered.insert(SessionScenario::RestartRevocation);
                Ok(SessionDetail::LeaseRevoked(Box::new(requirement)))
            }
            SessionCommand::ObserveAmbiguity {
                lease,
                ambiguity_id,
                ambiguity_digest,
                observed_at_ns,
                recorded_at_ns,
                ..
            } => {
                let active = self.active_lease.as_ref().ok_or(Error::Disruption)?;
                if **lease != *active
                    || !lease.verify_digest()
                    || *ambiguity_id == [0; 32]
                    || *ambiguity_digest == [0; 32]
                    || self.used_disruption_ids.contains(ambiguity_id)
                    || *observed_at_ns < lease.last_heartbeat_at_ns
                    || *observed_at_ns > *recorded_at_ns
                {
                    return Err(Error::Disruption);
                }
                self.used_disruption_ids.insert(*ambiguity_id);
                let trigger = digest_json(
                    b"shadow-session-ambiguity-trigger-v1",
                    &(ambiguity_id, ambiguity_digest),
                );
                let requirement =
                    self.revoke_for_recovery(RecoveryReason::Ambiguity, trigger, *observed_at_ns)?;
                Ok(SessionDetail::LeaseRevoked(Box::new(requirement)))
            }
            SessionCommand::Recover {
                requirement,
                evidence,
                recorded_at_ns,
                ..
            } => {
                let current = self.recovery.as_ref().ok_or(Error::Recovery)?;
                let attestation = self.attestation.as_ref().ok_or(Error::Recovery)?;
                if **requirement != *current
                    || !requirement.verify_digest()
                    || !valid_recovery(evidence, requirement, attestation, *recorded_at_ns)
                    || self.used_recovery_ids.contains(&evidence.recovery_id)
                {
                    return Err(Error::Recovery);
                }
                self.used_recovery_ids.insert(evidence.recovery_id);
                if requirement.reason == RecoveryReason::Ambiguity {
                    self.covered.insert(SessionScenario::AmbiguityRecovery);
                }
                self.recoveries = self.recoveries.checked_add(1).ok_or(Error::Overflow)?;
                self.recovery = None;
                Ok(SessionDetail::Recovered)
            }
            SessionCommand::Finalize {
                report_id,
                finalized_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                let attestation = self.attestation.as_ref().ok_or(Error::Finalize)?;
                if *report_id == [0; 32]
                    || self.active_lease.is_some()
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
                let mut report = ShadowSessionReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    gateway_report_digest: plan.gateway_report.report_digest,
                    final_attestation_digest: attestation.attestation_digest,
                    covered_scenarios: self.covered.iter().copied().collect(),
                    opened_lease_count: self.opened_leases,
                    rotation_count: self.rotations,
                    recovery_count: self.recoveries,
                    finalized_at_ns: *finalized_at_ns,
                    status: SessionReportStatus::SimulationCompleted,
                    credential_material_created: false,
                    signature_produced: false,
                    provider_contacted: false,
                    socket_opened: false,
                    authentication_authority_granted: false,
                    external_submission_authority_granted: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    report_digest: [0; 32],
                };
                report.report_digest =
                    digest_without(b"shadow-auth-session-report-v1", &report, |v| {
                        v.report_digest = [0; 32];
                    });
                self.report = Some(report.clone());
                Ok(SessionDetail::Finalized(Box::new(report)))
            }
        }
    }

    fn revoke_for_recovery(
        &mut self,
        reason: RecoveryReason,
        trigger_digest: [u8; 32],
        triggered_at_ns: i64,
    ) -> Result<SessionRecoveryRequirement, Error> {
        let lease = self.active_lease.take().ok_or(Error::Disruption)?;
        let attestation = self.attestation.as_ref().ok_or(Error::Disruption)?;
        let subject_digest = digest_json(
            b"shadow-session-recovery-subject-v1",
            &(
                reason,
                lease.lease_digest,
                attestation.attestation_digest,
                trigger_digest,
            ),
        );
        let mut requirement = SessionRecoveryRequirement {
            reason,
            subject_digest,
            revoked_lease_digest: lease.lease_digest,
            attestation_digest: attestation.attestation_digest,
            trigger_digest,
            triggered_at_ns,
            requirement_digest: [0; 32],
        };
        requirement.requirement_digest =
            digest_without(b"session-recovery-requirement-v1", &requirement, |v| {
                v.requirement_digest = [0; 32];
            });
        self.recovery = Some(requirement.clone());
        Ok(requirement)
    }

    #[must_use]
    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            accepted_commands: self.accepted_commands,
            current_attestation: self.attestation.clone(),
            active_lease: self.active_lease.clone(),
            recovery_requirement: self.recovery.clone(),
            covered_scenarios: self.covered.clone(),
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
        hasher.update(b"shadow-auth-session-state-v1");
        hash_value(
            &mut hasher,
            &(
                &self.policy,
                &self.plan,
                &self.attestation,
                &self.active_lease,
                &self.recovery,
                &self.used_lease_ids,
                &self.used_heartbeat_ids,
                &self.used_attestation_ids,
                &self.used_disruption_ids,
                &self.used_recovery_ids,
                &self.covered,
                self.opened_leases,
                self.rotations,
                self.recoveries,
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

fn validate_policy(policy: &SessionPolicy) -> Result<(), Error> {
    if policy.maximum_gateway_report_age_ns <= 0
        || policy.maximum_plan_lifetime_ns <= 0
        || policy.maximum_attestation_lifetime_ns <= 0
        || policy.maximum_lease_lifetime_ns <= 0
        || policy.maximum_heartbeat_age_ns <= 0
        || policy.maximum_rotations == 0
        || policy.maximum_rotations > MAX_ITEMS
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn valid_upstream(report: &GatewayCertificationReport, policy: &SessionPolicy, at: i64) -> bool {
    report.verify_digest()
        && report.status == GatewayReportStatus::ShadowCertified
        && report.rejected_envelopes == 0
        && !report.credential_material_created
        && !report.signature_produced
        && !report.socket_opened
        && !report.authentication_authority_granted
        && !report.external_submission_authority_granted
        && !report.deployment_authority_granted
        && report.authentication_contract_digest != [0; 32]
        && report.channel_binding_digest != [0; 32]
        && report.token_binding_digest != [0; 32]
        && at
            .checked_sub(report.finalized_at_ns)
            .is_some_and(|age| age <= policy.maximum_gateway_report_age_ns)
}

fn valid_plan(plan: &ShadowSessionPlan, policy: &SessionPolicy, at: i64) -> bool {
    plan.plan_id != [0; 32]
        && plan.verify_digest(policy)
        && plan.required_scenarios == SessionScenario::ALL
        && plan.created_at_ns >= plan.gateway_report.finalized_at_ns
        && plan.created_at_ns <= at
        && plan.expires_at_ns > plan.created_at_ns
        && plan
            .expires_at_ns
            .checked_sub(plan.created_at_ns)
            .is_some_and(|age| age <= policy.maximum_plan_lifetime_ns)
        && valid_initial_attestation(&plan.initial_attestation, plan, policy)
}

fn valid_initial_attestation(
    value: &RecordedSessionAttestation,
    plan: &ShadowSessionPlan,
    policy: &SessionPolicy,
) -> bool {
    value.epoch == 0
        && value.predecessor_digest == [0; 32]
        && valid_attestation_common(value, plan, policy)
}

fn valid_rotated_attestation(
    value: &RecordedSessionAttestation,
    current: &RecordedSessionAttestation,
    plan: &ShadowSessionPlan,
    policy: &SessionPolicy,
    at: i64,
) -> bool {
    value.epoch == current.epoch.checked_add(1).unwrap_or(u64::MAX)
        && value.predecessor_digest == current.attestation_digest
        && value.observed_at_ns >= current.observed_at_ns
        && value.observed_at_ns <= at
        && valid_attestation_common(value, plan, policy)
}

fn valid_attestation_common(
    value: &RecordedSessionAttestation,
    plan: &ShadowSessionPlan,
    policy: &SessionPolicy,
) -> bool {
    value.verify_digest()
        && value.attestation_id != [0; 32]
        && value.gateway_report_digest == plan.gateway_report.report_digest
        && value.authentication_contract_digest
            == plan.gateway_report.authentication_contract_digest
        && value.channel_binding_digest == plan.gateway_report.channel_binding_digest
        && value.token_binding_digest == plan.gateway_report.token_binding_digest
        && value.observed_at_ns >= plan.created_at_ns
        && value.valid_until_ns > value.observed_at_ns
        && value.valid_until_ns <= plan.expires_at_ns
        && plan
            .gateway_report
            .finalized_at_ns
            .checked_add(policy.maximum_gateway_report_age_ns)
            .is_some_and(|limit| value.valid_until_ns <= limit)
        && value
            .valid_until_ns
            .checked_sub(value.observed_at_ns)
            .is_some_and(|age| age <= policy.maximum_attestation_lifetime_ns)
        && value.source_digest != [0; 32]
        && value.recorded_only
        && !value.credential_material_present
        && !value.certificate_private_key_present
        && !value.signature_bytes_present
        && !value.provider_contacted
        && !value.socket_opened
        && !value.external_authority_granted
}

fn valid_heartbeat(value: &RecordedSessionHeartbeat, lease: &ShadowSessionLease, at: i64) -> bool {
    value.verify_digest()
        && value.heartbeat_id != [0; 32]
        && value.lease_digest == lease.lease_digest
        && value.sequence == lease.heartbeat_sequence.checked_add(1).unwrap_or(u64::MAX)
        && value.observed_at_ns > lease.last_heartbeat_at_ns
        && value.observed_at_ns <= lease.expires_at_ns
        && value.observed_at_ns <= at
        && value.source_digest != [0; 32]
        && value.recorded_only
        && !value.credential_loaded
        && !value.signature_produced
        && !value.provider_contacted
        && !value.socket_opened
        && !value.authenticated_transport_used
        && !value.external_submission_observed
        && !value.external_mutation_observed
}

fn valid_recovery(
    value: &RecordedSessionRecovery,
    requirement: &SessionRecoveryRequirement,
    attestation: &RecordedSessionAttestation,
    at: i64,
) -> bool {
    value.verify_digest()
        && value.recovery_id != [0; 32]
        && value.requirement_digest == requirement.requirement_digest
        && value.subject_digest == requirement.subject_digest
        && value.attestation_digest == attestation.attestation_digest
        && value.no_mutation_digest != [0; 32]
        && value.opaque_operator_id != [0; 32]
        && value.observed_at_ns >= requirement.triggered_at_ns
        && value.observed_at_ns <= attestation.valid_until_ns
        && value.observed_at_ns <= at
        && value.recorded_only
        && !value.credential_loaded
        && !value.signature_produced
        && !value.provider_contacted
        && !value.socket_opened
        && !value.authenticated_transport_used
        && !value.external_submission_observed
        && !value.external_mutation_observed
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
    let bytes = serde_json::to_vec(value).expect("serializable shadow-session state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandWire {
    version: u16,
    command: SessionCommand,
}

/// Encodes one bounded canonical session command.
///
/// # Errors
///
/// Rejects serialization failures and oversized commands.
pub fn encode_command(command: &SessionCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one bounded canonical session command.
///
/// # Errors
///
/// Rejects malformed, unsupported, trailing, noncanonical, or oversized input.
pub fn decode_command(bytes: &[u8]) -> Result<SessionCommand, Error> {
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
