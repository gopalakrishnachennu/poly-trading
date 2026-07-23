#![forbid(unsafe_code)]

//! Deterministic offline deployment-package and operator-ceremony preflight.
//!
//! This crate produces evidence only. It has no credential, signer, network,
//! cloud-control, deployment, rollback-execution, wallet, RPC, or trading path.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, DeploymentCheckpoint,
    DeploymentRecovery, DeploymentStorageError, DurableDeploymentPreflight,
};
pub use report::{read_report, write_report_create_new, PreflightReportFileError};

use fleet_rollout_governance::CurrentFleetReadiness;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_REGIONS_HARD: usize = 128;
const MAX_DECISIONS_HARD: usize = 16;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct DeploymentCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentPolicy {
    pub minimum_regions: usize,
    pub maximum_regions: usize,
    pub maximum_decisions: usize,
    pub maximum_fleet_age_ns: i64,
    pub maximum_rollback_age_ns: i64,
    pub maximum_package_age_ns: i64,
    pub maximum_decision_age_ns: i64,
    pub maximum_order_notional_micros: u64,
    pub maximum_daily_loss_micros: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionConfiguration {
    pub region: String,
    pub environment_digest: [u8; 32],
    pub image_digest: [u8; 32],
    pub configuration_digest: [u8; 32],
    pub infrastructure_plan_digest: [u8; 32],
    pub network_policy_digest: [u8; 32],
    pub observability_digest: [u8; 32],
    pub failover_digest: [u8; 32],
    pub public_admin_enabled: bool,
    pub region_digest: [u8; 32],
}

impl RegionConfiguration {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.region_digest = region_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.region_digest == region_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct LeastPrivilegePolicy {
    pub policy_id: [u8; 32],
    pub release_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
    pub allowed_regions: Vec<String>,
    pub allowed_contract_digests: Vec<[u8; 32]>,
    pub signer_policy_digest: [u8; 32],
    pub maximum_order_notional_micros: u64,
    pub maximum_daily_loss_micros: u64,
    pub credential_material_present: bool,
    pub arbitrary_transfer_allowed: bool,
    pub withdrawal_allowed: bool,
    pub contract_upgrade_allowed: bool,
    pub policy_digest: [u8; 32],
}

impl LeastPrivilegePolicy {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.allowed_regions.sort();
        self.allowed_contract_digests.sort_unstable();
        self.policy_digest = privilege_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.policy_digest == privilege_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackPackage {
    pub rollback_package_id: [u8; 32],
    pub release_digest: [u8; 32],
    pub artifacts_digest: [u8; 32],
    pub rollback_digest: [u8; 32],
    pub rollback_binary_digest: [u8; 32],
    pub rollback_configuration_digest: [u8; 32],
    pub recovery_runbook_digest: [u8; 32],
    pub verification_evidence_digest: [u8; 32],
    pub verified_at_ns: i64,
    pub package_digest: [u8; 32],
}

impl RollbackPackage {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.package_digest = rollback_package_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.package_digest == rollback_package_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentPackage {
    pub deployment_package_id: [u8; 32],
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub fleet: CurrentFleetReadiness,
    pub regions: Vec<RegionConfiguration>,
    pub least_privilege: LeastPrivilegePolicy,
    pub rollback: RollbackPackage,
    pub policy_digest: [u8; 32],
    pub package_digest: [u8; 32],
}

impl DeploymentPackage {
    #[must_use]
    pub fn sealed(mut self, policy: &DeploymentPolicy) -> Self {
        self.regions
            .sort_by(|left, right| left.region.cmp(&right.region));
        self.policy_digest = digest_json(b"deployment-preflight-policy-v1", policy);
        self.package_digest = deployment_package_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self, policy: &DeploymentPolicy) -> bool {
        self.policy_digest == digest_json(b"deployment-preflight-policy-v1", policy)
            && self.package_digest == deployment_package_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRole {
    Release,
    Risk,
    Operations,
}

impl OperatorRole {
    const ALL: [Self; 3] = [Self::Release, Self::Risk, Self::Operations];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Approve,
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorDecision {
    pub decision_id: [u8; 32],
    pub deployment_package_id: [u8; 32],
    pub package_digest: [u8; 32],
    pub role: OperatorRole,
    pub operator_id: [u8; 32],
    pub decision: DecisionKind,
    pub reason_digest: Option<[u8; 32]>,
    pub decided_at_ns: i64,
    pub valid_until_ns: i64,
    pub decision_digest: [u8; 32],
}

impl OperatorDecision {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.decision_digest = operator_decision_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.decision_digest == operator_decision_digest(self)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PreflightReason {
    MissingRole(OperatorRole),
    RejectedRole(OperatorRole),
    ExpiredDecision(OperatorRole),
    OperatorsNotDistinct,
    FleetReadinessStale,
    PackageExpired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightStatus {
    ReadyForManualDeployment,
    NotReady,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct DeploymentPreflightReport {
    pub report_id: [u8; 32],
    pub deployment_package_id: [u8; 32],
    pub package_digest: [u8; 32],
    pub fleet_readiness_digest: [u8; 32],
    pub fleet_governance_digest: [u8; 32],
    pub package_expires_at_ns: i64,
    pub regions: Vec<String>,
    pub rollback_package_digest: [u8; 32],
    pub evaluated_at_ns: i64,
    pub status: PreflightStatus,
    pub reasons: Vec<PreflightReason>,
    pub approved_roles: Vec<OperatorRole>,
    pub distinct_operator_count: u64,
    pub manual_operator_execution_required: bool,
    pub credential_material_created: bool,
    pub signing_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub rollback_execution_authority_granted: bool,
    pub cloud_control_authority_granted: bool,
    pub live_trading_authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl DeploymentPreflightReport {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest == preflight_report_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DeploymentCommand {
    Register {
        command_id: DeploymentCommandId,
        package: Box<DeploymentPackage>,
        recorded_at_ns: i64,
    },
    Decide {
        command_id: DeploymentCommandId,
        decision: OperatorDecision,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: DeploymentCommandId,
        deployment_package_id: [u8; 32],
        report_id: [u8; 32],
        current_fleet: CurrentFleetReadiness,
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl DeploymentCommand {
    #[must_use]
    pub const fn command_id(&self) -> DeploymentCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::Decide { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::Decide { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum DeploymentDetail {
    Registered,
    DecisionRecorded { role: OperatorRole },
    Finalized(Box<DeploymentPreflightReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentOutcome {
    pub command_id: DeploymentCommandId,
    pub detail: DeploymentDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentSnapshot {
    pub accepted_commands: u64,
    pub package_id: Option<[u8; 32]>,
    pub decision_count: usize,
    pub last_report: Option<DeploymentPreflightReport>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("deployment-preflight configuration is invalid")]
    Config,
    #[error("deployment-preflight timestamp is invalid or regressed")]
    Timestamp,
    #[error("deployment-preflight command exceeds its canonical bound")]
    CommandBound,
    #[error("deployment-preflight JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported deployment-preflight version: {0}")]
    Version(u16),
    #[error("deployment command id was reused for different content")]
    IdempotencyConflict,
    #[error("deployment package identity, digest or lifecycle is invalid")]
    Package,
    #[error("fleet readiness binding is invalid, stale or mismatched")]
    Fleet,
    #[error("regional configuration is invalid or incomplete")]
    Region,
    #[error("least-privilege policy is invalid or excessive")]
    Privilege,
    #[error("rollback package is invalid, stale or mismatched")]
    Rollback,
    #[error("operator decision is invalid, duplicated or mismatched")]
    Decision,
    #[error("deployment preflight report lifecycle is invalid")]
    Report,
    #[error("deployment preflight is already finalized")]
    Finalized,
    #[error("deployment-preflight arithmetic overflow")]
    Overflow,
    #[error("deployment preflight is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct DeploymentPreflight {
    policy: DeploymentPolicy,
    package: Option<DeploymentPackage>,
    decisions: BTreeMap<OperatorRole, OperatorDecision>,
    report: Option<DeploymentPreflightReport>,
    processed: BTreeMap<DeploymentCommandId, ([u8; 32], DeploymentOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl DeploymentPreflight {
    /// Creates one empty credentialless preflight owner.
    ///
    /// # Errors
    ///
    /// Rejects invalid region, evidence, decision, or time bounds.
    pub fn new(policy: DeploymentPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            package: None,
            decisions: BTreeMap::new(),
            report: None,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic preflight command.
    ///
    /// # Errors
    ///
    /// Integrity, identity, chronology, lifecycle, or arithmetic failures halt.
    pub fn apply(&mut self, command: &DeploymentCommand) -> Result<DeploymentOutcome, Error> {
        if let Some(reason) = &self.halted {
            return Err(Error::Halted(reason.clone()));
        }
        if command.recorded_at_ns() < 0
            || self
                .last_recorded_at_ns
                .is_some_and(|last| command.recorded_at_ns() < last)
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
        let mut outcome = DeploymentOutcome {
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

    fn apply_fresh(&mut self, command: &DeploymentCommand) -> Result<DeploymentDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalized);
        }
        match command {
            DeploymentCommand::Register {
                package,
                recorded_at_ns,
                ..
            } => {
                if self.package.is_some() {
                    return Err(Error::Package);
                }
                validate_package(package, &self.policy, *recorded_at_ns)?;
                self.package = Some((**package).clone());
                Ok(DeploymentDetail::Registered)
            }
            DeploymentCommand::Decide {
                decision,
                recorded_at_ns,
                ..
            } => self.record_decision(decision, *recorded_at_ns),
            DeploymentCommand::Finalize {
                deployment_package_id,
                report_id,
                current_fleet,
                evaluated_at_ns,
                recorded_at_ns,
                ..
            } => self.finalize(
                *deployment_package_id,
                *report_id,
                current_fleet,
                *evaluated_at_ns,
                *recorded_at_ns,
            ),
        }
    }

    fn record_decision(
        &mut self,
        decision: &OperatorDecision,
        recorded_at_ns: i64,
    ) -> Result<DeploymentDetail, Error> {
        let package = self.package.as_ref().ok_or(Error::Package)?;
        if self.decisions.len() >= self.policy.maximum_decisions
            || self.decisions.contains_key(&decision.role)
            || decision.decision_id == [0; 32]
            || decision.operator_id == [0; 32]
            || decision.deployment_package_id != package.deployment_package_id
            || decision.package_digest != package.package_digest
            || decision.decided_at_ns != recorded_at_ns
            || decision.valid_until_ns < decision.decided_at_ns
            || decision.valid_until_ns - decision.decided_at_ns
                > self.policy.maximum_decision_age_ns
            || !decision.verify_digest()
            || (decision.decision == DecisionKind::Approve && decision.reason_digest.is_some())
            || (decision.decision == DecisionKind::Reject
                && decision.reason_digest.is_none_or(|value| value == [0; 32]))
        {
            return Err(Error::Decision);
        }
        self.decisions.insert(decision.role, decision.clone());
        Ok(DeploymentDetail::DecisionRecorded {
            role: decision.role,
        })
    }

    fn finalize(
        &mut self,
        package_id: [u8; 32],
        report_id: [u8; 32],
        current_fleet: &CurrentFleetReadiness,
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    ) -> Result<DeploymentDetail, Error> {
        let package = self.package.as_ref().ok_or(Error::Package)?;
        if package_id != package.deployment_package_id
            || report_id == [0; 32]
            || evaluated_at_ns < package.created_at_ns
            || evaluated_at_ns > recorded_at_ns
        {
            return Err(Error::Report);
        }
        validate_current_fleet(current_fleet, package, evaluated_at_ns)?;
        let mut reasons = BTreeSet::new();
        if evaluated_at_ns > package.expires_at_ns {
            reasons.insert(PreflightReason::PackageExpired);
        }
        if evaluated_at_ns - current_fleet.observed_at_ns > self.policy.maximum_fleet_age_ns {
            reasons.insert(PreflightReason::FleetReadinessStale);
        }
        let mut operators = BTreeSet::new();
        let mut approved_roles = Vec::new();
        for role in OperatorRole::ALL {
            match self.decisions.get(&role) {
                None => {
                    reasons.insert(PreflightReason::MissingRole(role));
                }
                Some(decision) => {
                    operators.insert(decision.operator_id);
                    let expired = evaluated_at_ns > decision.valid_until_ns
                        || evaluated_at_ns - decision.decided_at_ns
                            > self.policy.maximum_decision_age_ns;
                    if expired {
                        reasons.insert(PreflightReason::ExpiredDecision(role));
                    }
                    if decision.decision == DecisionKind::Reject {
                        reasons.insert(PreflightReason::RejectedRole(role));
                    } else if !expired {
                        approved_roles.push(role);
                    }
                }
            }
        }
        if operators.len() != self.decisions.len() {
            reasons.insert(PreflightReason::OperatorsNotDistinct);
        }
        let reasons: Vec<_> = reasons.into_iter().collect();
        let mut report = DeploymentPreflightReport {
            report_id,
            deployment_package_id: package.deployment_package_id,
            package_digest: package.package_digest,
            fleet_readiness_digest: current_fleet.current_readiness_digest,
            fleet_governance_digest: current_fleet.governance_digest,
            package_expires_at_ns: package.expires_at_ns,
            regions: package
                .regions
                .iter()
                .map(|item| item.region.clone())
                .collect(),
            rollback_package_digest: package.rollback.package_digest,
            evaluated_at_ns,
            status: if reasons.is_empty() {
                PreflightStatus::ReadyForManualDeployment
            } else {
                PreflightStatus::NotReady
            },
            reasons,
            approved_roles,
            distinct_operator_count: u64::try_from(operators.len()).map_err(|_| Error::Overflow)?,
            manual_operator_execution_required: true,
            credential_material_created: false,
            signing_authority_granted: false,
            deployment_authority_granted: false,
            rollback_execution_authority_granted: false,
            cloud_control_authority_granted: false,
            live_trading_authority_granted: false,
            report_digest: [0; 32],
        };
        report.report_digest = preflight_report_digest(&report);
        self.report = Some(report.clone());
        Ok(DeploymentDetail::Finalized(Box::new(report)))
    }

    #[must_use]
    pub fn snapshot(&self) -> DeploymentSnapshot {
        DeploymentSnapshot {
            accepted_commands: self.accepted_commands,
            package_id: self.package.as_ref().map(|item| item.deployment_package_id),
            decision_count: self.decisions.len(),
            last_report: self.report.clone(),
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
        hasher.update(b"deployment-preflight-state-v1");
        hash_json(&mut hasher, &self.policy);
        hash_json(&mut hasher, &self.package);
        hash_json(&mut hasher, &self.decisions);
        hash_json(&mut hasher, &self.report);
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

fn validate_policy(policy: &DeploymentPolicy) -> Result<(), Error> {
    if policy.minimum_regions == 0
        || policy.maximum_regions == 0
        || policy.minimum_regions > policy.maximum_regions
        || policy.maximum_regions > MAX_REGIONS_HARD
        || policy.maximum_decisions < OperatorRole::ALL.len()
        || policy.maximum_decisions > MAX_DECISIONS_HARD
        || policy.maximum_fleet_age_ns <= 0
        || policy.maximum_rollback_age_ns <= 0
        || policy.maximum_package_age_ns <= 0
        || policy.maximum_decision_age_ns <= 0
        || policy.maximum_order_notional_micros == 0
        || policy.maximum_daily_loss_micros == 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}

fn validate_package(
    package: &DeploymentPackage,
    policy: &DeploymentPolicy,
    at: i64,
) -> Result<(), Error> {
    if package.deployment_package_id == [0; 32]
        || package.created_at_ns != at
        || package.expires_at_ns <= at
        || package.expires_at_ns - at > policy.maximum_package_age_ns
        || !package.verify_digest(policy)
    {
        return Err(Error::Package);
    }
    validate_initial_fleet(&package.fleet, policy, at)?;
    validate_regions(package, policy)?;
    validate_privilege(package, policy)?;
    validate_rollback(package, policy, at)?;
    Ok(())
}

fn validate_initial_fleet(
    fleet: &CurrentFleetReadiness,
    policy: &DeploymentPolicy,
    at: i64,
) -> Result<(), Error> {
    if !fleet.verify_digest()
        || fleet.campaign_id == [0; 32]
        || fleet.dossier_id == [0; 32]
        || fleet.dossier_digest == [0; 32]
        || fleet.governance_digest == [0; 32]
        || fleet.release_digest == [0; 32]
        || fleet.artifacts_digest == [0; 32]
        || fleet.rollback_digest == [0; 32]
        || fleet.observed_at_ns < 0
        || fleet.observed_at_ns > at
        || at - fleet.observed_at_ns > policy.maximum_fleet_age_ns
        || fleet.completed_regions.is_empty()
        || !fleet
            .completed_regions
            .windows(2)
            .all(|pair| pair[0] < pair[1])
    {
        Err(Error::Fleet)
    } else {
        Ok(())
    }
}

fn validate_current_fleet(
    fleet: &CurrentFleetReadiness,
    package: &DeploymentPackage,
    evaluated_at_ns: i64,
) -> Result<(), Error> {
    if !fleet.verify_digest()
        || fleet.campaign_id != package.fleet.campaign_id
        || fleet.dossier_id != package.fleet.dossier_id
        || fleet.dossier_digest != package.fleet.dossier_digest
        || fleet.release_digest != package.fleet.release_digest
        || fleet.artifacts_digest != package.fleet.artifacts_digest
        || fleet.rollback_digest != package.fleet.rollback_digest
        || fleet.completed_regions != package.fleet.completed_regions
        || fleet.governance_digest != package.fleet.governance_digest
        || fleet.observed_at_ns < package.fleet.observed_at_ns
        || fleet.observed_at_ns > evaluated_at_ns
    {
        Err(Error::Fleet)
    } else {
        Ok(())
    }
}

fn validate_regions(package: &DeploymentPackage, policy: &DeploymentPolicy) -> Result<(), Error> {
    if package.regions.len() < policy.minimum_regions
        || package.regions.len() > policy.maximum_regions
        || !package
            .regions
            .windows(2)
            .all(|pair| pair[0].region < pair[1].region)
        || package
            .regions
            .iter()
            .map(|item| &item.region)
            .ne(package.fleet.completed_regions.iter())
    {
        return Err(Error::Region);
    }
    for region in &package.regions {
        if !valid_region(&region.region)
            || region.public_admin_enabled
            || region.environment_digest == [0; 32]
            || region.image_digest == [0; 32]
            || region.configuration_digest == [0; 32]
            || region.infrastructure_plan_digest == [0; 32]
            || region.network_policy_digest == [0; 32]
            || region.observability_digest == [0; 32]
            || region.failover_digest == [0; 32]
            || !region.verify_digest()
        {
            return Err(Error::Region);
        }
    }
    Ok(())
}

fn validate_privilege(package: &DeploymentPackage, policy: &DeploymentPolicy) -> Result<(), Error> {
    let privilege = &package.least_privilege;
    let regions: Vec<_> = package
        .regions
        .iter()
        .map(|item| item.region.clone())
        .collect();
    if privilege.policy_id == [0; 32]
        || privilege.release_digest != package.fleet.release_digest
        || privilege.artifacts_digest != package.fleet.artifacts_digest
        || privilege.allowed_regions != regions
        || privilege.allowed_contract_digests.is_empty()
        || privilege.allowed_contract_digests.contains(&[0; 32])
        || !privilege
            .allowed_contract_digests
            .windows(2)
            .all(|pair| pair[0] < pair[1])
        || privilege.signer_policy_digest == [0; 32]
        || privilege.maximum_order_notional_micros == 0
        || privilege.maximum_order_notional_micros > policy.maximum_order_notional_micros
        || privilege.maximum_daily_loss_micros == 0
        || privilege.maximum_daily_loss_micros > policy.maximum_daily_loss_micros
        || privilege.credential_material_present
        || privilege.arbitrary_transfer_allowed
        || privilege.withdrawal_allowed
        || privilege.contract_upgrade_allowed
        || !privilege.verify_digest()
    {
        Err(Error::Privilege)
    } else {
        Ok(())
    }
}

fn validate_rollback(
    package: &DeploymentPackage,
    policy: &DeploymentPolicy,
    at: i64,
) -> Result<(), Error> {
    let rollback = &package.rollback;
    if rollback.rollback_package_id == [0; 32]
        || rollback.release_digest != package.fleet.release_digest
        || rollback.artifacts_digest != package.fleet.artifacts_digest
        || rollback.rollback_digest != package.fleet.rollback_digest
        || rollback.rollback_binary_digest == [0; 32]
        || rollback.rollback_configuration_digest == [0; 32]
        || rollback.recovery_runbook_digest == [0; 32]
        || rollback.verification_evidence_digest == [0; 32]
        || rollback.verified_at_ns < 0
        || rollback.verified_at_ns > at
        || at - rollback.verified_at_ns > policy.maximum_rollback_age_ns
        || !rollback.verify_digest()
    {
        Err(Error::Rollback)
    } else {
        Ok(())
    }
}

fn valid_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 64
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn region_digest(value: &RegionConfiguration) -> [u8; 32] {
    let mut clone = value.clone();
    clone.region_digest = [0; 32];
    digest_json(b"deployment-region-configuration-v1", &clone)
}

fn privilege_digest(value: &LeastPrivilegePolicy) -> [u8; 32] {
    let mut clone = value.clone();
    clone.policy_digest = [0; 32];
    digest_json(b"deployment-least-privilege-v1", &clone)
}

fn rollback_package_digest(value: &RollbackPackage) -> [u8; 32] {
    let mut clone = value.clone();
    clone.package_digest = [0; 32];
    digest_json(b"deployment-rollback-package-v1", &clone)
}

fn deployment_package_digest(value: &DeploymentPackage) -> [u8; 32] {
    let mut clone = value.clone();
    clone.package_digest = [0; 32];
    digest_json(b"deployment-package-v1", &clone)
}

fn operator_decision_digest(value: &OperatorDecision) -> [u8; 32] {
    let mut clone = value.clone();
    clone.decision_digest = [0; 32];
    digest_json(b"deployment-operator-decision-v1", &clone)
}

fn preflight_report_digest(value: &DeploymentPreflightReport) -> [u8; 32] {
    let mut clone = value.clone();
    clone.report_digest = [0; 32];
    digest_json(b"deployment-preflight-report-v1", &clone)
}

fn outcome_digest(value: &DeploymentOutcome) -> [u8; 32] {
    let mut clone = value.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"deployment-preflight-outcome-v1", &clone)
}

fn digest_json<T: Serialize + ?Sized>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize + ?Sized>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable deployment state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: DeploymentCommand,
}

/// Encodes one bounded versioned deployment-preflight command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &DeploymentCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one bounded versioned deployment-preflight command.
///
/// # Errors
///
/// Rejects malformed, oversized, trailing or unsupported input.
pub fn decode_command(bytes: &[u8]) -> Result<DeploymentCommand, Error> {
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
