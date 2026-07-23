#![forbid(unsafe_code)]

//! Deterministic offline fleet-rollout evidence and release revocation.
//!
//! This crate produces evidence only. It cannot deploy, route, allocate capital,
//! execute rollback, authenticate, sign, access RPC/wallet state, or trade live.

mod dossier;
mod durable;

pub use dossier::{read_dossier, write_dossier_create_new, DossierFileError};
pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DurableFleetGovernance,
    FleetCheckpoint, FleetRecovery, FleetStorageError,
};

use canary_rollout_simulator::{RollbackTrigger, RolloutReport, RolloutReportStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVIDENCE_HARD: usize = 1_024;
const MAX_REGIONS_HARD: usize = 128;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct FleetCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetPolicy {
    pub maximum_evidence: usize,
    pub maximum_regions: usize,
    pub minimum_regions: usize,
    pub minimum_abort_drills: u64,
    pub minimum_rollback_drills: u64,
    pub maximum_report_age_ns: i64,
    pub maximum_campaign_age_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeFreeze {
    pub freeze_id: [u8; 32],
    pub release_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
    pub starts_at_ns: i64,
    pub ends_at_ns: i64,
    pub emergency_change_forbidden: bool,
    pub freeze_digest: [u8; 32],
}

impl ChangeFreeze {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.freeze_digest = freeze_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.freeze_digest == freeze_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionalEvidence {
    pub evidence_id: [u8; 32],
    pub region: String,
    pub environment_digest: [u8; 32],
    pub report: RolloutReport,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetManifest {
    pub campaign_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub release_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
    pub rollback_digest: [u8; 32],
    pub required_regions: Vec<String>,
    pub required_rollback_triggers: Vec<RollbackTrigger>,
    pub freeze: ChangeFreeze,
    pub evidence: Vec<RegionalEvidence>,
    pub policy_digest: [u8; 32],
    pub evidence_set_digest: [u8; 32],
    pub manifest_digest: [u8; 32],
}

impl FleetManifest {
    #[must_use]
    pub fn sealed(mut self, policy: &FleetPolicy) -> Self {
        self.required_regions.sort();
        self.required_rollback_triggers.sort();
        self.evidence.sort_by(|left, right| {
            (&left.region, left.evidence_id).cmp(&(&right.region, right.evidence_id))
        });
        self.policy_digest = digest_json(b"fleet-governance-policy-v1", policy);
        self.evidence_set_digest = fleet_evidence_digest(&self.evidence);
        self.manifest_digest = manifest_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &FleetPolicy) -> bool {
        self.policy_digest == digest_json(b"fleet-governance-policy-v1", policy)
            && self.evidence_set_digest == fleet_evidence_digest(&self.evidence)
            && self.manifest_digest == manifest_digest(self)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ReadinessReason {
    DuplicateEvidence,
    MissingRegionCompletion(String),
    InsufficientAbortDrills,
    InsufficientRollbackDrills,
    MissingRollbackTrigger(RollbackTrigger),
    StaleReport([u8; 32]),
    ChangeFreezeInactive,
    CampaignExpired,
    ReleaseRevoked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetAggregate {
    pub unique_report_count: u64,
    pub unique_plan_count: u64,
    pub completed_regions: Vec<String>,
    pub abort_drill_count: u64,
    pub rollback_drill_count: u64,
    pub covered_rollback_triggers: Vec<RollbackTrigger>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevocationRecord {
    pub revocation_id: [u8; 32],
    pub release_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
    pub operator_id: [u8; 32],
    pub reason_digest: [u8; 32],
    pub effective_at_ns: i64,
    pub revocation_digest: [u8; 32],
}

impl RevocationRecord {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.revocation_digest = revocation_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.revocation_digest == revocation_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DossierStatus {
    OperationallyReady,
    NotReady,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct OperationalReadinessDossier {
    pub dossier_id: [u8; 32],
    pub campaign_id: [u8; 32],
    pub manifest_digest: [u8; 32],
    pub release_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
    pub rollback_digest: [u8; 32],
    pub freeze_digest: [u8; 32],
    pub evaluated_at_ns: i64,
    pub status: DossierStatus,
    pub reasons: Vec<ReadinessReason>,
    pub aggregate: FleetAggregate,
    pub revocation_digest: Option<[u8; 32]>,
    pub operator_execution_required: bool,
    pub fleet_execution_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub rollback_execution_authority_granted: bool,
    pub credential_authority_granted: bool,
    pub live_trading_authority_granted: bool,
    pub dossier_digest: [u8; 32],
}

impl OperationalReadinessDossier {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.dossier_digest == dossier_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FleetCommand {
    Register {
        command_id: FleetCommandId,
        manifest: Box<FleetManifest>,
        recorded_at_ns: i64,
    },
    Revoke {
        command_id: FleetCommandId,
        campaign_id: [u8; 32],
        revocation: RevocationRecord,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: FleetCommandId,
        campaign_id: [u8; 32],
        dossier_id: [u8; 32],
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl FleetCommand {
    #[must_use]
    pub const fn command_id(&self) -> FleetCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::Revoke { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::Revoke { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum FleetDetail {
    Registered {
        aggregate: FleetAggregate,
        reasons: Vec<ReadinessReason>,
    },
    Revoked,
    Finalized(Box<OperationalReadinessDossier>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetOutcome {
    pub command_id: FleetCommandId,
    pub detail: FleetDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FleetSnapshot {
    pub accepted_commands: u64,
    pub campaign_id: Option<[u8; 32]>,
    pub revoked: bool,
    pub last_dossier: Option<OperationalReadinessDossier>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

/// Digest-bound evidence that a positive dossier is still the current,
/// unrevoked state of this owner at an explicit observation time.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentFleetReadiness {
    pub campaign_id: [u8; 32],
    pub dossier_id: [u8; 32],
    pub dossier_digest: [u8; 32],
    pub release_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
    pub rollback_digest: [u8; 32],
    pub completed_regions: Vec<String>,
    pub governance_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub current_readiness_digest: [u8; 32],
}

impl CurrentFleetReadiness {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.current_readiness_digest == current_readiness_digest(self)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("fleet-governance configuration is invalid")]
    Config,
    #[error("fleet-governance timestamp is invalid or regressed")]
    Timestamp,
    #[error("fleet-governance command exceeds its canonical bound")]
    CommandBound,
    #[error("fleet-governance JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported fleet-governance version: {0}")]
    Version(u16),
    #[error("fleet command id was reused for different content")]
    IdempotencyConflict,
    #[error("fleet manifest identity, digest or bounds are invalid")]
    Manifest,
    #[error("fleet evidence is corrupt, authority-bearing or subject-mismatched")]
    Evidence,
    #[error("change-freeze identity, digest or bounds are invalid")]
    Freeze,
    #[error("release revocation identity or subject is invalid")]
    Revocation,
    #[error("fleet dossier identity or lifecycle is invalid")]
    Dossier,
    #[error("fleet governance is already finalized")]
    Finalized,
    #[error("fleet-governance arithmetic overflow")]
    Overflow,
    #[error("fleet governance is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct FleetRolloutGovernance {
    policy: FleetPolicy,
    manifest: Option<FleetManifest>,
    aggregate: Option<FleetAggregate>,
    base_reasons: BTreeSet<ReadinessReason>,
    revocation: Option<RevocationRecord>,
    dossier: Option<OperationalReadinessDossier>,
    processed: BTreeMap<FleetCommandId, ([u8; 32], FleetOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl FleetRolloutGovernance {
    /// Creates an empty offline fleet-governance owner.
    ///
    /// # Errors
    ///
    /// Rejects invalid regional, evidence, drill, or time bounds.
    pub fn new(policy: FleetPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            manifest: None,
            aggregate: None,
            base_reasons: BTreeSet::new(),
            revocation: None,
            dossier: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic fleet-governance command.
    ///
    /// # Errors
    ///
    /// Integrity, identity, lifecycle, chronology, or arithmetic failures halt.
    pub fn apply(&mut self, command: &FleetCommand) -> Result<FleetOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        if command.recorded_at_ns() < 0
            || self
                .last_recorded_at_ns
                .is_some_and(|previous| command.recorded_at_ns() < previous)
        {
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
        let mut next = self.clone();
        let detail = match next.apply_fresh(command) {
            Ok(detail) => detail,
            Err(error) => return self.halt(error),
        };
        next.accepted_commands = next
            .accepted_commands
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        let mut outcome = FleetOutcome {
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

    fn apply_fresh(&mut self, command: &FleetCommand) -> Result<FleetDetail, Error> {
        match command {
            FleetCommand::Register {
                manifest,
                recorded_at_ns,
                ..
            } => self.register(manifest, *recorded_at_ns),
            FleetCommand::Revoke {
                campaign_id,
                revocation,
                recorded_at_ns,
                ..
            } => self.revoke(*campaign_id, revocation, *recorded_at_ns),
            FleetCommand::Finalize {
                campaign_id,
                dossier_id,
                evaluated_at_ns,
                recorded_at_ns,
                ..
            } => {
                if self.dossier.is_some() {
                    return Err(Error::Finalized);
                }
                self.finalize(*campaign_id, *dossier_id, *evaluated_at_ns, *recorded_at_ns)
            }
        }
    }

    fn register(&mut self, manifest: &FleetManifest, at: i64) -> Result<FleetDetail, Error> {
        if self.manifest.is_some() {
            return Err(Error::Manifest);
        }
        validate_manifest(manifest, &self.policy, at)?;
        let (aggregate, reasons) = aggregate(manifest, &self.policy)?;
        self.manifest = Some(manifest.clone());
        self.aggregate = Some(aggregate.clone());
        self.base_reasons.clone_from(&reasons);
        Ok(FleetDetail::Registered {
            aggregate,
            reasons: reasons.into_iter().collect(),
        })
    }

    fn revoke(
        &mut self,
        campaign_id: [u8; 32],
        revocation: &RevocationRecord,
        at: i64,
    ) -> Result<FleetDetail, Error> {
        let manifest = self.manifest.as_ref().ok_or(Error::Manifest)?;
        if self.revocation.is_some()
            || campaign_id != manifest.campaign_id
            || revocation.revocation_id == [0; 32]
            || revocation.operator_id == [0; 32]
            || revocation.reason_digest == [0; 32]
            || revocation.release_digest != manifest.release_digest
            || revocation.artifacts_digest != manifest.artifacts_digest
            || revocation.effective_at_ns != at
            || !revocation.verify_digest()
        {
            return Err(Error::Revocation);
        }
        self.revocation = Some(revocation.clone());
        // A revocation accepted after readiness makes that dossier historical.
        // The caller must finalize again to obtain an attributable NOT_READY
        // dossier bound to the revocation; there is no interval in which the
        // current snapshot still exposes the previous positive dossier.
        self.dossier = None;
        Ok(FleetDetail::Revoked)
    }

    fn finalize(
        &mut self,
        campaign_id: [u8; 32],
        dossier_id: [u8; 32],
        evaluated_at: i64,
        recorded_at: i64,
    ) -> Result<FleetDetail, Error> {
        let manifest = self.manifest.as_ref().ok_or(Error::Manifest)?;
        if campaign_id != manifest.campaign_id
            || dossier_id == [0; 32]
            || evaluated_at < manifest.created_at_ns
            || evaluated_at > recorded_at
        {
            return Err(Error::Dossier);
        }
        let mut reasons = self.base_reasons.clone();
        if evaluated_at < manifest.freeze.starts_at_ns || evaluated_at >= manifest.freeze.ends_at_ns
        {
            reasons.insert(ReadinessReason::ChangeFreezeInactive);
        }
        if evaluated_at > manifest.expires_at_ns {
            reasons.insert(ReadinessReason::CampaignExpired);
        }
        if self.revocation.is_some() {
            reasons.insert(ReadinessReason::ReleaseRevoked);
        }
        let reasons: Vec<_> = reasons.into_iter().collect();
        let mut dossier = OperationalReadinessDossier {
            dossier_id,
            campaign_id,
            manifest_digest: manifest.manifest_digest,
            release_digest: manifest.release_digest,
            artifacts_digest: manifest.artifacts_digest,
            rollback_digest: manifest.rollback_digest,
            freeze_digest: manifest.freeze.freeze_digest,
            evaluated_at_ns: evaluated_at,
            status: if reasons.is_empty() {
                DossierStatus::OperationallyReady
            } else {
                DossierStatus::NotReady
            },
            reasons,
            aggregate: self.aggregate.clone().ok_or(Error::Manifest)?,
            revocation_digest: self.revocation.as_ref().map(|item| item.revocation_digest),
            operator_execution_required: true,
            fleet_execution_authority_granted: false,
            deployment_authority_granted: false,
            rollback_execution_authority_granted: false,
            credential_authority_granted: false,
            live_trading_authority_granted: false,
            dossier_digest: [0; 32],
        };
        dossier.dossier_digest = dossier_digest(&dossier);
        self.dossier = Some(dossier.clone());
        Ok(FleetDetail::Finalized(Box::new(dossier)))
    }

    #[must_use]
    pub fn snapshot(&self) -> FleetSnapshot {
        FleetSnapshot {
            accepted_commands: self.accepted_commands,
            campaign_id: self.manifest.as_ref().map(|item| item.campaign_id),
            revoked: self.revocation.is_some(),
            last_dossier: self.dossier.clone(),
            halted: self.halted.is_some(),
            halt_reason: self.halted.clone(),
            digest: self.state_digest(),
        }
    }

    #[must_use]
    pub const fn is_halted(&self) -> bool {
        self.halted.is_some()
    }

    /// Captures current positive readiness without granting external authority.
    ///
    /// # Errors
    ///
    /// Rejects halted, revoked, absent, non-ready or time-regressed state.
    pub fn current_readiness(&self, observed_at_ns: i64) -> Result<CurrentFleetReadiness, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        if observed_at_ns < 0
            || self
                .last_recorded_at_ns
                .is_some_and(|last| observed_at_ns < last)
        {
            return Err(Error::Timestamp);
        }
        let dossier = self.dossier.as_ref().ok_or(Error::Dossier)?;
        if self.revocation.is_some()
            || dossier.status != DossierStatus::OperationallyReady
            || !dossier.reasons.is_empty()
            || !dossier.verify_digest()
        {
            return Err(Error::Dossier);
        }
        let mut readiness = CurrentFleetReadiness {
            campaign_id: dossier.campaign_id,
            dossier_id: dossier.dossier_id,
            dossier_digest: dossier.dossier_digest,
            release_digest: dossier.release_digest,
            artifacts_digest: dossier.artifacts_digest,
            rollback_digest: dossier.rollback_digest,
            completed_regions: dossier.aggregate.completed_regions.clone(),
            governance_digest: self.state_digest(),
            observed_at_ns,
            current_readiness_digest: [0; 32],
        };
        readiness.current_readiness_digest = current_readiness_digest(&readiness);
        Ok(readiness)
    }

    fn state_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"fleet-rollout-governance-state-v1");
        hash_json(&mut hasher, &self.policy);
        hash_json(&mut hasher, &self.manifest);
        hash_json(&mut hasher, &self.aggregate);
        hash_json(&mut hasher, &self.base_reasons);
        hash_json(&mut hasher, &self.revocation);
        hash_json(&mut hasher, &self.dossier);
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

fn validate_policy(policy: &FleetPolicy) -> Result<(), Error> {
    if policy.maximum_evidence == 0
        || policy.maximum_evidence > MAX_EVIDENCE_HARD
        || policy.maximum_regions == 0
        || policy.maximum_regions > MAX_REGIONS_HARD
        || policy.minimum_regions == 0
        || policy.minimum_regions > policy.maximum_regions
        || policy.minimum_abort_drills == 0
        || policy.minimum_rollback_drills == 0
        || policy.maximum_report_age_ns <= 0
        || policy.maximum_campaign_age_ns <= 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn validate_manifest(manifest: &FleetManifest, policy: &FleetPolicy, at: i64) -> Result<(), Error> {
    let regions: BTreeSet<_> = manifest.required_regions.iter().cloned().collect();
    let triggers: BTreeSet<_> = manifest
        .required_rollback_triggers
        .iter()
        .copied()
        .collect();
    if manifest.campaign_id == [0; 32]
        || manifest.release_digest == [0; 32]
        || manifest.artifacts_digest == [0; 32]
        || manifest.rollback_digest == [0; 32]
        || manifest.created_at_ns != at
        || manifest.expires_at_ns <= at
        || manifest.expires_at_ns - at > policy.maximum_campaign_age_ns
        || manifest.required_regions.len() < policy.minimum_regions
        || manifest.required_regions.len() > policy.maximum_regions
        || regions.len() != manifest.required_regions.len()
        || manifest
            .required_regions
            .iter()
            .any(|region| !valid_region(region))
        || manifest.required_rollback_triggers.is_empty()
        || triggers.len() != manifest.required_rollback_triggers.len()
        || manifest.evidence.is_empty()
        || manifest.evidence.len() > policy.maximum_evidence
        || !manifest.verify_digest(policy)
        || !manifest
            .required_regions
            .windows(2)
            .all(|pair| pair[0] < pair[1])
        || !manifest
            .required_rollback_triggers
            .windows(2)
            .all(|pair| pair[0] < pair[1])
        || !manifest.evidence.windows(2).all(|pair| {
            (&pair[0].region, pair[0].evidence_id) < (&pair[1].region, pair[1].evidence_id)
        })
    {
        return Err(Error::Manifest);
    }
    validate_freeze(manifest)?;
    let mut evidence_ids = BTreeSet::new();
    for item in &manifest.evidence {
        if item.evidence_id == [0; 32]
            || item.environment_digest == [0; 32]
            || !regions.contains(&item.region)
            || !evidence_ids.insert(item.evidence_id)
        {
            return Err(Error::Evidence);
        }
        validate_report(&item.report, manifest)?;
    }
    Ok(())
}

fn validate_freeze(manifest: &FleetManifest) -> Result<(), Error> {
    let freeze = &manifest.freeze;
    if freeze.freeze_id == [0; 32]
        || freeze.release_digest != manifest.release_digest
        || freeze.artifacts_digest != manifest.artifacts_digest
        || freeze.starts_at_ns > manifest.created_at_ns
        || freeze.ends_at_ns <= manifest.created_at_ns
        || manifest.expires_at_ns > freeze.ends_at_ns
        || !freeze.emergency_change_forbidden
        || !freeze.verify_digest()
    {
        Err(Error::Freeze)
    } else {
        Ok(())
    }
}

fn validate_report(report: &RolloutReport, manifest: &FleetManifest) -> Result<(), Error> {
    let terminal_consistent = match report.status {
        RolloutReportStatus::SimulatedCompleted => {
            report.completed_stage_count > 0
                && report.rollback_trigger.is_none()
                && report.abort_operator_id.is_none()
        }
        RolloutReportStatus::OperatorAborted => {
            report.abort_operator_id.is_some() && report.rollback_trigger.is_none()
        }
        RolloutReportStatus::RollbackRequired => {
            report.rollback_trigger.is_some() && report.abort_operator_id.is_none()
        }
    };
    if report.report_id == [0; 32]
        || report.plan_id == [0; 32]
        || report.plan_digest == [0; 32]
        || report.eligibility_record_digest != manifest.release_digest
        || report.artifacts_digest != manifest.artifacts_digest
        || report.rollback_digest != manifest.rollback_digest
        || report.finalized_at_ns < 0
        || report.finalized_at_ns > manifest.created_at_ns
        || !report.verify_digest()
        || !terminal_consistent
        || !report.operator_execution_required
        || report.rollout_execution_authority_granted
        || report.rollback_execution_authority_granted
        || report.deployment_authority_granted
        || report.credential_authority_granted
        || report.live_trading_authority_granted
    {
        Err(Error::Evidence)
    } else {
        Ok(())
    }
}

fn aggregate(
    manifest: &FleetManifest,
    policy: &FleetPolicy,
) -> Result<(FleetAggregate, BTreeSet<ReadinessReason>), Error> {
    let mut reasons = BTreeSet::new();
    let mut reports = BTreeSet::new();
    let mut plans = BTreeSet::new();
    let mut completed_regions = BTreeSet::new();
    let mut covered_triggers = BTreeSet::new();
    let mut aborts = 0_u64;
    let mut rollbacks = 0_u64;
    for item in &manifest.evidence {
        let report = &item.report;
        if !reports.insert(report.report_digest) || !plans.insert(report.plan_digest) {
            reasons.insert(ReadinessReason::DuplicateEvidence);
            continue;
        }
        if manifest.created_at_ns - report.finalized_at_ns > policy.maximum_report_age_ns {
            reasons.insert(ReadinessReason::StaleReport(report.report_id));
            continue;
        }
        match report.status {
            RolloutReportStatus::SimulatedCompleted => {
                completed_regions.insert(item.region.clone());
            }
            RolloutReportStatus::OperatorAborted => {
                aborts = aborts.checked_add(1).ok_or(Error::Overflow)?;
            }
            RolloutReportStatus::RollbackRequired => {
                rollbacks = rollbacks.checked_add(1).ok_or(Error::Overflow)?;
                covered_triggers.insert(report.rollback_trigger.ok_or(Error::Evidence)?);
            }
        }
    }
    for region in &manifest.required_regions {
        if !completed_regions.contains(region) {
            reasons.insert(ReadinessReason::MissingRegionCompletion(region.clone()));
        }
    }
    if aborts < policy.minimum_abort_drills {
        reasons.insert(ReadinessReason::InsufficientAbortDrills);
    }
    if rollbacks < policy.minimum_rollback_drills {
        reasons.insert(ReadinessReason::InsufficientRollbackDrills);
    }
    for trigger in &manifest.required_rollback_triggers {
        if !covered_triggers.contains(trigger) {
            reasons.insert(ReadinessReason::MissingRollbackTrigger(*trigger));
        }
    }
    let aggregate = FleetAggregate {
        unique_report_count: u64::try_from(reports.len()).map_err(|_| Error::Overflow)?,
        unique_plan_count: u64::try_from(plans.len()).map_err(|_| Error::Overflow)?,
        completed_regions: completed_regions.into_iter().collect(),
        abort_drill_count: aborts,
        rollback_drill_count: rollbacks,
        covered_rollback_triggers: covered_triggers.into_iter().collect(),
    };
    Ok((aggregate, reasons))
}

fn valid_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 64
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn freeze_digest(value: &ChangeFreeze) -> [u8; 32] {
    let mut clone = value.clone();
    clone.freeze_digest = [0; 32];
    digest_json(b"fleet-change-freeze-v1", &clone)
}

fn revocation_digest(value: &RevocationRecord) -> [u8; 32] {
    let mut clone = value.clone();
    clone.revocation_digest = [0; 32];
    digest_json(b"fleet-release-revocation-v1", &clone)
}

fn fleet_evidence_digest(value: &[RegionalEvidence]) -> [u8; 32] {
    digest_json(b"fleet-regional-evidence-v1", value)
}

fn manifest_digest(value: &FleetManifest) -> [u8; 32] {
    let mut clone = value.clone();
    clone.manifest_digest = [0; 32];
    digest_json(b"fleet-manifest-v1", &clone)
}

fn dossier_digest(value: &OperationalReadinessDossier) -> [u8; 32] {
    let mut clone = value.clone();
    clone.dossier_digest = [0; 32];
    digest_json(b"fleet-readiness-dossier-v1", &clone)
}

fn current_readiness_digest(value: &CurrentFleetReadiness) -> [u8; 32] {
    let mut clone = value.clone();
    clone.current_readiness_digest = [0; 32];
    digest_json(b"fleet-current-readiness-v1", &clone)
}

fn outcome_digest(value: &FleetOutcome) -> [u8; 32] {
    let mut clone = value.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"fleet-outcome-v1", &clone)
}

fn digest_json<T: Serialize + ?Sized>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize + ?Sized>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable fleet state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: FleetCommand,
}

/// Encodes one bounded versioned fleet command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &FleetCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one bounded versioned fleet command.
///
/// # Errors
///
/// Rejects malformed, oversized, trailing or unsupported input.
pub fn decode_command(bytes: &[u8]) -> Result<FleetCommand, Error> {
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
