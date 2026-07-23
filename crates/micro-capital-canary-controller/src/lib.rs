#![forbid(unsafe_code)]
mod durable;
mod report;
use authenticated_no_submit::{AuthReport, AuthReportStatus, AuthScenario};
pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, CanaryCheckpoint,
    CanaryRecovery, CanaryStorageError, DurableCanaryController,
};
pub use report::{read_report, write_report_create_new, CanaryReportFileError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CanaryCommandId(pub [u8; 32]);
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanaryPolicy {
    pub maximum_auth_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_cases: usize,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanaryLimits {
    pub allocated_capital_micros: i128,
    pub capital_floor_micros: i128,
    pub maximum_session_loss_micros: i128,
    pub maximum_exposure_micros: i128,
    pub maximum_candidate_cost_micros: i128,
    pub limits_digest: [u8; 32],
}
impl CanaryLimits {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.limits_digest = digest_without(b"canary-limits-v1", &self, |v| {
            v.limits_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.limits_digest
            == digest_without(b"canary-limits-v1", self, |v| {
                v.limits_digest = [0; 32];
            })
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanaryAllowlist {
    pub market_digest: [u8; 32],
    pub condition_digest: [u8; 32],
    pub up_token_digest: [u8; 32],
    pub down_token_digest: [u8; 32],
    pub allowlist_digest: [u8; 32],
}
impl CanaryAllowlist {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.allowlist_digest = digest_without(b"canary-allowlist-v1", &self, |v| {
            v.allowlist_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.allowlist_digest
            == digest_without(b"canary-allowlist-v1", self, |v| {
                v.allowlist_digest = [0; 32];
            })
    }
}
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanaryScenario {
    EligibleNoTrade,
    EligibleCompleteSet,
    CapitalFloorDenial,
    SessionLossDenial,
    ExposureDenial,
    AllowlistDenial,
    KillSwitch,
    DeadManCancel,
    OperatorAbort,
    Rollback,
}
impl CanaryScenario {
    pub const ALL: [Self; 10] = [
        Self::EligibleNoTrade,
        Self::EligibleCompleteSet,
        Self::CapitalFloorDenial,
        Self::SessionLossDenial,
        Self::ExposureDenial,
        Self::AllowlistDenial,
        Self::KillSwitch,
        Self::DeadManCancel,
        Self::OperatorAbort,
        Self::Rollback,
    ];
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanaryDisposition {
    NoTrade,
    CodeEligible,
    RollbackRequired,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct CanaryCase {
    pub case_id: [u8; 32],
    pub sequence: u64,
    pub scenario: CanaryScenario,
    pub market_digest: [u8; 32],
    pub condition_digest: [u8; 32],
    pub up_token_digest: [u8; 32],
    pub down_token_digest: [u8; 32],
    pub complete_set_only: bool,
    pub candidate_cost_micros: i128,
    pub worst_case_wealth_micros: i128,
    pub session_loss_micros: i128,
    pub exposure_micros: i128,
    pub disposition: CanaryDisposition,
    pub reservation_created: bool,
    pub kill_switch_latched: bool,
    pub cancellation_requested: bool,
    pub ambiguous_backing_retained: bool,
    pub external_action_observed: bool,
    pub case_digest: [u8; 32],
}
impl CanaryCase {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.case_digest = digest_without(b"micro-canary-case-v1", &self, |v| {
            v.case_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.case_digest
            == digest_without(b"micro-canary-case-v1", self, |v| {
                v.case_digest = [0; 32];
            })
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanaryPlan {
    pub plan_id: [u8; 32],
    pub auth_report: AuthReport,
    pub limits: CanaryLimits,
    pub allowlist: CanaryAllowlist,
    pub required_scenarios: Vec<CanaryScenario>,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}
impl CanaryPlan {
    #[must_use]
    pub fn sealed(mut self, p: &CanaryPolicy) -> Self {
        self.required_scenarios.sort();
        self.policy_digest = digest_json(b"micro-canary-policy-v1", p);
        self.plan_digest = digest_without(b"micro-canary-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self, p: &CanaryPolicy) -> bool {
        self.policy_digest == digest_json(b"micro-canary-policy-v1", p)
            && self.plan_digest
                == digest_without(b"micro-canary-plan-v1", self, |v| {
                    v.plan_digest = [0; 32];
                })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanaryReportStatus {
    CodeEligible,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct CanaryReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub auth_report_digest: [u8; 32],
    pub covered_scenarios: Vec<CanaryScenario>,
    pub finalized_at_ns: i64,
    pub status: CanaryReportStatus,
    pub live_canary_complete: bool,
    pub legal_eligibility_confirmed: bool,
    pub real_capital_allocated: bool,
    pub credential_material_created: bool,
    pub signature_produced: bool,
    pub external_order_submitted: bool,
    pub capital_authority_granted: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub submission_authority_granted: bool,
    pub report_digest: [u8; 32],
}
impl CanaryReport {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"micro-canary-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"micro-canary-report-v1", self, |v| {
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
pub enum CanaryCommand {
    Register {
        command_id: CanaryCommandId,
        plan: Box<CanaryPlan>,
        recorded_at_ns: i64,
    },
    Approve {
        command_id: CanaryCommandId,
        risk_operator_digest: [u8; 32],
        operations_operator_digest: [u8; 32],
        approved_at_ns: i64,
        recorded_at_ns: i64,
    },
    RecordCase {
        command_id: CanaryCommandId,
        case: CanaryCase,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: CanaryCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}
impl CanaryCommand {
    #[must_use]
    pub const fn command_id(&self) -> CanaryCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::Approve { command_id, .. }
            | Self::RecordCase { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::Approve { recorded_at_ns, .. }
            | Self::RecordCase { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CanaryDetail {
    Registered,
    Approved,
    CaseAccepted,
    Finalized(Box<CanaryReport>),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanaryOutcome {
    pub command_id: CanaryCommandId,
    pub detail: CanaryDetail,
    pub outcome_digest: [u8; 32],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanarySnapshot {
    pub approved: bool,
    pub kill_switch_latched: bool,
    pub covered_scenarios: BTreeSet<CanaryScenario>,
    pub case_count: u64,
    pub report: Option<CanaryReport>,
    pub accepted_commands: u64,
    pub halted: bool,
    pub digest: [u8; 32],
}
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("canary policy invalid")]
    Config,
    #[error("canary timestamp invalid")]
    Timestamp,
    #[error("canary command bound")]
    CommandBound,
    #[error("canary JSON invalid: {0}")]
    Json(String),
    #[error("canary version {0}")]
    Version(u16),
    #[error("canary id conflict")]
    IdempotencyConflict,
    #[error("Phase 3.6 evidence invalid")]
    Upstream,
    #[error("canary plan invalid")]
    Plan,
    #[error("canary approval invalid")]
    Approval,
    #[error("canary case invalid")]
    Case,
    #[error("canary finalization invalid")]
    Finalize,
    #[error("canary overflow")]
    Overflow,
    #[error("canary halted: {0}")]
    Halted(String),
}
#[derive(Clone, Debug)]
pub struct MicroCapitalCanaryController {
    policy: CanaryPolicy,
    plan: Option<CanaryPlan>,
    approved: bool,
    kill: bool,
    covered: BTreeSet<CanaryScenario>,
    used_cases: BTreeSet<[u8; 32]>,
    case_count: u64,
    processed: BTreeMap<CanaryCommandId, ([u8; 32], CanaryOutcome)>,
    accepted_commands: u64,
    report: Option<CanaryReport>,
    last_at: Option<i64>,
    halted: Option<String>,
}
impl MicroCapitalCanaryController {
    /// Creates an empty controller.
    /// # Errors
    /// Rejects invalid policy.
    pub fn new(policy: CanaryPolicy) -> Result<Self, Error> {
        if policy.maximum_auth_report_age_ns <= 0
            || policy.maximum_plan_lifetime_ns <= 0
            || policy.maximum_cases == 0
        {
            return Err(Error::Config);
        }
        Ok(Self {
            policy,
            plan: None,
            approved: false,
            kill: false,
            covered: BTreeSet::new(),
            used_cases: BTreeSet::new(),
            case_count: 0,
            processed: BTreeMap::new(),
            accepted_commands: 0,
            report: None,
            last_at: None,
            halted: None,
        })
    }
    /// Applies one deterministic command.
    /// # Errors
    /// Invalid evidence, arithmetic, case or finalization halts.
    pub fn apply(&mut self, c: &CanaryCommand) -> Result<CanaryOutcome, Error> {
        if let Some(r) = &self.halted {
            return Err(Error::Halted(r.clone()));
        }
        if c.recorded_at_ns() < 0 || self.last_at.is_some_and(|v| c.recorded_at_ns() < v) {
            return self.halt(Error::Timestamp);
        }
        let bytes = encode_command(c)?;
        let content = *blake3::hash(&bytes).as_bytes();
        if let Some((p, o)) = self.processed.get(&c.command_id()) {
            if *p == content {
                return Ok(o.clone());
            }
            return self.halt(Error::IdempotencyConflict);
        }
        let mut n = self.clone();
        let detail = match n.transition(c) {
            Ok(v) => v,
            Err(e) => return self.halt(e),
        };
        n.accepted_commands = n.accepted_commands.checked_add(1).ok_or(Error::Overflow)?;
        n.last_at = Some(c.recorded_at_ns());
        let mut o = CanaryOutcome {
            command_id: c.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        o.outcome_digest = digest_without(b"micro-canary-outcome-v1", &o, |v| {
            v.outcome_digest = [0; 32];
        });
        n.processed.insert(c.command_id(), (content, o.clone()));
        *self = n;
        Ok(o)
    }
    fn transition(&mut self, c: &CanaryCommand) -> Result<CanaryDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match c {
            CanaryCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some()
                    || !valid_upstream(&plan.auth_report, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(CanaryDetail::Registered)
            }
            CanaryCommand::Approve {
                risk_operator_digest,
                operations_operator_digest,
                approved_at_ns,
                ..
            } => {
                if self.plan.is_none()
                    || self.approved
                    || *risk_operator_digest == [0; 32]
                    || *operations_operator_digest == [0; 32]
                    || risk_operator_digest == operations_operator_digest
                    || *approved_at_ns < 0
                {
                    return Err(Error::Approval);
                }
                self.approved = true;
                Ok(CanaryDetail::Approved)
            }
            CanaryCommand::RecordCase { case, .. } => {
                let p = self.plan.as_ref().ok_or(Error::Case)?;
                if !self.approved
                    || self.used_cases.contains(&case.case_id)
                    || self.case_count
                        >= u64::try_from(self.policy.maximum_cases).map_err(|_| Error::Overflow)?
                    || case.sequence != self.case_count.checked_add(1).ok_or(Error::Overflow)?
                    || !case.verify_digest()
                    || case.external_action_observed
                    || !valid_case(case, p, self.kill)
                {
                    return Err(Error::Case);
                }
                self.case_count += 1;
                self.used_cases.insert(case.case_id);
                self.covered.insert(case.scenario);
                if case.scenario == CanaryScenario::KillSwitch {
                    self.kill = true;
                }
                Ok(CanaryDetail::CaseAccepted)
            }
            CanaryCommand::Finalize {
                report_id,
                finalized_at_ns,
                ..
            } => {
                let p = self.plan.as_ref().ok_or(Error::Finalize)?;
                if !self.approved
                    || !self.kill
                    || !p
                        .required_scenarios
                        .iter()
                        .all(|v| self.covered.contains(v))
                    || *finalized_at_ns > p.expires_at_ns
                {
                    return Err(Error::Finalize);
                }
                let r = CanaryReport {
                    report_id: *report_id,
                    plan_digest: p.plan_digest,
                    auth_report_digest: p.auth_report.report_digest,
                    covered_scenarios: self.covered.iter().copied().collect(),
                    finalized_at_ns: *finalized_at_ns,
                    status: CanaryReportStatus::CodeEligible,
                    live_canary_complete: false,
                    legal_eligibility_confirmed: false,
                    real_capital_allocated: false,
                    credential_material_created: false,
                    signature_produced: false,
                    external_order_submitted: false,
                    capital_authority_granted: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    submission_authority_granted: false,
                    report_digest: [0; 32],
                }
                .sealed();
                self.report = Some(r.clone());
                Ok(CanaryDetail::Finalized(Box::new(r)))
            }
        }
    }
    #[must_use]
    pub fn snapshot(&self) -> CanarySnapshot {
        let m = (
            self.approved,
            self.kill,
            &self.covered,
            self.case_count,
            &self.report,
            self.accepted_commands,
            &self.halted,
        );
        CanarySnapshot {
            approved: self.approved,
            kill_switch_latched: self.kill,
            covered_scenarios: self.covered.clone(),
            case_count: self.case_count,
            report: self.report.clone(),
            accepted_commands: self.accepted_commands,
            halted: self.halted.is_some(),
            digest: digest_json(b"micro-canary-state-v1", &m),
        }
    }
    #[must_use]
    pub const fn is_halted(&self) -> bool {
        self.halted.is_some()
    }
    fn halt<T>(&mut self, e: Error) -> Result<T, Error> {
        self.halted = Some(e.to_string());
        Err(e)
    }
}
fn valid_upstream(r: &AuthReport, p: &CanaryPolicy, at: i64) -> bool {
    r.verify_digest()
        && r.status == AuthReportStatus::LocallyCertified
        && r.covered_scenarios == AuthScenario::ALL
        && r.finalized_at_ns <= at
        && at - r.finalized_at_ns <= p.maximum_auth_report_age_ns
        && !r.real_identity_activated
        && !r.credential_material_created
        && !r.signature_produced
        && !r.authenticated_connection_opened
        && !r.submit_capability_present
        && !r.capital_authority_granted
        && !r.deployment_authority_granted
        && !r.trading_authority_granted
        && !r.submission_authority_granted
}
fn valid_plan(v: &CanaryPlan, p: &CanaryPolicy, at: i64) -> bool {
    v.verify_digest(p)
        && v.required_scenarios == CanaryScenario::ALL
        && v.allowlist.verify_digest()
        && v.limits.verify_digest()
        && v.allowlist.market_digest != [0; 32]
        && v.allowlist.up_token_digest != v.allowlist.down_token_digest
        && v.limits.allocated_capital_micros > 0
        && v.limits.capital_floor_micros >= 0
        && v.limits.capital_floor_micros <= v.limits.allocated_capital_micros
        && v.limits.maximum_session_loss_micros > 0
        && v.limits.maximum_exposure_micros > 0
        && v.limits.maximum_candidate_cost_micros > 0
        && v.limits.maximum_candidate_cost_micros <= v.limits.maximum_exposure_micros
        && v.created_at_ns <= at
        && v.expires_at_ns >= at
        && v.expires_at_ns - v.created_at_ns <= p.maximum_plan_lifetime_ns
}
fn ids_match(c: &CanaryCase, a: &CanaryAllowlist) -> bool {
    c.market_digest == a.market_digest
        && c.condition_digest == a.condition_digest
        && c.up_token_digest == a.up_token_digest
        && c.down_token_digest == a.down_token_digest
}
fn valid_case(c: &CanaryCase, p: &CanaryPlan, killed: bool) -> bool {
    if !c.complete_set_only
        || c.candidate_cost_micros < 0
        || c.session_loss_micros < 0
        || c.exposure_micros < 0
    {
        return false;
    }
    let within = c.candidate_cost_micros <= p.limits.maximum_candidate_cost_micros
        && c.worst_case_wealth_micros >= p.limits.capital_floor_micros
        && c.session_loss_micros <= p.limits.maximum_session_loss_micros
        && c.exposure_micros <= p.limits.maximum_exposure_micros
        && ids_match(c, &p.allowlist);
    match c.scenario {
        CanaryScenario::EligibleNoTrade => {
            ids_match(c, &p.allowlist)
                && c.disposition == CanaryDisposition::NoTrade
                && !c.reservation_created
        }
        CanaryScenario::EligibleCompleteSet => {
            !killed
                && within
                && c.disposition == CanaryDisposition::CodeEligible
                && !c.reservation_created
        }
        CanaryScenario::CapitalFloorDenial => {
            c.worst_case_wealth_micros < p.limits.capital_floor_micros
                && c.disposition == CanaryDisposition::NoTrade
                && !c.reservation_created
        }
        CanaryScenario::SessionLossDenial => {
            c.session_loss_micros > p.limits.maximum_session_loss_micros
                && c.disposition == CanaryDisposition::NoTrade
                && !c.reservation_created
        }
        CanaryScenario::ExposureDenial => {
            c.exposure_micros > p.limits.maximum_exposure_micros
                && c.disposition == CanaryDisposition::NoTrade
                && !c.reservation_created
        }
        CanaryScenario::AllowlistDenial => {
            !ids_match(c, &p.allowlist)
                && c.disposition == CanaryDisposition::NoTrade
                && !c.reservation_created
        }
        CanaryScenario::KillSwitch => {
            c.kill_switch_latched
                && c.disposition == CanaryDisposition::RollbackRequired
                && c.cancellation_requested
        }
        CanaryScenario::DeadManCancel => {
            killed
                && c.disposition == CanaryDisposition::RollbackRequired
                && c.cancellation_requested
                && c.ambiguous_backing_retained
        }
        CanaryScenario::OperatorAbort | CanaryScenario::Rollback => {
            killed
                && c.disposition == CanaryDisposition::RollbackRequired
                && c.cancellation_requested
        }
    }
}
fn digest_json<T: Serialize>(d: &[u8], v: &T) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(d);
    h.update(&serde_json::to_vec(v).expect("bounded serialization"));
    *h.finalize().as_bytes()
}
fn digest_without<T: Clone + Serialize>(d: &[u8], v: &T, f: impl FnOnce(&mut T)) -> [u8; 32] {
    let mut x = v.clone();
    f(&mut x);
    digest_json(d, &x)
}
/// Encodes a command.
/// # Errors
/// Rejects serialization or size failure.
pub fn encode_command(c: &CanaryCommand) -> Result<Vec<u8>, Error> {
    let b = serde_json::to_vec(&(WIRE_VERSION, c)).map_err(|e| Error::Json(e.to_string()))?;
    if b.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    Ok(b)
}
/// Decodes a strict command.
/// # Errors
/// Rejects malformed, trailing, noncanonical, oversized or unsupported data.
pub fn decode_command(b: &[u8]) -> Result<CanaryCommand, Error> {
    if b.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut d = serde_json::Deserializer::from_slice(b);
    let (v, c): (u16, CanaryCommand) =
        Deserialize::deserialize(&mut d).map_err(|e| Error::Json(e.to_string()))?;
    d.end().map_err(|e| Error::Json(e.to_string()))?;
    if v != WIRE_VERSION {
        return Err(Error::Version(v));
    }
    if encode_command(&c)? != b {
        return Err(Error::Json("noncanonical".into()));
    }
    Ok(c)
}
#[cfg(test)]
mod tests;
