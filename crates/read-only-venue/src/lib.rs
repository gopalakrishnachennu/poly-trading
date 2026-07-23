#![forbid(unsafe_code)]

//! Deterministic supervision for public and authenticated read-only venue data.
//!
//! This crate has no credential value, mutation endpoint, signer, wallet,
//! order submission, or cancellation capability.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableReadOnlyVenue,
    VenueCheckpoint, VenueRecovery, VenueStorageError,
};
pub use report::{read_report, write_report_create_new, VenueReportFileError};

use security_boundary::{ProviderClass, SecurityReport, SecurityReportStatus, SecurityScenario};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct VenueCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenuePolicy {
    pub maximum_security_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_channel_age_ns: i64,
    pub maximum_parameter_age_ns: i64,
    pub maximum_mode_age_ns: i64,
    pub maximum_backoff_ns: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    PublicMarket,
    AuthenticatedUser,
    RestMetadata,
    ReferencePrice,
}
impl ChannelKind {
    pub const ALL: [Self; 4] = [
        Self::PublicMarket,
        Self::AuthenticatedUser,
        Self::RestMetadata,
        Self::ReferencePrice,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelHealth {
    Synchronizing,
    Ready,
    Stale,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserEventClass {
    OrderLifecycle,
    TradeLifecycle,
    BalanceObservation,
    Heartbeat,
}
impl UserEventClass {
    pub const ALL: [Self; 4] = [
        Self::OrderLifecycle,
        Self::TradeLifecycle,
        Self::BalanceObservation,
        Self::Heartbeat,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct AuthenticatedObservationContract {
    pub host_digest: [u8; 32],
    pub channel_subject_digest: [u8; 32],
    pub allowed_events: Vec<UserEventClass>,
    pub subscription_only: bool,
    pub credential_value_present: bool,
    pub authorization_header_present: bool,
    pub order_endpoint_present: bool,
    pub cancel_endpoint_present: bool,
    pub wallet_endpoint_present: bool,
    pub arbitrary_request_allowed: bool,
    pub contract_digest: [u8; 32],
}
impl AuthenticatedObservationContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.allowed_events.sort_by_key(|v| *v as u8);
        self.contract_digest =
            digest_without(b"authenticated-observation-contract-v1", &self, |v| {
                v.contract_digest = [0; 32];
            });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest
            == digest_without(b"authenticated-observation-contract-v1", self, |v| {
                v.contract_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelObservation {
    pub observation_id: [u8; 32],
    pub channel: ChannelKind,
    pub epoch: u64,
    pub sequence: u64,
    pub snapshot_digest: [u8; 32],
    pub provenance_digest: [u8; 32],
    pub event_time_ns: i64,
    pub received_time_ns: i64,
    pub observed_at_ns: i64,
    pub health: ChannelHealth,
    pub observation_digest: [u8; 32],
}
impl ChannelObservation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.observation_digest = digest_without(b"venue-channel-observation-v1", &self, |v| {
            v.observation_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.observation_digest
            == digest_without(b"venue-channel-observation-v1", self, |v| {
                v.observation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketParameters {
    pub condition_id_digest: [u8; 32],
    pub up_token_digest: [u8; 32],
    pub down_token_digest: [u8; 32],
    pub version: u64,
    pub tick_size_micros: u32,
    pub minimum_order_quantity_micros: u64,
    pub maker_fee_bps: u32,
    pub taker_fee_bps: u32,
    pub taker_delay_ns: i64,
    pub minimum_order_age_ns: i64,
    pub observed_at_ns: i64,
    pub parameters_digest: [u8; 32],
}
impl MarketParameters {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.parameters_digest = digest_without(b"venue-market-parameters-v1", &self, |v| {
            v.parameters_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.parameters_digest
            == digest_without(b"venue-market-parameters-v1", self, |v| {
                v.parameters_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VenueMode {
    Normal,
    Restarting,
    PostOnly,
    CancelOnly,
    TradingDisabled,
    Recovering,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeObservation {
    pub observation_id: [u8; 32],
    pub sequence: u64,
    pub mode: VenueMode,
    pub source_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub observation_digest: [u8; 32],
}
impl ModeObservation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.observation_digest = digest_without(b"venue-mode-observation-v1", &self, |v| {
            v.observation_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.observation_digest
            == digest_without(b"venue-mode-observation-v1", self, |v| {
                v.observation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VenueScenario {
    PublicSync,
    UserSync,
    DynamicParameters,
    NormalMode,
    RestartRecovery,
    PostOnly,
    CancelOnly,
    RateLimitBackoff,
    IndependentChannelFailure,
    ReconnectRecovery,
}
impl VenueScenario {
    pub const ALL: [Self; 10] = [
        Self::PublicSync,
        Self::UserSync,
        Self::DynamicParameters,
        Self::NormalMode,
        Self::RestartRecovery,
        Self::PostOnly,
        Self::CancelOnly,
        Self::RateLimitBackoff,
        Self::IndependentChannelFailure,
        Self::ReconnectRecovery,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenuePlan {
    pub plan_id: [u8; 32],
    pub security_report: SecurityReport,
    pub authenticated_contract: AuthenticatedObservationContract,
    pub condition_id_digest: [u8; 32],
    pub up_token_digest: [u8; 32],
    pub down_token_digest: [u8; 32],
    pub required_scenarios: Vec<VenueScenario>,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}
impl VenuePlan {
    #[must_use]
    pub fn sealed(mut self, policy: &VenuePolicy) -> Self {
        self.required_scenarios.sort();
        self.policy_digest = digest_json(b"venue-policy-v1", policy);
        self.plan_digest = digest_without(b"venue-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self, policy: &VenuePolicy) -> bool {
        self.policy_digest == digest_json(b"venue-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"venue-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReason {
    Restart,
    ChannelFailure,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueRecoveryRequirement {
    pub reason: RecoveryReason,
    pub trigger_digest: [u8; 32],
    pub prior_epoch: u64,
    pub requirement_digest: [u8; 32],
}
impl VenueRecoveryRequirement {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.requirement_digest
            == digest_without(b"venue-recovery-requirement-v1", self, |v| {
                v.requirement_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct VenueRecoveryEvidence {
    pub recovery_id: [u8; 32],
    pub requirement_digest: [u8; 32],
    pub channel_snapshots: Vec<ChannelObservation>,
    pub parameters: MarketParameters,
    pub mode: ModeObservation,
    pub reconciliation_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub no_mutation_observed: bool,
    pub credential_value_present: bool,
    pub order_submitted: bool,
    pub cancellation_submitted: bool,
    pub evidence_digest: [u8; 32],
}
impl VenueRecoveryEvidence {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.channel_snapshots.sort_by_key(|v| v.channel);
        self.evidence_digest = digest_without(b"venue-recovery-evidence-v1", &self, |v| {
            v.evidence_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.evidence_digest
            == digest_without(b"venue-recovery-evidence-v1", self, |v| {
                v.evidence_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VenueReportStatus {
    LocallyCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct VenueReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub security_report_digest: [u8; 32],
    pub final_epoch: u64,
    pub final_parameter_version: u64,
    pub covered_scenarios: Vec<VenueScenario>,
    pub finalized_at_ns: i64,
    pub status: VenueReportStatus,
    pub live_environment_certified: bool,
    pub credential_material_created: bool,
    pub authenticated_session_opened: bool,
    pub order_endpoint_present: bool,
    pub cancel_endpoint_present: bool,
    pub order_submitted: bool,
    pub cancellation_submitted: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub submission_authority_granted: bool,
    pub report_digest: [u8; 32],
}
impl VenueReport {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"venue-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"venue-report-v1", self, |v| {
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
pub enum VenueCommand {
    Register {
        command_id: VenueCommandId,
        plan: Box<VenuePlan>,
        recorded_at_ns: i64,
    },
    ObserveChannel {
        command_id: VenueCommandId,
        observation: ChannelObservation,
        recorded_at_ns: i64,
    },
    ObserveParameters {
        command_id: VenueCommandId,
        parameters: MarketParameters,
        recorded_at_ns: i64,
    },
    ObserveMode {
        command_id: VenueCommandId,
        mode: ModeObservation,
        recorded_at_ns: i64,
    },
    ObserveRateLimit {
        command_id: VenueCommandId,
        observation_id: [u8; 32],
        backoff_ns: i64,
        automatic_retry_attempted: bool,
        observed_at_ns: i64,
        recorded_at_ns: i64,
    },
    FailChannel {
        command_id: VenueCommandId,
        channel: ChannelKind,
        failure_digest: [u8; 32],
        observed_at_ns: i64,
        recorded_at_ns: i64,
    },
    Recover {
        command_id: VenueCommandId,
        requirement: Box<VenueRecoveryRequirement>,
        evidence: Box<VenueRecoveryEvidence>,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: VenueCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}
impl VenueCommand {
    #[must_use]
    pub const fn command_id(&self) -> VenueCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::ObserveChannel { command_id, .. }
            | Self::ObserveParameters { command_id, .. }
            | Self::ObserveMode { command_id, .. }
            | Self::ObserveRateLimit { command_id, .. }
            | Self::FailChannel { command_id, .. }
            | Self::Recover { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::ObserveChannel { recorded_at_ns, .. }
            | Self::ObserveParameters { recorded_at_ns, .. }
            | Self::ObserveMode { recorded_at_ns, .. }
            | Self::ObserveRateLimit { recorded_at_ns, .. }
            | Self::FailChannel { recorded_at_ns, .. }
            | Self::Recover { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum VenueDetail {
    Registered,
    ChannelAccepted,
    ParametersAccepted,
    ModeAccepted,
    RateLimitAccepted,
    RecoveryRequired(Box<VenueRecoveryRequirement>),
    Recovered,
    Finalized(Box<VenueReport>),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VenueOutcome {
    pub command_id: VenueCommandId,
    pub detail: VenueDetail,
    pub outcome_digest: [u8; 32],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VenueSnapshot {
    pub channels: BTreeMap<ChannelKind, ChannelObservation>,
    pub parameters: Option<MarketParameters>,
    pub mode: Option<ModeObservation>,
    pub recovery: Option<VenueRecoveryRequirement>,
    pub observation_ready: bool,
    pub covered_scenarios: BTreeSet<VenueScenario>,
    pub report: Option<VenueReport>,
    pub accepted_commands: u64,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("venue policy invalid")]
    Config,
    #[error("venue timestamp invalid or regressed")]
    Timestamp,
    #[error("venue command exceeds bound")]
    CommandBound,
    #[error("venue JSON invalid: {0}")]
    Json(String),
    #[error("unsupported venue command version: {0}")]
    Version(u16),
    #[error("venue command id conflict")]
    IdempotencyConflict,
    #[error("Phase 3.1 evidence invalid, stale, incomplete, or authority-bearing")]
    Upstream,
    #[error("venue plan or authenticated observation contract invalid")]
    Plan,
    #[error("venue channel observation invalid or regressed")]
    Channel,
    #[error("venue parameters invalid, substituted, or regressed")]
    Parameters,
    #[error("venue mode observation invalid or regressed")]
    Mode,
    #[error("venue rate-limit evidence invalid")]
    RateLimit,
    #[error("venue failure or recovery transition invalid")]
    Recovery,
    #[error("venue finalization invalid")]
    Finalize,
    #[error("venue arithmetic overflow")]
    Overflow,
    #[error("venue supervisor halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct ReadOnlyVenueSupervisor {
    policy: VenuePolicy,
    plan: Option<VenuePlan>,
    channels: BTreeMap<ChannelKind, ChannelObservation>,
    parameters: Option<MarketParameters>,
    mode: Option<ModeObservation>,
    recovery: Option<VenueRecoveryRequirement>,
    covered: BTreeSet<VenueScenario>,
    used_observations: BTreeSet<[u8; 32]>,
    used_recoveries: BTreeSet<[u8; 32]>,
    processed: BTreeMap<VenueCommandId, ([u8; 32], VenueOutcome)>,
    accepted_commands: u64,
    report: Option<VenueReport>,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl ReadOnlyVenueSupervisor {
    /// Creates an empty read-only venue supervisor.
    ///
    /// # Errors
    ///
    /// Rejects zero policy bounds.
    pub fn new(policy: VenuePolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            channels: BTreeMap::new(),
            parameters: None,
            mode: None,
            recovery: None,
            covered: BTreeSet::new(),
            used_observations: BTreeSet::new(),
            used_recoveries: BTreeSet::new(),
            processed: BTreeMap::new(),
            accepted_commands: 0,
            report: None,
            last_recorded_at_ns: None,
            halted: None,
        })
    }
    /// Applies one deterministic venue-supervision command.
    ///
    /// # Errors
    ///
    /// Invalid chronology, evidence, sequence, recovery, or identity halts.
    pub fn apply(&mut self, command: &VenueCommand) -> Result<VenueOutcome, Error> {
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
        let mut outcome = VenueOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"venue-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &VenueCommand) -> Result<VenueDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            VenueCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some()
                    || !valid_upstream(&plan.security_report, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(VenueDetail::Registered)
            }
            VenueCommand::ObserveChannel {
                observation,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Channel)?;
                if self.recovery.is_some()
                    || self.used_observations.contains(&observation.observation_id)
                    || !valid_channel(observation, &self.policy, plan, *recorded_at_ns)
                {
                    return Err(Error::Channel);
                }
                if let Some(prior) = self.channels.get(&observation.channel) {
                    if observation.epoch != prior.epoch
                        || observation.sequence
                            != prior.sequence.checked_add(1).ok_or(Error::Overflow)?
                        || observation.observed_at_ns < prior.observed_at_ns
                    {
                        return Err(Error::Channel);
                    }
                } else if observation.sequence != 1 {
                    return Err(Error::Channel);
                }
                self.used_observations.insert(observation.observation_id);
                self.channels
                    .insert(observation.channel, observation.clone());
                if observation.channel == ChannelKind::PublicMarket
                    && observation.health == ChannelHealth::Ready
                {
                    self.covered.insert(VenueScenario::PublicSync);
                }
                if observation.channel == ChannelKind::AuthenticatedUser
                    && observation.health == ChannelHealth::Ready
                {
                    self.covered.insert(VenueScenario::UserSync);
                }
                Ok(VenueDetail::ChannelAccepted)
            }
            VenueCommand::ObserveParameters {
                parameters,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Parameters)?;
                if self.recovery.is_some()
                    || !valid_parameters(parameters, plan, &self.policy, *recorded_at_ns)
                    || self.parameters.as_ref().is_some_and(|prior| {
                        parameters.version != prior.version.checked_add(1).unwrap_or(u64::MAX)
                            || parameters.observed_at_ns < prior.observed_at_ns
                    })
                {
                    return Err(Error::Parameters);
                }
                self.parameters = Some(parameters.clone());
                self.covered.insert(VenueScenario::DynamicParameters);
                Ok(VenueDetail::ParametersAccepted)
            }
            VenueCommand::ObserveMode {
                mode,
                recorded_at_ns,
                ..
            } => {
                if self.recovery.is_some()
                    || !valid_mode(mode, &self.policy, *recorded_at_ns)
                    || self.mode.as_ref().is_some_and(|prior| {
                        mode.sequence != prior.sequence.checked_add(1).unwrap_or(u64::MAX)
                            || mode.observed_at_ns < prior.observed_at_ns
                    })
                {
                    return Err(Error::Mode);
                }
                self.used_observations.insert(mode.observation_id);
                match mode.mode {
                    VenueMode::Normal => {
                        self.covered.insert(VenueScenario::NormalMode);
                    }
                    VenueMode::PostOnly => {
                        self.covered.insert(VenueScenario::PostOnly);
                    }
                    VenueMode::CancelOnly => {
                        self.covered.insert(VenueScenario::CancelOnly);
                    }
                    VenueMode::Restarting => {
                        let prior_epoch = self.current_epoch();
                        let requirement = self.invalidate(
                            RecoveryReason::Restart,
                            mode.observation_digest,
                            prior_epoch,
                        )?;
                        self.mode = Some(mode.clone());
                        return Ok(VenueDetail::RecoveryRequired(Box::new(requirement)));
                    }
                    VenueMode::TradingDisabled | VenueMode::Recovering | VenueMode::Unknown => {}
                }
                self.mode = Some(mode.clone());
                Ok(VenueDetail::ModeAccepted)
            }
            VenueCommand::ObserveRateLimit {
                observation_id,
                backoff_ns,
                automatic_retry_attempted,
                observed_at_ns,
                recorded_at_ns,
                ..
            } => {
                if *observation_id == [0; 32]
                    || self.used_observations.contains(observation_id)
                    || *backoff_ns <= 0
                    || *backoff_ns > self.policy.maximum_backoff_ns
                    || *automatic_retry_attempted
                    || *observed_at_ns > *recorded_at_ns
                {
                    return Err(Error::RateLimit);
                }
                self.used_observations.insert(*observation_id);
                self.covered.insert(VenueScenario::RateLimitBackoff);
                Ok(VenueDetail::RateLimitAccepted)
            }
            VenueCommand::FailChannel {
                channel,
                failure_digest,
                observed_at_ns,
                recorded_at_ns,
                ..
            } => {
                let prior = self.channels.get(channel).ok_or(Error::Recovery)?;
                if *failure_digest == [0; 32]
                    || *observed_at_ns < prior.observed_at_ns
                    || *observed_at_ns > *recorded_at_ns
                    || self.recovery.is_some()
                {
                    return Err(Error::Recovery);
                }
                let prior_epoch = self.current_epoch();
                let requirement =
                    self.invalidate(RecoveryReason::ChannelFailure, *failure_digest, prior_epoch)?;
                self.covered
                    .insert(VenueScenario::IndependentChannelFailure);
                Ok(VenueDetail::RecoveryRequired(Box::new(requirement)))
            }
            VenueCommand::Recover {
                requirement,
                evidence,
                recorded_at_ns,
                ..
            } => {
                let current = self.recovery.as_ref().ok_or(Error::Recovery)?;
                let plan = self.plan.as_ref().ok_or(Error::Recovery)?;
                if **requirement != *current
                    || !requirement.verify_digest()
                    || self.used_recoveries.contains(&evidence.recovery_id)
                    || !valid_recovery(evidence, requirement, plan, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Recovery);
                }
                self.channels = evidence
                    .channel_snapshots
                    .iter()
                    .cloned()
                    .map(|v| (v.channel, v))
                    .collect();
                self.parameters = Some(evidence.parameters.clone());
                self.mode = Some(evidence.mode.clone());
                self.used_recoveries.insert(evidence.recovery_id);
                self.recovery = None;
                self.covered.insert(VenueScenario::ReconnectRecovery);
                if requirement.reason == RecoveryReason::Restart {
                    self.covered.insert(VenueScenario::RestartRecovery);
                }
                Ok(VenueDetail::Recovered)
            }
            VenueCommand::Finalize {
                report_id,
                finalized_at_ns,
                recorded_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                if *report_id == [0; 32]
                    || self.recovery.is_some()
                    || !self.observation_ready(*finalized_at_ns)
                    || !plan
                        .required_scenarios
                        .iter()
                        .all(|s| self.covered.contains(s))
                    || *finalized_at_ns > *recorded_at_ns
                {
                    return Err(Error::Finalize);
                }
                let mut report = VenueReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    security_report_digest: plan.security_report.report_digest,
                    final_epoch: self.current_epoch(),
                    final_parameter_version: self
                        .parameters
                        .as_ref()
                        .ok_or(Error::Finalize)?
                        .version,
                    covered_scenarios: self.covered.iter().copied().collect(),
                    finalized_at_ns: *finalized_at_ns,
                    status: VenueReportStatus::LocallyCertified,
                    live_environment_certified: false,
                    credential_material_created: false,
                    authenticated_session_opened: false,
                    order_endpoint_present: false,
                    cancel_endpoint_present: false,
                    order_submitted: false,
                    cancellation_submitted: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    submission_authority_granted: false,
                    report_digest: [0; 32],
                };
                report.report_digest = digest_without(b"venue-report-v1", &report, |v| {
                    v.report_digest = [0; 32];
                });
                self.report = Some(report.clone());
                Ok(VenueDetail::Finalized(Box::new(report)))
            }
        }
    }

    fn invalidate(
        &mut self,
        reason: RecoveryReason,
        trigger_digest: [u8; 32],
        prior_epoch: u64,
    ) -> Result<VenueRecoveryRequirement, Error> {
        if self.recovery.is_some() || trigger_digest == [0; 32] {
            return Err(Error::Recovery);
        }
        self.channels.clear();
        self.parameters = None;
        let mut requirement = VenueRecoveryRequirement {
            reason,
            trigger_digest,
            prior_epoch,
            requirement_digest: [0; 32],
        };
        requirement.requirement_digest =
            digest_without(b"venue-recovery-requirement-v1", &requirement, |v| {
                v.requirement_digest = [0; 32];
            });
        self.recovery = Some(requirement.clone());
        Ok(requirement)
    }
    fn current_epoch(&self) -> u64 {
        self.channels.values().map(|v| v.epoch).max().unwrap_or(0)
    }
    fn observation_ready(&self, at: i64) -> bool {
        self.recovery.is_none()
            && ChannelKind::ALL.iter().all(|kind| {
                self.channels.get(kind).is_some_and(|v| {
                    v.health == ChannelHealth::Ready
                        && at
                            .checked_sub(v.observed_at_ns)
                            .is_some_and(|age| age <= self.policy.maximum_channel_age_ns)
                })
            })
            && self
                .channels
                .values()
                .map(|v| v.epoch)
                .collect::<BTreeSet<_>>()
                .len()
                == 1
            && self.parameters.as_ref().is_some_and(|v| {
                at.checked_sub(v.observed_at_ns)
                    .is_some_and(|age| age <= self.policy.maximum_parameter_age_ns)
            })
            && self.mode.as_ref().is_some_and(|v| {
                matches!(
                    v.mode,
                    VenueMode::Normal
                        | VenueMode::PostOnly
                        | VenueMode::CancelOnly
                        | VenueMode::TradingDisabled
                ) && at
                    .checked_sub(v.observed_at_ns)
                    .is_some_and(|age| age <= self.policy.maximum_mode_age_ns)
            })
    }
    #[must_use]
    pub fn snapshot(&self, at: i64) -> VenueSnapshot {
        VenueSnapshot {
            channels: self.channels.clone(),
            parameters: self.parameters.clone(),
            mode: self.mode.clone(),
            recovery: self.recovery.clone(),
            observation_ready: self.observation_ready(at),
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
        let mut h = blake3::Hasher::new();
        h.update(b"read-only-venue-state-v1");
        hash_value(
            &mut h,
            &(
                &self.policy,
                &self.plan,
                &self.channels,
                &self.parameters,
                &self.mode,
                &self.recovery,
                &self.covered,
                &self.used_observations,
                &self.used_recoveries,
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

fn validate_policy(v: &VenuePolicy) -> Result<(), Error> {
    if v.maximum_security_report_age_ns <= 0
        || v.maximum_plan_lifetime_ns <= 0
        || v.maximum_channel_age_ns <= 0
        || v.maximum_parameter_age_ns <= 0
        || v.maximum_mode_age_ns <= 0
        || v.maximum_backoff_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}
fn valid_upstream(v: &SecurityReport, policy: &VenuePolicy, at: i64) -> bool {
    v.verify_digest()
        && v.status == SecurityReportStatus::LocallyCertified
        && v.covered_scenarios == SecurityScenario::ALL
        && v.covered_providers == ProviderClass::ALL
        && !v.real_provider_certified
        && !v.secret_material_created
        && !v.signature_produced
        && !v.provider_contacted
        && !v.socket_opened
        && !v.signer_activated
        && !v.deployment_authority_granted
        && !v.trading_authority_granted
        && !v.submission_authority_granted
        && at
            .checked_sub(v.finalized_at_ns)
            .is_some_and(|age| age <= policy.maximum_security_report_age_ns)
}
fn valid_plan(v: &VenuePlan, policy: &VenuePolicy, at: i64) -> bool {
    v.verify_digest(policy)
        && v.plan_id != [0; 32]
        && valid_contract(&v.authenticated_contract)
        && v.condition_id_digest != [0; 32]
        && v.up_token_digest != [0; 32]
        && v.down_token_digest != [0; 32]
        && v.up_token_digest != v.down_token_digest
        && v.required_scenarios == VenueScenario::ALL
        && v.created_at_ns <= at
        && v.expires_at_ns > at
        && v.expires_at_ns
            <= v.created_at_ns
                .checked_add(policy.maximum_plan_lifetime_ns)
                .unwrap_or(i64::MIN)
}
fn valid_contract(v: &AuthenticatedObservationContract) -> bool {
    v.verify_digest()
        && v.host_digest != [0; 32]
        && v.channel_subject_digest != [0; 32]
        && v.allowed_events == UserEventClass::ALL
        && v.subscription_only
        && !v.credential_value_present
        && !v.authorization_header_present
        && !v.order_endpoint_present
        && !v.cancel_endpoint_present
        && !v.wallet_endpoint_present
        && !v.arbitrary_request_allowed
}
fn valid_channel(v: &ChannelObservation, policy: &VenuePolicy, plan: &VenuePlan, at: i64) -> bool {
    v.verify_digest()
        && v.observation_id != [0; 32]
        && v.epoch > 0
        && v.sequence > 0
        && v.snapshot_digest != [0; 32]
        && v.provenance_digest != [0; 32]
        && v.received_time_ns >= v.event_time_ns
        && v.observed_at_ns >= v.received_time_ns
        && v.observed_at_ns >= plan.created_at_ns
        && v.observed_at_ns <= at
        && at
            .checked_sub(v.observed_at_ns)
            .is_some_and(|age| age <= policy.maximum_channel_age_ns)
}
fn valid_parameters(v: &MarketParameters, plan: &VenuePlan, policy: &VenuePolicy, at: i64) -> bool {
    v.verify_digest()
        && v.condition_id_digest == plan.condition_id_digest
        && v.up_token_digest == plan.up_token_digest
        && v.down_token_digest == plan.down_token_digest
        && v.version > 0
        && v.tick_size_micros > 0
        && v.tick_size_micros <= 1_000_000
        && v.minimum_order_quantity_micros > 0
        && v.maker_fee_bps <= 10_000
        && v.taker_fee_bps <= 10_000
        && v.taker_delay_ns >= 0
        && v.minimum_order_age_ns >= 0
        && v.observed_at_ns >= plan.created_at_ns
        && v.observed_at_ns <= at
        && at
            .checked_sub(v.observed_at_ns)
            .is_some_and(|age| age <= policy.maximum_parameter_age_ns)
}
fn valid_mode(v: &ModeObservation, policy: &VenuePolicy, at: i64) -> bool {
    v.verify_digest()
        && v.observation_id != [0; 32]
        && v.sequence > 0
        && v.source_digest != [0; 32]
        && v.observed_at_ns <= at
        && at
            .checked_sub(v.observed_at_ns)
            .is_some_and(|age| age <= policy.maximum_mode_age_ns)
}
fn valid_recovery(
    v: &VenueRecoveryEvidence,
    requirement: &VenueRecoveryRequirement,
    plan: &VenuePlan,
    policy: &VenuePolicy,
    at: i64,
) -> bool {
    let epoch = requirement.prior_epoch.checked_add(1).unwrap_or(u64::MAX);
    v.verify_digest()
        && v.recovery_id != [0; 32]
        && v.requirement_digest == requirement.requirement_digest
        && v.channel_snapshots.len() == ChannelKind::ALL.len()
        && v.channel_snapshots
            .iter()
            .map(|c| c.channel)
            .eq(ChannelKind::ALL)
        && v.channel_snapshots.iter().all(|c| {
            c.epoch == epoch
                && c.sequence == 1
                && c.health == ChannelHealth::Ready
                && valid_channel(c, policy, plan, at)
        })
        && v.parameters.version > 1
        && valid_parameters(&v.parameters, plan, policy, at)
        && v.mode.mode == VenueMode::Normal
        && valid_mode(&v.mode, policy, at)
        && v.reconciliation_digest != [0; 32]
        && v.observed_at_ns <= at
        && v.no_mutation_observed
        && !v.credential_value_present
        && !v.order_submitted
        && !v.cancellation_submitted
}

/// Encodes one bounded versioned venue command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &VenueCommand) -> Result<Vec<u8>, Error> {
    let body = serde_json::to_vec(command).map_err(|e| Error::Json(e.to_string()))?;
    if body.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut bytes = Vec::with_capacity(body.len() + 2);
    bytes.extend_from_slice(&WIRE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}
/// Decodes one bounded versioned venue command.
///
/// # Errors
///
/// Rejects size, version, JSON, unknown fields, or trailing bytes.
pub fn decode_command(bytes: &[u8]) -> Result<VenueCommand, Error> {
    if bytes.len() < 2 || bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let version = u16::from_le_bytes(bytes[..2].try_into().map_err(|_| Error::CommandBound)?);
    if version != WIRE_VERSION {
        return Err(Error::Version(version));
    }
    let mut d = serde_json::Deserializer::from_slice(&bytes[2..]);
    let c = VenueCommand::deserialize(&mut d).map_err(|e| Error::Json(e.to_string()))?;
    d.end().map_err(|e| Error::Json(e.to_string()))?;
    Ok(c)
}
fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(&serde_json::to_vec(value).expect("serializable venue state"));
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
fn hash_value<T: Serialize>(h: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable venue state");
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(&bytes);
}

#[cfg(test)]
mod tests;
