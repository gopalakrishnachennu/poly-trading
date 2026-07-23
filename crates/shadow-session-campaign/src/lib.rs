#![forbid(unsafe_code)]

//! Deterministic multi-session campaigns over the credentialless shadow gateway.
//!
//! This crate replays recorded and synthetic inputs only. It cannot load a
//! credential, sign, authenticate, access a wallet or RPC endpoint, contact a
//! relayer, deploy, promote, or submit a live order or transaction.

mod durable;
mod evidence;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, CampaignCheckpoint,
    CampaignRecovery, DurableCampaignRunner, StorageError,
};
pub use evidence::{read_evidence_bundle, write_evidence_bundle_create_new, EvidenceFileError};

use serde::{Deserialize, Serialize};
use shadow_gateway_harness::{
    GatewayCommand, GatewayConfig, GatewayDetail, GatewayMode, GatewayOutcome, ShadowGatewayHarness,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 4 * 1024 * 1024;
const MAX_SESSIONS_HARD: usize = 512;
const MAX_REQUIRED_SCENARIOS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CampaignCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignPolicy {
    pub max_sessions: usize,
    pub max_steps: u64,
    pub minimum_duration_ns: i64,
    pub minimum_sessions: usize,
    pub maximum_step_gap_ns: i64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedSession {
    pub session_id: [u8; 32],
    pub start_ns: i64,
    pub end_ns: i64,
    pub recording_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredScenario {
    CertificationRenewal,
    CertificationExpiry,
    MarketPartition,
    UserPartition,
    DeadMan,
    HeartbeatLoss,
    Restart,
    UnknownStateRecovery,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignManifest {
    pub campaign_id: [u8; 32],
    pub start_ns: i64,
    pub end_ns: i64,
    pub sessions: Vec<RecordedSession>,
    pub required_scenarios: Vec<RequiredScenario>,
    pub expected_step_count: u64,
    pub expected_schedule_digest: [u8; 32],
    pub manifest_digest: [u8; 32],
}

impl CampaignManifest {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.manifest_digest = manifest_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.manifest_digest == manifest_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CampaignAction {
    OpenSession {
        session_id: [u8; 32],
    },
    Gateway {
        session_id: Option<[u8; 32]>,
        command: Box<GatewayCommand>,
    },
    CloseSession {
        session_id: [u8; 32],
        replay_digest: [u8; 32],
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignStep {
    pub sequence: u64,
    pub scheduled_at_ns: i64,
    pub previous_step_digest: [u8; 32],
    pub action: CampaignAction,
    pub step_digest: [u8; 32],
}

impl CampaignStep {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.step_digest = step_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.step_digest == step_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignStatus {
    PromotionEligible,
    NotEligible,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum EvidenceReason {
    StepsIncomplete,
    ScheduleDigestMismatch,
    SessionsIncomplete,
    ScenarioMissing(RequiredScenario),
    GatewayHalted,
    GatewayNotReady,
    UnresolvedBacking,
    UnresolvedConversion,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct OperatorEvidenceBundle {
    pub bundle_id: [u8; 32],
    pub campaign_id: [u8; 32],
    pub manifest_digest: [u8; 32],
    pub schedule_digest: [u8; 32],
    pub evaluated_at_ns: i64,
    pub status: CampaignStatus,
    pub reasons: Vec<EvidenceReason>,
    pub required_scenarios: Vec<RequiredScenario>,
    pub covered_scenarios: Vec<RequiredScenario>,
    pub session_count: usize,
    pub completed_session_count: usize,
    pub applied_step_count: u64,
    pub certification_install_count: u64,
    pub dead_man_count: u64,
    pub restart_count: u64,
    pub unknown_recovery_count: u64,
    pub initial_gateway_digest: [u8; 32],
    pub final_gateway_digest: [u8; 32],
    pub final_cash_reserved_micros: i128,
    pub final_pending_conversion_count: usize,
    pub final_gateway_ready: bool,
    pub operator_decision_required: bool,
    pub promotion_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub bundle_digest: [u8; 32],
}

impl OperatorEvidenceBundle {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.bundle_digest == evidence_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CampaignCommand {
    Register {
        command_id: CampaignCommandId,
        manifest: CampaignManifest,
        recorded_at_ns: i64,
    },
    ApplyStep {
        command_id: CampaignCommandId,
        campaign_id: [u8; 32],
        step: Box<CampaignStep>,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: CampaignCommandId,
        campaign_id: [u8; 32],
        bundle_id: [u8; 32],
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl CampaignCommand {
    #[must_use]
    pub const fn command_id(&self) -> CampaignCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::ApplyStep { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::ApplyStep { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CampaignDetail {
    Registered,
    SessionOpened { session_id: [u8; 32] },
    GatewayApplied(GatewayOutcome),
    SessionClosed { session_id: [u8; 32] },
    Finalized(Box<OperatorEvidenceBundle>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignOutcome {
    pub command_id: CampaignCommandId,
    pub detail: CampaignDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CampaignSnapshot {
    pub accepted_commands: u64,
    pub campaign_id: Option<[u8; 32]>,
    pub applied_step_count: u64,
    pub completed_session_count: usize,
    pub active_session: Option<[u8; 32]>,
    pub covered_scenarios: BTreeSet<RequiredScenario>,
    pub last_bundle: Option<OperatorEvidenceBundle>,
    pub gateway_digest: [u8; 32],
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("campaign configuration is invalid")]
    Config,
    #[error("campaign timestamp is invalid or regressed")]
    Timestamp,
    #[error("campaign command exceeds its canonical bound")]
    CommandBound,
    #[error("campaign command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported campaign command version: {0}")]
    Version(u16),
    #[error("campaign command id was reused for different content")]
    IdempotencyConflict,
    #[error("campaign identity was reused or substituted")]
    Identity,
    #[error("campaign manifest is invalid")]
    Manifest,
    #[error("campaign step chain, sequence, or schedule is invalid")]
    Schedule,
    #[error("campaign session lifecycle is invalid")]
    Session,
    #[error("campaign is already finalized")]
    Finalized,
    #[error("shadow gateway failed: {0}")]
    Gateway(String),
    #[error("campaign arithmetic or counter overflow")]
    Overflow,
    #[error("campaign runner is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct ShadowSessionCampaign {
    policy: CampaignPolicy,
    gateway: ShadowGatewayHarness,
    manifest: Option<CampaignManifest>,
    sessions: BTreeMap<[u8; 32], RecordedSession>,
    active_session: Option<[u8; 32]>,
    opened_sessions: BTreeSet<[u8; 32]>,
    completed_sessions: BTreeMap<[u8; 32], [u8; 32]>,
    covered_scenarios: BTreeSet<RequiredScenario>,
    schedule_head: [u8; 32],
    applied_steps: u64,
    last_step_at_ns: Option<i64>,
    certification_installs: u64,
    dead_man_count: u64,
    restart_count: u64,
    unknown_recovery_count: u64,
    unknown_outstanding: bool,
    initial_gateway_digest: Option<[u8; 32]>,
    bundles: BTreeMap<[u8; 32], OperatorEvidenceBundle>,
    last_bundle: Option<OperatorEvidenceBundle>,
    processed: BTreeMap<CampaignCommandId, ([u8; 32], CampaignOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl ShadowSessionCampaign {
    /// Creates an empty offline campaign owner.
    ///
    /// # Errors
    ///
    /// Rejects invalid campaign, gateway, or reconciliation configuration.
    pub fn new(
        policy: CampaignPolicy,
        gateway: GatewayConfig,
        reconciliation: settlement_reconciliation::ReconcilerConfig,
    ) -> Result<Self, Error> {
        validate_policy(&policy)?;
        let gateway = ShadowGatewayHarness::new(gateway, reconciliation)
            .map_err(|error| Error::Gateway(error.to_string()))?;
        Ok(Self {
            policy,
            gateway,
            manifest: None,
            sessions: BTreeMap::new(),
            active_session: None,
            opened_sessions: BTreeSet::new(),
            completed_sessions: BTreeMap::new(),
            covered_scenarios: BTreeSet::new(),
            schedule_head: [0; 32],
            applied_steps: 0,
            last_step_at_ns: None,
            certification_installs: 0,
            dead_man_count: 0,
            restart_count: 0,
            unknown_recovery_count: 0,
            unknown_outstanding: false,
            initial_gateway_digest: None,
            bundles: BTreeMap::new(),
            last_bundle: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic campaign command.
    ///
    /// # Errors
    ///
    /// Identity, schedule, lifecycle, nested, arithmetic, or durability errors halt.
    pub fn apply(&mut self, command: &CampaignCommand) -> Result<CampaignOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        if command.recorded_at_ns() < 0 {
            return self.halt(Error::Timestamp);
        }
        let encoded = encode_command(command)?;
        let content = *blake3::hash(&encoded).as_bytes();
        let id = command.command_id();
        if let Some((existing, outcome)) = self.processed.get(&id) {
            if *existing == content {
                return Ok(outcome.clone());
            }
            return self.halt(Error::IdempotencyConflict);
        }
        if self
            .last_recorded_at_ns
            .is_some_and(|previous| command.recorded_at_ns() < previous)
        {
            return self.halt(Error::Timestamp);
        }
        let mut next = self.clone();
        let detail = match next.apply_fresh(command) {
            Ok(value) => value,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = next
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        let mut outcome = CampaignOutcome {
            command_id: id,
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = outcome_digest(&outcome);
        next.last_recorded_at_ns = Some(command.recorded_at_ns());
        next.processed.insert(id, (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    fn apply_fresh(&mut self, command: &CampaignCommand) -> Result<CampaignDetail, Error> {
        if self.last_bundle.is_some() {
            return Err(Error::Finalized);
        }
        match command {
            CampaignCommand::Register {
                manifest,
                recorded_at_ns,
                ..
            } => self.register(manifest, *recorded_at_ns),
            CampaignCommand::ApplyStep {
                campaign_id,
                step,
                recorded_at_ns,
                ..
            } => self.apply_step(*campaign_id, step, *recorded_at_ns),
            CampaignCommand::Finalize {
                campaign_id,
                bundle_id,
                evaluated_at_ns,
                recorded_at_ns,
                ..
            } => self.finalize(*campaign_id, *bundle_id, *evaluated_at_ns, *recorded_at_ns),
        }
    }

    fn register(&mut self, manifest: &CampaignManifest, at: i64) -> Result<CampaignDetail, Error> {
        if self.manifest.is_some() {
            return Err(Error::Identity);
        }
        validate_manifest(manifest, &self.policy)?;
        if at > manifest.start_ns {
            return Err(Error::Timestamp);
        }
        self.sessions = manifest
            .sessions
            .iter()
            .cloned()
            .map(|session| (session.session_id, session))
            .collect();
        self.initial_gateway_digest = Some(self.gateway.snapshot().digest);
        self.manifest = Some(manifest.clone());
        Ok(CampaignDetail::Registered)
    }

    fn apply_step(
        &mut self,
        campaign_id: [u8; 32],
        step: &CampaignStep,
        at: i64,
    ) -> Result<CampaignDetail, Error> {
        let manifest = self.manifest.as_ref().ok_or(Error::Manifest)?;
        if campaign_id != manifest.campaign_id
            || step.sequence != self.applied_steps + 1
            || step.scheduled_at_ns != at
            || step.previous_step_digest != self.schedule_head
            || !step.verify_digest()
            || at < manifest.start_ns
            || at > manifest.end_ns
            || step.sequence > manifest.expected_step_count
        {
            return Err(Error::Schedule);
        }
        if self.last_step_at_ns.is_some_and(|previous| {
            at < previous || at - previous > self.policy.maximum_step_gap_ns
        }) {
            return Err(Error::Schedule);
        }
        let detail = match &step.action {
            CampaignAction::OpenSession { session_id } => self.open_session(*session_id, at)?,
            CampaignAction::Gateway {
                session_id,
                command,
            } => self.apply_gateway(*session_id, command, at)?,
            CampaignAction::CloseSession {
                session_id,
                replay_digest,
            } => self.close_session(*session_id, *replay_digest, at)?,
        };
        self.applied_steps = step.sequence;
        self.schedule_head = step.step_digest;
        self.last_step_at_ns = Some(at);
        Ok(detail)
    }

    fn open_session(&mut self, session_id: [u8; 32], at: i64) -> Result<CampaignDetail, Error> {
        let session = self.sessions.get(&session_id).ok_or(Error::Session)?;
        if self.active_session.is_some()
            || self.opened_sessions.contains(&session_id)
            || at != session.start_ns
        {
            return Err(Error::Session);
        }
        self.opened_sessions.insert(session_id);
        self.active_session = Some(session_id);
        Ok(CampaignDetail::SessionOpened { session_id })
    }

    fn apply_gateway(
        &mut self,
        session_id: Option<[u8; 32]>,
        command: &GatewayCommand,
        at: i64,
    ) -> Result<CampaignDetail, Error> {
        if command.recorded_at_ns() != at {
            return Err(Error::Timestamp);
        }
        let runtime_replay = matches!(command, GatewayCommand::ApplyRuntime { .. });
        match session_id {
            Some(session) if self.active_session == Some(session) => {
                let bounds = self.sessions.get(&session).ok_or(Error::Session)?;
                if at < bounds.start_ns || at >= bounds.end_ns {
                    return Err(Error::Session);
                }
            }
            Some(_) => return Err(Error::Session),
            None if runtime_replay => return Err(Error::Session),
            None => {}
        }
        let outcome = self
            .gateway
            .apply(command)
            .map_err(|error| Error::Gateway(error.to_string()))?;
        self.derive_coverage(command, &outcome)?;
        Ok(CampaignDetail::GatewayApplied(outcome))
    }

    fn close_session(
        &mut self,
        session_id: [u8; 32],
        replay_digest: [u8; 32],
        at: i64,
    ) -> Result<CampaignDetail, Error> {
        let session = self.sessions.get(&session_id).ok_or(Error::Session)?;
        if self.active_session != Some(session_id)
            || self.completed_sessions.contains_key(&session_id)
            || at != session.end_ns
            || replay_digest != session.recording_digest
        {
            return Err(Error::Session);
        }
        self.active_session = None;
        self.completed_sessions.insert(session_id, replay_digest);
        Ok(CampaignDetail::SessionClosed { session_id })
    }

    fn derive_coverage(
        &mut self,
        command: &GatewayCommand,
        outcome: &GatewayOutcome,
    ) -> Result<(), Error> {
        match command {
            GatewayCommand::InstallCertification { .. } => {
                self.certification_installs = self
                    .certification_installs
                    .checked_add(1)
                    .ok_or(Error::Overflow)?;
                if self.certification_installs >= 2 {
                    self.covered_scenarios
                        .insert(RequiredScenario::CertificationRenewal);
                }
            }
            GatewayCommand::ObserveHeartbeat { heartbeat, .. } => {
                if !heartbeat.market_feed_healthy {
                    self.covered_scenarios
                        .insert(RequiredScenario::MarketPartition);
                }
                if !heartbeat.user_feed_healthy {
                    self.covered_scenarios
                        .insert(RequiredScenario::UserPartition);
                }
            }
            GatewayCommand::ApplyFixture { fixture, .. } => match fixture.kind {
                shadow_adapter_certification::FixtureKind::HeartbeatLost => {
                    self.covered_scenarios
                        .insert(RequiredScenario::HeartbeatLoss);
                }
                shadow_adapter_certification::FixtureKind::Restart425 => {
                    self.restart_count =
                        self.restart_count.checked_add(1).ok_or(Error::Overflow)?;
                    self.covered_scenarios.insert(RequiredScenario::Restart);
                }
                shadow_adapter_certification::FixtureKind::UnknownOrder => {
                    self.unknown_outstanding = true;
                }
                _ => {}
            },
            _ => {}
        }
        match &outcome.detail {
            GatewayDetail::HeartbeatObserved {
                dead_man_triggered: true,
                ..
            }
            | GatewayDetail::TickApplied {
                dead_man_triggered: true,
                ..
            } => {
                self.dead_man_count = self.dead_man_count.checked_add(1).ok_or(Error::Overflow)?;
                self.covered_scenarios.insert(RequiredScenario::DeadMan);
            }
            GatewayDetail::TickApplied {
                certification_expired: true,
                ..
            } => {
                self.covered_scenarios
                    .insert(RequiredScenario::CertificationExpiry);
            }
            GatewayDetail::Recovered { .. } if self.unknown_outstanding => {
                self.unknown_outstanding = false;
                self.unknown_recovery_count = self
                    .unknown_recovery_count
                    .checked_add(1)
                    .ok_or(Error::Overflow)?;
                self.covered_scenarios
                    .insert(RequiredScenario::UnknownStateRecovery);
            }
            _ => {}
        }
        Ok(())
    }

    fn finalize(
        &mut self,
        campaign_id: [u8; 32],
        bundle_id: [u8; 32],
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    ) -> Result<CampaignDetail, Error> {
        let manifest = self.manifest.as_ref().ok_or(Error::Manifest)?;
        if campaign_id != manifest.campaign_id
            || bundle_id == [0; 32]
            || evaluated_at_ns < manifest.end_ns
            || evaluated_at_ns > recorded_at_ns
            || self.bundles.contains_key(&bundle_id)
        {
            return Err(Error::Identity);
        }
        let gateway = self.gateway.snapshot();
        let runtime = self.gateway.runtime().snapshot();
        let required: BTreeSet<_> = manifest.required_scenarios.iter().copied().collect();
        let mut reasons = BTreeSet::new();
        if self.applied_steps != manifest.expected_step_count {
            reasons.insert(EvidenceReason::StepsIncomplete);
        }
        if self.schedule_head != manifest.expected_schedule_digest {
            reasons.insert(EvidenceReason::ScheduleDigestMismatch);
        }
        if self.active_session.is_some() || self.completed_sessions.len() != self.sessions.len() {
            reasons.insert(EvidenceReason::SessionsIncomplete);
        }
        for scenario in required.difference(&self.covered_scenarios) {
            reasons.insert(EvidenceReason::ScenarioMissing(*scenario));
        }
        if gateway.halted {
            reasons.insert(EvidenceReason::GatewayHalted);
        }
        if !gateway.new_shadow_exposure_allowed || gateway.mode != GatewayMode::Ready {
            reasons.insert(EvidenceReason::GatewayNotReady);
        }
        if runtime.cash_reserved_micros != 0 {
            reasons.insert(EvidenceReason::UnresolvedBacking);
        }
        if runtime.pending_conversion_count != 0 {
            reasons.insert(EvidenceReason::UnresolvedConversion);
        }
        let reasons: Vec<_> = reasons.into_iter().collect();
        let mut bundle = OperatorEvidenceBundle {
            bundle_id,
            campaign_id,
            manifest_digest: manifest.manifest_digest,
            schedule_digest: self.schedule_head,
            evaluated_at_ns,
            status: if reasons.is_empty() {
                CampaignStatus::PromotionEligible
            } else {
                CampaignStatus::NotEligible
            },
            reasons,
            required_scenarios: required.into_iter().collect(),
            covered_scenarios: self.covered_scenarios.iter().copied().collect(),
            session_count: self.sessions.len(),
            completed_session_count: self.completed_sessions.len(),
            applied_step_count: self.applied_steps,
            certification_install_count: self.certification_installs,
            dead_man_count: self.dead_man_count,
            restart_count: self.restart_count,
            unknown_recovery_count: self.unknown_recovery_count,
            initial_gateway_digest: self.initial_gateway_digest.ok_or(Error::Manifest)?,
            final_gateway_digest: gateway.digest,
            final_cash_reserved_micros: runtime.cash_reserved_micros,
            final_pending_conversion_count: runtime.pending_conversion_count,
            final_gateway_ready: gateway.new_shadow_exposure_allowed
                && gateway.mode == GatewayMode::Ready,
            operator_decision_required: true,
            promotion_authority_granted: false,
            deployment_authority_granted: false,
            bundle_digest: [0; 32],
        };
        bundle.bundle_digest = evidence_digest(&bundle);
        self.bundles.insert(bundle_id, bundle.clone());
        self.last_bundle = Some(bundle.clone());
        Ok(CampaignDetail::Finalized(Box::new(bundle)))
    }

    #[must_use]
    pub const fn gateway(&self) -> &ShadowGatewayHarness {
        &self.gateway
    }

    #[must_use]
    pub fn snapshot(&self) -> CampaignSnapshot {
        CampaignSnapshot {
            accepted_commands: self.accepted_commands,
            campaign_id: self.manifest.as_ref().map(|value| value.campaign_id),
            applied_step_count: self.applied_steps,
            completed_session_count: self.completed_sessions.len(),
            active_session: self.active_session,
            covered_scenarios: self.covered_scenarios.clone(),
            last_bundle: self.last_bundle.clone(),
            gateway_digest: self.gateway.snapshot().digest,
            halted: self.halted.is_some(),
            halt_reason: self.halted.clone(),
            digest: self.state_digest(),
        }
    }

    #[must_use]
    pub const fn is_halted(&self) -> bool {
        self.halted.is_some()
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"shadow-session-campaign-state-v1");
        hash_json(&mut hasher, &self.policy);
        hasher.update(&self.gateway.snapshot().digest);
        hash_json(&mut hasher, &self.manifest);
        for (id, session) in &self.sessions {
            hasher.update(id);
            hash_json(&mut hasher, session);
        }
        hash_json(&mut hasher, &self.active_session);
        for id in &self.opened_sessions {
            hasher.update(id);
        }
        for (id, digest) in &self.completed_sessions {
            hasher.update(id);
            hasher.update(digest);
        }
        hash_json(&mut hasher, &self.covered_scenarios);
        hasher.update(&self.schedule_head);
        hash_json(&mut hasher, &self.applied_steps);
        hash_json(&mut hasher, &self.last_step_at_ns);
        hash_json(&mut hasher, &self.certification_installs);
        hash_json(&mut hasher, &self.dead_man_count);
        hash_json(&mut hasher, &self.restart_count);
        hash_json(&mut hasher, &self.unknown_recovery_count);
        hash_json(&mut hasher, &self.unknown_outstanding);
        hash_json(&mut hasher, &self.initial_gateway_digest);
        for (id, bundle) in &self.bundles {
            hasher.update(id);
            hash_json(&mut hasher, bundle);
        }
        hash_json(&mut hasher, &self.last_bundle);
        for (id, (content, outcome)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_json(&mut hasher, outcome);
        }
        hash_json(&mut hasher, &self.accepted_commands);
        hash_json(&mut hasher, &self.last_recorded_at_ns);
        hash_json(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }

    fn halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }
}

fn validate_policy(policy: &CampaignPolicy) -> Result<(), Error> {
    if policy.max_sessions == 0
        || policy.max_sessions > MAX_SESSIONS_HARD
        || policy.max_steps == 0
        || policy.minimum_duration_ns <= 0
        || policy.minimum_sessions == 0
        || policy.minimum_sessions > policy.max_sessions
        || policy.maximum_step_gap_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn validate_manifest(manifest: &CampaignManifest, policy: &CampaignPolicy) -> Result<(), Error> {
    let ids: BTreeSet<_> = manifest
        .sessions
        .iter()
        .map(|session| session.session_id)
        .collect();
    let scenarios: BTreeSet<_> = manifest.required_scenarios.iter().copied().collect();
    let ordered = manifest
        .sessions
        .windows(2)
        .all(|pair| pair[0].end_ns <= pair[1].start_ns);
    if !manifest.verify_digest()
        || manifest.campaign_id == [0; 32]
        || manifest.start_ns < 0
        || manifest.end_ns <= manifest.start_ns
        || manifest.end_ns - manifest.start_ns < policy.minimum_duration_ns
        || manifest.sessions.len() < policy.minimum_sessions
        || manifest.sessions.len() > policy.max_sessions
        || ids.len() != manifest.sessions.len()
        || manifest.required_scenarios.is_empty()
        || manifest.required_scenarios.len() > MAX_REQUIRED_SCENARIOS
        || scenarios.len() != manifest.required_scenarios.len()
        || manifest.expected_step_count == 0
        || manifest.expected_step_count > policy.max_steps
        || manifest.expected_schedule_digest == [0; 32]
        || !ordered
        || manifest.sessions.iter().any(|session| {
            session.session_id == [0; 32]
                || session.recording_digest == [0; 32]
                || session.start_ns < manifest.start_ns
                || session.end_ns > manifest.end_ns
                || session.end_ns <= session.start_ns
        })
    {
        Err(Error::Manifest)
    } else {
        Ok(())
    }
}

fn manifest_digest(manifest: &CampaignManifest) -> [u8; 32] {
    let mut clone = manifest.clone();
    clone.manifest_digest = [0; 32];
    digest_json(b"shadow-session-manifest-v1", &clone)
}

fn step_digest(step: &CampaignStep) -> [u8; 32] {
    let mut clone = step.clone();
    clone.step_digest = [0; 32];
    digest_json(b"shadow-session-step-v1", &clone)
}

fn evidence_digest(bundle: &OperatorEvidenceBundle) -> [u8; 32] {
    let mut clone = bundle.clone();
    clone.bundle_digest = [0; 32];
    digest_json(b"shadow-session-evidence-v1", &clone)
}

fn outcome_digest(outcome: &CampaignOutcome) -> [u8; 32] {
    let mut clone = outcome.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"shadow-session-outcome-v1", &clone)
}

fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable campaign state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: CampaignCommand,
}

/// Encodes one bounded versioned campaign command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &CampaignCommand) -> Result<Vec<u8>, Error> {
    let bytes = serde_json::to_vec(&WireCommand {
        version: WIRE_VERSION,
        command: command.clone(),
    })
    .map_err(|error| Error::Json(error.to_string()))?;
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    Ok(bytes)
}

/// Decodes one bounded versioned campaign command.
///
/// # Errors
///
/// Rejects malformed, oversized, trailing, or unsupported input.
pub fn decode_command(bytes: &[u8]) -> Result<CampaignCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let wire = WireCommand::deserialize(&mut deserializer)
        .map_err(|error| Error::Json(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| Error::Json(error.to_string()))?;
    if wire.version != WIRE_VERSION {
        return Err(Error::Version(wire.version));
    }
    Ok(wire.command)
}

#[cfg(test)]
mod tests;
