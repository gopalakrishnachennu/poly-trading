#![forbid(unsafe_code)]

//! Deterministic, read-only, multi-provider blockchain and wallet observation.
//!
//! Agreement is intentionally strict: three independently identified providers
//! must report the same finalized block and canonical wallet state. This crate
//! has no RPC client, credential, signer, wallet, or transaction capability.

mod durable;
mod report;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, ChainCheckpoint,
    ChainRecovery, ChainStorageError, DurableChainObserver,
};
pub use report::{read_report, write_report_create_new, ChainReportFileError};

use read_only_venue::{VenueReport, VenueReportStatus, VenueScenario};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ChainCommandId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainPolicy {
    pub maximum_venue_report_age_ns: i64,
    pub maximum_plan_lifetime_ns: i64,
    pub maximum_observation_age_ns: i64,
    pub maximum_head_lag_blocks: u64,
    pub maximum_token_balances: usize,
    pub maximum_transactions: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcProviderId {
    Primary,
    Secondary,
    Archive,
}
impl RpcProviderId {
    pub const ALL: [Self; 3] = [Self::Primary, Self::Secondary, Self::Archive];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RpcProviderContract {
    pub provider: RpcProviderId,
    pub endpoint_digest: [u8; 32],
    pub region_digest: [u8; 32],
    pub chain_id: u64,
    pub genesis_digest: [u8; 32],
    pub read_only: bool,
    pub credential_present: bool,
    pub signer_present: bool,
    pub wallet_mutation_present: bool,
    pub transaction_submission_present: bool,
    pub arbitrary_request_allowed: bool,
    pub contract_digest: [u8; 32],
}
impl RpcProviderContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.contract_digest = digest_without(b"rpc-provider-contract-v1", &self, |v| {
            v.contract_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest
            == digest_without(b"rpc-provider-contract-v1", self, |v| {
                v.contract_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainIdentityContract {
    pub chain_id: u64,
    pub genesis_digest: [u8; 32],
    pub wallet_digest: [u8; 32],
    pub collateral_token_digest: [u8; 32],
    pub ctf_contract_digest: [u8; 32],
    pub exchange_contract_digest: [u8; 32],
    pub identity_digest: [u8; 32],
}
impl ChainIdentityContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.identity_digest = digest_without(b"chain-identity-contract-v1", &self, |v| {
            v.identity_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.identity_digest
            == digest_without(b"chain-identity-contract-v1", self, |v| {
                v.identity_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenBalance {
    pub token_digest: [u8; 32],
    pub balance_micros: i128,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStatus {
    Pending,
    Mined,
    Finalized,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionObservation {
    pub transaction_digest: [u8; 32],
    pub status: TransactionStatus,
    pub block_number: Option<u64>,
    pub effect_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletSnapshot {
    pub collateral_micros: i128,
    pub allowance_micros: i128,
    pub token_balances: Vec<TokenBalance>,
    pub transactions: Vec<TransactionObservation>,
    pub wallet_state_digest: [u8; 32],
}
impl WalletSnapshot {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.token_balances.sort_by_key(|v| v.token_digest);
        self.transactions.sort_by_key(|v| v.transaction_digest);
        self.wallet_state_digest = digest_without(b"wallet-snapshot-v1", &self, |v| {
            v.wallet_state_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.wallet_state_digest
            == digest_without(b"wallet-snapshot-v1", self, |v| {
                v.wallet_state_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSnapshot {
    pub observation_id: [u8; 32],
    pub provider: RpcProviderId,
    pub provider_contract_digest: [u8; 32],
    pub chain_id: u64,
    pub genesis_digest: [u8; 32],
    pub head_number: u64,
    pub head_hash: [u8; 32],
    pub head_parent_hash: [u8; 32],
    pub finalized_number: u64,
    pub finalized_hash: [u8; 32],
    pub wallet: WalletSnapshot,
    pub event_time_ns: i64,
    pub received_time_ns: i64,
    pub observed_at_ns: i64,
    pub observation_digest: [u8; 32],
}
impl ProviderSnapshot {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.observation_digest = digest_without(b"provider-snapshot-v1", &self, |v| {
            v.observation_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.observation_digest
            == digest_without(b"provider-snapshot-v1", self, |v| {
                v.observation_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgreementFrame {
    pub frame_id: [u8; 32],
    pub snapshots: Vec<ProviderSnapshot>,
    pub agreed_finalized_number: u64,
    pub agreed_finalized_hash: [u8; 32],
    pub agreed_wallet_state_digest: [u8; 32],
    pub observed_at_ns: i64,
    pub frame_digest: [u8; 32],
}
impl AgreementFrame {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.snapshots.sort_by_key(|v| v.provider);
        self.frame_digest = digest_without(b"chain-agreement-frame-v1", &self, |v| {
            v.frame_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.frame_digest
            == digest_without(b"chain-agreement-frame-v1", self, |v| {
                v.frame_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainScenario {
    ProviderAgreement,
    HeadAdvance,
    FinalityAdvance,
    ReorgBeforeFinality,
    ProviderDisagreement,
    StaleHead,
    ChainMismatch,
    BalanceChange,
    AllowanceChange,
    TransactionLifecycle,
}
impl ChainScenario {
    pub const ALL: [Self; 10] = [
        Self::ProviderAgreement,
        Self::HeadAdvance,
        Self::FinalityAdvance,
        Self::ReorgBeforeFinality,
        Self::ProviderDisagreement,
        Self::StaleHead,
        Self::ChainMismatch,
        Self::BalanceChange,
        Self::AllowanceChange,
        Self::TransactionLifecycle,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeDisposition {
    Deny,
    Halt,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct FailureFixture {
    pub fixture_id: [u8; 32],
    pub scenario: ChainScenario,
    pub disposition: SafeDisposition,
    pub trigger_digest: [u8; 32],
    pub isolated: bool,
    pub state_contribution: bool,
    pub rpc_mutation_observed: bool,
    pub wallet_mutation_observed: bool,
    pub fixture_digest: [u8; 32],
}
impl FailureFixture {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.fixture_digest = digest_without(b"chain-failure-fixture-v1", &self, |v| {
            v.fixture_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.fixture_digest
            == digest_without(b"chain-failure-fixture-v1", self, |v| {
                v.fixture_digest = [0; 32];
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainPlan {
    pub plan_id: [u8; 32],
    pub venue_report: VenueReport,
    pub identity: ChainIdentityContract,
    pub providers: Vec<RpcProviderContract>,
    pub required_scenarios: Vec<ChainScenario>,
    pub created_at_ns: i64,
    pub expires_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub plan_digest: [u8; 32],
}
impl ChainPlan {
    #[must_use]
    pub fn sealed(mut self, policy: &ChainPolicy) -> Self {
        self.providers.sort_by_key(|v| v.provider);
        self.required_scenarios.sort();
        self.policy_digest = digest_json(b"chain-policy-v1", policy);
        self.plan_digest = digest_without(b"chain-plan-v1", &self, |v| {
            v.plan_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self, policy: &ChainPolicy) -> bool {
        self.policy_digest == digest_json(b"chain-policy-v1", policy)
            && self.plan_digest
                == digest_without(b"chain-plan-v1", self, |v| v.plan_digest = [0; 32])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReorgRequirement {
    pub requirement_id: [u8; 32],
    pub prior_frame_digest: [u8; 32],
    pub prior_finalized_number: u64,
    pub prior_finalized_hash: [u8; 32],
    pub old_head_hash: [u8; 32],
    pub new_head_hash: [u8; 32],
    pub reorg_block_number: u64,
    pub requirement_digest: [u8; 32],
}
impl ReorgRequirement {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.requirement_digest
            == digest_without(b"chain-reorg-requirement-v1", self, |v| {
                v.requirement_digest = [0; 32];
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainReportStatus {
    LocallyCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct ChainReport {
    pub report_id: [u8; 32],
    pub plan_digest: [u8; 32],
    pub venue_report_digest: [u8; 32],
    pub final_frame_digest: [u8; 32],
    pub final_finalized_number: u64,
    pub covered_scenarios: Vec<ChainScenario>,
    pub finalized_at_ns: i64,
    pub status: ChainReportStatus,
    pub live_environment_certified: bool,
    pub rpc_connection_opened: bool,
    pub credential_material_created: bool,
    pub wallet_access_granted: bool,
    pub signature_produced: bool,
    pub transaction_submitted: bool,
    pub deployment_authority_granted: bool,
    pub trading_authority_granted: bool,
    pub submission_authority_granted: bool,
    pub report_digest: [u8; 32],
}
impl ChainReport {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.report_digest = digest_without(b"chain-report-v1", &self, |v| {
            v.report_digest = [0; 32];
        });
        self
    }
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest
            == digest_without(b"chain-report-v1", self, |v| v.report_digest = [0; 32])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ChainCommand {
    Register {
        command_id: ChainCommandId,
        plan: Box<ChainPlan>,
        recorded_at_ns: i64,
    },
    ObserveAgreement {
        command_id: ChainCommandId,
        frame: Box<AgreementFrame>,
        recorded_at_ns: i64,
    },
    ObserveReorg {
        command_id: ChainCommandId,
        requirement_id: [u8; 32],
        old_head_hash: [u8; 32],
        new_head_hash: [u8; 32],
        reorg_block_number: u64,
        recorded_at_ns: i64,
    },
    Recover {
        command_id: ChainCommandId,
        requirement: Box<ReorgRequirement>,
        frame: Box<AgreementFrame>,
        no_mutation_observed: bool,
        recorded_at_ns: i64,
    },
    RecordFailure {
        command_id: ChainCommandId,
        fixture: FailureFixture,
        recorded_at_ns: i64,
    },
    Finalize {
        command_id: ChainCommandId,
        report_id: [u8; 32],
        finalized_at_ns: i64,
        recorded_at_ns: i64,
    },
}
impl ChainCommand {
    #[must_use]
    pub const fn command_id(&self) -> ChainCommandId {
        match self {
            Self::Register { command_id, .. }
            | Self::ObserveAgreement { command_id, .. }
            | Self::ObserveReorg { command_id, .. }
            | Self::Recover { command_id, .. }
            | Self::RecordFailure { command_id, .. }
            | Self::Finalize { command_id, .. } => *command_id,
        }
    }
    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. }
            | Self::ObserveAgreement { recorded_at_ns, .. }
            | Self::ObserveReorg { recorded_at_ns, .. }
            | Self::Recover { recorded_at_ns, .. }
            | Self::RecordFailure { recorded_at_ns, .. }
            | Self::Finalize { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ChainDetail {
    Registered,
    AgreementAccepted,
    RecoveryRequired(Box<ReorgRequirement>),
    Recovered,
    FailureRecorded,
    Finalized(Box<ChainReport>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChainOutcome {
    pub command_id: ChainCommandId,
    pub detail: ChainDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChainSnapshot {
    pub frame: Option<AgreementFrame>,
    pub recovery: Option<ReorgRequirement>,
    pub covered_scenarios: BTreeSet<ChainScenario>,
    pub observation_ready: bool,
    pub spendable_collateral_micros: i128,
    pub report: Option<ChainReport>,
    pub accepted_commands: u64,
    pub halted: bool,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("chain observer policy invalid")]
    Config,
    #[error("chain observer timestamp invalid or regressed")]
    Timestamp,
    #[error("chain observer command exceeds bound")]
    CommandBound,
    #[error("chain observer JSON invalid: {0}")]
    Json(String),
    #[error("unsupported chain command version: {0}")]
    Version(u16),
    #[error("chain command id conflict")]
    IdempotencyConflict,
    #[error("Phase 3.2 evidence invalid, stale, incomplete, or authority-bearing")]
    Upstream,
    #[error("chain observation plan invalid")]
    Plan,
    #[error("provider agreement invalid, stale, incomplete, or equivocated")]
    Agreement,
    #[error("pre-finality reorganization transition invalid")]
    Reorg,
    #[error("chain failure fixture invalid")]
    Fixture,
    #[error("chain report finalization invalid")]
    Finalize,
    #[error("chain arithmetic overflow")]
    Overflow,
    #[error("chain observer halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug)]
pub struct ChainObserver {
    policy: ChainPolicy,
    plan: Option<ChainPlan>,
    frame: Option<AgreementFrame>,
    recovery: Option<ReorgRequirement>,
    covered: BTreeSet<ChainScenario>,
    used_frames: BTreeSet<[u8; 32]>,
    used_fixtures: BTreeSet<[u8; 32]>,
    processed: BTreeMap<ChainCommandId, ([u8; 32], ChainOutcome)>,
    accepted_commands: u64,
    report: Option<ChainReport>,
    last_recorded_at_ns: Option<i64>,
    halted: Option<String>,
}

impl ChainObserver {
    /// Creates an empty observer.
    ///
    /// # Errors
    /// Rejects zero or negative bounds.
    pub fn new(policy: ChainPolicy) -> Result<Self, Error> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            plan: None,
            frame: None,
            recovery: None,
            covered: BTreeSet::new(),
            used_frames: BTreeSet::new(),
            used_fixtures: BTreeSet::new(),
            processed: BTreeMap::new(),
            accepted_commands: 0,
            report: None,
            last_recorded_at_ns: None,
            halted: None,
        })
    }

    /// Applies one deterministic, journalable observation command.
    ///
    /// # Errors
    /// Invalid chronology, identity, agreement, recovery or finalization halts.
    pub fn apply(&mut self, command: &ChainCommand) -> Result<ChainOutcome, Error> {
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
        let mut outcome = ChainOutcome {
            command_id: command.command_id(),
            detail,
            outcome_digest: [0; 32],
        };
        outcome.outcome_digest = digest_without(b"chain-outcome-v1", &outcome, |v| {
            v.outcome_digest = [0; 32];
        });
        next.processed
            .insert(command.command_id(), (content, outcome.clone()));
        *self = next;
        Ok(outcome)
    }

    #[allow(clippy::too_many_lines)]
    fn transition(&mut self, command: &ChainCommand) -> Result<ChainDetail, Error> {
        if self.report.is_some() {
            return Err(Error::Finalize);
        }
        match command {
            ChainCommand::Register {
                plan,
                recorded_at_ns,
                ..
            } => {
                if self.plan.is_some()
                    || !valid_upstream(&plan.venue_report, &self.policy, *recorded_at_ns)
                {
                    return Err(Error::Upstream);
                }
                if !valid_plan(plan, &self.policy, *recorded_at_ns) {
                    return Err(Error::Plan);
                }
                self.plan = Some((**plan).clone());
                Ok(ChainDetail::Registered)
            }
            ChainCommand::ObserveAgreement {
                frame,
                recorded_at_ns,
                ..
            } => {
                if self.recovery.is_some()
                    || self.used_frames.contains(&frame.frame_id)
                    || !self.valid_frame(frame, *recorded_at_ns)
                {
                    return Err(Error::Agreement);
                }
                if let Some(prior) = &self.frame {
                    validate_finalized_successor(prior, frame)?;
                    let prior_head = minimum_head(prior);
                    let next_head = minimum_head(frame);
                    if next_head > prior_head {
                        self.covered.insert(ChainScenario::HeadAdvance);
                    }
                    if frame.agreed_finalized_number > prior.agreed_finalized_number {
                        self.covered.insert(ChainScenario::FinalityAdvance);
                    }
                    let old = &prior.snapshots[0].wallet;
                    let new = &frame.snapshots[0].wallet;
                    if old.collateral_micros != new.collateral_micros {
                        self.covered.insert(ChainScenario::BalanceChange);
                    }
                    if old.allowance_micros != new.allowance_micros {
                        self.covered.insert(ChainScenario::AllowanceChange);
                    }
                    if old.transactions != new.transactions {
                        self.covered.insert(ChainScenario::TransactionLifecycle);
                    }
                }
                self.covered.insert(ChainScenario::ProviderAgreement);
                self.used_frames.insert(frame.frame_id);
                self.frame = Some((**frame).clone());
                Ok(ChainDetail::AgreementAccepted)
            }
            ChainCommand::ObserveReorg {
                requirement_id,
                old_head_hash,
                new_head_hash,
                reorg_block_number,
                ..
            } => {
                let prior = self.frame.as_ref().ok_or(Error::Reorg)?;
                if old_head_hash == new_head_hash
                    || *reorg_block_number <= prior.agreed_finalized_number
                    || !prior
                        .snapshots
                        .iter()
                        .any(|v| v.head_hash == *old_head_hash)
                {
                    return Err(Error::Reorg);
                }
                let mut requirement = ReorgRequirement {
                    requirement_id: *requirement_id,
                    prior_frame_digest: prior.frame_digest,
                    prior_finalized_number: prior.agreed_finalized_number,
                    prior_finalized_hash: prior.agreed_finalized_hash,
                    old_head_hash: *old_head_hash,
                    new_head_hash: *new_head_hash,
                    reorg_block_number: *reorg_block_number,
                    requirement_digest: [0; 32],
                };
                requirement.requirement_digest =
                    digest_without(b"chain-reorg-requirement-v1", &requirement, |v| {
                        v.requirement_digest = [0; 32];
                    });
                self.frame = None;
                self.recovery = Some(requirement.clone());
                self.covered.insert(ChainScenario::ReorgBeforeFinality);
                Ok(ChainDetail::RecoveryRequired(Box::new(requirement)))
            }
            ChainCommand::Recover {
                requirement,
                frame,
                no_mutation_observed,
                recorded_at_ns,
                ..
            } => {
                let expected = self.recovery.as_ref().ok_or(Error::Reorg)?;
                if requirement.as_ref() != expected
                    || !requirement.verify_digest()
                    || !*no_mutation_observed
                    || self.used_frames.contains(&frame.frame_id)
                    || !self.valid_frame(frame, *recorded_at_ns)
                    || frame.agreed_finalized_number < requirement.prior_finalized_number
                    || (frame.agreed_finalized_number == requirement.prior_finalized_number
                        && frame.agreed_finalized_hash != requirement.prior_finalized_hash)
                {
                    return Err(Error::Reorg);
                }
                self.used_frames.insert(frame.frame_id);
                self.frame = Some((**frame).clone());
                self.recovery = None;
                Ok(ChainDetail::Recovered)
            }
            ChainCommand::RecordFailure { fixture, .. } => {
                if self.used_fixtures.contains(&fixture.fixture_id)
                    || !fixture.verify_digest()
                    || !fixture.isolated
                    || fixture.state_contribution
                    || fixture.rpc_mutation_observed
                    || fixture.wallet_mutation_observed
                {
                    return Err(Error::Fixture);
                }
                match (fixture.scenario, fixture.disposition) {
                    (
                        ChainScenario::ProviderDisagreement | ChainScenario::ChainMismatch,
                        SafeDisposition::Halt,
                    )
                    | (ChainScenario::StaleHead, SafeDisposition::Deny) => {}
                    _ => return Err(Error::Fixture),
                }
                self.used_fixtures.insert(fixture.fixture_id);
                self.covered.insert(fixture.scenario);
                Ok(ChainDetail::FailureRecorded)
            }
            ChainCommand::Finalize {
                report_id,
                finalized_at_ns,
                ..
            } => {
                let plan = self.plan.as_ref().ok_or(Error::Finalize)?;
                let frame = self.frame.as_ref().ok_or(Error::Finalize)?;
                if self.recovery.is_some()
                    || *finalized_at_ns < frame.observed_at_ns
                    || *finalized_at_ns > plan.expires_at_ns
                    || !self.valid_frame(frame, *finalized_at_ns)
                    || !plan
                        .required_scenarios
                        .iter()
                        .all(|v| self.covered.contains(v))
                {
                    return Err(Error::Finalize);
                }
                let report = ChainReport {
                    report_id: *report_id,
                    plan_digest: plan.plan_digest,
                    venue_report_digest: plan.venue_report.report_digest,
                    final_frame_digest: frame.frame_digest,
                    final_finalized_number: frame.agreed_finalized_number,
                    covered_scenarios: self.covered.iter().copied().collect(),
                    finalized_at_ns: *finalized_at_ns,
                    status: ChainReportStatus::LocallyCertified,
                    live_environment_certified: false,
                    rpc_connection_opened: false,
                    credential_material_created: false,
                    wallet_access_granted: false,
                    signature_produced: false,
                    transaction_submitted: false,
                    deployment_authority_granted: false,
                    trading_authority_granted: false,
                    submission_authority_granted: false,
                    report_digest: [0; 32],
                }
                .sealed();
                self.report = Some(report.clone());
                Ok(ChainDetail::Finalized(Box::new(report)))
            }
        }
    }

    fn valid_frame(&self, frame: &AgreementFrame, at: i64) -> bool {
        let Some(plan) = &self.plan else {
            return false;
        };
        if !frame.verify_digest()
            || frame.snapshots.len() != RpcProviderId::ALL.len()
            || frame.observed_at_ns > at
            || at - frame.observed_at_ns > self.policy.maximum_observation_age_ns
        {
            return false;
        }
        let providers: Vec<_> = frame.snapshots.iter().map(|v| v.provider).collect();
        if providers != RpcProviderId::ALL {
            return false;
        }
        frame.snapshots.iter().all(|snapshot| {
            let Some(contract) = plan
                .providers
                .iter()
                .find(|v| v.provider == snapshot.provider)
            else {
                return false;
            };
            snapshot.verify_digest()
                && snapshot.wallet.verify_digest()
                && snapshot.provider_contract_digest == contract.contract_digest
                && snapshot.chain_id == plan.identity.chain_id
                && snapshot.genesis_digest == plan.identity.genesis_digest
                && snapshot.head_number >= snapshot.finalized_number
                && snapshot.head_number - snapshot.finalized_number
                    <= self.policy.maximum_head_lag_blocks
                && snapshot.finalized_number == frame.agreed_finalized_number
                && snapshot.finalized_hash == frame.agreed_finalized_hash
                && snapshot.wallet.wallet_state_digest == frame.agreed_wallet_state_digest
                && snapshot.event_time_ns <= snapshot.received_time_ns
                && snapshot.received_time_ns <= snapshot.observed_at_ns
                && snapshot.observed_at_ns <= at
                && at - snapshot.observed_at_ns <= self.policy.maximum_observation_age_ns
                && valid_wallet(&snapshot.wallet, &self.policy, snapshot.finalized_number)
        })
    }

    #[must_use]
    pub fn snapshot(&self, at: i64) -> ChainSnapshot {
        let ready = self
            .frame
            .as_ref()
            .is_some_and(|v| self.recovery.is_none() && self.valid_frame(v, at));
        let spendable = if ready {
            self.frame
                .as_ref()
                .map_or(0, |v| v.snapshots[0].wallet.collateral_micros)
        } else {
            0
        };
        let material = (
            &self.frame,
            &self.recovery,
            &self.covered,
            ready,
            spendable,
            &self.report,
            self.accepted_commands,
            &self.halted,
        );
        ChainSnapshot {
            frame: self.frame.clone(),
            recovery: self.recovery.clone(),
            covered_scenarios: self.covered.clone(),
            observation_ready: ready,
            spendable_collateral_micros: spendable,
            report: self.report.clone(),
            accepted_commands: self.accepted_commands,
            halted: self.halted.is_some(),
            digest: digest_json(b"chain-state-v1", &material),
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
}

fn validate_policy(p: &ChainPolicy) -> Result<(), Error> {
    if p.maximum_venue_report_age_ns <= 0
        || p.maximum_plan_lifetime_ns <= 0
        || p.maximum_observation_age_ns <= 0
        || p.maximum_head_lag_blocks == 0
        || p.maximum_token_balances == 0
        || p.maximum_transactions == 0
    {
        Err(Error::Config)
    } else {
        Ok(())
    }
}
fn valid_upstream(r: &VenueReport, p: &ChainPolicy, at: i64) -> bool {
    r.verify_digest()
        && r.status == VenueReportStatus::LocallyCertified
        && r.covered_scenarios == VenueScenario::ALL
        && r.finalized_at_ns <= at
        && at - r.finalized_at_ns <= p.maximum_venue_report_age_ns
        && !r.live_environment_certified
        && !r.credential_material_created
        && !r.authenticated_session_opened
        && !r.order_endpoint_present
        && !r.cancel_endpoint_present
        && !r.order_submitted
        && !r.cancellation_submitted
        && !r.deployment_authority_granted
        && !r.trading_authority_granted
        && !r.submission_authority_granted
}
fn valid_plan(plan: &ChainPlan, p: &ChainPolicy, at: i64) -> bool {
    if !plan.verify_digest(p)
        || !plan.identity.verify_digest()
        || plan.required_scenarios != ChainScenario::ALL
        || plan.providers.len() != 3
        || plan.created_at_ns > at
        || plan.expires_at_ns < at
        || plan.expires_at_ns - plan.created_at_ns > p.maximum_plan_lifetime_ns
    {
        return false;
    }
    let ids: Vec<_> = plan.providers.iter().map(|v| v.provider).collect();
    ids == RpcProviderId::ALL
        && plan.providers.iter().all(|v| {
            v.verify_digest()
                && v.chain_id == plan.identity.chain_id
                && v.genesis_digest == plan.identity.genesis_digest
                && v.read_only
                && !v.credential_present
                && !v.signer_present
                && !v.wallet_mutation_present
                && !v.transaction_submission_present
                && !v.arbitrary_request_allowed
        })
        && plan.providers.windows(2).all(|w| {
            w[0].endpoint_digest != w[1].endpoint_digest && w[0].region_digest != w[1].region_digest
        })
}
fn valid_wallet(wallet: &WalletSnapshot, p: &ChainPolicy, finalized: u64) -> bool {
    wallet.collateral_micros >= 0
        && wallet.allowance_micros >= 0
        && wallet.token_balances.len() <= p.maximum_token_balances
        && wallet.transactions.len() <= p.maximum_transactions
        && wallet
            .token_balances
            .windows(2)
            .all(|w| w[0].token_digest < w[1].token_digest)
        && wallet.token_balances.iter().all(|v| v.balance_micros >= 0)
        && wallet
            .transactions
            .windows(2)
            .all(|w| w[0].transaction_digest < w[1].transaction_digest)
        && wallet.transactions.iter().all(|tx| match tx.status {
            TransactionStatus::Pending => tx.block_number.is_none(),
            TransactionStatus::Mined => tx.block_number.is_some_and(|b| b > finalized),
            TransactionStatus::Finalized => tx.block_number.is_some_and(|b| b <= finalized),
            TransactionStatus::Failed => true,
        })
}
fn validate_finalized_successor(
    prior: &AgreementFrame,
    next: &AgreementFrame,
) -> Result<(), Error> {
    if next.agreed_finalized_number < prior.agreed_finalized_number
        || (next.agreed_finalized_number == prior.agreed_finalized_number
            && next.agreed_finalized_hash != prior.agreed_finalized_hash)
    {
        Err(Error::Agreement)
    } else {
        Ok(())
    }
}
fn minimum_head(frame: &AgreementFrame) -> u64 {
    frame
        .snapshots
        .iter()
        .map(|v| v.head_number)
        .min()
        .unwrap_or(0)
}

fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(&serde_json::to_vec(value).expect("serializing bounded internal state cannot fail"));
    *h.finalize().as_bytes()
}
fn digest_without<T: Clone + Serialize>(
    domain: &[u8],
    value: &T,
    clear: impl FnOnce(&mut T),
) -> [u8; 32] {
    let mut v = value.clone();
    clear(&mut v);
    digest_json(domain, &v)
}

/// Encodes a versioned bounded command.
/// # Errors
/// Rejects serialization and over-bound payloads.
pub fn encode_command(command: &ChainCommand) -> Result<Vec<u8>, Error> {
    let bytes =
        serde_json::to_vec(&(WIRE_VERSION, command)).map_err(|e| Error::Json(e.to_string()))?;
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    Ok(bytes)
}
/// Decodes one versioned bounded command.
/// # Errors
/// Rejects oversized, malformed, trailing, noncanonical or unsupported data.
pub fn decode_command(bytes: &[u8]) -> Result<ChainCommand, Error> {
    if bytes.len() > MAX_COMMAND_BYTES {
        return Err(Error::CommandBound);
    }
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let (version, command): (u16, ChainCommand) =
        Deserialize::deserialize(&mut de).map_err(|e| Error::Json(e.to_string()))?;
    de.end().map_err(|e| Error::Json(e.to_string()))?;
    if version != WIRE_VERSION {
        return Err(Error::Version(version));
    }
    if encode_command(&command)? != bytes {
        return Err(Error::Json("noncanonical command".into()));
    }
    Ok(command)
}

#[cfg(test)]
mod tests;
