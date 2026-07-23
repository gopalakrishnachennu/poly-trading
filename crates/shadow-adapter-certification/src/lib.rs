#![forbid(unsafe_code)]

//! Deterministic shadow-adapter certification without live authority.
//!
//! This crate cannot load credentials, sign, authenticate, access a wallet,
//! call a network or RPC endpoint, contact a relayer, or submit anything.

mod durable;

pub use durable::{
    read_checkpoint, recover_segmented, write_checkpoint_create_new, CertificationCheckpoint,
    CertificationRecovery, DurableCertificationAuthority, StorageError,
};

use accounting_ledger::TokenKey;
use order_intent_policy::SignerPolicyFrame;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const WIRE_VERSION: u16 = 1;
const MAX_COMMAND_BYTES: usize = 512 * 1024;
const MAX_TEXT_BYTES: usize = 512;
const MAX_REGIONS: usize = 16;
const MAX_TOKENS: usize = 64;
const MICROS_PER_UNIT: i128 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CertificationCommandId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct FixtureId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct DryRunId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct FailureId(pub [u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterContract {
    pub contract_id: [u8; 32],
    pub venue: String,
    pub rest_host: String,
    pub websocket_host: String,
    pub chain_id: u64,
    pub exchange_contract: String,
    pub settlement_contract: String,
    pub schema_version: u16,
    pub required_regions: Vec<String>,
    pub max_evidence_age_ns: i64,
    pub minimum_collateral_micros: i128,
    pub required_allowance_micros: i128,
    pub minimum_gas_micros: i128,
    pub max_relayer_queue_depth: u64,
    pub rules_digest: [u8; 32],
    pub contract_digest: [u8; 32],
}

impl AdapterContract {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.contract_digest = contract_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.contract_digest == contract_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureKind {
    Restart425,
    PostOnlyWindow,
    CancelOnlyMode,
    TakerDelay,
    TickSizeChange,
    RateLimit429,
    UnknownOrder,
    SettlementRetrying,
    HeartbeatLost,
}

impl FixtureKind {
    const ALL: [Self; 9] = [
        Self::Restart425,
        Self::PostOnlyWindow,
        Self::CancelOnlyMode,
        Self::TakerDelay,
        Self::TickSizeChange,
        Self::RateLimit429,
        Self::UnknownOrder,
        Self::SettlementRetrying,
        Self::HeartbeatLost,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureAction {
    BackoffWithoutAutomaticRetry,
    MakerOnly,
    CancelOnly,
    RetainBackingUntilDelayEnds,
    RevalidateOpenOrders,
    ReconcileUnknownOrder,
    RetainUnconfirmedValue,
    CancelOrdersAndDenyExposure,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedFixture {
    pub fixture_id: FixtureId,
    pub contract_digest: [u8; 32],
    pub sequence: u64,
    pub kind: FixtureKind,
    pub captured_at_ns: i64,
    pub received_at_ns: i64,
    pub payload_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EligibilityAttestation {
    pub sequence: u64,
    pub region: String,
    pub egress_fingerprint: [u8; 32],
    pub eligible: bool,
    pub checked_at_ns: i64,
    pub valid_until_ns: i64,
    pub source_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalObservation {
    pub sequence: u64,
    pub wallet_alias: String,
    pub chain_id: u64,
    pub collateral_micros: i128,
    pub allowance_micros: i128,
    pub gas_micros: i128,
    pub relayer_available: bool,
    pub relayer_queue_depth: u64,
    pub observed_at_ns: i64,
    pub valid_until_ns: i64,
    pub observation_digest: [u8; 32],
}

impl OperationalObservation {
    #[must_use]
    pub fn sealed(mut self) -> Self {
        self.observation_digest = operational_digest(&self);
        self
    }

    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.observation_digest == operational_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DryRunIntent {
    pub venue: String,
    pub exchange_contract: String,
    pub token: TokenKey,
    pub quantity_micros: i128,
    pub price_micros: i64,
    pub maker: bool,
    pub evaluated_at_ns: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DryRunReason {
    Permitted,
    PolicyInvalid,
    PolicyInactive,
    VenueForbidden,
    ContractForbidden,
    TokenForbidden,
    QuantityLimit,
    PriceLimit,
    NotionalLimit,
    MakerForbidden,
    TakerForbidden,
}

impl DryRunReason {
    const REQUIRED_DENIALS: [Self; 4] = [
        Self::ContractForbidden,
        Self::TokenForbidden,
        Self::QuantityLimit,
        Self::PolicyInactive,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DryRunResult {
    pub dry_run_id: DryRunId,
    pub permitted: bool,
    pub reason: DryRunReason,
    pub policy_digest: [u8; 32],
    pub intent_digest: [u8; 32],
    pub result_digest: [u8; 32],
}

impl DryRunResult {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.result_digest == dry_run_result_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    AllowanceInsufficient,
    GasInsufficient,
    RelayerUnavailable,
    EligibilityBlocked,
    UnknownSubmission,
    EngineRestarting,
    RateLimited,
    SettlementRetrying,
}

impl FailureKind {
    const ALL: [Self; 8] = [
        Self::AllowanceInsufficient,
        Self::GasInsufficient,
        Self::RelayerUnavailable,
        Self::EligibilityBlocked,
        Self::UnknownSubmission,
        Self::EngineRestarting,
        Self::RateLimited,
        Self::SettlementRetrying,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeAction {
    DenyNewExposure,
    RetainBackingAndReconcile,
    BackoffWithoutAutomaticRetry,
    RetainUnconfirmedValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureResult {
    pub failure_id: FailureId,
    pub kind: FailureKind,
    pub action: SafeAction,
    pub result_digest: [u8; 32],
}

impl FailureResult {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.result_digest == failure_result_digest(self)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CertificationReason {
    ContractMissing,
    FixtureMissing(FixtureKind),
    BaselineDryRunMissing,
    DenialDryRunMissing(DryRunReason),
    EligibilityMissing(String),
    EligibilityBlocked(String),
    EligibilityStale(String),
    OperationalMissing,
    OperationalStale,
    CollateralInsufficient,
    AllowanceInsufficient,
    GasInsufficient,
    RelayerUnavailable,
    RelayerQueueExceeded,
    FailureSimulationMissing(FailureKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificationStatus {
    Certified,
    NotCertified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationReport {
    pub profile_id: [u8; 32],
    pub contract_digest: Option<[u8; 32]>,
    pub evaluated_at_ns: i64,
    pub status: CertificationStatus,
    pub reasons: Vec<CertificationReason>,
    pub fixture_count: usize,
    pub dry_run_count: usize,
    pub failure_count: usize,
    pub authority_granted: bool,
    pub report_digest: [u8; 32],
}

impl CertificationReport {
    #[must_use]
    pub fn verify_digest(&self) -> bool {
        self.report_digest == certification_report_digest(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CertificationCommand {
    RegisterContract {
        command_id: CertificationCommandId,
        contract: AdapterContract,
        recorded_at_ns: i64,
    },
    RecordFixture {
        command_id: CertificationCommandId,
        fixture: RecordedFixture,
        recorded_at_ns: i64,
    },
    ObserveEligibility {
        command_id: CertificationCommandId,
        attestation: EligibilityAttestation,
        recorded_at_ns: i64,
    },
    ObserveOperational {
        command_id: CertificationCommandId,
        observation: OperationalObservation,
        recorded_at_ns: i64,
    },
    DryRunSigner {
        command_id: CertificationCommandId,
        dry_run_id: DryRunId,
        policy: SignerPolicyFrame,
        intent: DryRunIntent,
        recorded_at_ns: i64,
    },
    SimulateFailure {
        command_id: CertificationCommandId,
        failure_id: FailureId,
        kind: FailureKind,
        recorded_at_ns: i64,
    },
    Evaluate {
        command_id: CertificationCommandId,
        profile_id: [u8; 32],
        evaluated_at_ns: i64,
        recorded_at_ns: i64,
    },
}

impl CertificationCommand {
    #[must_use]
    pub const fn command_id(&self) -> CertificationCommandId {
        match self {
            Self::RegisterContract { command_id, .. }
            | Self::RecordFixture { command_id, .. }
            | Self::ObserveEligibility { command_id, .. }
            | Self::ObserveOperational { command_id, .. }
            | Self::DryRunSigner { command_id, .. }
            | Self::SimulateFailure { command_id, .. }
            | Self::Evaluate { command_id, .. } => *command_id,
        }
    }

    #[must_use]
    pub const fn recorded_at_ns(&self) -> i64 {
        match self {
            Self::RegisterContract { recorded_at_ns, .. }
            | Self::RecordFixture { recorded_at_ns, .. }
            | Self::ObserveEligibility { recorded_at_ns, .. }
            | Self::ObserveOperational { recorded_at_ns, .. }
            | Self::DryRunSigner { recorded_at_ns, .. }
            | Self::SimulateFailure { recorded_at_ns, .. }
            | Self::Evaluate { recorded_at_ns, .. } => *recorded_at_ns,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CertificationDetail {
    ContractRegistered,
    FixtureRecorded {
        kind: FixtureKind,
        action: FixtureAction,
    },
    EligibilityObserved,
    OperationalObserved,
    DryRun(DryRunResult),
    FailureSimulated(FailureResult),
    Evaluated(CertificationReport),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationOutcome {
    pub command_id: CertificationCommandId,
    pub detail: CertificationDetail,
    pub outcome_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CertificationSnapshot {
    pub accepted_commands: u64,
    pub contract_digest: Option<[u8; 32]>,
    pub fixture_count: usize,
    pub fixture_kinds: BTreeSet<FixtureKind>,
    pub eligibility_count: usize,
    pub dry_run_count: usize,
    pub failure_count: usize,
    pub last_report: Option<CertificationReport>,
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum Error {
    #[error("certification command timestamp is invalid or regressed")]
    Timestamp,
    #[error("certification command exceeds its canonical bound")]
    CommandBound,
    #[error("certification command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported certification command version: {0}")]
    Version(u16),
    #[error("certification command id was reused for different content")]
    IdempotencyConflict,
    #[error("adapter contract is invalid or was substituted")]
    Contract,
    #[error("certification evidence identity was reused or substituted")]
    Identity,
    #[error("certification evidence history regressed or equivocated")]
    History,
    #[error("certification evidence is invalid")]
    Evidence,
    #[error("certification arithmetic or counter overflow")]
    Overflow,
    #[error("certification authority is halted: {0}")]
    Halted(String),
}

#[derive(Clone, Debug, Default)]
pub struct ShadowAdapterCertification {
    contract: Option<AdapterContract>,
    fixtures: BTreeMap<FixtureId, RecordedFixture>,
    fixture_sequences: BTreeMap<u64, FixtureId>,
    last_fixture_sequence: Option<u64>,
    eligibility: BTreeMap<String, EligibilityAttestation>,
    operational: Option<OperationalObservation>,
    wallet_alias: Option<String>,
    dry_runs: BTreeMap<DryRunId, DryRunResult>,
    failures: BTreeMap<FailureId, FailureResult>,
    failure_kinds: BTreeMap<FailureKind, FailureId>,
    reports: BTreeMap<[u8; 32], CertificationReport>,
    processed: BTreeMap<CertificationCommandId, ([u8; 32], CertificationOutcome)>,
    accepted_commands: u64,
    last_recorded_at_ns: Option<i64>,
    last_report: Option<CertificationReport>,
    halted: Option<String>,
}

impl ShadowAdapterCertification {
    /// Applies one deterministic certification command.
    ///
    /// # Errors
    ///
    /// Returns absorbing identity, history, arithmetic, or durable failures.
    pub fn apply(&mut self, command: &CertificationCommand) -> Result<CertificationOutcome, Error> {
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
        let mut outcome = CertificationOutcome {
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

    fn apply_fresh(
        &mut self,
        command: &CertificationCommand,
    ) -> Result<CertificationDetail, Error> {
        match command {
            CertificationCommand::RegisterContract { contract, .. } => {
                validate_contract(contract)?;
                if self.contract.is_some() {
                    return Err(Error::Contract);
                }
                self.contract = Some(contract.clone());
                Ok(CertificationDetail::ContractRegistered)
            }
            CertificationCommand::RecordFixture {
                fixture,
                recorded_at_ns,
                ..
            } => {
                if fixture.received_at_ns > *recorded_at_ns {
                    return Err(Error::Timestamp);
                }
                self.record_fixture(fixture)?;
                Ok(CertificationDetail::FixtureRecorded {
                    kind: fixture.kind,
                    action: fixture_action(fixture.kind),
                })
            }
            CertificationCommand::ObserveEligibility {
                attestation,
                recorded_at_ns,
                ..
            } => {
                if attestation.checked_at_ns > *recorded_at_ns {
                    return Err(Error::Timestamp);
                }
                self.observe_eligibility(attestation)?;
                Ok(CertificationDetail::EligibilityObserved)
            }
            CertificationCommand::ObserveOperational {
                observation,
                recorded_at_ns,
                ..
            } => {
                if observation.observed_at_ns > *recorded_at_ns {
                    return Err(Error::Timestamp);
                }
                self.observe_operational(observation)?;
                Ok(CertificationDetail::OperationalObserved)
            }
            CertificationCommand::DryRunSigner {
                dry_run_id,
                policy,
                intent,
                recorded_at_ns,
                ..
            } => {
                if self.dry_runs.contains_key(dry_run_id) {
                    return Err(Error::Identity);
                }
                if intent.evaluated_at_ns > *recorded_at_ns {
                    return Err(Error::Timestamp);
                }
                let contract = self.contract.as_ref().ok_or(Error::Contract)?;
                let result = dry_run(*dry_run_id, contract, policy, intent)?;
                self.dry_runs.insert(*dry_run_id, result.clone());
                Ok(CertificationDetail::DryRun(result))
            }
            CertificationCommand::SimulateFailure {
                failure_id, kind, ..
            } => {
                if self.failures.contains_key(failure_id) || self.failure_kinds.contains_key(kind) {
                    return Err(Error::Identity);
                }
                let mut result = FailureResult {
                    failure_id: *failure_id,
                    kind: *kind,
                    action: safe_action(*kind),
                    result_digest: [0; 32],
                };
                result.result_digest = failure_result_digest(&result);
                self.failures.insert(*failure_id, result.clone());
                self.failure_kinds.insert(*kind, *failure_id);
                Ok(CertificationDetail::FailureSimulated(result))
            }
            CertificationCommand::Evaluate {
                profile_id,
                evaluated_at_ns,
                recorded_at_ns,
                ..
            } => {
                if *evaluated_at_ns < 0 || *evaluated_at_ns > *recorded_at_ns {
                    return Err(Error::Timestamp);
                }
                if *profile_id == [0; 32] || self.reports.contains_key(profile_id) {
                    return Err(Error::Identity);
                }
                let report = self.evaluate(*profile_id, *evaluated_at_ns)?;
                self.reports.insert(*profile_id, report.clone());
                self.last_report = Some(report.clone());
                Ok(CertificationDetail::Evaluated(report))
            }
        }
    }

    fn record_fixture(&mut self, fixture: &RecordedFixture) -> Result<(), Error> {
        let contract = self.contract.as_ref().ok_or(Error::Contract)?;
        if fixture.contract_digest != contract.contract_digest
            || fixture.sequence == 0
            || fixture.captured_at_ns < 0
            || fixture.received_at_ns < fixture.captured_at_ns
            || fixture.payload_digest == [0; 32]
        {
            return Err(Error::Evidence);
        }
        if self.fixtures.contains_key(&fixture.fixture_id)
            || self.fixture_sequences.contains_key(&fixture.sequence)
        {
            return Err(Error::Identity);
        }
        if self
            .last_fixture_sequence
            .is_some_and(|previous| fixture.sequence <= previous)
        {
            return Err(Error::History);
        }
        self.fixtures.insert(fixture.fixture_id, fixture.clone());
        self.fixture_sequences
            .insert(fixture.sequence, fixture.fixture_id);
        self.last_fixture_sequence = Some(fixture.sequence);
        Ok(())
    }

    fn observe_eligibility(&mut self, attestation: &EligibilityAttestation) -> Result<(), Error> {
        let contract = self.contract.as_ref().ok_or(Error::Contract)?;
        if !contract.required_regions.contains(&attestation.region)
            || attestation.sequence == 0
            || !valid_text(&attestation.region)
            || attestation.egress_fingerprint == [0; 32]
            || attestation.source_digest == [0; 32]
            || attestation.checked_at_ns < 0
            || attestation.valid_until_ns < attestation.checked_at_ns
        {
            return Err(Error::Evidence);
        }
        if self
            .eligibility
            .get(&attestation.region)
            .is_some_and(|old| {
                attestation.sequence <= old.sequence
                    || attestation.checked_at_ns < old.checked_at_ns
            })
        {
            return Err(Error::History);
        }
        self.eligibility
            .insert(attestation.region.clone(), attestation.clone());
        Ok(())
    }

    fn observe_operational(&mut self, observation: &OperationalObservation) -> Result<(), Error> {
        let contract = self.contract.as_ref().ok_or(Error::Contract)?;
        if !observation.verify_digest()
            || observation.sequence == 0
            || !valid_text(&observation.wallet_alias)
            || observation.chain_id != contract.chain_id
            || observation.collateral_micros < 0
            || observation.allowance_micros < 0
            || observation.gas_micros < 0
            || observation.observed_at_ns < 0
            || observation.valid_until_ns < observation.observed_at_ns
        {
            return Err(Error::Evidence);
        }
        if self
            .wallet_alias
            .as_ref()
            .is_some_and(|alias| alias != &observation.wallet_alias)
        {
            return Err(Error::Identity);
        }
        if self.operational.as_ref().is_some_and(|old| {
            observation.sequence <= old.sequence || observation.observed_at_ns < old.observed_at_ns
        }) {
            return Err(Error::History);
        }
        self.wallet_alias = Some(observation.wallet_alias.clone());
        self.operational = Some(observation.clone());
        Ok(())
    }

    fn evaluate(&self, profile_id: [u8; 32], at: i64) -> Result<CertificationReport, Error> {
        let mut reasons = BTreeSet::new();
        let Some(contract) = &self.contract else {
            reasons.insert(CertificationReason::ContractMissing);
            return Ok(make_report(profile_id, at, None, reasons, self));
        };
        let kinds: BTreeSet<_> = self.fixtures.values().map(|value| value.kind).collect();
        for kind in FixtureKind::ALL {
            if !kinds.contains(&kind) {
                reasons.insert(CertificationReason::FixtureMissing(kind));
            }
        }
        if !self.dry_runs.values().any(|value| value.permitted) {
            reasons.insert(CertificationReason::BaselineDryRunMissing);
        }
        for denial in DryRunReason::REQUIRED_DENIALS {
            if !self
                .dry_runs
                .values()
                .any(|value| !value.permitted && value.reason == denial)
            {
                reasons.insert(CertificationReason::DenialDryRunMissing(denial));
            }
        }
        for region in &contract.required_regions {
            match self.eligibility.get(region) {
                None => {
                    reasons.insert(CertificationReason::EligibilityMissing(region.clone()));
                }
                Some(value) if !value.eligible => {
                    reasons.insert(CertificationReason::EligibilityBlocked(region.clone()));
                }
                Some(value) if stale(value.checked_at_ns, value.valid_until_ns, at, contract)? => {
                    reasons.insert(CertificationReason::EligibilityStale(region.clone()));
                }
                Some(_) => {}
            }
        }
        match &self.operational {
            None => {
                reasons.insert(CertificationReason::OperationalMissing);
            }
            Some(value) => {
                if stale(value.observed_at_ns, value.valid_until_ns, at, contract)? {
                    reasons.insert(CertificationReason::OperationalStale);
                }
                if value.collateral_micros < contract.minimum_collateral_micros {
                    reasons.insert(CertificationReason::CollateralInsufficient);
                }
                if value.allowance_micros < contract.required_allowance_micros {
                    reasons.insert(CertificationReason::AllowanceInsufficient);
                }
                if value.gas_micros < contract.minimum_gas_micros {
                    reasons.insert(CertificationReason::GasInsufficient);
                }
                if !value.relayer_available {
                    reasons.insert(CertificationReason::RelayerUnavailable);
                }
                if value.relayer_queue_depth > contract.max_relayer_queue_depth {
                    reasons.insert(CertificationReason::RelayerQueueExceeded);
                }
            }
        }
        for failure in FailureKind::ALL {
            if !self.failure_kinds.contains_key(&failure) {
                reasons.insert(CertificationReason::FailureSimulationMissing(failure));
            }
        }
        Ok(make_report(
            profile_id,
            at,
            Some(contract.contract_digest),
            reasons,
            self,
        ))
    }

    #[must_use]
    pub fn snapshot(&self) -> CertificationSnapshot {
        CertificationSnapshot {
            accepted_commands: self.accepted_commands,
            contract_digest: self.contract.as_ref().map(|value| value.contract_digest),
            fixture_count: self.fixtures.len(),
            fixture_kinds: self.fixtures.values().map(|value| value.kind).collect(),
            eligibility_count: self.eligibility.len(),
            dry_run_count: self.dry_runs.len(),
            failure_count: self.failures.len(),
            last_report: self.last_report.clone(),
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
        hasher.update(b"shadow-adapter-certification-state-v1");
        hash_json(&mut hasher, &self.contract);
        for (id, fixture) in &self.fixtures {
            hasher.update(&id.0);
            hash_json(&mut hasher, fixture);
        }
        for (sequence, id) in &self.fixture_sequences {
            hasher.update(&sequence.to_le_bytes());
            hasher.update(&id.0);
        }
        hash_json(&mut hasher, &self.last_fixture_sequence);
        hash_json(&mut hasher, &self.eligibility);
        hash_json(&mut hasher, &self.operational);
        hash_json(&mut hasher, &self.wallet_alias);
        for (id, result) in &self.dry_runs {
            hasher.update(&id.0);
            hash_json(&mut hasher, result);
        }
        for (id, result) in &self.failures {
            hasher.update(&id.0);
            hash_json(&mut hasher, result);
        }
        for (kind, id) in &self.failure_kinds {
            hash_json(&mut hasher, kind);
            hasher.update(&id.0);
        }
        for (id, report) in &self.reports {
            hasher.update(id);
            hash_json(&mut hasher, report);
        }
        for (id, (content, outcome)) in &self.processed {
            hasher.update(&id.0);
            hasher.update(content);
            hash_json(&mut hasher, outcome);
        }
        hash_json(&mut hasher, &self.accepted_commands);
        hash_json(&mut hasher, &self.last_recorded_at_ns);
        hash_json(&mut hasher, &self.last_report);
        hash_json(&mut hasher, &self.halted);
        *hasher.finalize().as_bytes()
    }

    fn halt<T>(&mut self, error: Error) -> Result<T, Error> {
        self.halted = Some(error.to_string());
        Err(error)
    }
}

fn validate_contract(contract: &AdapterContract) -> Result<(), Error> {
    let regions: BTreeSet<_> = contract.required_regions.iter().collect();
    if !contract.verify_digest()
        || contract.contract_id == [0; 32]
        || contract.rules_digest == [0; 32]
        || !valid_text(&contract.venue)
        || !valid_text(&contract.rest_host)
        || !valid_text(&contract.websocket_host)
        || !valid_text(&contract.exchange_contract)
        || !valid_text(&contract.settlement_contract)
        || contract.chain_id == 0
        || contract.schema_version == 0
        || contract.required_regions.is_empty()
        || contract.required_regions.len() > MAX_REGIONS
        || regions.len() != contract.required_regions.len()
        || contract
            .required_regions
            .iter()
            .any(|value| !valid_text(value))
        || contract.max_evidence_age_ns <= 0
        || contract.minimum_collateral_micros < 0
        || contract.required_allowance_micros < 0
        || contract.minimum_gas_micros < 0
        || contract.max_relayer_queue_depth == 0
    {
        Err(Error::Contract)
    } else {
        Ok(())
    }
}

fn dry_run(
    id: DryRunId,
    contract: &AdapterContract,
    policy: &SignerPolicyFrame,
    intent: &DryRunIntent,
) -> Result<DryRunResult, Error> {
    let reason = dry_run_reason(contract, policy, intent)?;
    let mut result = DryRunResult {
        dry_run_id: id,
        permitted: reason == DryRunReason::Permitted,
        reason,
        policy_digest: policy.digest(),
        intent_digest: *blake3::hash(
            &serde_json::to_vec(intent).map_err(|error| Error::Json(error.to_string()))?,
        )
        .as_bytes(),
        result_digest: [0; 32],
    };
    result.result_digest = dry_run_result_digest(&result);
    Ok(result)
}

fn dry_run_reason(
    contract: &AdapterContract,
    policy: &SignerPolicyFrame,
    intent: &DryRunIntent,
) -> Result<DryRunReason, Error> {
    let tokens: BTreeSet<_> = policy.allowed_tokens.iter().collect();
    if policy.policy_id == [0; 32]
        || !valid_text(&policy.venue)
        || !valid_text(&policy.exchange_contract)
        || policy.allowed_tokens.is_empty()
        || policy.allowed_tokens.len() > MAX_TOKENS
        || tokens.len() != policy.allowed_tokens.len()
        || policy.max_quantity_micros <= 0
        || !(0..=1_000_000).contains(&policy.max_price_micros)
        || policy.max_notional_micros <= 0
        || policy.valid_from_ns < 0
        || policy.valid_until_ns <= policy.valid_from_ns
        || !valid_text(&intent.venue)
        || !valid_text(&intent.exchange_contract)
        || intent.quantity_micros <= 0
        || !(0..=1_000_000).contains(&intent.price_micros)
        || intent.evaluated_at_ns < 0
    {
        return Ok(DryRunReason::PolicyInvalid);
    }
    if policy.venue != contract.venue {
        return Ok(DryRunReason::VenueForbidden);
    }
    if policy.exchange_contract != contract.exchange_contract {
        return Ok(DryRunReason::ContractForbidden);
    }
    if intent.evaluated_at_ns < policy.valid_from_ns
        || intent.evaluated_at_ns > policy.valid_until_ns
    {
        return Ok(DryRunReason::PolicyInactive);
    }
    if intent.venue != policy.venue {
        return Ok(DryRunReason::VenueForbidden);
    }
    if intent.exchange_contract != policy.exchange_contract {
        return Ok(DryRunReason::ContractForbidden);
    }
    if !tokens.contains(&intent.token) {
        return Ok(DryRunReason::TokenForbidden);
    }
    if intent.quantity_micros > policy.max_quantity_micros {
        return Ok(DryRunReason::QuantityLimit);
    }
    if intent.price_micros > policy.max_price_micros {
        return Ok(DryRunReason::PriceLimit);
    }
    let product = intent
        .quantity_micros
        .checked_mul(i128::from(intent.price_micros))
        .ok_or(Error::Overflow)?;
    let notional = product
        .checked_add(MICROS_PER_UNIT - 1)
        .ok_or(Error::Overflow)?
        / MICROS_PER_UNIT;
    if notional > policy.max_notional_micros {
        return Ok(DryRunReason::NotionalLimit);
    }
    if intent.maker && !policy.allow_maker {
        return Ok(DryRunReason::MakerForbidden);
    }
    if !intent.maker && !policy.allow_taker {
        return Ok(DryRunReason::TakerForbidden);
    }
    Ok(DryRunReason::Permitted)
}

const fn safe_action(kind: FailureKind) -> SafeAction {
    match kind {
        FailureKind::AllowanceInsufficient
        | FailureKind::GasInsufficient
        | FailureKind::RelayerUnavailable
        | FailureKind::EligibilityBlocked => SafeAction::DenyNewExposure,
        FailureKind::UnknownSubmission => SafeAction::RetainBackingAndReconcile,
        FailureKind::EngineRestarting | FailureKind::RateLimited => {
            SafeAction::BackoffWithoutAutomaticRetry
        }
        FailureKind::SettlementRetrying => SafeAction::RetainUnconfirmedValue,
    }
}

const fn fixture_action(kind: FixtureKind) -> FixtureAction {
    match kind {
        FixtureKind::Restart425 | FixtureKind::RateLimit429 => {
            FixtureAction::BackoffWithoutAutomaticRetry
        }
        FixtureKind::PostOnlyWindow => FixtureAction::MakerOnly,
        FixtureKind::CancelOnlyMode => FixtureAction::CancelOnly,
        FixtureKind::TakerDelay => FixtureAction::RetainBackingUntilDelayEnds,
        FixtureKind::TickSizeChange => FixtureAction::RevalidateOpenOrders,
        FixtureKind::UnknownOrder => FixtureAction::ReconcileUnknownOrder,
        FixtureKind::SettlementRetrying => FixtureAction::RetainUnconfirmedValue,
        FixtureKind::HeartbeatLost => FixtureAction::CancelOrdersAndDenyExposure,
    }
}

fn stale(
    observed: i64,
    valid_until: i64,
    at: i64,
    contract: &AdapterContract,
) -> Result<bool, Error> {
    if at < observed {
        return Err(Error::Timestamp);
    }
    Ok(at > valid_until || at - observed > contract.max_evidence_age_ns)
}

fn make_report(
    profile_id: [u8; 32],
    at: i64,
    contract_digest: Option<[u8; 32]>,
    reasons: BTreeSet<CertificationReason>,
    authority: &ShadowAdapterCertification,
) -> CertificationReport {
    let reasons: Vec<_> = reasons.into_iter().collect();
    let mut report = CertificationReport {
        profile_id,
        contract_digest,
        evaluated_at_ns: at,
        status: if reasons.is_empty() {
            CertificationStatus::Certified
        } else {
            CertificationStatus::NotCertified
        },
        reasons,
        fixture_count: authority.fixtures.len(),
        dry_run_count: authority.dry_runs.len(),
        failure_count: authority.failures.len(),
        authority_granted: false,
        report_digest: [0; 32],
    };
    report.report_digest = certification_report_digest(&report);
    report
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT_BYTES && !value.chars().any(char::is_control)
}

fn contract_digest(contract: &AdapterContract) -> [u8; 32] {
    let mut clone = contract.clone();
    clone.contract_digest = [0; 32];
    digest_json(b"shadow-adapter-contract-v1", &clone)
}

fn operational_digest(observation: &OperationalObservation) -> [u8; 32] {
    let mut clone = observation.clone();
    clone.observation_digest = [0; 32];
    digest_json(b"shadow-adapter-operational-v1", &clone)
}

fn dry_run_result_digest(result: &DryRunResult) -> [u8; 32] {
    let mut clone = result.clone();
    clone.result_digest = [0; 32];
    digest_json(b"shadow-adapter-dry-run-v1", &clone)
}

fn failure_result_digest(result: &FailureResult) -> [u8; 32] {
    let mut clone = result.clone();
    clone.result_digest = [0; 32];
    digest_json(b"shadow-adapter-failure-v1", &clone)
}

fn certification_report_digest(report: &CertificationReport) -> [u8; 32] {
    let mut clone = report.clone();
    clone.report_digest = [0; 32];
    digest_json(b"shadow-adapter-report-v1", &clone)
}

fn outcome_digest(outcome: &CertificationOutcome) -> [u8; 32] {
    let mut clone = outcome.clone();
    clone.outcome_digest = [0; 32];
    digest_json(b"shadow-adapter-outcome-v1", &clone)
}

fn digest_json<T: Serialize>(domain: &[u8], value: &T) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hash_json(&mut hasher, value);
    *hasher.finalize().as_bytes()
}

fn hash_json<T: Serialize>(hasher: &mut blake3::Hasher, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable state");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: CertificationCommand,
}

/// Encodes one bounded versioned certification command.
///
/// # Errors
///
/// Rejects serialization or size failures.
pub fn encode_command(command: &CertificationCommand) -> Result<Vec<u8>, Error> {
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

/// Decodes one bounded versioned certification command.
///
/// # Errors
///
/// Rejects malformed, oversized, trailing, or unsupported input.
pub fn decode_command(bytes: &[u8]) -> Result<CertificationCommand, Error> {
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
